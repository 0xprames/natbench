//! Repeat a frozen scenario while preserving each attempt's evidence.
use crate::scenario::{self, CaseReport, Report, Status};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::atomic::Ordering,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Serialize)]
pub struct Environment {
    natbench_version: &'static str,
    os: &'static str,
    architecture: &'static str,
    kernel_release: Option<String>,
    effective_uid: u32,
    recorded_at_unix_seconds: u64,
}
impl Environment {
    fn current() -> Result<Self> {
        Ok(Self {
            natbench_version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            kernel_release: fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|value| value.trim().to_owned()),
            // SAFETY: geteuid has no arguments or memory-safety requirements.
            effective_uid: unsafe { libc::geteuid() },
            recorded_at_unix_seconds: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        })
    }
}
#[derive(Serialize, Debug)]
pub struct Timing {
    pub samples: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
}
impl Timing {
    fn from_samples(mut samples: Vec<f64>) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        samples.sort_by(f64::total_cmp);
        let length = samples.len();
        let median = if length.is_multiple_of(2) {
            (samples[length / 2 - 1] + samples[length / 2]) / 2.
        } else {
            samples[length / 2]
        };
        Some(Self {
            samples: length,
            min: samples[0],
            max: samples[length - 1],
            mean: samples.iter().sum::<f64>() / length as f64,
            median,
            // Nearest-rank percentile, including the maximum for fewer than 20 samples.
            p95: samples[(length * 95).div_ceil(100) - 1],
        })
    }
}
#[derive(Serialize, Default, Debug, PartialEq)]
pub struct Verdicts {
    pub passed: u32,
    pub assertion_failed: u32,
    pub inconclusive: u32,
    pub infrastructure_failed: u32,
}
impl Verdicts {
    fn add(&mut self, status: Status) {
        match status {
            Status::Passed => self.passed += 1,
            Status::AssertionFailed => self.assertion_failed += 1,
            Status::Inconclusive => self.inconclusive += 1,
            Status::InfrastructureFailed => self.infrastructure_failed += 1,
        }
    }
}
#[derive(Serialize)]
pub struct CaseSummary {
    pub name: String,
    pub requested_runs: u32,
    pub completed_runs: u32,
    pub interrupted_runs: u32,
    pub unfinished_runs: u32,
    pub unreported_runs: u32,
    pub verdicts: Verdicts,
    pub timing_seconds: BTreeMap<Status, Timing>,
}
#[derive(Serialize)]
pub struct Sample {
    pub name: String,
    pub status: Status,
    pub messages: Vec<String>,
    pub elapsed_seconds: Option<f64>,
}
#[derive(Serialize)]
pub struct Run {
    pub index: u32,
    pub artifacts_directory: String,
    pub complete: bool,
    pub interrupted: bool,
    pub exit_code: i32,
    pub elapsed_seconds: f64,
    pub active_case: Option<String>,
    pub error: Option<String>,
    pub cases: Vec<Sample>,
}
#[derive(Serialize)]
pub struct Summary {
    pub schema_version: u32,
    pub kind: &'static str,
    pub scenario: String,
    pub requested_runs: u32,
    pub completed_runs: u32,
    pub active_run: Option<u32>,
    pub complete: bool,
    pub interrupted: bool,
    pub environment: Environment,
    pub completed_run_seconds: Option<Timing>,
    pub cases: Vec<CaseSummary>,
    pub runs: Vec<Run>,
}
impl Summary {
    fn refresh(&mut self) {
        self.completed_runs = self
            .runs
            .iter()
            .filter(|run| run.complete && run.error.is_none())
            .count() as u32;
        self.complete = self.completed_runs == self.requested_runs;
        self.completed_run_seconds = Timing::from_samples(
            self.runs
                .iter()
                .filter(|run| run.complete && run.error.is_none())
                .map(|run| run.elapsed_seconds)
                .collect(),
        );
        for case in &mut self.cases {
            case.completed_runs = 0;
            case.interrupted_runs = 0;
            case.unfinished_runs = 0;
            case.verdicts = Verdicts::default();
            let mut timings: BTreeMap<Status, Vec<f64>> = BTreeMap::new();
            for run in &self.runs {
                if let Some(sample) = run.cases.iter().find(|sample| sample.name == case.name) {
                    case.completed_runs += 1;
                    case.verdicts.add(sample.status);
                    if let Some(elapsed) = sample.elapsed_seconds {
                        timings.entry(sample.status).or_default().push(elapsed);
                    }
                } else if run.active_case.as_ref() == Some(&case.name) {
                    if run.interrupted {
                        case.interrupted_runs += 1;
                    } else {
                        case.unfinished_runs += 1;
                    }
                }
            }
            case.unreported_runs = case.requested_runs
                - case.completed_runs
                - case.interrupted_runs
                - case.unfinished_runs;
            case.timing_seconds = timings
                .into_iter()
                .filter_map(|(status, values)| {
                    Timing::from_samples(values).map(|timing| (status, timing))
                })
                .collect();
        }
    }
    pub fn exit_code(&self) -> i32 {
        if self.interrupted {
            130
        } else if self.runs.iter().any(|run| run.exit_code == 2) {
            2
        } else if !self.complete || self.runs.iter().any(|run| run.exit_code == 3) {
            3
        } else if self.runs.iter().any(|run| run.exit_code == 1) {
            1
        } else {
            0
        }
    }
    pub fn readable(&self) -> String {
        let mut text = format!(
            "{} / {} runs completed{}\n",
            self.completed_runs,
            self.requested_runs,
            if self.interrupted {
                "; interrupted"
            } else {
                ""
            }
        );
        for case in &self.cases {
            text.push_str(&format!("{}: {} passed, {} assertion failures, {} fixture failures, {} inconclusive, {} interrupted, {} unfinished, {} unreported\n", case.name, case.verdicts.passed, case.verdicts.assertion_failed, case.verdicts.infrastructure_failed, case.verdicts.inconclusive, case.interrupted_runs, case.unfinished_runs, case.unreported_runs));
            if let Some(timing) = case.timing_seconds.get(&Status::Passed) {
                text.push_str(&format!(
                    "  passing case time: median {:.3}s, p95 {:.3}s ({} samples)\n",
                    timing.median, timing.p95, timing.samples
                ));
            }
        }
        for run in &self.runs {
            if let Some(error) = &run.error {
                text.push_str(&format!("run-{:03} runner: {error}\n", run.index));
            }
        }
        text
    }
    pub fn junit(&self) -> String {
        let names: Vec<String> = self.cases.iter().map(|case| case.name.clone()).collect();
        let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"natbench repetitions\">\n".to_owned();
        for index in 1..=self.requested_runs {
            let run = self.runs.iter().find(|run| run.index == index);
            let report = Report {
                schema_version: scenario::REPORT_SCHEMA_VERSION,
                scenario: self.scenario.clone(),
                planned_cases: names.clone(),
                cases: run
                    .map(|run| {
                        run.cases
                            .iter()
                            .map(|sample| CaseReport {
                                name: sample.name.clone(),
                                status: sample.status,
                                messages: sample.messages.clone(),
                                observation: None,
                                elapsed_seconds: sample.elapsed_seconds,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                active_case: run.and_then(|run| run.active_case.clone()),
                complete: run.is_some_and(|run| run.complete),
                interrupted: run.is_some_and(|run| run.interrupted),
            };
            xml.push_str(&report.junit_fragment(&format!("run-{index:03}")));
        }
        let errors: Vec<_> = self
            .runs
            .iter()
            .filter_map(|run| run.error.as_ref().map(|error| (run.index, error)))
            .collect();
        let runner_errors = errors.len() + usize::from(self.interrupted);
        if runner_errors > 0 {
            xml.push_str(&format!("<testsuite name=\"repeat runner\" tests=\"{}\" errors=\"{}\" failures=\"0\" skipped=\"0\">\n", runner_errors, runner_errors));
            for (index, error) in errors {
                xml.push_str(&format!("<testcase name=\"run-{index:03} runner\"><error type=\"infrastructure\" message=\"{}\"/></testcase>\n", scenario::escape(error)));
            }
            if self.interrupted {
                xml.push_str("<testcase name=\"repetitions\"><error type=\"interrupted\" message=\"repetition execution interrupted\"/></testcase>\n");
            }
            xml.push_str("</testsuite>\n");
        }
        xml.push_str("</testsuites>\n");
        xml
    }
    fn save(&self, artifacts: &Path) -> Result<()> {
        scenario::atomic_write(
            &artifacts.join("summary.json"),
            &serde_json::to_vec_pretty(self)?,
        )?;
        scenario::atomic_write(&artifacts.join("junit.xml"), self.junit().as_bytes())
    }
}
fn checkpoint(directory: &Path, names: &[String]) -> Option<Report> {
    let bytes = fs::read(directory.join("report.json")).ok()?;
    let report: Report = serde_json::from_slice(&bytes).ok()?;
    let valid = report.schema_version == scenario::REPORT_SCHEMA_VERSION
        && report.planned_cases == names
        && report.cases.len() <= names.len()
        && report.cases.iter().zip(names).all(|(case, name)| {
            case.name == *name
                && case
                    .elapsed_seconds
                    .is_none_or(|value| value.is_finite() && value >= 0.)
        })
        && (!report.complete || report.cases.len() == names.len())
        && report
            .active_case
            .as_ref()
            .is_none_or(|name| names.get(report.cases.len()) == Some(name));
    valid.then_some(report)
}
fn execute(
    source: String,
    names: Vec<String>,
    input: &[u8],
    runs: u32,
    artifacts: &Path,
    mut run: impl FnMut(&Path) -> Result<Report>,
    cancelled: impl Fn() -> bool,
) -> Result<Summary> {
    anyhow::ensure!((1..=100).contains(&runs), "runs must be 1–100");
    let environment = Environment::current()?;
    fs::create_dir(artifacts)
        .context("artifact directory must be new and its parent must exist")?;
    fs::write(artifacts.join("scenario.json"), input)?;
    let mut summary = Summary {
        schema_version: 1,
        kind: "repetition_report",
        scenario: source,
        requested_runs: runs,
        completed_runs: 0,
        active_run: None,
        complete: false,
        interrupted: false,
        environment,
        completed_run_seconds: None,
        cases: names
            .iter()
            .map(|name| CaseSummary {
                name: name.clone(),
                requested_runs: runs,
                completed_runs: 0,
                interrupted_runs: 0,
                unfinished_runs: 0,
                unreported_runs: runs,
                verdicts: Verdicts::default(),
                timing_seconds: BTreeMap::new(),
            })
            .collect(),
        runs: Vec::new(),
    };
    summary.save(artifacts)?;
    for index in 1..=runs {
        if cancelled() {
            summary.interrupted = true;
            break;
        }
        summary.active_run = Some(index);
        summary.save(artifacts)?;
        let directory_name = format!("run-{index:03}");
        let directory = artifacts.join(&directory_name);
        let started = Instant::now();
        let result = run(&directory);
        let elapsed_seconds = started.elapsed().as_secs_f64();
        let (report, error) = match result {
            Ok(report) => (Some(report), None),
            Err(error) => (checkpoint(&directory, &names), Some(format!("{error:#}"))),
        };
        let interrupted = cancelled() || report.as_ref().is_some_and(|report| report.interrupted);
        let exit_code = if interrupted {
            130
        } else if error.is_some() {
            2
        } else {
            report.as_ref().unwrap().exit_code()
        };
        let complete = report.as_ref().is_some_and(|report| report.complete);
        let active_case = report
            .as_ref()
            .and_then(|report| report.active_case.clone());
        let samples = report
            .map(|report| {
                report
                    .cases
                    .into_iter()
                    .map(|case| Sample {
                        name: case.name,
                        status: case.status,
                        messages: case.messages,
                        elapsed_seconds: case.elapsed_seconds,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let failed_runner = error.is_some();
        summary.runs.push(Run {
            index,
            artifacts_directory: directory_name,
            complete,
            interrupted,
            exit_code,
            elapsed_seconds,
            active_case,
            error,
            cases: samples,
        });
        summary.active_run = None;
        summary.interrupted |= interrupted;
        summary.refresh();
        summary.save(artifacts)?;
        if interrupted || failed_runner || !complete {
            break;
        }
    }
    summary.interrupted |= cancelled();
    summary.refresh();
    summary.save(artifacts)?;
    Ok(summary)
}
/// Validate once and run every repetition with the same captured input and fresh fixtures.
pub fn run(path: &Path, artifacts: &Path, runs: u32) -> Result<Summary> {
    anyhow::ensure!((1..=100).contains(&runs), "runs must be 1–100");
    let prepared = scenario::prepare(path)?;
    execute(
        path.display().to_string(),
        prepared.names(),
        &prepared.input,
        runs,
        artifacts,
        |directory| prepared.run(directory),
        || crate::cancellation().load(Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, path::PathBuf};
    struct Temp(PathBuf);
    impl Temp {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "natbench-repeat-unit-{}-{label}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn names() -> Vec<String> {
        vec!["a & peer".into(), "b".into()]
    }
    fn report(statuses: &[Status], duration: f64) -> Report {
        Report {
            schema_version: scenario::REPORT_SCHEMA_VERSION,
            scenario: "test".into(),
            planned_cases: names(),
            active_case: None,
            complete: statuses.len() == names().len(),
            interrupted: false,
            cases: statuses
                .iter()
                .zip(names())
                .map(|(status, name)| CaseReport {
                    name,
                    status: *status,
                    messages: vec![],
                    observation: None,
                    elapsed_seconds: Some(duration),
                })
                .collect(),
        }
    }
    #[test]
    fn timing_reports_even_median_and_nearest_rank_percentile() {
        assert!(Timing::from_samples(vec![]).is_none());
        let timing = Timing::from_samples(vec![15., 3., 9., 5.]).unwrap();
        assert_eq!(
            (
                timing.samples,
                timing.min,
                timing.max,
                timing.mean,
                timing.median,
                timing.p95
            ),
            (4, 3., 15., 8., 7., 15.)
        );
        let timing = Timing::from_samples((1..=100).map(f64::from).collect()).unwrap();
        assert_eq!((timing.mean, timing.median, timing.p95), (50.5, 50.5, 95.));
    }
    #[test]
    fn all_verdicts_count_and_failed_runs_do_not_disappear() {
        let root = Temp::new("verdicts");
        let mut index = 0;
        let statuses = [
            [Status::Passed, Status::InfrastructureFailed],
            [Status::AssertionFailed, Status::Passed],
            [Status::Passed, Status::Inconclusive],
        ];
        let summary = execute(
            "test".into(),
            names(),
            b"frozen input",
            3,
            &root.0.join("artifacts"),
            |_| {
                index += 1;
                Ok(report(&statuses[index - 1], index as f64))
            },
            || false,
        )
        .unwrap();
        assert_eq!(summary.exit_code(), 2);
        assert!(summary.complete);
        assert_eq!(summary.completed_runs, 3);
        assert_eq!(
            summary.cases[0].verdicts,
            Verdicts {
                passed: 2,
                assertion_failed: 1,
                ..Verdicts::default()
            }
        );
        assert_eq!(
            summary.cases[1].verdicts,
            Verdicts {
                passed: 1,
                infrastructure_failed: 1,
                inconclusive: 1,
                ..Verdicts::default()
            }
        );
        assert_eq!(summary.cases[0].timing_seconds[&Status::Passed].mean, 2.);
        assert_eq!(summary.cases[0].timing_seconds[&Status::Passed].samples, 2);
        assert_eq!(
            summary.cases[0].timing_seconds[&Status::AssertionFailed].samples,
            1
        );
        assert_eq!(summary.cases[0].unreported_runs, 0);
        assert_eq!(summary.junit().matches("<?xml").count(), 1);
        assert!(summary.junit().contains("a &amp; peer"));
        assert!(summary.junit().contains("time=\"1.000000\""));
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../schemas/repetition-report-v1.schema.json"))
                .unwrap();
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(&serde_json::to_value(&summary).unwrap())
            .unwrap();
    }
    #[test]
    fn interruption_keeps_completed_samples_and_future_runs_unreported() {
        let root = Temp::new("cancel");
        let artifacts = root.0.join("artifacts");
        let cancelled = Cell::new(false);
        let mut index = 0;
        let summary = execute(
            "test".into(),
            names(),
            b"input",
            3,
            &artifacts,
            |_| {
                index += 1;
                let checkpoint: serde_json::Value =
                    serde_json::from_slice(&fs::read(artifacts.join("summary.json"))?).unwrap();
                assert_eq!(checkpoint["active_run"], index);
                if index == 1 {
                    return Ok(report(&[Status::Passed, Status::Passed], 1.));
                }
                let mut partial = report(&[Status::AssertionFailed], 2.);
                partial.active_case = Some("b".into());
                partial.interrupted = true;
                cancelled.set(true);
                Ok(partial)
            },
            || cancelled.get(),
        )
        .unwrap();
        assert_eq!(summary.exit_code(), 130);
        assert!(!summary.complete);
        assert_eq!(summary.completed_runs, 1);
        assert_eq!(summary.runs.len(), 2);
        assert_eq!(summary.active_run, None);
        assert_eq!(summary.cases[0].completed_runs, 2);
        assert_eq!(summary.cases[0].unreported_runs, 1);
        assert_eq!(summary.cases[1].completed_runs, 1);
        assert_eq!(summary.cases[1].interrupted_runs, 1);
        assert_eq!(summary.cases[1].unreported_runs, 1);
        assert_eq!(summary.cases[1].timing_seconds[&Status::Passed].samples, 1);
        assert!(summary.junit().contains("type=\"interrupted\""));
        assert!(summary.junit().contains("name=\"run-003\""));
    }
    #[test]
    fn runner_io_failure_keeps_checkpoint_and_cannot_be_a_passing_summary() {
        let root = Temp::new("io");
        let artifacts = root.0.join("artifacts");
        let summary = execute(
            "test".into(),
            names(),
            b"input",
            2,
            &artifacts,
            |directory| {
                fs::create_dir(directory)?;
                let mut partial = report(&[Status::Passed], 1.);
                partial.active_case = Some("b".into());
                fs::write(directory.join("report.json"), serde_json::to_vec(&partial)?)?;
                anyhow::bail!("disk <full>")
            },
            || false,
        )
        .unwrap();
        assert_eq!(summary.exit_code(), 2);
        assert_eq!(summary.completed_runs, 0);
        assert!(!summary.complete);
        assert_eq!(summary.cases[0].verdicts.passed, 1);
        assert_eq!(summary.cases[1].unfinished_runs, 1);
        assert_eq!(summary.cases[1].unreported_runs, 1);
        assert!(summary.junit().contains("disk &lt;full&gt;"));
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(artifacts.join("summary.json")).unwrap()).unwrap();
        assert_eq!(saved["runs"][0]["error"], "disk <full>");
    }
    #[test]
    fn malformed_checkpoint_is_not_counted_as_completed_evidence() {
        let root = Temp::new("invalid-checkpoint");
        let directory = &root.0;
        let mut malformed = report(&[Status::Passed, Status::Passed], 1.);
        malformed.cases[1].name = "a & peer".into();
        fs::write(
            directory.join("report.json"),
            serde_json::to_vec(&malformed).unwrap(),
        )
        .unwrap();
        assert!(checkpoint(directory, &names()).is_none());
    }
}
