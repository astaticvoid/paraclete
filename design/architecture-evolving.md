# Paraclete — Architecture Evolution Log

> **Append-only from this version onward.** Each phase adds a section at the
> bottom recording what shipped, what was found, and what was deferred.
> Never edit existing entries. For current roadmap and open questions see
> `roadmap.md`.
>
> **Last structural update:** May 2026 (v0.5 — split into log + roadmap)

---

## Stable references

- `architecture-core.md` — layer model, principles, node API, signal types, glossary
- `roadmap.md` — current phase, roadmap table, open questions (living, replaced per phase)
- `instrument-vision.md` — the concrete instrument being built; design tiebreaker
- `decisions/ADR-001` through `ADR-017` — all architectural decisions

---

## Phase Log

### P0 — Skeleton (Complete)

**Delivered:** Six-crate workspace compiling clean. NodeExecutor/NodeConfigurator
split with SPSC ring buffer. petgraph topology with cycle detection. cpal audio
backend. Rhai sandbox. LaunchpadEmulator stub. One node, one audio buffer,
one clock tick.

**Key findings:**
- petgraph required from P0 per ADR-005 — initial Vec implementation caught
  and corrected before it became load-bearing
- NodeOrDevice enum pattern established for mixed Node/HardwareDevice
  dispatch (Rust 1.75 trait object upcasting limitation)

**Deferred:** State bus, multi-node graphs, event routing, capability negotiation,
extended event slab.

---

### P1 — First Sound (Complete)

**Delivered:** LaunchpadEmulator → HardwareMappingNode → SineOscillator → AudioOutput.
Terminal keyboard triggers oscillator. Full typed HardwareDevice surface.
Event::Hardware variant. 96 tests.

**Key findings:**
- HardwareDevice as a graph node (not a MIDI translator) validated — the
  HardwareMappingNode in the graph, not inside the HAL, is the correct model
- midi.rs re-export in paraclete-node-api adopted — third-party node authors
  get MIDI 2.0 types without a separate dependency
- Terminal I/O in process() acceptable for developer tool at this phase;
  flagged for main-thread refactor at P4

**Deferred:** State bus, LED feedback, graphical emulator.

---

### P2 — Sequencer v1 (Complete)

**Delivered:** InternalClock → Sequencer → SineOscillator. Full naming
transition applied (subsequently reverted — see vocabulary cleanup). Clock
domain established. StateBus provisional (Arc<RwLock<HashMap>>).
LED step position feedback. serialize()/deserialize() pattern persistence.
103 tests.

**Key findings:**
- global_start required for Sequencer to begin playback — InternalClock
  emits this on first tick. The clock signals when transport starts;
  sequencers wait for the signal. Correct model.
- TransportEvent routed identically to Event — PortType::Clock is semantic,
  not a separate transport mechanism.
- Naming transition (Greek vocabulary) applied and subsequently fully
  reverted — see vocabulary cleanup section below.

**Deferred:** StateBus SPSC upgrade (moved to P3), connected signal port
views, graphical emulator.

---

### P3 — First Instrument (Complete)

**Delivered:** InternalClock → Sequencer → Sampler → AudioOutput. 4-voice
polyphonic sampler with filesystem WAV loading. Full Negotiable handshake
protocol. Parameter lock full path — emit, route, apply, restore. StateBus
SPSC upgrade (rtrb). Rhai StateBus bindings. 133 tests.

**Key findings:**
- Negotiable as a marker trait, negotiate() on base Node — Rust 1.75 trait
  object upcasting limitation. Resolution: Rust 1.76+ upcasting. No API
  change for node authors.
- published_state() allocates per cycle (OQ-9) — acceptable at P3 scale,
  needs pre-allocated push-down at P6+.
- Pre-computed effective params before voice iterator — borrow checker
  constraint; correct pattern for all future multi-voice nodes.
- Pre-allocated render buffers at activate() — any Vec used in process()
  must be allocated at activate(). Platform convention.
- assert() registered in Rhai — Rhai 1.x omits it; registering in
  ScriptingEngine::new() is correct and permanent.
- StateBusSubscription moved to L2 — correct license boundary.
- EdgeMeta src_port/dst_port fields used in P3 Negotiable handshake;
  #[allow(dead_code)] removed.

**Deferred:** Connected signal port views (P4), slice boundaries (P5),
high-quality resampling (P6, rubato), broader format support (P6, symphonia),
hardware update_output() main-thread refactor (P4).

---

### Vocabulary Cleanup (Between P2 and P3)

The Greek/theological naming scheme introduced at P2 (Melos, Ekthesis,
Symbolon, anaphora(), epiclesis(), etc.) was fully reverted before P3 began.
All identifiers returned to plain English. jargon.md and naming-transition.md
were removed from the codebase.

Reason: Clarity matters more than conceptual density in a technical codebase.
The theological vocabulary is preserved in the design history but does not
belong in identifiers. The project name Paraclete is a proper name and stays.

Full mapping: see vocabulary-revert.md in design/phases/.

---

### Architecture Decisions — May 2026 (Pre-P4)

**ADR-016 — Multi-track sequencer is multiple node instances**
8 tracks = 8 Sequencer node instances sharing one InternalClock. Multi-track
is a graph topology, not a node feature. Each track pairs its own sequencer
with its own instrument node. The Rhai profile manages the node array.

**ADR-017 — Effects are plain nodes, no dedicated trait**
Distortion, filter, reverb, delay — all plain Node implementations with audio
ports. Wet/dry and bypass are parameters, fully lockable by the sequencer.
AudioEffect marker trait for discovery only. DistortionNode and FilterNode
moved to P4 — the instrument is not musically useful without them.

**Instrument vision documented**
instrument-vision.md created. Dawless industrial/techno/hardcore. Three
devices: Launchpad (drum machine/sequencer), Launch Control XL (contextual
parameter control), Keystep 37 (melodic).

**Hardware decisions**
- Launch Control XL Mk2 in use now, Mk3 target
- Mk2: absolute knobs, calibration gesture on connect establishes positions
- Mk3: endless encoders, query protocol, bidirectional sync
- EncoderBehaviour::Absolute vs Relative declared in EncoderDescriptor
- Keystep 37: standard MIDI over USB, no special protocol

**Document model changed**
architecture-evolving.md is now append-only (this file).
roadmap.md is a new short living document replaced per phase.
