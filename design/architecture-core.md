# Paraclete — Core Architecture

> **Stable document.** This covers the foundational decisions that are unlikely to change.
> For the evolving roadmap and open questions see `architecture-evolving.md`.
> For individual decisions see `decisions/`.

-----

## Vision

This platform is not a DAW. It is not a plugin. It is a runtime for musical computation — a living environment where instruments, sequencers, controllers, and effects are composable nodes in a graph, communicating over well-defined protocols.

The closest analogies are not musical. They are architectural:

- **Plan 9** — everything is a node. Nodes advertise capabilities and negotiate protocols. The graph is the universal interface. Small, composable, combinable to arbitrary depth.
- **Emacs** — the platform is a live, introspectable, reprogrammable runtime. Bindings are programs. The user is always a developer of their own instrument. Nothing requires a restart.
- **Elektron** — hardware-first workflow. Parameter locks, trigs, macro knobs. The immediacy of physical interaction. Muscle memory over menus. The device is the instrument.

The result: an open-source Maschine that runs standalone or as a CLAP plugin inside a DAW, built around a modular signal graph as deep as Reaktor, with a hardware-first interface model where the mouse is never required.

-----

## Primary Goals

- Mouse-free operation from any supported hardware controller
- Composable signal graph: audio, CV, events, and modulation as typed ports
- Capability-negotiating nodes: Plan 9-style rich APIs, not passive data processors
- Live reprogrammability: mappings, macros, and behaviors editable at runtime
- Incremental shipability: every phase produces something usable and standalone
- True cross-platform parity: Linux is not an afterthought
- Open hardware extensibility: any controller can become a first-class instrument

-----

## Stack Summary

|                  |                                                                      |
|------------------|----------------------------------------------------------------------|
|**Language**      |Rust (stable toolchain)                                               |
|**Plugin Format** |CLAP (primary), VST3 (secondary)                                      |
|**MIDI**          |MIDI 2.0 / UMP first-class, MIDI 1.0 compatible                       |
|**Platforms**     |Linux, macOS, Windows — equal priority                                |
|**Audio Backends**|PipeWire/JACK (Linux), CoreAudio (macOS), WASAPI (Windows) via cpal   |
|**License**       |GPL3 (platform, runtime, first-party nodes) / LGPL3 (L2 Node API only)|
|**Scripting**     |Rhai (embedded, sandboxed, Rust-native)                               |

-----

## Design Principles

These are binding constraints, not guidelines. Every architectural and API decision is evaluated against them. Violations must be documented with a resolution plan.

### 1. Stub Deep, Ship Shallow

Interfaces are designed for full depth from day one. Implementations ship at the level needed to be useful. No interface decision forecloses a future capability. A stubbed capability is acceptable. A foreclosed capability is a design failure.

### 2. Unix Pipes / Plan 9 Nodes

Everything is a composable node with well-defined typed ports. Nodes advertise capabilities and negotiate protocols. The graph is the universal interface. Nodes are agents, not passive processors. Composition is the primary mode of construction at every layer.

**There is no legitimate second class of platform object.** Any design that appears to require a component outside the graph is a signal that the Node API needs extension — not a reason to create an exception. The scripting engine is the one acknowledged exemption: it is the live environment within which nodes operate, not a node within it. See ADR-018.

### 3. Cellular, Cybernetic Architecture

The platform is a **cellular** system: each node is a cell with defined inputs and outputs. Cells communicate through connections. The whole emerges from composition. Nothing is hardwired at the structural level. The runtime is the connective tissue — it routes and schedules; it does not decide.

Computation happens at the edges. The runtime is a nervous system — it orchestrates without controlling. Dumb core, smart nodes. Central orchestration is available but never imposed. Local intelligence is always respected. The system degrades gracefully when the center is absent.

### 4. Incremental Shipability

Every layer must produce something usable independently. MVP is not a compromise — it is a vertical slice of the full vision that stands on its own. Features ship when they are useful, not when the system is complete.

### 5. Clock Domain Federation

There is no single mandatory clock owner. The runtime maintains a primary domain but respects and translates between sub-domains and external clock sources. Nodes declare clock relationships — they do not inherit them blindly. Polyrhythm, drift, and external sync are features, not edge cases.

### 6. Eat Your Own Cooking

The platform’s own first-party features are the framework’s first and primary customers. No framework API exists without a real internal use case driving it. The Elektron sequencer is the framework’s first stress test. If it cannot be built cleanly on the public API, the API is wrong.

