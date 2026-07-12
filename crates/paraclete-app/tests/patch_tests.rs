// Tests for NodeRegistry, apply_patch, and project v2 format (P9 Commit 4).

use paraclete_app::{
    apply_patch, build_registry, PatchError, TopologyChange,
};
use paraclete_app::project::{save_project_v2, load_project_v2};
use paraclete_hal::AudioEngine;
use paraclete_nodes::{DistortionNode, FilterNode};
use paraclete_runtime::NodeConfigurator;

const SR: f32     = 44100.0;
const BLOCK: usize = 256;

// ── NodeRegistry tests ─────────────────────────────────────────────────────

#[test]
fn node_registry_build_known_type_tag() {
    let registry = build_registry();
    let node = registry.build("filter");
    assert!(node.is_some());
    // FilterNode doesn't override type_name so it returns fully-qualified path.
    // Just verify it builds without panic.
    let _ = node.unwrap();
}

#[test]
fn node_registry_build_unknown_type_tag() {
    let registry = build_registry();
    assert!(registry.build("nonexistent_node_xyz").is_none());
}

#[test]
fn node_registry_known_type_tags_contains_loop_break() {
    let registry = build_registry();
    let tags = registry.known_type_tags();
    assert!(
        tags.contains(&"loop_break"),
        "expected loop_break in {:?}",
        tags
    );
}

#[test]
fn registry_contains_inner_graph() {
    let registry = build_registry();
    let node = registry.build("inner_graph");
    assert!(node.is_some(), "expected inner_graph in registry");
    let node = node.unwrap();
    assert_eq!(node.type_name(), "InnerGraphNode");
}

#[test]
fn registry_known_type_tags_contains_inner_graph() {
    let registry = build_registry();
    let tags = registry.known_type_tags();
    assert!(
        tags.contains(&"inner_graph"),
        "expected inner_graph in {:?}",
        tags
    );
}

// ── apply_patch tests ──────────────────────────────────────────────────────

#[test]
fn apply_patch_add_node_returns_id() {
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let engine   = AudioEngine::new_paused();
    let registry = build_registry();

    let result = apply_patch(
        vec![TopologyChange::AddNode {
            type_tag:       "distortion".to_string(),
            initial_params: Default::default(),
        }],
        &engine,
        &mut conf,
        &registry,
    );

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    let ids = result.unwrap();
    assert_eq!(ids.len(), 1);
    assert!(ids[0] > 0);
}

#[test]
fn apply_patch_unknown_type_tag_returns_error() {
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let engine   = AudioEngine::new_paused();
    let registry = build_registry();

    let result = apply_patch(
        vec![TopologyChange::AddNode {
            type_tag:       "nonexistent_xyz".to_string(),
            initial_params: Default::default(),
        }],
        &engine,
        &mut conf,
        &registry,
    );

    assert!(
        matches!(result, Err(PatchError::UnknownTypeTag(_))),
        "expected UnknownTypeTag, got: {:?}",
        result
    );
}

#[test]
fn apply_patch_add_edge_cycle_without_loop_break_returns_error() {
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let engine   = AudioEngine::new_paused();
    let registry = build_registry();

    // Add two nodes with matching ports via the registry.
    // We need two nodes with at least one port each to attempt an edge.
    // Use `distortion` (has audio_in port 0 and audio_out port 1) and
    // `filter` (same) — so we can connect distortion→filter and then
    // attempt filter→distortion to form a cycle.
    let dist_id = conf.add_node_tagged(Box::new(DistortionNode::new()), "distortion");
    let filt_id = conf.add_node_tagged(Box::new(FilterNode::new()), "filter");

    // Connect distortion audio_out (port 1) → filter audio_in (port 0).
    conf.connect(dist_id, 1, filt_id, 0).ok();

    // Try to close a cycle: filter audio_out (port 1) → distortion audio_in (port 0).
    let result = apply_patch(
        vec![TopologyChange::AddEdge {
            src:      filt_id,
            src_port: 1,
            dst:      dist_id,
            dst_port: 0,
        }],
        &engine,
        &mut conf,
        &registry,
    );

    assert!(
        matches!(result, Err(PatchError::CycleError(_))),
        "expected CycleError, got: {:?}",
        result
    );
}

