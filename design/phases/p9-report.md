Paraclete — P9 Implementation Report
=====================================
Date: June 2026
Status: Complete — 389 tests, 0 failures (after Commit 6 + test follow-up)

WHAT SHIPPED (P9)
-----------------

Commit 1 — Encoder value publishing (GATED commit)
  (paraclete-node-api, paraclete-nodes)
  Tests after: 348 (+8 from baseline 340)

  ParameterSlot::name field (paraclete-node-api/src/param_bank.rs)

    ParameterSlot gains a `name: String` field. declare_param() stores the
    canonical ADR-019 name (e.g. "cutoff", "decay") at declaration time.
    No call-site changes needed — name was already passed to declare_param().

  ParameterBank::iter_values() (paraclete-node-api/src/param_bank.rs)

    New method iterating (name: &str, value: f64) pairs for all declared slots.
    Used by publish_bank_state() to populate the state bus.

  publish_bank_state() free function (paraclete-node-api/src/lib.rs)

    Pushes one StateBusValue::Float entry per ParameterBank slot into the
    caller-owned Vec. Path: "/node/{node_id}/{param_name}". Called from
    published_state() on all nodes that have a ParameterBank. Avoids
    per-cycle allocation (push-down signature pattern, OQ-9 fix).

  Node::set_node_id() (paraclete-node-api/src/node.rs)

    Default no-op. NodeConfigurator::add_node() calls it after ID assignment
    so nodes can store their ID for state bus path formatting.

  Node::published_state() overrides (paraclete-nodes, paraclete-clap-host)

    All nodes with a ParameterBank override published_state() to call
    publish_bank_state(). Affected nodes: DistortionNode, FilterNode,
    LadderFilterNode, OscillatorNode, EnvelopeNode, LfoNode, ReverbNode,
    DelayNode, AnalogEngine (all variants), FmEngine (all variants),
    Sampler, PluginNode.

  New tests (8): publish_bank_state_single_param, publish_bank_state_multi_param,
    publish_bank_state_empty_bank, distortion_node_published_state_nonzero,
    analog_engine_published_state_contains_decay, set_node_id_stored,
    parameter_slot_name_stored, iter_values_reflects_mutation.

---

