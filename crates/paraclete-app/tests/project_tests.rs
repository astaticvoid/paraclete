use paraclete_app::project::{
    load_project, save_project, ProfileBinding, Project, ProjectError, ProjectMetadata,
};
use paraclete_node_api::{Node, ParamDescriptor};
use paraclete_nodes::{
    AnalogEngine, DistortionNode, FilterNode, FmEngine, InternalClock, ReverbNode, Sequencer,
};
use paraclete_runtime::NodeConfigurator;

const SR: f32 = 44100.0;
const BLOCK: usize = 512;

fn make_metadata() -> ProjectMetadata {
    ProjectMetadata {
        name: "test".to_string(),
        bpm: 120.0,
        created: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn empty_profiles() -> ProfileBinding {
    ProfileBinding { active: vec![] }
}

#[test]
fn project_save_creates_valid_ron_file() {
    let tmp = std::env::temp_dir().join("paraclete_save_test.ron");
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));

    save_project(&tmp, &conf, make_metadata(), empty_profiles()).expect("save should succeed");

    assert!(tmp.exists(), "ron file should have been created");

    let content = std::fs::read_to_string(&tmp).unwrap();
    let project: Project = ron::de::from_str(&content).expect("should parse back to Project");
    assert_eq!(project.version, 1);
    assert_eq!(project.graph.nodes.len(), 1);
    assert_eq!(project.graph.nodes[0].id, 1);

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn project_save_then_load_restores_state() {
    let tmp = std::env::temp_dir().join("paraclete_roundtrip_test.ron");

    // Build + configure a sequencer with a non-default step.
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));
    let mut seq = Sequencer::new();
    seq.set_step(3, 72, 32768, true);
    conf.add_node(2, Box::new(seq));

    save_project(&tmp, &conf, make_metadata(), empty_profiles()).expect("save should succeed");

    // Fresh configurator — load into it.
    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    conf2.add_node(1, Box::new(InternalClock::new()));
    conf2.add_node(2, Box::new(Sequencer::new()));

    let warnings = load_project(&tmp, &mut conf2).expect("load should succeed");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    // Verify: serialise the loaded node and compare byte-for-byte against a
    // reference sequencer with exactly step 3 set — this catches any offset or
    // field mapping bug in the roundtrip.
    let node = conf2.node_mut(2).expect("node 2 should exist");
    let restored_bytes = node.serialize();

    let mut reference = Sequencer::new();
    reference.set_step(3, 72, 32768, true);
    let reference_bytes = reference.serialize();

    assert_eq!(
        restored_bytes, reference_bytes,
        "loaded sequencer state should match byte-for-byte with the saved pattern"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// #154: the five param-bearing nodes that inherited the trait defaults —
/// `serialize() -> vec![]` and a no-op `deserialize()` — so patterns survived
/// a save and **sound did not**. Every tweak to a kick's decay, a filter's
/// cutoff or the reverb's room size was lost.
///
/// Drives the real `save_project` / `load_project` pair rather than the nodes'
/// own round trip, because the ordering this depends on lives in
/// `project.rs:281-287`: `add_node` activates (resetting the bank to
/// defaults), and only then may `deserialize` run.
#[test]
fn project_save_then_load_restores_engine_and_effect_params() {
    let tmp = std::env::temp_dir().join("paraclete_param_roundtrip_test.ron");
    let pid = |name: &str| ParamDescriptor::id_for_name(name);

    // (node id, param name, value to store) — one per previously-lossy node.
    // `cutoff_hz`, not the canonical `cutoff`: FilterNode declares it that way
    // (`filter.rs:109`), which is its own deviation from AGENTS.md's canonical
    // name list and is not this test's to change.
    let edits: [(u32, &str, f64); 5] = [
        (20, "decay", 0.42),
        (27, "tune", 7.0),
        (40, "cutoff_hz", 3210.0),
        (30, "drive", 0.77),
        (200, "wet", 0.31),
    ];

    // `initial` is applied inside `activate()`, which `add_node` runs — the
    // one route into a node's bank that does not need the audio thread.
    let build = |initial: bool| {
        let pick = |name: &str| -> std::collections::HashMap<String, f64> {
            let mut m = std::collections::HashMap::new();
            if initial {
                if let Some((_, n, v)) = edits.iter().find(|(_, n, _)| *n == name) {
                    m.insert(n.to_string(), *v);
                }
            }
            m
        };
        let mut conf = NodeConfigurator::new(SR, BLOCK);
        conf.add_node(1, Box::new(InternalClock::new()));
        let mut e20 = AnalogEngine::kick();
        e20.set_initial_params(&pick("decay"));
        conf.add_node(20, Box::new(e20));
        let mut e27 = FmEngine::bass();
        e27.set_initial_params(&pick("tune"));
        conf.add_node(27, Box::new(e27));
        let mut e40 = FilterNode::new();
        e40.set_initial_params(&pick("cutoff_hz"));
        conf.add_node(40, Box::new(e40));
        let mut e30 = DistortionNode::new();
        e30.set_initial_params(&pick("drive"));
        conf.add_node(30, Box::new(e30));
        let mut e200 = ReverbNode::new();
        e200.set_initial_params(&pick("wet"));
        conf.add_node(200, Box::new(e200));
        conf
    };

    let mut conf = build(true);
    let mut fresh = build(false);
    for (id, name, value) in edits {
        assert_eq!(
            read_param(conf.node_mut(id).expect("node exists"), pid(name)),
            value,
            "node {id}: fixture could not store {name}={value} — is it in range?"
        );
        assert_ne!(
            read_param(fresh.node_mut(id).expect("node exists"), pid(name)),
            value,
            "node {id}: {name}'s probe value equals its default, so the test \
             would pass on a node that persists nothing"
        );
    }

    save_project(&tmp, &conf, make_metadata(), empty_profiles()).expect("save should succeed");

    // Loaded into nodes built at their defaults — so anything correct here
    // came out of the file, not out of the fixture.
    let mut conf2 = build(false);
    let warnings = load_project(&tmp, &mut conf2).expect("load should succeed");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    for (id, name, value) in edits {
        let node = conf2.node_mut(id).expect("node exists");
        assert_eq!(
            read_param(node, pid(name)),
            value,
            "node {id}: {name} did not survive save/load"
        );
    }

    let _ = std::fs::remove_file(&tmp);
}

/// Read a live param off a node via the state bus it publishes, so the test
/// asserts on what the node actually reports rather than reaching into its
/// bank.
fn read_param(node: &dyn Node, param_id: u32) -> f64 {
    let mut buf = Vec::new();
    node.published_state(&mut buf);
    const SUFFIX: &str = "/param/";
    for (path, value) in buf {
        if let Some(idx) = path.find(SUFFIX) {
            let name = &path[idx + SUFFIX.len()..];
            if ParamDescriptor::id_for_name(name) == param_id {
                return match value {
                    paraclete_node_api::StateBusValue::Float(f) => f,
                    paraclete_node_api::StateBusValue::Int(i) => i as f64,
                    other => panic!("unexpected value for {path}: {other:?}"),
                };
            }
        }
    }
    panic!("no published param with id {param_id}");
}

#[test]
fn project_load_unknown_node_id_skips_with_warning() {
    let tmp = std::env::temp_dir().join("paraclete_unknown_id_test.ron");

    // Save with node 1 only.
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));
    save_project(&tmp, &conf, make_metadata(), empty_profiles()).unwrap();

    // Manually edit the RON to inject an unknown node id=999.
    let content = std::fs::read_to_string(&tmp).unwrap();
    let injected = content.replace("id: 1,", "id: 999,");
    std::fs::write(&tmp, &injected).unwrap();

    // Load into a configurator that has no node 999 — should warn, not panic.
    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    conf2.add_node(1, Box::new(InternalClock::new()));
    let warnings = load_project(&tmp, &mut conf2).expect("should be Ok even with unknown id");
    assert!(
        !warnings.is_empty(),
        "should have a warning for unknown id 999"
    );

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn project_load_unknown_version_returns_error() {
    let tmp = std::env::temp_dir().join("paraclete_bad_version_test.ron");

    // Write a minimal project with version 99.
    let bad_ron = r#"(
    version: 99,
    metadata: (
        name: "bad",
        bpm: 120.0,
        created: "",
    ),
    graph: (
        nodes: [],
        edges: [],
    ),
    profiles: (
        active: [],
    ),
)"#;
    std::fs::write(&tmp, bad_ron).unwrap();

    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let result = load_project(&tmp, &mut conf);
    assert!(
        matches!(result, Err(ProjectError::UnknownVersion(99))),
        "expected UnknownVersion(99), got {result:?}",
    );

    let _ = std::fs::remove_file(&tmp);
}
