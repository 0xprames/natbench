use crate::{
    check_cancelled,
    lab::{Lab, Process, Profile, RouterInput},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

const DESTINATIONS: [(&str, u16); 3] = [
    ("198.18.0.1", 9000),
    ("198.18.0.1", 9001),
    ("198.18.0.2", 9000),
];
const CLIENT_A: &str = "10.1.0.2";
const CLIENT_A2: &str = "10.3.0.2";
const CLIENT_SAME_LAN: &str = "10.1.0.3";

/// The helper executable must be a natbench binary (normally current_exe()).
pub struct Options {
    pub a: Profile,
    pub b: Profile,
    pub router_input: RouterInput,
    pub timeout_seconds: f64,
    pub delay_ms: u64,
    pub loss_percent: u64,
    pub nest_a: bool,
    /// External program run as `nat --lan-interface lan --wan-interface wan` in each router.
    /// UDP translation moves to that program. TCP masquerade stays in nftables.
    pub translator: Option<PathBuf>,
    /// Learn mapped addresses with STUN Binding on the traversal socket.
    pub stun: bool,
    pub executable: PathBuf,
}
impl Options {
    pub fn current_exe() -> Result<Self> {
        Ok(Self {
            a: Profile::Preserve,
            b: Profile::Preserve,
            router_input: RouterInput::Drop,
            timeout_seconds: 2.,
            delay_ms: 0,
            loss_percent: 0,
            nest_a: false,
            translator: None,
            stun: false,
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
        anyhow::ensure!(
            self.delay_ms <= 1000,
            "delay must be at most 1000 milliseconds"
        );
        anyhow::ensure!(self.loss_percent <= 100, "loss must be at most 100 percent");
        anyhow::ensure!(
            self.translator.is_none() || !self.nest_a,
            "userspace translation does not combine with nested NAT"
        );
        Ok(())
    }
}

/// Conntrack expiry and refresh observations. The executable must be a natbench binary.
pub struct LifetimeOptions {
    pub profile: Profile,
    pub router_input: RouterInput,
    pub udp_timeout_seconds: u64,
    pub executable: PathBuf,
}
impl LifetimeOptions {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (2..=60).contains(&self.udp_timeout_seconds),
            "udp timeout must be between 2 and 60 seconds"
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
    fn write(&mut self, request: Value) -> Result<()> {
        check_cancelled()?;
        serde_json::to_writer(&mut self.input, &request)?;
        writeln!(self.input)?;
        self.input.flush()?;
        Ok(())
    }
    fn request(&mut self, request: Value) -> Result<Value> {
        let timeout = request["timeout"].as_f64().unwrap_or(0.5) + 3.;
        self.write(request)?;
        self.read(Duration::from_secs_f64(timeout))
    }
    fn send(&mut self, socket: usize, destination: Value, data: &str) -> Result<()> {
        self.request(json!({"action": "send", "socket": socket, "to": destination, "data": data}))?;
        Ok(())
    }
    fn receive(&mut self, token: &str, timeout: f64) -> Result<Value> {
        self.request(json!({"action": "receive", "match": token, "timeout": timeout}))
    }
    fn stun(&mut self, socket: usize, destination: Value) -> Result<Value> {
        self.request(json!({"action": "stun", "socket": socket, "to": destination, "timeout": 0.5}))
    }
    fn relay(&mut self, request: Value) -> Result<Value> {
        self.request(json!({"action": "relay", "request": request}))
    }
}
fn destination(index: usize) -> Value {
    json!([DESTINATIONS[index].0, DESTINATIONS[index].1])
}

fn measure(
    client: &mut Endpoint,
    observer: &mut Endpoint,
    label: &str,
    stun: bool,
) -> Result<Value> {
    let mut mappings = Vec::new();
    if stun {
        for index in 0..3 {
            let response = client.stun(0, destination(index))?;
            mappings.push(response["mapped"].clone());
        }
    } else {
        for index in 0..3 {
            let token = format!("mapping-{label}-{index}");
            client.send(0, destination(index), &token)?;
            let packet = observer.receive(&token, 0.3)?;
            mappings.push(packet["from"].clone());
        }
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
    if options.nest_a {
        lab.add_outer_nat(options.a, options.router_input)?;
    }
    if let Some(translator) = &options.translator {
        lab.use_userspace_translator(translator)?;
    }
    let binds = json!(DESTINATIONS).to_string();
    let mut observer = if options.stun {
        Endpoint::launch(&mut lab, "wan", &options.executable, &["__stun", &binds])?
    } else {
        Endpoint::udp(&mut lab, "wan", &options.executable, json!(DESTINATIONS))?
    };
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
        measure(&mut clients[0], &mut observer, "a", options.stun)?,
        measure(&mut clients[1], &mut observer, "b", options.stun)?,
    ];
    let targets = [
        observations[0]["observed_endpoints"][0].clone(),
        observations[1]["observed_endpoints"][0].clone(),
    ];
    if options.delay_ms > 0 || options.loss_percent > 0 {
        // Discovery stays clean. Loss and delay apply to the punch and to recovery.
        lab.set_wan_impairment(options.delay_ms, options.loss_percent)?;
    }
    // Each direction crosses one router's egress qdisc, so the round trip is two delays.
    let poll = (options.delay_ms as f64 * 4. / 1000.).max(0.1);
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
            reached[0] |= !clients[0].receive("punch-b", poll)?.is_null();
            reached[1] |= !clients[1].receive("punch-a", poll)?.is_null();
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
        survived[0] = !clients[0]
            .receive("independent-b", poll.max(0.5))?
            .is_null();
        survived[1] = !clients[1]
            .receive("independent-a", poll.max(0.5))?
            .is_null();
    }
    let _restarted_relay = Endpoint::launch(&mut lab, "wan", &options.executable, &["__relay"])?;
    let recovered = relay_exchange(&mut clients, "recovered")?;
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    Ok(
        json!({"schema_version": 1, "kernel": kernel, "backend": "linux-nftables",
        "profiles": {"a": options.a, "b": options.b}, "router_input": options.router_input,
        "impairment": {"delay_ms": options.delay_ms, "loss_percent": options.loss_percent},
        "nested_a": options.nest_a,
        "translator": options.translator.as_ref().map(|path| path.display().to_string()),
        "discovery": if options.stun { "stun" } else { "observer" },
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
                delay_ms: options.delay_ms,
                loss_percent: options.loss_percent,
                nest_a: options.nest_a,
                translator: options.translator.clone(),
                stun: options.stun,
                executable: options.executable.clone(),
            })?);
        }
    }
    Ok(results)
}

