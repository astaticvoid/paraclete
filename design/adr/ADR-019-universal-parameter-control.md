# ADR-019: Universal Parameter Control

**Date:** May 2026
**Status:** Accepted — amended June 2026 (P6.5: parameter naming convention)

---

## Decision

Any hardware control, script, or node can reach any parameter declared in any
node's `capability_document()` generically — without knowing the node's specific
type. Two universal `NodeCommand` type IDs handle this:

```rust
pub const CMD_SET_PARAM: u32 = 0;   // arg0 = param_id, arg1 = absolute value
pub const CMD_BUMP_PARAM: u32 = 1;  // arg0 = param_id, arg1 = signed delta
```

`ParameterBank` — a new L2 helper — implements this contract generically and
allocation-free. Every node that declares parameters in `capability_document()`
uses `ParameterBank` to store and service them.

Parameter declarations are **hints**: flexible contracts that describe what a
node offers, not rigid specifications that lock it into a particular shape.
When two nodes negotiate a richer shared vocabulary via `ConnectionAgreement`,
richer behavior activates. When they don't, the generic form always works and
nothing breaks.

Parameters not declared in `capability_document()` are private to the node.
The declaration is the boundary.

---

## Context

The cellular architecture (ADR-018) establishes that hardware controllers are
nodes with the same standing as any other node, and that any parameter, state,
or event in the system must be reachable from hardware. Principle 7 in
architecture-core.md states this as a binding constraint: "Any parameter, state,
or event in the system must be reachable from hardware."

Without an explicit mechanism for generic parameter control, this principle is
aspirational but not actionable. A hardware encoder wanting to control a
`FilterNode`'s cutoff needs to know it is a `FilterNode`. A profile script
wanting to control a third-party effect node has no generic path. Every
hardware→parameter mapping becomes a type-specific contract that breaks when
the node is replaced.

The existing `NodeCommand` system (introduced at P4) provides the transport.
The existing `capability_document()` mechanism (ADR-014) provides the
declaration. This ADR connects them.

---

## Rationale

### Parameters as published interfaces

ADR-014 established that `capability_document()` is the universal introspection
mechanism — the means by which the sequencer discovers lockable parameters, the
GUI discovers what to display, and hardware mappings discover what parameters
are available. The logical extension: a declared parameter is not just observable
but controllable, through a standard path that requires no type knowledge.

This is the same principle as the state bus ownership rule: nodes own their
`/node/{id}/*` namespace. Parameters declared in `capability_document()` are
the node's published interface. The platform treats everything published as
reachable.

### Hints, not rigid contracts

Parameters are declared with min, max, default, unit, and a content-addressed
ID (hash of the parameter name). These are hints — they describe the parameter's
expected range and meaning. The platform uses them to:

- Clamp incoming `CMD_SET_PARAM` values to the declared range
- Compute `CMD_BUMP_PARAM` deltas relative to the current value
- Display meaningful labels and ranges in tooling
- Allow parameter locks to survive capability renegotiation (content-addressed
  IDs mean a lock for `"cutoff"` remains valid across preset changes)

They do not prevent a node from accepting values outside the declared range
internally, or from interpreting a parameter in a context-sensitive way. The
hint is for the platform and for tooling; the node remains in control of its
own behavior.

### Graceful degradation as the floor

When a hardware encoder sends `CMD_BUMP_PARAM` to a node, the minimum guaranteed
behavior is: the delta is applied to the declared current value, clamped to the
declared range, and stored. The node reads the updated value from `ParameterBank`
in its next `process()` call.

When two nodes share a richer vocabulary — established via `ConnectionAgreement`
at connection time (ADR-015) — that vocabulary activates. A sequencer and a synth
that both understand MPE zone parameters can negotiate a richer contract. A
hardware surface that speaks a device-specific parameter protocol can negotiate
it bilaterally. The generic floor is always present underneath; the richer
behavior is additive.

This is the same graceful degradation model that `ConnectionAgreement` uses for
event space negotiation — applied to parameter control.

### ParameterBank belongs in L2

The correct implementation of `CMD_SET_PARAM` and `CMD_BUMP_PARAM` on the audio
thread requires:

1. Cached parameter metadata (min, max, default) — `capability_document()` must
   not be called in `process()` as it may allocate
2. Clamped value storage — one `f64` per declared parameter
3. Handling of unknown `param_id` values — silently ignored, not a panic

