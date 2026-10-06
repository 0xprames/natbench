use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
fn scenario(processes: Value, steps: Value) -> Value {
    json!({"schema_version":2,"cases":[{"name":"external programs","a":"preserve","b":"random","router_input":"drop","processes":processes,"steps":steps}]})
}
fn program(name: &str, argv: Value, timeout: f64, ready: Value) -> Value {
    json!({"name":name,"role":"wan","argv":argv,"timeout_seconds":timeout,"ready":ready})
}
fn namespace_snapshot() -> std::collections::BTreeSet<String> {
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

struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("natbench-app-{}-{label}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn invoke(root: &Path, suite: &Value, name: &str) -> (i32, Value) {
    let source = root.join(format!("{name}.json"));
    fs::write(&source, serde_json::to_vec(suite).unwrap()).unwrap();
    let artifacts = root.join(name);
    let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(source)
        .arg("--artifacts")
        .arg(&artifacts)
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    let report = fs::read(artifacts.join("report.json"))
        .ok()
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap_or(Value::Null);
    (code, report)
}
#[test]
fn invalid_application_plan_is_rejected_before_resources() {
    let root = Temp::new("invalid");
    let suite = scenario(
        json!([program(
            "service",
            json!(["sh", "-c", "sleep 60"]),
            1.,
            Value::Null
        )]),
        json!([{"action":"start","process":"service"}]),
    );
    assert_eq!(invoke(&root.0, &suite, "missing-ready").0, 2);
    assert!(!root.0.join("missing-ready").exists());
    let suite = scenario(
        json!([program("bad/name", json!(["true"]), 1., Value::Null)]),
        json!([{"action":"run","process":"bad/name"}]),
    );
    assert_eq!(invoke(&root.0, &suite, "unsafe-name").0, 2);
    assert!(!root.0.join("unsafe-name").exists());
}

#[test]
fn invalid_waits_and_delays_are_rejected_before_resources() {
    let root = Temp::new("invalid-waits");
    for (name, steps) in [
        ("zero-delay", json!([{"action":"delay","seconds":0}])),
        ("large-delay", json!([{"action":"delay","seconds":3601}])),
        (
            "unstarted",
            json!([{"action":"wait_exit","process":"job","timeout_seconds":1}]),
        ),
        (
            "missing-process",
            json!([{"action":"wait_stdout","process":"missing","text":"READY","timeout_seconds":1}]),
        ),
        (
            "empty-pattern",
            json!([{"action":"start","process":"job"},{"action":"wait_stdout","process":"job","text":"","timeout_seconds":1}]),
        ),
        (
            "bad-timeout",
            json!([{"action":"start","process":"job"},{"action":"wait_exit","process":"job","timeout_seconds":-1}]),
        ),
        (
            "double-exit",
            json!([{"action":"start","process":"job"},{"action":"wait_exit","process":"job","timeout_seconds":1},{"action":"stop","process":"job"}]),
        ),
    ] {
        let mut suite = scenario(
            json!([program(
                "job",
                json!(["true"]),
                1.,
                json!({"kind":"stdout_contains","text":"READY"})
            )]),
            steps,
        );
        suite["schema_version"] = json!(3);
        assert_eq!(invoke(&root.0, &suite, name).0, 2, "{name}");
        assert!(!root.0.join(name).exists(), "{name}");
    }
    let suite = scenario(
        json!([program("job", json!(["true"]), 1., Value::Null)]),
        json!([{"action":"delay","seconds":0.1}]),
    );
    assert_eq!(invoke(&root.0, &suite, "schema-two").0, 2);
    assert!(!root.0.join("schema-two").exists());
}

#[test]
#[ignore = "requires root and Linux network namespaces"]
fn asynchronous_output_and_completion_survive_delays_and_keep_verdicts() {
    let before = namespace_snapshot();
    let root = Temp::new("waits");
    let ready = json!({"kind":"stdout_contains","text":"READY"});
    let mut suite = scenario(json!([
        program("service", json!(["sh","-c","echo READY; sleep 60"]), 2., ready.clone()),
        program("job", json!(["sh","-c","echo READY; while [ ! -f finish ]; do sleep 0.01; done; printf RE; sleep 0.05; printf SULT; exit 7"]), 2., ready.clone()),
        program("release", json!(["touch","finish"]), 2., Value::Null)
    ]), json!([
        {"action":"start","process":"service"},
        {"action":"start","process":"job"},
        {"action":"run","process":"release"},
        {"action":"wait_stdout","process":"job","text":"RESULT","timeout_seconds":2},
        {"action":"delay","seconds":0.1},
        {"action":"wait_exit","process":"job","timeout_seconds":2,"expect_exit":7,"stdout_contains":"RESULT"},
        {"action":"stop","process":"service"}
    ]));
    suite["schema_version"] = json!(3);
    let (code, report) = invoke(&root.0, &suite, "passed");
    assert_eq!(code, 0, "{report}");
    let events = report["cases"][0]["observation"]["events"]
        .as_array()
        .unwrap();
    assert!(events
        .iter()
        .any(|event| event["state"] == "stdout_matched"));
    let delay_start = events
        .iter()
        .find(|event| event["state"] == "delay_started")
        .unwrap()["elapsed_ms"]
        .as_u64()
        .unwrap();
    let delay_end = events
        .iter()
        .find(|event| event["state"] == "delay_completed")
        .unwrap()["elapsed_ms"]
        .as_u64()
        .unwrap();
    assert!(delay_end - delay_start >= 99);
    fs::remove_file(root.0.join("finish")).unwrap();
    suite["cases"][0]["steps"][5]["expect_exit"] = json!(0);
    let (code, report) = invoke(&root.0, &suite, "wrong-exit");
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["cases"][0]["status"], "assertion_failed");
    assert!(report["cases"][0]["messages"][0]
        .as_str()
        .unwrap()
        .contains("expected exit 0"));

    // A short asynchronous command may exit before the next controller poll.
    let mut fast = scenario(
        json!([program(
            "job",
            json!(["sh", "-c", "echo READY; echo RESULT; exit 7"]),
            2.,
            ready.clone()
        )]),
        json!([
            {"action":"start","process":"job"},
            {"action":"delay","seconds":0.05},
            {"action":"wait_stdout","process":"job","text":"RESULT","timeout_seconds":1},
            {"action":"wait_exit","process":"job","timeout_seconds":1,"expect_exit":7}
        ]),
    );
    fast["schema_version"] = json!(3);
    let (code, report) = invoke(&root.0, &fast, "early-exit");
    assert_eq!(code, 0, "{report}");

    let mut suite = scenario(
        json!([program(
            "job",
            json!(["sh", "-c", "echo READY; sleep 60"]),
            2.,
            ready.clone()
        )]),
        json!([
            {"action":"start","process":"job"},
            {"action":"wait_stdout","process":"job","text":"missing","timeout_seconds":0.1},
            {"action":"wait_exit","process":"job","timeout_seconds":1}
        ]),
    );
    suite["schema_version"] = json!(3);
    let (code, report) = invoke(&root.0, &suite, "missing-output");
    assert_eq!(code, 1, "{report}");
    assert!(root
        .0
        .join("missing-output/case-000/job-1.stdout.log")
        .exists());
    suite["cases"][0]["steps"] = json!([
        {"action":"start","process":"job"},
        {"action":"wait_exit","process":"job","timeout_seconds":0.1}
    ]);
    let (code, report) = invoke(&root.0, &suite, "exit-timeout");
    assert_eq!(code, 2, "{report}");
    assert!(report["cases"][0]["messages"][0]
        .as_str()
        .unwrap()
        .contains("step 2 for process job"));
    assert_eq!(before, namespace_snapshot());
}

