use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("natbench-network-{}-{label}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn input(&self, mut value: Value) -> PathBuf {
        // Use a local no-op application; these tests validate input/orchestration,
        // while the real adapter verifier proves application delivery under shaping.
        if value["cases"][0].get("processes").is_none() {
            value["cases"][0]["processes"] =
                json!([{"name":"noop","role":"wan","argv":["true"],"timeout_seconds":3}]);
            value["cases"][0]["steps"] = json!([{"action":"run","process":"noop"}]);
        }
        let path = self.0.join("input.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn network() -> Value {
    json!({"links":[{"egress":"wan_router_a","conditions":{"delay_ms":20,"loss_percent":0}}]})
}
fn application() -> Value {
    json!({"schema_version":4,"cases":[{"name":"conditions","a":"preserve","b":"preserve","router_input":"drop","network":network()}]})
}
fn namespaces() -> std::collections::BTreeSet<String> {
    let result = Command::new("ip").args(["netns", "list"]).output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| n.starts_with("nb"))
        .map(str::to_owned)
        .collect()
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
#[test]
fn network_requires_new_versions_and_rejects_missing_invalid_or_unavailable_controls() {
    let root = Temp::new("invalid");
    let mut values = Vec::new();
    for version in [2, 3] {
        for network in [network(), Value::Null] {
            let mut value = application();
            value["schema_version"] = version.into();
            value["cases"][0]["network"] = network;
            values.push(value);
        }
    }
    let mut missing = application();
    missing["cases"][0]
        .as_object_mut()
        .unwrap()
        .remove("network");
    values.push(missing);
    let mut bad = application();
    bad["cases"][0]["network"]["links"][0]["conditions"]["delay_ms"] = 1001.into();
    values.push(bad);
    for (index, value) in values.into_iter().enumerate() {
        let input = root.input(value);
        let directory = root.0.join(format!("rejected-{index}"));
        let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .arg("test")
            .arg(input)
            .arg("--artifacts")
            .arg(&directory)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!directory.exists());
    }
    let input = root.input(application());
    let directory = root.0.join("missing-tc");
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(input)
        .arg("--artifacts")
        .arg(&directory)
        .env("PATH", "/missing-tools")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("require tc"));
    assert!(!directory.exists());
    let source = json!({"schema_version":1,"kind":"transport_comparison","adapters":[{"name":"a","argv":["true"]},{"name":"b","argv":["true"]}],"cases":[{"name":"case","profile":"preserve","deadline_ms":1000,"workload":{"payload_bytes":128,"warmup_messages":0,"measured_messages":1,"bulk_bytes":1024},"network":null}]});
    let path = root.0.join("compare.json");
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("compare")
        .arg(&path)
        .arg("--artifacts")
        .arg(root.0.join("old-compare"))
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(!root.0.join("old-compare").exists());
}
#[test]
#[ignore = "requires root, namespaces and tc"]
fn application_network_changes_are_fixture_errors_and_retain_kernel_evidence() {
    let before = namespaces();
    let root = Temp::new("drift");
    let mut value = application();
    value["cases"][0]["processes"] = json!([{"name":"change","role":"wan","argv":["tc","qdisc","change","dev","a","root","handle","1:","netem","delay","1ms"],"timeout_seconds":3}]);
    value["cases"][0]["steps"] = json!([{"action":"run","process":"change"}]);
    let input = root.input(value);
    let artifacts = root.0.join("evidence");
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(input)
        .arg("--artifacts")
        .arg(&artifacts)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        read(&artifacts.join("report.json"))["cases"][0]["status"],
        "infrastructure_failed"
    );
    let evidence = read(&artifacts.join("case-000/network.json"));
    assert_eq!(evidence["configured"], true);
    assert_eq!(evidence["complete"], false);
    assert!(evidence["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap().contains("configuration changed")));
    assert!(evidence["links"][0]["after"].is_array());
    assert_eq!(before, namespaces());
}
#[test]
#[ignore = "requires root, namespaces, tc and shell"]
fn second_link_setup_failure_preserves_partial_evidence_and_cleans_up() {
    let before = namespaces();
    let root = Temp::new("setup-failure");
    let real = Command::new("sh")
        .args(["-c", "command -v tc"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
    let fake = root.0.join("tc");
    fs::write(&fake,format!("#!/bin/sh\ncase \"$*\" in *'qdisc add dev a '*) echo 'forced second-link setup failure' >&2; exit 1;; esac\nexec '{}' \"$@\"\n",real.replace('\'',"'\\''"))).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let mut value = application();
    value["cases"][0]["network"]["links"] = json!([{"egress":"router_a_wan","conditions":{"delay_ms":10,"loss_percent":0}},{"egress":"wan_router_a","conditions":{"delay_ms":10,"loss_percent":0}}]);
    let input = root.input(value);
    let artifacts = root.0.join("evidence");
    let path = std::env::join_paths(
        std::iter::once(root.0.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(input)
        .arg("--artifacts")
        .arg(&artifacts)
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let evidence = read(&artifacts.join("case-000/network.json"));
    assert_eq!(evidence["configured"], false);
    assert_eq!(evidence["complete"], false);
    assert_eq!(evidence["links"][0]["installed"], true);
    assert_eq!(evidence["links"][1]["installed"], false);
    assert!(evidence["links"][0]["after"].is_array());
    assert_eq!(before, namespaces());
}

fn scheduled() -> Value {
    json!({"schema_version":5,"cases":[{"name":"scheduled","a":"preserve","b":"preserve","router_input":"drop","network":{"links":[{"egress":"router_a_wan","conditions":{"delay_ms":0,"loss_percent":0}},{"egress":"wan_router_a","conditions":{"delay_ms":0,"loss_percent":0}}]},"processes":[{"name":"noop","role":"wan","argv":["true"],"timeout_seconds":3}],"steps":[{"action":"run","process":"noop"},{"action":"network_set","id":"outage","marker":"outage.json","network":{"links":[{"egress":"router_a_wan","conditions":{"delay_ms":0,"loss_percent":100}},{"egress":"wan_router_a","conditions":{"delay_ms":0,"loss_percent":100}}]}},{"action":"delay","seconds":0.01},{"action":"network_set","id":"restored","marker":"restored.json","network":{"links":[{"egress":"router_a_wan","conditions":{"delay_ms":0,"loss_percent":0}},{"egress":"wan_router_a","conditions":{"delay_ms":0,"loss_percent":0}}]}}]}]})
}
#[test]
fn scheduled_changes_reject_old_versions_colliding_markers_and_reserved_context() {
    let root = Temp::new("scheduled-invalid");
    let mut values = Vec::new();
    for version in [2, 3, 4] {
        let mut v = scheduled();
        v["schema_version"] = version.into();
        values.push(v);
    }
    for marker in [
        "../out.json",
        ".hidden.json",
        "network.json",
        "capture.json",
        "capture-config.json",
        "out..json",
    ] {
        let mut v = scheduled();
        v["cases"][0]["steps"][1]["marker"] = marker.into();
        values.push(v);
    }
    let mut v = scheduled();
    v["cases"][0]["steps"][3]["id"] = "outage".into();
    values.push(v);
    let mut v = scheduled();
    v["cases"][0]["steps"][3]["marker"] = "outage.json".into();
    values.push(v);
    let mut v = scheduled();
    v["cases"][0]["steps"][1]["network"]["links"][0]["egress"] = "client_b".into();
    values.push(v);
    for key in ["NATBENCH_CASE_ARTIFACTS", "NATBENCH_RUN_ID"] {
        let mut v = scheduled();
        v["cases"][0]["processes"][0]["env"] = json!({key:"spoofed"});
        values.push(v);
    }
    for (index, value) in values.into_iter().enumerate() {
        let input = root.input(value);
        let artifacts = root.0.join(format!("invalid-{index}"));
        let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .arg("test")
            .arg(input)
            .arg("--artifacts")
            .arg(&artifacts)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!artifacts.exists());
    }
}
#[test]
#[ignore = "requires root, namespaces and tc"]
fn scheduled_changes_preserve_initial_final_and_each_transition_evidence() {
    let before = namespaces();
    let root = Temp::new("scheduled");
    let input = root.input(scheduled());
    let artifacts = root.0.join("runs");
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(input)
        .arg("--artifacts")
        .arg(&artifacts)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let evidence = read(&artifacts.join("case-000/network.json"));
    assert_eq!(evidence["schema_version"], 2);
    assert_eq!(evidence["complete"], true);
    let schema: Value =
        serde_json::from_str(include_str!("../schemas/network-conditions-v2.schema.json")).unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&evidence)
        .unwrap();
    let events = evidence["transitions"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    for event in events {
        assert_eq!(event["complete"], true);
        assert!(
            event["started_monotonic_ns"].as_u64().unwrap()
                <= event["applied_monotonic_ns"].as_u64().unwrap()
        );
        assert_eq!(
            event,
            &read(&artifacts.join(format!("case-000/{}.json", event["id"].as_str().unwrap())))
        );
        let schema: Value =
            serde_json::from_str(include_str!("../schemas/network-transition-v1.schema.json"))
                .unwrap();
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(event)
            .unwrap();
    }
    assert_eq!(events[0]["run_id"], events[1]["run_id"]);
    for link in evidence["links"].as_array().unwrap() {
        assert!(link["initial_configured"].is_array());
        assert_eq!(link["installed"], false);
        assert_eq!(
            link["current_conditions"]["loss_percent"].as_f64(),
            Some(0.)
        );
    }
    assert_eq!(before, namespaces());
}
#[test]
#[ignore = "requires root, namespaces, tc and shell"]
fn failed_scheduled_second_link_keeps_partial_change_and_no_success_marker() {
    let before = namespaces();
    let root = Temp::new("scheduled-partial");
    let real = Command::new("sh")
        .args(["-c", "command -v tc"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
    let fake = root.0.join("tc");
    fs::write(&fake,format!("#!/bin/sh\ncase \"$*\" in *'qdisc replace dev a '*) echo 'forced second-link change failure' >&2; exit 1;; esac\nexec '{}' \"$@\"\n",real.replace('\'',"'\\''"))).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let input = root.input(scheduled());
    let artifacts = root.0.join("runs");
    let path = std::env::join_paths(
        std::iter::once(root.0.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(input)
        .arg("--artifacts")
        .arg(&artifacts)
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(!artifacts.join("case-000/outage.json").exists());
    let evidence = read(&artifacts.join("case-000/network.json"));
    assert_eq!(evidence["complete"], false);
    let transition = &evidence["transitions"][0];
    assert_eq!(transition["complete"], false);
    assert_eq!(transition["links"][0]["applied"], true);
    assert_eq!(transition["links"][1]["applied"], false);
    assert!(transition["error"]
        .as_str()
        .unwrap()
        .contains("forced second-link"));
    assert!(evidence["links"][0]["after"].is_array());
    assert_eq!(before, namespaces());
}
#[test]
#[ignore = "requires root, namespaces and tc"]
fn cancellation_during_outage_retains_transition_and_cleans_fixture() {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let before = namespaces();
    let root = Temp::new("scheduled-cancel");
    let mut value = scheduled();
    value["cases"][0]["steps"][2]["seconds"] = 30.into();
    let input = root.input(value);
    let artifacts = root.0.join("runs");
    let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(input)
        .arg("--artifacts")
        .arg(&artifacts)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !artifacts.join("case-000/outage.json").exists() {
        assert!(child.try_wait().unwrap().is_none());
        if started.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            panic!("outage marker timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: signal only the test's owned child process.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            panic!("cancellation timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(child.wait().unwrap().code(), Some(130));
    let evidence = read(&artifacts.join("case-000/network.json"));
    assert_eq!(evidence["interrupted"], true);
    assert_eq!(evidence["complete"], true);
    let report = read(&artifacts.join("report.json"));
    assert_eq!(report["interrupted"], true);
    assert_eq!(report["complete"], false);
    assert_eq!(evidence["transitions"].as_array().unwrap().len(), 1);
    assert!(evidence["links"][0]["after"].is_array());
    assert!(!artifacts.join("case-000/restored.json").exists());
    assert_eq!(before, namespaces());
}
#[test]
#[ignore = "requires root, namespaces and tc"]
fn drift_before_scheduled_change_is_a_fixture_failure() {
    let before = namespaces();
    let root = Temp::new("scheduled-drift");
    let mut value = scheduled();
    value["cases"][0]["network"]["links"][0]["conditions"]["delay_ms"] = 20.into();
    value["cases"][0]["processes"][0]["role"] = "ra".into();
    value["cases"][0]["processes"][0]["argv"] = json!([
        "tc", "qdisc", "change", "dev", "wan", "root", "handle", "1:", "netem", "delay", "1ms"
    ]);
    let input = root.input(value);
    let artifacts = root.0.join("runs");
    let result = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(input)
        .arg("--artifacts")
        .arg(&artifacts)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let evidence = read(&artifacts.join("case-000/network.json"));
    assert_eq!(evidence["complete"], false);
    assert_eq!(evidence["transitions"][0]["complete"], false);
    assert!(evidence["transitions"][0]["error"]
        .as_str()
        .unwrap()
        .contains("qdisc drift"));
    assert!(!artifacts.join("case-000/outage.json").exists());
    assert_eq!(before, namespaces());
}
