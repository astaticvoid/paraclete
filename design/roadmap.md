# Paraclete — Roadmap

> **Living document.** Replace this file when a phase completes or significant
> planning changes occur. Keep it short — current state only.
> 
> **Last updated:** June 2026  
> **Current phase:** P5 complete — P6 next

-----

## Roadmap

|Phase   |Name                   |Deliverable                                                                                                                                                     |Status       |
|--------|-----------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------|
|**P0**  |Skeleton               |Empty runtime, one node, one clock tick                                                                                                                         |**Complete** |
|**P1**  |First Sound            |Sine oscillator triggered by Launchpad emulator                                                                                                                 |**Complete** |
|**P2**  |Sequencer v1           |16-step sequencer, clock domain, LED feedback                                                                                                                   |**Complete** |
|**P3**  |First Instrument       |Sampler, parameter locks, StateBus SPSC, Rhai bindings                                                                                                          |**Complete** |
|**P4**  |Hardware Integration   |3-device setup live. Launchpad = 8-track drum machine. XL = contextual params. Keystep = melodic. DistortionNode + FilterNode.                                  |**Complete** |
|**P4.5**|Foundation Verification|Fix four blocking gaps (LED routing, device ID tagging, Sampler ParameterBank, audio_inputs wiring). Integration test.                                          |**Complete** |
|**P5**  |Depth                  |Sequencer v2 (conditional trigs, micro-timing, swing). ReverbNode, DelayNode, SplitNode. Signal port buffer wiring.                                             |**Complete** |
|**P6**  |Synthesis              |Synthesis primitive study. FM engine, analog emulation engine. Machine variants. High-quality resampling. Broader audio format support.                         |—            |
|**P7**  |DAW Integration        |CLAP plugin mode. Minimal host adapter. Machine bank plugins. DAW transport sync. Project save/recall. paraclete-node-api ships to crates.io.                   |—            |
|**P8**  |CLAP Host              |Platform loads third-party CLAP plugins as nodes. Third-party machine slots.                                                                                    |—            |
|**P9**  |Modular Graph          |GraphNode instrument encapsulation. Patchable primitive graph. Runtime machine slot swapping. Single-sample feedback. Nested sequencers. Sequencer as CV source.|—            |

-----

## P6 Scope (next)

**Deliverable:** The platform can synthesize sounds, not just play samples. Machine
variant pattern established. Plugin extraction path validated.

**Pre-P6 synthesis primitive study:** Systematic review of reference instrument
architectures to extract the complete primitive node vocabulary — every filter
topology, envelope shape, LFO mode, oscillator type, and routing option needed to
compose any reference instrument topology without gaps. Output is an internal design
document driving P6 node design. See ADR-023.

**Synthesis nodes:**

- Primitive nodes — oscillators, envelopes (ADSR, AD, looping), filters (multimode
  SVF, ladder), LFOs (sine, square, ramp, random), operators (FM)
- `AnalogEngine` — analog voice emulation. Machine variants: KickMachine,
  SnareMachine, HiHatMachine
- `FmEngine` — operator-based FM synthesis. Machine variants: FmKickMachine,
  FmBellMachine, FmBassMachine

**Machine variant pattern:** Named engine configurations with constrained
`capability_document()` surfaces. Full engine depth available via
`ConnectionAgreement`. See ADR-022.

**Audio quality:**

- High-quality resampling (rubato crate) replacing linear interpolation in Sampler
- Broader audio format support (symphonia crate) replacing WAV-only

-----

## Long-Range Vision

The end goal is an open-source hardware-first instrument platform where complete
instrument topologies — 8-track drum machines, 4-voice FM synthesizers, 12-track
machines with swappable synthesis engines, analog voice instruments — exist as named
`GraphNode` configurations: single nodes whose inner graphs are complete synthesis
engines, exposing focused performer workflow surfaces to the outside world.

These instrument configurations ship as standalone CLAP plugins (P7), load third-party
synthesis engines as machine slots (P8), and become fully patchable with runtime
machine swapping and nested sequencing (P9).

The composition hierarchy:

```
Primitive nodes (P6)
    oscillators, envelopes, filters, LFOs, operators
        ↓
Generator nodes (P6) — portable, machine-configurable (ADR-022)
    AnalogEngine, FmEngine
        ↓
Machine variants (P6/P7)
    named configurations with constrained capability surfaces
        ↓
GraphNode instruments (P9) — ADR-023
    inner graph = machine bank + sequencers + mixer
    outer surface = performer workflow parameters
        ↓
Standalone CLAP plugins (P7+)
    any subgraph wrapped in the minimal host adapter
```

Each layer is useful independently. No layer requires the layers above it.

See ADR-022 (node portability), ADR-023 (instrument encapsulation).

-----

## Known Provisional Implementations