#[test]
#[ignore = "requires root and Linux network namespaces"]
fn waits_supervise_services_and_do_not_hide_unexpected_exits() {
    let before = namespace_snapshot();
    let root = Temp::new("wait-supervision");
    for (name, wait) in [
        ("delay", json!({"action":"delay","seconds":1})),
        (
            "output",
            json!({"action":"wait_stdout","process":"job","text":"missing","timeout_seconds":1}),
        ),
        (
            "exit",
            json!({"action":"wait_exit","process":"job","timeout_seconds":1}),
        ),
    ] {
        let mut suite = scenario(
            json!([
                program(
                    "service",
                    json!([
                        "sh",
                        "-c",
                        "echo READY; while [ ! -f fail ]; do sleep 0.01; done; sleep 0.1; exit 9"
                    ]),
                    2.,
                    json!({"kind":"stdout_contains","text":"READY"})
                ),
                program(
                    "job",
                    json!(["sh", "-c", "echo READY; sleep 60"]),
                    2.,
                    json!({"kind":"stdout_contains","text":"READY"})
                ),
                program("release", json!(["touch", "fail"]), 2., Value::Null)
            ]),
            json!([
                {"action":"start","process":"service"},
                {"action":"start","process":"job"},
                {"action":"run","process":"release"},
                wait,
                {"action":"stop","process":"service"}
            ]),
        );
        suite["schema_version"] = json!(3);
        let (code, report) = invoke(&root.0, &suite, name);
        assert_eq!(code, 2, "{report}");
        assert!(report["cases"][0]["messages"][0]
            .as_str()
            .unwrap()
            .contains("service service exited unexpectedly"));
        fs::remove_file(root.0.join("fail")).unwrap();
    }
    assert_eq!(before, namespace_snapshot());
}

