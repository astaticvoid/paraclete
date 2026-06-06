# ADR-029: Dynamic Graph Topology — Patchable Modular Graph

**Date:** June 2026
**Status:** Accepted — P9 implementation target

---

## Decision

The node graph becomes patchable at runtime: nodes can be added, removed, connected,
and disconnected while the application is running. Topology changes are applied via a
**pause-rebuild-resume** protocol that accepts one buffer of audio silence (~5 ms).

A `NodeRegistry` maps type-tag strings to zero-argument constructor functions, enabling
runtime node creation and topology reconstruction from project files.

Project file format is amended to version 2: `NodeSnapshot` gains a `type_tag` field;
edges become authoritative for reconstruction (not just validation on load).

---

## Context

Through P8, Paraclete's graph topology was fixed at application startup.
`NodeConfigurator` built the graph before `build_executor()` moved nodes into the
audio-thread executor. ADR-025 noted: *"a full topology-from-file model is P9+ when
the graph becomes user-configurable at runtime."* The P9 roadmap names this capability
"patchable primitive node graph."

The instrument vision (`instrument-vision.md`) describes the operator patching the
instrument the way a modular synthesist patches cables. That metaphor is the design
tiebreaker for all decisions in this ADR.

The `capability_document()` Box::leak finding (P8 Commit 6) is a prerequisite blocker:
calling `capability_document()` per topology change would leak memory without bound.
This must be resolved (NodeConfigurator caching) before dynamic topology is active.

---

## Why Pause-Rebuild-Resume (Not Lock-Free Hot-Patch)

Two approaches were evaluated:

**Option A — Pause-rebuild-resume.** Signal the audio thread to stop after the current
buffer, wait for acknowledgement, rebuild the executor on the main thread, resume. One
buffer of silence on each topology change.

**Option B — Lock-free hot-patch.** An SPSC command queue carries topology changes
(AddNode, RemoveNode, Connect, Disconnect) to the audio thread, which drains them at
the start of each buffer without pausing.

Option B is conceptually attractive but introduces significant complexity:
- Adding a node requires moving a `Box<dyn Node>` across the thread boundary without
  allocation on the audio thread. Pre-allocation schemes add substantial bookkeeping.
- Removing a node must defer heap deallocation to the main thread. The audio thread
  cannot safely `drop(node)` because drop may allocate/free.
- Reconnecting edges requires recomputing the topological execution order on the audio
  thread — a non-trivial computation that should not run inside a real-time callback.
- Loop-break node detection and `back_edge` tracking (ADR-028) further complicate
  the incremental sort.

Option A is chosen. Rationale:

1. **Honest metaphor:** In hardware modular synthesis, patching a cable produces a
   brief noise or silence. A single-buffer pause (~5 ms) is the digital analogue.
   The instrument vision describes modular patching; the behaviour is authentic.
2. **Safe:** No audio-thread allocation, no cross-thread node movement, no real-time
   topology analysis. The audio thread is only ever in one of two states: running a
   stable executor, or paused.
3. **Correct:** The main thread has full access to `NodeConfigurator`, can run cycle
   detection (ADR-005 / ADR-028), and can rebuild the executor with no time pressure.
4. **Incremental upgrade path:** If performance profiling at P10+ reveals that 5 ms
   pauses are unacceptable for live performance, Option B can be added as a fast path
   without changing the observable semantics.

---

## NodeRegistry

`NodeRegistry` maps type-tag strings to constructor closures. It is defined and
populated in `paraclete-app` (GPL3). The main thread uses it; the audio thread does
not need it.

```rust
// In paraclete-app:

pub struct NodeRegistry {
    constructors: HashMap<String, Arc<dyn Fn() -> Box<dyn Node> + Send + Sync>>,
}

impl NodeRegistry {
    pub fn new() -> Self;

    /// Register a zero-argument constructor for a type_tag.
    /// Called once per node type at application startup.
    pub fn register(
        &mut self,
        type_tag: impl Into<String>,
        ctor: impl Fn() -> Box<dyn Node> + Send + Sync + 'static,
    );

    /// Construct a new node instance from its type_tag.
    /// Returns None if the type_tag is not registered.
    pub fn build(&self, type_tag: &str) -> Option<Box<dyn Node>>;

    /// Returns all registered type_tags (for tooling / TUI display).
    pub fn known_type_tags(&self) -> Vec<&str>;
}
```