#[derive(Clone, Copy)]
enum Refresh {
    None,
    Outbound,
    Inbound,
}

fn pause(total: Duration) -> Result<()> {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        check_cancelled()?;
        let slice =
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now()));
        if slice.is_zero() {
            break;
        }
        thread::sleep(slice);
    }
    Ok(())
}

fn set_udp_timeout(lab: &Lab, key: &str, seconds: u64) -> Result<()> {
    let assignment = format!("net.netfilter.{key}={seconds}");
    lab.run("ra", &["sysctl", "-q", "-w", &assignment])?;
    let output = lab.run("ra", &["sysctl", "-n", &format!("net.netfilter.{key}")])?;
    let actual = String::from_utf8(output.stdout)?.trim().parse::<u64>()?;
    anyhow::ensure!(
        actual == seconds,
        "router kept {key}={actual}, not {seconds}"
    );
    Ok(())
}

fn conntrack_tool_available() -> bool {
    Command::new("conntrack")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn conntrack_tool_dump(ns: &str) -> Result<String> {
    check_cancelled()?;
    let output = Command::new("ip")
        .args([
            "netns",
            "exec",
            ns,
            "conntrack",
            "-L",
            "-f",
            "ipv4",
            "-p",
            "udp",
        ])
        .output()
        .context("run conntrack")?;
    let stderr = String::from_utf8(output.stderr)?;
    anyhow::ensure!(output.status.success(), "conntrack -L: {}", stderr.trim());
    Ok(String::from_utf8(output.stdout)?)
}

fn conntrack_dump(lab: &Lab) -> Result<String> {
    // Ubuntu runners omit /proc/net/nf_conntrack inside a namespace. The
    // conntrack tool reads the same table over netlink.
    if conntrack_tool_available() {
        let ns = lab
            .namespaces
            .get("ra")
            .context("missing router namespace")?;
        return conntrack_tool_dump(ns);
    }
    match lab.run("ra", &["cat", "/proc/net/nf_conntrack"]) {
        Ok(output) => Ok(String::from_utf8(output.stdout)?),
        Err(error) if error.to_string().contains("No such file or directory") => {
            bail!(
                "conntrack table is not available at /proc/net/nf_conntrack; install the conntrack package"
            )
        }
        Err(error) => Err(error),
    }
}

/// The first `src=` group is the LAN tuple. The reply tuple repeats those ports
/// when allocation preserves them, so a later `dport=` is not the mapping key.
fn original_udp_tuple(line: &str) -> Option<(&str, u16, &str, u16)> {
    let mut src = None;
    let mut dst = None;
    let mut sport = None;
    let mut dport = None;
    let mut sources = 0u8;
    for field in line.split_whitespace() {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        if key == "src" {
            sources += 1;
            if sources > 1 {
                break;
            }
        }
        if sources != 1 {
            continue;
        }
        match key {
            "src" => src = Some(value),
            "dst" => dst = Some(value),
            "sport" => sport = value.parse().ok(),
            "dport" => dport = value.parse().ok(),
            _ => {}
        }
    }
    Some((src?, sport?, dst?, dport?))
}

fn ipv4_udp_line(line: &str) -> bool {
    let mut fields = line.split_whitespace();
    match fields.next() {
        Some("udp") => true,
        Some("ipv4") => fields.any(|field| field == "udp"),
        _ => false,
    }
}

fn udp_flow_present(dump: &str, src: &str, sport: u16, dst: &str, dport: u16) -> bool {
    dump.lines().any(|line| {
        ipv4_udp_line(line) && original_udp_tuple(line) == Some((src, sport, dst, dport))
    })
}

fn mapping_phase(
    lab: &Lab,
    client: &mut Endpoint,
    observer: &mut Endpoint,
    refresh: Refresh,
    timeout: u64,
    label: &str,
) -> Result<Value> {
    let sport = client.ready["ready"][0][1]
        .as_u64()
        .context("local port missing")?;
    anyhow::ensure!(sport <= u16::MAX as u64, "local port missing");
    let sport = sport as u16;
    let destination_port = DESTINATIONS[0].1;
    let opened = format!("open-{label}");
    client.send(0, destination(0), &opened)?;
    let packet = observer.receive(&opened, 0.5)?;
    if packet.is_null() {
        return Ok(Value::Null);
    }
    let mapped = packet["from"].clone();
    let dump = conntrack_dump(lab)?;
    anyhow::ensure!(
        udp_flow_present(&dump, CLIENT_A, sport, DESTINATIONS[0].0, destination_port),
        "translated packet has no conntrack entry:\n{dump}"
    );
    // The entry is created by the open packet. Keepalives, or the lack of them,
    // then have to outlast that initial timer.
    let window = Duration::from_secs(timeout + 2);
    match refresh {
        Refresh::None => pause(window)?,
        Refresh::Outbound | Refresh::Inbound => {
            let started = Instant::now();
            loop {
                match refresh {
                    Refresh::Outbound => {
                        client.send(0, destination(0), &format!("keep-{label}"))?;
                    }
                    Refresh::Inbound => {
                        observer.send(0, mapped.clone(), &format!("keep-{label}"))?;
                    }
                    Refresh::None => unreachable!("idle waits without keepalives"),
                }
                let remaining = window.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    break;
                }
                pause(Duration::from_secs(1).min(remaining))?;
            }
        }
    }
    let present = udp_flow_present(
        &conntrack_dump(lab)?,
        CLIENT_A,
        sport,
        DESTINATIONS[0].0,
        destination_port,
    );
    let probe = format!("probe-{label}");
    observer.send(0, mapped.clone(), &probe)?;
    let returned = !client.receive(&probe, 0.5)?.is_null();
    Ok(json!({
        "mapped_endpoint": mapped,
        "conntrack_present": present,
        "return_traffic": returned,
    }))
}

