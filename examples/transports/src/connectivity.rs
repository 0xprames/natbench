//! Real iroh peers and a locally supervised relay, outside natbench's core.
use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use iroh::{endpoint::Connection, Endpoint, EndpointAddr, RelayMode};
use natbench_transport_protocol::{Request, Role, Workload, FRAME_HEADER, MAX_EVENT_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const ALPN: &[u8] = b"natbench/iroh-connectivity/1";
const RELAY_URL: &str = "https://198.18.0.1:8443/";
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Serve a real HTTPS relay and QUIC address discovery in the WAN namespace.
    Relay {
        #[arg(long)]
        certificate: PathBuf,
    },
    /// Run a peer behind router A or B using an isolated request.
    Peer {
        #[arg(long)]
        request: PathBuf,
    },
    /// Release the client's application exchange after the runner stopped the relay.
    Resume {
        #[arg(long)]
        request: PathBuf,
    },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, ValueEnum, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Policy {
    Automatic,
    RelayOnly,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ExpectedPath {
    Direct,
    Relay,
    Any,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    kind: String,
    run_id: String,
    role: Role,
    policy: Policy,
    expected_path: ExpectedPath,
    interrupt_relay: bool,
    peer_file: PathBuf,
    certificate_file: PathBuf,
    resume_file: PathBuf,
    deadline_ms: u64,
    workload: Workload,
}
impl Config {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.kind == "iroh_connectivity_request",
            "unsupported connectivity request"
        );
        ensure!(
            self.peer_file.is_absolute()
                && self.certificate_file.is_absolute()
                && self.resume_file.is_absolute(),
            "artifact paths must be absolute"
        );
        ensure!(
            self.peer_file != self.certificate_file
                && self.peer_file != self.resume_file
                && self.certificate_file != self.resume_file,
            "artifact paths must differ"
        );
        ensure!(
            self.policy != Policy::RelayOnly || self.expected_path != ExpectedPath::Direct,
            "relay-only cannot require a direct path"
        );
        ensure!(
            (1..=30_000).contains(&self.deadline_ms),
            "deadline_ms must be 1–30000"
        );
        self.frame_request().validate()
    }
    fn frame_request(&self) -> Request {
        // Only the shared byte-verification helpers use this request; no socket is
        // bound or dialed from these addresses. Connectivity uses EndpointAddr below.
        Request {
            schema_version: 1,
            kind: "transport_request".into(),
            run_id: self.run_id.clone(),
            role: self.role,
            listen_address: "0.0.0.0:9443".parse().unwrap(),
            peer_address: "198.18.0.20:9443".parse().unwrap(),
            peer_file: self.peer_file.clone(),
            deadline_ms: self.deadline_ms,
            workload: self.workload.clone(),
        }
    }
}
fn read_config(path: &Path) -> Result<Config> {
    let config: Config = serde_json::from_slice(&read_bounded(path)?)?;
    config.validate()?;
    Ok(config)
}
fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_EVENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_EVENT_BYTES, "input exceeds budget");
    Ok(bytes)
}
fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    // Requests, certificates, peer identities and controller signals have fresh paths.
    ensure!(!path.exists(), "artifact already exists");
    let temporary = path.with_extension("pending");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}
