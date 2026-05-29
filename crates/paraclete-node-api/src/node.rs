use crate::agreement::ConnectionAgreement;
use crate::capability::CapabilityDocument;
use crate::context::{ProcessInput, ProcessOutput};
use crate::state_bus::StateBusValue;
use crate::port::PortDescriptor;

/// The universal node contract. Every Paraclete node implements this trait.
///
/// Three implicit engagement levels (see ADR-008):
/// - L1: implement only `ports()` and `process()` — passive signal processor.
/// - L2: also override `capability_document()` and lifecycle hooks — instrument.
/// - L3: also override `negotiate()` — smart/protocol-aware node.
///
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
    /// Called on the main thread before `activate()`.
    fn deserialize(&mut self, _data: &[u8]) {}

    /// Called by the runtime immediately after registration to inform this
    /// node of its assigned ID. Used by nodes that publish state to construct
    /// StateBus path strings.
    ///
    /// Default is a no-op. Override in nodes that publish state.
    fn set_node_id(&mut self, _id: u32) {}

    /// Publish named values to the StateBus.
    ///
    /// Called by the executor after each process cycle. Returns key-value
    /// pairs to be written to the StateBus snapshot. Keys should use the
    /// convention `/node/{id}/state/{key}`.
    ///
    /// Default returns empty — only nodes implementing `StatePublisher` override this.
    /// Allocation is permitted here (not on the hot audio path).
    fn published_state(&self) -> Vec<(String, StateBusValue)> {
        vec![]
    }

    // ── Level 3 — override for smart nodes ───────────────────────────────────

    /// Participate in the connection handshake.
    ///
    /// When two nodes connect, the runtime calls `negotiate()` on both sides,
    /// passing each node the other's `CapabilityDocument`. The runtime reconciles
    /// the two `ConnectionAgreement`s into a final agreement shared by both nodes.
    ///
    /// Default returns `ConnectionAgreement::baseline()` — standard audio format,
    /// no custom event spaces.
    ///
    /// Called on the main thread at connection time.
    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        ConnectionAgreement::baseline()
    }
}
