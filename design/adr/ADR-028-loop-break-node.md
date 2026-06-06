# ADR-028: Single-Sample Feedback Loop Break Node

**Date:** June 2026
**Status:** Accepted — P9 implementation target (resolves OQ-5)

---

## Decision

Sanctioned feedback loops in the node graph are created by placing a `LoopBreakNode`
in the cycle. `LoopBreakNode` is a first-class `CvSignal`-only node that holds a
block of samples from the previous buffer cycle and presents them as output for the
current cycle, introducing exactly one block of latency in the feedback path.

The executor detects cycles containing exactly one `LoopBreakNode` and accepts them,
handling execution via a pre-populate / normal-order / post-swap protocol. All other
cycles (zero `LoopBreakNode` instances, or two or more) continue to be rejected at
connection time with a descriptive error.

---

## Context

ADR-005 committed to petgraph from P0 and deferred sanctioned feedback to P9:
*"At P9, explicit loop-break nodes are introduced. When a loop-break node is present
in a cycle, the scheduler accepts the cycle and handles it via the loop-break's
single-sample delay semantics."* OQ-5 restated the requirement: *"Single-sample
feedback loop break model. Explicit loop-break nodes preferred. ADR needed at P9."*

Feedback loops are foundational to modular synthesis. Representative cases:
- **Filter FM:** a signal fed back through a filter whose cutoff is modulated by the
  filter's own output (self-resonance, comb filter effects)
- **Envelope follower:** amplitude envelope tracking the level of the signal it is
  applied to
- **Karplus-Strong:** recirculating comb-filter feedback through a pitch-tuned delay
- **Phase-locked oscillator:** pitch CV generated from the output of the same oscillator

Without sanctioned feedback, any of these connections causes `connect()` to return a
`CycleError` and refuse the connection. `LoopBreakNode` is the designated escape hatch.

---

## Why Explicit Loop-Break, Not Automatic Latency Compensation

Two approaches were available:

**Option A — Explicit loop-break node.** The programmer places a `LoopBreakNode` in
the feedback path. The executor sees the node, accepts the cycle, and delays the
feedback signal by exactly one buffer block. The loop-break node is visible in the
graph; its latency is known and declared.

**Option B — Automatic latency compensation (ALC).** The scheduler detects cycles,
inserts implicit single-sample delay buffers at cut points, and compensates downstream
latencies. Used in some DAW plugin routing systems.

Option A is chosen. Reasons:

1. **Deterministic:** the delay is exactly one buffer block, always. No hidden
   implicit delays.
2. **Composable and inspectable:** `LoopBreakNode` is an ordinary node. It appears in
   `capability_document()`, serialises via `Node::serialize()`, is introspectable by
   tooling, and participates in the standard graph topology.
3. **Debuggable:** a feedback path without a `LoopBreakNode` is an explicit error, not
   silent wrong behaviour. The error message names the cycle; the fix is obvious.
4. **Consistent with the instrument mental model:** in hardware modular synthesis, the
   patch cable is always explicit. There is no implicit buffering. `LoopBreakNode` is
   the digital equivalent of the physical feedback cable path.
5. **No fragility:** ALC requires full cycle analysis, buffer management, and
   compensation bookkeeping that breaks under complex topologies or when nodes
   change their reported latency. There is no latency-reporting protocol in Paraclete;
   introducing ALC would require one.

---

## `LoopBreakNode` Specification

`LoopBreakNode` is defined in `paraclete-nodes` (GPL3). It has exactly two ports:

| ID | Name     | Kind       | Direction |
|----|----------|------------|-----------|
| 0  | `cv_in`  | CvSignal   | Input     |
| 1  | `cv_out` | CvSignal   | Output    |

**Semantics:** at the start of each buffer cycle, `LoopBreakNode` outputs the
`CvSignal` block received as input during the previous buffer cycle. On the first
cycle after `activate()`, output is all zeros.

**Internal state:**

