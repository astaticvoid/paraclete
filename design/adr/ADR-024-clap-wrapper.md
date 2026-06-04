# ADR-024: CLAP Plugin Wrapper Architecture

**Date:** June 2026
**Status:** Accepted — P7 implementation target

-----

## Decision

The CLAP plugin adapter is a hand-rolled translation layer, not a framework
wrapper. It synthesises `ProcessInput` from CLAP buffer pointers, calls
`node.process()` (or `executor.process()` for a subgraph), and reads
`ProcessOutput`. The adapter is written once and is reused for all plugin
targets. Nodes are not modified. The `nih-plug` framework is removed from
workspace dependencies.

-----

## Context

P7 delivers CLAP plugin mode: nodes and machine bank subgraphs exportable
as standalone CLAP instruments. ADR-022 established the portability rule and
estimated the adapter at ~200 lines. This ADR specifies the adapter design.

Two plugin targets:

**Single-node plugin** — one portable node (e.g. Sequencer, a synthesis engine)
wrapped as a CLAP instrument. The DAW drives the plugin's audio callback;
the adapter translates that into `activate()` / `process()` / `deactivate()`.

**Machine bank plugin** — a subgraph (Sequencer + generator node + MixNode)
wrapped as a single CLAP instrument. The adapter drives a `NodeExecutor`
rather than a single node. The plugin exposes the generator node's machine
variant parameters as CLAP parameters.

-----

## Why hand-rolled over nih-plug

`nih-plug` is listed in `architecture-core.md` as a workspace dependency.
After P6, the case against it is clear:

**Parameter system conflict.** nih-plug has its own parameter management
system (enums, `#[id = "..."]` macros, `ParamPtr` handles). Paraclete's
nodes use `ParameterBank` driven by `NodeCommand`. Bridging the two requires
maintaining parallel parameter declarations and a sync layer. This complexity
exists permanently and grows with every new parameter added to any node.

**Lifecycle coupling.** nih-plug plugins implement the `Plugin` trait;
the framework drives the lifecycle. Paraclete nodes implement `Node`; the
caller drives the lifecycle. The adapter's job is to be the caller. nih-plug
inverts this: the adapter becomes the callee, adding a layer of indirection
with no benefit.

**No LGPL3 clarity.** nih-plug is MIT/GPL3. The combined plugin binary is
GPL3. This is correct and expected — but nih-plug's abstractions embed
themselves throughout the plugin code, making the GPL3 scope unclear to
third-party node authors trying to understand the LGPL3/GPL3 boundary.

**Scope.** A hand-rolled adapter that synthesises `ProcessInput` and reads
`ProcessOutput` is genuinely ~200 lines for a single-node target and ~350
lines for a subgraph target. There is nothing nih-plug provides that justifies
the coupling cost.

The `clack` crate (safe CLAP host bindings) remains as a P8 dependency for
loading third-party CLAP plugins. It is not used in the P7 adapter — the
P7 adapter is the plugin side, not the host side.

-----

## Adapter Interface

Two public types, both in a new `paraclete-clap` crate (GPL3):

```rust
// paraclete-clap/src/lib.rs

/// Wraps a single portable node as a CLAP instrument plugin.
/// The node's capability_document() determines the CLAP parameter list.
/// The node's event port receives CLAP note and automation events.
pub struct SingleNodePlugin {
    node:        Box<dyn Node>,
    bank_mirror: ClapParamBridge,   // CLAP param ID ↔ Paraclete param_id
    sample_rate: f32,
    block_size:  usize,
}

/// Wraps a NodeExecutor subgraph as a CLAP instrument plugin.
/// Exposes one generator node's capability_document() as CLAP parameters.
/// Used for machine bank plugins (Sequencer + generator + Mixer).
pub struct SubgraphPlugin {
    executor:       NodeExecutor,
    exposed_node:   NodeId,         // which node's params to surface
    bank_mirror:    ClapParamBridge,
    sample_rate:    f32,
    block_size:     usize,
}
```

Both implement the raw CLAP plugin C API via the `clap-sys` crate
(raw C bindings, no framework). The CLAP entry point and factory are
boilerplate — identical for both targets.

-----

## CLAP Parameter ↔ NodeCommand Bridge

CLAP assigns its own `param_id` (u32) to each plugin parameter.
Paraclete uses `id_for_name()`-based `param_id` values in `NodeCommand`.
These are not the same namespace and must not be conflated.

`ClapParamBridge` owns the translation table:

```rust
struct ClapParamBridge {
    // CLAP param_id (sequential, assigned at plugin instantiation)
    // ↔ Paraclete param_id (id_for_name hash, content-addressed)
    clap_to_paraclete: Vec<(u32, u32)>,  // (clap_id, paraclete_id)
}
```

At plugin instantiation, `ClapParamBridge::from_capability_document()` iterates
the node's declared parameters in order, assigning sequential CLAP IDs starting
from 0. The content-addressed Paraclete IDs are stored alongside.

When CLAP automation sends a `ParamValue` event:
```rust
// In the CLAP process callback:
let paraclete_id = bridge.paraclete_id_for(clap_param_id);
commands.push(NodeCommand {
    type_id: CMD_SET_PARAM,
    arg0:    paraclete_id as f64,
    arg1:    value,
});
```

When the plugin needs to report parameter values to the host (state save,
automation readout):
```rust
let value = node.bank().get(bridge.paraclete_id_for(clap_param_id));
```

