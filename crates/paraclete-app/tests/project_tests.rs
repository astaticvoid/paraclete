use paraclete_app::project::{
    load_project, save_project, ProfileBinding, Project, ProjectError, ProjectMetadata,
};
use paraclete_node_api::Node;
use paraclete_nodes::{InternalClock, Sequencer};
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
