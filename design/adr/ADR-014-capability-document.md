# ADR-014: Capability Document — Mandatory with Default Implementation

**Date:** May 2026  
**Status:** Accepted

## Decision

Every node produces a `CapabilityDocument` describing its ports, event vocabulary, parameters, and negotiation preferences. Capability declaration is mandatory — it is part of the base `Node` trait. A default implementation introspects the node’s declared ports and produces a valid minimal document automatically.

## Context

Two options were considered:

**Opt-in via `Negotiable` trait:** Simple nodes skip capability declaration. Richer nodes implement `Negotiable`. Lower implementation burden but loses the guarantee that everything is introspectable.

**Mandatory with default implementation (chosen):** Every node is introspectable. Simple nodes get a sensible default for free. Complex nodes override what they need.

## Rationale

“Nodes should be intelligent and inspectable” is a core design principle. Opt-in negotiation creates a two-tier system where some nodes are black boxes. Mandatory declaration with a sensible default costs nothing for simple nodes and gives the platform a universal introspection guarantee.

The `CapabilityDocument` is the mechanism by which:

- The sequencer discovers lockable parameters on connected nodes
- The scripting layer discovers what actions a node supports
- The GUI discovers what to display
- Hardware mappings discover what parameters are available
- Two nodes negotiate their connection agreement

Without a universal capability document, each of these requires a separate protocol. With it, they all use the same mechanism.

## Template nodes

Template nodes (AudioProcessorTemplate, InstrumentTemplate, SequencerTemplate, ControllerTemplate) provide pre-built `CapabilityDocument` implementations for common node archetypes. A reverb plugin using `AudioProcessorTemplate` gets a complete capability document with no additional code. Template nodes are the primary contribution path for third-party developers.

## Consequences

- The base `Node` trait includes `fn capability_document(&self) -> CapabilityDocument`
- Default implementation calls `self.ports()` and builds a minimal document
- Nodes override `capability_document()` to add parameters, custom event spaces, and negotiation preferences
- The runtime queries capability documents at connection time and at any time via the state bus
- `CapabilityDocument` is serialisable — it can be published to the state bus and read by scripts and the GUI

-----

## Amendment — May 2026

### ParamUnit is extensible, not a closed enum

The initial design assumed a fixed `ParamUnit` enum. This is too restrictive for a fully modular platform. The correct model:

```rust
pub enum ParamUnit {
    Generic,
    Hz,
    Decibels,
    Milliseconds,
    Seconds,
    Semitones,
    Cents,
    Percent,
    Beats,
    Custom(&'static str),       // compile-time custom unit label
    CustomDynamic(String),      // runtime-generated unit label
}
```

Built-in variants get smart formatting. `Custom` and `CustomDynamic` display the number and label with no special formatting logic.

### CapabilityDocument is regenerable, not static

The initial design assumed `CapabilityDocument` is built once at node initialisation. This forecloses:

- Modular nodes with dynamic child graphs
- Nodes that load presets with variable parameter counts
- FM synths where algorithm changes affect visible parameters
- CLAP plugin host nodes where parameters come from the loaded plugin
- Macro nodes that aggregate parameters from connected nodes

The correct model:

- `capability_document()` is a method, not a field — it returns a freshly constructed document reflecting current node state
- The base `Node` trait adds `capabilities_changed() -> bool` defaulting to `false`
- The runtime polls `capabilities_changed()` at safe reconfiguration points (main thread only, never audio thread)
- When true, the runtime re-queries `capability_document()`, updates the state bus, and triggers renegotiation on affected connections
- Static nodes implement nothing — the default covers them

### Parameter IDs are content-addressed

Parameter IDs are a hash of the parameter name — stable by name regardless of position or session. This means:

- A parameter lock for `"operator_2_ratio"` survives a preset change that temporarily removes and re-adds the parameter
- Parameter locks that reference IDs no longer present in the current capability document are held in suspension, not discarded — they reactivate if the parameter returns
- Content addressing makes parameter locks durable across capability renegotiation