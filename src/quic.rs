//! QUIC client and server on the UDP socket used for STUN and hole punching.
//!
//! Role `b` accepts one connection. Role `a` connects to `b`'s mapped address.
//! Both exchange one payload, wait for the controller, then exchange another on
//! the same connection after the TCP relay is gone.

use anyhow::{bail, Context, Result};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
const RELAY: &str = "198.18.0.1:9100";
const STUN_SERVER: &str = "198.18.0.1:9000";

pub fn run(role: &str) -> Result<()> {
    anyhow::ensure!(role == "a" || role == "b", "quic role must be a or b");
    let socket = UdpSocket::bind("0.0.0.0:10000")?;
    let mapped = learn_mapping(&socket)?;
    emit(&json!({"ready": true, "mapped": [mapped.ip().to_string(), mapped.port()]}))?;
    let peer = peer_mapping(role, mapped)?;
    punch(&socket, peer)?;
    // Both sides finish punching before either starts QUIC, then drop datagrams
    // still sitting on the socket so they are not parsed as QUIC packets.
    wait_for_peer(role, "punched")?;
    thread::sleep(Duration::from_millis(50));
    drain(&socket)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(session(role, socket, peer))
}

async fn session(role: &str, socket: UdpSocket, peer: SocketAddr) -> Result<()> {
    let runtime = Arc::new(quinn::TokioRuntime);
    let (endpoint, connection) = if role == "b" {
        let endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(server_config()?),
            socket,
            runtime,
        )?;
        let incoming = endpoint
            .accept()
            .await
            .context("no QUIC client connected")?;
        (endpoint, incoming.await?)
    } else {
        let mut endpoint =
            quinn::Endpoint::new(quinn::EndpointConfig::default(), None, socket, runtime)?;
        endpoint.set_default_client_config(client_config()?);
        let connection = endpoint.connect(peer, "localhost")?.await?;
        (endpoint, connection)
    };
    exchange(&connection, role, b"quic-before").await?;
    emit(&json!({"data_before": true}))?;
    wait_again()?;
    exchange(&connection, role, b"quic-after").await?;
    emit(&json!({"data_after": true}))?;
    // Keep the connection until the controller has read both results. Exiting
    // here would send CONNECTION_CLOSE before the peer finishes its read.
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(connection);
    endpoint.wait_idle().await;
    Ok(())
}

async fn exchange(connection: &quinn::Connection, role: &str, payload: &[u8]) -> Result<()> {
    if role == "a" {
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(payload).await?;
        send.finish()?;
        let got = recv.read_to_end(64).await?;
        anyhow::ensure!(got == payload, "peer echoed unexpected data");
    } else {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let got = recv.read_to_end(64).await?;
        anyhow::ensure!(got == payload, "peer sent unexpected data");
        send.write_all(&got).await?;
        send.finish()?;
        send.stopped().await?;
    }
    Ok(())
}

fn learn_mapping(socket: &UdpSocket) -> Result<SocketAddr> {
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut transaction = [0u8; 12];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut transaction)?;
    let request = crate::stun::binding_request(&transaction);
    let server: SocketAddr = STUN_SERVER.parse()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        socket.send_to(&request, server)?;
        match socket.recv_from(&mut buf) {
            Ok((length, _)) => {
                if let Some((ip, port)) = crate::stun::mapped_address(&buf[..length], &transaction)
                {
                    socket.set_read_timeout(None)?;
                    return Ok(SocketAddr::from((Ipv4Addr::from(ip), port)));
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
    }
    bail!("STUN Binding response did not arrive")
}

fn punch(socket: &UdpSocket, peer: SocketAddr) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        socket.send_to(b"punch", peer)?;
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn drain(socket: &UdpSocket) -> Result<()> {
    socket.set_nonblocking(true)?;
    let mut buf = [0u8; 1500];
    loop {
        match socket.recv_from(&mut buf) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.into()),
        }
    }
    socket.set_nonblocking(false)?;
    Ok(())
}

fn wait_for_peer(role: &str, token: &str) -> Result<()> {
    let other = if role == "a" { "b" } else { "a" };
    relay(&json!({"action": "put", "to": other, "data": token}))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let fetched = relay(&json!({"action": "fetch", "role": role}))?;
        if fetched
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(token)))
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!("peer did not finish punching")
}

fn peer_mapping(role: &str, mapped: SocketAddr) -> Result<SocketAddr> {
    let other = if role == "a" { "b" } else { "a" };
    relay(&json!({
        "action": "put",
        "to": other,
        "data": [mapped.ip().to_string(), mapped.port()],
    }))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let fetched = relay(&json!({"action": "fetch", "role": role}))?;
        if let Some(address) = fetched
            .as_array()
            .and_then(|items| items.iter().rev().find_map(socket_addr))
        {
            return Ok(address);
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!("peer mapping did not arrive")
}

fn socket_addr(value: &Value) -> Option<SocketAddr> {
    let ip: Ipv4Addr = value.get(0)?.as_str()?.parse().ok()?;
    let port = u16::try_from(value.get(1)?.as_u64()?).ok()?;
    Some(SocketAddr::from((ip, port)))
}

fn relay(request: &Value) -> Result<Value> {
    let mut stream = TcpStream::connect_timeout(&RELAY.parse()?, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    serde_json::to_writer(&mut stream, request)?;
    writeln!(stream)?;
    let mut line = String::new();
    BufReader::new(stream).take(4096).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

fn wait_again() -> Result<()> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let request: Value = serde_json::from_str(line.trim())?;
    anyhow::ensure!(request["action"] == "again", "unexpected quic command");
    Ok(())
}

fn emit(value: &Value) -> Result<()> {
    println!("{value}");
    std::io::stdout().flush()?;
    Ok(())
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn transport() -> Result<Arc<quinn::TransportConfig>> {
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(Duration::from_secs(10).try_into()?));
    Ok(Arc::new(transport))
}

fn server_config() -> Result<quinn::ServerConfig> {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let cert = certified.cert.der().clone();
    let key = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let mut crypto = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3 is unavailable")?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())?;
    crypto.alpn_protocols = vec![b"natbench".to_vec()];
    crypto.max_early_data_size = u32::MAX;
    let quic = quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(quic));
    config.transport = transport()?;
    Ok(config)
}

fn client_config() -> Result<quinn::ClientConfig> {
    let mut crypto = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3 is unavailable")?
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"natbench".to_vec()];
    let quic = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic));
    config.transport_config(transport()?);
    Ok(config)
}

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(provider()))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