pub fn lifetime(options: &LifetimeOptions) -> Result<Value> {
    options.validate()?;
    let started = Instant::now();
    let mut lab = Lab::create(options.profile, options.profile, options.router_input)?;
    // Unreplied and answered flows use different timers. Equal values make one
    // idle gap mean the same thing for both, whichever timer the flow is on.
    let timeout = options.udp_timeout_seconds;
    set_udp_timeout(&lab, "nf_conntrack_udp_timeout", timeout)?;
    set_udp_timeout(&lab, "nf_conntrack_udp_timeout_stream", timeout)?;
    let mut observer = Endpoint::udp(
        &mut lab,
        "wan",
        &options.executable,
        json!([DESTINATIONS[0]]),
    )?;
    let mut client = Endpoint::udp(
        &mut lab,
        "a",
        &options.executable,
        json!([["0.0.0.0", 10000]]),
    )?;
    let idle = mapping_phase(
        &lab,
        &mut client,
        &mut observer,
        Refresh::None,
        timeout,
        "idle",
    )?;
    let (outbound, inbound) = if idle.is_null() {
        (Value::Null, Value::Null)
    } else {
        let outbound = mapping_phase(
            &lab,
            &mut client,
            &mut observer,
            Refresh::Outbound,
            timeout,
            "outbound",
        )?;
        anyhow::ensure!(
            !outbound.is_null(),
            "outbound refresh mapping was not observed"
        );
        let inbound = mapping_phase(
            &lab,
            &mut client,
            &mut observer,
            Refresh::Inbound,
            timeout,
            "inbound",
        )?;
        anyhow::ensure!(
            !inbound.is_null(),
            "inbound refresh mapping was not observed"
        );
        (outbound, inbound)
    };
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    Ok(json!({
        "schema_version": 1,
        "experiment": "mapping-lifetime",
        "kernel": kernel,
        "backend": "linux-nftables",
        "profile": options.profile,
        "router_input": options.router_input,
        "udp_timeout_seconds": timeout,
        "udp_stream_timeout_seconds": timeout,
        "established": !idle.is_null(),
        "idle": idle,
        "outbound_refresh": outbound,
        "inbound_refresh": inbound,
        "elapsed_seconds": (started.elapsed().as_secs_f64() * 1000.).round() / 1000.,
    }))
}

