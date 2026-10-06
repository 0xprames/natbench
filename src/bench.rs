use crate::{
    check_cancelled,
    lab::{Lab, Process, Profile, RouterInput},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::ChildStdin,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

const DESTINATIONS: [(&str, u16); 3] = [
    ("198.18.0.1", 9000),
    ("198.18.0.1", 9001),
    ("198.18.0.2", 9000),
];

/// The helper executable must be a natbench binary (normally current_exe()).
pub struct Options {
    pub a: Profile,
    pub b: Profile,
    pub router_input: RouterInput,
    pub timeout_seconds: f64,
    pub executable: PathBuf,
}
impl Options {
    pub fn current_exe() -> Result<Self> {
        Ok(Self {
            a: Profile::Preserve,
            b: Profile::Preserve,
            router_input: RouterInput::Drop,
            timeout_seconds: 2.,
            executable: std::env::current_exe()?,
        })
    }
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.timeout_seconds.is_finite()
                && self.timeout_seconds > 0.
                && self.timeout_seconds <= 3600.,
            "timeout must be finite and between 0 and 3600 seconds (exclusive of 0)"
        );
        Ok(())
    }
}

struct Endpoint {
    process: Process,
    input: ChildStdin,
    output: Receiver<std::io::Result<String>>,
    ready: Value,
}
impl Endpoint {
    fn launch(lab: &mut Lab, role: &str, exe: &Path, args: &[&str]) -> Result<Self> {
        let exe = exe.to_str().context("executable path must be UTF-8")?;
        let argv = std::iter::once(exe)
            .chain(args.iter().copied())
            .collect::<Vec<_>>();
        let process = lab.spawn_piped(role, &argv)?;
        let (input, stdout) = {
            let mut child = process.0.lock().unwrap();
            (child.stdin.take().unwrap(), child.stdout.take().unwrap())
        };
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut endpoint = Self {
            process,
            input,
            output,
            ready: Value::Null,
        };
        endpoint.ready = endpoint.read(Duration::from_secs(4))?;
        Ok(endpoint)
    }
    fn udp(lab: &mut Lab, role: &str, exe: &Path, binds: Value) -> Result<Self> {
        Self::launch(lab, role, exe, &["__endpoint", &binds.to_string()])
    }
    fn read(&self, timeout: Duration) -> Result<Value> {
        let start = Instant::now();
        loop {
            check_cancelled()?;
            match self.output.recv_timeout(Duration::from_millis(25)) {
                Ok(line) => return Ok(serde_json::from_str(&line?)?),
                Err(RecvTimeoutError::Disconnected) => bail!("endpoint exited before responding"),
                Err(RecvTimeoutError::Timeout) if start.elapsed() >= timeout => {
                    bail!("endpoint response timed out")
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn request(&mut self, request: Value) -> Result<Value> {
        check_cancelled()?;
        serde_json::to_writer(&mut self.input, &request)?;
        writeln!(self.input)?;
        self.input.flush()?;
        self.read(Duration::from_secs_f64(
            request["timeout"].as_f64().unwrap_or(0.5) + 3.,
        ))
    }
    fn send(&mut self, socket: usize, destination: Value, data: &str) -> Result<()> {
        self.request(json!({"action": "send", "socket": socket, "to": destination, "data": data}))?;
        Ok(())
    }
    fn receive(&mut self, token: &str, timeout: f64) -> Result<Value> {
        self.request(json!({"action": "receive", "match": token, "timeout": timeout}))
    }
    fn relay(&mut self, request: Value) -> Result<Value> {
        self.request(json!({"action": "relay", "request": request}))
    }
}
fn destination(index: usize) -> Value {
    json!([DESTINATIONS[index].0, DESTINATIONS[index].1])
}

fn measure(client: &mut Endpoint, observer: &mut Endpoint, label: &str) -> Result<Value> {
    let mut mappings = Vec::new();
    for index in 0..3 {
        let token = format!("mapping-{label}-{index}");
        client.send(0, destination(index), &token)?;
        let packet = observer.receive(&token, 0.3)?;
        mappings.push(packet["from"].clone());
    }
    if mappings.iter().any(Value::is_null) {
        return Ok(
            json!({"observed_endpoints": mappings, "mapping": "unobserved", "filtering": "unobserved", "port_preserved": null}),
        );
    }
    let mapping = if mappings[0] == mappings[1] && mappings[1] == mappings[2] {
        "endpoint-independent"
    } else if mappings[0] == mappings[1] {
        "address-dependent"
    } else {
        "address-and-port-dependent"
    };
    // A fresh socket has contacted only destination 0, isolating filtering from mapping probes.
    let token = format!("filter-open-{label}");
    client.send(1, destination(0), &token)?;
    let packet = observer.receive(&token, 0.3)?;
    anyhow::ensure!(!packet.is_null(), "filtering setup packet missing");
    let mut allowed = Vec::new();
    for index in [2, 1, 0] {
        let token = format!("filter-{label}-{index}");
        observer.send(index, packet["from"].clone(), &token)?;
        allowed.push(!client.receive(&token, 0.3)?.is_null());
    }
    let filtering = match allowed.as_slice() {
        [true, true, true] => "endpoint-independent",
        [false, true, true] => "address-dependent",
        [false, false, true] => "address-and-port-dependent",
        [false, false, false] => "no-return-traffic",
        _ => "inconclusive",
    };
    Ok(
        json!({"observed_endpoints": mappings, "mapping": mapping, "filtering": filtering,
        "filter_accepts_different_ip_same_ip_original": allowed,
        "port_preserved": mappings[0][1] == client.ready["ready"][0][1]}),
    )
}
fn relay_exchange(clients: &mut [Endpoint; 2], label: &str) -> Result<bool> {
    for (index, side, peer) in [(0, "a", "b"), (1, "b", "a")] {
        let response = clients[index]
            .relay(json!({"action": "put", "to": peer, "data": format!("{label}-{side}")}))?;
        if response["ok"] != true || response["response"] != json!({"accepted": true}) {
            return Ok(false);
        }
    }
    for (index, side, peer) in [(0, "a", "b"), (1, "b", "a")] {
        let response = clients[index].relay(json!({"action": "fetch", "role": side}))?;
        if response["ok"] != true || response["response"] != json!([format!("{label}-{peer}")]) {
            return Ok(false);
        }
    }
    Ok(true)
}
pub fn benchmark(options: &Options) -> Result<Value> {
    options.validate()?;
    let started = Instant::now();
    let mut lab = Lab::create(options.a, options.b, options.router_input)?;
    let mut observer = Endpoint::udp(&mut lab, "wan", &options.executable, json!(DESTINATIONS))?;
    let relay = Endpoint::launch(&mut lab, "wan", &options.executable, &["__relay"])?;
    let mut clients = [
        Endpoint::udp(
            &mut lab,
            "a",
            &options.executable,
            json!([["0.0.0.0", 10000], ["0.0.0.0", 10001]]),
        )?,
        Endpoint::udp(
            &mut lab,
            "b",
            &options.executable,
            json!([["0.0.0.0", 10000], ["0.0.0.0", 10001]]),
        )?,
    ];
    let before = relay_exchange(&mut clients, "before")?;
    anyhow::ensure!(before, "TCP relay baseline failed");
    let observations = [
        measure(&mut clients[0], &mut observer, "a")?,
        measure(&mut clients[1], &mut observer, "b")?,
    ];
    let targets = [
        observations[0]["observed_endpoints"][0].clone(),
        observations[1]["observed_endpoints"][0].clone(),
    ];
    let mut reached = [false; 2];
    let mut attempts = 0;
    let punch_started = Instant::now();
    if targets.iter().all(|value| !value.is_null()) {
        while punch_started.elapsed().as_secs_f64() < options.timeout_seconds
            && !reached.iter().all(|&v| v)
        {
            attempts += 1;
            clients[0].send(0, targets[1].clone(), "punch-a")?;
            clients[1].send(0, targets[0].clone(), "punch-b")?;
            reached[0] |= !clients[0].receive("punch-b", 0.1)?.is_null();
            reached[1] |= !clients[1].receive("punch-a", 0.1)?.is_null();
        }
    }
    relay.process.stop()?;
    let outage = [
        clients[0].relay(json!({"action": "fetch", "role": "a"}))?["ok"] == false,
        clients[1].relay(json!({"action": "fetch", "role": "b"}))?["ok"] == false,
    ];
    observer.process.stop()?;
    let mut survived = [false; 2];
    if reached.iter().all(|&v| v) {
        clients[0].send(0, targets[1].clone(), "independent-a")?;
        clients[1].send(0, targets[0].clone(), "independent-b")?;
        survived[0] = !clients[0].receive("independent-b", 0.5)?.is_null();
        survived[1] = !clients[1].receive("independent-a", 0.5)?.is_null();
    }
    let _restarted_relay = Endpoint::launch(&mut lab, "wan", &options.executable, &["__relay"])?;
    let recovered = relay_exchange(&mut clients, "recovered")?;
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    Ok(
        json!({"schema_version": 1, "kernel": kernel, "backend": "linux-nftables",
        "profiles": {"a": options.a, "b": options.b}, "router_input": options.router_input,
        "observations": {"a": observations[0], "b": observations[1]},
        "relay": {"bidirectional_before": before, "outage_detected": {"a": outage[0], "b": outage[1]}, "bidirectional_after_restart": recovered},
        "traversal": {"received": {"a": reached[0], "b": reached[1]}, "bidirectional": reached.iter().all(|&v| v),
            "after_observer_shutdown": {"a": survived[0], "b": survived[1]}, "attempts": attempts, "timeout_seconds": options.timeout_seconds},
        "elapsed_seconds": (started.elapsed().as_secs_f64() * 1000.).round() / 1000.}),
    )
}
pub fn matrix(options: &Options) -> Result<Vec<Value>> {
    options.validate()?;
    let mut results = Vec::new();
    for a in Profile::ALL {
        for b in Profile::ALL {
            results.push(benchmark(&Options {
                a,
                b,
                router_input: options.router_input,
                timeout_seconds: options.timeout_seconds,
                executable: options.executable.clone(),
            })?);
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_timeouts_do_not_create_namespaces() {
        let mut options = Options::current_exe().unwrap();
        for timeout in [0., -1., f64::NAN, f64::INFINITY, 3601.] {
            options.timeout_seconds = timeout;
            assert!(benchmark(&options).is_err());
        }
    }
}
