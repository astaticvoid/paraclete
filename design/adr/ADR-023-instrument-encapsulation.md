# ADR-023: Instrument Encapsulation — GraphNode and the Instrument Vision

**Date:** June 2026  
**Status:** Accepted — P9 implementation target; shapes design from P6 onward

-----

## Decision

An *instrument* is a named, bounded subgraph that presents a focused workflow surface
to the outside world while enclosing a complete synthesis graph internally. The formal
primitive for this is `GraphNode` — a node whose `process()` implementation drives an
internal `NodeExecutor`. The outer node’s `capability_document()` is the instrument’s
workflow surface. The inner graph is the synthesis engine.

The runtime sees one node. The internals are a complete graph.

This is the long-range composition model for Paraclete. It shapes synthesis node design
from P6 onward, even though the `GraphNode` primitive itself ships at P9.

-----

## Context

The platform vision (architecture-core.md) names three reference points: Plan 9, Emacs,
and Elektron. The Elektron reference is the most concrete: the goal is an open-source
runtime capable of replicating the complete workflow of Elektron instruments —
Digitakt, Digitone, Syntakt, Analog Rytm, Analog Four — and hardware from comparable
makers, as composable instrument configurations rather than hardcoded applications.

Each of these instruments is, structurally, a bounded graph:

|Instrument |Internal topology                                                                     |
|-----------|--------------------------------------------------------------------------------------|
|Digitakt   |8 sampler voices × (amplitude envelope + filter + distortion) + 8 sequencers + mixer  |
|Digitone   |4 FM voices × (4 operators + envelope + filter + LFO) + 4 sequencers + mixer          |
|Syntakt    |12 voice slots × swappable machine (analog, FM, sample, noise) + 12 sequencers + mixer|
|Analog Rytm|8 analog drum voices × (analog engine + sample layer + filter + envelope) + sequencer |
|Analog Four|4 analog synth voices × (oscillators + filter + envelopes + LFOs) + sequencer         |

None of the components in these topologies are special objects — they are synthesis
primitives composed in a fixed arrangement with a fixed UI surface. In Paraclete, the
topology is a subgraph. The UI surface is the instrument node’s declared parameter space.

The end goal: these instruments exist as named `GraphNode` configurations, swappable
into any Paraclete graph as single nodes, exposable as standalone CLAP plugins, and
modifiable by replacing or rewiring their internal primitives.

-----

## GraphNode

`GraphNode` is a node whose internal implementation is itself a Paraclete subgraph.

```rust
/// A node that encloses a complete subgraph.
/// Outer capability document = instrument workflow surface.
/// Inner executor drives the synthesis graph each process() call.
pub trait GraphNode: Node {
    /// The inner graph. Built at activate(), torn down at deactivate().
    fn inner_graph(&self) -> &NodeExecutor;
    fn inner_graph_mut(&mut self) -> &mut NodeExecutor;
}
```

Lifecycle:

```
activate():
    build inner graph topology
    activate() all inner nodes
    pre-allocate all inner buffers

process(input):
    translate outer ProcessInput → inner clock / event / command inputs
    tick inner NodeExecutor for this buffer
    translate inner audio outputs → outer port outputs

deactivate():
    deactivate() all inner nodes
    release inner buffers
```

The outer node’s `capability_document()` declares the instrument’s workflow surface —
the parameters and ports the outside world interacts with. The inner graph’s full
parameter depth is accessible via `ConnectionAgreement` negotiation but is not the
default surface.

-----

## Instrument Workflow Surface vs. Internal Depth

The distinction between workflow surface and internal depth is central to the model.

**Workflow surface:** The parameters a performer reaches for during a session. On a
Digitakt kick track: tune, punch, decay, drive, tone, filter cutoff, filter resonance,
reverb send. These are declared in `capability_document()`. A hardware surface sees
them. DAW automation sees them. Parameter locks target them.

**Internal depth:** The full synthesis graph — every operator ratio, every envelope
stage, every filter coefficient mode. Available to a node that negotiates a richer
vocabulary via `ConnectionAgreement`, or to a developer building a new machine variant.
Not exposed by default. Not hidden — the inner graph is introspectable to any node with
sufficient negotiated access.

This is not simplification by omission. It is deliberate curation: the instrument
presents the surface that serves its workflow. The full depth serves synthesis designers
and advanced users, not the primary performance surface.

-----

## Synthesis Primitive Vocabulary Study (Pre-P6)

Before designing the synthesis engines that populate these instruments, a systematic
study of Elektron instrument architectures is required. The study extracts the complete
set of primitive building blocks across the reference instruments:

- Every filter topology (multimode SVF, ladder, comb)
- Every envelope shape and triggering mode (ADSR, AD, looping, retriggerable)
- Every LFO mode (sine, square, ramp, random, envelope-synced)
- Every machine type across Syntakt’s engine bank
- Every oscillator type (analog-modelled, wavetable, FM operator, noise)
- Every routing option (filter FM, envelope-to-pitch, LFO-to-filter, velocity routing)

