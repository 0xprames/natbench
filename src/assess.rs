//! Offline requirements over raw comparison attempts, with explicit baseline compatibility.
use crate::{
    compare::{Attempt, CaseConditions, Outcome, Planned},
    repeat::Timing,
    scenario,
};
use anyhow::{ensure, Context, Result};
use natbench_transport_protocol::{Implementation, Request, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::atomic::Ordering,
};

const REPORT_LIMIT: u64 = 128 * 1024 * 1024;
const POLICY_LIMIT: u64 = 1024 * 1024;
type Key = (String, String);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    kind: String,
    cohorts: Vec<Rule>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    case: String,
    adapter: String,
    min_attempts: u32,
    min_successful_attempts: u32,
    min_message_rtt_samples: u32,
    min_delivery_rate: f64,
    #[serde(default, deserialize_with = "present")]
    max_message_rtt_p95_seconds: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    min_bulk_goodput_bytes_per_second: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    baseline: Option<Relative>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Relative {
    #[serde(default, deserialize_with = "present")]
    max_delivery_rate_drop: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    max_message_rtt_p95_increase_fraction: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    max_bulk_goodput_drop_fraction: Option<f64>,
    allowed_metadata_changes: Vec<AllowedChange>,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum AllowedChange {
    ImplementationVersion,
    AdapterVersion,
    AdapterSourceSha256,
    QuicEngineVersion,
}
impl AllowedChange {
    fn field(self) -> &'static str {
        match self {
            Self::ImplementationVersion => "implementation_version",
            Self::AdapterVersion => "adapter_version",
            Self::AdapterSourceSha256 => "adapter_source_sha256",
            Self::QuicEngineVersion => "quic_engine_version",
        }
    }
}
fn present<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
impl Policy {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.kind == "transport_regression_policy",
            "unsupported regression policy"
        );
        ensure!(
            (1..=256).contains(&self.cohorts.len()),
            "policy needs 1–256 cohorts"
        );
        let mut keys = BTreeSet::new();
        for rule in &self.cohorts {
            ensure!(
                !rule.case.trim().is_empty()
                    && !rule.adapter.trim().is_empty()
                    && keys.insert((rule.case.clone(), rule.adapter.clone())),
                "policy cohorts must have unique nonempty case/adapter names"
            );
            ensure!(
                (1..=100).contains(&rule.min_attempts)
                    && (1..=100).contains(&rule.min_successful_attempts)
                    && (1..=100_000).contains(&rule.min_message_rtt_samples),
                "invalid policy sample requirements"
            );
            bounded(rule.min_delivery_rate, 0., 1.)?;
            if let Some(value) = rule.max_message_rtt_p95_seconds {
                positive(value, 60.)?;
            }
            if let Some(value) = rule.min_bulk_goodput_bytes_per_second {
                positive(value, f64::MAX)?;
            }
            if let Some(relative) = &rule.baseline {
                ensure!(
                    relative.max_delivery_rate_drop.is_some()
                        || relative.max_message_rtt_p95_increase_fraction.is_some()
                        || relative.max_bulk_goodput_drop_fraction.is_some(),
                    "baseline rule needs at least one relative requirement"
                );
                if let Some(value) = relative.max_delivery_rate_drop {
                    bounded(value, 0., 1.)?;
                }
                if let Some(value) = relative.max_message_rtt_p95_increase_fraction {
                    bounded(value, 0., 100.)?;
                }
                if let Some(value) = relative.max_bulk_goodput_drop_fraction {
                    bounded(value, 0., 1.)?;
                }
                ensure!(
                    relative
                        .allowed_metadata_changes
                        .iter()
                        .copied()
                        .collect::<BTreeSet<_>>()
                        .len()
                        == relative.allowed_metadata_changes.len(),
                    "duplicate allowed metadata change"
                );
            }
        }
        Ok(())
    }
}
fn bounded(value: f64, min: f64, max: f64) -> Result<()> {
    ensure!(
        value.is_finite() && (min..=max).contains(&value),
        "policy value must be finite and within {min}..={max}"
    );
    Ok(())
}
fn positive(value: f64, max: f64) -> Result<()> {
    bounded(value, 0., max)?;
    ensure!(value > 0., "policy value must be positive");
    Ok(())
}

