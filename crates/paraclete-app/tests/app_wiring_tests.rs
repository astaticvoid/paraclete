// SPDX-License-Identifier: GPL-3.0-or-later
use paraclete_app::builder::{build_from_instrument, parse_instrument_definition};
use paraclete_app::project::{load_project, save_project, ProfileBinding, ProjectMetadata};
use paraclete_app::tui_enabled;
use paraclete_runtime::NodeConfigurator;
use paraclete_scripting::ScriptingEngine;

const SR: f32 = 44100.0;
const BLOCK: usize = 512;

const MINIMAL_YAML: &str = r#"
format_version: 1
name: "wiring-test"
bpm: 120.0
nodes:
  - id: 1
    type: internal_clock
  - id: 60
    type: audio_output
edges: []
"#;

#[test]
fn app_wiring_load_instrument_and_build_no_panic() {
    let def = parse_instrument_definition(MINIMAL_YAML).expect("should parse");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let ids =
        build_from_instrument(&def, &mut conf, &Default::default()).expect("build should succeed");
    assert_ne!(ids.clock, 0, "clock id must be set");
    assert_eq!(ids.clock, 1);
}

#[test]
fn app_wiring_macro_publish_context_populates_state_bus() {
    use paraclete_node_api::capability::ParamDescriptor;
    use paraclete_node_api::StateBusValue;

    let def = parse_instrument_definition(MINIMAL_YAML).expect("parse");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    build_from_instrument(&def, &mut conf, &Default::default()).expect("build");

    let bus_handle = conf.state_bus_handle();
    let mut scripting = ScriptingEngine::new();
    scripting.bind_state_bus(bus_handle.clone());

    // Simulate macro pre-population: encoder 0 → node 42, param "decay".
    let node_id: u32 = 42;
    let script = format!(r#"publish_context("encoder_0", {node_id}, "decay");"#,);
    scripting.eval_str(&script).expect("eval should succeed");

    // publish_context writes /context/encoder_0/node and /context/encoder_0/param.
    let bus = bus_handle.borrow();
    let node_val = bus.read("/context/encoder_0/node");
    let param_val = bus.read("/context/encoder_0/param");

    assert!(
        matches!(node_val, Some(StateBusValue::Float(v)) if (*v - node_id as f64).abs() < 1e-9),
        "expected node id {node_id} at /context/encoder_0/node, got {node_val:?}"
    );

    let expected_param_id = ParamDescriptor::id_for_name("decay") as f64;
    assert!(
        matches!(param_val, Some(StateBusValue::Float(v)) if (*v - expected_param_id).abs() < 1e-9),
        "expected param id {expected_param_id} at /context/encoder_0/param, got {param_val:?}"
    );
}

#[test]
fn app_wiring_no_tui_flag_skips_terminal_init() {
    assert!(!tui_enabled(true), "--no-tui should disable TUI");
    assert!(tui_enabled(false), "no --no-tui flag should enable TUI");
}

#[test]
fn app_wiring_project_save_load_roundtrip() {
    let def = parse_instrument_definition(MINIMAL_YAML).expect("parse");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    build_from_instrument(&def, &mut conf, &Default::default()).expect("build");

    let tmp = std::env::temp_dir().join("paraclete_p8_roundtrip_test.ron");
    let meta = ProjectMetadata {
        name: "roundtrip".to_string(),
        bpm: 120.0,
        created: String::new(),
    };
    save_project(&tmp, &conf, meta, ProfileBinding { active: vec![] })
        .expect("save should succeed");

    let warnings = load_project(&tmp, &mut conf).expect("load should succeed");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    let _ = std::fs::remove_file(&tmp);
}