/// Two clients behind one router, both bound to the same source port.
pub struct CollisionOptions {
    pub profile: Profile,
    pub router_input: RouterInput,
    pub executable: PathBuf,
}

fn mapped_port(endpoint: &Value) -> Option<u64> {
    endpoint.get(1).and_then(Value::as_u64)
}

fn collision_behavior(
    first_present: bool,
    second_present: bool,
    first_port: Option<u64>,
    second_port: Option<u64>,
) -> &'static str {
    match (first_present, second_present, first_port, second_port) {
        (true, true, Some(first), Some(second)) if first != second => "port-preserving",
        (false, true, Some(first), Some(second)) if first == second => "port-overloading",
        _ => "inconclusive",
    }
}

pub fn collision(options: &CollisionOptions) -> Result<Value> {
    let started = Instant::now();
    let mut lab = Lab::create(options.profile, options.profile, options.router_input)?;
    lab.add_lan_client()?;
    let mut observer = Endpoint::udp(
        &mut lab,
        "wan",
        &options.executable,
        json!([DESTINATIONS[0]]),
    )?;
    let mut first = Endpoint::udp(
        &mut lab,
        "a",
        &options.executable,
        json!([["0.0.0.0", 10000]]),
    )?;
    let mut second = Endpoint::udp(
        &mut lab,
        "a2",
        &options.executable,
        json!([["0.0.0.0", 10000]]),
    )?;
    let first_sport = first.ready["ready"][0][1]
        .as_u64()
        .context("local port missing")?;
    let second_sport = second.ready["ready"][0][1]
        .as_u64()
        .context("local port missing")?;
    anyhow::ensure!(
        first_sport <= u16::MAX as u64 && second_sport <= u16::MAX as u64,
        "local port missing"
    );
    first.send(0, destination(0), "open-first")?;
    let first_packet = observer.receive("open-first", 0.5)?;
    let (first_report, second_report, behavior) = if first_packet.is_null() {
        (Value::Null, Value::Null, "unobserved")
    } else {
        let dump = conntrack_dump(&lab)?;
        anyhow::ensure!(
            udp_flow_present(
                &dump,
                CLIENT_A,
                first_sport as u16,
                DESTINATIONS[0].0,
                DESTINATIONS[0].1
            ),
            "translated packet has no conntrack entry:\n{dump}"
        );
        second.send(0, destination(0), "open-second")?;
        let second_packet = observer.receive("open-second", 0.5)?;
        let dump = conntrack_dump(&lab)?;
        let first_present = udp_flow_present(
            &dump,
            CLIENT_A,
            first_sport as u16,
            DESTINATIONS[0].0,
            DESTINATIONS[0].1,
        );
        let second_present = !second_packet.is_null()
            && udp_flow_present(
                &dump,
                CLIENT_A2,
                second_sport as u16,
                DESTINATIONS[0].0,
                DESTINATIONS[0].1,
            );
        first.send(0, destination(0), "again-first")?;
        let again = observer.receive("again-first", 0.5)?;
        let behavior = if second_packet.is_null() {
            "second-unobserved"
        } else {
            collision_behavior(
                first_present,
                second_present,
                mapped_port(&first_packet["from"]),
                mapped_port(&second_packet["from"]),
            )
        };
        let first_report = json!({
            "lan": [CLIENT_A, first_sport],
            "mapped_endpoint": first_packet["from"],
            "port_preserved": mapped_port(&first_packet["from"]) == Some(first_sport),
            "conntrack_present_after_collision": first_present,
            "mapped_endpoint_after_collision": again.get("from").cloned().unwrap_or(Value::Null),
        });
        let second_report = if second_packet.is_null() {
            Value::Null
        } else {
            json!({
                "lan": [CLIENT_A2, second_sport],
                "mapped_endpoint": second_packet["from"],
                "port_preserved": mapped_port(&second_packet["from"]) == Some(second_sport),
                "conntrack_present": second_present,
            })
        };
        (first_report, second_report, behavior)
    };
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    Ok(json!({
        "schema_version": 1,
        "experiment": "port-collision",
        "kernel": kernel,
        "backend": "linux-nftables",
        "profile": options.profile,
        "router_input": options.router_input,
        "established": !first_packet.is_null(),
        "behavior": behavior,
        "first": first_report,
        "second": second_report,
        "elapsed_seconds": (started.elapsed().as_secs_f64() * 1000.).round() / 1000.,
    }))
}

