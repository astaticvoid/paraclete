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

# Run the app (P2 demo: InternalClock → Sequencer → SineOscillator)
cargo run

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
| `add_hardware_device(id, Box<dyn HardwareDevice>)` | Physical or emulated controller; also receives `update_output()` after each cycle |
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
- **L3** — also implement `Negotiable`. `negotiate()` returns a `ConnectionAgreement` (declaring `lockable_params` for instruments); `set_connection_record()` receives the reconciled `ConnectionRecord` from both sides. The runtime invokes the handshake at `connect()` time if either node implements `Negotiable`.

`HardwareDevice` is a supertrait of `Node` that additionally receives `update_output()` callbacks after each cycle and declares a `SurfaceDescriptor`.

Four convenience super-traits exist in `paraclete-node-api::templates` as contribution scaffolding for third-party authors:

- `SignalNode` — implement `dsp()`; blanket `Node` impl handles port routing
- `InstrumentNode` — `on_note_on()`, `on_note_off()`, `render()`
- `SequencerNode` — `on_clock_tick()`, `emit_events()`
- `ControllerNode` — `on_packet()`, `set_led()`

## Signal / Port Types

Ports are typed. Type compatibility is enforced at connection time.

| Type | Description |
|------|-------------|
| `Audio` | Block-rate stereo or multi-channel audio |
| `Event` | Timestamped event queue (notes, MIDI 2.0 UMP, `HardwareEvent`) |
| `Cv`, `Mod`, `Logic`, `Phase`, `Pitch` | Signal types for modulation and CV |

Events are routed between nodes within the same buffer cycle. The executor builds an `event_routes` table at `build_executor()` time by inspecting edge `src_port_type`.

## Transport Start Model

`InternalClock` emits `global_start: true` on its **first tick**. Downstream nodes (e.g. `Sequencer`) start with `playing: false` and only enter their pattern on receiving `global_start`. A `sync_pulse` (emitted on every bar boundary) lets a node that connects mid-session position itself correctly before playing. Nodes must not assume a running transport — they wait for the signal.

## StateBus

**P3 replaces the provisional `Arc<RwLock<HashMap>>` with lock-free `rtrb` SPSC.** The audio thread (executor) pushes a `StateBusUpdate` at end-of-cycle; the main thread drains it in `NodeConfigurator::process_state_bus()`. No lock is held on the audio thread.

The stable API (`StateBusHandle`) is the only surface consumers should touch:

```rust
handle.read(path)             // → Option<&StateBusValue>
handle.write(path, value)     // main thread only
handle.subscribe(path)        // → StateBusSubscription
```

The main loop must call `conf.process_state_bus()` between audio cycles — this drains the SPSC, notifies subscriptions, and calls `update_output()` on all hardware devices.

Rhai scripts access the state bus through this same API. Scripts may `read()` any path and `write()` to `/node/{id}/param/*`. Writing to `/transport/*` or `/hw/*` is sandbox-rejected. Scripts run on the main thread (`Rc<RefCell>` not `Arc<Mutex>`).

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

## New Dependencies at P3

- `hound = "3.5"` (MIT) in `paraclete-nodes` — WAV file loading for `Sampler`. Isolated to one function; `symphonia` is the documented upgrade path for broader format support.
- `rtrb = "0.3"` (MIT) in `paraclete-runtime` — lock-free SPSC ring buffer for the StateBus upgrade.

## DSP Source Policy

All DSP algorithm implementations must be sourced from MIT/Apache-licensed code or written from scratch. Mutable Instruments firmware (MIT) is the primary DSP reference. HexoDSP may be studied as a GPL3-to-GPL3 reference but implementations must be independent.

## Design Documents

All design documents live in `design/`. Key documents:

- `design/architecture-core.md` — stable: layer model, Node API, signal types, design principles
- `design/architecture-evolving.md` — living: roadmap, open questions, phase notes
- `design/adr/` — all Architecture Decision Records (ADRs are **append-only** — never edit a past ADR, add a new one to supersede it)
- `design/adr/ADR-005-scheduler-cycles.md` — why the graph must be a DAG (cycle rejection)
- `design/adr/ADR-008-node-api-level.md` — the three engagement levels
- `design/phases/` — per-phase interface specs and implementation reports (append-only once a phase ships)
- `design/phases/p3-interfaces.md` — P3 spec: `Negotiable` trait, parameter lock protocol, `Sampler` node, StateBus SPSC upgrade, Rhai bindings
