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
}
fn execute(action: Action) -> Result<i32> {
    match action {
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
        Action::Endpoint { .. } | Action::StunServer { .. } | Action::Relay | Action::Nat { .. }
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
