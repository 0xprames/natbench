//! Synthetic report fixtures test offline policy semantics, not transport performance.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "natbench-assess-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn assess(
        &self,
        source: &Value,
        policy: &Value,
        baseline: Option<&Value>,
    ) -> (i32, Option<Value>) {
        fs::write(
            self.0.join("current.json"),
            serde_json::to_vec(source).unwrap(),
        )
        .unwrap();
        fs::write(
            self.0.join("policy.json"),
            serde_json::to_vec(policy).unwrap(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_natbench"));
        command
            .args([
                "assess",
                self.0.join("current.json").to_str().unwrap(),
                "--policy",
                self.0.join("policy.json").to_str().unwrap(),
                "--artifacts",
                self.0.join("out").to_str().unwrap(),
            ])
            .env("PATH", "");
        if let Some(baseline) = baseline {
            fs::write(
                self.0.join("baseline.json"),
                serde_json::to_vec(baseline).unwrap(),
            )
            .unwrap();
            command.arg("--baseline").arg(self.0.join("baseline.json"));
        }
        let output = command.output().unwrap();
        let code = output.status.code().unwrap();
        let report = self.0.join("out/report.json");
        if report.exists() {
            let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
            let schema: Value = serde_json::from_str(include_str!(
                "../schemas/transport-regression-report-v1.schema.json"
            ))
            .unwrap();
            jsonschema::validator_for(&schema)
                .unwrap()
                .validate(&report)
                .unwrap();
            assert_eq!(
                report["exit_code"],
                code,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(self.0.join("out/junit.xml").exists());
            assert_eq!(
                fs::read(self.0.join("out/current.json")).unwrap(),
                serde_json::to_vec(source).unwrap()
            );
            (code, Some(report))
        } else {
            assert!(
                !self.0.join("out").exists(),
                "invalid inputs should fail before artifacts: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            (code, None)
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn source() -> Value {
    let workload =
        json!({"payload_bytes":8,"warmup_messages":0,"measured_messages":10,"bulk_bytes":1024});
    let implementation = json!({"name":"synthetic-policy-fixture","version":"1","settings":{"authentication":"none","adapter_source_sha256":"test-a","adapter_version":"1","quic_engine_version":"1"}});
    let mut planned = Vec::new();
    let mut attempts = Vec::new();
    for run in 1..=2 {
        for adapter in ["a", "b"] {
            let index = planned.len();
            planned.push(json!({"index":index,"run":run,"case":"case","adapter":adapter,"artifacts_directory":format!("attempt-{index}")}));
            attempts.push(json!({"index":index,"outcome":"passed","messages":[],"implementation":implementation,"measurement":{"workload":workload,"path":"direct","path_evidence":{"source":"synthetic policy fixture"},"first_data_seconds":0.01,"message_rtt_seconds":[0.001,0.002,0.003,0.004,0.005,0.006,0.007,0.008,0.009,0.01],"bulk_verified_bytes":1024,"bulk_seconds":0.1}}));
        }
    }
    json!({"schema_version":1,"kind":"transport_comparison_report","topology":"ipv4_client_a_to_wan","workload_semantics":"direct reliable streams; fresh endpoints; bulk includes receiver verification and acknowledgement","environment":{"os":"linux","architecture":"x86_64","kernel_release":"test-kernel","cpu_model":"test-cpu","available_parallelism":2},"case_conditions":[{"name":"case","profile":"preserve","network":null,"deadline_ms":5000,"workload":workload}],"planned":planned,"attempts":attempts,"active_attempt":null,"complete":true,"interrupted":false,"summaries":[{"source":"deliberately ignored; assessor recomputes raw data","message_rtt_seconds":{"p95":0.0,"samples":1000000}}]})
}
fn policy(relative: bool) -> Value {
    let mut rules = Vec::new();
    for adapter in ["a", "b"] {
        let mut rule = json!({"case":"case","adapter":adapter,"min_attempts":2,"min_successful_attempts":2,"min_message_rtt_samples":20,"min_delivery_rate":1.0,"max_message_rtt_p95_seconds":0.02,"min_bulk_goodput_bytes_per_second":1000});
        if relative {
            rule["baseline"] = json!({"max_delivery_rate_drop":0.1,"max_message_rtt_p95_increase_fraction":0.1,"max_bulk_goodput_drop_fraction":0.1,"allowed_metadata_changes":[]});
        }
        rules.push(rule);
    }
    json!({"schema_version":1,"kind":"transport_regression_policy","cohorts":rules})
}
fn tolerant() -> Value {
    let mut policy = policy(false);
    for rule in policy["cohorts"].as_array_mut().unwrap() {
        rule["min_successful_attempts"] = 1.into();
        rule["min_message_rtt_samples"] = 10.into();
        rule["min_delivery_rate"] = 0.5.into();
    }
    policy
}
fn fail(source: &mut Value, index: usize, outcome: &str) {
    source["attempts"][index]["outcome"] = outcome.into();
    source["attempts"][index]["measurement"] = Value::Null;
}

#[test]
fn gates_recompute_raw_metrics_and_self_baseline_without_network_tools() {
    let source = source();
    let (code, report) = Temp::new().assess(&source, &policy(true), Some(&source));
    assert_eq!(code, 0);
    let report = report.unwrap();
    assert!(report["complete"].as_bool().unwrap());
    for cohort in report["cohorts"].as_array().unwrap() {
        assert_eq!(cohort["current"]["message_rtt_seconds"]["samples"], 20);
        assert_eq!(cohort["current"]["message_rtt_seconds"]["p95"], 0.01);
        assert_eq!(
            cohort["current"]["bulk_goodput_bytes_per_second"]["min"],
            10240.0
        );
        assert!(cohort["metadata_changes"].as_array().unwrap().is_empty());
    }
}

#[test]
fn explicit_delivery_tolerance_keeps_failed_attempts_and_original_exit() {
    let mut source = source();
    fail(&mut source, 2, "transport_failed");
    fail(&mut source, 3, "transport_failed");
    let (code, report) = Temp::new().assess(&source, &tolerant(), None);
    assert_eq!(code, 0);
    let report = report.unwrap();
    assert_eq!(report["current_source_exit_code"], 1);
    for cohort in report["cohorts"].as_array().unwrap() {
        assert_eq!(cohort["current"]["counts"]["transport_failed"], 1);
        assert_eq!(cohort["current"]["delivery_rate"], 0.5);
        assert_eq!(cohort["current"]["message_rtt_seconds"]["samples"], 10);
    }
    assert_eq!(Temp::new().assess(&source, &policy(false), None).0, 1);
}

#[test]
fn unsupported_incomplete_errors_and_interruption_cannot_pass_tolerant_policy() {
    for (outcome, expected) in [
        ("unsupported", 3),
        ("infrastructure_failed", 2),
        ("interrupted", 130),
    ] {
        let mut source = source();
        fail(&mut source, 2, outcome);
        if outcome == "interrupted" {
            source["interrupted"] = true.into();
        }
        assert_eq!(
            Temp::new().assess(&source, &tolerant(), None).0,
            expected,
            "{outcome}"
        );
    }
    let mut partial = source();
    partial["attempts"].as_array_mut().unwrap().truncate(2);
    partial["complete"] = false.into();
    partial["active_attempt"] = 2.into();
    let (code, report) = Temp::new().assess(&partial, &tolerant(), None);
    assert_eq!(code, 3);
    assert!(report.unwrap()["cohorts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|cohort| cohort["current"]["counts"]["unreported"] == 1));
    let mut cancelled = source();
    cancelled["interrupted"] = true.into();
    assert_eq!(Temp::new().assess(&cancelled, &policy(false), None).0, 130);
    let mut empty = source();
    empty["attempts"] = json!([]);
    empty["complete"] = false.into();
    empty["active_attempt"] = 0.into();
    assert_eq!(
        Temp::new().assess(&empty, &policy(true), Some(&source())).0,
        3
    );
    assert_eq!(
        Temp::new().assess(&source(), &policy(true), Some(&empty)).0,
        3
    );
}

#[test]
fn absolute_and_relative_rtt_and_goodput_requirements_fail_independently() {
    let baseline = source();
    for metric in ["rtt", "goodput"] {
        let mut current = source();
        for attempt in current["attempts"].as_array_mut().unwrap() {
            if metric == "rtt" {
                attempt["measurement"]["message_rtt_seconds"] = json!(vec![0.02; 10]);
            } else {
                attempt["measurement"]["bulk_seconds"] = 0.2.into();
            }
        }
        let (code, report) = Temp::new().assess(&current, &policy(true), Some(&baseline));
        assert_eq!(code, 1, "{metric}");
        let name = if metric == "rtt" {
            "message_rtt_p95_increase_fraction"
        } else {
            "minimum_bulk_goodput_drop_fraction"
        };
        assert!(report.unwrap()["cohorts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|cohort| cohort["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["name"] == name && check["passed"] == false)));
        let mut policy = policy(false);
        for rule in policy["cohorts"].as_array_mut().unwrap() {
            if metric == "rtt" {
                rule["max_message_rtt_p95_seconds"] = 0.015.into();
            } else {
                rule["min_bulk_goodput_bytes_per_second"] = 6000.into();
            }
        }
        assert_eq!(Temp::new().assess(&current, &policy, None).0, 1);
    }
}

#[test]
fn baseline_rejects_changed_experiment_machine_and_security_settings() {
    let base = source();
    for field in [
        "profile",
        "deadline_ms",
        "network",
        "payload_bytes",
        "architecture",
        "kernel_release",
        "cpu_model",
        "available_parallelism",
        "packet_capture",
        "authentication",
        "implementation_name",
        "missing_cpu",
    ] {
        let mut current = source();
        match field {
            "profile" => current["case_conditions"][0][field] = "random".into(),
            "deadline_ms" => current["case_conditions"][0][field] = 4000.into(),
            "network" => {
                current["case_conditions"][0][field] = json!({"client_to_server":{"delay_ms":1,"loss_percent":0},"server_to_client":{"delay_ms":0,"loss_percent":0}})
            }
            "payload_bytes" => {
                current["case_conditions"][0]["workload"][field] = 9.into();
                for attempt in current["attempts"].as_array_mut().unwrap() {
                    attempt["measurement"]["workload"][field] = 9.into();
                }
            }
            "available_parallelism" => current["environment"][field] = 4.into(),
            "packet_capture" => {
                current["environment"][field] = json!({"packets_per_role":1000,"snaplen_bytes":256,"roles":["a","b","ra","rb","wan"]})
            }
            "authentication" => {
                for attempt in current["attempts"].as_array_mut().unwrap() {
                    attempt["implementation"]["settings"][field] = "mutual authentication".into();
                }
            }
            "implementation_name" => {
                for attempt in current["attempts"].as_array_mut().unwrap() {
                    attempt["implementation"]["name"] = "different implementation".into();
                }
            }
            "missing_cpu" => {
                current["environment"]
                    .as_object_mut()
                    .unwrap()
                    .remove("cpu_model");
            }
            _ => current["environment"][field] = "different".into(),
        }
        let (code, report) = Temp::new().assess(&current, &policy(true), Some(&base));
        assert_eq!(code, 2, "{field}");
        assert!(report.unwrap()["cohorts"][0]["metadata_changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["allowed"] == false));
    }
}

#[test]
fn implementation_changes_require_explicit_bounded_metadata_allowance() {
    let baseline = source();
    let mut current = source();
    for attempt in current["attempts"].as_array_mut().unwrap() {
        attempt["implementation"]["version"] = "2".into();
        attempt["implementation"]["settings"]["adapter_source_sha256"] = "test-b".into();
    }
    assert_eq!(
        Temp::new()
            .assess(&current, &policy(true), Some(&baseline))
            .0,
        2
    );
    let mut allowed = policy(true);
    for rule in allowed["cohorts"].as_array_mut().unwrap() {
        rule["baseline"]["allowed_metadata_changes"] =
            json!(["implementation_version", "adapter_source_sha256"]);
    }
    let (code, report) = Temp::new().assess(&current, &allowed, Some(&baseline));
    assert_eq!(code, 0);
    assert!(report.unwrap()["cohorts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|cohort| cohort["metadata_changes"].as_array().unwrap().len() == 2));
    allowed["cohorts"][0]["baseline"]["allowed_metadata_changes"] = json!(["authentication"]);
    assert_eq!(Temp::new().assess(&current, &allowed, Some(&baseline)).0, 2);
}

#[test]
fn invalid_or_filtered_policy_and_malformed_reports_fail_before_artifacts() {
    let valid = source();
    for variation in 0..8 {
        let mut policy = policy(false);
        match variation {
            0 => {
                policy["cohorts"].as_array_mut().unwrap().pop();
            }
            1 => policy["cohorts"][1]["adapter"] = "a".into(),
            2 => policy["cohorts"][0]["typo"] = 1.into(),
            3 => policy["cohorts"][0]["min_delivery_rate"] = 1.1.into(),
            4 => policy["cohorts"][0]["max_message_rtt_p95_seconds"] = Value::Null,
            5 => policy["cohorts"][0]["min_message_rtt_samples"] = 0.into(),
            6 => policy["cohorts"][0]["baseline"] = json!({"allowed_metadata_changes":[]}),
            _ => policy["schema_version"] = 2.into(),
        }
        assert_eq!(Temp::new().assess(&valid, &policy, None).0, 2);
    }
    for variation in 0..9 {
        let mut current = source();
        match variation {
            0 => current["attempts"][0]["index"] = 1.into(),
            1 => current["planned"][0]["run"] = 2.into(),
            2 => current["complete"] = false.into(),
            3 => current["attempts"][0]["measurement"]["message_rtt_seconds"] = json!([0.001]),
            4 => current["attempts"][0]["measurement"]["bulk_verified_bytes"] = 2048.into(),
            5 => current["attempts"][0]["measurement"]["bulk_seconds"] = 0.0.into(),
            6 => current["attempts"][0]["outcome"] = "transport_failed".into(),
            7 => {
                current["case_conditions"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("workload");
            }
            _ => current["active_attempt"] = 0.into(),
        }
        assert_eq!(Temp::new().assess(&current, &policy(false), None).0, 2);
    }
    assert_eq!(Temp::new().assess(&valid, &policy(true), None).0, 2);
    assert_eq!(
        Temp::new().assess(&valid, &policy(false), Some(&valid)).0,
        2
    );
}

#[test]
fn published_policy_schema_matches_examples_and_rejects_null_and_unknown_rules() {
    let schema: Value = serde_json::from_str(include_str!(
        "../schemas/transport-regression-policy-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let example: Value = serde_json::from_str(include_str!(
        "../examples/transports/regression-policy.json"
    ))
    .unwrap();
    validator.validate(&example).unwrap();
    validator.validate(&policy(true)).unwrap();
    let mut invalid = policy(true);
    invalid["cohorts"][0]["baseline"]["allowed_metadata_changes"] = json!(["authentication"]);
    assert!(!validator.is_valid(&invalid));
    let mut invalid = policy(false);
    invalid["cohorts"][0]["max_message_rtt_p95_seconds"] = Value::Null;
    assert!(!validator.is_valid(&invalid));
    let mut invalid = policy(false);
    invalid["cohorts"][0]["typo"] = true.into();
    assert!(!validator.is_valid(&invalid));
}

#[test]
fn additive_output_metadata_is_accepted_but_inconsistent_builds_are_blocked() {
    let mut current = source();
    current["future"] = json!({"notes":"additive output metadata"});
    current["case_conditions"][0]["workload"]["future"] = true.into();
    for attempt in current["attempts"].as_array_mut().unwrap() {
        attempt["measurement"]["workload"]["future"] = true.into();
    }
    assert_eq!(
        Temp::new()
            .assess(&current, &policy(true), Some(&source()))
            .0,
        0
    );
    current["attempts"][2]["implementation"]["version"] = "2".into();
    assert_eq!(Temp::new().assess(&current, &policy(false), None).0, 2);
}