/// Two clients on one LAN, each sending to the other's public mapping.
pub struct HairpinOptions {
    pub profile: Profile,
    pub router_input: RouterInput,
    pub executable: PathBuf,
}

fn shared_hairpin_behavior(left: &Value, right: &Value) -> &'static str {
    let left = left["behavior"].as_str();
    let right = right["behavior"].as_str();
    if left != right {
        return "inconclusive";
    }
    match left {
        Some("no-hairpin") => "no-hairpin",
        Some("internal-source") => "internal-source",
        Some("external-source") => "external-source",
        _ => "inconclusive",
    }
}
fn hairpin_behavior(
    source_ip: Option<&str>,
    sender_lan: &str,
    sender_public: Option<&str>,
) -> &'static str {
    match source_ip {
        None => "no-hairpin",
        Some(ip) if ip == sender_lan => "internal-source",
        Some(ip) if sender_public == Some(ip) => "external-source",
        Some(_) => "inconclusive",
    }
}

fn hairpin_direction(
    sender: &mut Endpoint,
    receiver: &mut Endpoint,
    target: &Value,
    token: &str,
    sender_lan: &str,
    sender_public: Option<&str>,
) -> Result<Value> {
    sender.send(0, target.clone(), token)?;
    let packet = receiver.receive(token, 0.5)?;
    let source = packet.get("from").cloned().unwrap_or(Value::Null);
    let source_ip = source.get(0).and_then(Value::as_str);
    Ok(json!({
        "target": target,
        "received": !packet.is_null(),
        "source": source,
        "behavior": hairpin_behavior(source_ip, sender_lan, sender_public),
    }))
}

