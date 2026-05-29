# ADR-008: Node API Level — Single Trait, Three Implicit Levels

**Date:** May 2026  
**Status:** Accepted (supersedes previous version)

## Decision

The Node API is a single Rust trait with sensible default implementations for all methods except `ports()` and `process()`. Three implicit levels of engagement emerge from which methods a node overrides. No node is required to engage beyond its needs.

## Context

The previous version of this ADR defined Level 2 (buffers + params + events) as the v1 target and Level 3 (graph access) as v2. This framing was wrong in two ways:

First, it implied nodes must implement a specific level — a binary choice. In practice, a node that only processes signals should not be required to implement event handling or capability negotiation. Imposing that burden makes Paraclete the “XML of music” — technically correct, fully specified, and too complex to build against casually.

Second, it missed a simpler signal-processing level entirely. Bitwig Grid and VCV Rack succeed precisely because a module is just two things: ports and a process function. That simplicity is a feature, not a limitation.

The correct model is one trait with defaults. Complexity is opt-in.

## The three implicit levels

### Level 1 — Signal Node

Implements `ports()` and `process()` only. Gets everything else via default implementations. This is the Bitwig Grid / VCV Rack model — oscillators, filters, math modules, effects. No events, no parameters, no negotiation required.

A competent Rust developer can implement a Level 1 node in an afternoon without reading the rest of the architecture documentation.

### Level 2 — Instrument Node

Overrides `capability_document()` to declare parameters. Handles `Midi2` events in `process()`. Implements `activate()` / `deactivate()` for sample-rate-dependent DSP state. Implements `serialize()` / `deserialize()` for save/recall.

This is what the sampler, FM synth, analog synth, and most third-party instruments need.

### Level 3 — Smart Node

Overrides `negotiate()` to participate in connection handshakes. Overrides `capabilities_changed()` to signal dynamic parameter changes. Implements `StatePublisher` to expose values on the state bus. May implement `GraphHost` to host child nodes.

This is what the sequencer, hardware controller nodes, and modular graph nodes need.

## The trait

```rust
pub trait Node: Send {
    // ── Required ────────────────────────────────────────────────────────────

    /// Declare this node's ports. Called at registration time.
    fn ports(&self) -> &[PortDescriptor];

    /// Process one buffer cycle. The hot path.
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput);

    // ── Level 2 — override for instruments ──────────────────────────────────

    /// Describe this node's capabilities. Default builds from ports() only.
    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument::from_ports(self.ports())
    }

    /// Return true if capability_document() would return something different
    /// from the last call. Runtime polls this at safe reconfiguration points.
    fn capabilities_changed(&self) -> bool { false }

    /// Called when the audio engine starts. Use to allocate DSP state.
    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {}

    /// Called when the audio engine stops.
    fn deactivate(&mut self) {}

    /// Serialise private state for project save. Return empty vec if stateless.
    fn serialize(&self) -> Vec<u8> { vec![] }

    /// Restore private state from project save.
    fn deserialize(&mut self, _data: &[u8]) {}

    // ── Level 3 — override for smart nodes ──────────────────────────────────

    /// Participate in connection handshake. Default produces baseline agreement.
    fn negotiate(
        &mut self,
        _their_doc: &CapabilityDocument,
    ) -> ConnectionAgreement {
        ConnectionAgreement::baseline()
    }
}
```

## Template nodes

Template nodes provide pre-built implementations for common archetypes:

- `SignalNodeTemplate` — for pure signal processors (oscillators, filters, effects)
- `InstrumentTemplate` — for MIDI-driven sound generators
- `SequencerTemplate` — for event-generating sequencer nodes
- `ControllerTemplate` — for hardware controller nodes

Template nodes are the primary contribution path for third-party developers. A reverb effect using `SignalNodeTemplate` requires implementing one function: the DSP.

## GraphHost

`GraphHost` remains a separate trait, not part of the base `Node` trait. Nodes that can host child nodes implement both `Node` and `GraphHost`. This ships when the modular framework is ready at Phase 9, unchanged from the previous version of this ADR.

## Rationale for single trait over multiple traits

Multiple traits (`SignalNode`, `InstrumentNode`, `SmartNode`) would require the runtime to handle different node types differently. A single trait with defaults means the runtime calls the same methods on every node — simpler scheduling, simpler graph management, no type dispatch.

The levels are a documentation and contribution concept, not a type system concept.

## Consequences

- A Level 1 node is two method implementations. Barrier to entry is low.
- The runtime treats all nodes identically — same scheduling, same lifecycle calls
- Template nodes make Level 2 and 3 nearly as simple as Level 1
- GraphHost remains additive and deferred to Phase 9
- The signal type system (PortType taxonomy) is enforced at connection time regardless of node level — a Level 1 node still declares typed ports