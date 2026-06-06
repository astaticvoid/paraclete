# Paraclete — P9 Interface Specification

> **Implementation blueprint.** These are the contracts P9 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
>
> **P9 deliverable:** Encoder value publishing (gate). Housekeeping residuals.
> Feedback loop break (`LoopBreakNode`). Patchable primitive node graph.
> Sequencer as CV source. GraphNode inner graph with nested sequencers.  
> **Last updated:** June 2026  
> **Baseline:** P8 complete — 340 tests, 0 failures  
> **Depends on:** p0–p8 interfaces and reports — all prior types assumed
> present and correct.  
> **References:** ADR-028 (loop break node), ADR-029 (dynamic topology),
> ADR-023 (instrument encapsulation), ADR-005 (scheduler cycles),
> ADR-009 (clock federation), ADR-019 (universal parameter control),
> ADR-025 (project file format), ADR-001 (license)

-----

## How to use this document

**Commit 1 is gated.** No new P9 work begins until Commit 1 passes
`cargo test --workspace` with zero failures. Commit 1 closes the encoder
value display gap — a broken user-facing feature present since TUI launch.
It has no new features; it is pure gap closure.

**Implement in commit order.** Commit 2 (housekeeping) must precede Commit 4
(dynamic topology) because the `cap_doc_cache` fix is a hard prerequisite
for `apply_patch()` correctness. Commits 3 and 4 are independent of each
other and may be developed in parallel, but Commit 3 must merge before
Commit 4 because the `NodeRegistry` must register `loop_break`.

**The new crate.** Commit 6 introduces `paraclete-graph-nodes` (GPL3).
This crate is the only way to express a node that internally owns a
`NodeExecutor`. It sits above `paraclete-nodes` (portability) and above
`paraclete-runtime` (executor access). No existing crate changes license.

**Portability rule applies throughout.** `cargo tree -p paraclete-nodes`
must show no dependency on `paraclete-runtime` or `paraclete-scripting`
before merge for any commit touching `paraclete-nodes`. This invariant also
applies to `paraclete-node-api`. The new `paraclete-graph-nodes` crate is
explicitly permitted to depend on `paraclete-runtime` — that is its
architectural purpose.

**StateBusValue variants.** The enum is `Float(f64)`, `Int(i64)`,
`Bool(bool)`, `Text(String)`. Every use of the state bus must use the
correct variant. The TUI match logic depends on this.

**Parameter names are long-term contracts.** No new parameter name is
introduced in P9 without first checking the ADR-019 canonical names table.
Sequencer CV lock indices are positional, not named — they are not ADR-019
parameter names.

**`paraclete-node-api` is LGPL3 at v0.1.0.** Every new method added to the
`Node` trait must have a default implementation. No breaking change to the
public API is permitted.

-----

## Commit Sequence

| Commit | Crate(s) | Deliverable |
|--------|----------|-------------|
| **1 (GATED)** | `paraclete-node-api`, `paraclete-nodes` | Encoder value publishing via `publish_bank_state()` |
| 2 | `paraclete-node-api`, `paraclete-runtime`, `paraclete-nodes`, `paraclete-clap-host`, `paraclete-app` | Housekeeping: cap-doc cache, serde migration, `set_initial_params` coverage, effect CLAP plugins |
| 3 | `paraclete-node-api`, `paraclete-runtime`, `paraclete-nodes` | Feedback loop break — `LoopBreakNode`, executor pre/post phases (ADR-028) |
| 4 | `paraclete-runtime`, `paraclete-app` | Dynamic topology — `NodeRegistry`, `apply_patch()`, pause-rebuild-resume (ADR-029) |
| 5 | `paraclete-nodes` | Sequencer as CV source — CV output ports, step CV locks |
| 6 | `paraclete-node-api`, `paraclete-nodes`, `paraclete-graph-nodes` (new) | GraphNode marker trait, `InnerGraphNode` with nested sequencer |

-----

# Part 1: Encoder Value Publishing (Commit 1 — GATED)

**Crates:** `paraclete-node-api` (LGPL3), `paraclete-nodes` (GPL3)

**Problem:** The TUI encoder row displays `0.0` for every parameter because
no node publishes per-parameter values to the state bus. `published_state()`
on all generator nodes returns only coarse status (playing, active, etc.).
The `ParameterBank` slots hold live values but they never reach the bus.

**Fix:** A helper `publish_bank_state()` in `paraclete-node-api` iterates
a `ParameterBank` and pushes one `StateBusValue::Float` entry per declared
parameter into the node's state bus output. Nodes call it from
`published_state()`. The TUI reads `/node/{id}/{name}` and gets a live value.

This is the gate. `cargo test --workspace` must pass zero failures after this
commit before any other P9 work begins.

-----

## 1.1 `ParameterSlot` — name field

`ParameterSlot` in `paraclete-node-api` gains a `name` field, stored when a
node calls `bank.declare_param(name, default)` at initialisation time. This
name is the canonical ADR-019 parameter name (e.g. `"decay"`, `"drive"`).

```rust
// In paraclete-node-api:

pub struct ParameterSlot {
    pub name:    String,   // NEW: canonical parameter name (ADR-019)
    pub value:   f64,
    pub default: f64,
}
```

`ParameterBank::declare_param()` stores the name into the new field. No
change to call sites — the name was already being passed to `declare_param()`.

-----

## 1.2 `ParameterBank::iter_values()`

New method on `ParameterBank`:

```rust
impl ParameterBank {
    /// Iterate declared parameter slots as (name, current_value) pairs.
    /// Only slots that have been declared are yielded (not uninitialised slots).
    pub fn iter_values(&self) -> impl Iterator<Item = (&str, f64)> + '_;
}
```

-----

## 1.3 `publish_bank_state()` free function

New free function in `paraclete-node-api`:

