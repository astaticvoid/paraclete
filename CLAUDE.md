# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Paraclete Is

Paraclete is an open-source musical computation runtime — a live, composable signal graph for instruments, sequencers, controllers, and effects. It is not a DAW and not a plugin. The closest analogies are Plan 9 (everything is a composable node), Emacs (live, reprogrammable runtime), and Elektron hardware (hardware-first, mouse-free workflow).

Target: standalone binary and CLAP plugin. Primary development controller: Novation Launchpad.

## Commands

```bash
# Build everything
cargo build

# Build release
cargo build --release

# Run the app (P4 graph: 8-track drum machine at 140 BPM)
# WAV files samples/track0.wav–track7.wav are used by Sampler[0–7].
# Hardware devices (Launchpad, Digitakt, Keystep) opened if present; falls back gracefully.
# Profile scripts loaded from profiles/ if present.
cargo run

# Run with developer UI (prints sequencer step positions to stderr each second)
cargo run -- --dev-ui

# Run all tests
cargo test

# Run tests for a single crate
cargo test -p paraclete-runtime
cargo test -p paraclete-node-api

# Run a single test by name
cargo test -p paraclete-runtime configurator_connect_rejects_two_node_cycle

# Check without building
cargo check

# Lint
cargo clippy
```

## Architecture: Five-Layer Model

No layer may reach across another layer's boundary. This is a hard constraint.

| Layer | Crate | License | Responsibility |
|-------|-------|---------|----------------|
| L0 HAL | `paraclete-hal` | GPL3 | Audio I/O via cpal, hardware emulator. The only layer that knows the physical world. |
| L1 Runtime | `paraclete-runtime` | GPL3 | Node graph, scheduling, clock federation, node lifecycle. No DSP logic. |
| L2 Node API | `paraclete-node-api` | **LGPL3** | The contract every node implements. Third-party nodes only depend on this crate. |
| L3 Nodes | `paraclete-nodes` | GPL3 | First-party node implementations (oscillator, hardware mapper). All built on L2 only. |
| L4 Scripting | `paraclete-scripting` | GPL3 | Rhai scripting sandbox. Live mappings and macros. Runs off the audio thread. |
| App | `paraclete-app` | GPL3 | Binary entry point. Wires the graph and starts the audio backend. |

**The LGPL3 boundary at L2 is intentional:** third parties can write closed-source nodes by implementing the L2 Node API without being forced to open-source their implementations.

## Configurator API

`NodeConfigurator` has three distinct `add_*` methods — use the correct one:

| Method | Use when |
|--------|----------|
| `add_node(id, Box<dyn Node>)` | Standard node (oscillator, sequencer, mapper) |
| `add_hardware_device(id, Box<dyn HardwareDevice>)` | Physical or emulated controller. Calls `take_output_handle()` at registration; if `Some`, the handle is ticked each main loop iteration instead of calling `update_output()` from the executor. |
| `add_tempo_source(id, Box<dyn TempoSource>)` | Clock master; registers a clock domain, returns `(NodeId, domain_id)` |

## Core Design: Configurator / Executor Split

The runtime splits into two halves to keep the audio thread allocation-free:

- **`NodeConfigurator`** — main thread. Owns the graph topology (via petgraph DAG), manages node lifecycle, sends incremental changes over a lock-free ring buffer.
- **`NodeExecutor`** — audio thread. Receives `ConfigMessage`s from the ring buffer, executes nodes in topological order, sums audio output. Never allocates, blocks, or takes a mutex.

The graph enforces a DAG (cycles rejected at `connect()` time per ADR-005). `build_executor()` drains nodes from the configurator in topological order — it can only be called once.

## The Node Contract

The core trait is `Node` in `paraclete-node-api`. Three engagement levels:

- **L1** — implement only `ports()` and `process()`. Passive signal processor (oscillator, filter, math module).
- **L2** — also override `capability_document()`, `activate()`, `deactivate()`, `serialize()`, `deserialize()`. Instruments and stateful nodes.
- **L3** — also implement `Negotiable` and override `is_negotiable()` to return `true`. `negotiate()` returns a `ConnectionAgreement` (declaring `lockable_params` for instruments); `set_connection_record()` receives the reconciled `ConnectionRecord` from both sides. The runtime invokes the handshake at `connect()` time for all connections (handshake defaults are no-ops for non-Negotiable nodes).