### 7. Hardware Is a First-Class Node

Controllers are not peripheral. They are graph participants with the same standing as any audio or sequencer node. Any parameter, state, or event in the system must be reachable from hardware. Success is defined as: mouse never required. The GUI reflects hardware state — hardware does not mirror the GUI.

### 8. The Platform Is a Live Runtime

Inspired by Emacs: the system is introspectable and reprogrammable while running. Mappings, macros, behaviors, and node configurations are redefinable at runtime without restart. The user is always a developer of their own instrument. The scripting layer is a programming environment.

-----

## Layer Model

The platform is organized into five layers. Each layer has a strict contract with adjacent layers. No layer reaches across another layer’s boundary.

|Layer |Name                |Responsibility                                                                                                                            |
|------|--------------------|------------------------------------------------------------------------------------------------------------------------------------------|
|**L0**|HAL                 |Hardware Abstraction Layer. Audio I/O, MIDI I/O, controller I/O. Platform-specific backends hidden behind uniform Rust traits.            |
|**L1**|Runtime             |The nervous system. Graph scheduling, clock federation, state bus, node lifecycle, capability negotiation. No DSP logic.                  |
|**L2**|Node API            |The contract every node implements. Typed ports, event queues, capability declaration, state publication. Versioned and stable. **LGPL3.**|
|**L3**|Node Implementations|First-party and third-party nodes. Sequencer, sampler, synth engines, effects, controllers. All built exclusively on L2.                  |
|**L4**|Scripting & Binding |Live runtime scripting (Rhai). Hardware mappings, macros, behavioral extensions. Reaches L2 API only.                                     |

### License boundary

L2 (Node API) is **LGPL3**. Everything else is **GPL3**. Third parties implementing L2 traits can keep their node implementations closed-source. Third parties modifying L1, L0, or L4 must open their changes.

-----

## The Runtime (L1) — Nervous System

The runtime’s job is matchmaking and scheduling. It never makes musical decisions.

**Owns:**

- Graph topology — which nodes exist, how their ports are connected
- Master clock domain — the primary tempo/position reference
- State bus — publish/subscribe address space for observable values
- Node lifecycle — spawn, suspend, terminate, serialize/deserialize
- Capability negotiation facilitation

**Does NOT own:**

- DSP logic of any kind
- Musical or sequencing decisions
- UI state
- Device-specific behavior
- Controller mapping logic

This boundary is strictly enforced. Any pressure to add musical logic to the runtime is a signal that the Node API needs to be extended instead.

### Configurator / Executor split

The runtime uses a two-sided design (derived from HexoDSP’s pattern, independently implemented):

- **NodeConfigurator** — runs on the main thread. Accepts graph changes, parameter updates, new node connections. Pre-allocates all messages.
- **NodeExecutor** — runs on the audio thread. Receives changes via a lock-free ring buffer. Executes the graph. Never blocks.

No mutex is ever taken on the audio thread. This is a hard constraint.

-----

## Node API (L2) — Universal Contract

Every node implements a base trait contract. Optional capability traits extend this base.

```
Node            — base trait, every node implements this
AudioProcessor  — produces/consumes audio buffers at block rate
EventProcessor  — receives/emits timestamped events
CvProcessor     — produces/consumes CV signals at audio rate
GraphHost       — can host child nodes (v2 API)
TempoSource     — can provide a clock domain
Negotiable      — participates in capability handshake
StatePublisher  — publishes named values to the state bus
HardwareDevice  — declares physical controls and their semantics
Scriptable      — exposes entry points callable from L4
```

The runtime inspects declared traits to determine scheduling strategy, connection compatibility, and negotiation protocol.

-----

## Signal Types — Typed Ports

Connections between nodes are typed. The runtime enforces type compatibility at connection time.

|Type             |Description                                                       |
|-----------------|------------------------------------------------------------------|
|`AudioBuffer`    |Block-rate stereo or multi-channel audio                          |
|`MonoAudioBuffer`|Block-rate mono audio                                             |
|`CvSignal`       |Audio-rate single-channel CV. Enables feedback loops.             |
|`ControlRate`    |Modulation-rate parameter value. LFOs, envelopes, automation.     |
|`EventStream`    |Timestamped event queue. Notes, MIDI 2.0 UMP, custom events.      |
|`ClockSignal`    |Tempo and position. BPM, beat position, time signature, domain ID.|

