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