**Standard registrations** (in `build_registry()` in `paraclete-app`):

```rust
pub fn build_registry() -> NodeRegistry {
    let mut r = NodeRegistry::new();
    r.register("internal_clock",       || Box::new(InternalClock::new()));
    r.register("sequencer",            || Box::new(Sequencer::new()));
    r.register("sampler",              || Box::new(Sampler::new()));
    r.register("loop_break",           || Box::new(LoopBreakNode::new()));
    r.register("distortion",           || Box::new(DistortionNode::new()));
    r.register("filter",               || Box::new(FilterNode::new()));
    r.register("ladder_filter",        || Box::new(LadderFilterNode::new()));
    r.register("oscillator",           || Box::new(OscillatorNode::new()));
    r.register("envelope",             || Box::new(EnvelopeNode::new()));
    r.register("lfo",                  || Box::new(LfoNode::new()));
    r.register("reverb",               || Box::new(ReverbNode::new()));
    r.register("delay",                || Box::new(DelayNode::new()));
    r.register("mix",                  || Box::new(MixNode::new()));
    r.register("split",                || Box::new(SplitNode::new()));
    r.register("audio_output",         || Box::new(AudioOutputNode::new()));
    r.register("analog_engine:kick",   || Box::new(AnalogEngine::kick()));
    r.register("analog_engine:snare",  || Box::new(AnalogEngine::snare()));
    r.register("analog_engine:hihat",  || Box::new(AnalogEngine::hihat()));
    r.register("fm_engine:kick",       || Box::new(FmEngine::kick()));
    r.register("fm_engine:bell",       || Box::new(FmEngine::bell()));
    r.register("fm_engine:bass",       || Box::new(FmEngine::bass()));
    r
    // clap_plugin: built separately via PluginLibrary; not in NodeRegistry
}
```

`clap_plugin` nodes are not registered here because construction requires a
`PluginLibrary` argument. They are built by `paraclete-clap-host` separately and
inserted into `NodeConfigurator` directly.

---

## TopologyChange

```rust
// In paraclete-app:

pub enum TopologyChange {
    /// Add a new node. Returns the new NodeId via apply_patch's Vec<u32>.
    AddNode {
        type_tag: String,
        /// Applied via set_initial_params() after construction, before activate().
        initial_params: HashMap<String, f64>,
    },
    /// Remove a node and sever all connected edges.
    RemoveNode { id: u32 },
    /// Connect two ports.
    AddEdge {
        src: u32, src_port: u16,
        dst: u32, dst_port: u16,
    },
    /// Disconnect two ports.
    RemoveEdge {
        src: u32, src_port: u16,
        dst: u32, dst_port: u16,
    },
}

pub enum PatchError {
    UnknownTypeTag(String),
    NodeNotFound(u32),
    PortNotFound { node: u32, port: u16 },
    CycleError(CycleError),
    ConfigError(String),
}
```

---

## `apply_patch`

```rust
// In paraclete-app:

/// Apply a batch of topology changes atomically.
///
/// Pauses the audio engine (one buffer of silence), applies all changes to the
/// NodeConfigurator, rebuilds the executor, and resumes. Returns one NodeId per
/// AddNode change in the order the AddNode entries appear in `changes`.
///
/// If any change fails (UnknownTypeTag, cycle, etc.) the patch is aborted at
/// that step. Successfully applied earlier changes in the batch are NOT rolled
/// back; it is the caller's responsibility to apply corrective changes if needed.
pub fn apply_patch(
    changes: Vec<TopologyChange>,
    engine: &mut AudioEngine,
    conf: &mut NodeConfigurator,
    registry: &NodeRegistry,
) -> Result<Vec<u32>, PatchError>;
```

**Protocol:**