pub fn hairpin(options: &HairpinOptions) -> Result<Value> {
    let started = Instant::now();
    let mut lab = Lab::create(options.profile, options.profile, options.router_input)?;
    lab.add_same_lan_client()?;
    let mut observer = Endpoint::udp(
        &mut lab,
        "wan",
        &options.executable,
        json!([DESTINATIONS[0]]),
    )?;
    let mut client = Endpoint::udp(
        &mut lab,
        "a",
        &options.executable,
        json!([["0.0.0.0", 10000]]),
    )?;
    let mut peer = Endpoint::udp(
        &mut lab,
        "a2",
        &options.executable,
        json!([["0.0.0.0", 10000]]),
    )?;
    client.send(0, destination(0), "map-a")?;
    let mapped_a = observer.receive("map-a", 0.5)?;
    let (mapped_a, mapped_peer, a_to_peer, peer_to_a, behavior) = if mapped_a.is_null() {
        (
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            "unobserved",
        )
    } else {
        peer.send(0, destination(0), "map-peer")?;
        let mapped_peer = observer.receive("map-peer", 0.5)?;
        anyhow::ensure!(!mapped_peer.is_null(), "peer mapping was not observed");
        let public_a = mapped_a["from"][0].as_str().map(str::to_owned);
        let public_peer = mapped_peer["from"][0].as_str().map(str::to_owned);
        let target_a = mapped_a["from"].clone();
        let target_peer = mapped_peer["from"].clone();
        let a_to_peer = hairpin_direction(
            &mut client,
            &mut peer,
            &target_peer,
            "hairpin-a",
            CLIENT_A,
            public_a.as_deref(),
        )?;
        let peer_to_a = hairpin_direction(
            &mut peer,
            &mut client,
            &target_a,
            "hairpin-peer",
            CLIENT_SAME_LAN,
            public_peer.as_deref(),
        )?;
        let behavior = shared_hairpin_behavior(&a_to_peer, &peer_to_a);
        (target_a, target_peer, a_to_peer, peer_to_a, behavior)
    };
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    Ok(json!({
        "schema_version": 1,
        "experiment": "hairpin",
        "kernel": kernel,
        "backend": "linux-nftables",
        "profile": options.profile,
        "router_input": options.router_input,
        "established": !mapped_a.is_null(),
        "behavior": behavior,
        "mappings": {"a": mapped_a, "peer": mapped_peer},
        "a_to_peer": a_to_peer,
        "peer_to_a": peer_to_a,
        "elapsed_seconds": (started.elapsed().as_secs_f64() * 1000.).round() / 1000.,
    }))
}

pub struct QuicOptions {
    pub timeout_seconds: f64,
    pub executable: PathBuf,
}

