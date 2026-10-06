use natbench::{
    bench::{
        benchmark, collision, hairpin, lifetime, matrix, quic, throughput, webrtc,
        CollisionOptions, HairpinOptions, LifetimeOptions, Options, QuicOptions, ThroughputOptions,
        WebrtcOptions,
    },
    lab::{Lab, Profile, RouterInput},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn namespaces() -> BTreeSet<String> {
    let output = Command::new("ip").args(["netns", "list"]).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}
fn options() -> Options {
    Options {
        a: Profile::Preserve,
        b: Profile::Preserve,
        router_input: RouterInput::Drop,
        timeout_seconds: 1.,
        delay_ms: 0,
        loss_percent: 0,
        nest_a: false,
        translator: None,
        stun: false,
        executable: env!("CARGO_BIN_EXE_natbench").into(),
    }
}
fn wait_bounded(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not stop within eight seconds");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn real_network_matrix_and_lifecycle() {
    let before = namespaces();
    let results = matrix(&options()).unwrap();
    assert_eq!(results.len(), 9);
    for result in &results {
        assert_eq!(result["schema_version"], 1);
        assert_eq!(result["relay"]["bidirectional_before"], true, "{result}");
        assert_eq!(
            result["relay"]["bidirectional_after_restart"], true,
            "{result}"
        );
        for side in ["a", "b"] {
            assert_eq!(result["relay"]["outage_detected"][side], true, "{result}");
            let profile = result["profiles"][side].as_str().unwrap();
            let observation = &result["observations"][side];
            if profile == "udp-blocked" {
                assert_eq!(observation["mapping"], "unobserved");
            } else {
                assert_eq!(observation["filtering"], "address-and-port-dependent");
            }
            if profile == "preserve" {
                assert_eq!(observation["port_preserved"], true);
            }
        }
        if result["profiles"] == serde_json::json!({"a": "preserve", "b": "preserve"}) {
            assert_eq!(result["traversal"]["bidirectional"], true, "{result}");
            assert_eq!(result["traversal"]["after_observer_shutdown"]["a"], true);
            assert_eq!(result["traversal"]["after_observer_shutdown"]["b"], true);
        } else if result["profiles"]["a"] == "udp-blocked"
            || result["profiles"]["b"] == "udp-blocked"
            || result["profiles"] == serde_json::json!({"a": "random", "b": "random"})
        {
            assert_eq!(result["traversal"]["bidirectional"], false, "{result}");
        }
    }
    assert_eq!(before, namespaces());
    let mut collision = options();
    collision.router_input = RouterInput::Accept;
    let result = benchmark(&collision).unwrap();
    assert_eq!(result["traversal"]["bidirectional"], false, "{result}");
    assert_eq!(before, namespaces());

    // Force a user operation to fail while a child is running; dropping the fixture
    // must remove its processes and network resources on the error path.
    let failed = (|| -> anyhow::Result<()> {
        let mut lab = Lab::create(Profile::Preserve, Profile::Preserve, RouterInput::Drop)?;
        lab.spawn("a", &["sleep", "60"])?;
        lab.run("b", &["false"])?;
        Ok(())
    })();
    assert!(failed.is_err());
    assert_eq!(before, namespaces());

    let status = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .args(["run", "--", "false"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
    assert_eq!(before, namespaces());
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_natbench"))
            .args(["run", "--", "sh", "-c", "echo ready; sleep 60 & wait"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        reader.read_line(&mut ready).unwrap();
        assert_eq!(ready.trim(), "ready");
        // SAFETY: the child is a live, positively identified process we own.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        assert_eq!(wait_bounded(&mut child).code(), Some(130));
        assert_eq!(before, namespaces());
    }
    // Check the CLI output is parseable and the result schema remains usable.
    let output = Command::new(env!("CARGO_BIN_EXE_natbench"))
        .args(["bench", "--a", "udp-blocked", "--b", "random"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["traversal"]["bidirectional"], false);
    assert_eq!(result["relay"]["bidirectional_after_restart"], true);
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn mapping_expires_without_traffic_and_refreshes_from_either_direction() {
    let before = namespaces();
    let result = lifetime(&LifetimeOptions {
        profile: Profile::Preserve,
        router_input: RouterInput::Drop,
        udp_timeout_seconds: 3,
        executable: env!("CARGO_BIN_EXE_natbench").into(),
    })
    .unwrap();
    assert_eq!(result["experiment"], "mapping-lifetime");
    assert_eq!(result["established"], true, "{result}");
    assert_eq!(result["idle"]["conntrack_present"], false, "{result}");
    assert_eq!(result["idle"]["return_traffic"], false, "{result}");
    for phase in ["outbound_refresh", "inbound_refresh"] {
        assert_eq!(result[phase]["conntrack_present"], true, "{result}");
        assert_eq!(result[phase]["return_traffic"], true, "{result}");
    }
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn preserve_keeps_the_first_mapping_when_the_source_port_collides() {
    let before = namespaces();
    let result = collision(&CollisionOptions {
        profile: Profile::Preserve,
        router_input: RouterInput::Drop,
        executable: env!("CARGO_BIN_EXE_natbench").into(),
    })
    .unwrap();
    assert_eq!(result["experiment"], "port-collision");
    assert_eq!(result["behavior"], "port-preserving", "{result}");
    assert_eq!(result["first"]["port_preserved"], true, "{result}");
    assert_eq!(result["second"]["port_preserved"], false, "{result}");
    assert_eq!(
        result["first"]["conntrack_present_after_collision"], true,
        "{result}"
    );
    assert_eq!(
        result["first"]["mapped_endpoint_after_collision"], result["first"]["mapped_endpoint"],
        "{result}"
    );
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn masquerade_does_not_hairpin_to_the_public_mapping() {
    let before = namespaces();
    let result = hairpin(&HairpinOptions {
        profile: Profile::Preserve,
        router_input: RouterInput::Drop,
        executable: env!("CARGO_BIN_EXE_natbench").into(),
    })
    .unwrap();
    assert_eq!(result["experiment"], "hairpin");
    assert_eq!(result["established"], true, "{result}");
    assert_eq!(result["behavior"], "no-hairpin", "{result}");
    for direction in ["a_to_peer", "peer_to_a"] {
        assert_eq!(result[direction]["received"], false, "{result}");
        assert_eq!(result[direction]["behavior"], "no-hairpin", "{result}");
    }
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn wan_delay_still_lets_preserve_traverse() {
    let before = namespaces();
    let mut options = options();
    options.delay_ms = 20;
    options.timeout_seconds = 2.;
    let result = benchmark(&options).unwrap();
    assert_eq!(result["impairment"]["delay_ms"], 20);
    assert_eq!(result["traversal"]["bidirectional"], true, "{result}");
    assert_eq!(
        result["relay"]["bidirectional_after_restart"], true,
        "{result}"
    );
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn total_wan_loss_blocks_the_punch_after_discovery() {
    let before = namespaces();
    let mut options = options();
    options.loss_percent = 100;
    options.timeout_seconds = 1.;
    let result = benchmark(&options).unwrap();
    assert_eq!(
        result["observations"]["a"]["mapping"], "endpoint-independent",
        "{result}"
    );
    assert_eq!(result["relay"]["bidirectional_before"], true, "{result}");
    assert_eq!(result["traversal"]["bidirectional"], false, "{result}");
    assert_eq!(
        result["relay"]["bidirectional_after_restart"], false,
        "{result}"
    );
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn nested_preserve_still_maps_and_traverses() {
    let before = namespaces();
    let mut options = options();
    options.nest_a = true;
    options.timeout_seconds = 2.;
    let result = benchmark(&options).unwrap();
    assert_eq!(result["nested_a"], true);
    assert_eq!(result["relay"]["bidirectional_before"], true, "{result}");
    assert_eq!(
        result["observations"]["a"]["mapping"], "endpoint-independent",
        "{result}"
    );
    assert_eq!(
        result["observations"]["a"]["observed_endpoints"][0][0], "198.18.0.10",
        "{result}"
    );
    assert_eq!(result["traversal"]["bidirectional"], true, "{result}");
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn userspace_translator_filters_endpoint_independently() {
    let before = namespaces();
    let mut options = options();
    options.translator = Some(env!("CARGO_BIN_EXE_natbench").into());
    options.timeout_seconds = 2.;
    let result = benchmark(&options).unwrap();
    assert!(result["translator"].is_string(), "{result}");
    assert_eq!(result["relay"]["bidirectional_before"], true, "{result}");
    assert_eq!(
        result["observations"]["a"]["mapping"], "endpoint-independent",
        "{result}"
    );
    assert_eq!(
        result["observations"]["a"]["filtering"], "endpoint-independent",
        "{result}"
    );
    assert_eq!(
        result["observations"]["a"]["port_preserved"], true,
        "{result}"
    );
    assert_eq!(
        result["observations"]["a"]["observed_endpoints"][0][0], "198.18.0.10",
        "{result}"
    );
    assert_eq!(result["traversal"]["bidirectional"], true, "{result}");
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn stun_binding_learns_the_preserve_mapping_on_the_punch_socket() {
    let before = namespaces();
    let mut options = options();
    options.stun = true;
    options.timeout_seconds = 2.;
    let result = benchmark(&options).unwrap();
    assert_eq!(result["discovery"], "stun");
    assert_eq!(result["relay"]["bidirectional_before"], true, "{result}");
    assert_eq!(
        result["observations"]["a"]["mapping"], "endpoint-independent",
        "{result}"
    );
    assert_eq!(
        result["observations"]["a"]["filtering"], "address-and-port-dependent",
        "{result}"
    );
    assert_eq!(
        result["observations"]["a"]["port_preserved"], true,
        "{result}"
    );
    assert_eq!(
        result["observations"]["a"]["observed_endpoints"][0][0], "198.18.0.10",
        "{result}"
    );
    assert_eq!(result["traversal"]["bidirectional"], true, "{result}");
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn quic_handshake_on_the_stun_socket_survives_relay_shutdown() {
    let before = namespaces();
    let result = quic(&QuicOptions {
        timeout_seconds: 4.,
        executable: env!("CARGO_BIN_EXE_natbench").into(),
    })
    .unwrap();
    assert_eq!(result["experiment"], "quic");
    assert_eq!(result["discovery"], "stun");
    assert_eq!(result["mapped"]["a"][0], "198.18.0.10", "{result}");
    assert_eq!(result["mapped"]["b"][0], "198.18.0.20", "{result}");
    assert_eq!(result["data_before_relay_shutdown"], true, "{result}");
    assert_eq!(result["relay_shutdown"], true, "{result}");
    assert_eq!(result["data_after_relay_shutdown"], true, "{result}");
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn webrtc_data_channel_on_the_stun_socket_survives_relay_shutdown() {
    let before = namespaces();
    let result = webrtc(&WebrtcOptions {
        timeout_seconds: 8.,
        executable: env!("CARGO_BIN_EXE_natbench").into(),
    })
    .unwrap();
    assert_eq!(result["experiment"], "webrtc");
    assert_eq!(result["discovery"], "stun");
    assert_eq!(result["mapped"]["a"][0], "198.18.0.10", "{result}");
    assert_eq!(result["mapped"]["b"][0], "198.18.0.20", "{result}");
    assert_eq!(result["data_before_relay_shutdown"], true, "{result}");
    assert_eq!(result["relay_shutdown"], true, "{result}");
    assert_eq!(result["data_after_relay_shutdown"], true, "{result}");
    assert_eq!(before, namespaces());
}

#[test]
#[ignore = "requires root, Linux network namespaces, iproute2 and nftables"]
fn punched_udp_transfer_moves_the_requested_bytes() {
    let before = namespaces();
    let bytes = 256 * 1024;
    let result = throughput(&ThroughputOptions {
        bytes,
        chunk: 1200,
        timeout_seconds: 2.,
        executable: env!("CARGO_BIN_EXE_natbench").into(),
    })
    .unwrap();
    assert_eq!(result["experiment"], "throughput");
    assert_eq!(result["punch_bidirectional"], true, "{result}");
    assert_eq!(result["requested_bytes"], bytes, "{result}");
    assert_eq!(result["sent_bytes"], bytes, "{result}");
    assert_eq!(result["received_bytes"], bytes, "{result}");
    assert!(
        result["receive_bytes_per_second"].as_f64().unwrap() > 0.,
        "{result}"
    );
    assert_eq!(before, namespaces());
}