The CLAP parameter list is stable for the lifetime of the plugin instance.
Adding parameters to a node in a future version changes the CLAP parameter
list — this is a breaking change to any DAW project that has automated that
plugin. This reinforces the importance of the P6.5 parameter name cleanup
and the ADR-019 naming convention: parameter names are long-term contracts.

-----

## DAW Transport → TransportInfo / TransportEvent

ADR-009 places DAW host transport at the top of the clock priority order.
The concrete translation:

```rust
// Called each CLAP process() callback, before node.process():
fn translate_transport(
    clap_transport: &clap_sys::clap_event_transport,
    block_size: usize,
    sample_rate: f32,
) -> (TransportInfo, Option<TransportEvent>) {
    let playing = clap_transport.flags & CLAP_TRANSPORT_IS_PLAYING != 0;
    let bpm     = clap_transport.tempo;   // f64, always valid
    let beat_pos = clap_transport.song_pos_beats;  // in beats × 2^31

    let ticks_per_beat = TICKS_PER_BEAT as f64;
    let tick_pos = (beat_pos as f64 / (1u64 << 31) as f64) * ticks_per_beat;

    let info = TransportInfo {
        bpm:           bpm as f32,
        ticks_per_beat: TICKS_PER_BEAT,
        current_tick:  tick_pos as u64,
        block_size,
        sample_rate,
        playing,
    };

    let event = if playing {
        // GlobalStart equivalent — emit once when playback begins
        Some(TransportEvent::GlobalStart)
    } else {
        Some(TransportEvent::Stop)
    };

    (info, event)
}
```

The `InternalClock` node is not used when the adapter drives transport.
The adapter synthesises `TransportInfo` directly and injects it into
the ProcessInput that drives the Sequencer nodes. The Sequencers receive
DAW transport as if it were an `InternalClock` output — no Sequencer changes
required.

**Loop point handling:** CLAP provides loop start/end in beats. These map
to `TransportEvent::LoopStart` and `TransportEvent::LoopEnd`. When the DAW
loops, the adapter emits the appropriate events; the Sequencer's existing
`cmd_set_length` and loop-count logic handles them.

-----

## LGPL3 / GPL3 Boundary

| Crate | License | Role |
|-------|---------|------|
| `paraclete-node-api` | LGPL3 | Public node contract — third parties link against this |
| `paraclete-nodes` | GPL3 | First-party nodes |
| `paraclete-clap` | GPL3 | CLAP adapter — wraps nodes for DAW use |
| Combined plugin binary | GPL3 | The `.clap` file distributed to users |

Third-party node authors write against `paraclete-node-api` (LGPL3). Their
nodes can link into a `paraclete-clap` plugin binary. The resulting binary
is GPL3. This is correct: the platform infrastructure is GPL3; the LGPL3
boundary is at the node API to allow third-party node distribution without
GPL3 infection.

Node authors who wish to distribute their nodes under a proprietary license
must use dynamic linking (`.so` / `.dll` against the LGPL3 API). The platform
does not support this at P7; it is a P8+ concern.

-----

## `paraclete-clap` Crate Structure

```
paraclete-clap/
├── Cargo.toml             (GPL3; deps: paraclete-node-api, paraclete-nodes,
│                           paraclete-runtime, clap-sys)
├── src/
│   ├── lib.rs             (re-exports SingleNodePlugin, SubgraphPlugin)
│   ├── plugin.rs          (CLAP entry point, factory, plugin lifecycle)
│   ├── single_node.rs     (SingleNodePlugin impl)
│   ├── subgraph.rs        (SubgraphPlugin impl)
│   ├── bridge.rs          (ClapParamBridge)
│   ├── transport.rs       (translate_transport())
│   └── process_input.rs   (synthesise ProcessInput from CLAP buffers)
└── tests/
    └── clap_validator.rs  (clap-validator integration — runs on CI)
```

`clap-validator` runs as a CI step against every plugin binary produced
by `paraclete-clap`. This is the correctness gate for all CLAP work.

-----

## Consequences

**P7:**
- `paraclete-clap` crate created as described
- `nih-plug` removed from workspace `Cargo.toml`
- `clap-sys` added as a direct dependency of `paraclete-clap`
- `clap-validator` added as a dev dependency / CI step
- Machine bank plugins: one `.clap` per machine variant (KickMachine,
  SnareMachine, FmKickMachine, FmBellMachine, FmBassMachine)
- Single-node plugin: Sequencer as a CLAP MIDI effect (receives MIDI,
  outputs MIDI note events at the step trigger times)

**P8:**
- `clack` used in the platform to load third-party CLAP plugins as nodes
- `SubgraphPlugin` extended to support runtime machine slot swapping
  (depends on ADR-023 GraphNode, targeted at P9)

**Ongoing:**
- Every new parameter added to any node is a potential change to the
  CLAP parameter list for any plugin that surfaces that node. Parameter
  additions are backwards-compatible (new param, new CLAP ID). Parameter
  renames or removals are breaking changes. The ADR-019 naming convention
  is the primary guard against accidental breaking changes.

-----

## References

- ADR-001 — License (GPL3 / LGPL3 boundary)
- ADR-003 — Plugin format (CLAP primary)
- ADR-009 — Clock Federation (DAW transport priority)
- ADR-014 — Capability Document (parameter declaration)
- ADR-019 — Universal Parameter Control (ParameterBank, param IDs)
- ADR-022 — Node Portability (portability rule; machine bank architecture)
- ADR-023 — Instrument Encapsulation (GraphNode; P9 inner graph)
- `clap-sys` — raw CLAP C API bindings (Rust)
- `clap-validator` — CLAP plugin validation tool