#[derive(Deserialize)]
struct Snapshot {
    schema_version: u32,
    kind: String,
    topology: String,
    workload_semantics: String,
    environment: Value,
    case_conditions: Vec<CaseConditions>,
    planned: Vec<Planned>,
    attempts: Vec<Attempt>,
    active_attempt: Option<usize>,
    complete: bool,
    interrupted: bool,
}
impl Snapshot {
    fn keys(&self) -> BTreeSet<Key> {
        self.planned
            .iter()
            .map(|p| (p.case.clone(), p.adapter.clone()))
            .collect()
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.kind == "transport_comparison_report",
            "unsupported comparison report"
        );
        ensure!(self.topology == "ipv4_client_a_to_wan" && self.workload_semantics == "direct reliable streams; fresh endpoints; bulk includes receiver verification and acknowledgement", "unsupported comparison semantics");
        ensure!(
            self.environment.is_object()
                && ["os", "architecture"]
                    .iter()
                    .all(|field| self.environment[field]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())),
            "invalid report environment"
        );
        for field in ["kernel_release", "cpu_model"] {
            ensure!(
                self.environment[field].is_null()
                    || self.environment[field]
                        .as_str()
                        .is_some_and(|value| !value.is_empty()),
                "invalid environment {field}"
            );
        }
        ensure!(
            self.environment["available_parallelism"].is_null()
                || self.environment["available_parallelism"]
                    .as_u64()
                    .is_some_and(|value| value > 0),
            "invalid available parallelism"
        );
        let capture = &self.environment["packet_capture"];
        if !capture.is_null() {
            ensure!(
                capture.is_object()
                    && capture["packets_per_role"]
                        .as_u64()
                        .is_some_and(|value| (1..=100_000).contains(&value))
                    && capture["snaplen_bytes"] == 256
                    && capture["roles"] == serde_json::json!(["a", "b", "ra", "rb", "wan"]),
                "invalid capture options"
            );
        }
        ensure!(
            (1..=4096).contains(&self.planned.len()) && self.attempts.len() <= self.planned.len(),
            "invalid report attempt budget"
        );
        ensure!(
            self.complete == (self.attempts.len() == self.planned.len()),
            "report completion contradicts attempt coverage"
        );
        if let Some(active) = self.active_attempt {
            ensure!(
                active == self.attempts.len() && active < self.planned.len(),
                "invalid active attempt index"
            );
        }
        ensure!((1..=32).contains(&self.case_conditions.len()), "report needs case conditions with workload/deadline metadata; rerun with current natbench");
        let mut names = BTreeSet::new();
        for case in &self.case_conditions {
            ensure!(
                !case.name.trim().is_empty() && names.insert(case.name.clone()),
                "invalid report case identity"
            );
            let workload = case
                .workload
                .as_ref()
                .context("report lacks case workload metadata; rerun with current natbench")?;
            workload.validate()?;
            ensure!(
                case.deadline_ms
                    .is_some_and(|value| (1..=60_000).contains(&value)),
                "report lacks valid case deadline metadata; rerun with current natbench"
            );
            if let Some(network) = &case.network {
                network.client_to_server.validate()?;
                network.server_to_client.validate()?;
            }
        }
        let mut runs: BTreeMap<Key, BTreeSet<u32>> = BTreeMap::new();
        for (index, plan) in self.planned.iter().enumerate() {
            ensure!(
                plan.index == index
                    && names.contains(&plan.case)
                    && !plan.adapter.is_empty()
                    && (1..=100).contains(&plan.run),
                "invalid planned attempt identity"
            );
            ensure!(
                runs.entry((plan.case.clone(), plan.adapter.clone()))
                    .or_default()
                    .insert(plan.run),
                "duplicate planned case/adapter/run"
            );
        }
        ensure!(
            self.planned
                .iter()
                .map(|p| p.case.clone())
                .collect::<BTreeSet<_>>()
                == names,
            "case metadata differs from planned cases"
        );
        let adapters: BTreeSet<_> = self.planned.iter().map(|p| p.adapter.clone()).collect();
        ensure!(
            (2..=8).contains(&adapters.len()) && runs.len() == names.len() * adapters.len(),
            "comparison matrix has missing cohorts"
        );
        for values in runs.values() {
            ensure!(
                values.iter().copied().eq(1..=values.len() as u32),
                "planned repetition numbers must be contiguous"
            );
        }
        ensure!(runs.values().map(BTreeSet::len).collect::<BTreeSet<_>>().len()==1,"planned cohort repetition counts must match; filtered attempts are not a valid comparison matrix");
        for (index, attempt) in self.attempts.iter().enumerate() {
            ensure!(
                attempt.index == index,
                "attempts must form an ordered prefix of planned"
            );
            if attempt.outcome == Outcome::Interrupted {
                ensure!(
                    self.interrupted,
                    "interrupted attempt missing report interruption flag"
                );
            }
            ensure!(
                (attempt.outcome == Outcome::Passed) == attempt.measurement.is_some(),
                "measurement must exist exactly for a passing attempt"
            );
            if matches!(
                attempt.outcome,
                Outcome::Passed | Outcome::TransportFailed | Outcome::Unsupported
            ) {
                let info = attempt
                    .implementation
                    .as_ref()
                    .context("attempt lacks implementation identity")?;
                ensure!(
                    !info.name.trim().is_empty() && !info.version.trim().is_empty(),
                    "invalid implementation identity"
                );
            }
            if let Some(measurement) = &attempt.measurement {
                let case = self.case_for(&self.planned[index].case);
                let request = Request {
                    schema_version: 1,
                    kind: "transport_request".into(),
                    run_id: "0123456789abcdef0123456789abcdef".into(),
                    role: Role::Client,
                    listen_address: "0.0.0.0:0".parse()?,
                    peer_address: "198.18.0.1:9443".parse()?,
                    peer_file: "/tmp/assessment-peer.json".into(),
                    deadline_ms: case.deadline_ms.unwrap(),
                    workload: case.workload.clone().unwrap(),
                };
                measurement.validate(&request)?;
            }
        }
        Ok(())
    }
    fn case_for(&self, name: &str) -> &CaseConditions {
        self.case_conditions
            .iter()
            .find(|case| case.name == name)
            .unwrap()
    }
    fn exit_code(&self) -> i32 {
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
    fn observe(&self, key: &Key) -> Observation {
        let indices: Vec<_> = self
            .planned
            .iter()
            .filter(|p| (&p.case, &p.adapter) == (&key.0, &key.1))
            .map(|p| p.index)
            .collect();
        let mut counts = Counts {
            requested: indices.len(),
            ..Default::default()
        };
        let mut rtt = Vec::new();
        let mut goodput = Vec::new();
        let mut implementation = None;
        let mut changed = false;
        for index in &indices {
            let Some(attempt) = self.attempts.get(*index) else {
                counts.unreported += 1;
                continue;
            };
            match attempt.outcome {
                Outcome::Passed => counts.passed += 1,
                Outcome::TransportFailed => counts.transport_failed += 1,
                Outcome::Unsupported => counts.unsupported += 1,
                Outcome::InfrastructureFailed => counts.infrastructure_failed += 1,
                Outcome::Interrupted => counts.interrupted += 1,
            }
            if let Some(info) = &attempt.implementation {
                if let Some(previous) = &implementation {
                    changed |= previous != info;
                } else {
                    implementation = Some(info.clone());
                }
            }
            if let Some(measurement) = &attempt.measurement {
                rtt.extend(&measurement.message_rtt_seconds);
                goodput.push(f64::from(measurement.bulk_verified_bytes) / measurement.bulk_seconds);
            }
        }
        Observation {
            delivery_rate: counts.passed as f64 / counts.requested as f64,
            counts,
            attempt_indices: indices,
            implementation,
            implementation_changed: changed,
            message_rtt_seconds: Timing::from_samples(rtt),
            bulk_goodput_bytes_per_second: Timing::from_samples(goodput),
        }
    }
}

