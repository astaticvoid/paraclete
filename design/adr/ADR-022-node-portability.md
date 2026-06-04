# ADR-022: Node Portability and Plugin Isolation

**Date:** June 2026  
**Status:** Accepted

-----

## Decision

Every node in `paraclete-nodes` (L3) must be implementable against `paraclete-node-api`
(L2) alone, with zero coupling to the runtime (L1), the scripting layer (L4), or any
hardware abstraction (L0). The runtime, scripting engine, and hardware nodes are
*consumers* of nodes — never dependencies of them.

This is already implied by the layer model but is now a binding constraint with explicit
consequences: any node that cannot be extracted into a standalone CLAP/VST3 plugin by
wrapping it in a minimal host adapter is a design violation.

-----

## Context

The P7 roadmap includes CLAP plugin mode. Two delivery targets are anticipated:

**Standalone plugin nodes** — a single node (e.g. Sequencer) wrapped in a minimal CLAP
host that translates DAW transport to `TransportEvent`, DAW parameter automation to
`NodeCommand`, and node event output to CLAP note ports.

**Machine banks** — a curated subgraph of generator nodes (sequencer + sound engine)
wrapped as a single CLAP instrument. The Sampler is the first generator. Subsequent
generators include analog-emulation engines and FM engines. Each generator has
instrument-specific machine variants that expose only the parameters relevant to that
machine’s character — a kick machine, a snare machine, and so on. The full runtime graph
is not present in the plugin — only the subgraph needed for the instrument.

Neither target requires changes to the nodes themselves. The plugin wrapper is the only
new component. This is only guaranteed if nodes are kept free of runtime coupling now.

-----

## The Portability Rule

A node is portable if and only if:

1. It links only against `paraclete-node-api` (LGPL3)
1. Its `activate()` / `process()` / `deactivate()` cycle is driven by the caller — it
   makes no assumptions about what the caller is
1. Its parameters are fully declared in `capability_document()` and handled via
   `ParameterBank` — no side-channel parameter paths
1. Its serialization state is fully captured by `serialize()` / `deserialize()` — no
   external state dependencies
1. It does not hold references to runtime internals, the state bus beyond its declared
   `/node/{id}/*` namespace, or the scripting engine

The minimal host a portable node requires:

```
activate() once
each buffer:
    synthesize ProcessInput
    call process()
    read outputs
    translate host parameter changes → NodeCommand (CMD_SET_PARAM / CMD_BUMP_PARAM)
deactivate() on teardown
```

That adapter is approximately 200 lines and is platform-agnostic. It is the P7 CLAP
wrapper, written once and reused for all plugin targets.

-----

## Machine Bank Architecture

A machine bank is a named subgraph exposed as a single CLAP instrument. It contains:

- One `Sequencer` per voice track
- One *generator node* per voice track
- One `MixNode` summing the generators
- The minimal host adapter driving the subgraph from DAW transport

The generator node is the variable. The Sampler is the first. The intended series:

### Analog emulation engine

Models a complete analog voice circuit per machine. Machine variants constrain the
parameter space to what is musically meaningful for that sound. The full synthesis engine
may have many internal parameters; only the machine-relevant subset is declared in
`capability_document()` and therefore reachable from hardware or DAW automation.

Example machine surfaces:

|Machine       |Declared parameters                 |
|--------------|------------------------------------|
|`KickMachine` |tune, punch, decay, drive, tone     |
|`SnareMachine`|tune, snap, noise, decay, tone      |
|`HiHatMachine`|tune, decay, open/closed ratio, tone|

### FM engine

Operator-based synthesis per machine. FM has large parameter depth; the machine variant
exposes only the parameters that shape that machine’s character usefully.

|Machine        |Declared parameters                         |
|---------------|--------------------------------------------|
|`FmKickMachine`|pitch envelope depth, decay, feedback, drive|
|`FmBellMachine`|ratio, index, decay, feedback               |
|`FmBassMachine`|ratio, index, attack, decay, drive          |

### Machine variant implementation pattern

A machine variant is not a subtype. It is a named *configuration* of a synthesis engine
node — a preset that also constrains the declared parameter surface:

- The synthesis engine loads its machine preset at `activate()`
- `capability_document()` declares only the machine-relevant parameters
- The host (DAW, hardware surface, or platform runtime) sees only those parameters
- The full synthesis depth is available to nodes that negotiate a richer vocabulary via
  `ConnectionAgreement` — the machine surface is the floor, not the ceiling

This pattern generalises: any synthesis node with deep internal parameter space should
ship with named machine configurations that present focused, musically coherent surfaces.
The full depth is never hidden — it is simply not the default surface.

-----

## Consequences

**Immediate:**

- Every new node written at P5 and beyond is verified against the portability rule
  before merge
- Existing nodes (Sequencer, Sampler, FilterNode, DistortionNode, ReverbNode, DelayNode)
  are already compliant — this ADR formalises what was already true
- Code review checklist gains: “does this node link only against L2?”

**P6:**

- Analog emulation engine and FM engine designed as portable nodes
- Machine variants implemented as named configurations with constrained
  `capability_document()` surfaces
- Pre-P6 synthesis primitive study (see ADR-023) establishes the complete vocabulary
  before engine design begins

**P7:**

- Minimal CLAP host adapter written once, reused for all plugin targets
- DAW transport → `TransportEvent` translation layer
- CLAP parameter automation → `NodeCommand` translation layer
- `serialize()` / `deserialize()` hooks into CLAP state extension
- Machine bank plugin wraps subgraph; DAW sees one instrument with the machine variants’
  declared parameters

**Ongoing:**

- The machine variant pattern is the standard model for all future instrument nodes.
  Deep synthesis engines do not expose their full parameter depth by default. They expose
  machine-appropriate surfaces. Full depth is available only to nodes that negotiate
  richer vocabulary via `ConnectionAgreement`.

-----

## References

- ADR-001 — License (LGPL3 boundary enables clean plugin extraction)
- ADR-014 — Capability Document (declared parameters as the published interface)
- ADR-015 — Connection Negotiation (ConnectionAgreement as the richer vocabulary path)
- ADR-018 — Cellular Architecture (nodes have no legitimate runtime dependencies)
- ADR-019 — Universal Parameter Control (ParameterBank as the parameter isolation boundary)
- ADR-023 — Instrument Encapsulation and GraphNode (the long-range composition model)
- `roadmap.md` — P7 CLAP plugin mode; P6 synthesis engines