Every node that declares parameters needs exactly this. Without a standard
helper, every node author solves the same problem independently. With
`ParameterBank` in L2, the correct implementation is available to all node
authors — first-party and third-party — as part of the public API contract.

Node-specific command type IDs start at 16, reserving 0–15 for future
universal commands alongside `CMD_SET_PARAM` and `CMD_BUMP_PARAM`.

---

## `ParameterBank` Specification

```rust
/// Pre-allocated parameter storage. Build at activate(), use in process().
/// Handles CMD_SET_PARAM and CMD_BUMP_PARAM with zero audio-thread allocation.
pub struct ParameterBank {
    slots: Vec<ParameterSlot>,
}

// Private — not part of the public API
struct ParameterSlot {
    param_id: u32,
    current:  f64,
    min:      f64,
    max:      f64,
    default:  f64,
}

impl ParameterBank {
    /// Build from a capability document. Call at activate() time.
    /// Iterates declared parameters; caches metadata; sets current = default.
    pub fn from_capability_document(doc: &CapabilityDocument) -> Self;

    /// Apply CMD_SET_PARAM and CMD_BUMP_PARAM commands from input.commands().
    /// All other type_ids are silently ignored.
    /// Call before any DSP logic in process().
    pub fn handle_commands(&mut self, commands: &[NodeCommand]);

    /// Current value of a parameter. Returns declared default if not found.
    pub fn get(&self, param_id: u32) -> f64;

    /// Set a parameter value directly (clamped to declared range).
    /// For use in activate(), deserialize(), or direct node-internal control.
    pub fn set(&mut self, param_id: u32, value: f64);

    /// Reset all parameters to their declared defaults.
    pub fn reset(&mut self);
}
```

**`handle_commands()` implementation contract:**

```
for each command in commands:
    if command.type_id == CMD_SET_PARAM:
        find slot where slot.param_id == command.arg0 as u32
        if found: slot.current = clamp(command.arg1, slot.min, slot.max)
        if not found: silently ignore

    if command.type_id == CMD_BUMP_PARAM:
        find slot where slot.param_id == command.arg0 as u32
        if found: slot.current = clamp(slot.current + command.arg1, slot.min, slot.max)
        if not found: silently ignore
```

Linear scan over `slots` is correct and efficient for typical parameter counts
(< 32). A HashMap is not used — it would require allocation at construction time
on the audio thread. `Vec` pre-allocated at `activate()` is the right structure.

---

## Parameter Naming Convention

*(Added P6.5 — motivated by the LadderFilterNode parameter naming regression.)*

The content-addressed parameter ID model (`id_for_name()`) only provides
cross-node genericness if parameters that represent the same concept use the
same name string. A hardware encoder mapped to `id_for_name("cutoff")` reaches
any node that declares a parameter named `"cutoff"` — and only those nodes.

**Rule: parameters representing the same physical concept must use the same
name string across all nodes that declare them.**

Canonical names for common parameters:

| Name | Concept | Typical range | Example nodes |
|------|---------|---------------|---------------|
| `"cutoff"` | Filter cutoff frequency | Hz | FilterNode, LadderFilterNode |
| `"resonance"` | Filter resonance / Q | 0.0–1.0 | FilterNode, LadderFilterNode |
| `"drive"` | Saturation / input gain | 0.0–1.0 | DistortionNode, AnalogEngine, FmEngine, LadderFilterNode |
| `"wet"` | Wet mix level | 0.0–1.0 | ReverbNode, DelayNode |
| `"dry"` | Dry mix level | 0.0–1.0 | ReverbNode, DelayNode |
| `"decay"` | Amplitude decay time | seconds | AnalogEngine, FmEngine, EnvelopeNode |
| `"attack"` | Amplitude attack time | seconds | EnvelopeNode, FmEngine (FmBass) |
| `"release"` | Amplitude release time | seconds | EnvelopeNode, Sampler |
| `"tune"` | Pitch offset | semitones | OscillatorNode, AnalogEngine, FmEngine |

This table is non-exhaustive. Before introducing a new parameter name, check
whether an existing canonical name covers the concept. Prefer the existing name.

**Corollary: parameter names that are node-type-specific prefixes are wrong.**
`"ladder_cutoff"` is wrong; `"cutoff"` is right. A hardware encoder should not
need to know whether it is talking to an SVF or a ladder filter to control cutoff.