#[derive(Default, Serialize)]
struct Counts {
    requested: usize,
    passed: usize,
    transport_failed: usize,
    unsupported: usize,
    infrastructure_failed: usize,
    interrupted: usize,
    unreported: usize,
}
#[derive(Serialize)]
struct Observation {
    counts: Counts,
    delivery_rate: f64,
    attempt_indices: Vec<usize>,
    implementation: Option<Implementation>,
    implementation_changed: bool,
    message_rtt_seconds: Option<Timing>,
    bulk_goodput_bytes_per_second: Option<Timing>,
}
#[derive(Serialize)]
struct Check {
    name: String,
    passed: bool,
    observed: Option<f64>,
    required: Option<f64>,
    message: String,
}
#[derive(Serialize)]
struct Change {
    field: String,
    current: Value,
    baseline: Value,
    allowed: bool,
}
#[derive(Serialize)]
struct Cohort {
    case: String,
    adapter: String,
    exit_code: i32,
    current: Observation,
    baseline: Option<Observation>,
    checks: Vec<Check>,
    metadata_changes: Vec<Change>,
}
#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    kind: &'static str,
    current_report: String,
    baseline_report: Option<String>,
    current_source_exit_code: i32,
    baseline_source_exit_code: Option<i32>,
    complete: bool,
    interrupted: bool,
    exit_code: i32,
    cohorts: Vec<Cohort>,
}
impl Report {
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
    pub fn readable(&self) -> String {
        let mut text = String::new();
        for cohort in &self.cohorts {
            text.push_str(&format!(
                "{} / {}: {} ({}/{} delivered)\n",
                cohort.case,
                cohort.adapter,
                if cohort.exit_code == 0 {
                    "passed"
                } else {
                    "not passed"
                },
                cohort.current.counts.passed,
                cohort.current.counts.requested
            ));
            for check in cohort.checks.iter().filter(|c| !c.passed) {
                text.push_str(&format!("  {}: {}", check.name, check.message));
                if check.observed.is_some() || check.required.is_some() {
                    text.push_str(&format!(
                        " (observed {}, required {})",
                        check
                            .observed
                            .map_or_else(|| "unavailable".into(), |value| value.to_string()),
                        check
                            .required
                            .map_or_else(|| "not applicable".into(), |value| value.to_string())
                    ));
                }
                text.push('\n');
            }
            for change in &cohort.metadata_changes {
                text.push_str(&format!(
                    "  {}: {} → {} ({})\n",
                    change.field,
                    change.baseline,
                    change.current,
                    if change.allowed {
                        "allowed explicitly"
                    } else {
                        "incompatible"
                    }
                ));
            }
        }
        text
    }
}
fn check(
    checks: &mut Vec<Check>,
    name: &str,
    observed: Option<f64>,
    required: Option<f64>,
    passed: bool,
    message: &str,
) {
    checks.push(Check {
        name: name.into(),
        passed,
        observed,
        required,
        message: if passed {
            "requirement satisfied".into()
        } else {
            message.into()
        },
    });
}
fn sample_checks(checks: &mut Vec<Check>, prefix: &str, observation: &Observation, rule: &Rule) {
    for (name, observed, required) in [
        ("attempts", observation.counts.requested, rule.min_attempts),
        (
            "successful_attempts",
            observation.counts.passed,
            rule.min_successful_attempts,
        ),
        (
            "message_rtt_samples",
            observation
                .message_rtt_seconds
                .as_ref()
                .map_or(0, |t| t.samples),
            rule.min_message_rtt_samples,
        ),
    ] {
        check(
            checks,
            &format!("{prefix}{name}"),
            Some(observed as f64),
            Some(f64::from(required)),
            observed >= required as usize,
            "insufficient declared or successful samples",
        );
    }
}
fn record(changes: &mut Vec<Change>, field: &str, new: Value, old: Value, allowed: bool) {
    if new != old {
        changes.push(Change {
            field: field.into(),
            current: new,
            baseline: old,
            allowed,
        });
    }
}
fn compatibility(
    current: &Snapshot,
    base: &Snapshot,
    key: &Key,
    now: &Observation,
    old: &Observation,
    relative: &Relative,
) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    record(
        &mut changes,
        "cohort_coverage",
        serde_json::to_value(current.keys())?,
        serde_json::to_value(base.keys())?,
        false,
    );
    record(
        &mut changes,
        "case_conditions",
        serde_json::to_value(current.case_for(&key.0))?,
        serde_json::to_value(base.case_for(&key.0))?,
        false,
    );
    for field in [
        "os",
        "architecture",
        "kernel_release",
        "cpu_model",
        "available_parallelism",
        "packet_capture",
    ] {
        let new = &current.environment[field];
        let old = &base.environment[field];
        if field != "packet_capture" && (new.is_null() || old.is_null()) {
            changes.push(Change {
                field: format!("environment.{field}"),
                current: new.clone(),
                baseline: old.clone(),
                allowed: false,
            });
        } else {
            record(
                &mut changes,
                &format!("environment.{field}"),
                new.clone(),
                old.clone(),
                false,
            );
        }
    }
    if let (Some(now), Some(old)) = (&now.implementation, &old.implementation) {
        record(
            &mut changes,
            "implementation_name",
            now.name.clone().into(),
            old.name.clone().into(),
            false,
        );
        record(
            &mut changes,
            "implementation_version",
            now.version.clone().into(),
            old.version.clone().into(),
            relative
                .allowed_metadata_changes
                .contains(&AllowedChange::ImplementationVersion),
        );
        let fields: BTreeSet<_> = now.settings.keys().chain(old.settings.keys()).collect();
        for field in fields {
            let allowed = relative
                .allowed_metadata_changes
                .iter()
                .any(|allowed| allowed.field() == field);
            record(
                &mut changes,
                field,
                serde_json::to_value(now.settings.get(field))?,
                serde_json::to_value(old.settings.get(field))?,
                allowed,
            );
        }
    } else if now.counts.unreported == 0 && old.counts.unreported == 0 {
        changes.push(Change {
            field: "implementation identity unavailable".into(),
            current: Value::Null,
            baseline: Value::Null,
            allowed: false,
        });
    }
    Ok(changes)
}
fn evaluate(
    current: &Snapshot,
    baseline: Option<&Snapshot>,
    policy: &Policy,
    current_path: &Path,
    baseline_path: Option<&Path>,
) -> Result<Report> {
    let mut report = Report {
        schema_version: 1,
        kind: "transport_regression_report",
        current_report: current_path.to_string_lossy().into_owned(),
        baseline_report: baseline_path.map(|path| path.to_string_lossy().into_owned()),
        current_source_exit_code: current.exit_code(),
        baseline_source_exit_code: baseline.map(Snapshot::exit_code),
        complete: false,
        interrupted: false,
        exit_code: 0,
        cohorts: Vec::new(),
    };
    for rule in &policy.cohorts {
        if crate::cancellation().load(Ordering::Relaxed) {
            report.interrupted = true;
            break;
        }
        let key = (rule.case.clone(), rule.adapter.clone());
        let now = current.observe(&key);
        let old = baseline.map(|base| base.observe(&key));
        let mut checks = Vec::new();
        let mut changes = Vec::new();
        sample_checks(&mut checks, "current_", &now, rule);
        check(
            &mut checks,
            "delivery_rate",
            Some(now.delivery_rate),
            Some(rule.min_delivery_rate),
            now.delivery_rate >= rule.min_delivery_rate,
            "delivery rate below absolute requirement; denominator includes every planned attempt",
        );
        if let Some(max) = rule.max_message_rtt_p95_seconds {
            let got = now.message_rtt_seconds.as_ref().map(|t| t.p95);
            check(
                &mut checks,
                "message_rtt_p95_seconds",
                got,
                Some(max),
                got.is_some_and(|v| v <= max),
                "conditional p95 RTT exceeds absolute maximum or is unavailable",
            );
        }
        if let Some(min) = rule.min_bulk_goodput_bytes_per_second {
            let got = now.bulk_goodput_bytes_per_second.as_ref().map(|t| t.min);
            check(
                &mut checks,
                "minimum_bulk_goodput_bytes_per_second",
                got,
                Some(min),
                got.is_some_and(|v| v >= min),
                "minimum successful-attempt verified goodput below absolute floor or unavailable",
            );
        }
        let mut incompatible = now.implementation_changed;
        if let (Some(relative), Some(base), Some(old)) = (&rule.baseline, baseline, &old) {
            sample_checks(&mut checks, "baseline_", old, rule);
            changes = compatibility(current, base, &key, &now, old, relative)?;
            incompatible |=
                old.implementation_changed || changes.iter().any(|change| !change.allowed);
            if let Some(max) = relative.max_delivery_rate_drop {
                let drop = old.delivery_rate - now.delivery_rate;
                check(
                    &mut checks,
                    "delivery_rate_drop",
                    Some(drop),
                    Some(max),
                    drop <= max,
                    "delivery-rate decrease exceeds relative allowance",
                );
            }
            if let Some(max) = relative.max_message_rtt_p95_increase_fraction {
                let pair = now
                    .message_rtt_seconds
                    .as_ref()
                    .zip(old.message_rtt_seconds.as_ref());
                let change = pair.and_then(|(n, b)| {
                    let v = n.p95 / b.p95 - 1.;
                    v.is_finite().then_some(v)
                });
                check(
                    &mut checks,
                    "message_rtt_p95_increase_fraction",
                    change,
                    Some(max),
                    pair.is_some_and(|(n, b)| n.p95 <= b.p95 * (1. + max)),
                    "p95 RTT increase exceeds relative allowance or samples unavailable",
                );
            }
            if let Some(max) = relative.max_bulk_goodput_drop_fraction {
                let pair = now
                    .bulk_goodput_bytes_per_second
                    .as_ref()
                    .zip(old.bulk_goodput_bytes_per_second.as_ref());
                let change = pair.and_then(|(n, b)| {
                    let v = 1. - n.min / b.min;
                    v.is_finite().then_some(v)
                });
                check(&mut checks,"minimum_bulk_goodput_drop_fraction",change,Some(max),pair.is_some_and(|(n,b)|n.min>=b.min*(1.-max)),"minimum verified-goodput drop exceeds relative allowance or samples unavailable");
            }
        }
        check(
            &mut checks,
            "compatible_recorded_settings",
            None,
            None,
            !incompatible,
            "recorded experiment/build settings are incompatible or changed within a cohort",
        );
        let source_error = now.counts.infrastructure_failed > 0
            || old
                .as_ref()
                .is_some_and(|o| o.counts.infrastructure_failed > 0);
        let interrupted = current.interrupted || baseline.is_some_and(|b| b.interrupted);
        let unsupported =
            now.counts.unsupported > 0 || old.as_ref().is_some_and(|o| o.counts.unsupported > 0);
        let incomplete =
            now.counts.unreported > 0 || old.as_ref().is_some_and(|o| o.counts.unreported > 0);
        check(&mut checks,"coverage",None,None,!source_error&&!interrupted&&!unsupported&&!incomplete,"fixture errors, interruptions, unsupported or unreported attempts block passing assessment");
        let code = if interrupted {
            130
        } else if source_error || incompatible {
            2
        } else if unsupported || incomplete {
            3
        } else if checks.iter().any(|c| !c.passed) {
            1
        } else {
            0
        };
        report.cohorts.push(Cohort {
            case: rule.case.clone(),
            adapter: rule.adapter.clone(),
            exit_code: code,
            current: now,
            baseline: old,
            checks,
            metadata_changes: changes,
        });
    }
    report.complete = report.cohorts.len() == policy.cohorts.len();
    report.interrupted |= current.interrupted
        || baseline.is_some_and(|b| b.interrupted)
        || crate::cancellation().load(Ordering::Relaxed);
    report.exit_code = if report.interrupted || report.cohorts.iter().any(|c| c.exit_code == 130) {
        130
    } else if report.cohorts.iter().any(|c| c.exit_code == 2) {
        2
    } else if !report.complete || report.cohorts.iter().any(|c| c.exit_code == 3) {
        3
    } else if report.cohorts.iter().any(|c| c.exit_code == 1) {
        1
    } else {
        0
    };
    Ok(report)
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "input exceeds {limit} bytes");
    Ok(bytes)
}
fn freeze(path: &Path, bytes: &[u8]) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)?;
    Ok(())
}
/// Assess frozen reports without starting processes or requiring network privileges.
pub fn run(
    current_path: &Path,
    policy_path: &Path,
    baseline_path: Option<&Path>,
    artifacts: &Path,
) -> Result<Report> {
    let current_path = fs::canonicalize(current_path)?;
    let baseline_path = baseline_path.map(fs::canonicalize).transpose()?;
    let input = read(&current_path, REPORT_LIMIT)?;
    let policy_bytes = read(policy_path, POLICY_LIMIT)?;
    let current: Snapshot = serde_json::from_slice(&input).context("read comparison report")?;
    current.validate()?;
    let policy: Policy = serde_json::from_slice(&policy_bytes).context("read regression policy")?;
    policy.validate()?;
    ensure!(
        current.keys()
            == policy
                .cohorts
                .iter()
                .map(|r| (r.case.clone(), r.adapter.clone()))
                .collect(),
        "policy must cover every case/adapter cohort exactly; no filtered coverage"
    );
    ensure!(
        baseline_path.is_some() == policy.cohorts.iter().any(|r| r.baseline.is_some()),
        "provide --baseline exactly when the policy declares relative requirements"
    );
    let baseline_bytes = baseline_path
        .as_deref()
        .map(|path| read(path, REPORT_LIMIT))
        .transpose()?;
    let baseline = baseline_bytes
        .as_ref()
        .map(|bytes| serde_json::from_slice::<Snapshot>(bytes))
        .transpose()?;
    if let Some(base) = &baseline {
        base.validate()?;
        ensure!(
            base.keys() == current.keys(),
            "baseline must contain exactly the same case/adapter cohorts"
        );
    }
    fs::create_dir(artifacts)
        .context("artifact directory must be new and its parent must exist")?;
    freeze(&artifacts.join("current.json"), &input)?;
    freeze(&artifacts.join("policy.json"), &policy_bytes)?;
    if let Some(bytes) = baseline_bytes {
        freeze(&artifacts.join("baseline.json"), &bytes)?;
    }
    let report = evaluate(
        &current,
        baseline.as_ref(),
        &policy,
        &current_path,
        baseline_path.as_deref(),
    )?;
    scenario::atomic_write(
        &artifacts.join("report.json"),
        &serde_json::to_vec_pretty(&report)?,
    )?;
    let failures = report.cohorts.iter().filter(|c| c.exit_code == 1).count();
    let errors = report
        .cohorts
        .iter()
        .filter(|c| matches!(c.exit_code, 2 | 130))
        .count();
    let skipped = policy.cohorts.len() - report.cohorts.len()
        + report.cohorts.iter().filter(|c| c.exit_code == 3).count();
    let mut xml=format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"transport regression\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\">\n",policy.cohorts.len());
    for rule in &policy.cohorts {
        xml.push_str(&format!(
            "<testcase classname=\"{}\" name=\"{}\">",
            scenario::escape(&rule.adapter),
            scenario::escape(&rule.case)
        ));
        match report
            .cohorts
            .iter()
            .find(|c| c.case == rule.case && c.adapter == rule.adapter)
        {
            Some(cohort) if cohort.exit_code == 0 => {}
            Some(cohort) => {
                let messages = cohort
                    .checks
                    .iter()
                    .filter(|c| !c.passed)
                    .map(|c| format!("{}: {}", c.name, c.message))
                    .chain(
                        cohort
                            .metadata_changes
                            .iter()
                            .filter(|c| !c.allowed)
                            .map(|c| format!("incompatible {}", c.field)),
                    )
                    .collect::<Vec<_>>()
                    .join("; ");
                let tag = match cohort.exit_code {
                    1 => "failure",
                    3 => "skipped",
                    _ => "error",
                };
                xml.push_str(&format!(
                    "<{tag} message=\"{}\"/>",
                    scenario::escape(&messages)
                ));
            }
            None => xml.push_str("<skipped message=\"unassessed\"/>"),
        }
        xml.push_str("</testcase>\n");
    }
    xml.push_str("</testsuite>\n");
    scenario::atomic_write(&artifacts.join("junit.xml"), xml.as_bytes())?;
    Ok(report)
}