```rust
/// Push one state bus entry per declared ParameterBank slot.
///
/// Pushes path `/node/{node_id}/{param_name}` with value
/// `StateBusValue::Float(current_value)` for every declared parameter
/// in the bank. Appends to `buf`; does not clear it.
///
/// Nodes call this from `published_state()`. The TUI subscribes to
/// `/node/{id}/*` and renders the value under the parameter name.
pub fn publish_bank_state(
    node_id: u32,
    bank:    &ParameterBank,
    buf:     &mut Vec<(String, StateBusValue)>,
);
```

**Implementation:** iterate `bank.iter_values()`, format path
`format!("/node/{}/{}", node_id, name)`, push
`(path, StateBusValue::Float(value))`.

-----

## 1.4 Nodes — `published_state()` overrides

All nodes with a non-empty `ParameterBank` override `published_state()` to
call `publish_bank_state()`. The `node_id` argument must be the node's own
ID as assigned by `NodeConfigurator`. Nodes receive their ID at `activate()`
time; add an `id: u32` field, set it there.

**Node trait addition (LGPL3, default no-op):**

```rust
// In paraclete-node-api, Node trait:

/// Called by the executor before activate() to inform the node of its
/// assigned ID. Nodes that publish named state should store this.
/// Default: no-op.
fn set_node_id(&mut self, id: u32) { let _ = id; }
```

`NodeConfigurator::add_node()` calls `node.set_node_id(assigned_id)` after
assigning the ID.

**Affected nodes and their parameter sets** (all in `paraclete-nodes`):

| Node | Bank parameters (ADR-019 names) |
|------|---------------------------------|
| `DistortionNode` | `drive` |
| `FilterNode` | `cutoff`, `resonance` |
| `LadderFilterNode` | `cutoff`, `resonance` |
| `OscillatorNode` | `tune`, `waveform` |
| `EnvelopeNode` | `attack`, `decay`, `sustain`, `release` |
| `LfoNode` | `rate`, `depth`, `waveform` |
| `ReverbNode` | `room_size`, `damping`, `wet` |
| `DelayNode` | `time`, `feedback`, `wet` |
| `AnalogEngine` (all variants) | `tune`, `punch`, `decay`, `drive`, `tone` |
| `FmEngine` (all variants) | `tune`, `ratio`, `index`, `decay`, `feedback` |
| `Sampler` | `tune`, `attack`, `decay` |
| `PluginNode` | _(publishes CLAP-declared parameters by their plugin-assigned names)_ |

Each node's `published_state()`:

```rust
fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
    publish_bank_state(self.id, &self.bank, buf);
    // ... any additional node-specific entries (e.g. playing status)
}
```

`PluginNode` publishes its parameters via the CLAP params extension query.
The parameter key is the plugin-supplied name string, not an ADR-019 name.
If the plugin does not implement the params extension, `PluginNode` publishes
nothing for parameters (no change from P8 behaviour).

-----

## 1.5 Tests (Commit 1)

```
publish_bank_state_single_param
    Declare a ParameterBank with one "cutoff" param at value 0.7.
    Call publish_bank_state(42, &bank, &mut buf).
    Assert buf.len() == 1.
    Assert buf[0].0 == "/node/42/cutoff".
    Assert buf[0].1 == StateBusValue::Float(0.7).

publish_bank_state_multi_param
    Declare bank with "attack"=0.01, "decay"=0.3, "sustain"=0.8, "release"=0.5.
    Call publish_bank_state(7, &bank, &mut buf).
    Assert buf.len() == 4.
    Assert each path is "/node/7/{name}" and value matches.

publish_bank_state_empty_bank
    Empty bank (no declared params). Call publish_bank_state.
    Assert buf remains empty.

distortion_node_published_state_nonzero
    Construct DistortionNode, set_node_id(3).
    Set drive to 0.8 via CMD_BUMP_PARAM.
    published_state(&mut buf).
    Assert buf contains ("/node/3/drive", StateBusValue::Float(0.8)).

analog_engine_published_state_contains_decay
    Construct AnalogEngine::kick(), set_node_id(10).
    published_state(&mut buf).
    Assert buf contains an entry whose path ends with "/decay".
    Assert the value is StateBusValue::Float (exact value ≥ 0.0).

set_node_id_stored
    Construct FilterNode. Call set_node_id(99).
    published_state(&mut buf).
    Assert at least one path starts with "/node/99/".

parameter_slot_name_stored
    Declare ParameterBank param "resonance" at default 0.5.
    iter_values().collect::<Vec<_>>() contains ("resonance", 0.5).

iter_values_reflects_mutation
    Declare param "drive" at 0.0. Set value to 0.6 via bump.
    iter_values() yields ("drive", 0.6).
```

Commit 1 adds **+8 tests** → 348 total.

-----

# Part 2: Housekeeping (Commit 2)

**Crates:** `paraclete-node-api` (LGPL3), `paraclete-runtime` (GPL3),
`paraclete-nodes` (GPL3), `paraclete-clap-host` (GPL3), `paraclete-app` (GPL3)

Commit 2 closes four independent carryover items. None are features; all are
correctness or maintenance fixes. Order within this commit is flexible.

-----

## 2.1 Capability Document Cache (`paraclete-runtime`)

**Problem:** `capability_document()` is called on the same node multiple
times per session (TUI startup, project save, dynamic topology). Each call
leaks two heap strings via `Box::leak`.

**Fix:** `NodeConfigurator` caches each node's `CapabilityDocument` at
registration time and serves subsequent requests from the cache.

```rust
// In paraclete-runtime, NodeConfigurator:

pub struct NodeConfigurator {
    // ... existing fields ...
    cap_doc_cache: HashMap<u32, CapabilityDocument>,  // NEW
}

impl NodeConfigurator {
    // Modified — called after add_node() assigns the ID:
    //   doc = node.capability_document();
    //   self.cap_doc_cache.insert(node_id, doc);

    /// Returns the cached CapabilityDocument for a node.
    /// Returns None only if the node ID is not registered.
    pub fn get_node_cap_doc(&self, node_id: u32) -> Option<&CapabilityDocument>;
}
```

`add_node()` calls `node.capability_document()` exactly once and stores
the result. Subsequent calls to `get_node_cap_doc()` return a reference to
the cached document. No `Box::leak` occurs after this fix.

This fix MUST be merged before Commit 4 (`apply_patch()`).

-----

## 2.2 `serde_yaml` → `serde_yml`

`serde_yaml 0.9` is deprecated. Replace with `serde_yml` throughout.

**Changes:**
- `paraclete-app/Cargo.toml`: `serde_yaml = "0.9"` → `serde_yml = "0.9"`
- All `use serde_yaml::` → `use serde_yml::` in `paraclete-app`
- The `serde_yml` public API is drop-in compatible with `serde_yaml 0.9`

No logic changes. The instrument definition YAML format is unchanged. CI
confirms `cargo build --workspace` succeeds after the swap.

-----

## 2.3 `set_initial_params` Coverage

`set_initial_params()` allows a caller to supply parameter values at node
construction time before `activate()`. At P8, `DistortionNode` and
`FilterNode` implement it; four additional nodes are missing coverage.

The `Node` trait method (already present from P8):

```rust
// Node trait — already exists:
fn set_initial_params(&mut self, params: &HashMap<String, f64>) { let _ = params; }
```

**Add implementations for:**

`AnalogEngine` — store `initial_params: HashMap<String, f64>` in the struct;
apply via `ParameterBank` slot lookup in `activate()`.

`FmEngine` — same pattern.

`Sampler` — apply `tune`, `attack`, `decay` from params map in `activate()`.

`ReverbNode` — apply `room_size`, `damping`, `wet` from params map in
`activate()`.

**Pattern for each node:**

```rust
fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
    self.initial_params = params.clone();
}

fn activate(&mut self, sample_rate: f32, block_size: usize) {
    // ... existing setup ...
    for (name, value) in &self.initial_params {
        if let Some(id) = self.bank.id_for_name(name) {
            self.bank.set(id, *value);
        }
    }
}
```

Unknown param names are silently ignored (no panic, no error return).

-----

## 2.4 Effect-Type CLAP Plugins (`paraclete-clap-host`)

At P8, `PluginNode` is audio-output-only. Plugins that require audio input
(compressors, reverbs, saturators, etc.) cannot be used. Fix: `PluginNode`
queries the CLAP `audio_ports` extension at load time. If the plugin declares
input audio buses, `PluginNode` exposes a `PORT_AUDIO_IN` port and wires it
to the CLAP audio buffer input.

**Port layout change:**

| Scenario | Port 0 | Port 1 | Port 2 |
|----------|--------|--------|--------|
| Generator (no audio input, as today) | `events_in` | `audio_out` | — |
| Effect (has audio input) | `events_in` | `audio_in` | `audio_out` |

```rust
// In paraclete-clap-host, PluginNode:

pub const PORT_EVENTS_IN: u16 = 0;
pub const PORT_AUDIO_IN:  u16 = 1;   // present only when has_audio_input == true
pub const PORT_AUDIO_OUT: u16 = 1;   // generator (no audio input)
pub const PORT_AUDIO_OUT_EFFECT: u16 = 2; // effect (has audio input)
```

**Detection at construction:**

```rust
impl PluginNode {
    fn detect_audio_input(plugin: &ClapPlugin) -> bool {
        if let Some(audio_ports) = plugin.get_extension::<AudioPorts>() {
            audio_ports.count(false /* is_input: true */) > 0
        } else {
            false
        }
    }
}
```

**`has_audio_input: bool`** stored on `PluginNode`; gates port exposure in
`ports()` and process routing in `process()`.

**`process()` change for effects:**

When `has_audio_input == true`:
- Read from input port `PORT_AUDIO_IN` (buffer from upstream node)
- Pass buffer as CLAP audio input channels
- Write CLAP audio output to `PORT_AUDIO_OUT_EFFECT`

Generator path is unchanged.

**`capability_document()`** and **`ports()`** reflect the layout: an effect
plugin's `CapabilityDocument` includes `PORT_AUDIO_IN` as an `AudioBuffer /
Input` port.

**`build_from_instrument()` change:** `"clap_plugin"` entries with an audio
input plugin now connect correctly; the port name `"audio_in"` resolves to
`PORT_AUDIO_IN` for effect nodes.

-----

## 2.5 Tests (Commit 2)

```
cap_doc_cache_populated_on_add_node
    Add a node to NodeConfigurator.
    get_node_cap_doc(id) returns Some.
    Assert the returned document matches capability_document() output.