```rust
pub struct LoopBreakNode {
    /// Value presented as output this cycle (captured from input last cycle).
    prev: Vec<f32>,   // capacity = block_size
    /// Staging area: this cycle's input, becomes next cycle's output after swap.
    next: Vec<f32>,   // capacity = block_size
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
        // Capture this cycle's input into `next` (will become `prev` after swap).
        let cv_in = input.modulation(0, self.block_size);
        self.next.copy_from_slice(cv_in);
        // Write `prev` (previous cycle's value) to output.
        // This is idempotent with the executor's pre-phase write, but makes
        // LoopBreakNode correct even in contexts where pre-phase is not applied.
        let cv_out = output.mod_output_mut(0, self.block_size);
        cv_out.copy_from_slice(&self.prev);
    }

    fn is_loop_break(&self) -> bool { true }

    fn loop_break_prev(&self) -> &[f32] { &self.prev }

    fn loop_break_swap(&mut self) {
        std::mem::swap(&mut self.prev, &mut self.next);
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            parameters: vec![],   // no parameters
            ports:      self.ports(),
        }
    }

    fn ports(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor { id: 0, name: "cv_in",  kind: PortType::CvSignal, direction: Direction::Input  },
            PortDescriptor { id: 1, name: "cv_out", kind: PortType::CvSignal, direction: Direction::Output },
        ]
    }

    fn type_name(&self) -> &'static str { "LoopBreakNode" }

    fn serialize(&self)   -> Vec<u8> { vec![] }
    fn deserialize(&mut self, _: &[u8]) {}
}
```

---

## Node Trait Additions

Three new default-no-op methods are added to the `Node` trait in `paraclete-node-api`.
All have default implementations; the LGPL3 public API is not broken.

```rust
/// Returns true if this node is a loop-break node.
///
/// The executor treats `is_loop_break() == true` nodes specially:
/// their stored-previous-cycle output is written into the graph's port buffers
/// before the main execution loop, and `loop_break_swap()` is called after
/// all nodes have processed.
///
/// Default: `false`. Only `LoopBreakNode` in `paraclete-nodes` overrides this.
fn is_loop_break(&self) -> bool { false }

/// Returns the previous-cycle CvSignal block used as this cycle's output.
///
/// Called by the executor in the pre-execution phase. Must return a slice
/// of length `block_size` (as set in `activate()`). Default: empty slice
/// (safe no-op when `is_loop_break()` is false).
fn loop_break_prev(&self) -> &[f32] { &[] }

/// Swap `prev` ← `next` buffers.
///
/// Called by the executor after all nodes in the execution order have
/// processed. Only meaningful when `is_loop_break()` is true. Default: no-op.
fn loop_break_swap(&mut self) {}
```

---

## Executor Changes

### Connection-time cycle detection

The existing behaviour — any cycle is rejected with `CycleError` — is amended:

1. When a cycle is detected in `connect()`, collect the node IDs in the cycle.
2. Count the number of nodes in the cycle where `node.is_loop_break() == true`.
3. If the count is **exactly 1**: accept the cycle. Record the back edge (the edge from
   `LoopBreakNode`'s output port back to its downstream dependent). Remove this back
   edge from the topological-sort graph.
4. If the count is **0**: return `CycleError::MissingLoopBreak { cycle: Vec<u32> }`.
5. If the count is **≥ 2**: return `CycleError::TooManyLoopBreaks { cycle: Vec<u32>,
   count: usize }`.

The "back edge" is the edge `LoopBreakNode → A` where A is the node in the cycle that
reads from LoopBreakNode's output. Removing it makes the remaining graph acyclic,
allowing a valid topological sort.

### `LoopBackEdge` record

```rust
/// Describes a feedback back edge accepted at connection time.
/// Stored in `NodeExecutor` for use in the pre- and post-cycle phases.
struct LoopBackEdge {
    /// Node ID of the LoopBreakNode.
    lb_id:        u32,
    /// Port ID on LoopBreakNode that carries the feedback output.
    lb_out_port:  u16,
    /// Node ID of the node that reads from LoopBreakNode's output.
    dst_id:       u32,
    /// Port ID on the destination node that receives the feedback.
    dst_in_port:  u16,
}
```

`NodeExecutor` gains field: `back_edges: Vec<LoopBackEdge>`.

### Execution order

With the back edge removed, `LoopBreakNode` has no outgoing edges in the sort graph.
It becomes a sink and is scheduled **last** among the nodes in the feedback cycle.
This is correct: B's output (the current cycle's input to LoopBreak) is ready by the
time LoopBreak runs.

