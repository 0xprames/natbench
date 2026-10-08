//! Executable contract/lifecycle wrapper. Replace transport.rs with your library integration.
mod transport;
use anyhow::{ensure, Result};
use clap::Parser;
use natbench_transport_protocol::{
    Bootstrap, Event, EventData, Implementation, Measurement, Request, Role, MAX_EVENT_BYTES,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(name=env!("CARGO_PKG_NAME"),version)]
struct Cli {
    #[arg(long)]
    request: PathBuf,
    /// Demonstrate valid unsupported-capability reporting; no workload is run.
    #[arg(long)]
    unsupported: bool,
}
fn implementation(unsupported: bool) -> Result<Implementation> {
    let (kernel, mut settings) = transport::settings()?;
    settings.extend(BTreeMap::from([
        ("transport_engine_version".into(), kernel),
        ("rustc".into(), env!("ADAPTER_RUSTC").into()),
        ("build_profile".into(), env!("ADAPTER_PROFILE").into()),
        (
            "adapter_source_sha256".into(),
            env!("ADAPTER_SOURCE_SHA256").into(),
        ),
        (
            "capability_override".into(),
            if unsupported {
                "unsupported demonstration"
            } else {
                "none"
            }
            .into(),
        ),
        ("relay".into(), "disabled".into()),
        (
            "address_lookup".into(),
            "controller-provided peer; no discovery".into(),
        ),
    ]));
    Ok(Implementation {
        name: env!("CARGO_PKG_NAME").into(),
        version: env!("CARGO_PKG_VERSION").into(),
        settings,
    })
}
fn emit(request: &Request, info: &Implementation, data: EventData) -> Result<()> {
    let event = Event::new(request, info.clone(), data);
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
fn publish(
    request: &Request,
    info: &Implementation,
    details: serde_json::Value,
    unsupported: bool,
) -> Result<()> {
    ensure!(!request.peer_file.exists(), "peer file already exists");
    let bootstrap = Bootstrap {
        schema_version: 1,
        kind: "transport_peer".into(),
        run_id: request.run_id.clone(),
        implementation: info.clone(),
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
        info,
        EventData::Ready {
            capabilities: if unsupported {
                vec![]
            } else {
                vec!["direct_reliable_stream".into()]
            },
        },
    )
}
fn peer(request: &Request, info: &Implementation) -> Result<Bootstrap> {
    let mut bytes = Vec::new();
    File::open(&request.peer_file)?
        .take((MAX_EVENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_EVENT_BYTES,
        "peer exceeds contract limit"
    );
    let peer: Bootstrap = serde_json::from_slice(&bytes)?;
    ensure!(
        peer.schema_version == 1
            && peer.kind == "transport_peer"
            && peer.run_id == request.run_id
            && peer.implementation == *info,
        "peer identity/version/settings mismatch"
    );
    ensure!(
        peer.details["address"] == request.peer_address.to_string(),
        "peer address differs from topology"
    );
    Ok(peer)
}
async fn client(
    request: &Request,
    info: &Implementation,
    phase: &mut &'static str,
) -> Result<Measurement> {
    let peer = peer(request, info)?;
    *phase = "connect";
    let start = Instant::now();
    let mut connection = transport::connect(request).await?;
    *phase = "first_data";
    let first = natbench_transport_protocol::frame(request, 0)?;
    ensure!(
        connection.exchange(&first, first.len()).await? == first,
        "first frame was not echoed intact"
    );
    let first_data_seconds = start.elapsed().as_secs_f64();
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
        // Construction precedes timing; receiver verification and ACK are inside.
        let start = Instant::now();
        let got = connection.exchange(&frame, expected.len()).await?;
        ensure!(
            got == expected,
            "response identity, sequence or bytes differ"
        );
        let elapsed = start.elapsed().as_secs_f64();
        if bulk {
            bulk_seconds = elapsed;
        } else if !warmup {
            samples.push(elapsed);
        }
    }
    let measurement = Measurement {
        workload: request.workload.clone(),
        path: "direct".into(),
        path_evidence: json!({"source":"TCP socket snapshots","client":connection.evidence()?,"server_listener":peer.details["socket"]}),
        first_data_seconds,
        message_rtt_seconds: samples,
        bulk_verified_bytes: request.workload.bulk_bytes,
        bulk_seconds,
    };
    measurement.validate(request)?;
    *phase = "close";
    connection.close().await?;
    Ok(measurement)
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut bytes = Vec::new();
    File::open(cli.request)?
        .take((MAX_EVENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_EVENT_BYTES,
        "request exceeds contract limit"
    );
    let request: Request = serde_json::from_slice(&bytes)?;
    request.validate()?;
    let info = implementation(cli.unsupported)?;
    if cli.unsupported {
        if request.role == Role::Server {
            publish(
                &request,
                &info,
                json!({"address":request.peer_address.to_string()}),
                true,
            )?;
            std::future::pending::<()>().await;
        }
        emit(
            &request,
            &info,
            EventData::Unsupported {
                reason: "direct reliable stream disabled by explicit demonstration flag".into(),
            },
        )?;
        std::process::exit(3);
    }
    let mut phase = "bootstrap";
    let result = if request.role == Role::Server {
        phase = "serve";
        transport::serve(&request, |details| publish(&request, &info, details, false))
            .await
            .map(|_| None)
    } else {
        match tokio::time::timeout(
            Duration::from_millis(request.deadline_ms),
            client(&request, &info, &mut phase),
        )
        .await
        {
            Ok(result) => result.map(Some),
            Err(_) => Err(anyhow::anyhow!("application deadline exceeded")),
        }
    };
    match result {
        Ok(Some(measurement)) => emit(&request, &info, EventData::Completed { measurement }),
        Ok(None) => Ok(()),
        Err(error) => {
            emit(
                &request,
                &info,
                EventData::Failed {
                    phase: phase.into(),
                    message: format!("{error:#}"),
                },
            )?;
            std::process::exit(1)
        }
    }
}
