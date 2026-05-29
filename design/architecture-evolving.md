# Paraclete — Evolving Architecture

> **Living document.** This changes frequently as the project progresses.
> Replace this file when significant updates are made.
> Stable foundational decisions are in `architecture-core.md`.

**Last updated:** May 2026  
**Current phase:** P2 complete — P3 design in progress  
**Version:** 0.4

---

## Development Strategy

The runtime and the sequencer are built simultaneously in a co-evolution strategy. The sequencer is the framework's first and only customer until interfaces are stable. Each phase produces something independently usable.

No phase begins without its interface contracts defined. No framework capability is added without a real internal use case driving it.

---

## Development Roadmap

| Phase | Name | Deliverable | Framework Output | Status |
|---|---|---|---|---|
| **P0** | Skeleton | Empty runtime hosting one node, one audio buffer, one clock tick. | L0 audio backend, L1 runtime shell, L2 Node trait v0, NodeConfigurator/Executor split, L4 Rhai sandbox, hardware emulator target | **Complete** |
| **P1** | First Sound | Sine oscillator triggered by Launchpad emulator. Audio out. | AudioBuffer port, EventStream port, HardwareDevice trait, hardware emulator v1, HardwareMappingNode | **Complete** |
| **P2** | Sequencer v1 | 16-step Sequencer driving the oscillator. Clock domain. Basic parameter locks. LED step position feedback. | TempoSource trait, TransportEvent routing, StateBus (provisional), StatePublisher, Sequencer node | **Complete** |
| **P3** | First Instrument | Sampler node. Sequencer drives sampler. Full parameter lock protocol. StateBus scripting access. | Negotiable trait, full parameter lock protocol, Rhai StateBus bindings, StateBus lock-free upgrade | In design |
| **P4** | Launchpad Integration | Full Launchpad profile. All operations accessible without mouse. Terminal UI loop. | Default Launchpad profile script, macro system v1, StateBus → lock-free SPSC | — |
| **P5** | Sequencer v2 | Conditional trigs, micro-timing, swing, pattern chaining. Full Elektron parity. | Sub-buffer event timestamping, clock subdivision, swing implementation | — |
| **P6** | FM & Analog Synths | FM and analog-style synth nodes. Three sound sources. | GraphHost trait v1 (internal micro-graph for analog synth), FM operator DSP | — |
| **P7** | CLAP Plugin Mode | Platform runs inside Bitwig/Reaper. DAW transport syncs. Project saves. | nih-plug integration, CLAP state extension, DawHost clock domain | — |
| **P8** | CLAP Host Mode | Platform loads third-party CLAP plugins as Node. | clack integration, CLAP parameter → StateBus bridge | — |
| **P9** | Modular Graph v1 | Patchable primitive node graph. Single-sample feedback path. | GraphHost public API stable, CvSignal processing, loop-break node | — |

---

## Phase Completion Notes

### P0 — Skeleton
Six-crate workspace compiling clean. NodeExecutor/NodeConfigurator split with SPSC ring buffer. petgraph topology with cycle detection. cpal audio backend. Rhai sandbox. LaunchpadEmulator stub.

### P1 — First Sound
`LaunchpadEmulator → HardwareMappingNode → SineOscillator → AudioOutput`. Terminal keyboard triggers oscillator. Full typed `HardwareDevice` surface with `PadDescriptor`, `ButtonDescriptor` etc. `Event::Hardware` variant added. 96 tests.

### P2 — Sequencer v1
`InternalClock → Sequencer → SineOscillator`. Full naming transition applied. StateBus state bus (provisional `Arc<RwLock<HashMap>>`). LED step position feedback via StateBus. `serialize()`/`deserialize()` pattern persistence. 103 tests.

**Key architectural finding from P2:** `Sequencer` starts with `playing: false` and only begins on a `global_start` event. `InternalClock` emits `global_start: true` on its first tick. This is the correct model — the `TempoSource` signals when transport starts; downstream nodes wait for the signal rather than assuming a running state. A node connected to a running clock mid-session will wait for `global_start` before entering its pattern, then use `sync_pulse` to position itself correctly.