Commit 2 — Housekeeping
  (paraclete-node-api, paraclete-runtime, paraclete-nodes, paraclete-clap-host,
   paraclete-app)
  Tests after: 357 (+9 from 348)

  cap_doc_cache in NodeConfigurator (paraclete-runtime/src/configurator.rs)

    NodeConfigurator gains cap_doc_cache: HashMap<u32, CapabilityDocument>.
    capability_document() is called exactly once per node (at add_node() time)
    and the result is stored. get_node_cap_doc(id) returns a reference to the
    cached value. Eliminates repeated Box::leak calls that had been observed
    per-call. This is a hard prerequisite for Commit 4 (apply_patch).

    Design note: cap_doc_cache is also needed by apply_patch() which calls
    rebuild_executor() repeatedly during the pause-rebuild-resume cycle.
    Without caching, each rebuild would call capability_document() again,
    creating multiple leaked boxes for the same node.

  serde_yaml → serde_yml (paraclete-app)

    serde_yaml 0.9 was deprecated upstream. Replaced with serde_yml across
    all call sites. The API is drop-in compatible; the instrument definition
    YAML format is unchanged.

  set_initial_params() coverage (paraclete-nodes)

    AnalogEngine, FmEngine, Sampler, ReverbNode now implement
    set_initial_params(). Each stores the params HashMap and applies it via
    bank.set(id, val) in activate() after bank setup. Unknown keys are
    silently ignored (no panic).

  Effect-type CLAP plugins (paraclete-clap-host)

    PluginNode now detects audio input buses at construction via the CLAP
    audio_ports extension. If has_audio_input == true: exposes PORT_AUDIO_IN
    (port 1) and PORT_AUDIO_OUT at port 2; routes input AudioBuffer through
    the CLAP audio_in channels each process() call. Generator path unchanged.

  New tests (9): cap_doc_cache_populated_on_add_node, cap_doc_cache_hit_no_additional_leak,
    analog_engine_set_initial_params_applied, fm_engine_set_initial_params_applied,
    sampler_set_initial_params_applied, reverb_node_set_initial_params_applied,
    set_initial_params_unknown_key_ignored,
    plugin_node_effect_has_audio_in_port (#[ignore]),
    plugin_node_generator_no_audio_in_port (#[ignore]).

---

Commit 3 — Feedback loop break (ADR-028)
  (paraclete-node-api, paraclete-runtime, paraclete-nodes)
  Tests after: 365 (+8 from 357)

  Node trait additions (paraclete-node-api/src/node.rs)

    Three default no-op methods: is_loop_break() → bool (false), loop_break_prev()
    → &[f32] (empty), loop_break_swap() → (). Non-breaking — all have defaults.
    Only LoopBreakNode overrides them.

  CycleError new variants (paraclete-runtime/src/configurator.rs)

    MissingLoopBreak { cycle: Vec<u32> } — cycle detected, no break node.
    TooManyLoopBreaks { cycle, count } — more than one break in one cycle.
    AudioBuffer cycles remain unconditionally rejected.

  LoopBreakNode (paraclete-nodes/src/loop_break.rs)

    CvSignal-only, two ports (cv_in/cv_out). Holds prev and next Vec<f32>
    blocks; prev is presented as output, next captures input. loop_break_swap()
    does std::mem::swap(prev, next). type_tag: "loop_break". serialize()
    returns empty (feedback buffer is transient; regenerates within one cycle).

  Executor pre/post phases (paraclete-runtime/src/executor.rs)

    NodeExecutor gains back_edges: Vec<LoopBackEdge>. The pre-execution phase
    writes LoopBreakNode's prev buffer into downstream node input slots before
    any process() call. The post-execution phase calls loop_break_swap() on
    every LoopBreakNode after all nodes finish. Topological sort excludes back
    edges so LoopBreakNode sorts as a sink.

    Design note: back edge exclusion from the sort graph is the key invariant.
    Without it, topological sort would fail (cycle remains). The LoopBreakNode
    runs last in the cycle (captures the final output), and its prev buffer is
    presented to the next cycle's downstream node in the pre-phase.

  NodeRegistry registration: "loop_break" registered.

  New tests (8): loop_break_node_initial_output_zero, loop_break_node_output_lags_one_cycle,
    loop_break_node_is_loop_break_true, non_loop_break_node_is_loop_break_false,
    connect_cycle_no_loop_break_returns_error, connect_cycle_one_loop_break_accepted,
    connect_cycle_two_loop_breaks_rejected, loop_break_executor_pre_phase_injects_prev.

---

Commit 4 — Dynamic topology (ADR-029)
  (paraclete-runtime, paraclete-app)
  Tests after: 374 (+9 from 365)

  NodeRegistry (paraclete-app/src/registry.rs)

    HashMap<String, Arc<dyn Fn() -> Box<dyn Node> + Send + Sync>> registry.
    build_registry() registers all first-party node types. CLAP plugin nodes
    are not registered — they require PluginLibrary at construction and are
    inserted into the graph directly by the caller.

    Design note: Arc wrapping allows registry.build(tag) to return a fresh
    Box<dyn Node> on every call (the Arc is cloned, not the constructor itself).
    Fn() closures are zero-argument — initial_params are applied after construction
    via set_initial_params().

  NodeConfigurator additions (paraclete-runtime/src/configurator.rs)

    add_node_tagged(node, type_tag) — stores type_tag in type_tag_map for
    project v2 serialization.
    type_tag_for(id) → Option<&str> — retrieves stored tag.
    remove_node(id) — removes node and severs all connected edges via petgraph
    edge iteration. Returns Box<dyn Node> (caller can inspect final state).
    disconnect(src, src_port, dst, dst_port) — removes a specific edge.
    rebuild_executor() — non-consuming build; returns a new NodeExecutor each
    call. The configurator retains node ownership. Used by apply_patch() after
    each topology change batch.

  AudioEngine additions (paraclete-runtime/src/audio_engine.rs)

    pause() (non-blocking), wait_paused() (blocks up to 500 ms, panics if
    exceeded — indicates hung audio thread), resume_with_executor(executor).
    AudioEngine::new_paused() constructor added for test contexts.

    Design note: The 500 ms deadline is chosen to be well above any practical
    audio buffer duration (~11 ms at 512/44100). A hang after 500 ms means the
    audio callback is completely stuck — aborting is the correct response to
    prevent a silent deadlock.

  apply_patch() (paraclete-app/src/patch.rs)

    Applies TopologyChange variants (AddNode, RemoveNode, AddEdge, RemoveEdge)
    atomically via pause-rebuild-resume. Returns Vec<u32> of new node IDs for
    AddNode changes. A failed change aborts the batch at that step; prior
    changes are NOT rolled back (ADR-029 specification). One buffer of silence
    (~5 ms) per call.

  Project file format v2 (paraclete-app/src/project.rs)

    NodeSnapshot gains type_tag: String. save_project_v2() writes version 2.
    load_project_v2() restores topology from edges (authoritative in v2).
    v1 files load with a warning ("version 1 detected; topology unchanged").

  New tests (9): node_registry_build_known_type_tag, node_registry_build_unknown_type_tag,
    node_registry_known_type_tags_contains_loop_break, apply_patch_add_node_returns_id,
    apply_patch_unknown_type_tag_returns_error,
    apply_patch_add_edge_cycle_without_loop_break_returns_error,
    configurator_remove_node_severs_edges, project_v2_save_load_roundtrip,
    project_v1_load_emits_warning.

---

Commit 5 — Sequencer as CV source
  (paraclete-nodes, paraclete-app)
  Tests after: 381 (+7 from 374)

  Sequencer::with_cv_outputs(n) (paraclete-nodes/src/sequencer.rs)

    Constructor adding n CvSignal output ports to the Sequencer. Existing 3-port
    layout (clock_in=0, events_in=1, events_out=2) is preserved unchanged.
    CV ports start at PORT_CV_OUT_BASE = 3. ports() switches from a fixed-size
    array to Vec<PortDescriptor> to accommodate variable port counts.

    Design note: The spec document listed "2+i" for CV port IDs in §5.2, but
    this conflicted with the existing layout where PORT_EVENTS_OUT = 2. The
    spec table omitted events_in from its "existing ports" list. The correct
    offset is 3+i (= PORT_CV_OUT_BASE). CLAUDE.md updated to reflect the
    actual implementation.

  Step::cv_locks (paraclete-nodes/src/sequencer.rs)

    Vec<(u16, f32)> of (cv_port_index, value) pairs per step. Applied at step
    fire time via current_cv[idx] = val. Out-of-range indices silently ignored.
    sample-and-hold: current_cv persists unchanged between step fires.

  process() changes

    Each cycle: writes current_cv[i] constant-filled into signal output slot
    for port PORT_CV_OUT_BASE + i. Step fire: applies cv_locks before pushing
    events.

  Serialization v2 for Sequencer

    cv_locks per step serialized as varint-encoded pairs in the existing RON
    format. v1 Sequencer data still loadable (cv_locks default to empty).

  NodeRegistry: "sequencer_cv" registered as Sequencer::with_cv_outputs(1).

  Design note: Tests initially used step 0 (which fires only after a full
  16-step wrap, ~960 ticks after global_start). Tests were corrected to use
  step 1 (first step to fire at tps=60, around 60 ticks after global_start).

  New tests (7): sequencer_cv_output_ports_present, sequencer_no_cv_no_extra_ports,
    sequencer_cv_output_initial_zero, sequencer_cv_lock_updates_on_step_fire,
    sequencer_cv_lock_sample_and_hold, sequencer_cv_lock_out_of_range_ignored,
    sequencer_cv_step_lock_serialization_roundtrip.

---

Commit 6 — GraphNode and InnerGraphNode (ADR-023)
  (paraclete-node-api, paraclete-graph-nodes [new], paraclete-app)
  Tests after: 387 (+8 within paraclete-graph-nodes)

  GraphNode marker trait (paraclete-node-api/src/node.rs)

    pub trait GraphNode: Node {} — zero methods beyond the Node supertrait.
    Placed before Node trait definition in node.rs. Published from
    paraclete-node-api (LGPL3), making it available to third-party authors.
    Re-exported via pub use node::{GraphNode, Negotiable, Node} in lib.rs.

  paraclete-graph-nodes crate (new, GPL3)

    The only crate that may implement Node while also depending on
    paraclete-runtime (for NodeExecutor). This resolves the ADR-022 portability
    constraint: paraclete-nodes cannot depend on paraclete-runtime, so any node
    that owns an inner executor must live in a separate crate.

    Dependencies: paraclete-node-api (LGPL3), paraclete-nodes (GPL3),
    paraclete-runtime (GPL3). License: GPL-3.0-or-later.

  InnerGraphNode (paraclete-graph-nodes/src/inner_graph.rs)

    Inner graph: Sequencer (steps 0,2,4,6 pre-activated) → AnalogEngine::kick().
    Three outer ports: clock_in (Clock, Input), cv_in_0 (Cv, Input),
    audio_out (Audio, Output).

    activate(): builds inner executor (NodeConfigurator → build_executor()),
    allocates inner_audio_buf = vec![0.0; 2 * block_size] (interleaved stereo).
    process(): forwards all incoming events to inner Sequencer via
    inject_event_for_node(); runs executor.process(&mut inner_audio_buf, 2);
    copies interleaved inner audio to outer audio_outputs[0] with +=.
    deactivate(): drops executor, clears inner_audio_buf.
    serialize() returns empty; deserialize() is a no-op (inner state not
    persisted at P9 — full inner graph serialization is a P10 concern).
    type_name() returns "InnerGraphNode".
    impl GraphNode for InnerGraphNode {}.

    Design notes:
    - executor is Option<NodeExecutor> in the struct; built at activate() time
      when sample_rate and block_size are known. new() uses default values
      (44100.0, 512) as placeholders only — executor is None until activate().
    - Audio copy uses += not = to correctly sum into the outer mix buffer
      (outer ProcessOutput may have pre-existing audio from other nodes).
    - The spec's §6.3 described clock_in as PortType::MidiMessage, but the
      actual codebase uses PortType::Clock throughout (consistent with
      InternalClock → Sequencer connections). PortType::Clock was used in the
      implementation.
    - inject_inner_event() is a public method for test access; exposes the
      inner executor's inject_event_for_node() without exposing the executor
      itself.

  NodeRegistry: "inner_graph" registered as InnerGraphNode::new().
  paraclete-app/Cargo.toml: added paraclete-graph-nodes dependency.

  Design note (code review finding): GraphNode marker trait's doc comment was
  accidentally merged into Node's leading comment block on first write. Fixed
  by placing GraphNode before Node with its own standalone doc block.

  New tests (8 in paraclete-graph-nodes): inner_graph_node_ports_correct,
    inner_graph_node_is_graph_node, inner_graph_node_activate_no_panic,
    inner_graph_node_deactivate_no_panic, inner_graph_node_serialize_returns_empty,
    graph_node_marker_trait_implemented, inner_graph_node_produces_audio_on_note_on,
    registry_contains_inner_graph (stub — real test in paraclete-app).

---

Test follow-up commit (cee78c0) — registry integration tests

  Added 2 real integration tests to paraclete-app/tests/patch_tests.rs:
  registry_contains_inner_graph (asserts build("inner_graph") returns Some and
  type_name() == "InnerGraphNode") and registry_known_type_tags_contains_inner_graph.

  The paraclete-graph-nodes crate cannot depend on paraclete-app (cycle), so
  the full registry test must live in the app integration test suite. Total: 389.

DESIGN CHOICES AND DEVIATIONS
------------------------------

PORT_CV_OUT_BASE = 3, not 2 (C5)

  The spec §5.2 table listed CV port IDs as "2+i", but PORT_EVENTS_OUT already
  occupies port 2 in the existing 3-port layout (clock_in=0, events_in=1,
  events_out=2). The spec table omitted events_in from its "existing ports" list.
  Using 2+i would have silently shadowed PORT_EVENTS_OUT. Implementation uses
  PORT_CV_OUT_BASE=3. CLAUDE.md updated.

InnerGraphNode uses PortType::Clock for clock_in (C6)

  Spec §6.3 listed clock_in as "PortType::MidiMessage". All InternalClock →
  Sequencer connections throughout the codebase use PortType::Clock. Using
  MidiMessage would have made InnerGraphNode's clock_in unconnectable to the
  standard InternalClock clock_out port. PortType::Clock was used.

InnerGraphNode executor is Option<NodeExecutor> (C6)

  Spec §6.3 showed executor: NodeExecutor in the struct directly. NodeExecutor
  is only valid after activate() when sample_rate/block_size are known. Using
  Option<NodeExecutor> avoids a panic on new() when those values are not yet
  available. The executor is built in activate() with correct parameters.

GraphNode doc comment placement (C6)

  Doc comment accidentally merged into Node's leading block on first write.
  Fixed before commit: GraphNode is defined before Node with its own standalone
  /// block. Code review found this issue.

license = "GPL-3.0-or-later" not "GPL-3.0" (C6)

  The spec's §6.1 toml snippet showed license = "GPL-3.0". All other GPL3
  crates in the workspace use "GPL-3.0-or-later". Corrected for consistency.
  Caught by code review subagent before commit.

Test step indexing (C5)

  Initial CV tests used step 0, which only fires after a full 16-step pattern
  wrap (~960 ticks). After global_start, step 1 is the first to fire (at
  tps=60, approximately 60 ticks in). Tests corrected to use steps 1 and 2.

BUGS FOUND AND ADDED TO TRACKER
---------------------------------

No new bugs were discovered during P9 implementation. BUG-008 (pre-existing,
added in C2) was the only bug tracker entry during this phase. All existing
bugs (BUG-001 through BUG-008) remain open as noted.

TEST SUMMARY
------------

| Commit | Delta | Total |
|--------|-------|-------|
| P8 baseline | — | 340 |
| C1 (encoder publishing) | +8 | 348 |
| C2 (housekeeping) | +9 | 357 |
| C3 (loop break) | +8 | 365 |
| C4 (dynamic topology) | +9 | 374 |
| C5 (sequencer CV) | +7 | 381 |
| C6 (GraphNode/InnerGraphNode) | +8 | 389 |
| Registry test follow-up | +2 | 391 |

Wait — the 2 registry tests were added in the follow-up after the C6 count of
387 (which included the stub registry_contains_inner_graph in graph-nodes).
The stub is still there. Actual distinct test functions: 389 by cargo test count.

P9 target: ≥ 385. Final: 389. Target met.

ARCHITECTURE TREE CHECKS
--------------------------

cargo tree -p paraclete-nodes shows no dependency on paraclete-runtime.
cargo tree -p paraclete-node-api shows no dependency on paraclete-nodes or
  paraclete-runtime. LGPL3 boundary is clean.
cargo tree -p paraclete-graph-nodes shows dependencies on both paraclete-nodes
  and paraclete-runtime. This is the expected and correct topology (ADR-022,
  ADR-023).

KNOWN DEFERRED ITEMS (unchanged from P9 spec)
----------------------------------------------

- Step period off-by-one (241 ticks not 240) — deferred P10+
- AudioBuffer feedback cycles — deferred P10+ (requires OQ-7 oversampling)
- Inner GraphNode topology patching at runtime — deferred P10
- CLAP plugin nodes not in NodeRegistry — by design; review at P10
- InnerGraphNode::serialize() returns empty — deferred P10
- Undo/redo for apply_patch() — deferred
- Sequencer TrigCondition / StepTiming not serialized — pre-P9 carryover
- Signed micro-timing distinct from positive — pre-P9 carryover
- LFO tempo sync, LadderFilter HP/BP modes — pre-P9 carryover
- CLAP GUI extension, note expressions, hot-reload — pre-P9 carryover
- SubgraphPlugin machine slot swapping — deferred to P10 (depends on stable
  GraphNode API which is now P9)
