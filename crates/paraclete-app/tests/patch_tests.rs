// Tests for NodeRegistry, apply_patch, and project v2 format (P9 Commit 4).

use paraclete_app::{
    apply_patch, build_registry, PatchError, TopologyChange,
};
use paraclete_app::project::{save_project_v2, load_project_v2};
use paraclete_hal::{AudioEngine, LaunchpadEmulator};
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

/// A RemoveNode on a surface device must surface the real refusal reason
/// (ConfigError with the device message), not mask it as NodeNotFound — the
/// round-2 audit tracker note (error mapping discarded the message).
#[test]
fn apply_patch_remove_device_reports_refusal_reason() {
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let engine   = AudioEngine::new_paused();
    let registry = build_registry();
    conf.add_surface(55, Box::new(LaunchpadEmulator::new()));

    let result = apply_patch(
        vec![TopologyChange::RemoveNode { id: 55 }],
        &engine,
        &mut conf,
        &registry,
    );

    match result {
        Err(PatchError::ConfigError(msg)) => assert!(
            msg.contains("surface device"),
            "expected the device-refusal reason, got: {msg}"
        ),
        other => panic!("expected ConfigError with device reason, got: {other:?}"),
    }
    // The device must survive the refused removal.
    assert!(conf.contains_node(55), "device must remain registered after refusal");
}

/// A RemoveNode on a genuinely absent id is still the typed NodeNotFound.
#[test]
fn apply_patch_remove_unknown_id_is_node_not_found() {
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    let engine   = AudioEngine::new_paused();
    let registry = build_registry();

    let result = apply_patch(
        vec![TopologyChange::RemoveNode { id: 9999 }],
        &engine,
        &mut conf,
        &registry,
    );

    assert!(
        matches!(result, Err(PatchError::NodeNotFound(9999))),
        "expected NodeNotFound(9999), got: {result:?}"
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

// ── Load test (opt-in) ──────────────────────────────────────────────────────

/// LOAD TEST — `#[ignore]`, needs a real audio output device.
///
/// Runs the default instrument through the **real cpal callback** and hammers
/// `apply_patch` (topology swaps) for ~15 s, then reads the ADR-034
/// `RuntimeCounters` directly off the executor. This is the only way to
/// exercise the two audio-callback dropout counters
/// (`dropout_lock_miss` / `dropout_no_executor`) — the headless test-driver
/// drives `ex.process()` directly and never touches the callback path, so it
/// leaves those two trivially 0 (see `engine_counters_quiet.yaml`).
///
/// The pause-rebuild-resume protocol (ADR-029) sets `pause()` before it touches
/// the executor lock, and the callback's pause path early-returns without
/// counting — so a correct implementation must show **zero** self-inflicted
/// dropouts even under heavy churn. A non-zero count here is a real race, not
/// noise: that is the tripwire this test arms.
///
/// Run:
///   cargo test -p paraclete-app --test patch_tests \
///       -- --ignored --nocapture loadtest_topology_churn_under_live_audio
#[test]
#[ignore]
fn loadtest_topology_churn_under_live_audio() {
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use paraclete_app::builder::{build_from_instrument, load_instrument_definition};
    use paraclete_node_api::NodeCommand;
    use paraclete_nodes::Sequencer;

    // 1. Build the default instrument graph (analog + FM voices; no samples).
    let instrument =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../instrument.yaml");
    let def = load_instrument_definition(&instrument).expect("load instrument.yaml");
    // NOTE: audio.rs starts the cpal stream with BufferSize::Default — the
    // device's native buffer size, NOT the executor's block_size — so this
    // MUST match the device or the executor's `debug_assert_eq!(out.len(),
    // block_size * channels)` fires on the audio thread. 512 matches the app's
    // hardcoded BLOCK_SIZE and this dev machine. That coupling *is* BUG-002 /
    // BUG-012 (rate + buffer assumed, not negotiated); this test surfaced it.
    const DEVICE_BLOCK: usize = 512;
    let mut conf = NodeConfigurator::new(SR, DEVICE_BLOCK);
    let ids = build_from_instrument(&def, &mut conf, &HashMap::new()).expect("build graph");

    // 2. Build the executor and start REAL audio. Headless CI without an output
    //    device skips cleanly — this is an opt-in hardware-ish measurement.
    let executor = conf.build_executor();
    let engine = match AudioEngine::start(executor) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[loadtest] no audio device ({e:?}) — skipping");
            return;
        }
    };

    // 3. Put real load on the audio thread: four-on-the-floor on every track,
    //    then let the audio thread drain the commands before churn begins.
    for &seq in &ids.sequencers {
        for step in [0i64, 4, 8, 12] {
            let _ = conf.send_command(NodeCommand {
                target_id: seq,
                type_id: Sequencer::CMD_TOGGLE_STEP,
                arg0: step,
                arg1: 0.0,
            });
        }
    }
    std::thread::sleep(Duration::from_millis(300));

    // 4. Hammer topology swaps for ~15 s. Each iteration removes the node added
    //    last round and adds a fresh (disconnected, harmless) one, driving the
    //    full pause → take → rebuild → resume cycle against the live callback.
    let registry = build_registry();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut swaps = 0u64;
    let mut patch_failures = 0u64;
    let mut last_added: Option<u32> = None;
    while Instant::now() < deadline {
        let mut changes = Vec::new();
        if let Some(id) = last_added.take() {
            changes.push(TopologyChange::RemoveNode { id });
        }
        changes.push(TopologyChange::AddNode {
            type_tag: "distortion".to_string(),
            initial_params: Default::default(),
        });
        match apply_patch(changes, &engine, &mut conf, &registry) {
            Ok(new_ids) => {
                last_added = new_ids.last().copied();
                swaps += 1;
            }
            Err(e) => {
                eprintln!("[loadtest] patch failed: {e:?}");
                patch_failures += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }

    // 5. Quiesce and read the counters straight off the executor (the Arc is
    //    shared with the audio callback, so these are the live totals).
    engine.pause();
    engine.wait_paused();
    let executor = engine.take_executor().expect("executor present after run");
    let c = executor.counters();
    let buffers = c.buffers_processed.load(Ordering::Relaxed);
    let lock_miss = c.dropout_lock_miss.load(Ordering::Relaxed);
    let no_exec = c.dropout_no_executor.load(Ordering::Relaxed);
    let overflows = c.state_bus_overflows.load(Ordering::Relaxed);

    eprintln!("[loadtest] --- results over ~15 s of live audio ---");
    eprintln!("[loadtest]   topology swaps applied : {swaps}");
    eprintln!("[loadtest]   patch failures         : {patch_failures}");
    eprintln!("[loadtest]   buffers_processed      : {buffers}");
    eprintln!("[loadtest]   dropout_lock_miss      : {lock_miss}");
    eprintln!("[loadtest]   dropout_no_executor    : {no_exec}");
    eprintln!("[loadtest]   state_bus_overflows    : {overflows}");

    // Liveness: the real callback ran and topology actually churned.
    assert!(buffers > 0, "audio callback never processed a buffer");
    assert!(swaps > 0, "no topology swaps applied");
    // Tripwire: zero self-inflicted dropouts even under heavy churn.
    assert_eq!(lock_miss, 0, "audio callback missed the executor lock {lock_miss}x");
    assert_eq!(no_exec, 0, "audio callback found no executor {no_exec}x");
    assert_eq!(overflows, 0, "state bus overflowed {overflows}x");
}