/// BUG-029 — a failed change mid-batch must not strand the engine paused
/// with no executor: apply_patch rebuilds and resumes with the partial
/// changes applied, then reports the error.
#[test]
fn apply_patch_failed_change_still_installs_executor() {
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let engine   = AudioEngine::new_paused();
    let registry = build_registry();

    let result = apply_patch(
        vec![
            TopologyChange::AddNode {
                type_tag:       "distortion".to_string(),
                initial_params: Default::default(),
            },
            TopologyChange::AddNode {
                type_tag:       "nonexistent_xyz".to_string(),
                initial_params: Default::default(),
            },
        ],
        &engine,
        &mut conf,
        &registry,
    );

    assert!(
        matches!(result, Err(PatchError::UnknownTypeTag(_))),
        "expected UnknownTypeTag, got: {:?}",
        result
    );
    assert!(
        engine.take_executor().is_some(),
        "failed patch must leave the engine with a fresh executor, not stranded paused"
    );
}

// ── NodeConfigurator::remove_node tests ───────────────────────────────────

#[test]
fn configurator_remove_node_severs_edges() {
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let a_id = conf.add_node_tagged(Box::new(FilterNode::new()), "filter");
    let b_id = conf.add_node_tagged(Box::new(DistortionNode::new()), "distortion");

    // Connect filter audio_out (port 1) → distortion audio_in (port 0).
    let _ = conf.connect(a_id, 1, b_id, 0);

    // Remove the filter — should sever the edge.
    let result = conf.remove_node(a_id);
    assert!(result.is_ok(), "remove_node should succeed");

    // Verify no edges reference a_id.
    let edges: Vec<_> = conf.all_edges().collect();
    assert!(
        edges.iter().all(|e| e.src_node != a_id && e.dst_node != a_id),
        "edges referencing removed node still present: {:?}",
        edges
    );
}

// ── Project v2 roundtrip tests ─────────────────────────────────────────────

#[test]
fn project_v2_save_load_roundtrip() {
    use paraclete_nodes::{AudioOutputNode, InternalClock, Sequencer};

    let dir  = std::env::temp_dir();
    let path = dir.join("paraclete_v2_roundtrip_test.ron");

    // Build a 3-node graph.
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let clock_id = conf.add_node_tagged(Box::new(InternalClock::new()), "internal_clock");
    let seq_id   = conf.add_node_tagged(Box::new(Sequencer::new()),     "sequencer");
    let out_id   = conf.add_node_tagged(Box::new(AudioOutputNode::new()), "audio_output");

    // Save v2.
    let save_result = save_project_v2(&conf, &path);
    assert!(save_result.is_ok(), "save_project_v2 failed: {:?}", save_result);

    // Load into fresh conf.
    let registry  = build_registry();
    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    let load_result = load_project_v2(&path, &mut conf2, &registry);
    assert!(load_result.is_ok(), "load_project_v2 failed: {:?}", load_result);

    let warnings = load_result.unwrap();
    // Only expect warnings about nodes with empty type_tag (none here).
    let bad_warnings: Vec<_> = warnings.iter()
        .filter(|w| !w.contains("upgrade") && !w.contains("edge"))
        .collect();
    assert!(bad_warnings.is_empty(), "unexpected warnings: {:?}", bad_warnings);

    // Check that nodes were loaded with the same IDs.
    let ids: Vec<u32> = conf2.all_nodes().map(|(id, _)| id).collect();
    assert!(ids.contains(&clock_id), "clock_id {clock_id} missing from loaded conf");
    assert!(ids.contains(&seq_id),   "seq_id {seq_id} missing from loaded conf");
    assert!(ids.contains(&out_id),   "out_id {out_id} missing from loaded conf");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn project_v1_load_emits_warning() {
    // Create a minimal v1 project file (RON format, no type_tag).
    let v1_ron = r#"(
    version: 1,
    metadata: (
        name: "test_v1",
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

    let dir  = std::env::temp_dir();
    let path = dir.join("paraclete_v1_warning_test.ron");
    std::fs::write(&path, v1_ron).unwrap();

    let registry  = build_registry();
    let mut conf  = NodeConfigurator::new(SR, BLOCK);
    let result    = load_project_v2(&path, &mut conf, &registry);

    assert!(result.is_ok(), "v1 load should succeed: {:?}", result);
    let warnings = result.unwrap();
    // Should contain a v1-upgrade warning.
    assert!(
        warnings.iter().any(|w| w.contains("version 1")),
        "expected v1 upgrade warning, got: {:?}",
        warnings
    );

    let _ = std::fs::remove_file(&path);
}
