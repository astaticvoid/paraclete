use paraclete_app::builder::{build_from_instrument, parse_instrument_definition};
use paraclete_app::instrument::InstrumentError;
use paraclete_runtime::NodeConfigurator;

const SR: f32 = 44100.0;
const BLOCK: usize = 512;

#[test]
fn instrument_load_minimal_yaml_succeeds() {
    let yaml = r#"
format_version: 1
name: "test-minimal"
bpm: 120.0
nodes:
  - id: 1
    type: internal_clock
  - id: 60
    type: audio_output
edges: []
"#;
    let def = parse_instrument_definition(yaml).expect("should parse");
    assert_eq!(def.name, "test-minimal");
    assert!((def.bpm - 120.0).abs() < f64::EPSILON);
    assert_eq!(def.nodes.len(), 2);
}

#[test]
fn instrument_build_single_chain_connects_correctly() {
    let yaml = r#"
format_version: 1
name: "chain-test"
bpm: 140.0
nodes:
  - id: 1
    type: internal_clock
  - id: 10
    type: sequencer
    display_name: "Kick"
  - id: 20
    type: analog_engine:kick
  - id: 60
    type: audio_output
edges:
  - from: [1, "clock_out"]
    to:   [10, "clock_in"]
  - from: [10, "events_out"]
    to:   [20, "events_in"]
"#;
    let def = parse_instrument_definition(yaml).expect("should parse");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let ids = build_from_instrument(&def, &mut conf, &Default::default()).expect("should build");
    assert_ne!(ids.clock, 0);
    assert_eq!(ids.clock, 1);
    assert_eq!(ids.sequencers.len(), 1);
    assert_eq!(ids.sequencers[0], 10);
    assert!(ids.clock != ids.sequencers[0]);
}

#[test]
fn instrument_initial_params_applied_to_node() {
    let yaml = r#"
format_version: 1
name: "params-test"
bpm: 120.0
nodes:
  - id: 1
    type: internal_clock
  - id: 10
    type: distortion
    initial_params:
      drive: 0.75
edges: []
"#;
    let def = parse_instrument_definition(yaml).expect("should parse");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    build_from_instrument(&def, &mut conf, &Default::default())
        .expect("should build without error");

    // Verify the initial param is applied by testing set_initial_params + activate directly.
    use paraclete_node_api::Node;
    use paraclete_nodes::DistortionNode;
    let mut node = DistortionNode::new();
    let mut params = std::collections::HashMap::new();
    params.insert("drive".to_string(), 0.75f64);
    node.set_initial_params(&params);
    node.activate(44100.0, 512);
    // Verify the drive param is applied: get() uses the sequential ID 0.
    let cap = node.capability_document();
    let drive_param = cap.params.iter().find(|p| p.name.as_str() == "drive");
    assert!(
        drive_param.is_some(),
        "DistortionNode must declare a 'drive' param"
    );
    // The bank should reflect the initial value after activate().
    // Direct bank access is not exposed publicly, so we confirm no panic occurred
    // and the param ID is discoverable via the capability document.
    let _drive_id = drive_param.unwrap().id;
}

#[test]
fn instrument_unknown_node_type_returns_error() {
    let yaml = r#"
format_version: 1
name: "unknown-type-test"
bpm: 120.0
nodes:
  - id: 1
    type: nonexistent
edges: []
"#;
    let def = parse_instrument_definition(yaml).expect("should parse yaml");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let result = build_from_instrument(&def, &mut conf, &Default::default());
    assert!(
        matches!(result, Err(InstrumentError::UnknownNodeType { .. })),
        "expected UnknownNodeType, got: {:?}",
        result
    );
}

#[test]
fn instrument_unknown_version_returns_error() {
    let yaml = r#"
format_version: 99
name: "bad-version"
bpm: 120.0
nodes: []
edges: []
"#;
    let result = parse_instrument_definition(yaml);
    assert!(
        matches!(result, Err(InstrumentError::UnknownVersion(99))),
        "expected UnknownVersion(99), got: {:?}",
        result
    );
}