**Important:** `CvSignal` is designed for single-sample processing and feedback loops. The scheduler must support cyclic graphs for this type. V1 ships block-rate only but the scheduler’s topology model must not assume a DAG. See ADR-005.

-----

## State Bus

The state bus is the platform’s shared observable namespace. It is not a database — it is a publish/subscribe bus with a hierarchical address scheme.

```
/transport/bpm
/transport/position/beat
/transport/position/bar
/node/{id}/param/{name}
/node/{id}/state/{key}
/hw/{device-id}/pad/{n}/velocity
/hw/{device-id}/knob/{n}/value
```

**Ownership rules:**

- Runtime owns `/transport/*`
- Nodes own `/node/{their-id}/*`
- Hardware nodes own `/hw/{their-id}/*`
- No node can publish to an address it does not own
- Any node or UI element can subscribe to any address

-----

## Clock Domain Federation

The runtime maintains a primary clock domain. Additional domains can be registered by any `TempoSource` node. Events crossing domain boundaries are timestamped in both coordinate systems.

**Priority (descending):**

1. DAW host transport (when running as CLAP plugin)
1. Ableton Link (when active)
1. External hardware clock (CV or MIDI clock input)
1. Internal transport (standalone default)
1. Node-local sub-domain (polyrhythmic sequencer, etc.)

Ownership is negotiated, not assigned. Domains never fight — they federate.

-----

## Hardware Abstraction Layer (L0)

The HAL is the only layer that knows about the physical world. All hardware appears as graph nodes above the HAL boundary.

**Responsibilities:**

- Audio I/O via cpal (JACK/PipeWire, CoreAudio, WASAPI)
- MIDI I/O via midir. MIDI 2.0 UMP primary. MIDI 1.0 translated at the boundary.
- Generic controller I/O: HID, proprietary USB, OSC
- CV I/O: DC-coupled audio interface channels as CV ports
- Clock I/O: MIDI clock, DIN sync, analog clock pulse

### Controller node model

A controller implements `HardwareDevice`. It:

- Publishes its physical controls to the state bus
- Emits events into the graph
- Subscribes to output values (LED colors, display content, motor fader positions)

The HAL node has no musical opinions. Mapping is the scripting layer’s job.

### Hardware emulator

Every hardware controller must have a software emulator target. The emulator:

- Simulates the full surface (pads, buttons, encoders, LEDs)
- Responds to keyboard/mouse input in a desktop window
- Is usable in CI for automated testing without physical hardware

This is a P1 deliverable, not an afterthought. See ADR-006.

### Primary development controller

**Novation Launchpad** — 8×8 RGB pad grid, scene launch buttons, track control buttons, mode buttons, full RGB LED feedback.

**Shipped profiles:** Launchpad (all variants), Launchpad Pro, Ableton Push 2/3, NI Maschine (controller mode), Akai APC series, generic MIDI fallback.

-----

## Scripting & Binding Layer (L4)

The platform’s Emacs Lisp equivalent. A live, evaluated environment running inside the platform process. Scripts are programs, not config files.

### Runtime: Rhai

Chosen for: no FFI boundary, strong Rust type integration, sandboxed execution, Don’t Panic guarantee, surgically configurable as a DSL.

Scripts run exclusively off the audio thread.

**Scripts can:**

- Read and write authorized state bus addresses
- Call `Scriptable` entry points on nodes
- Subscribe to and emit events
- Define and redefine hardware mappings
- Create and destroy macro definitions
- Query node capability documents

**Scripts cannot:**

- Access the filesystem directly (platform-mediated API only)
- Spawn threads
- Access L0/L1 internals
- Block the audio thread

### Mapping model

A mapping is a script defining the relationship between a hardware control and a platform action. Mappings are live — redefinable without restart. They are first-class objects with identity, metadata, and versioning.

This is not MIDI learn. It is a programmable event router with state, context awareness, and compound behavior.

### Macro system

Macros are named sequences of actions bound to any hardware control or keyboard shortcut. They can set multiple parameters atomically, trigger compound actions, conditionally branch, and record/replay automation.

-----

## Plugin Host & Guest Duality

### As a CLAP plugin (guest mode)