### Pre-execution phase

Before any node's `process()` is called in a given buffer cycle:

```
for each back_edge in executor.back_edges:
    prev_slice = executor.nodes[back_edge.lb_id].loop_break_prev()
    write prev_slice into the port buffer for (lb_id, lb_out_port)
    propagate that buffer to dst_id's input slot for dst_in_port
```

This makes the previous cycle's `LoopBreakNode` output available to node A before A
runs — without changing the topological execution order.

### Main execution phase

Normal topological-order execution. LoopBreakNode runs as a sink (last in its cycle).
When it runs:
- Copies its input port data (B's current output) into `self.next`
- Copies `self.prev` to its output port (idempotent with pre-phase)

### Post-execution phase

After all nodes have processed:

```
for each lb_id in executor.loop_break_nodes:
    executor.nodes[lb_id].loop_break_swap()
```

`loop_break_swap()` exchanges `prev` ↔ `next`, making this cycle's captured input the
next cycle's output.

---

## Constraints

- **CvSignal only.** `LoopBreakNode` operates on `CvSignal` (modulation) ports only.
  `AudioBuffer` feedback is not supported at P9; audio-rate feedback requires
  oversampling to avoid aliasing, and oversampling support is deferred per OQ-7.
  `AudioBuffer` cycles continue to be rejected unconditionally.
- **Exactly one per cycle.** Multiple `LoopBreakNode` instances in a single cycle are
  rejected at connection time. The developer must choose one break point; ambiguity
  about which break determines the execution order is an error.
- **One block of latency.** The delay is exactly one buffer block (e.g. ~5.8 ms at
  44.1 kHz / 256-sample blocks). This is the declared cost of block-rate feedback. It
  is an accurate and honest representation of the signal timing.
- **Not a general-purpose delay node.** `LoopBreakNode` exists to break cycles. It
  does not expose a configurable delay length. A parameterized delay node (e.g.
  `DelayNode`) serves that purpose. Do not use `LoopBreakNode` as a timing element.
- **type_tag:** `"loop_break"` — registered in `NodeRegistry` at P9.

---

## Error Variants

Two new variants added to the cycle-detection error type:

```rust
pub enum CycleError {
    /// A cycle was detected with no LoopBreakNode to sanction it.
    MissingLoopBreak { cycle: Vec<u32> },
    /// A cycle was detected with more than one LoopBreakNode.
    TooManyLoopBreaks { cycle: Vec<u32>, count: usize },
    // ... existing variants unchanged
}
```

---

## Consequences

- `is_loop_break()`, `loop_break_prev()`, and `loop_break_swap()` added to `Node`
  trait in `paraclete-node-api` (default no-ops; non-breaking for v0.1.0)
- `LoopBreakNode` added to `paraclete-nodes`; type_tag `"loop_break"` registered
- `NodeExecutor` gains `back_edges: Vec<LoopBackEdge>` and pre/post-cycle phases
- Cycle detection in `connect()` amended to accept sanctioned cycles
- Filter FM, envelope-follower, and Karplus-Strong synthesis topologies become
  expressible in the Paraclete graph
- `GraphNode` inner graphs (ADR-023) may also contain `LoopBreakNode` instances;
  the same rules apply inside an inner `NodeExecutor`
- `LoopBreakNode::serialize()` returns an empty blob (no persistent state — the stored
  feedback buffer is transient and regenerates within one cycle after load)
- OQ-5 is resolved and closed

---

## References

- ADR-005 — Scheduler Cycles (original deferral; petgraph from P0; loop-break node
  approach preferred)
- ADR-004 — Signal Model (CvSignal port type; mixed-rate typed ports)
- ADR-023 — Instrument Encapsulation (GraphNode inner graphs also subject to this ADR)
- ADR-011 — Executor Model (single-threaded push; topological sort; pre/post phases
  added here are within the push model)
- OQ-5 — Single-sample feedback loop break model (resolved)
- OQ-7 — Oversampling strategy (AudioBuffer feedback deferred pending OQ-7 resolution)
