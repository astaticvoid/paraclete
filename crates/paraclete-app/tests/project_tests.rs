use paraclete_app::perform_state::PerformState;
use paraclete_app::project::{
    load_project, load_project_v3, save_project, save_project_v3, ProfileBinding, Project,
    ProjectError, ProjectMetadata,
};
use paraclete_app::sample_pool::{PoolEntry, SampleSource};
use paraclete_node_api::{Node, ParamDescriptor};
use paraclete_nodes::{
    AnalogEngine, DistortionNode, FilterNode, FmEngine, InternalClock, ReverbNode, Sampler,
    Sequencer,
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
    // Uses FilterNode's canonical `cutoff` (renamed from cutoff_hz in
    // BUG-061, e8ba0ce — FilterNode now declares the canonical name).
    let edits: [(u32, &str, f64); 5] = [
        (20, "decay", 0.42),
        (27, "tune", 7.0),
        (40, "cutoff", 3210.0),
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
        e40.set_initial_params(&pick("cutoff"));
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

/// AN0 C0: a v3 project file saved before the sample pool existed (no
/// `sample_pool` key in the RON) must still load — `#[serde(default)]`
/// gives an empty pool, same tolerance `kits`/`kit_binding` already have.
/// Generates a real v3 file via `save_project_v3`, then strips the
/// `sample_pool` field (RON's `PrettyConfig` puts it on its own line) to
/// simulate a file saved before AN0 — same technique as
/// `project_load_unknown_node_id_skips_with_warning`'s string injection.
#[test]
fn project_load_v3_without_sample_pool_key_defaults_to_empty_pool() {
    let tmp = std::env::temp_dir().join("paraclete_v3_no_pool_test.ron");

    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));
    save_project_v3(&tmp, &conf, make_metadata(), empty_profiles(), &[], [None; 8], &[])
        .expect("save should succeed");

    let content = std::fs::read_to_string(&tmp).unwrap();
    let stripped: String = content
        .lines()
        .filter(|l| !l.trim_start().starts_with("sample_pool"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(stripped, content, "fixture must actually contain a sample_pool line to strip");
    std::fs::write(&tmp, &stripped).unwrap();

    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    conf2.add_node(1, Box::new(InternalClock::new()));
    let mut perform = PerformState::new();
    let warnings = load_project_v3(&tmp, &mut conf2, &mut perform).expect("load should succeed");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert!(
        perform.sample_pool.slots.iter().all(|s| s.is_none()),
        "pre-AN0 file has no sample_pool key; pool must default to empty"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// AN0 C0: a populated sample pool survives a `save_project_v3` /
/// `load_project_v3` round trip through the real app-facing entry points.
#[test]
fn project_v3_save_then_load_restores_sample_pool() {
    let tmp = std::env::temp_dir().join("paraclete_v3_pool_roundtrip_test.ron");
    // A real, resolvable path — #185's rehydration pass (load_project_v3)
    // now attempts to decode every File-backed entry on load, so a
    // placeholder path would surface as an unexpected warning here.
    let kick = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples/kick.wav")
        .to_string_lossy()
        .into_owned();

    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));

    let mut pool: Vec<Option<PoolEntry>> = vec![None; 4];
    pool[1] = Some(PoolEntry {
        name: "kick".into(),
        source: SampleSource::File(kick.clone()),
        frames: 1000,
        rate: 44100.0,
        bars: None,
        gain: 1.0,
        slice_table: vec![(0, 1000)],
    });

    save_project_v3(
        &tmp,
        &conf,
        make_metadata(),
        empty_profiles(),
        &[],
        [None; 8],
        &pool,
    )
    .expect("save should succeed");

    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    conf2.add_node(1, Box::new(InternalClock::new()));
    let mut perform = PerformState::new();
    let warnings = load_project_v3(&tmp, &mut conf2, &mut perform).expect("load should succeed");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    let entry = perform.sample_pool.get(1).expect("slot 1 should be restored");
    assert_eq!(entry.name, "kick");
    assert!(matches!(&entry.source, SampleSource::File(p) if *p == kick));
    assert_eq!(entry.frames, 1000);
    assert!(perform.sample_pool.get(0).is_none());

    let _ = std::fs::remove_file(&tmp);
}

/// A pool grown past `NOMINAL_POOL_SIZE` (128 — `SamplePool::load_file`
/// deliberately allows this, D2) must not be truncated back down on load.
/// Regression for a resize-clobbers-growth bug caught in C0 review.
#[test]
fn project_v3_round_trip_preserves_a_pool_grown_past_nominal_size() {
    let tmp = std::env::temp_dir().join("paraclete_v3_pool_overflow_test.ron");
    let track0 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples/track0.wav")
        .to_string_lossy()
        .into_owned();

    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));

    let mut pool: Vec<Option<PoolEntry>> = vec![None; 130];
    pool[129] = Some(PoolEntry {
        name: "overflow".into(),
        source: SampleSource::File(track0),
        frames: 500,
        rate: 44100.0,
        bars: None,
        gain: 1.0,
        slice_table: vec![(0, 500)],
    });

    save_project_v3(
        &tmp,
        &conf,
        make_metadata(),
        empty_profiles(),
        &[],
        [None; 8],
        &pool,
    )
    .expect("save should succeed");

    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    conf2.add_node(1, Box::new(InternalClock::new()));
    let mut perform = PerformState::new();
    let warnings = load_project_v3(&tmp, &mut conf2, &mut perform).expect("load should succeed");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    let entry = perform
        .sample_pool
        .get(129)
        .expect("slot 129 must survive the round trip, not be truncated to the 128-slot floor");
    assert_eq!(entry.name, "overflow");

    let _ = std::fs::remove_file(&tmp);
}

