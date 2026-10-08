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
            "natbench-conform-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn invalid_conformance_and_generated_plans_are_rejected_before_artifacts() {
    for variant in 0..6 {
        let root = Temp::new();
        let mut config = json!({"schema_version":1,"kind":"transport_adapter_config","adapter":{"name":"candidate","argv":["true"]}});
        match variant {
            0 => config["schema_version"] = 2.into(),
            1 => config["typo"] = true.into(),
            2 => config["adapter"]["argv"] = json!([]),
            3 => config["adapter"]["name"] = "bad name".into(),
            4 => config["adapter"]["cwd"] = "not-a-directory".into(),
            _ => config["adapter"]["env"] = json!({"bad=key":"value"}),
        };
        let input = root.0.join("adapter.json");
        fs::write(&input, serde_json::to_vec(&config).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .arg("conform")
            .arg(&input)
            .arg("--artifacts")
            .arg(root.0.join("out"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!root.0.join("out").exists());
    }
    let config: Value =
        serde_json::from_str(include_str!("../examples/adapter-starter/adapter.json")).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../schemas/transport-adapter-config-v1.schema.json"
    ))
    .unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&config)
        .unwrap();
    let root = Temp::new();
    let plan = json!({"schema_version":1,"kind":"transport_adapter_conformance_plan","adapters":[{"name":"candidate","argv":["true"]}],"cases":[]});
    let input = root.0.join("plan.json");
    fs::write(&input, serde_json::to_vec(&plan).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("compare")
        .arg(&input)
        .arg("--artifacts")
        .arg(root.0.join("out"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!root.0.join("out").exists());
}
#[test]
fn generator_refuses_existing_or_unsafe_destinations() {
    let root = Temp::new();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/new-adapter.sh");
    let output = Command::new(&script).arg(&root.0).output().unwrap();
    assert!(!output.status.success());
    let output = Command::new(&script)
        .arg(root.0.join("new"))
        .arg("bad/name")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!root.0.join("new").exists());
    let output = Command::new(&script)
        .arg(root.0.join("new"))
        .arg("example-transport")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = fs::read_to_string(root.0.join("new/Cargo.toml")).unwrap();
    assert!(
        manifest.contains("name = \"example-transport\"")
            && manifest.contains("crates/transport-protocol")
    );
    assert!(!manifest.contains("../../crates"));
    assert!(root
        .0
        .join("new/crates/transport-protocol/src/lib.rs")
        .is_file());
}

fn fixture(root: &Temp, mode: &str) -> PathBuf {
    let script = root.0.join("adapter.py");
    fs::write(&script,r#"import json,pathlib,sys,time
mode=sys.argv[1]
r=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
info=dict(name='synthetic-negative-contract-fixture',version='1',settings={})
base=dict(schema_version=1,kind='transport_event',run_id=r['run_id'],implementation=info)
if r['role']=='server':
    pathlib.Path(r['peer_file']).write_text(json.dumps(dict(schema_version=1,kind='transport_peer',run_id=r['run_id'],implementation=info,details={})))
    event=dict(base,event='ready',capabilities=['direct_reliable_stream'])
    if mode=='stale':event['run_id']='f'*32
    print(json.dumps(event),flush=True)
    while True:time.sleep(1)
if mode=='slow':
    print(json.dumps(dict(base,event='ready',capabilities=[])),flush=True)
    while True:time.sleep(1)
measurement=dict(workload=r['workload'],path='direct',path_evidence={'source':'synthetic negative parser fixture; no delivery claim is accepted'},first_data_seconds=.01,message_rtt_seconds=[.001]*r['workload']['measured_messages'],bulk_verified_bytes=r['workload']['bulk_bytes'],bulk_seconds=.01)
print(json.dumps(dict(base,event='completed',measurement=measurement)),flush=True)
"#).unwrap();
    let config = root.0.join("adapter.json");
    fs::write(&config,serde_json::to_vec(&json!({"schema_version":1,"kind":"transport_adapter_config","adapter":{"name":"candidate","argv":["python3",script,mode]}})).unwrap()).unwrap();
    config
}
fn namespaces() -> std::collections::BTreeSet<String> {
    String::from_utf8(
        Command::new("ip")
            .args(["netns", "list"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .filter_map(|line| line.split_whitespace().next())
    .filter(|name| name.starts_with("nb"))
    .map(str::to_owned)
    .collect()
}
#[test]
#[ignore = "requires root, Linux namespaces, tc and python3; synthetic fixture checks rejection, not delivery"]
fn stale_readiness_is_a_contract_error_and_cannot_pass_conformance() {
    let before = namespaces();
    let root = Temp::new();
    let input = fixture(&root, "stale");
    let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("conform")
        .arg(input)
        .arg("--artifacts")
        .arg(root.0.join("out"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: Value =
        serde_json::from_slice(&fs::read(root.0.join("out/report.json")).unwrap()).unwrap();
    assert!(report["complete"].as_bool().unwrap());
    assert!(report["verdicts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v["observed"] == "infrastructure_failed" && !v["passed"].as_bool().unwrap()));
    assert_eq!(namespaces(), before);
}
#[test]
#[ignore = "requires root, Linux namespaces, tc and python3; synthetic fixture checks cancellation and cleanup"]
fn cancellation_retains_all_planned_verdicts_and_cleans_active_fixture() {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let before = namespaces();
    let root = Temp::new();
    let input = fixture(&root, "slow");
    let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("conform")
        .arg(input)
        .arg("--artifacts")
        .arg(root.0.join("out"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let log = root
        .0
        .join("out/runs/attempt-0001/evidence/case-000/client-1.stdout.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read_to_string(&log).is_ok_and(|text| text.contains("ready")) {
            break;
        }
        assert!(Instant::now() < deadline);
        assert!(child.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: signal this test's owned controller process only.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(130));
    let report: Value =
        serde_json::from_slice(&fs::read(root.0.join("out/report.json")).unwrap()).unwrap();
    assert!(!report["complete"].as_bool().unwrap() && report["interrupted"].as_bool().unwrap());
    assert_eq!(report["verdicts"].as_array().unwrap().len(), 8);
    assert!(report["verdicts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| !v["passed"].as_bool().unwrap()));
    assert_eq!(namespaces(), before);
}
