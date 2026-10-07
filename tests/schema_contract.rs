use natbench::scenario::Suite;
use serde_json::{json, Value};

fn validator(document: &str) -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(document).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}
#[test]
fn published_schemas_accept_current_inputs_and_completed_reports() {
    let scenario: Value =
        serde_json::from_str(include_str!("../scenarios/connectivity.json")).unwrap();
    let input = validator(include_str!("../schemas/scenario-v1.schema.json"));
    input.validate(&scenario).unwrap();
    let suite: Suite = serde_json::from_value(scenario).unwrap();
    let report = suite
        .evaluate("schema test".into(), |_| {
            Ok(json!({"traversal":{"bidirectional":true}}))
        })
        .unwrap();
    let report = serde_json::to_value(report).unwrap();
    let current = validator(include_str!("../schemas/suite-report-v2.schema.json"));
    current.validate(&report).unwrap();
    let old = validator(include_str!("../schemas/suite-report-v1.schema.json"));
    assert!(!old.is_valid(&report));
    let legacy = json!({"schema_version":1,"scenario":"legacy","cases":[]});
    old.validate(&legacy).unwrap();
    assert!(!current.is_valid(&legacy));
}
#[test]
fn output_allows_additive_metadata_but_input_rejects_unknown_fields() {
    let mut scenario: Value =
        serde_json::from_str(include_str!("../scenarios/connectivity.json")).unwrap();
    scenario["typo"] = json!(true);
    assert!(!validator(include_str!("../schemas/scenario-v1.schema.json")).is_valid(&scenario));
    assert!(serde_json::from_value::<Suite>(scenario).is_err());
    let checkpoint = json!({"schema_version":2,"scenario":"test","cases":[],"planned_cases":["a"],"active_case":null,"complete":false,"interrupted":false,"future_metadata":{"supported":true}});
    validator(include_str!("../schemas/suite-report-v2.schema.json"))
        .validate(&checkpoint)
        .unwrap();
}

#[test]
fn application_example_matches_its_declared_schema() {
    let instance: Value =
        serde_json::from_str(include_str!("../examples/udp-echo/scenario.json")).unwrap();
    validator(include_str!("../schemas/scenario-v2.schema.json"))
        .validate(&instance)
        .unwrap();
}

#[test]
fn recovery_example_requires_schema_three_and_preserves_schema_two() {
    let mut instance: Value =
        serde_json::from_str(include_str!("../examples/udp-recovery/scenario.json")).unwrap();
    let recovery = validator(include_str!("../schemas/scenario-v3.schema.json"));
    recovery.validate(&instance).unwrap();
    instance["schema_version"] = json!(2);
    assert!(!validator(include_str!("../schemas/scenario-v2.schema.json")).is_valid(&instance));
    let mut legacy: Value =
        serde_json::from_str(include_str!("../examples/udp-echo/scenario.json")).unwrap();
    legacy["schema_version"] = json!(3);
    recovery.validate(&legacy).unwrap();
}

#[test]
fn suite_report_two_retains_older_cases_without_optional_timing() {
    let legacy = json!({"schema_version":2,"scenario":"older build","cases":[{"name":"peer","status":"passed","messages":[],"observation":null}],"planned_cases":["peer"],"active_case":null,"complete":true,"interrupted":false});
    validator(include_str!("../schemas/suite-report-v2.schema.json"))
        .validate(&legacy)
        .unwrap();
    let report: natbench::scenario::Report = serde_json::from_value(legacy).unwrap();
    assert_eq!(report.cases[0].elapsed_seconds, None);
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn transport_inputs_are_closed_and_adapter_outputs_allow_additive_metadata() {
    use natbench_transport_protocol::{
        Event, EventData, Implementation, Measurement, Request, Role, Workload,
    };
    let input: Value =
        serde_json::from_str(include_str!("../examples/transports/direct.json")).unwrap();
    validator(include_str!(
        "../schemas/transport-comparison-v1.schema.json"
    ))
    .validate(&input)
    .unwrap();
    let request = Request {
        schema_version: 1,
        kind: "transport_request".into(),
        run_id: "0123456789abcdef0123456789abcdef".into(),
        role: Role::Client,
        listen_address: "0.0.0.0:0".parse().unwrap(),
        peer_address: "198.18.0.1:9443".parse().unwrap(),
        peer_file: "/tmp/peer.json".into(),
        deadline_ms: 5000,
        workload: Workload {
            payload_bytes: 128,
            warmup_messages: 1,
            measured_messages: 2,
            bulk_bytes: 1024,
        },
    };
    let mut request_json = serde_json::to_value(&request).unwrap();
    let schema = validator(include_str!("../schemas/transport-request-v1.schema.json"));
    schema.validate(&request_json).unwrap();
    request_json["workload"]["typo"] = json!(true);
    assert!(!schema.is_valid(&request_json));
    assert!(serde_json::from_value::<Request>(request_json).is_err());
    let event = Event::new(
        &request,
        Implementation {
            name: "reference".into(),
            version: "1".into(),
            settings: Default::default(),
        },
        EventData::Completed {
            measurement: Measurement {
                workload: request.workload.clone(),
                path: "direct".into(),
                path_evidence: Value::Null,
                first_data_seconds: 0.1,
                message_rtt_seconds: vec![0.01; 2],
                bulk_verified_bytes: 1024,
                bulk_seconds: 0.1,
            },
        },
    );
    let mut output = serde_json::to_value(event).unwrap();
    output["future"] = json!({"metadata":true});
    output["measurement"]["workload"]["future"] = json!(true);
    validator(include_str!("../schemas/transport-event-v1.schema.json"))
        .validate(&output)
        .unwrap();
    serde_json::from_value::<Event>(output)
        .unwrap()
        .validate(&request)
        .unwrap();
}

#[test]
fn versioned_network_inputs_are_closed_and_comparison_one_stays_unchanged() {
    let comparison: Value =
        serde_json::from_str(include_str!("../examples/transports/conditions.json")).unwrap();
    let input = validator(include_str!(
        "../schemas/transport-comparison-v2.schema.json"
    ));
    input.validate(&comparison).unwrap();
    assert!(!validator(include_str!(
        "../schemas/transport-comparison-v1.schema.json"
    ))
    .is_valid(&comparison));
    let mut old = comparison.clone();
    old["schema_version"] = 1.into();
    assert!(!validator(include_str!(
        "../schemas/transport-comparison-v1.schema.json"
    ))
    .is_valid(&old));
    let mut typo = comparison.clone();
    typo["cases"][0]["network"]["client_to_server"]["jitter_ms"] = 1.into();
    assert!(!input.is_valid(&typo));
    let mut missing = comparison;
    missing["cases"][0]
        .as_object_mut()
        .unwrap()
        .remove("network");
    assert!(!input.is_valid(&missing));
    let application = json!({"schema_version":4,"cases":[{"name":"network","a":"preserve","b":"preserve","router_input":"drop","network":{"links":[{"egress":"router_a_wan","conditions":{"delay_ms":10,"loss_percent":1}}]},"processes":[{"name":"app","role":"wan","argv":["true"],"timeout_seconds":1}],"steps":[{"action":"run","process":"app"}]}]});
    validator(include_str!("../schemas/scenario-v4.schema.json"))
        .validate(&application)
        .unwrap();
    assert!(!validator(include_str!("../schemas/scenario-v3.schema.json")).is_valid(&application));
}
