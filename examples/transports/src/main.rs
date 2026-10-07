//! Independently built adapters; the controller supplies no transport library or socket.
use anyhow::{ensure, Context, Result};
use clap::{Parser, ValueEnum};
use natbench_transport_protocol::{
    Bootstrap, Event, EventData, Implementation, Measurement, Request, Role, FRAME_HEADER,
    MAX_EVENT_BYTES,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

const ALPN: &[u8] = b"natbench/direct-stream/1";
#[derive(Clone, Copy, ValueEnum)]
enum Transport {
    Iroh,
    Quinn,
}
#[derive(Parser)]
struct Cli {
    #[arg(long, value_enum)]
    transport: Transport,
    #[arg(long)]
    request: PathBuf,
}
fn implementation(transport: Transport) -> Implementation {
    let (name, version, engine, authentication) = match transport {
        Transport::Iroh => ("iroh", "1.3.0", "noq", "mutual endpoint identity"),
        Transport::Quinn => ("quinn", "0.11.12", "quinn", "pinned server certificate"),
    };
    Implementation {
        name: name.into(),
        version: version.into(),
        settings: BTreeMap::from([
            ("quic_engine".into(), engine.into()),
            (
                "quic_engine_version".into(),
                match transport {
                    Transport::Iroh => env!("ADAPTER_NOQ"),
                    Transport::Quinn => "0.11.12",
                }
                .into(),
            ),
            ("authentication".into(), authentication.into()),
            ("crypto_provider".into(), "ring".into()),
            ("transport_settings".into(), "library defaults".into()),
            ("session".into(), "fresh endpoint; no resumption".into()),
            ("relay".into(), "disabled".into()),
            (
                "address_lookup".into(),
                "controller-provided peer; no discovery".into(),
            ),
            ("adapter_version".into(), env!("CARGO_PKG_VERSION").into()),
            ("rustc".into(), env!("ADAPTER_RUSTC").into()),
            ("build_profile".into(), env!("ADAPTER_PROFILE").into()),
            (
                "adapter_source_sha256".into(),
                env!("ADAPTER_SOURCE_SHA256").into(),
            ),
        ]),
    }
}
fn emit(request: &Request, transport: Transport, data: EventData) -> Result<()> {
    let event = Event::new(request, implementation(transport), data);
    event.validate(request)?;
    let bytes = serde_json::to_vec(&event)?;
    ensure!(
        bytes.len() <= MAX_EVENT_BYTES,
        "event exceeds contract limit"
    );
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
fn publish_peer(request: &Request, transport: Transport, details: serde_json::Value) -> Result<()> {
    ensure!(!request.peer_file.exists(), "peer file already exists");
    let bootstrap = Bootstrap {
        schema_version: 1,
        kind: "transport_peer".into(),
        run_id: request.run_id.clone(),
        implementation: implementation(transport),
        details,
    };
    let temporary = request.peer_file.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer(&mut file, &bootstrap)?;
    file.sync_all()?;
    fs::rename(temporary, &request.peer_file)?;
    emit(
        request,
        transport,
        EventData::Ready {
            capabilities: vec!["direct_reliable_stream".into()],
        },
    )
}
fn peer(request: &Request, transport: Transport) -> Result<Bootstrap> {
    let mut bytes = Vec::new();
    File::open(&request.peer_file)?
        .take((MAX_EVENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_EVENT_BYTES,
        "peer file exceeds contract limit"
    );
    let peer: Bootstrap = serde_json::from_slice(&bytes)?;
    ensure!(
        peer.schema_version == 1
            && peer.kind == "transport_peer"
            && peer.run_id == request.run_id
            && peer.implementation == implementation(transport),
        "peer bootstrap identity/version/settings mismatch"
    );
    ensure!(
        peer.details["address"] == request.peer_address.to_string(),
        "peer address differs from requested topology"
    );
    Ok(peer)
}
async fn iroh_endpoint(request: &Request) -> Result<iroh::Endpoint> {
    Ok(iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .bind_addr(request.listen_address)?
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?)
}

enum Connection {
    Iroh {
        endpoint: iroh::Endpoint,
        connection: iroh::endpoint::Connection,
    },
    Quinn {
        endpoint: quinn::Endpoint,
        connection: quinn::Connection,
    },
}
impl Connection {
    async fn exchange(&self, bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
        match self {
            Self::Iroh { connection, .. } => {
                let (mut send, mut recv) = connection.open_bi().await?;
                send.write_all(bytes).await?;
                send.finish()?;
                Ok(recv.read_to_end(limit).await?)
            }
            Self::Quinn { connection, .. } => {
                let (mut send, mut recv) = connection.open_bi().await?;
                send.write_all(bytes).await?;
                send.finish()?;
                Ok(recv.read_to_end(limit).await?)
            }
        }
    }
    fn evidence(&self) -> serde_json::Value {
        match self {
            Self::Iroh { connection, .. } => {
                json!({"source":"iroh paths snapshot", "paths":format!("{:?}", connection.paths()), "remote_id":connection.remote_id().to_string(), "configured_transports":["ipv4_udp"]})
            }
            Self::Quinn {
                endpoint,
                connection,
            } => {
                json!({"source":"quinn connection addresses", "local":endpoint.local_addr().ok().map(|a|a.to_string()), "remote":connection.remote_address().to_string()})
            }
        }
    }
    async fn close(self) {
        match self {
            Self::Iroh {
                endpoint,
                connection,
            } => {
                connection.close(0u32.into(), b"verified");
                endpoint.close().await;
            }
            Self::Quinn {
                endpoint,
                connection,
            } => {
                connection.close(0u32.into(), b"verified");
                endpoint.wait_idle().await;
            }
        }
    }
}
async fn connect(request: &Request, transport: Transport, peer: &Bootstrap) -> Result<Connection> {
    match transport {
        Transport::Iroh => {
            let id = peer.details["endpoint_id"]
                .as_str()
                .context("missing endpoint_id")?
                .parse()?;
            let endpoint = iroh_endpoint(request).await?;
            let address = iroh::EndpointAddr::new(id).with_ip_addr(request.peer_address);
            let connection = endpoint.connect(address, ALPN).await?;
            Ok(Connection::Iroh {
                endpoint,
                connection,
            })
        }
        Transport::Quinn => {
            let cert: Vec<u8> = serde_json::from_value(peer.details["certificate"].clone())?;
            let mut roots = rustls::RootCertStore::empty();
            roots.add(cert.into())?;
            let mut crypto = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            crypto.alpn_protocols = vec![ALPN.to_vec()];
            let config = quinn::ClientConfig::new(Arc::new(
                quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
            ));
            let mut endpoint = quinn::Endpoint::client(request.listen_address)?;
            endpoint.set_default_client_config(config);
            let connection = endpoint
                .connect(request.peer_address, "natbench.local")?
                .await?;
            Ok(Connection::Quinn {
                endpoint,
                connection,
            })
        }
    }
}
async fn client(
    request: &Request,
    transport: Transport,
    phase: &mut &'static str,
) -> Result<Measurement> {
    let peer = peer(request, transport)?;
    *phase = "connect";
    let started = Instant::now();
    let connection = connect(request, transport, &peer).await?;
    *phase = "first_data";
    let first = natbench_transport_protocol::frame(request, 0)?;
    ensure!(
        connection.exchange(&first, first.len()).await? == first,
        "first message was not echoed intact"
    );
    let first_data_seconds = started.elapsed().as_secs_f64();
    let mut samples = Vec::new();
    let mut bulk_seconds = 0.;
    for sequence in 1..request.workload.exchanges() {
        let bulk = sequence + 1 == request.workload.exchanges();
        let warmup = sequence <= request.workload.warmup_messages;
        *phase = if bulk {
            "bulk"
        } else if warmup {
            "warmup"
        } else {
            "messages"
        };
        let frame = natbench_transport_protocol::frame(request, sequence)?;
        let expected = natbench_transport_protocol::response(request, sequence, &frame)?;
        // Payload construction is outside the measured interval; receiver verification is inside.
        let started = Instant::now();
        let got = connection.exchange(&frame, expected.len()).await?;
        ensure!(
            got == expected,
            "application response identity, sequence or bytes differ"
        );
        let elapsed = started.elapsed().as_secs_f64();
        if bulk {
            bulk_seconds = elapsed;
        } else if !warmup {
            samples.push(elapsed);
        }
    }
    let measurement = Measurement {
        workload: request.workload.clone(),
        path: "direct".into(),
        path_evidence: connection.evidence(),
        first_data_seconds,
        message_rtt_seconds: samples,
        bulk_verified_bytes: request.workload.bulk_bytes,
        bulk_seconds,
    };
    measurement.validate(request)?;
    *phase = "close";
    connection.close().await;
    Ok(measurement)
}
async fn server(request: &Request, transport: Transport) -> Result<()> {
    let limit = request
        .workload
        .bulk_bytes
        .max(request.workload.payload_bytes) as usize
        + FRAME_HEADER;
    match transport {
        Transport::Iroh => {
            let endpoint = iroh_endpoint(request).await?;
            publish_peer(
                request,
                transport,
                json!({"address":request.peer_address.to_string(),"endpoint_id":endpoint.id().to_string()}),
            )?;
            let connection = endpoint.accept().await.context("listener closed")?.await?;
            for sequence in 0..request.workload.exchanges() {
                let (mut send, mut recv) = connection.accept_bi().await?;
                let data = recv.read_to_end(limit).await?;
                let response = natbench_transport_protocol::response(request, sequence, &data)?;
                send.write_all(&response).await?;
                send.finish()?;
                // Keep serving after queuing FIN; waiting for its transport ACK
                // would insert delayed-ACK time into the next application exchange.
            }
            connection.closed().await;
            std::future::pending::<()>().await;
        }
        Transport::Quinn => {
            let cert = rcgen::generate_simple_self_signed(vec!["natbench.local".into()])?;
            let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
            let mut crypto = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert.cert.der().clone()], key.into())?;
            crypto.alpn_protocols = vec![ALPN.to_vec()];
            let config = quinn::ServerConfig::with_crypto(Arc::new(
                quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?,
            ));
            let endpoint = quinn::Endpoint::server(config, request.listen_address)?;
            publish_peer(
                request,
                transport,
                json!({"address":request.peer_address.to_string(),"certificate":cert.cert.der().to_vec()}),
            )?;
            let connection = endpoint.accept().await.context("listener closed")?.await?;
            for sequence in 0..request.workload.exchanges() {
                let (mut send, mut recv) = connection.accept_bi().await?;
                let data = recv.read_to_end(limit).await?;
                let response = natbench_transport_protocol::response(request, sequence, &data)?;
                send.write_all(&response).await?;
                send.finish()?;
                // Keep serving after queuing FIN; waiting for its transport ACK
                // would insert delayed-ACK time into the next application exchange.
            }
            connection.closed().await;
            std::future::pending::<()>().await;
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let request: Request = serde_json::from_slice(&fs::read(cli.request)?)?;
    request.validate()?;
    let mut phase = "bootstrap";
    let result = if request.role == Role::Server {
        phase = "serve";
        server(&request, cli.transport).await.map(|_| None)
    } else {
        match tokio::time::timeout(
            Duration::from_millis(request.deadline_ms),
            client(&request, cli.transport, &mut phase),
        )
        .await
        {
            Ok(result) => result.map(Some),
            Err(_) => Err(anyhow::anyhow!("application deadline exceeded")),
        }
    };
    match result {
        Ok(Some(measurement)) => emit(
            &request,
            cli.transport,
            EventData::Completed { measurement },
        ),
        Ok(None) => Ok(()),
        Err(error) => {
            emit(
                &request,
                cli.transport,
                EventData::Failed {
                    phase: phase.into(),
                    message: format!("{error:#}"),
                },
            )?;
            std::process::exit(1)
        }
    }
}
