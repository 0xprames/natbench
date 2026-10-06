use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn namespaces() -> BTreeSet<String> {
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
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn load(path: &Path) -> Option<Value> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

#[test]
#[ignore = "requires root, Linux namespaces, iproute2 and nftables"]
fn signals_keep_completed_evidence_and_clean_active_fixture() {
    let before = namespaces();
    let retained = std::env::var_os("NATBENCH_TEST_ARTIFACTS").map(std::path::PathBuf::from);
    let base = retained.clone().unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let root = base.join(format!("natbench-cancel-{}-{signal}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let source = root.join("suite.json");
        let artifacts = root.join("artifacts");
        let case = |name: &str, a: &str, b: &str, timeout: u64, direct: bool| json!({"name":name,"a":a,"b":b,"router_input":"drop","timeout_seconds":timeout,"expect":[{"pointer":"/traversal/bidirectional","equals":direct}]});
        fs::write(&source, serde_json::to_vec(&json!({"schema_version":1,"cases":[case("completed", "preserve", "preserve",2,true),case("active","random","random",30,false),case("pending","preserve","preserve",2,true)]})).unwrap()).unwrap();
        let stdout = fs::File::create(root.join("stdout.json")).unwrap();
        let stderr = fs::File::create(root.join("stderr.log")).unwrap();
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_natbench"))
                .arg("test")
                .arg(&source)
                .arg("--artifacts")
                .arg(&artifacts)
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let snapshot = load(&artifacts.join("report.json"));
            let active = namespaces()
                .difference(&before)
                .cloned()
                .collect::<Vec<_>>();
            let pids = active
                .iter()
                .map(|ns| {
                    Command::new("ip")
                        .args(["netns", "pids", ns])
                        .output()
                        .unwrap()
                        .stdout
                        .split(|&c| c == b'\n')
                        .filter(|s| !s.is_empty())
                        .count()
                })
                .sum::<usize>();
            if snapshot.is_some_and(|s| {
                s["cases"].as_array().unwrap().len() == 1 && s["active_case"] == "active"
            }) && pids >= 4
            {
                break;
            }
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "suite exited before signal: {}",
                fs::read_to_string(root.join("stderr.log")).unwrap()
            );
            assert!(
                Instant::now() < deadline,
                "suite did not reach active fixture; evidence: {}",
                root.display()
            );
            thread::sleep(Duration::from_millis(25));
        }
        // SAFETY: this is the positive PID of the child process owned by this test.
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, signal) }, 0);
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert_eq!(status.code(), Some(130));
                break;
            }
            assert!(Instant::now() < deadline, "cancellation did not stop suite");
            thread::sleep(Duration::from_millis(25));
        }
        let report = load(&artifacts.join("report.json")).unwrap();
        let schema: Value =
            serde_json::from_str(include_str!("../schemas/suite-report-v2.schema.json")).unwrap();
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(&report)
            .unwrap();
        assert_eq!(report["schema_version"], 2);
        assert_eq!(report["complete"], false);
        assert_eq!(report["interrupted"], true);
        assert_eq!(report["active_case"], "active");
        assert_eq!(report["cases"].as_array().unwrap().len(), 1);
        assert_eq!(report["cases"][0]["status"], "passed");
        assert_eq!(
            report["cases"][0]["observation"]["traversal"]["bidirectional"],
            true
        );
        assert_eq!(load(&root.join("stdout.json")).unwrap(), report);
        let junit = fs::read_to_string(artifacts.join("junit.xml")).unwrap();
        assert!(
            junit.contains("tests=\"3\" failures=\"0\" errors=\"1\" skipped=\"1\""),
            "{junit}"
        );
        assert!(junit.contains("type=\"interrupted\""));
        assert!(junit.contains("name=\"pending\"><skipped"));
        assert_eq!(before, namespaces());
        assert!(!artifacts.join("report.checkpoint.tmp").exists());
        if retained.is_none() {
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn bad_input_is_rejected_before_artifact_creation() {
    let root = std::env::temp_dir().join(format!("natbench-bad-schema-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    for (index, input) in [
        json!({"schema_version":99,"cases":[]}),
        json!({"schema_version":1,"cases":[],"unknown":true}),
        json!({"schema_version":1,"cases":[]}),
    ]
    .into_iter()
    .enumerate()
    {
        let source = root.join("suite.json");
        let artifacts = root.join(format!("artifacts-{index}"));
        fs::write(&source, serde_json::to_vec(&input).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .arg("test")
            .arg(&source)
            .arg("--artifacts")
            .arg(&artifacts)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!artifacts.exists());
    }
    fs::remove_dir_all(root).unwrap();
}