**Stability:** Parameter names are long-term contracts. A renamed parameter in a
published node is a breaking change for any DAW project that has automated that
parameter (via CLAP, P7+) or any profile script that references it by name. Names
in `capability_document()` should be treated with the same care as public API
function names.

---

## ParamLock Pattern for Generator Nodes

*(Added P6.5 — codifying the pattern caught in the P6 code review.)*

Generator nodes (AnalogEngine, FmEngine, and any future nodes with per-step
parameter override behaviour) must implement the per-cycle node_locks pattern,
not route ParamLock events through `bank.handle_commands()`.

**The pattern:**

```rust
// In the generator node struct:
node_locks: Vec<(u32, f64)>,  // cleared at top of process()

// In process(), first thing:
self.node_locks.clear();

// ParamLock event handling (inside the event scan loop):
Event::ParamLock(pl) => {
    self.node_locks.push((pl.param_id, pl.value));
}

// Parameter read helper:
fn get_param(&self, param_id: u32) -> f64 {
    self.node_locks.iter()
        .find(|(id, _)| *id == param_id)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| self.bank.get(param_id))
}
```

`bank.handle_commands()` handles `CMD_SET_PARAM` and `CMD_BUMP_PARAM` (base
value changes). `node_locks` handles `ParamLock` events (per-step overrides).
They must not be conflated — routing ParamLock events through the bank
permanently changes the base value, causing the lock to bleed into subsequent
cycles that did not request the lock.

---

## Consequences

**For node authors:**

A node that declares parameters in `capability_document()` and uses `ParameterBank`
gets generic hardware control with no additional code. The node's parameters are
reachable by any hardware encoder, any profile script, and any other node that
sends `CMD_SET_PARAM` or `CMD_BUMP_PARAM`.

A node that does not use `ParameterBank` can still handle these commands manually
in `process()` — `ParameterBank` is a helper, not a requirement. But the behavior
it provides (clamping, default handling, unknown-ID silence) must be replicated
correctly if not using it.

**For hardware profiles:**

Profile scripts can control any parameter on any node using:

```rhai
// Absolute set
send_cmd(node_id, 0, param_id, value);   // CMD_SET_PARAM

// Relative bump — encoder delta
send_cmd(node_id, 1, param_id, delta);   // CMD_BUMP_PARAM
```

The script does not need to know the node's type. It queries `param_id` from
the node's capability document if needed, or uses a known constant if the
relationship is fixed (e.g. a dedicated encoder always controls filter cutoff).
With canonical parameter names, the same encoder binding works across node types.

**For the sequencer:**

Parameter locks (already implemented at P3 via `Event::ParamLock`) and generic
parameter control (CMD_SET_PARAM/CMD_BUMP_PARAM via NodeCommand) are now two
paths to the same underlying storage. `ParameterBank` reconciles them per the
ParamLock pattern above: parameter locks apply per-step overrides temporarily;
`CMD_SET_PARAM` changes the base value persistently. Nodes that use `ParameterBank`
and the `node_locks` pattern handle both correctly.

**For third-party nodes:**

Any third-party node published after P4 that declares parameters in
`capability_document()` and uses `ParameterBank` is automatically controllable
from any hardware surface in any Paraclete instrument — without any platform
changes. Using canonical parameter names means the node's parameters are
reachable by existing hardware profiles without modification. This is the
open extensibility the LGPL3 boundary is designed to enable.

**For CLAP plugins (P7+):**

Parameter names in `capability_document()` become the basis for CLAP parameter
IDs and display names. A rename after crates.io publication breaks DAW automation
lanes and saved projects. Treat parameter names as stable public API from
`paraclete-node-api` v0.1.0 onward.

---

## References

- ADR-014 — Capability Document (mandatory with defaults; content-addressed
  param IDs; regenerable)
- ADR-015 — Connection Negotiation (bilateral; graceful degradation model)
- ADR-018 — Cellular Architecture (hardware reaches any declared parameter;
  no second-class platform objects)
- ADR-024 — CLAP Plugin Wrapper (CLAP parameter ↔ NodeCommand bridge)
- p4-interfaces.md — `ParameterBank` implementation spec; `CMD_SET_PARAM` and
  `CMD_BUMP_PARAM` constants; node-specific type IDs start at 16
- p6-report.md — ParamLock bleed finding (P6 post-ship code review)