pub fn quic(options: &QuicOptions) -> Result<Value> {
    anyhow::ensure!(
        options.timeout_seconds.is_finite() && options.timeout_seconds > 0.,
        "invalid timeout"
    );
    let started = Instant::now();
    let mut lab = Lab::create(Profile::Preserve, Profile::Preserve, RouterInput::Drop)?;
    let binds = json!(DESTINATIONS).to_string();
    let _stun = Endpoint::launch(&mut lab, "wan", &options.executable, &["__stun", &binds])?;
    let relay = Endpoint::launch(&mut lab, "wan", &options.executable, &["__relay"])?;
    let mut server = Endpoint::launch(&mut lab, "b", &options.executable, &["__quic", "b"])?;
    let mut client = Endpoint::launch(&mut lab, "a", &options.executable, &["__quic", "a"])?;
    let timeout = Duration::from_secs_f64(options.timeout_seconds);
    let before_server = server.read(timeout)?;
    let before_client = client.read(timeout)?;
    relay.process.stop()?;
    server.write(json!({"action": "again"}))?;
    client.write(json!({"action": "again"}))?;
    let after_server = server.read(timeout)?;
    let after_client = client.read(timeout)?;
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    Ok(json!({
        "schema_version": 1,
        "experiment": "quic",
        "kernel": kernel,
        "backend": "linux-nftables",
        "profiles": {"a": Profile::Preserve, "b": Profile::Preserve},
        "discovery": "stun",
        "mapped": {
            "a": client.ready["mapped"].clone(),
            "b": server.ready["mapped"].clone(),
        },
        "data_before_relay_shutdown": before_client["data_before"] == true
            && before_server["data_before"] == true,
        "relay_shutdown": true,
        "data_after_relay_shutdown": after_client["data_after"] == true
            && after_server["data_after"] == true,
        "elapsed_seconds": (started.elapsed().as_secs_f64() * 1000.).round() / 1000.,
    }))
}

pub struct WebrtcOptions {
    pub timeout_seconds: f64,
    pub executable: PathBuf,
}

