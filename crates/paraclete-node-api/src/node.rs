use crate::agreement::{ConnectionAgreement, ConnectionRecord};
use crate::capability::CapabilityDocument;
use crate::context::{ProcessInput, ProcessOutput};
use crate::port::PortDescriptor;
use crate::state_bus::StateBusValue;

/// Marker trait for nodes that contain an inner node graph.
///
/// Implementations must also implement [`Node`]. The inner graph is not
/// directly accessible from outside the node — it is encapsulated.
/// See ADR-023 for the design rationale.
pub trait GraphNode: Node {}

/// The universal node contract. Every Paraclete node implements this trait.
///
/// Three implicit engagement levels (see ADR-008):
/// - L1: implement only `ports()` and `process()` — passive signal processor.
/// - L2: also override `capability_document()` and lifecycle hooks — instrument.
/// - L3: also override `negotiate()` and `set_connection_record()` — smart node.
pub trait Node: Send {
    // ── Required ──────────────────────────────────────────────────────────────

    /// Declare this node's ports.
    /// Called once at registration time on the main thread.
    fn ports(&self) -> &[PortDescriptor];

    /// Called once per buffer cycle on the audio thread.
    /// Must never allocate, block, or take a lock.
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput);

    // ── Level 2 — override for instruments ───────────────────────────────────

    /// Return this node's capability document.
    /// Default builds one from `ports()` only.
    /// Override to declare parameters, extensions, and negotiation preferences.
    ///
    /// May be called at any time on the main thread.
    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument::from_ports(self.ports())
    }

    /// Return `true` if `capability_document()` would return a different value
    /// from the last time the runtime queried it.
    ///
    /// The runtime polls this at safe reconfiguration points (main thread only).
    /// When `true` the runtime re-queries, updates the state bus, and triggers
    /// renegotiation on affected connections.
    fn capabilities_changed(&self) -> bool {
        false
    }

    /// Called when the audio engine starts or the sample rate / block size
    /// changes. Use to allocate and initialise DSP state that depends on
    /// `sample_rate` or `block_size` (delay lines, filter coefficients, etc.).
    ///
    /// Called on the main thread before any `process()` calls.
    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {}

    /// Called when the audio engine stops.
    /// Use to release resources or flush state.
    /// Called on the main thread after the last `process()` call.
    fn deactivate(&mut self) {}

    /// Hand over private state for project save.
    /// Returns an opaque byte blob. Format is node-defined.
    fn serialize(&self) -> Vec<u8> {
        vec![]
    }

    /// Receive back private state from a project load.
    /// `data` is the blob previously returned by `serialize()`.
    ///
    /// Called on the main thread after `activate()`. Implementations that use
    /// `ParameterBank` must call `bank.set()` here (not in `activate()`) so
    /// that loaded values overwrite the defaults that `activate()` establishes.
    /// Implementations that allocate DSP state in `activate()` (delay lines,
    /// resamplers) should not re-allocate in `deserialize()` — `activate()` has
    /// already done that; `deserialize()` only restores *values*.
    fn deserialize(&mut self, _data: &[u8]) {}

    /// Called by the runtime immediately after registration to inform this
    /// node of its assigned ID. Used by nodes that publish state to construct
    /// StateBus path strings.
    ///
    /// Default is a no-op. Override in nodes that publish state.
    fn set_node_id(&mut self, _id: u32) {}

    /// Returns a human-readable type label for this node.
    ///
    /// Used in project file snapshots (`NodeSnapshot.type_name`) and in
    /// diagnostic output. Not used for dispatch — project load is by `NodeId`.
    ///
    /// Default: `std::any::type_name::<Self>()`. Override to provide a stable
    /// name that does not change with module path refactors (e.g. `"AnalogEngine"`
    /// rather than `"paraclete_nodes::analog_engine::AnalogEngine"`).
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Apply initial parameter values from the instrument definition file.
    /// Called after node construction, before activate().
    /// Nodes with ParameterBank should store the values and apply them
    /// in activate() before returning (after bank construction).
    fn set_initial_params(&mut self, _params: &std::collections::HashMap<String, f64>) {}

    /// Publish runtime state to the state bus.
    ///
    /// Called by the executor at the end of each audio process cycle (audio
    /// thread). Push zero or more `(key, value)` pairs into `buf`. `buf` is
    /// pre-allocated by the runtime and cleared before each call; the node
    /// must not call `buf.clear()` itself.
    ///
    /// Key strings follow the `/node/{id}/state/` convention. The runtime
    /// does not prepend anything — nodes are responsible for full paths.
    ///
    /// Default: no-op (nodes that publish no state do nothing).
    fn published_state(&self, _buf: &mut Vec<(String, StateBusValue)>) {}

    // ── Loop break protocol (ADR-028) ─────────────────────────────────────────

    /// Returns `true` if this node is a feedback loop break node (ADR-028).
    ///
    /// Only `LoopBreakNode` overrides this to return `true`. The runtime uses
    /// this to sanction cycles that contain exactly one loop-break node.
    ///
    /// Default: `false`.
    fn is_loop_break(&self) -> bool {
        false
    }

    /// Returns the "previous cycle" output slice stored inside the loop-break node.
    ///
    /// Called by the executor in the pre-execution phase to inject the previous
    /// cycle's signal into the downstream node's input buffer before any
    /// `process()` call. Returns an empty slice from the default implementation.
    fn loop_break_prev(&self) -> &[f32] {
        &[]
    }

    /// Swaps the "previous" and "next" internal buffers.
    ///
    /// Called by the executor in the post-execution phase, after all nodes have
    /// been processed. Makes the data captured in `next` (this cycle's input)
    /// available via `loop_break_prev()` in the next cycle. Default is a no-op.
    fn loop_break_swap(&mut self) {}

    // ── Level 3 — override for smart nodes ───────────────────────────────────

    /// Returns `true` if this node actively participates in the connection
    /// handshake — i.e., it implements `Negotiable` and overrides `negotiate()`
    /// and/or `set_connection_record()` with meaningful behaviour.
    ///
    /// Default returns `false`. Override to `true` whenever `Negotiable` is
    /// implemented. The runtime uses this to log negotiation-aware connections
    /// and may use it to skip redundant handshakes in performance-sensitive paths.
    fn is_negotiable(&self) -> bool {
        false
    }

    /// Participate in the connection handshake.
    ///
    /// When two nodes connect, the runtime calls `negotiate()` on both sides,
    /// passing each node the other's `CapabilityDocument`. The runtime reconciles
    /// the two `ConnectionAgreement`s into a final agreement shared by both nodes.
    ///
    /// Default returns `ConnectionAgreement::baseline()`.
    ///
    /// Called on the main thread at connection time.
    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        ConnectionAgreement::baseline()
    }

    /// Receive the reconciled connection record after both sides have negotiated.
    ///
    /// Store the record to know who you are connected to and what was agreed.
    /// Called by the runtime after `negotiate()` on both sides completes.
    ///
    /// Default is a no-op.
    fn set_connection_record(&mut self, _record: ConnectionRecord) {}
}

// ── Negotiable ────────────────────────────────────────────────────────────────

/// Marker trait for nodes that actively participate in connection handshakes.
///
/// Implementors **must** also:
/// 1. Override `Node::negotiate()` to return a meaningful `ConnectionAgreement`.
/// 2. Override `Node::set_connection_record()` to store the reconciled record.
/// 3. Override `Node::is_negotiable()` to return `true` so the runtime can
///    discover this node's participation at connection time.
///
/// The `Sampler` implements `Negotiable` to advertise its lockable parameters.
/// The `Sequencer` reads those params from the returned `ConnectionAgreement`
/// to build its per-step lock UI.
pub trait Negotiable: Node {}
