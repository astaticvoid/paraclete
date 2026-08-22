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
        build_from_instrument(&def, &mut conf, &Default::default(), &mut Default::default()).expect("build should succeed");
    assert_ne!(ids.clock, 0, "clock id must be set");
    assert_eq!(ids.clock, 1);
}

#[test]
fn app_wiring_macro_publish_context_populates_state_bus() {
    use paraclete_node_api::capability::ParamDescriptor;
    use paraclete_node_api::StateBusValue;

    let def = parse_instrument_definition(MINIMAL_YAML).expect("parse");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    build_from_instrument(&def, &mut conf, &Default::default(), &mut Default::default()).expect("build");

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

/// P14-C8: `instrument-fm.yaml` is the fixture that gives `FmVoice` (node
/// 28) a graph to live in — proves the actual shipped file parses and
/// builds, not a copy pasted into the test.
#[test]
fn app_wiring_loads_the_fm_voice_fixture() {
    use paraclete_app::builder::load_instrument_definition;

    let instrument = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../instrument-fm.yaml");
    let def = load_instrument_definition(&instrument).expect("load instrument-fm.yaml");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let ids = build_from_instrument(&def, &mut conf, &Default::default(), &mut Default::default())
        .expect("build should succeed");
    assert!(ids.generators.contains(&28), "expected node 28 (FmVoice) classified as a generator");
}

/// TK1 C7 7.5 (#139/#138): `instrument-8track.yaml` realizes the AGENTS.md
/// 8-track graph (8 sequencers -> 8 voices, each behind its own
/// distortion+filter chain -> mix). Discovery must yield exactly 8 tracks,
/// paired sequencer-to-voice by the edge each sequencer drives (the same
/// edge-derived pairing `paraclete-app/src/main.rs` uses to build Theotokos's
/// `gen_ids`), matching the fixture's documented 20-27 voice-id order.
#[test]
fn eight_track_fixture_discovers_eight_tracks() {
    use paraclete_app::builder::load_instrument_definition;

    let instrument =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../instrument-8track.yaml");
    let def = load_instrument_definition(&instrument).expect("load instrument-8track.yaml");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let ids = build_from_instrument(&def, &mut conf, &Default::default(), &mut Default::default())
        .expect("build should succeed");

    assert_eq!(ids.sequencers.len(), 8, "expected 8 discovered sequencer tracks");
    assert_eq!(ids.sequencers, vec![10, 11, 12, 13, 14, 15, 16, 17]);
    assert_eq!(ids.generators.len(), 8, "expected 8 discovered voices");

    let edges: Vec<(u32, u32)> = conf.all_edges().map(|e| (e.src_node, e.dst_node)).collect();
    let gen_ids: Vec<u32> = ids
        .sequencers
        .iter()
        .map(|&seq_id| {
            edges
                .iter()
                .find(|(s, _)| *s == seq_id)
                .map(|(_, t)| *t)
                .unwrap_or(0)
        })
        .collect();
    assert_eq!(
        gen_ids,
        vec![20, 21, 22, 23, 24, 25, 26, 27],
        "expected edge-derived sequencer->voice pairing to match the fixture's documented order"
    );
}

#[test]
fn app_wiring_project_save_load_roundtrip() {
    let def = parse_instrument_definition(MINIMAL_YAML).expect("parse");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    build_from_instrument(&def, &mut conf, &Default::default(), &mut Default::default()).expect("build");

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
