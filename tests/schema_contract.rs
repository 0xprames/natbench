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
