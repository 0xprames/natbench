use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("natbench-compare-{}-{label}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self, mode: &str) -> PathBuf {
        let adapter = self.0.join("adapter.py");
        fs::write(&adapter,r#"import json,pathlib,sys,time
mode = sys.argv[1]
r = json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
state = pathlib.Path(__file__).with_name('version.txt')
if mode == 'flip' and r['role']=='server':
    state.write_text(str(int(state.read_text())+1 if state.exists() else 1))
version = state.read_text() if mode == 'flip' else '1'
info = dict(name='synthetic-contract-adapter',version=version,settings={})
base = dict(schema_version=1,kind='transport_event',run_id=r['run_id'],implementation=info)
def emit(**fields): print(json.dumps(dict(base,**fields),separators=(',',':')),flush=True)
if r['role']=='server':
    pathlib.Path(r['peer_file']).write_text(json.dumps(dict(schema_version=1,kind='transport_peer',run_id=r['run_id'],implementation=info,details={'source':'synthetic contract test'})))
    emit(event='ready',capabilities=['direct_reliable_stream'])
    while True: time.sleep(1)
if mode=='slow':
    emit(event='ready',capabilities=['direct_reliable_stream'])
    while True: time.sleep(1)
m = dict(workload=r['workload'],path='direct',path_evidence={'source':'synthetic parser test'},first_data_seconds=0.01,message_rtt_seconds=[0.002]*r['workload']['measured_messages'],bulk_verified_bytes=r['workload']['bulk_bytes'],bulk_seconds=0.02)
emit(event='completed',measurement=m)
"#).unwrap();
        let path = self.0.join("comparison.json");
        let config = json!({"schema_version":1,"kind":"transport_comparison","adapters":[{"name":"reference-a","argv":["python3",adapter,mode]},{"name":"reference-b","argv":["python3",adapter,mode]}],"cases":[{"name":"synthetic contract test","profile":"preserve","deadline_ms":3000,"workload":{"payload_bytes":128,"warmup_messages":1,"measured_messages":2,"bulk_bytes":1024}}]});
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn namespaces() -> std::collections::BTreeSet<String> {
    let output = Command::new("ip").args(["netns", "list"]).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.starts_with("nb"))
        .map(str::to_owned)
        .collect()
}
#[test]
fn invalid_comparison_and_missing_capture_tool_fail_before_artifacts() {
    let root = Temp::new("invalid");
    let input = root.config("pass");
    let mut invalid: Value = serde_json::from_slice(&fs::read(&input).unwrap()).unwrap();
    invalid["cases"][0]["workload"]["payload_bytes"] = json!(0);
    fs::write(&input, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("compare")
        .arg(&input)
        .args(["--runs", "2", "--artifacts"])
        .arg(root.0.join("invalid"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(!root.0.join("invalid").exists());
    let input = root.config("pass");
    let out = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("compare")
        .arg(input)
        .args(["--capture", "--artifacts"])
        .arg(root.0.join("missing"))
        .env("PATH", "/missing-tools")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(!root.0.join("missing").exists());
}
fn report(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path.join("report.json")).unwrap()).unwrap()
}
#[test]
#[ignore = "requires root, namespaces and python3; synthetic adapter validates orchestration, not real delivery"]
fn comparison_rotates_adapters_and_rejects_version_changes_with_cleanup() {
    let before = namespaces();
    for mode in ["pass", "flip"] {
        let root = Temp::new(mode);
        let input = root.config(mode);
        let artifacts = root.0.join("artifacts");
        let out = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .arg("compare")
            .arg(input)
            .args(["--runs", "2", "--artifacts"])
            .arg(&artifacts)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(if mode == "pass" { 0 } else { 2 }),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report = report(&artifacts);
        assert_eq!(report["complete"], true);
        let order: Vec<_> = report["planned"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["adapter"].as_str().unwrap())
            .collect();
        assert_eq!(
            order,
            ["reference-a", "reference-b", "reference-b", "reference-a"]
        );
        let outcomes: Vec<_> = report["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["outcome"].as_str().unwrap())
            .collect();
        assert_eq!(
            outcomes,
            if mode == "pass" {
                vec!["passed"; 4]
            } else {
                vec![
                    "passed",
                    "passed",
                    "infrastructure_failed",
                    "infrastructure_failed",
                ]
            }
        );
        let schema: Value = serde_json::from_str(include_str!(
            "../schemas/transport-comparison-report-v1.schema.json"
        ))
        .unwrap();
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(&report)
            .unwrap();
        for summary in report["summaries"].as_array().unwrap() {
            assert_eq!(
                summary["first_data_seconds"]["samples"],
                if mode == "pass" { 2 } else { 1 }
            );
            assert_eq!(
                summary["message_rtt_seconds"]["samples"],
                if mode == "pass" { 4 } else { 2 }
            );
        }
    }
    assert_eq!(before, namespaces());
}
#[test]
#[ignore = "requires root, namespaces and python3"]
fn interruption_retains_active_evidence_and_future_slots() {
    let before = namespaces();
    let root = Temp::new("cancel");
    let input = root.config("slow");
    let artifacts = root.0.join("artifacts");
    let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("compare")
        .arg(input)
        .args(["--runs", "2", "--artifacts"])
        .arg(&artifacts)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fs::read_to_string(artifacts.join("attempt-0001/evidence/case-000/client-1.stdout.log"))
        .is_ok_and(|text| !text.is_empty())
    {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("comparison did not start");
        }
        thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: positive PID of this test's owned, unreaped child.
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
            panic!("comparison cleanup timed out");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let r = report(&artifacts);
    assert_eq!(r["interrupted"], true);
    assert_eq!(r["complete"], false);
    assert_eq!(r["attempts"][0]["outcome"], "interrupted");
    assert_eq!(r["planned"].as_array().unwrap().len(), 4);
    assert!(artifacts.join("attempt-0001/server.json").exists());
    assert!(artifacts.join("attempt-0001/evidence/report.json").exists());
    let xml = fs::read_to_string(artifacts.join("junit.xml")).unwrap();
    assert!(xml.contains("errors=\"1\""));
    assert!(xml.contains("skipped=\"3\""));
    assert_eq!(before, namespaces());
}
