use serde_json::Value;
use std::process::Command;

fn owned_namespaces() -> std::collections::BTreeSet<String> {
    let output = Command::new("ip").args(["netns", "list"]).output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.starts_with("nb"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn missing_tools_explain_remedies_without_creating_a_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .args(["doctor", "--probe", "--json"])
        .env("PATH", "/nonexistent-natbench-tools")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = report["checks"].as_array().unwrap();
    assert!(checks.iter().any(|c| c["name"] == "ip"
        && c["status"] == "failed"
        && c["remedy"].as_str().unwrap().contains("iproute2")));
    assert!(checks
        .iter()
        .any(|c| c["name"] == "kernel_fixture" && c["status"] == "not_checked"));
}

#[test]
#[ignore = "requires root, Linux namespaces, nftables, conntrack and tc"]
fn active_probe_checks_features_and_cleans_up() {
    let before = owned_namespaces();
    let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .args(["doctor", "--probe", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["kernel_fixture", "netem", "conntrack_table", "nfqueue"] {
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["name"] == name && c["status"] == "passed"),
            "{report}"
        );
    }
    let after = owned_namespaces();
    assert_eq!(before, after);
    // Probing an existing namespace from the parent must fail before binding NFQUEUE.
    let mut lab = natbench::lab::Lab::create(
        natbench::lab::Profile::Preserve,
        natbench::lab::Profile::Preserve,
        natbench::lab::RouterInput::Drop,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .args(["__nfqueue-probe", &lab.namespaces["ra"]])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must run inside"));
    // Confirm the helper can still bind inside the actual fixture afterwards.
    assert!(lab
        .spawn(
            "ra",
            &[
                env!("CARGO_BIN_EXE_natbench"),
                "__nfqueue-probe",
                &lab.namespaces["ra"].clone()
            ]
        )
        .unwrap()
        .wait()
        .unwrap()
        .success());
    drop(lab);
    assert_eq!(before, owned_namespaces());
    let read_only = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&read_only.stdout).unwrap();
    assert_eq!(report["active_probe"], false);
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["name"] == "kernel_fixture" && c["status"] == "not_checked"));
}