pub fn webrtc(options: &WebrtcOptions) -> Result<Value> {
    anyhow::ensure!(
        options.timeout_seconds.is_finite() && options.timeout_seconds > 0.,
        "invalid timeout"
    );
    let started = Instant::now();
    let mut lab = Lab::create(Profile::Preserve, Profile::Preserve, RouterInput::Drop)?;
    let binds = json!(DESTINATIONS).to_string();
    let _stun = Endpoint::launch(&mut lab, "wan", &options.executable, &["__stun", &binds])?;
    let relay = Endpoint::launch(&mut lab, "wan", &options.executable, &["__relay"])?;
    let mut answerer = Endpoint::launch(&mut lab, "b", &options.executable, &["__webrtc", "b"])?;
    let mut offerer = Endpoint::launch(&mut lab, "a", &options.executable, &["__webrtc", "a"])?;
    let timeout = Duration::from_secs_f64(options.timeout_seconds);
    let before_answerer = answerer.read(timeout)?;
    let before_offerer = offerer.read(timeout)?;
    relay.process.stop()?;
    answerer.write(json!({"action": "again"}))?;
    offerer.write(json!({"action": "again"}))?;
    let after_answerer = answerer.read(timeout)?;
    let after_offerer = offerer.read(timeout)?;
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    Ok(json!({
        "schema_version": 1,
        "experiment": "webrtc",
        "kernel": kernel,
        "backend": "linux-nftables",
        "profiles": {"a": Profile::Preserve, "b": Profile::Preserve},
        "discovery": "stun",
        "mapped": {
            "a": offerer.ready["mapped"].clone(),
            "b": answerer.ready["mapped"].clone(),
        },
        "data_before_relay_shutdown": before_offerer["data_before"] == true
            && before_answerer["data_before"] == true,
        "relay_shutdown": true,
        "data_after_relay_shutdown": after_offerer["data_after"] == true
            && after_answerer["data_after"] == true,
        "elapsed_seconds": (started.elapsed().as_secs_f64() * 1000.).round() / 1000.,
    }))
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
    #[test]
    fn invalid_impairment_does_not_create_namespaces() {
        let mut options = Options::current_exe().unwrap();
        options.delay_ms = 1001;
        assert!(benchmark(&options).is_err());
        options.delay_ms = 0;
        options.loss_percent = 101;
        assert!(benchmark(&options).is_err());
    }
    #[test]
    fn invalid_lifetime_timeouts_do_not_create_namespaces() {
        let executable = std::env::current_exe().unwrap();
        for udp_timeout_seconds in [0, 1, 61] {
            let options = LifetimeOptions {
                profile: Profile::Preserve,
                router_input: RouterInput::Drop,
                udp_timeout_seconds,
                executable: executable.clone(),
            };
            assert!(lifetime(&options).is_err());
        }
    }
    #[test]
    fn conntrack_match_uses_the_lan_tuple_only() {
        let dump = "\
ipv4     2 udp      17 2 src=10.1.0.2 dst=198.18.0.1 sport=10000 dport=9000 src=198.18.0.1 dst=198.18.0.10 sport=9000 dport=10000 use=1
ipv4     2 tcp      6 2 src=10.1.0.2 dst=198.18.0.1 sport=10000 dport=9000 src=198.18.0.1 dst=198.18.0.10 sport=9000 dport=10000 use=1
";
        assert!(udp_flow_present(dump, CLIENT_A, 10000, "198.18.0.1", 9000));
        let prefixed = "\
ipv4     2 udp      17 2 src=10.1.0.2 dst=198.18.0.1 sport=100000 dport=90001 src=198.18.0.1 dst=198.18.0.10 sport=90001 dport=100000 use=1
";
        assert!(!udp_flow_present(
            prefixed,
            CLIENT_A,
            10000,
            "198.18.0.1",
            9000
        ));
        assert!(!udp_flow_present(
            dump,
            "198.18.0.1",
            9000,
            "198.18.0.10",
            10000
        ));
        assert!(!udp_flow_present("", CLIENT_A, 10000, "198.18.0.1", 9000));
        let listed = "\
udp      17 29 src=10.1.0.2 dst=198.18.0.1 sport=10000 dport=9000 [UNREPLIED] src=198.18.0.1 dst=198.18.0.10 sport=9000 dport=10000 mark=0 use=1
";
        assert!(udp_flow_present(
            listed,
            CLIENT_A,
            10000,
            "198.18.0.1",
            9000
        ));
        assert!(!udp_flow_present(
            listed,
            "198.18.0.1",
            9000,
            "198.18.0.10",
            10000
        ));
    }
    #[test]
    fn collision_classes_cover_preserve_and_overload() {
        assert_eq!(
            collision_behavior(true, true, Some(10000), Some(40000)),
            "port-preserving"
        );
        assert_eq!(
            collision_behavior(false, true, Some(10000), Some(10000)),
            "port-overloading"
        );
        assert_eq!(
            collision_behavior(true, true, Some(10000), Some(10000)),
            "inconclusive"
        );
    }
    #[test]
    fn hairpin_classes_cover_the_three_rfc_behaviors() {
        assert_eq!(
            hairpin_behavior(None, CLIENT_A, Some("198.18.0.10")),
            "no-hairpin"
        );
        assert_eq!(
            hairpin_behavior(Some(CLIENT_SAME_LAN), CLIENT_SAME_LAN, Some("198.18.0.10")),
            "internal-source"
        );
        assert_eq!(
            hairpin_behavior(Some("198.18.0.10"), CLIENT_SAME_LAN, Some("198.18.0.10")),
            "external-source"
        );
        let left = json!({"behavior": "no-hairpin"});
        let right = json!({"behavior": "external-source"});
        assert_eq!(shared_hairpin_behavior(&left, &left), "no-hairpin");
        assert_eq!(shared_hairpin_behavior(&left, &right), "inconclusive");
    }
}
