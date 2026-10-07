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
