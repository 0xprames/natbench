use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "natbench-repeat-cli-{}-{label}",
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
fn case(name: &str, argv: Value, timeout: f64) -> Value {
    json!({"name":name,"a":"preserve","b":"random","router_input":"drop","processes":[{"name":"client","role":"a","argv":argv,"timeout_seconds":timeout}],"steps":[{"action":"run","process":"client","stdout_contains":"PASS"}]})
}
fn namespace_snapshot() -> std::collections::BTreeSet<String> {
    let result = Command::new("ip").args(["netns", "list"]).output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.starts_with("nb"))
        .map(str::to_owned)
        .collect()
}
#[test]
fn invalid_repeat_input_and_run_counts_do_not_create_artifacts() {
    let root = Temp::new("invalid");
    let source = root.0.join("suite.json");
    fs::write(&source, "{\"schema_version\":3,\"cases\":[]}").unwrap();
    let artifacts = root.0.join("artifacts");
    for runs in ["0", "101", "2"] {
        let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .arg("repeat")
            .arg(&source)
            .arg("--runs")
            .arg(runs)
            .arg("--artifacts")
            .arg(&artifacts)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!artifacts.exists());
    }
}
#[test]
#[ignore = "requires root and Linux network namespaces"]
fn repeated_applications_keep_failed_attempts_and_use_frozen_input() {
    let before = namespace_snapshot();
    let root = Temp::new("frozen");
    let source = root.0.join("suite.json");
    let suite = json!({"schema_version":3,"cases":[
        case("delivery & snapshot", json!(["sh","-c","printf changed > suite.json; echo PASS"]), 2.),
        case("application regression", json!(["sh","-c","echo broken; exit 5"]), 2.)
    ]});
    let input = serde_json::to_vec(&suite).unwrap();
    fs::write(&source, &input).unwrap();
    let artifacts = root.0.join("artifacts");
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("repeat")
        .arg(&source)
        .arg("--runs")
        .arg("2")
        .arg("--artifacts")
        .arg(&artifacts)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(summary["complete"], true);
    assert_eq!(summary["completed_runs"], 2);
    assert_eq!(summary["cases"][0]["verdicts"]["passed"], 2);
    assert_eq!(summary["cases"][1]["verdicts"]["assertion_failed"], 2);
    assert_eq!(
        summary["cases"][0]["timing_seconds"]["passed"]["samples"],
        2
    );
    assert_eq!(
        summary["cases"][1]["timing_seconds"]["assertion_failed"]["samples"],
        2
    );
    assert!(summary["environment"]["kernel_release"].is_string());
    assert_eq!(fs::read(&source).unwrap(), b"changed");
    assert_eq!(fs::read(artifacts.join("scenario.json")).unwrap(), input);
    for index in ["run-001", "run-002"] {
        let directory = artifacts.join(index);
        assert_eq!(fs::read(directory.join("scenario.json")).unwrap(), input);
        let report: Value =
            serde_json::from_slice(&fs::read(directory.join("report.json")).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 2);
        assert_eq!(report["cases"][0]["status"], "passed");
        assert!(report["cases"][0]["elapsed_seconds"].as_f64().unwrap() > 0.);
        assert!(directory.join("case-000/client-1.stdout.log").exists());
        assert!(directory.join("case-001/client-1.stderr.log").exists());
    }
    let schema: Value =
        serde_json::from_str(include_str!("../schemas/repetition-report-v1.schema.json")).unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&summary)
        .unwrap();
    let xml = fs::read_to_string(artifacts.join("junit.xml")).unwrap();
    assert!(xml.contains("name=\"run-002\""));
    assert!(xml.contains("delivery &amp; snapshot"));
    assert_eq!(xml.matches("<failure ").count(), 2);
    // Existing evidence must be refused even when the input file is restored.
    fs::write(&source, &input).unwrap();
    let previous = fs::read(artifacts.join("summary.json")).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("repeat")
        .arg(&source)
        .arg("--artifacts")
        .arg(&artifacts)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(fs::read(artifacts.join("summary.json")).unwrap(), previous);
    assert_eq!(before, namespace_snapshot());
}
#[test]
#[ignore = "requires root and Linux network namespaces"]
fn cancellation_keeps_active_logs_and_marks_future_runs_unreported() {
    let before = namespace_snapshot();
    let root = Temp::new("cancel");
    let source = root.0.join("suite.json");
    let suite = json!({"schema_version":2,"cases":[case("active application",json!(["sh","-c","echo READY; sleep 60"]),60.)]});
    fs::write(&source, serde_json::to_vec(&suite).unwrap()).unwrap();
    let artifacts = root.0.join("artifacts");
    let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("repeat")
        .arg(&source)
        .arg("--runs")
        .arg("3")
        .arg("--artifacts")
        .arg(&artifacts)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let log = artifacts.join("run-001/case-000/client-1.stdout.log");
    let deadline = Instant::now() + Duration::from_secs(8);
    while !fs::read_to_string(&log).is_ok_and(|value| value.contains("READY")) {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("application did not start");
        }
        thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: this is the positive PID of the child owned by this test.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(130));
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("cancellation timed out");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let summary: Value =
        serde_json::from_slice(&fs::read(artifacts.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["interrupted"], true);
    assert_eq!(summary["complete"], false);
    assert_eq!(summary["completed_runs"], 0);
    assert_eq!(summary["cases"][0]["interrupted_runs"], 1);
    assert_eq!(summary["cases"][0]["unreported_runs"], 2);
    assert!(summary["cases"][0]["timing_seconds"]
        .as_object()
        .unwrap()
        .is_empty());
    assert_eq!(summary["runs"][0]["exit_code"], 130);
    assert!(log.exists());
    assert!(!artifacts.join("run-002").exists());
    assert!(fs::read_to_string(artifacts.join("junit.xml"))
        .unwrap()
        .contains("type=\"interrupted\""));
    assert_eq!(before, namespace_snapshot());
}
