# Paraclete — Roadmap

> **Living document.** Replace this file when a phase completes or significant
> planning changes occur. Keep it short — current state only.
>
> **Last updated:** May 2026  
> **Current phase:** P3 complete — P4 in design

---

## Roadmap

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0** | Skeleton | Empty runtime, one node, one clock tick | **Complete** |
| **P1** | First Sound | Sine oscillator triggered by Launchpad emulator | **Complete** |
| **P2** | Sequencer v1 | 16-step sequencer, clock domain, LED feedback | **Complete** |
| **P3** | First Instrument | Sampler, parameter locks, StateBus SPSC, Rhai bindings | **Complete** |
| **P4** | Hardware Integration | 3-device setup live. Launchpad = 8-track drum machine. XL = contextual params. Keystep = melodic. DistortionNode + FilterNode. | **In design** |
| **P5** | Depth | Sequencer v2 (conditional trigs, micro-timing, swing, multi-pattern). ReverbNode, DelayNode, SplitNode, MixNode. | — |
| **P6** | Synthesis | FM synth, analog-style synth. High-quality resampling. GraphHost v1. Broader audio format support. | — |
| **P7** | DAW Integration | CLAP plugin mode. DAW transport sync. Project save. paraclete-node-api ships to crates.io. | — |
| **P8** | CLAP Host | Platform loads third-party CLAP plugins as nodes. | — |
| **P9** | Modular Graph | Patchable primitive node graph. Single-sample feedback. Sequencer as CV source. Nested sequencers. | — |

---

## P4 Scope

**Deliverable:** Sit down with the three devices and make a beat. Mouse never touched.

**Hardware:**
- Novation Launchpad — 8-track XOX/Digitakt-style drum machine
- Launch Control XL Mk2 — contextual parameter control (calibration gesture on connect)
- Arturia Keystep 37 — melodic/bassline input over drum tracks

**Nodes shipping at P4:**
- 8 `Sequencer` instances (ADR-016 — multi-track is graph topology)
- 8 `Sampler` instances (one per track)
- `DistortionNode` — drive/saturation
- `FilterNode` — state-variable filter
- `HardwareMappingNode` extended for multi-track routing
- Launch Control XL HAL node (Mk2 profile + Mk3 architecture)
- Keystep HAL node

**Framework output:**
- Full Launchpad profile script (Rhai)
- XL profile script — contextual, follows track selection
- Keystep profile script
- Macro system v1
- Hardware update_output() moved to main thread
- Connected signal port views (modulation() for wired ports)
- EncoderBehaviour enum (Absolute/Relative) in EncoderDescriptor
- egui developer window (state bus monitor, node connection view)

---

## Known Provisional Implementations

| Item | Status | Resolution | Target |
|---|---|---|---|
| StatePublisher as default method on Node | Active | Rust 1.76+ trait upcasting | When 1.76+ is minimum |
| published_state() allocates per cycle (OQ-9) | Active | Pre-allocated push-down | P6 |
| Linear interpolation resampler in Sampler | Active | rubato crate | P6 |
| WAV-only format support | Active | symphonia crate | P6 |
| Hardcoded `for pad in 0..16` in executor | Active | Surface descriptor iteration | P4 |

---

## Open Questions

| # | Question | Blocking | Notes |
|---|---|---|---|
| OQ-1 | GUI library for production UI | P6 | egui for P4 dev tooling only. Full GUI deferred. |
| OQ-2 | State bus persistence boundary | P7 | serialize() for structured state + StateBus snapshot for observable values. Boundary unspecified. Deferred from P3 — not blocking until P7 save/recall. |
| OQ-3 | MIDI 2.0 CI depth | P5 | Negotiable trait shipped at P3 with no CI. Real hardware integration at P5 forces a decision. |
| OQ-4 | Network / distributed nodes | P9 | Shapes EventStream serialisation format. Not blocking. |
| OQ-5 | Single-sample feedback loop break model | P9 | Explicit loop-break nodes preferred. ADR needed at P9. |
| OQ-6 | Micro-timing clock representation | P5 | Step-internal offsets vs TransportEvent model. P5 sequencer forces decision. |
| OQ-7 | Oversampling strategy | P9 | No oversampling until CvSignal audio-rate modulation ships. |
| OQ-9 | published_state() allocation per cycle | P6 | Pre-allocated push-down approach needed at scale. |
| OQ-10 | XL Mk3 query protocol integration | P4/Mk3 | CC channel 7/8 protocol for bidirectional sync. Implement when Mk3 arrives. Profile script handles it. |

---

## Resolved Open Questions

| Question | Resolution | ADR |
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
| Multi-track sequencer | Multiple node instances (ADR-016) | ADR-016 |
| Effect node architecture | Plain nodes, AudioEffect marker only | ADR-017 |
| TempoSource start | InternalClock emits global_start on first tick | P2 finding |
| TransportEvent routing | Same path as Event | P2 finding |
| Knob sync (XL Mk2) | Calibration gesture on connect | P4 design |
| Knob sync (XL Mk3) | Query protocol CC ch 7/8; profile script | OQ-10 |