fn emit(config: Option<&Config>, event: &str, detail: Value) -> Result<()> {
    let value = json!({"schema_version":1,"kind":"iroh_connectivity_event",
        "run_id":config.map(|c|c.run_id.as_str()),"role":config.map(|c|c.role),
        "iroh_version":"1.3.0","relay_version":"1.3.0",
        "adapter_source_sha256":env!("ADAPTER_SOURCE_SHA256"),
        "rustc":env!("ADAPTER_RUSTC"),"build_profile":env!("ADAPTER_PROFILE"),
        "policy":config.map(|c|c.policy),"event":event,"detail":detail});
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &value)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
async fn relay(certificate: &Path) -> Result<()> {
    use iroh_relay::server::{
        CertConfig, QuicConfig, RelayConfig, Server, ServerConfig, TlsConfig,
    };
    let cert = rcgen::generate_simple_self_signed(vec!["198.18.0.1".into()])?;
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key.into())?;
    let mut http = RelayConfig::new("0.0.0.0:8080".parse::<std::net::SocketAddr>()?);
    http.tls = Some(TlsConfig::new(
        "0.0.0.0:8443".parse::<std::net::SocketAddr>()?,
        CertConfig::Manual {
            server_config: tls.clone(),
        },
    ));
    let mut quic = QuicConfig::new("0.0.0.0:7842".parse::<std::net::SocketAddr>()?);
    quic.server_config = Some(tls);
    let mut config = ServerConfig::default();
    config.relay = Some(http);
    config.quic = Some(quic);
    let mut server = Server::spawn(config).await?;
    publish(certificate, cert.cert.der())?;
    emit(
        None,
        "ready",
        json!({"relay_url":RELAY_URL,"quic_discovery_port":7842,"tls":"fresh pinned certificate"}),
    )?;
    server.join().await??;
    bail!("relay stopped unexpectedly")
}
async fn endpoint(config: &Config) -> Result<Endpoint> {
    let url = RELAY_URL.parse()?;
    let map = iroh_relay::RelayMap::from_iter([iroh_relay::RelayConfig::new(
        url,
        Some(iroh_relay::RelayQuicConfig::new(7842)),
    )]);
    let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .clear_address_lookup()
        .relay_mode(RelayMode::Custom(map))
        .ca_tls_config(iroh::tls::CaTlsConfig::custom_roots([read_bounded(
            &config.certificate_file,
        )?
        .into()]))
        .alpns(vec![ALPN.to_vec()]);
    if config.policy == Policy::Automatic {
        builder = builder.bind_addr("0.0.0.0:0")?;
    }
    let endpoint = builder.bind().await?;
    endpoint.online().await;
    Ok(endpoint)
}
fn snapshot(connection: &Connection) -> Value {
    let paths = connection.paths();
    json!({"source":"iroh Connection::paths","remote_id":connection.remote_id().to_string(),
        "paths":paths.iter().map(|p|json!({"id":format!("{:?}",p.id()),
            "kind":if p.is_ip(){"direct"}else if p.is_relay(){"relay"}else{"other"},
            "selected":p.is_selected(),"remote":format!("{:?}",p.remote_addr()),
            "local":format!("{:?}",p.local_addr())})).collect::<Vec<_>>()})
}
fn selected(connection: &Connection) -> Option<&'static str> {
    connection
        .paths()
        .iter()
        .find(|p| p.is_selected())
        .and_then(|p| {
            if p.is_ip() {
                Some("direct")
            } else if p.is_relay() {
                Some("relay")
            } else {
                None
            }
        })
}
async fn exchange(connection: &Connection, request: &Request, sequence: u32) -> Result<()> {
    let frame = natbench_transport_protocol::frame(request, sequence)?;
    let expected = natbench_transport_protocol::response(request, sequence, &frame)?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(&frame).await?;
    send.finish()?;
    ensure!(
        recv.read_to_end(expected.len()).await? == expected,
        "response identity, sequence or payload differs"
    );
    Ok(())
}
async fn server(config: &Config, phase: &mut &'static str) -> Result<()> {
    *phase = "relay_registration";
    let endpoint = endpoint(config).await?;
    // Bootstrap only the endpoint identity and local relay URL. Iroh discovers
    // public UDP addresses and performs its own disco/hole punching via the relay.
    publish(
        &config.peer_file,
        &serde_json::to_vec(
            &json!({"schema_version":1,"kind":"iroh_connectivity_peer", "run_id":config.run_id,
        "policy":config.policy,"endpoint_id":endpoint.id().to_string(),"relay_url":RELAY_URL}),
        )?,
    )?;
    emit(
        Some(config),
        "ready",
        json!({"endpoint_id":endpoint.id().to_string(),"bootstrap":"identity and relay URL only","address_lookup":"disabled"}),
    )?;
    *phase = "accept";
    let connection = endpoint.accept().await.context("endpoint closed")?.await?;
    emit(Some(config), "connected", snapshot(&connection))?;
    let request = config.frame_request();
    *phase = "receive";
    for sequence in 0..config.workload.exchanges() {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let data = recv
            .read_to_end(
                config
                    .workload
                    .bulk_bytes
                    .max(config.workload.payload_bytes) as usize
                    + FRAME_HEADER,
            )
            .await?;
        let response = natbench_transport_protocol::response(&request, sequence, &data)?;
        send.write_all(&response).await?;
        send.finish()?;
        emit(
            Some(config),
            "received",
            json!({"sequence":sequence,"verified_bytes":data.len()-FRAME_HEADER,"path_evidence":snapshot(&connection)}),
        )?;
    }
    connection.closed().await;
    // The runner owns shutdown; do not race its explicit stop step.
    std::future::pending::<()>().await;
    Ok(())
}
async fn client(config: &Config, phase: &mut &'static str) -> Result<()> {
    *phase = "bootstrap";
    let peer: Value = serde_json::from_slice(&read_bounded(&config.peer_file)?)?;
    ensure!(
        peer["schema_version"] == 1
            && peer["kind"] == "iroh_connectivity_peer"
            && peer["run_id"] == config.run_id
            && peer["policy"] == serde_json::to_value(config.policy)?
            && peer["relay_url"] == RELAY_URL,
        "bootstrap version/attempt/policy/relay mismatch"
    );
    let id = peer["endpoint_id"]
        .as_str()
        .context("missing endpoint ID")?
        .parse()?;
    *phase = "relay_registration";
    let endpoint = endpoint(config).await?;
    *phase = "connect";
    let started = Instant::now();
    let connection = endpoint
        .connect(
            EndpointAddr::new(id).with_relay_url(RELAY_URL.parse()?),
            ALPN,
        )
        .await?;
    emit(Some(config), "connected", snapshot(&connection))?;
    let request = config.frame_request();
    *phase = "first_data";
    exchange(&connection, &request, 0).await?;
    emit(
        Some(config),
        "verified",
        json!({"sequence":0,"seconds":started.elapsed().as_secs_f64(),"path_evidence":snapshot(&connection)}),
    )?;
    *phase = "select_path";
    let mut prior = Value::Null;
    let selection_started = Instant::now();
    loop {
        let evidence = snapshot(&connection);
        if evidence != prior {
            emit(Some(config), "path_observed", evidence.clone())?;
            prior = evidence;
        }
        let path = selected(&connection);
        if matches!(
            (config.expected_path, path),
            (ExpectedPath::Direct, Some("direct"))
                | (ExpectedPath::Relay, Some("relay"))
                | (ExpectedPath::Any, Some("direct"))
        ) || (config.expected_path == ExpectedPath::Any
            && path == Some("relay")
            && selection_started.elapsed() >= Duration::from_secs(2))
        {
            break;
        }
        tokio::select! {
            _ = connection.closed() => bail!("connection closed while selecting path"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
    emit(
        Some(config),
        "path_selected",
        json!({"path_evidence":snapshot(&connection),
        "selection_seconds":selection_started.elapsed().as_secs_f64(),
        "relay_observation_budget_seconds":if config.expected_path == ExpectedPath::Any{Some(2)}else{None}}),
    )?;
    if config.interrupt_relay {
        emit(
            Some(config),
            "before_relay_stop",
            json!({"verified_sequence":0,"path_evidence":snapshot(&connection)}),
        )?;
        *phase = "wait_controller";
        loop {
            if config.resume_file.exists() {
                let signal: Value = serde_json::from_slice(&read_bounded(&config.resume_file)?)?;
                ensure!(
                    signal["run_id"] == config.run_id && signal["relay_stopped"] == true,
                    "controller signal mismatch"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        emit(Some(config), "after_relay_stop", snapshot(&connection))?;
    }
    *phase = if config.interrupt_relay {
        "exchange_after_relay_stop"
    } else {
        "workload"
    };
    for sequence in 1..config.workload.exchanges() {
        let started = Instant::now();
        // Bound expected relay-outage failure independently from whole-peer supervision.
        tokio::time::timeout(
            Duration::from_secs(2),
            exchange(&connection, &request, sequence),
        )
        .await
        .context("no verified application response within 2 seconds")??;
        emit(
            Some(config),
            "verified",
            json!({"sequence":sequence,"seconds":started.elapsed().as_secs_f64(),"path_evidence":snapshot(&connection)}),
        )?;
    }
    let path = selected(&connection).context("no selected path after delivery")?;
    ensure!(
        config.expected_path != ExpectedPath::Direct || path == "direct",
        "direct path requirement was lost"
    );
    ensure!(
        config.expected_path != ExpectedPath::Relay || path == "relay",
        "relay path requirement was lost"
    );
    emit(
        Some(config),
        "completed",
        json!({"verified_exchanges":config.workload.exchanges(),"bulk_verified_bytes":config.workload.bulk_bytes,
        "selected_path":path,"same_connection":true,"relay_interrupted":config.interrupt_relay,"path_evidence":snapshot(&connection)}),
    )?;
    connection.close(0u32.into(), b"verified");
    endpoint.close().await;
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    match Cli::parse().command {
        Command::Relay { certificate } => relay(&certificate).await,
        Command::Resume { request } => {
            let config = read_config(&request)?;
            ensure!(
                config.role == Role::Client && config.interrupt_relay,
                "resume requires an interrupted client request"
            );
            publish(
                &config.resume_file,
                &serde_json::to_vec(&json!({"run_id":config.run_id,"relay_stopped":true}))?,
            )
        }
        Command::Peer { request } => {
            let config = read_config(&request)?;
            let mut phase = "bootstrap";
            let task = async {
                if config.role == Role::Server {
                    server(&config, &mut phase).await
                } else {
                    client(&config, &mut phase).await
                }
            };
            let result = if config.role == Role::Client {
                tokio::time::timeout(Duration::from_millis(config.deadline_ms), task)
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("peer deadline exceeded")))
            } else {
                task.await
            };
            if let Err(error) = result {
                emit(
                    Some(&config),
                    "failed",
                    json!({"phase":phase,"message":format!("{error:#}")}),
                )?;
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_inconsistent_policy_paths_versions_and_attempts() {
        let valid = json!({"schema_version":1,"kind":"iroh_connectivity_request",
            "run_id":"0123456789abcdef0123456789abcdef","role":"client",
            "policy":"automatic","expected_path":"direct","interrupt_relay":true,
            "peer_file":"/tmp/peer.json","certificate_file":"/tmp/relay.der",
            "resume_file":"/tmp/resume.json","deadline_ms":12000,
            "workload":{"payload_bytes":128,"warmup_messages":2,"measured_messages":10,"bulk_bytes":262144}});
        serde_json::from_value::<Config>(valid.clone())
            .unwrap()
            .validate()
            .unwrap();
        for (field, value) in [
            ("policy", json!("relay_only")),
            ("schema_version", json!(2)),
            ("run_id", json!("stale")),
            ("deadline_ms", json!(30001)),
            ("resume_file", json!("/tmp/peer.json")),
            ("certificate_file", json!("relative.der")),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                serde_json::from_value::<Config>(invalid)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{field}"
            );
        }
        let mut typo = valid;
        typo["hidden_punch"] = true.into();
        assert!(serde_json::from_value::<Config>(typo).is_err());
    }
}