#[test]
#[ignore = "requires root and Linux network namespaces"]
fn cancellation_interrupts_each_wait_and_preserves_its_timeline() {
    let before = namespace_snapshot();
    let root = Temp::new("wait-cancel");
    for (name, step, event) in [
        (
            "delay",
            json!({"action":"delay","seconds":60}),
            "delay_started",
        ),
        (
            "output",
            json!({"action":"wait_stdout","process":"job","text":"never","timeout_seconds":60}),
            "stdout_wait_started",
        ),
        (
            "exit",
            json!({"action":"wait_exit","process":"job","timeout_seconds":60}),
            "exit_wait_started",
        ),
    ] {
        let mut suite = scenario(
            json!([program(
                "job",
                json!(["sh", "-c", "echo READY; sleep 60"]),
                2.,
                json!({"kind":"stdout_contains","text":"READY"})
            )]),
            json!([
                {"action":"start","process":"job"}, step
            ]),
        );
        suite["schema_version"] = json!(3);
        let source = root.0.join(format!("{name}.json"));
        fs::write(&source, serde_json::to_vec(&suite).unwrap()).unwrap();
        let artifacts = root.0.join(name);
        let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .arg("test")
            .arg(source)
            .arg("--artifacts")
            .arg(&artifacts)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !fs::read_to_string(artifacts.join("case-000/timeline.jsonl"))
            .is_ok_and(|text| text.contains(event))
        {
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{name} wait did not start");
            }
            thread::sleep(Duration::from_millis(10));
        }
        // SAFETY: this is the positive PID of the child owned by the test.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(130), "{name}");
                break;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{name} cancellation timed out");
            }
            thread::sleep(Duration::from_millis(10));
        }
        let report: Value =
            serde_json::from_slice(&fs::read(artifacts.join("report.json")).unwrap()).unwrap();
        assert_eq!(report["interrupted"], true, "{name}");
        assert_eq!(report["complete"], false, "{name}");
        assert!(
            fs::read_to_string(artifacts.join("case-000/job-1.stdout.log"))
                .unwrap()
                .contains("READY")
        );
        assert_eq!(before, namespace_snapshot(), "{name}");
    }
}
#[test]
#[ignore = "requires root and Linux network namespaces"]
fn commands_readiness_restart_and_failures_keep_logs_and_cleanup() {
    let before = namespace_snapshot();
    let root = Temp::new("lifecycle");
    let ready = json!({"kind":"stdout_contains","text":"READY"});
    let mut client = program(
        "client",
        json!(["sh", "-c", "printf '%s:%s' \"$MESSAGE\" \"$PWD\""]),
        2.,
        Value::Null,
    );
    client["cwd"] = json!("work");
    client["env"] = json!({"MESSAGE":"hello","PATH":"/bin"});
    fs::create_dir(root.0.join("work")).unwrap();
    let suite = scenario(
        json!([
            program(
                "service",
                json!(["sh", "-c", "printf READY; sleep 60"]),
                2.,
                ready
            ),
            client
        ]),
        json!([
 {"action":"start","process":"service"},{"action":"run","process":"client","stdout_contains":"hello:"},{"action":"restart","process":"service"},{"action":"run","process":"client","stdout_contains":"/work"},{"action":"stop","process":"service"}]),
    );
    let (code, report) = invoke(&root.0, &suite, "passed");
    assert_eq!(code, 0, "{report}");
    assert_eq!(
        report["cases"][0]["observation"]["experiment"],
        "application"
    );
    assert!(root.0.join("passed/case-000/service-2.stdout.log").exists());
    assert!(root.0.join("passed/case-000/client-2.stderr.log").exists());
    assert!(
        fs::read_to_string(root.0.join("passed/case-000/timeline.jsonl"))
            .unwrap()
            .contains("stopped")
    );
    let suite = scenario(
        json!([program(
            "client",
            json!(["sh", "-c", "echo wrong; exit 7"]),
            2.,
            Value::Null
        )]),
        json!([{"action":"run","process":"client","expect_exit":0,"stdout_contains":"PASS"}]),
    );
    let (code, report) = invoke(&root.0, &suite, "assertion");
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["cases"][0]["status"], "assertion_failed");
    let suite = scenario(
        json!([program(
            "service",
            json!(["sleep", "60"]),
            0.1,
            json!({"kind":"stdout_contains","text":"never"})
        )]),
        json!([{"action":"start","process":"service"}]),
    );
    let (code, report) = invoke(&root.0, &suite, "ready-timeout");
    assert_eq!(code, 2, "{report}");
    assert!(report["cases"][0]["messages"][0]
        .as_str()
        .unwrap()
        .contains("step 1 for process service"));
    let suite = scenario(
        json!([program("client", json!(["sleep", "60"]), 0.1, Value::Null)]),
        json!([{"action":"run","process":"client"}]),
    );
    assert_eq!(invoke(&root.0, &suite, "execution-timeout").0, 2);
    let suite = scenario(
        json!([program(
            "client",
            json!(["/does-not-exist"]),
            1.,
            Value::Null
        )]),
        json!([{"action":"run","process":"client"}]),
    );
    assert_eq!(invoke(&root.0, &suite, "missing-binary").0, 2);
    let suite = scenario(
        json!([
            program(
                "service",
                json!([
                    "sh",
                    "-c",
                    "sleep 60 & echo $! > child.pid; echo READY; wait"
                ]),
                2.,
                json!({"kind":"stdout_contains","text":"READY"})
            ),
            program(
                "save",
                json!(["sh", "-c", "cp child.pid previous.pid"]),
                2.,
                Value::Null
            ),
            program(
                "verify",
                json!(["sh", "-c", "test ! -e /proc/$(cat previous.pid)/fd/1"]),
                2.,
                Value::Null
            )
        ]),
        json!([{"action":"start","process":"service"},{"action":"run","process":"save"},{"action":"restart","process":"service"},{"action":"run","process":"verify"},{"action":"stop","process":"service"}]),
    );
    let (code, report) = invoke(&root.0, &suite, "children");
    assert_eq!(code, 0, "{report}");
    assert_eq!(before, namespace_snapshot());
}
#[test]
#[ignore = "requires root, Linux namespaces and python3"]
fn tcp_readiness_probe_uses_the_service_namespace() {
    let before = namespace_snapshot();
    let root = Temp::new("tcp");
    let suite = scenario(
        json!([program(
            "http",
            json!([
                "python3",
                "-m",
                "http.server",
                "9876",
                "--bind",
                "127.0.0.1"
            ]),
            3.,
            json!({"kind":"tcp","address":"127.0.0.1:9876"})
        )]),
        json!([{"action":"start","process":"http"},{"action":"stop","process":"http"}]),
    );
    let (code, report) = invoke(&root.0, &suite, "passed");
    assert_eq!(code, 0, "{report}");
    assert_eq!(before, namespace_snapshot());
}
#[test]
#[ignore = "requires root and Linux namespaces"]
fn cancellation_keeps_application_logs_and_partial_report() {
    let before = namespace_snapshot();
    let root = Temp::new("cancel");
    let suite = scenario(
        json!([
            program(
                "service",
                json!(["sh", "-c", "echo READY; sleep 60"]),
                2.,
                json!({"kind":"stdout_contains","text":"READY"})
            ),
            program("client", json!(["sleep", "60"]), 30., Value::Null)
        ]),
        json!([{"action":"start","process":"service"},{"action":"run","process":"client"}]),
    );
    let source = root.0.join("suite.json");
    fs::write(&source, serde_json::to_vec(&suite).unwrap()).unwrap();
    let artifacts = root.0.join("artifacts");
    let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(source)
        .arg("--artifacts")
        .arg(&artifacts)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !artifacts.join("case-000/client-1.stdout.log").exists() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("application did not start");
        }
        thread::sleep(Duration::from_millis(25));
    }
    // SAFETY: this is the positive PID of the child owned by the test.
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
        thread::sleep(Duration::from_millis(25));
    }
    let report: Value =
        serde_json::from_slice(&fs::read(artifacts.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["interrupted"], true);
    assert_eq!(report["active_case"], "external programs");
    assert!(
        fs::read_to_string(artifacts.join("case-000/timeline.jsonl"))
            .unwrap()
            .contains("ready")
    );
    assert_eq!(before, namespace_snapshot());
}