---

## First-Party Nodes

### `TempoSource` node (P2 — shipped)
Internal clock master. 120 BPM default. Sub-sample tick accumulator. Emits `TransportEvent` per tick at correct `sample_offset`. Emits `sync_pulse: true` on bar boundaries. Implements `Node` + `TempoSource` trait. BPM modulation input defined, wiring deferred to P9.

### `Sequencer` — Elektron-Style Sequencer

**Shipped at P2 (v1):**
- 16 steps, fixed pattern length
- Per-step: active, note, velocity, length, param locks
- Responds to `global_start`, `global_stop`, `sync_pulse`
- Publishes step position to StateBus
- `serialize()`/`deserialize()` pattern persistence

**Deferred to P5 (v2):**
- Conditional trigs: 1:2, 2:2, 1:3 … 4:4, PRE, NEI, random %
- Micro-timing: per-step offset ±23 micro-steps
- Swing: per-pattern, 50–80%
- Pattern chaining and song mode
- Variable pattern length (up to 64 steps)
- Multiple tracks with independent length and time signature
- Real-time recording

**Node API traits used:** `Node`, `StatePublisher`, `serialize()`/`deserialize()`

### Sampler (P3 — in design)
- Polyphonic sample playback with per-voice pitch, volume, pan, start/end, loop
- Slice mode: chop sample addressable by MIDI note
- All parameters lockable by the Sequencer
- First node to implement `Negotiable` fully

### FM Synthesizer (P6)
- 4 operators, 8 algorithms
- Per-operator: ratio, level, ADSR envelope, feedback
- Mod matrix: any modulation source to any parameter
- All parameters lockable

### Analog-Style Synthesizer (P6)
- 2 oscillators (saw, square, triangle, sine with PWM), noise source
- 24dB ladder filter model, 2 ADSR, 2 LFOs
- Built internally as a micro-graph — first use of `GraphHost`

### Modular Graph (P9)
- Signal, math, routing, and IO primitive nodes
- Audio-rate and modulation-rate signals distinguished by port type
- Single-sample feedback loops via explicit loop-break nodes

---

## GUI Strategy

**The GUI is a hardware controller supplement, not an editing environment.**

Scope for the foreseeable future:
- Step sequencer visualisation (which steps are active, current position)
- Parameter value monitoring (StateBus subscriber display)
- Controller emulator for development (software Launchpad surface)
- Basic node connection view

It is not a timeline editor, piano roll, audio clip editor, or mixer.

**Library decision:** egui for P4 developer tooling and parameter monitoring. Production GUI decision deferred to P6 at the earliest.

The GUI never blocks audio engine development. If the GUI is broken, the engine still works and hardware control still works.

---

## Known Provisional Implementations

These are working implementations that are deliberately not final. Each has a documented resolution path.

### StateBus — `Arc<RwLock<HashMap>>` (P2)

**Current:** The StateBus state bus is backed by `Arc<RwLock<HashMap<String, StateBusValue>>>`. The write lock is held briefly at the end of each executor cycle, after all `process()` calls complete, for a bulk HashMap insert. The main thread holds read locks only during lookups.

**Why provisional:** A `RwLock` on anything touched by the audio subsystem is not ideal. The write lock is not held *during* `process()` calls, but it is held at the end of each cycle. Under load with many publishing nodes and frequent reads, this could cause latency spikes.

**Resolution path:** Replace with a lock-free SPSC channel at P4. The executor posts `(String, StateBusValue)` pairs to the channel at end-of-cycle. The main thread drains the channel and applies updates to a locally owned `HashMap` between cycles. The main thread never contends with the audio thread. **Target: P4.**

**Rhai API stability requirement:** The Rhai StateBus bindings added at P3 must be designed against the stable *API surface* (`state_bus.read(path)`, `state_bus.write(path, value)`) not the provisional backing implementation. The API must not change when the `RwLock` is replaced with SPSC at P4. Rhai scripts written at P3 must continue to work at P4 without modification.