|Item                                        |Status|Resolution                       |Target               |
|--------------------------------------------|------|---------------------------------|---------------------|
|StatePublisher as default method on Node    |Active|Rust 1.76+ trait upcasting       |When 1.76+ is minimum|
|NodeOrDevice enum in executor               |Active|Rust 1.76+ trait object upcasting|When 1.76+ is minimum|
|published_state() allocates per cycle (OQ-9)|Active|Pre-allocated push-down          |P6                   |
|Linear interpolation resampler in Sampler   |Active|rubato crate                     |P6                   |
|WAV-only format support                     |Active|symphonia crate                  |P6                   |
|Mutex<VecDeque> in hardware nodes (S-005)   |Active|SPSC ring buffer                 |P5                   |
|published_state() dirty flag missing (S-008)|Active|Per-node dirty flag              |P5                   |
|Multi-pattern bank storage                  |Active|Stub only at P5                  |P6                   |

-----

## Open Questions

|#    |Question                               |Blocking|Notes                                                                                                 |
|-----|---------------------------------------|--------|------------------------------------------------------------------------------------------------------|
|OQ-1 |GUI library for production UI          |P6      |egui for dev tooling only. Full GUI deferred.                                                         |
|OQ-2 |State bus persistence boundary         |P7      |serialize() for structured state + StateBus snapshot. Boundary unspecified until P7 save/recall.      |
|OQ-3 |MIDI 2.0 CI depth                      |P5      |Negotiable trait shipped at P3 with no CI. Real hardware integration at P5 forces a decision.         |
|OQ-4 |Network / distributed nodes            |P9      |Shapes EventStream serialisation format. Not blocking.                                                |
|OQ-5 |Single-sample feedback loop break model|P9      |Forced by GraphNode nested executor scheduling. Explicit loop-break nodes preferred. ADR needed at P9.|
|OQ-7 |Oversampling strategy                  |P9      |No oversampling until CvSignal audio-rate modulation ships.                                           |
|OQ-9 |published_state() allocation per cycle |P6      |Pre-allocated push-down approach needed at scale.                                                     |
|OQ-10|XL Mk3 query protocol integration      |P4/Mk3  |CC channel 7/8 protocol for bidirectional sync. Implement when Mk3 arrives. Profile script handles it.|

-----

## Resolved Open Questions

|Question                          |Resolution                                         |ADR / Reference|
|----------------------------------|---------------------------------------------------|---------------|
|Language                          |Rust                                               |ADR-002        |
|License                           |GPL3 platform / LGPL3 Node API                     |ADR-001        |
|Plugin format                     |CLAP primary, VST3 secondary                       |ADR-003        |
|Signal model                      |Mixed-rate typed ports                             |ADR-004        |
|Scheduler cycles                  |petgraph from P0, loop-break at P9                 |ADR-005        |
|Hardware emulator                 |Required at P1                                     |ADR-006        |
|Scripting runtime                 |Rhai                                               |ADR-007        |
|Node API level                    |Single trait, three implicit levels                |ADR-008        |
|Clock ownership                   |Federated domains                                  |ADR-009        |
|State model                       |Cybernetic hybrid                                  |ADR-010        |
|Executor model                    |Single-threaded push, partition deferred           |ADR-011        |
|Buffer allocation                 |Per-connection, main thread                        |ADR-012        |
|Event model                       |Typed enum with ExtendedEventSlab                  |ADR-013        |
|Capability documents              |Mandatory with defaults, regenerable               |ADR-014        |
|Connection negotiation            |Connection-time, no global registry                |ADR-015        |
|Multi-track sequencer             |Multiple node instances                            |ADR-016        |
|Effect node architecture          |Plain nodes, AudioEffect marker only               |ADR-017        |
|Cellular architecture             |Node is the universal primitive                    |ADR-018        |
|Universal parameter control       |ParameterBank + CMD_SET_PARAM/BUMP_PARAM           |ADR-019        |
|Launchpad X profile layout        |Two-mode Digitakt-inspired surface                 |ADR-020        |
|Launchpad X protocol              |Programmer Mode via MIDI port; init sequence       |ADR-021        |
|Node portability rule             |Nodes link L2 only; portable to any host           |ADR-022        |
|Instrument encapsulation          |GraphNode; machine variants; synthesis study       |ADR-023        |
|TempoSource start                 |InternalClock emits global_start on first tick     |P2 finding     |
|TransportEvent routing            |Same path as Event                                 |P2 finding     |
|Knob sync (XL Mk2)                |Calibration gesture on connect                     |P4 design      |
|Knob sync (XL Mk3)                |Query protocol CC ch 7/8; profile script           |OQ-10          |
|Micro-timing representation (OQ-6)|sample_offset on events; StepTiming i8 at 1/96 beat|P5 design      |