- Audio I/O connects to DAW audio routing
- DAW MIDI routing connects to platform event bus
- DAW transport offered as clock domain; platform can accept or override
- Full platform state serialized in DAW project via CLAP state extension
- Platform renders its own GUI in the DAW plugin window

### As a CLAP host (host mode)

- CLAP plugin parameters published to state bus
- CLAP note ports connect to platform event bus
- CLAP audio ports connect as typed AudioBuffer ports
- Sequencer can parameter-lock any CLAP plugin parameter
- Hardware mappings can reach any CLAP parameter

-----

## Crate Workspace Structure

Single workspace until Phase 5. Crates extracted to independent repositories only when their API is proven stable through at least two phases of use.

```
paraclete/
├── Cargo.toml          (workspace)
├── crates/
│   ├── paraclete-hal/        (L0 — GPL3)
│   ├── paraclete-runtime/    (L1 — GPL3)
│   ├── paraclete-node-api/   (L2 — LGPL3)
│   ├── paraclete-nodes/      (L3 first-party — GPL3)
│   ├── paraclete-scripting/  (L4 — GPL3)
│   └── paraclete-app/        (standalone binary — GPL3)
├── docs/
│   ├── architecture-core.md       (this file)
│   ├── architecture-evolving.md
│   ├── prior-art-analysis.md
│   └── decisions/
│       ├── ADR-001-license.md
│       ├── ADR-002-language.md
│       └── ...
└── profiles/           (hardware controller scripts)
    ├── launchpad.rhai
    └── launchpad-pro.rhai
```

-----

## Key Crate Dependencies

|Crate           |Layer|Purpose                            |License   |
|----------------|-----|-----------------------------------|----------|
|`nih-plug`      |L2/L3|CLAP + VST3 plugin wrapper         |MIT/GPL3  |
|`clack`         |L1/L3|Safe CLAP host bindings            |MIT       |
|`cpal`          |L0   |Cross-platform audio I/O           |Apache-2  |
|`midir`         |L0   |Cross-platform MIDI I/O            |MIT       |
|`midi2`         |L0/L2|MIDI 2.0 UMP types                 |MIT       |
|`midi20`        |L0   |MIDI-CI capability inquiry         |MIT       |
|`dasp_graph`    |L1   |Audio graph scheduler (reference)  |MIT       |
|`fundsp`        |L3   |DSP combinators for node internals |MIT/Apache|
|`rhai`          |L4   |Embedded scripting runtime         |MIT/Apache|
|`petgraph`      |L1   |Graph data structure and algorithms|MIT/Apache|
|`clap-validator`|Dev  |CLAP plugin validation for CI      |MIT       |

**DSP source policy:** All DSP algorithm implementations must be sourced from MIT/Apache-licensed code or written from scratch. Mutable Instruments firmware (MIT) is the primary reference for high-quality DSP. HexoDSP code may now be studied as a GPL3-to-GPL3 reference but implementations must be independent.

-----

## Glossary

|Term                 |Definition                                                                                |
|---------------------|------------------------------------------------------------------------------------------|
|Node                 |**The universal primitive.** Every component that participates in data flow, state management, or event routing. Implements the Node trait. Exists within the graph. There is no legitimate platform component outside the graph — only the scripting engine is exempt by nature. See ADR-018.|
|Port                 |Typed input or output on a node.                                                          |
|Graph                |The network of connected nodes, managed by the runtime.                                   |
|State Bus            |Publish/subscribe observable namespace with hierarchical addresses.                       |
|Clock Domain         |A self-consistent tempo/position reference. Multiple domains coexist.                     |
|Parameter Lock       |Elektron concept: per-step override of any parameter for that step’s duration only.       |
|Conditional Trig     |Elektron concept: a trig that fires only when a condition is met.                         |
|Micro-timing         |Elektron concept: per-step timing offset within a step in micro-steps.                    |
|Capability Document  |Structured declaration of a node’s capabilities. Used by HardwareDevice nodes.            |
|Negotiable           |Node trait enabling capability handshake at connection time.                              |
|HAL                  |Hardware Abstraction Layer. L0. The only layer that knows the physical world.             |
|Rhai                 |Embedded scripting language for L4. Rust-native, sandboxed.                               |
|CLAP                 |CLever Audio Plugin format. Primary plugin API.                                           |
|UMP                  |Universal MIDI Packet. MIDI 2.0 message format.                                           |
|Configurator/Executor|The two-sided runtime split: main thread (Configurator) and audio thread (Executor).      |