/// #185: `load_project_v3` must not just restore pool *metadata* — it must
/// re-decode and deliver the audio to every live Sampler node, the same
/// way `AppOp::PoolLoadFile` (C5) does at runtime. Proven the same way
/// C5's own delivery test is (`perform_state.rs`'s
/// `pool_load_file_delivers_to_every_live_sampler_node_via_the_asset_plane`):
/// through observable playback, not a field peek — `Sampler::pool_cache`
/// has no cross-crate test accessor.
#[test]
fn project_v3_load_rehydrates_a_samplers_pool_cache_from_a_file_backed_slot() {
    let tmp = std::env::temp_dir().join("paraclete_v3_pool_rehydrate_test.ron");
    let kick = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/kick.wav");

    // Save: a project whose sample_pool has a File-backed entry at slot 7
    // (no live Sampler needed for the save side — only the pool metadata
    // and the node's own base_sample matter).
    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));
    let mut pool: Vec<Option<PoolEntry>> = vec![None; 8];
    pool[7] = Some(PoolEntry {
        name: "kick".into(),
        source: SampleSource::File(kick.to_string_lossy().into_owned()),
        frames: 1,   // deliberately wrong — load must re-decode, not trust this
        rate: SR,
        bars: None,
        gain: 1.0,
        slice_table: vec![],
    });
    save_project_v3(&tmp, &conf, make_metadata(), empty_profiles(), &[], [None; 8], &pool)
        .expect("save should succeed");

    // Load: a fresh graph with a Sampler pointed at base_sample=7 (as a
    // real project would have saved it — set here directly since C2's
    // serialize/deserialize round-trips base_sample independently of this
    // test's node-snapshot list, which only carries the clock).
    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    conf2.add_node(1, Box::new(InternalClock::new()));
    let mut sampler = Sampler::new();
    sampler.set_base_sample(7);
    conf2.add_node(23, Box::new(sampler));

    let mut perform = PerformState::new();
    perform.sampler_ids = vec![23];
    let warnings = load_project_v3(&tmp, &mut conf2, &mut perform).expect("load should succeed");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    // Trigger and check for audio — pool_cache must have slot 7 populated,
    // not just perform.sample_pool's metadata.
    let mut exec = conf2.build_executor();
    let mut out = vec![0.0f32; BLOCK * 2];
    let _ = conf2.send_command(paraclete_node_api::NodeCommand {
        target_id: 23,
        type_id: paraclete_node_api::CMD_TRIGGER,
        arg0: -1,
        arg1: 0.0,
    });
    exec.process(&mut out, 2);
    assert!(
        out.iter().any(|&x| x != 0.0),
        "sampler should play slot 7's audio after project load rehydrates pool_cache"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// #185 review: the decode-failure branch of the rehydration pass had no
/// direct test — a `File`-sourced pool entry pointing at a missing path
/// must surface a warning (not silently vanish, not panic) and must not
/// stop the rest of load (the node-state restore still succeeds).
#[test]
fn project_v3_load_warns_on_a_pool_slot_that_fails_to_decode() {
    let tmp = std::env::temp_dir().join("paraclete_v3_pool_decode_failure_test.ron");

    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_node(1, Box::new(InternalClock::new()));
    let mut pool: Vec<Option<PoolEntry>> = vec![None; 4];
    pool[2] = Some(PoolEntry {
        name: "missing".into(),
        source: SampleSource::File("does/not/exist.wav".into()),
        frames: 1,
        rate: SR,
        bars: None,
        gain: 1.0,
        slice_table: vec![],
    });
    save_project_v3(&tmp, &conf, make_metadata(), empty_profiles(), &[], [None; 8], &pool)
        .expect("save should succeed");

    let mut conf2 = NodeConfigurator::new(SR, BLOCK);
    conf2.add_node(1, Box::new(InternalClock::new()));
    let mut perform = PerformState::new();
    let warnings = load_project_v3(&tmp, &mut conf2, &mut perform).expect("load should succeed");

    assert!(
        warnings.iter().any(|w| w.contains("pool slot 2") && w.contains("does/not/exist.wav")),
        "a decode failure must surface as a warning, got: {warnings:?}"
    );
    // Metadata still restores even though decode failed — only delivery
    // to a node's pool_cache is affected, not the pool's own record.
    assert!(perform.sample_pool.get(2).is_some());

    let _ = std::fs::remove_file(&tmp);
}
