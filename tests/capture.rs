use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("natbench-capture-{}-{label}", std::process::id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn namespaces() -> std::collections::BTreeSet<String> {
    let r = Command::new("ip").args(["netns", "list"]).output().unwrap();
    assert!(r.status.success());
    String::from_utf8(r.stdout)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| n.starts_with("nb"))
        .map(str::to_owned)
        .collect()
}
fn program(name: &str, argv: Value, ready: Value) -> Value {
    json!({"name":name,"role":"wan","argv":argv,"timeout_seconds":4,"ready":ready})
}
fn suite(processes: Value, steps: Value) -> Value {
    json!({"schema_version":3,"cases":[{"name":"capture fixture","a":"preserve","b":"preserve","router_input":"drop","processes":processes,"steps":steps}]})
}
fn invoke(root: &Path, input: &Value, label: &str, capture: &str) -> (i32, Value) {
    let source = root.join(format!("{label}.json"));
    fs::write(&source, serde_json::to_vec(input).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(source)
        .arg(capture)
        .arg("--artifacts")
        .arg(root.join(label))
        .output()
        .unwrap();
    let report = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), report)
}
#[test]
fn invalid_or_unavailable_capture_fails_before_resources() {
    let root = Temp::new("invalid");
    let input = suite(
        json!([program("client", json!(["true"]), Value::Null)]),
        json!([{"action":"run","process":"client"}]),
    );
    assert_eq!(invoke(&root.0, &input, "zero", "--capture=0").0, 2);
    assert!(!root.0.join("zero").exists());
    let source = root.0.join("suite.json");
    fs::write(&source, serde_json::to_vec(&input).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(&source)
        .arg("--capture")
        .arg("--artifacts")
        .arg(root.0.join("missing"))
        .env("PATH", "/missing-natbench-tools")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("requires tcpdump"));
    assert!(!root.0.join("missing").exists());
    let out = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scenarios/connectivity.json"
        ))
        .arg("--capture")
        .arg("--artifacts")
        .arg(root.0.join("builtin"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("application input"));
    assert!(!root.0.join("builtin").exists());
}
#[test]
#[ignore = "requires root, namespaces, tcpdump and the built Go UDP example"]
fn captures_show_delivery_and_locate_a_blocked_udp_failure() {
    let before = namespaces();
    let root = Temp::new("delivery");
    let binary = fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/udp-echo/udp-echo"
    ))
    .unwrap();
    let mut client = program("client", json!([binary, "--mode", "client"]), Value::Null);
    client["role"] = json!("a");
    let mut input = suite(
        json!([
            program(
                "server",
                json!([binary, "--mode", "server"]),
                json!({"kind":"stdout_contains","text":"READY"})
            ),
            client
        ]),
        json!([{"action":"start","process":"server"},{"action":"run","process":"client","stdout_contains":"PASS"},{"action":"stop","process":"server"}]),
    );
    let mut blocked = input["cases"][0].clone();
    blocked["name"] = json!("blocked UDP regression");
    blocked["a"] = json!("udp-blocked");
    input["cases"].as_array_mut().unwrap().push(blocked);
    let (code, report) = invoke(&root.0, &input, "artifacts", "--capture");
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["cases"][0]["status"], "passed");
    assert_eq!(report["cases"][1]["status"], "assertion_failed");
    for case in ["case-000", "case-001"] {
        let directory = root.0.join("artifacts").join(case);
        let manifest: Value =
            serde_json::from_slice(&fs::read(directory.join("capture.json")).unwrap()).unwrap();
        assert_eq!(manifest["complete"], true);
        let schema: Value =
            serde_json::from_str(include_str!("../schemas/packet-capture-v1.schema.json")).unwrap();
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(&manifest)
            .unwrap();
        assert_eq!(manifest["files"].as_array().unwrap().len(), 5);
        for file in manifest["files"].as_array().unwrap() {
            let path = directory.join(file["pcap_file"].as_str().unwrap());
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert!(file["packets"].as_u64().unwrap() <= 1000);
        }
    }
    let packets = |case: &str, role: &str| {
        let r = Command::new("tcpdump")
            .args(["-nn", "-r"])
            .arg(
                root.0
                    .join("artifacts")
                    .join(case)
                    .join(format!("{role}.pcap")),
            )
            .args(["udp", "port", "9999"])
            .output()
            .unwrap();
        assert!(r.status.success());
        String::from_utf8(r.stdout).unwrap()
    };
    assert!(!packets("case-000", "wan").is_empty());
    assert!(!packets("case-001", "a").is_empty());
    assert!(packets("case-001", "wan").is_empty());
    assert_eq!(before, namespaces());
}
#[test]
#[ignore = "requires root, namespaces, tcpdump and python3"]
fn exhausted_packet_budget_and_startup_failures_keep_evidence() {
    let before = namespaces();
    let root = Temp::new("budget");
    let input = suite(
        json!([program(
            "client",
            json!([
                "python3",
                "-c",
                "import socket; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.sendto(b'packet-budget', ('198.18.0.10',9998)); print('PASS')"
            ]),
            Value::Null
        )]),
        json!([{"action":"run","process":"client","stdout_contains":"PASS"}]),
    );
    let (code, report) = invoke(&root.0, &input, "budget", "--capture=1");
    assert_eq!(code, 0, "{report}");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.0.join("budget/case-000/capture.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["complete"], true);
    assert!(manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["packet_limit_reached"] == true));
    assert!(manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|file| file["packets"].as_u64().unwrap() <= 1));
    let wrapper = root.0.join("tcpdump");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&wrapper)
        .unwrap();
    fs::write(
        &wrapper,
        "#!/bin/sh\necho 'fixture capture startup failure' >&2\nexit 7\n",
    )
    .unwrap();
    let source = root.0.join("failure.json");
    fs::write(&source, serde_json::to_vec(&input).unwrap()).unwrap();
    let path = format!("{}:{}", root.0.display(), std::env::var("PATH").unwrap());
    let out = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(source)
        .arg("--capture")
        .arg("--artifacts")
        .arg(root.0.join("failure"))
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(!root.0.join("failure/case-000/client-1.stdout.log").exists());
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.0.join("failure/case-000/capture.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["complete"], false);
    assert!(manifest["error"]
        .as_str()
        .unwrap()
        .contains("startup failure"));
    assert_eq!(before, namespaces());
}
#[test]
#[ignore = "requires root, namespaces and tcpdump"]
fn interruption_flushes_pcaps_before_namespace_cleanup() {
    let before = namespaces();
    let root = Temp::new("cancel");
    let mut client = program(
        "client",
        json!(["sh", "-c", "echo READY; sleep 60"]),
        Value::Null,
    );
    client["timeout_seconds"] = json!(60);
    let input = suite(
        json!([client]),
        json!([{"action":"run","process":"client"}]),
    );
    let source = root.0.join("suite.json");
    fs::write(&source, serde_json::to_vec(&input).unwrap()).unwrap();
    let artifacts = root.0.join("artifacts");
    let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(source)
        .arg("--capture")
        .arg("--artifacts")
        .arg(&artifacts)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fs::read_to_string(artifacts.join("case-000/client-1.stdout.log"))
        .is_ok_and(|s| s.contains("READY"))
    {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("captured application did not start");
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
            panic!("capture cancellation timed out");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let manifest: Value =
        serde_json::from_slice(&fs::read(artifacts.join("case-000/capture.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["complete"], true);
    assert!(manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["forced_stop"] == false));
    let report: Value =
        serde_json::from_slice(&fs::read(artifacts.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["interrupted"], true);
    assert_eq!(before, namespaces());
}