### `StatePublisher` as default method on `Node` (P2)

**Current:** `published_state()` is a default method on `Node` rather than a method on a separate `StatePublisher` trait object. `StatePublisher: Node` exists as a semantic marker only.

**Why provisional:** Rust trait object upcasting (`&dyn Node as &dyn StatePublisher`) requires Rust 1.76+. The project currently targets 1.75 stable.

**Resolution path:** When 1.76+ becomes the minimum compiler version, `published_state()` can be moved to `StatePublisher` as a proper method, and the executor can hold `Vec<Box<dyn StatePublisher>>` for publishers only. **No API change for node authors — the method signature stays the same.**

---

## Open Questions

These are unresolved. Recorded here so they are answered deliberately, not accidentally. Each will become an ADR when resolved.

| # | Question | Blocking Phase | Notes |
|---|---|---|---|
| OQ-1 | GUI library for production UI | P6 | egui confirmed for P4. Full production GUI deferred. Vizia, iced, custom all remain options. |
| OQ-2 | State bus persistence boundary | P3 | Node-owned blobs vs StateBus snapshot at project save. Hybrid seems right — `serialize()` for structured state, StateBus snapshot for observable values. Boundary unspecified. |
| OQ-3 | MIDI 2.0 CI depth in v1 | P3 | How much of MIDI-CI protocol ships with the Sampler vs stubbed? Full spec is complex. Negotiable trait at P3 forces a decision. |
| OQ-4 | Network / distributed nodes | P9 | Plan 9 vision implies remote nodes. Not blocking any current phase but shapes EventStream serialisation format. |
| OQ-5 | Single-sample feedback loop break model | P9 | Explicit loop-break nodes (VCV Rack approach) vs automatic latency compensation. Explicit preferred — not yet documented as ADR. |
| OQ-6 | Micro-timing clock representation | P5 | Elektron micro-timing is step-internal offsets. The `Sequencer` P5 implementation will determine whether this fits the `TransportEvent` model or needs a sequencer-local offset field. |
| OQ-7 | Oversampling strategy | P9 | No oversampling at P0–P8. At P9 when `CvSignal` audio-rate modulation is introduced, aliasing becomes relevant. Options: no oversampling, optional per-node, boundary-only. |
| OQ-8 | StateBus SPSC design | P4 | Lock-free SPSC replacement for `RwLock<HashMap>`. API must be stable before P3 Rhai bindings are written. See provisional implementations section. |

---

## Resolved Open Questions

| Question | Resolution | ADR / Note |
|---|---|---|
| Language | Rust | ADR-002 |
| License | GPL3 (platform) / LGPL3 (L2 Node API) | ADR-001 |
| Plugin format | CLAP primary, VST3 secondary | ADR-003 |
| Signal model | Mixed-rate typed ports | ADR-004 |
| Scheduler cycle support | Must support cycles from P0; explicit loop-break node at P9 | ADR-005 |
| Hardware emulator | Required at P1, software simulation of controller surface | ADR-006 |
| Scripting runtime | Rhai | ADR-007 |
| Node API level | Single trait, three implicit levels via defaults | ADR-008 |
| Clock ownership | Federated domains, no mandatory owner | ADR-009 |
| State model | Cybernetic hybrid — private node state + StateBus | ADR-010 |
| Executor model | Single-threaded push, partition model deferred | ADR-011 |
| Buffer allocation | Per-connection, main thread only | ADR-012 |
| Event model | Typed enum with ExtendedEventSlab slab | ADR-013 |
| Capability documents | Mandatory with defaults, regenerable | ADR-014 |
| Connection negotiation | Connection-time, no global space_id registry | ADR-015 |
| TempoSource start signal | InternalClock emits global_start on first tick. Downstream nodes wait for this before entering pattern. sync_pulse positions them correctly. | P2 implementation finding |
| PortType::Clock routing | Routed identically to PortType::Event — TransportEvent is a TimedEvent | P2 implementation finding |
