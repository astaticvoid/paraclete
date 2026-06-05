# Paraclete — Roadmap

> **Living document.** Replace this file when a phase completes or significant
> planning changes occur. Keep it short — current state only.
>
> **Last updated:** June 2026
> **Current phase:** P7 in design

---

## Roadmap

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0** | Skeleton | Empty runtime, one node, one clock tick | **Complete** |
| **P1** | First Sound | Sine oscillator triggered by Launchpad emulator | **Complete** |
| **P2** | Sequencer v1 | 16-step sequencer, clock domain, LED feedback | **Complete** |
| **P3** | First Instrument | Sampler, parameter locks, StateBus SPSC, Rhai bindings | **Complete** |
| **P4** | Hardware Integration | 3-device setup live. Launchpad = 8-track drum machine. XL = contextual params. Keystep = melodic. DistortionNode + FilterNode. | **Complete** |
| **P5** | Depth | Sequencer v2 (conditional trigs, micro-timing, swing, multi-pattern). ReverbNode, DelayNode, SplitNode, MixNode. | **Complete** |
| **P6** | Synthesis | FM synth, analog-style synth. High-quality resampling. GraphHost v1. Broader audio format support. | **Complete** |
| **P6.5** | Cleanup | EnvelopeNode looping AD fix. LadderFilter param rename. Signal port executor allocation. ADR-024, ADR-025 written. | **Complete** |
| **P7** | DAW Integration | CLAP plugin mode. DAW transport sync. Project save/recall. paraclete-node-api ships to crates.io. Per-voice rubato. OQ-9. | **In design** |
| **P8** | Instrument Layer | Instrument definition file. Terminal UI. CLAP Host. | — |
| **P9** | Modular Graph | Patchable primitive node graph. Single-sample feedback. Sequencer as CV source. Nested sequencers. | — |

---

## P7 Scope (current)

**Deliverable:** CLAP plugin mode and project save/recall. paraclete-node-api v0.1.0 on crates.io.

- `paraclete-clap` crate: `SingleNodePlugin` and `SubgraphPlugin` (hand-rolled, no nih-plug)
- Machine bank plugins: one `.clap` per machine variant (Kick, Snare, FmKick, FmBell, FmBass)
- DAW transport sync: CLAP transport → `TransportInfo` / `TransportEvent`
- Project save/recall: RON format, per ADR-025
- Per-voice rubato pitch resampling in Sampler (deferred from P6)
- `published_state()` push-down — pre-allocated, zero audio-thread allocation (OQ-9 resolved)
- `paraclete-node-api` v0.1.0 published to crates.io

---

## P8 Scope (next)

**Deliverable:** The instrument is defined in a file, not in Rust. The terminal is the display.

- **Instrument definition file** — YAML (or equivalent) declares graph topology, machine
  assignments, initial parameter values, and macro bindings at startup. Operators configure
  their instrument without touching Rust. See ADR-026.
- **Terminal UI** (`paraclete-tui`) — page-based, Elektron-style live feedback: active
  parameter page, encoder-to-parameter mappings, values as they change, step sequencer state.
  The terminal is the primary operator feedback surface, prioritised over any graphical UI.
  Profile scripts call `publish_context()` to tell the TUI what each encoder controls.
  See ADR-026.
- **CLAP Host** — platform loads third-party CLAP plugins as nodes.

---

## Known Provisional Implementations

| Item | Status | Resolution | Target |
|---|---|---|---|
| StatePublisher as default method on Node | Active | Rust 1.76+ trait upcasting | When 1.76+ is minimum |
| published_state() allocates per cycle (OQ-9) | **Resolving at P7** | Pre-allocated push-down (p7-interfaces.md Commit 1) | P7 |
| Per-voice rubato resampling in Sampler | **Resolving at P7** | rubato SincFixedOut per voice (p7-interfaces.md Commit 2) | P7 |
| Graph topology hardcoded in Rust | Active | Instrument definition file | P8 |

---

## Open Questions

| # | Question | Blocking | Notes |
|---|---|---|---|
| OQ-1 | Operator display surface | P8 | Terminal UI is the answer. egui dev tooling remains separate. See ADR-026. |
| OQ-3 | MIDI 2.0 CI depth | P8 | Negotiable trait shipped at P3 with no CI. Deferred; not blocking P7. |
| OQ-4 | Network / distributed nodes | P9 | Shapes EventStream serialisation format. Not blocking. |
| OQ-5 | Single-sample feedback loop break model | P9 | Explicit loop-break nodes preferred. ADR needed at P9. |
| OQ-6 | Micro-timing clock representation | P8 | Signed micro-timing still deferred. |
| OQ-7 | Oversampling strategy | P9 | No oversampling until CvSignal audio-rate modulation ships. |
| OQ-10 | XL Mk3 query protocol integration | P4/Mk3 | CC channel 7/8 protocol for bidirectional sync. Implement when Mk3 arrives. |

---

## Resolved Open Questions

| Question | Resolution | ADR / Phase |
|---|---|---|
| Language | Rust | ADR-002 |
| License | GPL3 platform / LGPL3 Node API | ADR-001 |
| Plugin format | CLAP primary, VST3 secondary | ADR-003 |
| Signal model | Mixed-rate typed ports | ADR-004 |
| Scheduler cycles | petgraph from P0, loop-break at P9 | ADR-005 |
| Hardware emulator | Required at P1 | ADR-006 |
| Scripting runtime | Rhai | ADR-007 |
| Node API level | Single trait, three implicit levels | ADR-008 |
| Clock ownership | Federated domains | ADR-009 |
| State model | Cybernetic hybrid | ADR-010 |
| Executor model | Single-threaded push, partition deferred | ADR-011 |
| Buffer allocation | Per-connection, main thread | ADR-012 |
| Event model | Typed enum with ExtendedEventSlab | ADR-013 |
| Capability documents | Mandatory with defaults, regenerable | ADR-014 |
| Connection negotiation | Connection-time, no global registry | ADR-015 |
| Multi-track sequencer | Multiple node instances | ADR-016 |
| Effect node architecture | Plain nodes, AudioEffect marker only | ADR-017 |
| TempoSource start | InternalClock emits global_start on first tick | P2 finding |
| TransportEvent routing | Same path as Event | P2 finding |
| Knob sync (XL Mk2) | Calibration gesture on connect | P4 design |
| Knob sync (XL Mk3) | Query protocol CC ch 7/8; profile script | OQ-10 |
| WAV-only format support | symphonia crate (WAV/FLAC/AIFF/OGG/MP3) | P6 |
| State bus persistence boundary (OQ-2) | serialize() for node state; RON project file for aggregation; state bus is not persisted | ADR-025 |
| CLAP plugin wrapper approach | Hand-rolled adapter; nih-plug removed | ADR-024 |
| Instrument definition format | YAML-based startup-only topology file | ADR-026 |
| Operator feedback surface (OQ-1) | Terminal UI as primary display; profile scripts publish context | ADR-026 |