The output of this study is a **primitive node vocabulary** — a minimal set of
composable nodes from which all reference instrument topologies can be constructed
without gaps. This vocabulary drives P6 synthesis node design.

The study is conducted from published Elektron manuals and sound design documentation.
The outputs are internal design documents (node vocabulary, composition diagrams,
ADR amendments). Public-facing project text describes what the platform does, not which
instruments it targets. See the note on public framing below.

-----

## The Composition Hierarchy

From primitive to instrument:

```
Primitive nodes (P6)
    oscillators, envelopes, filters, LFOs, operators
        ↓
Generator nodes (P6)
    analog engine, FM engine — portable, machine-configurable
        ↓
Machine variants (P6/P7)
    KickMachine, SnareMachine, FmBellMachine, etc.
    named configurations with constrained capability surfaces
        ↓
GraphNode instruments (P9)
    Digitakt-topology instrument, Digitone-topology instrument, etc.
    inner graph = machine bank + sequencers + mixer
    outer surface = performer workflow parameters
        ↓
Standalone CLAP plugins (P7+)
    any GraphNode wrapped in the minimal host adapter
    DAW sees one instrument
```

Each layer is useful independently. Primitive nodes are usable in any graph. Generator
nodes are usable as machine slots or standalone voices. Machine variants are usable as
drop-in drum/synth voices. GraphNode instruments are usable as single-node instruments
in larger graphs or as plugins. No layer requires the layers above it.

-----

## Swappable Machine Slots

The Syntakt model — where each track can hold any machine type — is the general case.
A machine slot is a connection point in the inner graph where any compatible generator
node can be inserted. Compatibility is established via `ConnectionAgreement` at
connection time: the slot declares what it needs (audio output, a standard set of
envelope/filter parameter contracts); the machine declares what it provides.

This means a track in a GraphNode instrument can be reconfigured at runtime by
disconnecting the current generator and connecting a different one. The sequencer, mixer
connection, and hardware surface binding remain unchanged. Only the sound engine changes.

This is P9 scope in full generality. At P7, machine slots are fixed at instrument
construction time (preset configurations). Runtime swapping arrives with the full
`GraphNode` primitive.

-----

## Public Framing

The platform’s synthesis targets are Elektron-inspired but the public description is
capability-based, not comparative. Referencing specific Elektron product names in
public-facing text (README, crates.io, release notes) creates unnecessary legal and
reputational exposure.

**Internal documents** (ADRs, design docs, roadmap): Elektron instrument names and
workflow references are used freely. This is prior art research and design validation.

**Public-facing text**: Describes what the platform does.

|Internal                      |Public equivalent                                                         |
|------------------------------|--------------------------------------------------------------------------|
|“Digitakt-topology instrument”|“8-track drum machine with per-step parameter locks and conditional trigs”|
|“Digitone-topology instrument”|“4-voice FM synthesizer with per-track sequencing and LFO modulation”     |
|“Syntakt-style machine slots” |“Per-track swappable synthesis engines”                                   |
|“Elektron workflow”           |“Hardware-first step sequencer workflow”                                  |

The synthesis concepts themselves — parameter locks, conditional trigs, machine banks,
per-step micro-timing — are not proprietary. They are documented techniques with
independent prior art. The platform implements these techniques. The framing describes
the techniques, not the trademarked products that popularised them.

-----

## Consequences

**P6:**

- Synthesis primitive study conducted before engine design begins
- Primitive node vocabulary established as the P6 design foundation
- Analog emulation engine and FM engine designed as portable nodes per ADR-022
- Machine variant pattern (constrained capability surface) applied to all generator nodes

**P7:**

- Minimal CLAP host adapter enables any subgraph to ship as a standalone plugin
- Machine bank configurations ship as CLAP instruments
- Workflow surfaces validated against real DAW automation and hardware control

**P9:**

- `GraphNode` primitive implemented
- Inner graph fully introspectable and reconfigurable at runtime
- Machine slots become swappable at runtime via `ConnectionAgreement`
- Nested sequencers: a GraphNode instrument’s inner sequencer can receive CV from the
  outer graph, enabling polyrhythmic and generative topologies
- OQ-5 (single-sample feedback loop break) forced and resolved at this phase

**Ongoing:**

- Every synthesis node designed from P6 onward is evaluated against: “can this be a
  primitive in a GraphNode instrument’s inner graph?” If not, the granularity is wrong.
- The machine variant pattern is the default for all generator nodes. Deep engines ship
  with curated machine surfaces. Full depth is available but not the default.

-----

## References

- ADR-018 — Cellular Architecture (the node is the universal primitive; GraphNode
  extends this to subgraphs)
- ADR-022 — Node Portability and Plugin Isolation (portability rule; machine bank
  architecture; machine variant pattern)
- ADR-005 — Scheduler Cycles (loop-break model; relevant to nested graph scheduling)
- `architecture-core.md` — Vision (Plan 9 / Emacs / Elektron reference points)
- `roadmap.md` — P6 synthesis, P7 CLAP plugin mode, P8 CLAP host, P9 modular graph