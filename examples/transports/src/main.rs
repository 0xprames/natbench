//! Independently built reliable-stream adapters.
use anyhow::Result;
use clap::Parser;
use natbench_transport_adapters::{client, emit, server, Transport};
use natbench_transport_protocol::{EventData, Request, Role};
use std::{fs, path::PathBuf, time::Duration};
#[derive(Parser)]
#[command(name = "natbench-transport-adapters", version)]
struct Cli {
    #[arg(long, value_enum)]
    transport: Transport,
    #[arg(long)]
    request: PathBuf,
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
