//! One executable's advertised direct-stream contract, including real total-loss failures.
use crate::{
    compare::{self, Outcome},
    scenario,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    kind: String,
    adapter: Value,
}
#[derive(Serialize)]
struct Verdict {
    index: usize,
    case: String,
    expected: Outcome,
    observed: Option<Outcome>,
    passed: bool,
    messages: Vec<String>,
}
#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    kind: &'static str,
    complete: bool,
    interrupted: bool,
    exit_code: i32,
    verdicts: Vec<Verdict>,
    runs_report: &'static str,
}
impl Report {
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
    pub fn readable(&self) -> String {
        self.verdicts
            .iter()
            .map(|v| {
                if v.passed {
                    format!("{}: passed\n", v.case)
                } else {
                    let label = |outcome: Option<Outcome>| {
                        outcome.map_or_else(
                            || "unreported".into(),
                            |outcome| {
                                serde_json::to_value(outcome)
                                    .unwrap()
                                    .as_str()
                                    .unwrap()
                                    .to_owned()
                            },
                        )
                    };
                    format!(
                        "{}: expected {}, observed {}; {}\n",
                        v.case,
                        label(Some(v.expected)),
                        label(v.observed),
                        v.messages.join("; ")
                    )
                }
            })
            .collect()
    }
}
pub fn run(path: &Path, artifacts: &Path, runs: u32, capture: Option<u32>) -> Result<Report> {
    let mut input = Vec::new();
    fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut input)?;
    ensure!(input.len() <= 1024 * 1024, "adapter config exceeds 1 MiB");
    let config: Config = serde_json::from_slice(&input)?;
    ensure!(
        config.schema_version == 1 && config.kind == "transport_adapter_config",
        "unsupported adapter config"
    );
    let base = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let mut cases = Vec::new();
    for (label, workload, deadline) in [
        (
            "minimum",
            json!({"payload_bytes":1,"warmup_messages":0,"measured_messages":1,"bulk_bytes":1024}),
            10000,
        ),
        (
            "typical",
            json!({"payload_bytes":128,"warmup_messages":2,"measured_messages":10,"bulk_bytes":1048576}),
            10000,
        ),
        (
            "maximum",
            json!({"payload_bytes":65536,"warmup_messages":100,"measured_messages":1000,"bulk_bytes":16777216}),
            60000,
        ),
    ] {
        for profile in ["preserve", "random"] {
            cases.push(json!({"name":format!("{label} workload {profile}"),"profile":profile,"deadline_ms":deadline,"workload":workload,"network":{"client_to_server":{"delay_ms":0,"loss_percent":0},"server_to_client":{"delay_ms":0,"loss_percent":0}}}));
        }
    }
    for direction in ["client_to_server", "server_to_client"] {
        let mut case = cases[0].clone();
        case["name"] = format!("total loss {direction}").into();
        case["deadline_ms"] = 2000.into();
        case["network"][direction]["loss_percent"] = 100.into();
        cases.push(case);
    }
    let experiment = serde_json::to_vec(
        &json!({"schema_version":1,"kind":"transport_adapter_conformance_plan","adapters":[config.adapter],"cases":cases}),
    )?;
    compare::preflight_input(&experiment, &base, runs, capture, true)?;
    fs::create_dir(artifacts)
        .context("artifact directory must be new and its parent must exist")?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(artifacts.join("adapter.json"))?
        .write_all(&input)?;
    let raw = compare::run_input(
        &experiment,
        &base,
        &artifacts.join("runs"),
        runs,
        capture,
        true,
    )?;
    let mut verdicts = Vec::new();
    let mut failure = false;
    let mut error = false;
    let mut unsupported = false;
    for plan in &raw.planned {
        let expected = if plan.case.starts_with("total loss ") {
            Outcome::TransportFailed
        } else {
            Outcome::Passed
        };
        let attempt = raw.attempts.get(plan.index);
        let observed = attempt.map(|a| a.outcome);
        let passed = observed == Some(expected);
        error |= observed == Some(Outcome::InfrastructureFailed);
        unsupported |= observed.is_none() || observed == Some(Outcome::Unsupported);
        failure |= !passed;
        verdicts.push(Verdict {
            index: plan.index,
            case: plan.case.clone(),
            expected,
            observed,
            passed,
            messages: attempt.map_or_else(|| vec!["unreported".into()], |a| a.messages.clone()),
        });
    }
    let code = if raw.interrupted {
        130
    } else if error {
        2
    } else if !raw.complete || unsupported {
        3
    } else if failure {
        1
    } else {
        0
    };
    let report = Report {
        schema_version: 1,
        kind: "transport_adapter_conformance_report",
        complete: raw.complete,
        interrupted: raw.interrupted,
        exit_code: code,
        verdicts,
        runs_report: "runs/report.json",
    };
    scenario::atomic_write(
        &artifacts.join("report.json"),
        &serde_json::to_vec_pretty(&report)?,
    )?;
    let failed = report
        .verdicts
        .iter()
        .filter(|v| {
            !v.passed && matches!(v.observed, Some(Outcome::Passed | Outcome::TransportFailed))
        })
        .count();
    let errors = report
        .verdicts
        .iter()
        .filter(|v| {
            matches!(
                v.observed,
                Some(Outcome::InfrastructureFailed | Outcome::Interrupted)
            )
        })
        .count();
    let skipped = report
        .verdicts
        .iter()
        .filter(|v| v.observed.is_none() || v.observed == Some(Outcome::Unsupported))
        .count();
    let mut xml=format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"adapter conformance\" tests=\"{}\" failures=\"{failed}\" errors=\"{errors}\" skipped=\"{skipped}\">\n",report.verdicts.len());
    for v in &report.verdicts {
        xml.push_str(&format!(
            "<testcase name=\"{} attempt {}\">",
            scenario::escape(&v.case),
            v.index
        ));
        if !v.passed {
            let tag = match v.observed {
                None | Some(Outcome::Unsupported) => "skipped",
                Some(Outcome::InfrastructureFailed | Outcome::Interrupted) => "error",
                _ => "failure",
            };
            xml.push_str(&format!(
                "<{tag} message=\"{}\"/>",
                scenario::escape(&format!(
                    "expected {:?}, observed {:?}; {}",
                    v.expected,
                    v.observed,
                    v.messages.join("; ")
                ))
            ));
        }
        xml.push_str("</testcase>\n");
    }
    xml.push_str("</testsuite>\n");
    scenario::atomic_write(&artifacts.join("junit.xml"), xml.as_bytes())?;
    Ok(report)
}
