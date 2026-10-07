use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use natbench::{
    bench::{self, Options},
    lab::{Lab, Profile, RouterInput},
};
use std::{path::PathBuf, sync::atomic::Ordering};

#[derive(Parser)]
#[command(version, about = "Measure NAT behavior in isolated Linux networks")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Args)]
struct Network {
    #[arg(long, value_enum, default_value = "preserve")]
    a: Profile,
    #[arg(long, value_enum, default_value = "preserve")]
    b: Profile,
    #[arg(long, value_enum, default_value = "drop")]
    router_input: RouterInput,
}
#[derive(Subcommand)]
enum Action {
    /// Explain prerequisites; --probe verifies kernel features in disposable namespaces.
    Doctor {
        #[arg(long)]
        probe: bool,
        #[arg(long)]
        json: bool,
    },
    #[command(name = "__nfqueue-probe", hide = true)]
    NfqueueProbe { namespace: String },
    #[command(name = "__tcp-ready", hide = true)]
    TcpReady { address: std::net::SocketAddr },
    /// Execute a built-in or application scenario with JSON/JUnit artifacts.
    Test {
        scenario: PathBuf,
        #[arg(long)]
        artifacts: PathBuf,
        /// Capture application namespaces; optional =N bounds packets per role (default 1000).
        #[arg(long, num_args=0..=1, require_equals=true, default_missing_value="1000", value_parser=clap::value_parser!(u32).range(1..=100_000))]
        capture: Option<u32>,
    },
    /// Repeat a frozen scenario with fresh fixtures, verdict counts and timing summaries.
    Repeat {
        scenario: PathBuf,
        #[arg(long)]
        artifacts: PathBuf,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=100))]
        runs: u32,
        /// Capture each application attempt; optional =N bounds packets per role (default 1000).
        #[arg(long, num_args=0..=1, require_equals=true, default_missing_value="1000", value_parser=clap::value_parser!(u32).range(1..=100_000))]
        capture: Option<u32>,
    },
    Bench {
        #[command(flatten)]
        network: Network,
        #[arg(long, default_value_t = 2.)]
        timeout: f64,
    },
    Matrix {
        #[arg(long, value_enum, default_value = "drop")]
        router_input: RouterInput,
        #[arg(long, default_value_t = 2.)]
        timeout: f64,
    },
    Run {
        #[command(flatten)]
        network: Network,
        #[arg(long, default_value = "a", value_parser = ["a", "b", "ra", "rb", "wan"])]
        role: String,
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    #[command(name = "__endpoint", hide = true)]
    Endpoint { binds: String },
    #[command(name = "__stun", hide = true)]
    StunServer { binds: String },
    #[command(name = "__relay", hide = true)]
    Relay,
    /// Measure how long a UDP mapping survives, and which packets refresh it.
    Lifetime {
        #[arg(long, value_enum, default_value = "preserve")]
        profile: Profile,
        #[arg(long, value_enum, default_value = "drop")]
        router_input: RouterInput,
        #[arg(long, default_value_t = 3)]
        udp_timeout: u64,
    },
    /// See whether a second flow can take a source port the first flow already holds.
    Collision {
        #[arg(long, value_enum, default_value = "preserve")]
        profile: Profile,
        #[arg(long, value_enum, default_value = "drop")]
        router_input: RouterInput,
    },
    /// See whether two clients on one LAN can reach each other through their public mappings.
    Hairpin {
        #[arg(long, value_enum, default_value = "preserve")]
        profile: Profile,
        #[arg(long, value_enum, default_value = "drop")]
        router_input: RouterInput,
    },
    /// Run one preserve/preserve traversal after delaying or dropping WAN packets.
    Impair {
        #[arg(long, default_value_t = 20)]
        delay_ms: u64,
        #[arg(long, default_value_t = 0)]
        loss_percent: u64,
        #[arg(long, default_value_t = 2.)]
        timeout: f64,
    },
    /// Run one preserve/preserve measurement with a second NAT in front of client A.
    Nested {
        #[arg(long, default_value_t = 2.)]
        timeout: f64,
    },
    /// Measure preserve/preserve peers through a userspace UDP translator.
    Translate {
        /// Program invoked as `nat --lan-interface lan --wan-interface wan` in each router.
        /// Defaults to this binary, which keeps the source port and filters endpoint-independently.
        #[arg(long)]
        translator: Option<PathBuf>,
        #[arg(long, default_value_t = 2.)]
        timeout: f64,
    },
    /// Translate forwarded UDP from NFQUEUE 42. External translators use this same command.
    Nat {
        #[arg(long)]
        lan_interface: String,
        #[arg(long)]
        wan_interface: String,
    },
    /// Learn mapped addresses with STUN Binding requests on the traversal socket.
    Stun {
        #[arg(long, default_value_t = 2.)]
        timeout: f64,
    },
    /// Handshake QUIC on the STUN socket and exchange data again after the relay stops.
    Quic {
        #[arg(long, default_value_t = 4.)]
        timeout: f64,
    },
    #[command(name = "__quic", hide = true)]
    QuicWorker {
        #[arg(value_parser = ["a", "b"])]
        role: String,
    },
    /// Time one UDP transfer on the punched preserve/preserve path.
    Throughput {
        #[arg(long, default_value_t = 1_048_576)]
        bytes: u64,
        #[arg(long, default_value_t = 1200)]
        chunk: u64,
        #[arg(long, default_value_t = 2.)]
        timeout: f64,
    },
    /// Open a WebRTC data channel on the STUN socket and exchange data again after the relay stops.
    Webrtc {
        #[arg(long, default_value_t = 8.)]
        timeout: f64,
    },
    #[command(name = "__webrtc", hide = true)]
    WebrtcWorker {
        #[arg(value_parser = ["a", "b"])]
        role: String,
    },
    /// Try a TCP simultaneous open on the preserve/preserve path.
    Tcp {
        #[arg(long, default_value_t = 10.)]
        timeout: f64,
    },
    #[command(name = "__tcp", hide = true)]
    TcpWorker {
        #[arg(value_parser = ["a", "b"])]
        role: String,
    },
    #[command(name = "__tcp-observer", hide = true)]
    TcpObserver,
}
fn execute(action: Action) -> Result<i32> {
    match action {
        Action::Doctor { probe, json } => {
            let report = natbench::doctor::inspect(probe);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.summary());
            }
            return Ok(report.exit_code());
        }
        Action::TcpReady { address } => {
            std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(100))?;
        }
        Action::NfqueueProbe { namespace } => natbench::probe_nfqueue(&namespace)?,
        Action::Test {
            scenario,
            artifacts,
            capture,
        } => {
            let report = match natbench::scenario::run_with_capture(&scenario, &artifacts, capture)
            {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("natbench test: {error:#}");
                    return Ok(2);
                }
            };
            for case in &report.cases {
                eprintln!("{:?}: {}", case.status, case.name);
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(report.exit_code());
        }
        Action::Repeat {
            scenario,
            artifacts,
            runs,
            capture,
        } => {
            let summary =
                match natbench::repeat::run_with_capture(&scenario, &artifacts, runs, capture) {
                    Ok(summary) => summary,
                    Err(error) => {
                        eprintln!("natbench repeat: {error:#}");
                        return Ok(2);
                    }
                };
            eprint!("{}", summary.readable());
            println!("{}", serde_json::to_string_pretty(&summary)?);
            return Ok(summary.exit_code());
        }
        Action::Bench { network, timeout } => {
            let result = bench::benchmark(&Options {
                a: network.a,
                b: network.b,
                router_input: network.router_input,
                timeout_seconds: timeout,
                delay_ms: 0,
                loss_percent: 0,
                nest_a: false,
                translator: None,
                stun: false,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Matrix {
            router_input,
            timeout,
        } => {
            let mut options = Options::current_exe()?;
            options.router_input = router_input;
            options.timeout_seconds = timeout;
            println!(
                "{}",
                serde_json::to_string_pretty(&bench::matrix(&options)?)?
            );
        }
        Action::Run {
            network,
            role,
            argv,
        } => {
            let mut lab = Lab::create(network.a, network.b, network.router_input)?;
            let status = lab
                .spawn(&role, &argv.iter().map(String::as_str).collect::<Vec<_>>())?
                .wait()?;
            use std::os::unix::process::ExitStatusExt;
            return Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)));
        }
        Action::Endpoint { binds } => natbench::run_endpoint(&binds)?,
        Action::StunServer { binds } => natbench::run_stun(&binds)?,
        Action::Relay => natbench::run_relay()?,
        Action::Lifetime {
            profile,
            router_input,
            udp_timeout,
        } => {
            let result = bench::lifetime(&bench::LifetimeOptions {
                profile,
                router_input,
                udp_timeout_seconds: udp_timeout,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Collision {
            profile,
            router_input,
        } => {
            let result = bench::collision(&bench::CollisionOptions {
                profile,
                router_input,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Hairpin {
            profile,
            router_input,
        } => {
            let result = bench::hairpin(&bench::HairpinOptions {
                profile,
                router_input,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Impair {
            delay_ms,
            loss_percent,
            timeout,
        } => {
            let mut options = Options::current_exe()?;
            options.delay_ms = delay_ms;
            options.loss_percent = loss_percent;
            options.timeout_seconds = timeout;
            let result = bench::benchmark(&options)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Nested { timeout } => {
            let mut options = Options::current_exe()?;
            options.nest_a = true;
            options.timeout_seconds = timeout;
            let result = bench::benchmark(&options)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Translate {
            translator,
            timeout,
        } => {
            let mut options = Options::current_exe()?;
            options.translator = Some(translator.unwrap_or(options.executable.clone()));
            options.timeout_seconds = timeout;
            let result = bench::benchmark(&options)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Nat {
            lan_interface,
            wan_interface,
        } => natbench::run_nat(&lan_interface, &wan_interface)?,
        Action::Stun { timeout } => {
            let mut options = Options::current_exe()?;
            options.stun = true;
            options.timeout_seconds = timeout;
            let result = bench::benchmark(&options)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::Quic { timeout } => {
            let result = bench::quic(&bench::QuicOptions {
                timeout_seconds: timeout,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::QuicWorker { role } => natbench::run_quic(&role)?,
        Action::Webrtc { timeout } => {
            let result = bench::webrtc(&bench::WebrtcOptions {
                timeout_seconds: timeout,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::WebrtcWorker { role } => natbench::run_webrtc(&role)?,
        Action::Tcp { timeout } => {
            let result = bench::tcp(&bench::TcpOptions {
                timeout_seconds: timeout,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Action::TcpWorker { role } => natbench::run_tcp(&role)?,
        Action::TcpObserver => natbench::run_tcp_observer()?,
        Action::Throughput {
            bytes,
            chunk,
            timeout,
        } => {
            let result = bench::throughput(&bench::ThroughputOptions {
                bytes,
                chunk,
                timeout_seconds: timeout,
                executable: executable()?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(0)
}
fn executable() -> Result<PathBuf> {
    Ok(std::env::current_exe()?)
}
fn main() {
    let cli = Cli::parse();
    // Workers use the OS default termination behavior; only the controller owns cleanup.
    if !matches!(
        cli.command,
        Action::Endpoint { .. }
            | Action::NfqueueProbe { .. }
            | Action::TcpReady { .. }
            | Action::StunServer { .. }
            | Action::Relay
            | Action::Nat { .. }
            | Action::QuicWorker { .. }
            | Action::WebrtcWorker { .. }
            | Action::TcpWorker { .. }
            | Action::TcpObserver
    ) {
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            if let Err(error) =
                signal_hook::flag::register(signal, natbench::cancellation().clone())
            {
                eprintln!("natbench: register signal handler: {error}");
                std::process::exit(1);
            }
        }
    }
    let result = execute(cli.command);
    let code = if natbench::cancellation().load(Ordering::Relaxed) {
        130
    } else {
        match result {
            Ok(code) => code,
            Err(error) => {
                eprintln!("natbench: {error:#}");
                1
            }
        }
    };
    std::process::exit(code);
}