```
1. engine.pause()
2. engine.wait_paused()       // blocks main thread ≤ one buffer period
3. for each change in changes:
     AddNode { type_tag, initial_params }:
         node = registry.build(&type_tag)?  → PatchError::UnknownTypeTag
         node.set_initial_params(&initial_params)
         id = conf.add_node(node)
         new_ids.push(id)
     RemoveNode { id }:
         conf.remove_node(id)?              → PatchError::NodeNotFound
     AddEdge { src, src_port, dst, dst_port }:
         conf.connect(src, src_port, dst, dst_port)?  → PatchError::CycleError
     RemoveEdge { src, src_port, dst, dst_port }:
         conf.disconnect(src, src_port, dst, dst_port)?
4. new_executor = conf.rebuild_executor()
5. engine.resume_with_executor(new_executor)
6. return Ok(new_ids)
```

---

## `AudioEngine` Additions

```rust
// In paraclete-runtime:

impl AudioEngine {
    /// Signal the audio thread to stop processing after the current buffer.
    /// Non-blocking. Call `wait_paused()` to block until acknowledged.
    pub fn pause(&self);

    /// Block the calling thread until the audio thread confirms it is paused.
    /// Returns immediately if already paused. Timeout: 500 ms (panics if exceeded;
    /// indicates audio thread is hung, which is a fatal condition).
    pub fn wait_paused(&self);

    /// Replace the running executor and resume audio processing.
    /// Must be called only when `wait_paused()` has returned.
    pub fn resume_with_executor(&self, executor: NodeExecutor);
}
```

**Implementation:** atomic `pause_requested: AtomicBool`; the audio callback checks
it at the start of each buffer. When true, the callback stores the completed buffer,
sets `is_paused: AtomicBool = true`, and returns without processing. `wait_paused()`
spins on `is_paused`. `resume_with_executor()` atomically swaps the executor reference,
clears `is_paused` and `pause_requested`.

---

## `NodeConfigurator` Additions

```rust
// In paraclete-runtime:

impl NodeConfigurator {
    /// Remove a previously added node. Severs all edges connected to it.
    /// Returns the removed node (caller may inspect its final state).
    pub fn remove_node(&mut self, id: u32) -> Result<Box<dyn Node>, ConfigError>;

    /// Remove a specific edge. No-op if the edge does not exist.
    pub fn disconnect(
        &mut self,
        src: u32, src_port: u16,
        dst: u32, dst_port: u16,
    ) -> Result<(), ConfigError>;

    /// Build a NodeExecutor from the current configurator state.
    /// Unlike `build_executor()`, this is non-consuming and may be called
    /// multiple times (once per patch cycle).
    pub fn rebuild_executor(&mut self) -> NodeExecutor;
}
```

`rebuild_executor()` is semantically equivalent to the existing `build_executor()` but
non-consuming: it clones the node list into the new executor without moving ownership
out of the configurator. The configurator retains nodes; the executor receives cloned
(or Arc-shared) references. (The P7/P8 model moved nodes into the executor; P9
changes this to shared ownership or executor-side duplication as appropriate.)

**Alternatively** (simpler): `rebuild_executor()` does move nodes into the new
executor and repopulates the configurator from the new executor's node list on
completion. Either model is acceptable; the implementation decides. The API is stable
regardless.

---

## Capability Document Caching (Prerequisite — P9 Commit 2)

`capability_document()` currently leaks two strings per call via `Box::leak`. With
dynamic topology, each `AddNode` in a patch would leak two more strings unboundedly.

Fix (non-breaking): `NodeConfigurator` caches each node's `CapabilityDocument` at
registration time. `capability_document()` is called once per node per session.

```rust
// In NodeConfigurator:
cap_doc_cache: HashMap<u32, CapabilityDocument>,

// In add_node():
let doc = node.capability_document();
self.cap_doc_cache.insert(node_id, doc);

// In get_node_cap_doc():
self.cap_doc_cache.get(&id).cloned()
```

With caching, the total leak is bounded: 2 strings × (number of distinct node
instances ever registered in the session). For a typical instrument (< 64 nodes), this
is < 128 leaked strings — a fixed, bounded overhead.

