//! Declarative built-in experiments and explicit CI expectations.
use crate::{
    bench::{self, Options},
    lab::{Profile, RouterInput},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Instant,
};

pub const SCENARIO_SCHEMA_VERSION: u32 = 1;
pub const REPORT_SCHEMA_VERSION: u32 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub schema_version: u32,
    pub cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub a: Profile,
    pub b: Profile,
    pub router_input: RouterInput,
    pub timeout_seconds: f64,
    pub expect: Vec<Expectation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub pointer: String,
    pub equals: Value,
}
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    AssertionFailed,
    Inconclusive,
    InfrastructureFailed,
}
#[derive(Serialize, Deserialize)]
pub struct CaseReport {
    pub name: String,
    pub status: Status,
    pub messages: Vec<String>,
    pub observation: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_seconds: Option<f64>,
}
#[derive(Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub scenario: String,
    pub cases: Vec<CaseReport>,
    pub planned_cases: Vec<String>,
    pub active_case: Option<String>,
    pub complete: bool,
    pub interrupted: bool,
}
impl Report {
    pub fn exit_code(&self) -> i32 {
        if self.interrupted {
            return 130;
        }
        if !self.complete {
            return 3;
        }
        if self
            .cases
            .iter()
            .any(|c| c.status == Status::InfrastructureFailed)
        {
            2
        } else if self.cases.iter().any(|c| c.status == Status::Inconclusive) {
            3
        } else if self
            .cases
            .iter()
            .any(|c| c.status == Status::AssertionFailed)
        {
            1
        } else {
            0
        }
    }
    pub fn junit(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}",
            self.junit_fragment("natbench")
        )
    }
    pub(crate) fn junit_fragment(&self, suite_name: &str) -> String {
        let failures = self
            .cases
            .iter()
            .filter(|c| c.status == Status::AssertionFailed)
            .count();
        let errors = self
            .cases
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    Status::InfrastructureFailed | Status::Inconclusive
                )
            })
            .count()
            + usize::from(self.interrupted && self.active_case.is_some());
        let skipped = self.planned_cases.len()
            - self.cases.len()
            - usize::from(self.interrupted && self.active_case.is_some());
        let mut xml = format!("<testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\">\n", escape(suite_name), self.planned_cases.len());
        for name in &self.planned_cases {
            let case = self.cases.iter().find(|case| case.name == *name);
            let time = case
                .and_then(|case| case.elapsed_seconds)
                .map(|seconds| format!(" time=\"{seconds:.6}\""))
                .unwrap_or_default();
            xml.push_str(&format!("<testcase name=\"{}\"{time}>", escape(name)));
            if let Some(case) = case {
                let tag = match case.status {
                    Status::Passed => None,
                    Status::AssertionFailed => Some("failure"),
                    _ => Some("error"),
                };
                if let Some(tag) = tag {
                    xml.push_str(&format!(
                        "<{tag} message=\"{}\"/>",
                        escape(&case.messages.join("; "))
                    ));
                }
            } else if self.interrupted && self.active_case.as_ref() == Some(name) {
                xml.push_str("<error type=\"interrupted\" message=\"suite interrupted while this case was running\"/>");
            } else {
                xml.push_str("<skipped message=\"case has not completed\"/>");
            }
            xml.push_str("</testcase>\n");
        }
        xml.push_str("</testsuite>\n");
        xml
    }
}
pub(crate) fn escape(input: &str) -> String {
    input
        .chars()
        .filter(|c| matches!(*c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'))
        .flat_map(|c| match c {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            '\'' => "&apos;".chars().collect(),
            _ => vec![c],
        })
        .collect()
}
impl Suite {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == SCENARIO_SCHEMA_VERSION,
            "unsupported scenario schema_version"
        );
        anyhow::ensure!(
            !self.cases.is_empty() && self.cases.len() <= 100,
            "suite must have 1–100 cases"
        );
        let mut names = std::collections::HashSet::new();
        for case in &self.cases {
            anyhow::ensure!(
                !case.name.trim().is_empty() && names.insert(&case.name),
                "case names must be nonempty and unique"
            );
            anyhow::ensure!(
                case.timeout_seconds.is_finite()
                    && case.timeout_seconds > 0.
                    && case.timeout_seconds <= 3600.,
                "invalid timeout for {}",
                case.name
            );
            anyhow::ensure!(
                !case.expect.is_empty(),
                "{} must declare expectations",
                case.name
            );
            for expectation in &case.expect {
                anyhow::ensure!(
                    expectation.pointer.starts_with('/'),
                    "expectation must use an absolute JSON pointer"
                );
                let mut chars = expectation.pointer.chars();
                while let Some(c) = chars.next() {
                    if c == '~' {
                        anyhow::ensure!(
                            matches!(chars.next(), Some('0' | '1')),
                            "invalid JSON pointer escape"
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub fn evaluate(
        &self,
        source: String,
        experiment: impl FnMut(&Case) -> Result<Value>,
    ) -> Result<Report> {
        self.evaluate_checkpointed(
            source,
            experiment,
            |_| Ok(()),
            || crate::cancellation().load(Ordering::Relaxed),
        )
    }
    fn evaluate_checkpointed(
        &self,
        source: String,
        mut experiment: impl FnMut(&Case) -> Result<Value>,
        checkpoint: impl FnMut(&Report) -> Result<()>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Report> {
        self.validate()?;
        execute_plan(
            source,
            self.cases.iter().map(|c| c.name.clone()).collect(),
            |index| {
                let case = &self.cases[index];
                let observation = experiment(case)?;
                let mut messages = Vec::new();
                let mut missing = false;
                for expectation in &case.expect {
                    match observation.pointer(&expectation.pointer) {
                        None => {
                            missing = true;
                            messages.push(format!("missing observation {}", expectation.pointer));
                        }
                        Some(actual) if *actual != expectation.equals => messages.push(format!(
                            "{}: expected {}, observed {}",
                            expectation.pointer, expectation.equals, actual
                        )),
                        _ => {}
                    }
                }
                let status = if missing {
                    Status::Inconclusive
                } else if messages.is_empty() {
                    Status::Passed
                } else {
                    Status::AssertionFailed
                };
                Ok(CaseReport {
                    name: case.name.clone(),
                    status,
                    messages,
                    observation: Some(observation),
                    elapsed_seconds: None,
                })
            },
            checkpoint,
            cancelled,
        )
    }
}
fn execute_plan(
    source: String,
    names: Vec<String>,
    mut experiment: impl FnMut(usize) -> Result<CaseReport>,
    mut checkpoint: impl FnMut(&Report) -> Result<()>,
    cancelled: impl Fn() -> bool,
) -> Result<Report> {
    let mut report = Report {
        schema_version: REPORT_SCHEMA_VERSION,
        scenario: source,
        cases: Vec::new(),
        planned_cases: names,
        active_case: None,
        complete: false,
        interrupted: false,
    };
    checkpoint(&report)?;
    for index in 0..report.planned_cases.len() {
        if cancelled() {
            report.interrupted = true;
            break;
        }
        let name = report.planned_cases[index].clone();
        report.active_case = Some(name.clone());
        checkpoint(&report)?;
        let started = Instant::now();
        let result = experiment(index);
        if result.is_err() && cancelled() {
            report.interrupted = true;
            break;
        }
        let mut case = result.unwrap_or_else(|error| CaseReport {
            name,
            status: Status::InfrastructureFailed,
            messages: vec![format!("{error:#}")],
            observation: None,
            elapsed_seconds: None,
        });
        case.elapsed_seconds = Some(started.elapsed().as_secs_f64());
        report.cases.push(case);
        report.active_case = None;
        checkpoint(&report)?;
        if cancelled() {
            report.interrupted = true;
            break;
        }
    }
    report.complete = report.cases.len() == report.planned_cases.len();
    checkpoint(&report)?;
    Ok(report)
}
pub(crate) fn run_application_cases(
    source: String,
    names: Vec<String>,
    experiment: impl FnMut(usize) -> Result<CaseReport>,
    artifacts: &Path,
) -> Result<Report> {
    execute_plan(
        source,
        names,
        experiment,
        |report| save_checkpoint(artifacts, report),
        || crate::cancellation().load(Ordering::Relaxed),
    )
}

pub(crate) struct Prepared {
    source: PathBuf,
    pub(crate) input: Vec<u8>,
    suite: PreparedSuite,
}
enum PreparedSuite {
    Builtin(Suite),
    Application(crate::application::Plan),
}
pub(crate) fn prepare(path: &Path) -> Result<Prepared> {
    let input = fs::read(path).context("read scenario")?;
    let header: Value = serde_json::from_slice(&input).context("parse scenario")?;
    let suite = if matches!(header["schema_version"].as_u64(), Some(2 | 3)) {
        PreparedSuite::Application(crate::application::Plan::prepare(path, &input)?)
    } else {
        let suite: Suite = serde_json::from_slice(&input).context("parse scenario")?;
        suite.validate()?;
        PreparedSuite::Builtin(suite)
    };
    Ok(Prepared {
        source: path.to_owned(),
        input,
        suite,
    })
}
impl Prepared {
    pub(crate) fn names(&self) -> Vec<String> {
        match &self.suite {
            PreparedSuite::Builtin(suite) => {
                suite.cases.iter().map(|case| case.name.clone()).collect()
            }
            PreparedSuite::Application(plan) => plan.names(),
        }
    }
    pub(crate) fn run(&self, artifacts: &Path) -> Result<Report> {
        fs::create_dir(artifacts)
            .context("artifact directory must be new and its parent must exist")?;
        // Save the validated bytes, even if the source file changes during execution.
        fs::write(artifacts.join("scenario.json"), &self.input)?;
        let source = self.source.display().to_string();
        match &self.suite {
            PreparedSuite::Application(plan) => plan.run(source, artifacts),
            PreparedSuite::Builtin(suite) => suite.evaluate_checkpointed(
                source,
                |case| {
                    let mut options = Options::current_exe()?;
                    options.a = case.a;
                    options.b = case.b;
                    options.router_input = case.router_input;
                    options.timeout_seconds = case.timeout_seconds;
                    bench::benchmark(&options)
                },
                |report| save_checkpoint(artifacts, report),
                || crate::cancellation().load(Ordering::Relaxed),
            ),
        }
    }
}
pub fn run(path: &Path, artifacts: &Path) -> Result<Report> {
    prepare(path)?.run(artifacts)
}
pub(crate) fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let temporary = path.with_extension("checkpoint.tmp");
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("checkpoint {}", path.display()))
}
fn save_checkpoint(artifacts: &Path, report: &Report) -> Result<()> {
    // Each file is replaced atomically. JSON is authoritative if a crash occurs
    // between these two writes; readers must not assume a multi-file transaction.
    atomic_write(
        &artifacts.join("report.json"),
        &serde_json::to_vec_pretty(report)?,
    )?;
    atomic_write(&artifacts.join("junit.xml"), report.junit().as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn suite() -> Suite {
        serde_json::from_value(json!({"schema_version":1,"cases":[{"name":"direct & <peer>","a":"preserve","b":"preserve","router_input":"drop","timeout_seconds":1,"expect":[{"pointer":"/direct","equals":true}]}]})).unwrap()
    }
    #[test]
    fn distinguishes_verdicts_from_fixture_failures() {
        let s = suite();
        assert_eq!(
            s.evaluate("test".into(), |_| Ok(json!({"direct":true})))
                .unwrap()
                .exit_code(),
            0
        );
        assert_eq!(
            s.evaluate("test".into(), |_| Ok(json!({"direct":false})))
                .unwrap()
                .exit_code(),
            1
        );
        assert_eq!(
            s.evaluate("test".into(), |_| anyhow::bail!("fixture unavailable"))
                .unwrap()
                .exit_code(),
            2
        );
        let missing = s.evaluate("test".into(), |_| Ok(json!({}))).unwrap();
        assert_eq!(missing.exit_code(), 3);
        assert!(missing
            .junit()
            .contains("name=\"direct &amp; &lt;peer&gt;\""));
        assert!(missing.junit().contains("<error"));
    }
    #[test]
    fn rejects_ambiguous_or_unasserted_suites_before_execution() {
        let mut s = suite();
        s.cases[0].expect.clear();
        assert!(s
            .evaluate("test".into(), |_| panic!("must not execute"))
            .is_err());
        let mut s = suite();
        s.cases[0].expect[0].pointer = "/bad~2escape".into();
        assert!(s.validate().is_err());
    }
    #[test]
    fn checkpoints_keep_completed_cases_when_the_next_case_is_interrupted() {
        use std::cell::Cell;
        let mut suite = suite();
        suite.cases.push(serde_json::from_value(json!({"name":"second", "a":"random", "b":"random", "router_input":"drop", "timeout_seconds":30, "expect":[{"pointer":"/direct","equals":false}]})).unwrap());
        let cancelled = Cell::new(false);
        let mut snapshots = Vec::new();
        let report = suite
            .evaluate_checkpointed(
                "test".into(),
                |case| {
                    if case.name == "second" {
                        cancelled.set(true);
                        anyhow::bail!("interrupted");
                    }
                    Ok(json!({"direct":true}))
                },
                |report| {
                    snapshots.push(serde_json::to_value(report)?);
                    Ok(())
                },
                || cancelled.get(),
            )
            .unwrap();
        assert_eq!(report.schema_version, REPORT_SCHEMA_VERSION);
        assert_eq!(report.exit_code(), 130);
        assert_eq!(report.cases.len(), 1);
        assert_eq!(report.active_case.as_deref(), Some("second"));
        assert!(!report.complete);
        assert!(snapshots
            .iter()
            .any(|s| s["active_case"] == "second" && s["cases"].as_array().unwrap().len() == 1));
        assert!(report.junit().contains("type=\"interrupted\""));
    }
    #[test]
    fn checkpoint_io_failure_prevents_starting_an_experiment() {
        assert!(suite()
            .evaluate_checkpointed(
                "test".into(),
                |_| panic!("must not execute"),
                |_| anyhow::bail!("disk unavailable"),
                || false
            )
            .is_err());
    }
    #[test]
    fn completed_report_is_distinct_from_in_progress_checkpoints() {
        let mut snapshots = Vec::new();
        let report = suite()
            .evaluate_checkpointed(
                "test".into(),
                |_| Ok(json!({"direct":true})),
                |report| {
                    snapshots.push(serde_json::to_value(report)?);
                    Ok(())
                },
                || false,
            )
            .unwrap();
        assert!(report.complete);
        assert!(!report.interrupted);
        assert_eq!(snapshots[0]["complete"], false);
        assert_eq!(snapshots[0]["cases"], json!([]));
        assert_eq!(snapshots.last().unwrap()["complete"], true);
    }
    #[test]
    fn equality_keeps_numeric_representation_and_missing_null_distinct() {
        let mut s = suite();
        s.cases[0].expect[0].equals = json!(1);
        assert_eq!(
            s.evaluate("test".into(), |_| Ok(json!({"direct":1.0})))
                .unwrap()
                .exit_code(),
            1
        );
        s.cases[0].expect[0].equals = Value::Null;
        assert_eq!(
            s.evaluate("test".into(), |_| Ok(json!({"direct":null})))
                .unwrap()
                .exit_code(),
            0
        );
        assert_eq!(
            s.evaluate("test".into(), |_| Ok(json!({})))
                .unwrap()
                .exit_code(),
            3
        );
    }
}
