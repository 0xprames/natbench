//! Same-connection recovery, with fresh verified bytes sent after restoration.
use anyhow::{ensure, Context, Result};
use clap::Parser;
use natbench_transport_adapters::{connect, implementation, peer, server, Transport};
use natbench_transport_protocol::{Request, Role, Workload};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

#[derive(Parser)]
#[command(name = "natbench-transport-recovery", version)]
struct Cli {
    #[arg(long, value_enum)]
    transport: Transport,
    #[arg(long, value_parser = ["server", "client"])]
    role: String,
    #[arg(long, default_value_t = 10000, value_parser = clap::value_parser!(u64).range(1..=60000))]
    deadline_ms: u64,
}
fn monotonic_ns() -> Result<u64> {
    // SAFETY: clock_gettime writes one initialized timespec. Only network
    // namespaces are entered, so the controller and child share this clock.
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } == 0,
        "read monotonic clock"
    );
    let nanos = u64::try_from(time.tv_nsec)?;
    ensure!(nanos < 1_000_000_000, "invalid clock nanoseconds");
    u64::try_from(time.tv_sec)?
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(nanos))
        .context("monotonic overflow")
}
fn event(request: &Request, transport: Transport, name: &str, details: Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(
        &mut out,
        &json!({"schema_version":1,"kind":"transport_recovery_event","run_id":request.run_id,"implementation":implementation(transport)?,"event":name,"details":details}),
    )?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
fn read_marker(path: &Path, id: &str, run_id: &str) -> Result<u64> {
    let mut data = Vec::new();
    File::open(path)?.take(1_048_577).read_to_end(&mut data)?;
    ensure!(data.len() <= 1_048_576, "transition marker exceeds limit");
    let value: Value = serde_json::from_slice(&data)?;
    ensure!(
        value["schema_version"] == 1
            && value["kind"] == "network_transition"
            && value["id"] == id
            && value["run_id"] == run_id
            && value["complete"] == true
            && value["error"].is_null(),
        "invalid or stale network marker"
    );
    let started = value["started_monotonic_ns"]
        .as_u64()
        .context("missing transition start")?;
    let applied = value["applied_monotonic_ns"]
        .as_u64()
        .context("missing transition completion")?;
    ensure!(
        started <= applied && applied <= monotonic_ns()?,
        "invalid transition clock"
    );
    let links = value["links"]
        .as_array()
        .context("missing transition links")?;
    ensure!(
        !links.is_empty()
            && links
                .iter()
                .all(|link| link["applied"] == true && link["after"].is_array()),
        "transition was not fully applied"
    );
    Ok(applied)
}
async fn marker(request: &Request, id: &str) -> Result<u64> {
    let path = request
        .peer_file
        .parent()
        .unwrap()
        .join(format!("{id}.json"));
    loop {
        match std::fs::metadata(&path) {
            Ok(_) => return read_marker(&path, id, &request.run_id),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            Err(error) => return Err(error.into()),
        }
    }
}
async fn client(request: &Request, transport: Transport, phase: &mut &'static str) -> Result<()> {
    *phase = "connect";
    let mut connection = connect(request, transport, &peer(request, transport)?).await?;
    let before = connection.evidence()?;
    *phase = "initial_data";
    let first = natbench_transport_protocol::frame(request, 0)?;
    ensure!(
        connection.exchange(&first, first.len()).await? == first,
        "initial response differs"
    );
    event(
        request,
        transport,
        "initial_delivered",
        json!({"sequence":0,"payload_verified_bytes":request.workload.payload_bytes,"path_evidence":before}),
    )?;
    *phase = "await_outage";
    let outage = marker(request, "outage").await?;
    *phase = "outage_probe";
    let probe = natbench_transport_protocol::frame(request, 1)?;
    event(
        request,
        transport,
        "probe_started",
        json!({"sequence":1,"outage_applied_monotonic_ns":outage}),
    )?;
    ensure!(
        connection.exchange(&probe, probe.len()).await? == probe,
        "outage response differs"
    );
    // This request was queued during the outage. Its eventual response is
    // intentionally excluded from the recovery measurement.
    *phase = "await_restoration";
    let restored = marker(request, "restored").await?;
    *phase = "fresh_post_event_data";
    let fresh = natbench_transport_protocol::frame(request, 2)?;
    let sent = monotonic_ns()?;
    ensure!(sent >= restored, "fresh request predates restoration");
    ensure!(
        connection.exchange(&fresh, fresh.len()).await? == fresh,
        "fresh response differs"
    );
    let delivered = monotonic_ns()?;
    *phase = "fresh_post_event_bulk";
    let bulk = natbench_transport_protocol::frame(request, 3)?;
    let expected = natbench_transport_protocol::response(request, 3, &bulk)?;
    ensure!(
        connection.exchange(&bulk, expected.len()).await? == expected,
        "receiver bulk acknowledgement differs"
    );
    let after = connection.evidence()?;
    event(
        request,
        transport,
        "recovered",
        json!({
            "outage_applied_monotonic_ns":outage,"restored_applied_monotonic_ns":restored,
            "fresh_sent_monotonic_ns":sent,"fresh_delivered_monotonic_ns":delivered,
            "event_to_fresh_delivery_seconds":(delivered-restored) as f64 / 1e9,
            "fresh_sequence":2,"bulk_sequence":3,"payload_verified_bytes":request.workload.payload_bytes,
            "bulk_verified_bytes":request.workload.bulk_bytes,"existing_connection_survived":true,
            "connection_attempts":1,"path":"direct","path_before":before,"path_after":after
        }),
    )?;
    *phase = "close";
    connection.close().await
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::fs::canonicalize(
        std::env::var_os("NATBENCH_CASE_ARTIFACTS")
            .context("application schema 5 case directory required")?,
    )?;
    let request = Request {
        schema_version: 1,
        kind: "transport_request".into(),
        run_id: std::env::var("NATBENCH_RUN_ID").context("application schema 5 run ID required")?,
        role: if cli.role == "server" {
            Role::Server
        } else {
            Role::Client
        },
        listen_address: if cli.role == "server" {
            "0.0.0.0:9443"
        } else {
            "0.0.0.0:0"
        }
        .parse()?,
        peer_address: "198.18.0.1:9443".parse()?,
        peer_file: dir.join("peer.json"),
        deadline_ms: cli.deadline_ms,
        workload: Workload {
            payload_bytes: 128,
            warmup_messages: 0,
            measured_messages: 2,
            bulk_bytes: 16384,
        },
    };
    request.validate()?;
    let mut phase = "serve";
    let result = if request.role == Role::Server {
        server(&request, cli.transport).await
    } else {
        match tokio::time::timeout(
            Duration::from_millis(cli.deadline_ms),
            client(&request, cli.transport, &mut phase),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("application deadline exceeded")),
        }
    };
    if let Err(error) = result {
        event(
            &request,
            cli.transport,
            "failed",
            json!({"phase":phase,"message":format!("{error:#}"),"deadline_ms":cli.deadline_ms}),
        )?;
        std::process::exit(1);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_partial_and_future_markers_are_rejected() {
        let path =
            std::env::temp_dir().join(format!("natbench-marker-{}.json", std::process::id()));
        let mut value = json!({"schema_version":1,"kind":"network_transition","id":"restored","run_id":"a".repeat(32),"complete":true,"error":null,"started_monotonic_ns":1,"applied_monotonic_ns":2,"links":[{"applied":true,"after":[]}]});
        std::fs::write(&path, value.to_string()).unwrap();
        assert_eq!(read_marker(&path, "restored", &"a".repeat(32)).unwrap(), 2);
        assert!(read_marker(&path, "restored", &"b".repeat(32)).is_err());
        value["complete"] = false.into();
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(read_marker(&path, "restored", &"a".repeat(32)).is_err());
        value["complete"] = true.into();
        value["applied_monotonic_ns"] = u64::MAX.into();
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(read_marker(&path, "restored", &"a".repeat(32)).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