#[test]
#[ignore = "requires root and Linux namespaces"]
fn terminal_interrupt_during_namespace_creation_does_not_leak() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    let before = namespace_snapshot();
    let root = Temp::new("setup-interrupt");
    let marker = root.0.join("created");
    let wrapper = root.0.join("ip");
    // The generated paths use only our fixed ASCII test prefix and numeric PID.
    let script=format!("#!/bin/sh\n/usr/sbin/ip \"$@\"\nresult=$?\nif [ \"$1\" = netns ] && [ \"$2\" = add ] && [ \"$result\" = 0 ]; then\n printf '%s' \"$3\" > '{}'\n sleep 0.3\nfi\nexit \"$result\"\n",marker.display());
    fs::write(&wrapper, script).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let suite = scenario(
        json!([program("client", json!(["true"]), 1., Value::Null)]),
        json!([{"action":"run","process":"client"}]),
    );
    let source = root.0.join("suite.json");
    fs::write(&source, serde_json::to_vec(&suite).unwrap()).unwrap();
    let path = format!("{}:{}", root.0.display(), std::env::var("PATH").unwrap());
    let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .arg("test")
        .arg(source)
        .arg("--artifacts")
        .arg(root.0.join("artifacts"))
        .env("PATH", path)
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("setup marker missing");
        }
        thread::sleep(Duration::from_millis(5));
    }
    // SAFETY: the child has a dedicated process group; this simulates terminal Ctrl-C.
    assert_eq!(unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) }, 0);
    let status = child.wait().unwrap();
    let after = namespace_snapshot();
    if before != after {
        // Clean only the namespace named by this test's successful create operation.
        let name = fs::read_to_string(marker).unwrap();
        if name.starts_with("nb") {
            let _ = Command::new("/usr/sbin/ip")
                .args(["netns", "del", &name])
                .output();
        }
    }
    assert_eq!(status.code(), Some(130));
    assert_eq!(before, after);
}