This fix MUST be in place before `apply_patch()` is called for the first time. It is
implemented in P9 Commit 2 and is a prerequisite for Commit 4.

---

## Project File Format Version 2

ADR-025's v1 format is amended. `NodeSnapshot` gains a `type_tag` field:

```rust
#[derive(Serialize, Deserialize)]
struct NodeSnapshot {
    id:        u32,
    type_tag:  String,   // NEW in v2: used to reconstruct the node on load
    type_name: String,   // Retained from v1: human-readable label only
    state:     Vec<u8>,  // Node::serialize() output
}
```

**Edges are now authoritative in v2.** The v1 caveat — *"edges are for validation
only; runtime topology takes precedence"* — is lifted. On v2 load:

```
1. For each NodeSnapshot:
     node = registry.build(snapshot.type_tag)?
     conf.add_node_with_id(node, snapshot.id)   // id is stable from file
     node.deserialize(&snapshot.state)
2. For each Edge:
     conf.connect(edge.src_node, edge.src_port, edge.dst_node, edge.dst_port)?
3. conf.build_executor()
```

**Backward compatibility:** v1 project files remain loadable via the existing
fixed-construction-sequence path. A v1 file opened after a topology change produces a
warning: *"Project v1 does not contain topology data. Node states applied to current
graph; topology unchanged."*

**Version bump:** `save_project()` now writes `version: 2`. The `ron` format and
top-level structure are otherwise unchanged per ADR-025. Only `NodeSnapshot` has new
fields.

---

## Scope Boundaries at P9

**In scope:**
- Outer graph topology only. The outer `NodeConfigurator` / `NodeExecutor` pair.
- All first-party node types registered in `NodeRegistry`.
- Topology saved and restored via project file v2.

**Not in scope at P9:**
- **Inner GraphNode topology.** The inner graph of a `GraphNode` (ADR-023) is rebuilt
  as a unit. Patching the inner graph at runtime is deferred.
- **CLAP plugin nodes.** `PluginLibrary` loads at startup; dynamic plugin loading
  without restart is deferred (noted as "hot plugin reload" in P8 known gaps).
- **UI for patch operations.** No TUI node-list / connection display at P9; topology
  changes are API-only. A visual patcher is a future UI milestone.
- **Undo/redo.** Patch operations are permanent. Undo is a future concern.

---

## Consequences

- `NodeRegistry`, `TopologyChange`, `PatchError`, `apply_patch()` added to
  `paraclete-app` (GPL3)
- `AudioEngine::pause()`, `wait_paused()`, `resume_with_executor()` added to
  `paraclete-runtime` (GPL3)
- `NodeConfigurator::remove_node()`, `disconnect()`, `rebuild_executor()` added to
  `paraclete-runtime`
- `NodeConfigurator` gains `cap_doc_cache: HashMap<u32, CapabilityDocument>`
- Project file format bumped from v1 to v2; save writes v2; load reads both
- `Node::type_name()` (P7) is still available; `type_tag` is the construction key
  while `type_name` is the display label — they are distinct
- `build_from_instrument()` (P8) is unchanged; it continues to populate the
  configurator at startup. `apply_patch()` modifies the same configurator at runtime.
- No change to `paraclete-node-api` (LGPL3 boundary clean)
- No change to `paraclete-nodes` portability rule

---

## References

- ADR-025 — Project File Format (v1 definition; v2 amendments are additive)
- ADR-028 — Loop Break Node (cycle acceptance; `AddEdge` on a cycle requires a
  `LoopBreakNode` or returns `PatchError::CycleError`)
- ADR-023 — Instrument Encapsulation (GraphNode; inner graph not patchable via this
  ADR at P9)
- ADR-018 — Cellular Architecture (nodes are universal primitives; dynamic topology
  is the natural evolution of the cellular model)
- ADR-005 — Scheduler Cycles (cycle detection applies to `AddEdge` operations)
- `p8-report.md` — `capability_document()` Box::leak finding (prerequisite fix)
- `instrument-vision.md` — modular patching as the operator mental model