`HardwareDevice` is a supertrait of `Node` that declares a `SurfaceDescriptor` and provides two output paths:
- **Legacy path** — `update_output()` called from the executor (audio thread) each cycle.
- **P4+ path** — implement `take_output_handle()` returning `Some(Box<dyn HardwareOutputHandle>)`. The configurator holds the handle and calls `handle.tick()` each main-loop iteration (main thread). Use this for all new hardware nodes — it avoids audio-thread output. The `HardwareOutputHandle` pattern works via a device-internal SPSC: the audio-thread side pushes `HardwareOutput`; `tick()` drains it.

**ParameterBank** — any node that declares parameters in `capability_document()` should use `ParameterBank` (shipped, in L2). Build it at `activate()` time; call `bank.handle_commands(input.commands())` before DSP in `process()`. It handles `CMD_SET_PARAM` and `CMD_BUMP_PARAM` allocation-free, with clamping and silent ignore for unknown IDs.

**AudioEffect marker** — effect nodes (distortion, filter, reverb) are plain `Node` implementations with audio ports. Wet/dry and bypass are parameters, lockable by the sequencer. Optionally implement the `AudioEffect` marker trait (no required methods) so profile scripts can discover them by querying `CapabilityDocument.extensions` for `"paraclete.effect"`.

Four convenience super-traits exist in `paraclete-node-api::templates` as contribution scaffolding for third-party authors:

- `SignalNode` — implement `dsp()`; blanket `Node` impl handles port routing
- `InstrumentNode` — `on_note_on()`, `on_note_off()`, `render()`
- `SequencerNode` — `on_clock_tick()`, `emit_events()`
- `ControllerNode` — `on_packet()`, `set_led()`

## Signal / Port Types

Ports are typed. `connect()` validates: (1) both port IDs must exist on their respective nodes, (2) source port must be `Output` and destination port must be `Input`, (3) port types must match. All public L2 port/event enums carry `#[non_exhaustive]` — external matches require a `_` fallback arm.

| Type | Description |
|------|-------------|
| `Audio` | Block-rate stereo or multi-channel audio |
| `Event` | Timestamped event queue (notes, MIDI 2.0 UMP, `HardwareEvent`) |
| `Cv`, `Mod`, `Logic`, `Phase`, `Pitch` | Signal types for modulation and CV |

Events are routed between nodes within the same buffer cycle. The executor builds an `event_routes` table at `build_executor()` time by inspecting edge `src_port_type`.

## Transport Start Model

`InternalClock` emits `global_start: true` on its **first tick**. Downstream nodes (e.g. `Sequencer`) start with `playing: false` and only enter their pattern on receiving `global_start`. A `sync_pulse` (emitted on every bar boundary) lets a node that connects mid-session position itself correctly before playing. Nodes must not assume a running transport — they wait for the signal.

## StateBus

The audio thread (executor) pushes a `StateBusUpdate` at end-of-cycle via lock-free `rtrb` SPSC; the main thread drains it via `NodeConfigurator::process_main_thread()`. No lock is held on the audio thread.

The stable API (`StateBusHandle`) is the only surface consumers should touch:

```rust
handle.read(path)             // → Option<&StateBusValue>
handle.write(path, value)     // main thread only
handle.subscribe(path)        // → StateBusSubscription
```

The main loop must call `conf.process_main_thread()` each iteration — this drains the state bus SPSC, calls `handle.tick()` on all registered `HardwareOutputHandle`s, and notifies subscriptions. (`process_state_bus()` is kept as an alias.)

Rhai scripts access the state bus through this same API. Scripts may `read()` any path and `write_sandboxed()` to `/node/{id}/param/*` only. Writing to `/node/*/state/*`, `/transport/*`, or `/hw/*` is sandbox-rejected. Scripts run on the main thread (`Rc<RefCell>` not `Arc<Mutex>`).

## Event Delivery Ordering

Within the same `sample_offset`, the executor delivers events in this priority order:

1. `ParamLockEvent` — apply parameter overrides before note triggers
2. `TransportEvent` — position updates
3. `Midi2` — notes and controllers
4. `Hardware` — pad events
5. `Extended` — custom

`ParamLockEvent` is routed by `node_id` match, not by graph edges. A lock for an unknown `param_id` is silently ignored.

## Audio Thread Rules

These are hard constraints, not guidelines:

- `process()` must never allocate, block, or take a lock.
- The `NodeExecutor::process()` loop uses raw pointer aliasing to work around Rust's borrow checker for the `transport` and `extended_events` fields — this is intentional and documented with `SAFETY` comments.
- All main-thread → audio-thread communication goes through the lock-free ring buffer (`ConfigMessage`).
- `NodeCommand` messages (`CMD_SET_PARAM = 0`, `CMD_BUMP_PARAM = 1`) are routed per-node via a separate SPSC and delivered as `input.commands()` in `process()`. Node-specific command type IDs start at 16.

## Dependencies Added Per Phase

**P3:**
- `hound = "3.5"` (MIT) in `paraclete-nodes` — WAV loading for `Sampler`. `symphonia` is the P6 upgrade path.
- `rtrb = "0.3"` (MIT) in `paraclete-runtime` — lock-free SPSC ring buffer for the StateBus upgrade.

**P4:**
- `midir = "0.9"` (MIT) in `paraclete-hal` — cross-platform MIDI I/O for real hardware nodes.
- `rtrb = "0.3"` (MIT) in `paraclete-hal` — internal SPSC for `LaunchpadNode` output handle.
- `crossterm = "0.27"` in `paraclete-hal` — terminal emulator for `LaunchpadEmulator`.
- `egui`/`eframe` deferred — `--dev-ui` logs to stderr instead; full egui window pushed to P5+.

## DSP Source Policy

All DSP algorithm implementations must be sourced from MIT/Apache-licensed code or written from scratch. Mutable Instruments firmware (MIT) is the primary DSP reference. HexoDSP may be studied as a GPL3-to-GPL3 reference but implementations must be independent.

## Cellular Architecture Constraint

**Every component that participates in data flow, state management, or event routing is a `Node`.** There is no legitimate second class of platform object (ADR-018). Any design that appears to require a component outside the graph is a signal to extend the Node API — not a reason to create an exception. The scripting engine is the sole exemption: it is the live environment nodes operate within, not a node itself.

Multi-track is a graph topology, not a node feature (ADR-016): 8 tracks = 8 `Sequencer` node instances connected in parallel to one `InternalClock`. Any hardware control can reach any parameter declared in `capability_document()` generically via `CMD_SET_PARAM`/`CMD_BUMP_PARAM` — type knowledge is not required (ADR-019).

## Design Documents

All design documents live in `design/`. Key documents:

- `design/architecture-core.md` — stable: layer model, Node API, signal types, design principles (binding constraints)
- `design/architecture-evolving.md` — append-only phase log; records what shipped and what was found per phase
- `design/roadmap.md` — living: current phase scope, known provisional implementations, open questions
- `design/instrument-vision.md` — the concrete instrument being built; design tiebreaker for ambiguous decisions
- `design/adr/` — all Architecture Decision Records (**append-only** — never edit a past ADR, add a new one to supersede it)
- `design/adr/ADR-005-scheduler-cycles.md` — why the graph must be a DAG (cycle rejection)
- `design/adr/ADR-008-node-api-level.md` — the three engagement levels
- `design/adr/ADR-016-multi-track-sequencer.md` — multi-track = multiple node instances
- `design/adr/ADR-017-effect-node-architecture.md` — effects are plain nodes; `AudioEffect` is a marker only
- `design/adr/ADR-018-cellular-architecture.md` — no non-node platform components; Node API extension is the answer
- `design/adr/ADR-019-universal-parameter-control.md` — `CMD_SET_PARAM`/`CMD_BUMP_PARAM`; `ParameterBank` spec
- `design/phases/` — per-phase interface specs and implementation reports (append-only once a phase ships)
- `design/phases/p3-interfaces.md` — P3 spec: `Negotiable` trait, parameter lock protocol, `Sampler` node, StateBus SPSC upgrade, Rhai bindings
- `design/phases/p4-interfaces.md` — P4 spec: hardware integration, `ParameterBank`, `NodeCommand`, `HardwareOutputHandle`, effect nodes, profile scripts
- `design/phases/p4-report.md` — P4 implementation report: what shipped, node IDs, test coverage, deferred items
- `profiles/` — Rhai profile scripts loaded at startup (`launchpad.rhai`, `digitakt.rhai`, `keystep.rhai`). Constants `LP_DEVICE_ID`, `DT_DEVICE_ID`, `KS_DEVICE_ID`, `TRACK_SEQ_IDS`, etc. injected per profile.