cap_doc_cache_hit_no_additional_leak
    Add a node; call get_node_cap_doc(id) three times.
    Assert all three return identical content (same cached value).
    (Leak reduction is not directly testable; structural coverage suffices.)

analog_engine_set_initial_params_applied
    AnalogEngine::kick().set_initial_params({"decay": 0.9}).
    activate(44100, 256).
    published_state(&mut buf); extract "decay" entry.
    Assert value is 0.9 (within f64 epsilon).

fm_engine_set_initial_params_applied
    FmEngine::fm_bass().set_initial_params({"index": 4.0}).
    activate(44100, 256).
    Assert bank value for "index" is 4.0.

sampler_set_initial_params_applied
    Sampler.set_initial_params({"tune": 0.5}).
    activate(44100, 256).
    Assert "tune" bank value is 0.5.

reverb_node_set_initial_params_applied
    ReverbNode.set_initial_params({"wet": 0.25}).
    activate(44100, 256).
    Assert "wet" bank value is 0.25.

set_initial_params_unknown_key_ignored
    AnalogEngine::snare().set_initial_params({"nonexistent_param": 99.0}).
    activate(44100, 256). Assert no panic.

plugin_node_effect_has_audio_in_port
    (tagged #[ignore] — requires an effect .clap binary)
    Construct PluginNode from an effect plugin (has audio input).
    ports() contains a port with kind AudioBuffer and direction Input.

plugin_node_generator_no_audio_in_port
    (tagged #[ignore] — requires a generator .clap binary)
    Construct PluginNode from a generator plugin (no audio input).
    ports() contains no Input AudioBuffer port.
```

Commit 2 adds **+9 tests** → 357 total. The two `#[ignore]` tests are not
counted in the total; they run as separate CI steps.

-----

# Part 3: Feedback Loop Break (Commit 3)

**Crates:** `paraclete-node-api` (LGPL3), `paraclete-runtime` (GPL3),
`paraclete-nodes` (GPL3)

Implements ADR-028. Introduces `LoopBreakNode` and the executor pre/post
phases that give sanctioned single-sample feedback loops correct semantics.

-----

## 3.1 Node Trait Additions (`paraclete-node-api`)

Three new default-no-op methods. Non-breaking (all have defaults). Only
`LoopBreakNode` overrides them.

```rust
// Node trait additions in paraclete-node-api:

/// Returns true if this node is a loop-break node.
///
/// The executor treats loop-break nodes specially: their stored
/// previous-cycle output is written into port buffers before the main
/// execution phase, and loop_break_swap() is called after all nodes
/// have processed.
///
/// Default: false. Only LoopBreakNode overrides this.
fn is_loop_break(&self) -> bool { false }

/// Returns the previous-cycle CvSignal block used as this cycle's output.
///
/// Called by the executor in the pre-execution phase. Must return a slice
/// of length block_size (as set in activate()). Default: empty slice.
fn loop_break_prev(&self) -> &[f32] { &[] }

/// Swap prev ← next buffers, advancing the stored output by one cycle.
///
/// Called by the executor after all nodes have processed.
/// Default: no-op.
fn loop_break_swap(&mut self) {}
```

-----

## 3.2 `CycleError` — New Variants

```rust
// In paraclete-runtime (or paraclete-node-api if CycleError lives there):

pub enum CycleError {
    /// A cycle was detected with no LoopBreakNode to sanction it.
    MissingLoopBreak {
        /// Node IDs forming the cycle.
        cycle: Vec<u32>,
    },
    /// A cycle was detected with more than one LoopBreakNode.
    /// Only one break point is allowed per cycle.
    TooManyLoopBreaks {
        cycle: Vec<u32>,
        count: usize,
    },
    // ... existing variants unchanged
}
```

-----

## 3.3 `LoopBreakNode` (`paraclete-nodes`)

```rust
// In paraclete-nodes:

pub struct LoopBreakNode {
    /// Previous cycle's captured input — presented as output this cycle.
    prev:       Vec<f32>,
    /// This cycle's captured input — becomes prev after swap.
    next:       Vec<f32>,
    block_size: usize,
}

impl LoopBreakNode {
    pub fn new() -> Self {
        LoopBreakNode { prev: Vec::new(), next: Vec::new(), block_size: 0 }
    }
}

impl Node for LoopBreakNode {
    fn activate(&mut self, _sample_rate: f32, block_size: usize) {
        self.block_size = block_size;
        self.prev = vec![0.0_f32; block_size];
        self.next = vec![0.0_f32; block_size];
    }

    fn deactivate(&mut self) {
        self.prev.clear();
        self.next.clear();
        self.block_size = 0;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // Capture this cycle's input for next cycle.
        let cv_in = input.modulation(0, self.block_size);
        self.next.copy_from_slice(cv_in);
        // Write prev (previous cycle) to output — idempotent with pre-phase.
        let cv_out = output.mod_output_mut(0, self.block_size);
        cv_out.copy_from_slice(&self.prev);
    }

    fn is_loop_break(&self) -> bool { true }
    fn loop_break_prev(&self) -> &[f32] { &self.prev }
    fn loop_break_swap(&mut self) { std::mem::swap(&mut self.prev, &mut self.next); }

    fn ports(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor { id: 0, name: "cv_in",  kind: PortType::CvSignal, direction: Direction::Input  },
            PortDescriptor { id: 1, name: "cv_out", kind: PortType::CvSignal, direction: Direction::Output },
        ]
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument { parameters: vec![], ports: self.ports() }
    }

    fn type_name(&self)   -> &'static str { "LoopBreakNode" }
    fn serialize(&self)   -> Vec<u8>      { vec![] }
    fn deserialize(&mut self, _: &[u8])   {}
    // type_tag() per §4.2: "loop_break"
}
```

`LoopBreakNode` has no parameters. `serialize()` returns empty — the feedback
buffer is transient and regenerates within one cycle after load.

-----

## 3.4 `LoopBackEdge` and `NodeExecutor` Changes (`paraclete-runtime`)

```rust
// In paraclete-runtime:

/// Describes a feedback back edge recorded at connection time.
struct LoopBackEdge {
    /// Node ID of the LoopBreakNode.
    lb_id:       u32,
    /// Output port ID on the LoopBreakNode (always 1 — cv_out).
    lb_out_port: u16,
    /// Node ID that reads from the LoopBreakNode's output.
    dst_id:      u32,
    /// Input port ID on the destination node.
    dst_in_port: u16,
}
```

`NodeExecutor` gains field:

```rust
back_edges: Vec<LoopBackEdge>,   // populated at build/rebuild time
```

-----

## 3.5 Connection-Time Cycle Detection

`NodeConfigurator::connect()` amends cycle detection:

1. When petgraph detects a cycle on the proposed edge, collect the node IDs
   in the cycle.
2. Count nodes in the cycle where `node.is_loop_break() == true`.
3. **Count == 1:** accept. Record the back edge (the `lb_id → dst_id` edge).
   Remove this edge from the sort graph (so topological sort treats
   `LoopBreakNode` as a sink in the cycle). Store a `LoopBackEdge` in the
   configurator for propagation to the executor at build time.
4. **Count == 0:** return `CycleError::MissingLoopBreak { cycle }`.
5. **Count ≥ 2:** return `CycleError::TooManyLoopBreaks { cycle, count }`.

`AudioBuffer` cycles remain unconditionally rejected (count == 0 path applies;
`LoopBreakNode` is CvSignal-only).

-----

## 3.6 Executor Execution Phases

`NodeExecutor::process_cycle()` gains three phases:

**Pre-execution phase** (before any `process()` call):
```
for each back_edge in self.back_edges:
    slice = self.nodes[back_edge.lb_id].loop_break_prev()
    write slice → port buffer at (lb_id, lb_out_port)
    propagate to dst_id's input slot at dst_in_port
```
This makes the previous cycle's LoopBreakNode output available to the
downstream node before execution begins.

**Main execution phase:** unchanged topological order. `LoopBreakNode` runs
as a sink — last in its cycle (back edge removed from sort graph places it
at the end). When `process()` runs: captures input into `next`, writes `prev`
to output (idempotent).

**Post-execution phase** (after all `process()` calls):
```
for each back_edge in self.back_edges:
    self.nodes[back_edge.lb_id].loop_break_swap()
```
Advances the stored buffer: `prev` ← `next`, ready for the next cycle.

-----

## 3.7 Tests (Commit 3)

```
loop_break_node_initial_output_zero
    Construct LoopBreakNode. activate(44100, 4).
    Call process() with non-zero input.
    Assert output in the FIRST call is all zeros (prev starts zeroed).

loop_break_node_output_lags_one_cycle
    LoopBreakNode, block_size=4.
    Cycle 1: input=[1,1,1,1]. Assert output=[0,0,0,0].
    loop_break_swap(). Cycle 2: input=[2,2,2,2]. Assert output=[1,1,1,1].
    loop_break_swap(). Cycle 3: input=[3,3,3,3]. Assert output=[2,2,2,2].

loop_break_node_is_loop_break_true
    LoopBreakNode::new().is_loop_break() == true.

non_loop_break_node_is_loop_break_false
    FilterNode::new().is_loop_break() == false.

connect_cycle_no_loop_break_returns_error
    Build NodeConfigurator with FilterNode A → FilterNode B.
    connect(B, out, A, in) without a LoopBreakNode.
    Assert returns Err(CycleError::MissingLoopBreak { .. }).

connect_cycle_one_loop_break_accepted
    Build NodeConfigurator: FilterNode A → LoopBreakNode L → FilterNode A.
    (A output → L input; L output → A input)
    Assert connect() returns Ok(()).
    Assert back_edges contains one LoopBackEdge for L.

connect_cycle_two_loop_breaks_rejected
    Build configurator: A → L1 → L2 → A (two loop break nodes in cycle).
    Assert connect() returns Err(CycleError::TooManyLoopBreaks { count: 2, .. }).

loop_break_executor_pre_phase_injects_prev
    Build executor with a sanctioned feedback cycle.
    Run two process_cycle() calls.
    In cycle 2, verify the downstream node received the cycle-1 output
    from LoopBreakNode (not cycle-2 — confirms pre-phase is working).
```

Commit 3 adds **+8 tests** → 365 total.

-----

# Part 4: Dynamic Topology (Commit 4)

**Crates:** `paraclete-runtime` (GPL3), `paraclete-app` (GPL3)

Implements ADR-029. Prerequisite: Commit 2 (cap-doc cache) must be merged.

-----

## 4.1 `NodeRegistry` (`paraclete-app`)

```rust
// In paraclete-app:

pub struct NodeRegistry {
    constructors: HashMap<String, Arc<dyn Fn() -> Box<dyn Node> + Send + Sync>>,
}

impl NodeRegistry {
    pub fn new() -> Self;

    /// Register a zero-argument constructor for a type_tag.
    pub fn register(
        &mut self,
        type_tag:  impl Into<String>,
        ctor:      impl Fn() -> Box<dyn Node> + Send + Sync + 'static,
    );

    /// Construct a node from its type_tag.
    /// Returns None if the type_tag is not registered.
    pub fn build(&self, type_tag: &str) -> Option<Box<dyn Node>>;

    /// All registered type_tags. Used by tooling and TUI.
    pub fn known_type_tags(&self) -> Vec<&str>;
}

/// Build the standard NodeRegistry with all first-party node types.
pub fn build_registry() -> NodeRegistry {
    let mut r = NodeRegistry::new();
    r.register("internal_clock",      || Box::new(InternalClock::new(120.0)));
    r.register("sequencer",           || Box::new(Sequencer::new()));
    r.register("sampler",             || Box::new(Sampler::new()));
    r.register("loop_break",          || Box::new(LoopBreakNode::new()));
    r.register("distortion",          || Box::new(DistortionNode::new()));
    r.register("filter",              || Box::new(FilterNode::new()));
    r.register("ladder_filter",       || Box::new(LadderFilterNode::new()));
    r.register("oscillator",          || Box::new(OscillatorNode::new()));
    r.register("envelope",            || Box::new(EnvelopeNode::new()));
    r.register("lfo",                 || Box::new(LfoNode::new()));
    r.register("reverb",              || Box::new(ReverbNode::new()));
    r.register("delay",               || Box::new(DelayNode::new()));
    r.register("mix",                 || Box::new(MixNode::new()));
    r.register("split",               || Box::new(SplitNode::new()));
    r.register("audio_output",        || Box::new(AudioOutputNode::new()));
    r.register("analog_engine:kick",  || Box::new(AnalogEngine::kick()));
    r.register("analog_engine:snare", || Box::new(AnalogEngine::snare()));
    r.register("analog_engine:hihat", || Box::new(AnalogEngine::hihat()));
    r.register("fm_engine:kick",      || Box::new(FmEngine::kick()));
    r.register("fm_engine:bell",      || Box::new(FmEngine::bell()));
    r.register("fm_engine:bass",      || Box::new(FmEngine::bass()));
    r
    // clap_plugin nodes: constructed via PluginLibrary; not registered here.
}
```

`clap_plugin` nodes are excluded because their construction requires a
`PluginLibrary` argument. They are inserted into `NodeConfigurator` directly
by `paraclete-clap-host`.

-----

## 4.2 Type Tag Storage in `NodeConfigurator` (`paraclete-runtime`)

To save project files in v2 format, the runtime must know each node's
type_tag. No `type_tag()` method is added to the `Node` trait (ADR-029:
no change to `paraclete-node-api`). Instead, `NodeConfigurator` tracks it.

```rust
// In NodeConfigurator:

type_tag_map: HashMap<u32, String>,   // node_id → type_tag
```

New method:

```rust
impl NodeConfigurator {
    /// Add a node with an explicit type_tag (used by apply_patch and
    /// build_from_instrument). Stores the tag for project file serialization.
    pub fn add_node_tagged(
        &mut self,
        node:     Box<dyn Node>,
        type_tag: impl Into<String>,
    ) -> u32;

    /// Returns the stored type_tag for a node, if known.
    pub fn type_tag_for(&self, node_id: u32) -> Option<&str>;
}
```

`add_node()` (the existing untagged form) remains for internal or legacy use;
`add_node_tagged()` is preferred for all new call sites.

`build_from_instrument()` in `paraclete-app` is updated to call
`add_node_tagged()` so that topology built from the instrument definition
is serializable from the first session.

-----

## 4.3 `TopologyChange` and `PatchError` (`paraclete-app`)

```rust
// In paraclete-app:

pub enum TopologyChange {
    AddNode {
        type_tag:       String,
        /// Applied via set_initial_params() after construction.
        initial_params: HashMap<String, f64>,
    },
    RemoveNode {
        id: u32,
    },
    AddEdge {
        src:      u32, src_port: u16,
        dst:      u32, dst_port: u16,
    },
    RemoveEdge {
        src:      u32, src_port: u16,
        dst:      u32, dst_port: u16,
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

-----

## 4.4 `apply_patch()` (`paraclete-app`)

```rust
// In paraclete-app:

/// Apply a batch of topology changes atomically.
///
/// Pauses the audio engine (one buffer of silence), applies all changes to
/// the NodeConfigurator, rebuilds the executor, and resumes audio.
///
/// Returns one NodeId per AddNode change in the order the AddNode variants
/// appear in `changes`.
///
/// If a change fails, the patch aborts at that step. Prior changes in the
/// batch are NOT rolled back. The caller must apply corrective changes.
pub fn apply_patch(
    changes:  Vec<TopologyChange>,
    engine:   &mut AudioEngine,
    conf:     &mut NodeConfigurator,
    registry: &NodeRegistry,
) -> Result<Vec<u32>, PatchError>;
```

**Protocol:**

```
1. engine.pause()
2. engine.wait_paused()
3. for each change:
     AddNode { type_tag, initial_params }:
         node = registry.build(&type_tag)  → PatchError::UnknownTypeTag if None
         node.set_initial_params(&initial_params)
         id = conf.add_node_tagged(node, &type_tag)
         new_ids.push(id)
     RemoveNode { id }:
         conf.remove_node(id)?              → PatchError::NodeNotFound
     AddEdge { src, src_port, dst, dst_port }:
         conf.connect(src, src_port, dst, dst_port)?   → PatchError::CycleError
     RemoveEdge { src, src_port, dst, dst_port }:
         conf.disconnect(src, src_port, dst, dst_port)?
4. executor = conf.rebuild_executor()
5. engine.resume_with_executor(executor)
6. return Ok(new_ids)
```

-----

## 4.5 `AudioEngine` Additions (`paraclete-runtime`)

```rust
// In paraclete-runtime, AudioEngine:

/// Signal the audio thread to stop after the current buffer.
/// Non-blocking. Pair with wait_paused().
pub fn pause(&self);

/// Block until the audio thread confirms it has paused.
/// Timeout: 500 ms — panics if exceeded (audio thread hung = fatal).
pub fn wait_paused(&self);

/// Swap in a new executor and resume audio processing.
/// Must only be called after wait_paused() has returned.
pub fn resume_with_executor(&self, executor: NodeExecutor);
```

**Implementation:** atomic `pause_requested: AtomicBool` polled by the audio
callback at the start of each buffer. When set: write completed buffer, set
`is_paused: AtomicBool = true`, return without processing. `wait_paused()`
spins on `is_paused` with a 500 ms deadline. `resume_with_executor()` swaps
the executor reference (via a `Mutex<Option<NodeExecutor>>` or equivalent
lock-free cell), clears both atomics, resumes.

-----

## 4.6 `NodeConfigurator` Additions (`paraclete-runtime`)

```rust
// In paraclete-runtime, NodeConfigurator:

/// Remove a node and sever all edges connected to it.
/// Returns the removed node for final inspection.
pub fn remove_node(&mut self, id: u32) -> Result<Box<dyn Node>, ConfigError>;

/// Disconnect a specific edge. No-op if the edge does not exist.
pub fn disconnect(
    &mut self,
    src: u32, src_port: u16,
    dst: u32, dst_port: u16,
) -> Result<(), ConfigError>;

/// Build a NodeExecutor from current state. Non-consuming — the
/// configurator retains ownership of nodes for subsequent patch cycles.
/// Semantically equivalent to build_executor() but callable multiple times.
pub fn rebuild_executor(&mut self) -> NodeExecutor;
```

`rebuild_executor()` uses Arc-shared or cloned node references as
appropriate. The existing `build_executor()` method is retained for the
startup path. Implementation may delegate: `build_executor()` calls
`rebuild_executor()` internally.

-----

## 4.7 Project File Format v2 (`paraclete-app`)

`NodeSnapshot` gains `type_tag`:

```rust
#[derive(Serialize, Deserialize)]
struct NodeSnapshot {
    id:        u32,
    type_tag:  String,   // NEW in v2 — construction key
    type_name: String,   // retained — human label only
    state:     Vec<u8>,  // Node::serialize() output
}
```

**v2 save:** `save_project()` writes `version: 2`. For each node in the
configurator, `type_tag_for(id)` is called to populate `type_tag`.

**v2 load:**
```
1. For each NodeSnapshot:
     node = registry.build(&snapshot.type_tag)?
     conf.add_node_tagged(node, &snapshot.type_tag)  with id = snapshot.id
     node.deserialize(&snapshot.state)
2. For each Edge:
     conf.connect(edge.src_node, edge.src_port, edge.dst_node, edge.dst_port)?
3. conf.build_executor()
```

**v1 load:** topology built from application code (fixed-construction-sequence
path). Emits warning: `"Project v1 does not contain topology data. Node states
applied to current graph; topology unchanged."` Load otherwise succeeds.

-----

## 4.8 Tests (Commit 4)

```
node_registry_build_known_type_tag
    build_registry(). registry.build("filter") returns Some.
    Assert the node is a FilterNode (via type_name()).

node_registry_build_unknown_type_tag
    build_registry(). registry.build("nonexistent") returns None.

node_registry_known_type_tags_contains_loop_break
    build_registry().known_type_tags() contains "loop_break".

apply_patch_add_node_returns_id
    Construct a NodeConfigurator with an AudioEngine in paused state.
    apply_patch([AddNode { type_tag: "distortion", initial_params: {} }], ...).
    Assert Ok(ids) with ids.len() == 1. Assert id > 0.

apply_patch_unknown_type_tag_returns_error
    apply_patch([AddNode { type_tag: "nonexistent_node", .. }], ...).
    Assert Err(PatchError::UnknownTypeTag(_)).

apply_patch_add_edge_cycle_without_loop_break_returns_error
    Build a two-node graph (A → B). apply_patch([AddEdge(B.out → A.in)]).
    Assert Err(PatchError::CycleError(CycleError::MissingLoopBreak { .. })).

configurator_remove_node_severs_edges
    Add nodes A, B; connect(A, 0, B, 0).
    remove_node(A). Assert edges referencing A are gone.
    Assert A's node ID is no longer in the configurator.

project_v2_save_load_roundtrip
    Build a 3-node graph (clock → sequencer → audio_out).
    save_project_v2(path).
    Load into fresh NodeConfigurator via load_project_v2(path, &mut conf, &registry).
    Assert all three node IDs are present. Assert edges are restored.

project_v1_load_emits_warning
    load_project(v1_ron_bytes, &mut conf, registry).
    Assert Ok(warnings); assert warnings is non-empty and contains "v1".
```

Commit 4 adds **+9 tests** → 374 total.

-----

# Part 5: Sequencer as CV Source (Commit 5)

**Crate:** `paraclete-nodes` (GPL3)

The Sequencer gains CV output ports. Each step can lock CV values on each
output port independently. The Sequencer samples the active step's CV locks
once per step fire and holds the value until the next step fires (sample-and-hold).

-----

## 5.1 `Sequencer::with_cv_outputs()`

New constructor:

```rust
impl Sequencer {
    /// Construct a Sequencer with n CV output ports.
    /// n = 0 is valid; behaves identically to Sequencer::new().
    pub fn with_cv_outputs(n: usize) -> Self;
}
```

Internal additions:

```rust
struct Sequencer {
    // ... existing fields ...
    cv_outputs:  usize,       // number of CV output ports
    current_cv:  Vec<f32>,    // current held value per CV output; len == cv_outputs
}
```

`activate()` initialises `current_cv` to `vec![0.0_f32; cv_outputs]`.

-----

## 5.2 CV Output Ports

CV output ports are appended after the existing ports. Port IDs:

| Port ID | Name | Kind | Direction | Condition |
|---------|------|------|-----------|-----------|
| 0 | `clock_in` | _(existing)_ | Input | always |
| 1 | `events_out` | _(existing)_ | Output | always |
| `2 + i` | `cv_out_{i}` | CvSignal | Output | `i < cv_outputs` |

`ports()` returns existing ports followed by one `CvSignal / Output` entry
per CV output. Port IDs are stable: `cv_out_0` is always port 2, `cv_out_1`
is always port 3, etc. Adding CV outputs does not renumber existing ports.

-----

## 5.3 `Step::cv_locks`

```rust
pub struct Step {
    // ... existing fields ...
    /// CV value locks per CV output port.
    /// Each entry is (cv_port_index, value).
    /// cv_port_index is 0-relative (cv_out_0 = index 0).
    /// At most one lock per port index; last write wins if duplicated.
    pub cv_locks: Vec<(u16, f32)>,
}
```

Default: `cv_locks: vec![]` (no locks — CV outputs hold their previous
value, initially 0.0).

-----

## 5.4 `process()` Changes

On step fire (the tick when a step triggers):
1. For each `(idx, val)` in `step.cv_locks`:
   - If `idx < self.cv_outputs`: `self.current_cv[idx] = val`.
   - Out-of-range indices are silently ignored.

Each buffer cycle (regardless of step fire):
2. For each CV output port `i` in `0..self.cv_outputs`:
   - Fill output buffer for port `2 + i` with `current_cv[i]` repeated
     `block_size` times (constant block, sample-and-hold between steps).

This is the ADR-009 extension: the Sequencer becomes a CV source that the
clock federation can route to modulation inputs without an intermediary
profile script.

-----

## 5.5 `NodeRegistry` Update

`build_registry()` in `paraclete-app` is updated to register
`"sequencer_cv"` — a convenience constructor for a Sequencer with one CV
output:

```rust
r.register("sequencer_cv", || Box::new(Sequencer::with_cv_outputs(1)));
```

`"sequencer"` remains registered as `Sequencer::new()` (zero CV outputs) for
backward compatibility.

-----

## 5.6 Tests (Commit 5)

```
sequencer_cv_output_ports_present
    Sequencer::with_cv_outputs(2).ports() contains ports with names
    "cv_out_0" and "cv_out_1", both CvSignal Output.

sequencer_no_cv_no_extra_ports
    Sequencer::new().ports() contains no CvSignal Output ports.

sequencer_cv_output_initial_zero
    with_cv_outputs(1), activate(44100, 256). Run one process cycle.
    CV output buffer is all zeros (no step fired, initial hold value).

sequencer_cv_lock_updates_on_step_fire
    with_cv_outputs(1), step 0 has cv_locks = [(0, 0.75)].
    Run process cycles until step 0 fires.
    Assert cv_out_0 buffer holds 0.75 (within f32 epsilon).

sequencer_cv_lock_sample_and_hold
    Step 0 locks cv_out_0 = 0.5. Step 1 has no cv_locks.
    After step 0 fires: cv_out_0 = 0.5.
    After step 1 fires: cv_out_0 still == 0.5 (held from step 0).

sequencer_cv_lock_out_of_range_ignored
    with_cv_outputs(1). Step has cv_locks = [(5, 9.9)] (idx out of range).
    Run process. Assert no panic. cv_out_0 remains 0.0.

sequencer_cv_step_lock_serialization_roundtrip
    Create Sequencer with cv_locks on several steps.
    serialize() → deserialize() roundtrip on a fresh instance.
    Assert cv_locks on steps are restored correctly.
```

Commit 5 adds **+7 tests** → 381 total.

-----

# Part 6: GraphNode and Nested Sequencers (Commit 6)

**Crates:** `paraclete-node-api` (LGPL3), `paraclete-nodes` (GPL3),
`paraclete-graph-nodes` (GPL3, **new**)

-----

## 6.1 New Crate: `paraclete-graph-nodes`

`paraclete-graph-nodes` is a new GPL3 crate. It is the only location where
a `Node` implementation is permitted to own a `NodeExecutor`. This is
necessary because `paraclete-nodes` cannot depend on `paraclete-runtime`
(ADR-022 portability rule).

```toml
# paraclete-graph-nodes/Cargo.toml
[package]
name    = "paraclete-graph-nodes"
version = "0.1.0"
license = "GPL-3.0"

[dependencies]
paraclete-node-api = { path = "../paraclete-node-api" }
paraclete-nodes    = { path = "../paraclete-nodes" }
paraclete-runtime  = { path = "../paraclete-runtime" }
```

-----

## 6.2 `GraphNode` Marker Trait (`paraclete-node-api`)

A marker trait identifying nodes that contain an inner graph. No additional
methods beyond the `Node` supertrait. Provides a semantic tag for
introspection and future serialization tooling.

```rust
// In paraclete-node-api:

/// Marker trait for nodes that contain an inner node graph.
///
/// Implementations must also implement Node. The inner graph is not
/// directly accessible from outside the node — it is encapsulated.
/// See ADR-023 for the design rationale.
pub trait GraphNode: Node {}
```

-----

## 6.3 `InnerGraphNode` (`paraclete-graph-nodes`)

`InnerGraphNode` is a concrete `Node` implementation that owns a fully
wired inner graph: an inner `Sequencer` driven by clock, an inner synthesis
engine driven by the inner sequencer, and an inner audio output whose buffer
is exposed as the node's audio output port.

```rust
// In paraclete-graph-nodes:

pub struct InnerGraphNode {
    executor:      NodeExecutor,
    /// IDs of nodes inside the inner graph (for clock/cv routing).
    inner_seq_id:  u32,
    inner_cv_in_port: u16,    // port on inner Sequencer that receives outer CV
    /// Current CV value received from the outer graph.
    cv_in:         Vec<f32>,
    block_size:    usize,
}

impl InnerGraphNode {
    /// Construct an InnerGraphNode with the default inner graph:
    ///   InternalClock stub → Sequencer (with 1 CV input) → AnalogEngine::kick()
    ///   → MixNode → AudioOutputNode
    ///
    /// The inner Sequencer is driven by the outer clock (clock_in port),
    /// not by an inner clock. The inner AnalogEngine variant is kick by default.
    pub fn new() -> Self;
}

impl Node for InnerGraphNode {
    fn activate(&mut self, sample_rate: f32, block_size: usize);
    fn deactivate(&mut self);
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput);

    fn ports(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor { id: 0, name: "clock_in", kind: PortType::MidiMessage, direction: Direction::Input  },
            PortDescriptor { id: 1, name: "cv_in_0",  kind: PortType::CvSignal,    direction: Direction::Input  },
            PortDescriptor { id: 2, name: "audio_out",kind: PortType::AudioBuffer, direction: Direction::Output },
        ]
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument { parameters: vec![], ports: self.ports() }
    }

    fn type_name(&self)   -> &'static str { "InnerGraphNode" }
    fn serialize(&self)   -> Vec<u8>      { vec![] }  // inner state not persisted at P9
    fn deserialize(&mut self, _: &[u8])   {}
}

impl GraphNode for InnerGraphNode {}
```

**`activate()`:** calls `executor.activate(sample_rate, block_size)` which
activates all inner nodes. Initialises `cv_in` buffer.

**`process()`:**
1. Read MIDI/clock events from input port 0 (`clock_in`).
2. Forward clock events to the inner Sequencer's clock input.
3. Read CV from input port 1 (`cv_in_0`); store in `self.cv_in`.
4. Apply `cv_in` to the inner Sequencer's CV parameter (maps to step pitch
   or a declared CV input port on the inner Sequencer with 1 CV input —
   built using `Sequencer::with_cv_outputs(1)` configured as a receiver).
5. Run `self.executor.process_cycle()`.
6. Copy the inner AudioOutput node's output buffer to the `audio_out` port.

**Inner graph wiring (constructed in `new()`):**
```
clock_in (outer port 0) → inner_seq.clock_in (port 0)
cv_in_0  (outer port 1) → inner_seq.cv_in_0  (port 2, if Sequencer has 1 CV in)
inner_seq.events_out    → inner_kick.events_in
inner_kick.audio_out_l  → inner_mix.channel_0_in
inner_mix.audio_out     → inner_audio_out.audio_in
```

The inner Sequencer is `Sequencer::with_cv_outputs(1)`. The CV output of
the inner Sequencer feeds back to its own parameter (step CV → inner engine
tune). This requires a `LoopBreakNode` in the inner graph if the feedback
creates a cycle; the inner graph executor observes the same ADR-028 rules.

**Nested sequencer connection to outer graph:**

The intended wiring at the outer level:
```
outer_sequencer.cv_out_0  → inner_graph_node.cv_in_0
outer_clock.clock_out     → inner_graph_node.clock_in
inner_graph_node.audio_out → mix.channel_in
```

This makes each outer step's CV lock value drive the inner Sequencer,
enabling per-cell patterns that are modulated by the outer pattern.

-----

## 6.4 `NodeRegistry` Update

`build_registry()` in `paraclete-app` gains the `InnerGraphNode` registration.
This requires `paraclete-app` to depend on `paraclete-graph-nodes`.

```toml
# paraclete-app/Cargo.toml
[dependencies]
# ... existing ...
paraclete-graph-nodes = { path = "../paraclete-graph-nodes" }
```

```rust
r.register("inner_graph", || Box::new(InnerGraphNode::new()));
```

-----

## 6.5 Tests (Commit 6)

```
inner_graph_node_ports_correct
    InnerGraphNode::new().ports() has exactly 3 entries:
    clock_in (MidiMessage, Input), cv_in_0 (CvSignal, Input),
    audio_out (AudioBuffer, Output).

inner_graph_node_is_graph_node
    fn assert_graph_node<T: GraphNode>(_: &T) {}
    assert_graph_node(&InnerGraphNode::new());  // compiles

inner_graph_node_activate_no_panic
    InnerGraphNode::new().activate(44100.0, 256). No panic.

inner_graph_node_produces_audio_on_note_on
    Construct outer graph: InternalClock → InnerGraphNode → AudioOutputNode.
    Inject NoteOn into InnerGraphNode's inner Sequencer directly via its
    inner executor handle.
    Run 4 process cycles.
    Assert InnerGraphNode's audio_out port contains at least one non-zero sample.

graph_node_marker_trait_implemented
    InnerGraphNode: GraphNode is not object-safe, but compilation is the test.
    Confirm InnerGraphNode implements both Node and GraphNode.

registry_contains_inner_graph
    build_registry().build("inner_graph") returns Some.

inner_graph_node_deactivate_no_panic
    new().activate(44100.0, 256); deactivate(). No panic.

inner_graph_node_serialize_returns_empty
    new().serialize() == vec![] (inner state not persisted at P9).
```

Commit 6 adds **+8 tests** → 389 total.

-----

# P9 Done Criteria

All criteria must pass before P9 is marked complete.

## Functionality

- TUI encoder row displays live parameter values (not `0.0`) for all nodes
  with a `ParameterBank` — confirmed by running the instrument and observing
  the encoder row respond to `CMD_BUMP_PARAM` on any track
- `apply_patch([AddNode { type_tag: "filter", .. }, AddEdge { .. }])` succeeds
  on a running instrument without audio dropout longer than ~5 ms
- `apply_patch()` on a cycle without a `LoopBreakNode` returns
  `Err(PatchError::CycleError(CycleError::MissingLoopBreak { .. }))`
- A `LoopBreakNode` in a CvSignal feedback path produces correct one-cycle-
  delayed output — verified by the `loop_break_node_output_lags_one_cycle` test
- `save_project()` writes `version: 2` RON with `type_tag` populated for
  all nodes
- `load_project_v2()` on a saved v2 file restores topology (nodes and edges)
  without error
- Loading a v1 file on a running P9 binary emits a warning and applies node
  state to the current graph without changing topology
- `Sequencer::with_cv_outputs(2)` produces two CvSignal output ports
- A Sequencer step with `cv_locks = [(0, 0.5)]` causes `cv_out_0` to hold
  `0.5` from that step's fire until the next step fires
- `InnerGraphNode` processes audio — its `audio_out` is non-silent when its
  inner Sequencer fires a step

## Architecture

- `cargo tree -p paraclete-nodes` shows no dependency on
  `paraclete-runtime`, `paraclete-scripting`, or any L0 crate (portability
  rule unchanged through P9)
- `cargo tree -p paraclete-node-api` shows no dependency on
  `paraclete-nodes` or `paraclete-runtime` (LGPL3 boundary clean)
- `paraclete-graph-nodes/Cargo.toml` license field is `GPL-3.0`
- `cargo tree -p paraclete-graph-nodes` shows a dependency on both
  `paraclete-nodes` and `paraclete-runtime` (this is expected and correct)
- All three new `Node` trait methods from Commit 1 and Commit 3 have default
  implementations — `cargo publish --dry-run -p paraclete-node-api` exits 0
- `capability_document()` is no longer called more than once per node per
  session in any code path (cache confirmed via grep or structural test)

## Tests

| State | Tests |
|-------|-------|
| P8 ship | 340 |
| Commit 1 (publish_bank_state, set_node_id) | +8 → 348 |
| Commit 2 (housekeeping) | +9 → 357 |
| Commit 3 (loop break) | +8 → 365 |
| Commit 4 (dynamic topology) | +9 → 374 |
| Commit 5 (sequencer CV) | +7 → 381 |
| Commit 6 (GraphNode) | +8 → 389 |
| **P9 target** | **≥ 385** |

All 340 existing tests continue to pass unchanged through every commit.

`plugin_node_effect_has_audio_in_port`, `plugin_node_generator_no_audio_in_port`,
and any CLAP-binary-dependent tests are tagged `#[ignore]` and are not counted
in the 389 total. They run as a separate CI step.

-----

# P9 Known Gaps and Deferred Items

**AudioBuffer feedback (deferred to P10+):** `LoopBreakNode` operates on
`CvSignal` ports only. Audio-rate feedback cycles (e.g. Karplus-Strong
with `LoopBreakNode` on an `AudioBuffer` path) continue to be rejected by
cycle detection. Audio-rate feedback requires oversampling to avoid aliasing.
Deferred pending OQ-7 (oversampling strategy) resolution.

**Inner GraphNode topology not patchable (deferred):** `apply_patch()` acts
on the outer `NodeConfigurator` only. Patching the inner graph of an
`InnerGraphNode` at runtime is not supported at P9. The inner graph is rebuilt
as a unit when the outer graph is patched (e.g. when `RemoveNode` / `AddNode`
replaces an `InnerGraphNode`). Dynamic inner-graph patching is a P10 concern.

**CLAP plugin nodes not in NodeRegistry (by design — review at P10):** `clap_plugin`
nodes require a `PluginLibrary` argument and are not registered in the standard
`NodeRegistry`. `apply_patch()` cannot add CLAP plugin nodes via `AddNode`. The
caller must add them directly via `NodeConfigurator`. A `PluginLibrary`-aware
patch path may be introduced at P10.

**`InnerGraphNode::serialize()` returns empty (deferred):** The inner graph's
node states are not persisted to the project file at P9. Loading a project file
that previously had an `InnerGraphNode` starts the inner graph fresh (default
parameters, empty patterns). Full inner graph serialization is a P10 concern
and requires a recursive snapshot format (nested `NodeSnapshot` arrays).

**Undo/redo (deferred):** `apply_patch()` operations are permanent. There is
no undo stack. Undo/redo is a future concern.

**UI for patch operations (deferred):** No TUI node-list display or connection
visualiser at P9. Topology changes are API-only. A visual patcher is a future
UI milestone.

**`Sequencer::serialize()` P5 fields (still deferred):** `TrigCondition` and
`StepTiming` are not serialised. Pre-existing limitation noted at P5–P8.
`cv_locks` introduced at P9 are serialised (§5.3 roundtrip test confirms).

**Signed micro-timing (still deferred):** Negative `micro_offset` deferred
from P5 through P8; still deferred.

**CLAP GUI extension (still deferred):** No GUI surface for plugin nodes.
Deferred per P7 and P8 known gaps.

**CLAP note expressions (still deferred):** `NoteExpression` events not
translated. Deferred per P8 known gaps.

**Hot plugin reload (still deferred):** `PluginLibrary` loaded at startup.
Deferred per P8 known gaps.

**Multi-channel audio output from plugins (still deferred):** `PluginNode`
mixes output channels to mono. Deferred per P8 known gaps.

**`SubgraphPlugin` CLAP machine slot swapping (still deferred):** Depends
on `GraphNode` (now P9). Machine slot swapping for the CLAP wrapper is
deferred to P10 when the full `GraphNode` API is stable.

**LFO tempo sync (still deferred):** `LfoNode` runs free. Beat-synced LFO
rates require `TransportInfo` subscription. Deferred.

**Ladder filter HP/BP modes (still deferred):** Deferred.
