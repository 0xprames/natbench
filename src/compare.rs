//! Matched workloads through executable adapters; implementations own their sockets.
use crate::{
    lab::Profile,
    repeat::{Environment, Timing},
    scenario::{self, Status},
};
use anyhow::{ensure, Context, Result};
use natbench_transport_protocol::{
    Bootstrap, Event, EventData, Implementation, Measurement, Request, Role, Workload,
    MAX_EVENT_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    kind: String,
    adapters: Vec<Adapter>,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Adapter {
    name: String,
    argv: Vec<String>,
    #[serde(default = "cwd")]
    cwd: PathBuf,
    #[serde(default)]
    env: BTreeMap<String, String>,
}
fn cwd() -> PathBuf {
    ".".into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    profile: Profile,
    deadline_ms: u64,
    workload: Workload,
    #[serde(default)]
    network: Option<crate::network::DirectConfig>,
}
impl Config {
    fn parse(input: &[u8]) -> Result<Self> {
        let header: serde_json::Value = serde_json::from_slice(input)?;
        if header["schema_version"] == 1 {
            ensure!(
                !header["cases"]
                    .as_array()
                    .is_some_and(|cases| cases.iter().any(|case| case
                        .as_object()
                        .is_some_and(|object| object.contains_key("network")))),
                "network is unsupported in comparison schema 1; use schema 2"
            );
        }
        Ok(serde_json::from_slice(input)?)
    }
    fn validate(&mut self, base: &Path) -> Result<()> {
        ensure!(
            matches!(self.schema_version, 1 | 2) && self.kind == "transport_comparison",
            "unsupported comparison input"
        );
        ensure!(
            (2..=8).contains(&self.adapters.len()) && (1..=32).contains(&self.cases.len()),
            "comparison needs 2–8 adapters and 1–32 cases"
        );
        let mut names = HashSet::new();
        for adapter in &mut self.adapters {
            ensure!(
                !adapter.name.is_empty()
                    && adapter.name.len() <= 64
                    && adapter
                        .name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    && names.insert(adapter.name.clone()),
                "adapter names must be unique safe identifiers"
            );
            ensure!(
                !adapter.argv.is_empty()
                    && !adapter.argv[0].is_empty()
                    && adapter.argv.iter().all(|a| !a.contains('\0')),
                "adapter argv is invalid"
            );
            adapter.cwd = fs::canonicalize(base.join(&adapter.cwd))
                .context("adapter working directory missing")?;
            ensure!(adapter.cwd.is_dir(), "adapter cwd is not a directory");
            for (key, value) in &adapter.env {
                ensure!(
                    !key.is_empty() && !key.contains(['=', '\0']) && !value.contains('\0'),
                    "invalid adapter environment"
                );
            }
        }
        names.clear();
        for case in &self.cases {
            if self.schema_version == 2 {
                case.network
                    .as_ref()
                    .context("comparison schema 2 requires network on every case")?
                    .links()
                    .prerequisites()?;
            }
            ensure!(
                !case.name.trim().is_empty() && names.insert(case.name.clone()),
                "case names must be nonempty and unique"
            );
            ensure!(
                (1..=60_000).contains(&case.deadline_ms),
                "deadline_ms must be 1–60000"
            );
            case.workload.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Planned {
    pub index: usize,
    pub run: u32,
    pub case: String,
    pub adapter: String,
    pub artifacts_directory: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    TransportFailed,
    Unsupported,
    InfrastructureFailed,
    Interrupted,
}
#[derive(Deserialize, Serialize)]
pub struct Attempt {
    pub index: usize,
    pub outcome: Outcome,
    pub messages: Vec<String>,
    pub implementation: Option<Implementation>,
    pub measurement: Option<Measurement>,
}
#[derive(Serialize)]
pub struct Summary {
    pub case: String,
    pub adapter: String,
    pub requested_runs: u32,
    pub outcomes: BTreeMap<Outcome, u32>,
    pub unreported_runs: u32,
    pub first_data_seconds: Option<Timing>,
    pub message_rtt_seconds: Option<Timing>,
    pub bulk_bytes_per_second: Option<Timing>,
}
#[derive(Deserialize, Serialize)]
pub struct CaseConditions {
    pub name: String,
    pub profile: Profile,
    pub(crate) network: Option<crate::network::DirectConfig>,
    #[serde(default)]
    pub(crate) deadline_ms: Option<u64>,
    #[serde(default, deserialize_with = "case_workload")]
    pub(crate) workload: Option<Workload>,
}
fn case_workload<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Workload>, D::Error> {
    let mut value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, _| {
            [
                "payload_bytes",
                "warmup_messages",
                "measured_messages",
                "bulk_bytes",
            ]
            .contains(&key.as_str())
        });
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}
#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub kind: &'static str,
    pub topology: &'static str,
    pub workload_semantics: &'static str,
    pub environment: Environment,
    pub case_conditions: Vec<CaseConditions>,
    pub planned: Vec<Planned>,
    pub attempts: Vec<Attempt>,
    pub active_attempt: Option<usize>,
    pub complete: bool,
    pub interrupted: bool,
    pub summaries: Vec<Summary>,
}
impl Report {
    fn refresh(&mut self) {
        self.complete = self.attempts.len() == self.planned.len();
        for summary in &mut self.summaries {
            summary.outcomes.clear();
            let mut first = Vec::new();
            let mut rtt = Vec::new();
            let mut bulk = Vec::new();
            let mut completed = 0;
            for attempt in &self.attempts {
                let plan = &self.planned[attempt.index];
                if plan.case != summary.case || plan.adapter != summary.adapter {
                    continue;
                }
                completed += 1;
                *summary.outcomes.entry(attempt.outcome).or_default() += 1;
                if let Some(m) = &attempt.measurement {
                    first.push(m.first_data_seconds);
                    rtt.extend(&m.message_rtt_seconds);
                    bulk.push(f64::from(m.bulk_verified_bytes) / m.bulk_seconds);
                }
            }
            summary.unreported_runs = summary.requested_runs - completed;
            summary.first_data_seconds = Timing::from_samples(first);
            summary.message_rtt_seconds = Timing::from_samples(rtt);
            summary.bulk_bytes_per_second = Timing::from_samples(bulk);
        }
    }
    pub fn exit_code(&self) -> i32 {
        if self.interrupted {
            130
        } else if self
            .attempts
            .iter()
            .any(|a| a.outcome == Outcome::InfrastructureFailed)
        {
            2
        } else if !self.complete
            || self
                .attempts
                .iter()
                .any(|a| a.outcome == Outcome::Unsupported)
        {
            3
        } else if self
            .attempts
            .iter()
            .any(|a| a.outcome == Outcome::TransportFailed)
        {
            1
        } else {
            0
        }
    }
    pub fn readable(&self) -> String {
        let mut text = format!(
            "{} / {} attempts reported (direct client-to-WAN reliable stream; fresh endpoints)\n",
            self.attempts.len(),
            self.planned.len()
        );
        for summary in &self.summaries {
            let count = |outcome| summary.outcomes.get(&outcome).copied().unwrap_or(0);
            text.push_str(&format!("{} / {}: {}/{} passed, {} transport failures, {} unsupported, {} errors, {} interrupted, {} unreported\n",summary.case,summary.adapter,count(Outcome::Passed),summary.requested_runs,count(Outcome::TransportFailed),count(Outcome::Unsupported),count(Outcome::InfrastructureFailed),count(Outcome::Interrupted),summary.unreported_runs));
            if let Some(network) = self
                .case_conditions
                .iter()
                .find(|case| case.name == summary.case)
                .and_then(|case| case.network.as_ref())
            {
                text.push_str(&format!(
                    "  network: client→server {} ms / {}% loss; server→client {} ms / {}% loss\n",
                    network.client_to_server.delay_ms,
                    network.client_to_server.loss_percent,
                    network.server_to_client.delay_ms,
                    network.server_to_client.loss_percent
                ));
            }
            for (label, timing, scale, unit) in [
                (
                    "first verified data",
                    &summary.first_data_seconds,
                    1000.,
                    "ms",
                ),
                ("message RTT", &summary.message_rtt_seconds, 1000., "ms"),
                (
                    "verified bulk goodput",
                    &summary.bulk_bytes_per_second,
                    1. / 1_048_576.,
                    "MiB/s",
                ),
            ] {
                if let Some(t) = timing {
                    text.push_str(&format!(
                        "  {label}: median {:.3} {unit}, p95 {:.3} {unit} ({} samples)\n",
                        t.median * scale,
                        t.p95 * scale,
                        t.samples
                    ));
                }
            }
        }
        for attempt in self
            .attempts
            .iter()
            .filter(|a| a.outcome != Outcome::Passed)
        {
            let plan = &self.planned[attempt.index];
            for message in &attempt.messages {
                text.push_str(&format!(
                    "  {} / {} run {}: {} ({}\u{2f}evidence)\n",
                    plan.case,
                    plan.adapter,
                    plan.run,
                    message.chars().take(512).collect::<String>(),
                    plan.artifacts_directory
                ));
            }
        }
        text
    }
    fn save(&self, root: &Path) -> Result<()> {
        scenario::atomic_write(&root.join("report.json"), &serde_json::to_vec_pretty(self)?)?;
        let failures = self
            .attempts
            .iter()
            .filter(|a| a.outcome == Outcome::TransportFailed)
            .count();
        let active_error = self.interrupted
            && self
                .active_attempt
                .is_some_and(|index| !self.attempts.iter().any(|a| a.index == index));
        let errors = self
            .attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.outcome,
                    Outcome::InfrastructureFailed | Outcome::Interrupted
                )
            })
            .count()
            + usize::from(active_error);
        let skipped = self.planned.len() - self.attempts.len()
            + self
                .attempts
                .iter()
                .filter(|a| a.outcome == Outcome::Unsupported)
                .count()
            - usize::from(active_error);
        let mut xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"transport comparison\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\">\n",self.planned.len());
        for plan in &self.planned {
            let escape = scenario::escape;
            xml.push_str(&format!(
                "<testcase classname=\"{}\" name=\"{} run {}\">",
                escape(&plan.adapter),
                escape(&plan.case),
                plan.run
            ));
            if let Some(attempt) = self.attempts.get(plan.index) {
                let message = escape(&attempt.messages.join("; "));
                match attempt.outcome {
                    Outcome::Passed => {}
                    Outcome::TransportFailed => {
                        xml.push_str(&format!("<failure message=\"{message}\"/>"))
                    }
                    Outcome::Unsupported => {
                        xml.push_str(&format!("<skipped message=\"unsupported: {message}\"/>"))
                    }
                    _ => xml.push_str(&format!("<error message=\"{message}\"/>")),
                }
            } else if self.active_attempt == Some(plan.index) && self.interrupted {
                xml.push_str("<error message=\"interrupted\"/>");
            } else {
                xml.push_str("<skipped message=\"unreported\"/>");
            }
            xml.push_str("</testcase>\n");
        }
        xml.push_str("</testsuite>\n");
        scenario::atomic_write(&root.join("junit.xml"), xml.as_bytes())
    }
}
fn private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.flush()?;
    Ok(())
}
fn events(path: &Path, request: &Request) -> Result<Vec<Event>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_EVENT_BYTES * 8 + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_EVENT_BYTES * 8,
        "adapter output exceeds parser budget"
    );
    let mut result = Vec::new();
    let lines = bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty());
    for (index, line) in lines.enumerate() {
        ensure!(
            index < 128 && line.len() <= MAX_EVENT_BYTES,
            "adapter event budget exceeded"
        );
        let event: Event =
            serde_json::from_slice(line).context("adapter stdout must be versioned JSONL")?;
        event.validate(request)?;
        result.push(event);
    }
    Ok(result)
}
fn classify(index: usize, request: &Request, suite: &scenario::Report, log: &Path) -> Attempt {
    let mut result = Attempt {
        index,
        outcome: Outcome::InfrastructureFailed,
        messages: Vec::new(),
        implementation: None,
        measurement: None,
    };
    if suite.interrupted {
        result.outcome = Outcome::Interrupted;
        result.messages.push("interrupted active workload".into());
        return result;
    }
    let Some(case) = suite.cases.first() else {
        result
            .messages
            .push("fixture supplied no completed case".into());
        return result;
    };
    if case.status == Status::InfrastructureFailed {
        result.messages = case.messages.clone();
        return result;
    }
    let parsed = (|| -> Result<Event> {
        let client = events(log, request)?;
        ensure!(
            client
                .last()
                .is_some_and(|event| !matches!(event.data, EventData::Ready { .. })),
            "terminal event must finish client output"
        );
        let mut terminal = client
            .into_iter()
            .filter(|e| !matches!(e.data, EventData::Ready { .. }));
        let event = terminal
            .next()
            .context("adapter emitted no terminal event")?;
        ensure!(
            terminal.next().is_none(),
            "adapter emitted multiple terminal events"
        );
        let ready = events(&log.with_file_name("server-1.stdout.log"), request)?;
        ensure!(
            ready.len() == 1 && ready[0].implementation == event.implementation,
            "server readiness identity/version/settings differ from client"
        );
        let mut bytes = Vec::new();
        File::open(&request.peer_file)?
            .take((MAX_EVENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_EVENT_BYTES,
            "peer bootstrap exceeds parser budget"
        );
        let peer: Bootstrap = serde_json::from_slice(&bytes)?;
        ensure!(
            peer.schema_version == 1
                && peer.kind == "transport_peer"
                && peer.run_id == request.run_id
                && peer.implementation == event.implementation,
            "peer bootstrap identity/version/settings differ from request or events"
        );
        let EventData::Ready { capabilities } = &ready[0].data else {
            anyhow::bail!("server emitted no ready event");
        };
        if matches!(event.data, EventData::Completed { .. }) {
            ensure!(
                capabilities.iter().any(|c| c == "direct_reliable_stream"),
                "server did not declare direct reliable stream support"
            );
        }
        Ok(event)
    })();
    let event = match parsed {
        Ok(event) => event,
        Err(error) => {
            result.messages.push(format!("{error:#}"));
            return result;
        }
    };
    result.implementation = Some(event.implementation);
    let exit_code = case
        .observation
        .as_ref()
        .and_then(|o| o["events"].as_array())
        .and_then(|events| {
            events
                .iter()
                .find(|e| e["process"] == "client" && e["state"] == "exited")
        })
        .and_then(|e| e["exit_code"].as_i64());
    let expected = match &event.data {
        EventData::Completed { .. } => 0,
        EventData::Failed { .. } => 1,
        EventData::Unsupported { .. } => 3,
        EventData::Ready { .. } => unreachable!(),
    };
    if exit_code != Some(expected) {
        result.messages.push(format!(
            "terminal event requires exit {expected}, observed {exit_code:?}"
        ));
        return result;
    }
    match event.data {
        EventData::Completed { measurement } => {
            result.outcome = Outcome::Passed;
            result.measurement = Some(measurement);
        }
        EventData::Failed { phase, message } => {
            result.outcome = if phase == "bootstrap" {
                Outcome::InfrastructureFailed
            } else {
                Outcome::TransportFailed
            };
            result.messages.push(format!("{phase}: {message}"));
        }
        EventData::Unsupported { reason } => {
            result.outcome = Outcome::Unsupported;
            result.messages.push(reason);
        }
        EventData::Ready { .. } => unreachable!(),
    }
    result
}
fn attempt(
    root: &Path,
    plan: &Planned,
    adapter: &Adapter,
    case: &Case,
    capture: Option<u32>,
) -> Result<(scenario::Report, Request)> {
    let directory = root.join(&plan.artifacts_directory);
    fs::create_dir(&directory)?;
    let mut nonce = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let id: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let mut request = Request {
        schema_version: 1,
        kind: "transport_request".into(),
        run_id: id,
        role: Role::Server,
        listen_address: "0.0.0.0:9443".parse()?,
        peer_address: "198.18.0.1:9443".parse()?,
        peer_file: directory.join("peer.json"),
        deadline_ms: case.deadline_ms,
        workload: case.workload.clone(),
    };
    private_json(&directory.join("server.json"), &request)?;
    request.role = Role::Client;
    request.listen_address = "0.0.0.0:0".parse()?;
    private_json(&directory.join("client.json"), &request)?;
    let program = |role: &str| {
        let mut argv = adapter.argv.clone();
        argv.extend([
            "--request".into(),
            directory
                .join(format!("{role}.json"))
                .to_string_lossy()
                .into_owned(),
        ]);
        let mut value = json!({"name":role,"role":if role=="server"{"wan"}else{"a"},"argv":argv,"cwd":adapter.cwd,"env":adapter.env,"timeout_seconds":case.deadline_ms as f64 / 1000.+5.});
        if role == "server" {
            value["ready"] = json!({"kind":"stdout_contains","text":"\"ready\""});
        }
        value
    };
    let mut input = json!({"schema_version":2,"cases":[{"name":case.name,"a":case.profile,"b":"preserve","router_input":"drop","processes":[program("server"),program("client")],"steps":[{"action":"start","process":"server"},{"action":"run","process":"client"},{"action":"stop","process":"server"}]}]});
    if let Some(network) = &case.network {
        input["schema_version"] = 4.into();
        input["cases"][0]["network"] = serde_json::to_value(network.links())?;
    }
    let path = directory.join("scenario.json");
    private_json(&path, &input)?;
    let report = scenario::run_with_capture(&path, &directory.join("evidence"), capture)?;
    Ok((report, request))
}
/// Execute cyclically rotated adapter order with fresh fixtures and preserved evidence.
pub fn run(path: &Path, artifacts: &Path, runs: u32, capture: Option<u32>) -> Result<Report> {
    ensure!((1..=100).contains(&runs), "runs must be 1–100");
    let input = fs::read(path)?;
    let mut config = Config::parse(&input)?;
    let base = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    config.validate(&base)?;
    ensure!(
        config.cases.len() * config.adapters.len() * runs as usize <= 4096,
        "comparison budget exceeds 4096 attempts; split the matrix into smaller runs"
    );
    let options = capture.map(crate::capture::Options::new).transpose()?;
    let environment = Environment::current(options.as_ref())?;
    fs::create_dir(artifacts)
        .context("artifact directory must be new and its parent must exist")?;
    let root = fs::canonicalize(artifacts)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("comparison.json"))?
        .write_all(&input)?;
    let mut planned = Vec::new();
    let mut summaries = Vec::new();
    for case in &config.cases {
        for adapter in &config.adapters {
            summaries.push(Summary {
                case: case.name.clone(),
                adapter: adapter.name.clone(),
                requested_runs: runs,
                outcomes: BTreeMap::new(),
                unreported_runs: runs,
                first_data_seconds: None,
                message_rtt_seconds: None,
                bulk_bytes_per_second: None,
            });
        }
        for run in 1..=runs {
            for offset in 0..config.adapters.len() {
                let adapter =
                    &config.adapters[((run - 1) as usize + offset) % config.adapters.len()];
                let index = planned.len();
                planned.push(Planned {
                    index,
                    run,
                    case: case.name.clone(),
                    adapter: adapter.name.clone(),
                    artifacts_directory: format!("attempt-{:04}", index + 1),
                });
            }
        }
    }
    let mut report = Report { schema_version:1,kind:"transport_comparison_report",topology:"ipv4_client_a_to_wan",workload_semantics:"direct reliable streams; fresh endpoints; bulk includes receiver verification and acknowledgement",environment,case_conditions:config.cases.iter().map(|case|CaseConditions{name:case.name.clone(),profile:case.profile,network:case.network.clone(),deadline_ms:Some(case.deadline_ms),workload:Some(case.workload.clone())}).collect(),planned,attempts:Vec::new(),active_attempt:None,complete:false,interrupted:false,summaries };
    report.save(&root)?;
    for index in 0..report.planned.len() {
        if crate::cancellation().load(Ordering::Relaxed) {
            report.interrupted = true;
            break;
        }
        let plan = &report.planned[index];
        report.active_attempt = Some(index);
        report.save(&root)?;
        let adapter = config
            .adapters
            .iter()
            .find(|a| a.name == plan.adapter)
            .unwrap();
        let case = config.cases.iter().find(|c| c.name == plan.case).unwrap();
        let mut result = match attempt(&root, plan, adapter, case, capture) {
            Ok((suite, request)) => classify(
                index,
                &request,
                &suite,
                &root
                    .join(&plan.artifacts_directory)
                    .join("evidence/case-000/client-1.stdout.log"),
            ),
            Err(error) => Attempt {
                index,
                outcome: if crate::cancellation().load(Ordering::Relaxed) {
                    Outcome::Interrupted
                } else {
                    Outcome::InfrastructureFailed
                },
                messages: vec![format!("{error:#}")],
                implementation: None,
                measurement: None,
            },
        };
        if let Some(info) = &result.implementation {
            let changed = report.attempts.iter().any(|prior| {
                let old = &report.planned[prior.index];
                old.case == plan.case
                    && old.adapter == plan.adapter
                    && prior
                        .implementation
                        .as_ref()
                        .is_some_and(|previous| previous != info)
            });
            if changed {
                result.outcome = Outcome::InfrastructureFailed;
                result.measurement = None;
                result.messages.push("adapter identity/version/settings changed within a cohort; raw output retained".into());
            }
        }
        report.interrupted |= result.outcome == Outcome::Interrupted;
        report.attempts.push(result);
        report.active_attempt = None;
        report.refresh();
        report.save(&root)?;
        if report.interrupted {
            break;
        }
    }
    report.interrupted |= crate::cancellation().load(Ordering::Relaxed);
    report.refresh();
    report.save(&root)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn request(root: &Path) -> Request {
        Request {
            schema_version: 1,
            kind: "transport_request".into(),
            run_id: "0123456789abcdef0123456789abcdef".into(),
            role: Role::Client,
            listen_address: "0.0.0.0:0".parse().unwrap(),
            peer_address: "198.18.0.1:9443".parse().unwrap(),
            peer_file: root.join("peer.json"),
            deadline_ms: 5000,
            workload: Workload {
                payload_bytes: 128,
                warmup_messages: 1,
                measured_messages: 2,
                bulk_bytes: 1024,
            },
        }
    }
    fn suite(exit_code: i32) -> scenario::Report {
        serde_json::from_value(json!({"schema_version":2,"scenario":"synthetic classifier test","planned_cases":["case"],"active_case":null,"complete":true,"interrupted":false,"cases":[{"name":"case","status":if exit_code==0{"passed"}else{"assertion_failed"},"messages":[],"observation":{"events":[{"process":"client","state":"exited","exit_code":exit_code}]}}]})).unwrap()
    }
    #[test]
    fn classification_checks_identity_metrics_terminal_order_and_exit_status() {
        let root = std::env::temp_dir().join(format!(
            "natbench-compare-classifier-{}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let request = request(&root);
        let info = Implementation {
            name: "synthetic".into(),
            version: "1".into(),
            settings: BTreeMap::new(),
        };
        private_json(
            &request.peer_file,
            &Bootstrap {
                schema_version: 1,
                kind: "transport_peer".into(),
                run_id: request.run_id.clone(),
                implementation: info.clone(),
                details: Value::Null,
            },
        )
        .unwrap();
        let ready = Event::new(
            &request,
            info.clone(),
            EventData::Ready {
                capabilities: vec!["direct_reliable_stream".into()],
            },
        );
        fs::write(
            root.join("server-1.stdout.log"),
            serde_json::to_vec(&ready).unwrap(),
        )
        .unwrap();
        let measurement = Measurement {
            workload: request.workload.clone(),
            path: "direct".into(),
            path_evidence: Value::Null,
            first_data_seconds: 0.1,
            message_rtt_seconds: vec![0.01; 2],
            bulk_verified_bytes: 1024,
            bulk_seconds: 0.02,
        };
        let completed = Event::new(&request, info.clone(), EventData::Completed { measurement });
        let log = root.join("client-1.stdout.log");
        let check = |event: &Event, exit: i32| {
            fs::write(&log, serde_json::to_vec(event).unwrap()).unwrap();
            classify(0, &request, &suite(exit), &log).outcome
        };
        assert_eq!(check(&completed, 0), Outcome::Passed);
        assert_eq!(check(&completed, 1), Outcome::InfrastructureFailed);
        assert_eq!(check(&ready, 0), Outcome::InfrastructureFailed);
        let failed = Event::new(
            &request,
            info.clone(),
            EventData::Failed {
                phase: "connect".into(),
                message: "deadline".into(),
            },
        );
        assert_eq!(check(&failed, 1), Outcome::TransportFailed);
        assert_eq!(check(&failed, 0), Outcome::InfrastructureFailed);
        let unsupported = Event::new(
            &request,
            info,
            EventData::Unsupported {
                reason: "no reliable stream support".into(),
            },
        );
        assert_eq!(check(&unsupported, 3), Outcome::Unsupported);
        let mut stale = completed.clone();
        stale.run_id.replace_range(..1, "f");
        assert_eq!(check(&stale, 0), Outcome::InfrastructureFailed);
        let mut invalid = completed.clone();
        let EventData::Completed { measurement } = &mut invalid.data else {
            unreachable!()
        };
        measurement.message_rtt_seconds.clear();
        assert_eq!(check(&invalid, 0), Outcome::InfrastructureFailed);
        fs::write(
            &log,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&completed).unwrap(),
                serde_json::to_string(&completed).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(
            classify(0, &request, &suite(0), &log).outcome,
            Outcome::InfrastructureFailed
        );
        fs::write(&log, vec![b'x'; MAX_EVENT_BYTES + 1]).unwrap();
        assert_eq!(
            classify(0, &request, &suite(0), &log).outcome,
            Outcome::InfrastructureFailed
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn timing_summary_keeps_large_finite_samples_finite() {
        let timing = Timing::from_samples(vec![f64::MAX, f64::MAX]).unwrap();
        assert!(timing.mean.is_finite());
        assert!(timing.median.is_finite());
    }
}
