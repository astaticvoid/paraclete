# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Paraclete Is

Paraclete is an open-source musical computation runtime — a live, composable signal graph for instruments, sequencers, controllers, and effects. It is not a DAW and not a plugin. The closest analogies are Plan 9 (everything is a composable node), Emacs (live, reprogrammable runtime), and Elektron hardware (hardware-first, mouse-free workflow).

Target: standalone binary and CLAP plugin. Primary development controller: Novation Launchpad.

## Commands

```bash
# Generate synthesized drum samples (required before first run)
cargo run -p gen-samples           # writes samples/track0–7.wav
cargo run -p gen-samples -- path/  # optional output directory

# Debug Launchpad X hardware (verify MIDI connectivity, test LED output)
cargo run -p lpx-debug

# Build everything (bare `cargo build` only builds paraclete-app — it is the sole
# default-member in Cargo.toml — so pass --workspace to cover all crates)
cargo build --workspace

# Build release
cargo build --release

# Run the app (P8+: graph loaded from instrument.yaml; hardware devices opened if present;
# profile scripts from profiles/ if present; ratatui TUI shown by default.)
cargo run

# Run with a specific instrument definition file
cargo run -- --instrument=my-instrument.yaml

# Skip the terminal UI (falls back to --dev-ui style stderr logging)
cargo run -- --no-tui

# Run with developer UI (prints step position + 16-char pattern bitfield to stderr each second)
cargo run -- --dev-ui

# Load/save project state (RON format, ADR-025; --load applied before build_executor, --save after)
cargo run -- --load=project.ron --save=project.ron

# Build machine bank CLAP plugins (paraclete-clap crate; output: target/debug/*.clap)
cargo build -p paraclete-clap

# Validate a .clap binary (requires clap-validator installed; test is tagged #[ignore])
cargo test --test clap_validator -- --ignored

# Run all tests (bare `cargo test` runs only paraclete-app's ~24 tests due to
# default-members; --workspace runs the full suite)
cargo test --workspace

# Run tests for a single crate
cargo test -p paraclete-runtime
cargo test -p paraclete-node-api

# Run a single test by name
cargo test -p paraclete-runtime configurator_connect_rejects_two_node_cycle

# Check without building (--workspace for all crates)
cargo check --workspace

# Lint
cargo clippy --workspace
```

### Terminal emulator keyboard controls (LaunchpadEmulator)

When no physical Launchpad is connected, the full 8×8 surface is keyboard-driven via a track-cursor scheme (P9.5):

```
1 2 3 4 5 6 7 8   select active track row (0–7)
Q W E R T Y U I   toggle the 8 step pads in the active row
A S D F G H J K   scene buttons (page select; ids 64–71)
Z X C V B N M ,   top control row (modes / navigation; ids 72–79)
Tab               cycle input mode (Grid/Encoder/Piano)   Esc/Ctrl-C  quit
```

All 8 tracks + scene buttons + control row are reachable (replaces the legacy 3-row mapping that only reached tracks 0–2). Control-id map: grid 0–63 (`row*8+col`), scene 64–71, control 72–79. Esc or Ctrl-C to stop.

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

Three additional platform crates live outside the five-layer model:

| Crate | License | Role |
|-------|---------|------|
| `paraclete-clap` | GPL3 | Paraclete-as-CLAP-plugin adapter (ADR-024). `ClapParamBridge`, `translate_transport`, `SingleNodePlugin` (MIDI effect wrapping one node), `SubgraphPlugin` (InternalClock+Sequencer+generator graph). Machine bank workspace crates (`paraclete-machine-kick/snare/fm-kick/fm-bell/fm-bass`) produce cdylibs via real CLAP FFI (`clap-sys`). |
| `paraclete-clap-host` | GPL3 | Paraclete-as-CLAP-host (ADR-027, shipped P8). `PluginLibrary` loads `.clap` files (clap-sys + libloading); `PluginNode` wraps a loaded plugin as a `Node`. Arc-shared library handle keeps the .clap alive as long as any node exists. Generator plugins only at P8; effect plugins (audio_in port) in P9. |
| `paraclete-tui` | GPL3 | `ratatui`-based terminal UI (ADR-026, shipped P8). `TuiApp` reads transport, encoder context, and step state from the StateBus each frame. Three-row layout: transport bar, encoder row, step row. |
| `paraclete-graph-nodes` | GPL3 | Nodes that own a `NodeExecutor` inner graph (ADR-023, P9). The only crate permitted to depend on both `paraclete-nodes` and `paraclete-runtime`. `InnerGraphNode` is the initial concrete implementation. |

## Configurator API

`NodeConfigurator` has three distinct `add_*` methods — use the correct one:

| Method | Use when |
|--------|----------|
| `add_node(Box<dyn Node>)` | Standard node (oscillator, sequencer, mapper) |
| `add_node_tagged(Box<dyn Node>, type_tag)` | Same as `add_node` but records the type_tag for v2 project file serialization. Preferred for all new call sites. |
| `add_surface(id, Box<dyn Surface>)` | Physical or emulated controller. Calls `take_output_handle()` at registration; if `Some`, the handle is ticked each main loop iteration instead of calling `update_output()` from the executor. |
| `add_tempo_source(id, Box<dyn TempoSource>)` | Clock master; registers a clock domain, returns `(NodeId, domain_id)` |

## Core Design: Configurator / Executor Split

The runtime splits into two halves to keep the audio thread allocation-free:

- **`NodeConfigurator`** — main thread. Owns the graph topology (via petgraph DAG), manages node lifecycle, sends incremental changes over a lock-free ring buffer.
- **`NodeExecutor`** — audio thread. Receives `ConfigMessage`s from the ring buffer, executes nodes in topological order, sums audio output. Never allocates, blocks, or takes a mutex.

The graph enforces a DAG (cycles rejected at `connect()` time per ADR-005), with one exception: a cycle containing exactly one `LoopBreakNode` is accepted (ADR-028). `build_executor()` drains nodes from the configurator in topological order. `rebuild_executor()` (P9) is the non-consuming equivalent — callable multiple times for dynamic topology (ADR-029).

`NodeConfigurator` (P8) exposes additional accessors:
- `get_node_cap_doc(node_id)` → `Option<&CapabilityDocument>` — returns the cached capability document (P9: cached at `add_node()` time to avoid `Box::leak` per call); used to populate the cap_docs map for TuiApp
- `sample_rate()` → `f32` — exposes the configured sample rate for CLAP plugin instantiation
- `block_size()` → `usize` — exposes the configured block size for CLAP plugin instantiation

`NodeConfigurator` (P9) adds dynamic topology methods:
- `remove_node(id)` → `Result<Box<dyn Node>, ConfigError>` — removes a node and severs all connected edges
- `disconnect(src, src_port, dst, dst_port)` → `Result<(), ConfigError>` — removes a specific edge
- `rebuild_executor()` → `NodeExecutor` — non-consuming build; callable multiple times
- `type_tag_for(node_id)` → `Option<&str>` — returns the stored type_tag for project file serialization
- `cap_doc_cache: HashMap<u32, CapabilityDocument>` — `capability_document()` called once per node per session

`NodeExecutor` (P7) exposes injection methods for CLAP plugin use — safe to call before `process()` in the same audio callback:
- `set_transport_override(info, event)` — overrides the transport for one cycle (consumed via `.take()`); must be injected *after* `incoming.clear()` to survive the cycle boundary
- `inject_event_for_node(node_id, event)` — routes a `TimedEvent` to a specific node's incoming slot; guards unknown IDs at push time
- `inject_node_command(cmd)` — routes a `NodeCommand` to the executor's pending queue; guards unknown target IDs
- `serialize_node(node_id)` / `deserialize_node(node_id, data)` — delegates to the node's serialize/deserialize

## The Node Contract

The core trait is `Node` in `paraclete-node-api`. Three engagement levels:

- **L1** — implement only `ports()` and `process()`. Passive signal processor (oscillator, filter, math module).
- **L2** — also override `capability_document()`, `activate()`, `deactivate()`, `serialize()`, `deserialize()`. Instruments and stateful nodes.
- **L3** — also implement `Negotiable` and override `is_negotiable()` to return `true`. `negotiate()` returns a `ConnectionAgreement` (declaring `lockable_params` for instruments); `set_connection_record()` receives the reconciled `ConnectionRecord` from both sides. The runtime invokes the handshake at `connect()` time for all connections (handshake defaults are no-ops for non-Negotiable nodes).

`Surface` is a supertrait of `Node` that declares a `SurfaceDescriptor` and provides two output paths:
- **Legacy path** — `update_output()` called from the executor (audio thread) each cycle.
- **P4+ path** — implement `take_output_handle()` returning `Some(Box<dyn SurfaceOutputHandle>)`. The configurator holds the handle and calls `handle.tick()` each main-loop iteration (main thread). Use this for all new hardware nodes — it avoids audio-thread output. The `SurfaceOutputHandle` pattern works via a device-internal SPSC: the audio-thread side pushes `SurfaceOutput`; `tick()` drains it.

**ParameterBank** — any node that declares parameters in `capability_document()` should use `ParameterBank` (shipped, in L2). Build it at `activate()` time; call `bank.handle_commands(input.commands())` before DSP in `process()`. It handles `CMD_SET_PARAM` and `CMD_BUMP_PARAM` allocation-free, with clamping and silent ignore for unknown IDs.

**ParamLock pattern for generator nodes** — nodes that support per-step parameter locks (AnalogEngine, FmEngine, any future instrument node) must NOT route `ParamLock` events through `bank.handle_commands()`. That permanently mutates the bank and bleeds the locked value into subsequent steps. The correct pattern (ADR-019):
```rust
// In the struct:
node_locks: Vec<(u32, f64)>,  // cleared at top of every process() call

// In process(), first thing after bank.handle_commands:
self.node_locks.clear();

// In the event loop, ParamLock branch:
Event::ParamLock(pl) => self.node_locks.push((pl.param_id, pl.value)),

// Parameter read helper — checks per-cycle overrides before falling back to bank:
fn get_param(&self, param_id: u32) -> f32 {
    for &(id, val) in &self.node_locks { if id == param_id { return val as f32; } }
    self.bank.get(param_id) as f32
}
```

**Parameter naming convention** (ADR-019 amendment) — parameters representing the same concept must use the same name string across all nodes. A hardware encoder mapped to `id_for_name("cutoff")` reaches every node that declares `"cutoff"`, and only those nodes. Node-type-specific prefixes (`"ladder_cutoff"`) are wrong. Canonical names: `"cutoff"`, `"resonance"`, `"drive"`, `"wet"`, `"dry"`, `"decay"`, `"attack"`, `"release"`, `"tune"`. Parameter names are long-term contracts — renaming after crates.io publication is a breaking change.

**`Node::type_name()`** — returns a stable string label for the node type. Default impl returns `std::any::type_name::<Self>()` (fully-qualified Rust path). Override to return a string literal for project file readability after module renames: `fn type_name(&self) -> &'static str { "AnalogEngine" }`. Required by ADR-025: `NodeSnapshot.type_name` stores this label.

**`Node::published_state()` push-down signature** — accepts `&mut Vec<(String, StateBusValue)>` rather than returning a `Vec`. The executor clears the buffer before calling; nodes push into it. This is the pattern to use — the old returning signature was OQ-9 (allocates per cycle).

**`Node::set_initial_params()` (P8)** — default no-op; called with `HashMap<String, f64>` from the instrument definition file before `activate()`. ParameterBank nodes override it to store values and apply them at `activate()`. P9 extends coverage to AnalogEngine, FmEngine, Sampler, ReverbNode.

**`Node::set_node_id()` (P9)** — default no-op; called by `NodeConfigurator::add_node()` after assigning the ID. Nodes that publish named state (via `publish_bank_state()`) store this ID and use it to format state bus paths like `/node/{id}/{param_name}`.

**`Node::is_loop_break()` / `loop_break_prev()` / `loop_break_swap()` (P9)** — three default no-op methods for the feedback loop break protocol (ADR-028). Only `LoopBreakNode` overrides them. The executor calls `loop_break_prev()` in a pre-execution phase and `loop_break_swap()` in a post-execution phase for each back edge recorded at connection time.

**`publish_bank_state()` free function (P9)** — in `paraclete-node-api`; iterates a `ParameterBank` and pushes one `StateBusValue::Float` entry per declared parameter into the `published_state()` output buffer using path `/node/{node_id}/{param_name}`. The TUI reads these paths for live encoder value display.

**`ParameterSlot::name` field (P9)** — `ParameterSlot` gains a `name: String` field storing the canonical ADR-019 parameter name. `ParameterBank::iter_values()` yields `(&str, f64)` pairs for use by `publish_bank_state()`. No call-site change to `declare_param()`.

**`Node::deserialize()` ordering** — must be called *after* `activate()` for nodes that use `ParameterBank`. `activate()` resets the bank to defaults; `deserialize()` re-applies saved values on top. The doc comment in `paraclete-node-api/src/node.rs` was corrected in P7 to reflect this.

**`ParamDescriptor::id_for_name()` is `const fn`** — promoted in P7 Commit 7. Use `const CUTOFF_ID: u32 = ParamDescriptor::id_for_name("cutoff");` to get zero-cost compile-time IDs.

**`GraphNode` marker trait (P9)** — in `paraclete-node-api` (LGPL3); identifies nodes that contain an inner `NodeExecutor`. No required methods beyond `Node`. Implemented by `InnerGraphNode` in `paraclete-graph-nodes`. Inner graph patching at runtime is deferred to P10; the inner graph is rebuilt as a unit on outer graph patch.

**`LoopBreakNode` (P9)** — in `paraclete-nodes`; CvSignal-only, two ports (`cv_in`/`cv_out`). Holds one block of samples from the previous cycle, introducing exactly one buffer of latency in the feedback path. The only node that overrides `is_loop_break()` → `true`. Type-tag: `"loop_break"`. Not a general-purpose delay; use `DelayNode` for configurable timing.

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
| `Event` | Timestamped event queue (notes, MIDI 2.0 UMP, `SurfaceEvent`) |
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

The main loop must call `conf.process_main_thread()` each iteration — this drains the state bus SPSC, calls `handle.tick()` on all registered `SurfaceOutputHandle`s, and notifies subscriptions. (`process_state_bus()` is kept as an alias.)

Rhai scripts access the state bus through this same API. Scripts may `read()` any path and `write_sandboxed()` to `/node/{id}/param/*` only. Writing to `/node/*/state/*`, `/transport/*`, or `/hw/*` is sandbox-rejected. Scripts run on the main thread (`Rc<RefCell>` not `Arc<Mutex>`).

## Event Delivery Ordering

Within the same `sample_offset`, the executor delivers events in this priority order:

1. `ParamLockEvent` — apply parameter overrides before note triggers
2. `TransportEvent` — position updates
3. `Midi2` — notes and controllers
4. `Surface` — pad events
5. `Extended` — custom

`ParamLockEvent` is routed by `node_id` match, not by graph edges. A lock for an unknown `param_id` is silently ignored.

## Audio Thread Rules

These are hard constraints, not guidelines:

- `process()` must never allocate, block, or take a lock.
- The `NodeExecutor::process()` loop uses raw pointer aliasing to work around Rust's borrow checker for the `transport` and `extended_events` fields — this is intentional and documented with `SAFETY` comments.
- All main-thread → audio-thread communication goes through the lock-free ring buffer (`ConfigMessage`).
- `NodeCommand` messages (`CMD_SET_PARAM = 0`, `CMD_BUMP_PARAM = 1`) are routed per-node via a separate SPSC and delivered as `input.commands()` in `process()`. Node-specific command type IDs start at 16.

## App Graph Node IDs

The app hard-codes these IDs; profile scripts use them as injected constants:

| ID | Node |
|----|------|
| 1 | `InternalClock` |
| 2 | `MixNode` |
| 10–17 | `Sequencer[0–7]` |
| 20 | `AnalogEngine::kick()` |
| 21 | `AnalogEngine::snare()` |
| 22 | `AnalogEngine::hihat()` |
| 23–26 | `Sampler[3–6]` (WAV files) |
| 27 | `FmEngine::bass()` |
| 30–37 | `DistortionNode[0–7]` |
| 40–47 | `FilterNode[0–7]` |
| 101 | `LaunchpadEmulator` (fallback) |
| 102 | `LaunchpadNode` |
| 103 | `DigitaktMidiNode` |
| 104 | `KeystepNode` |
| 105 | `SurfaceMappingNode` |
| 110 | `ScriptingGatewayNode[LP]` (Launchpad or emulator gateway) |
| 111 | `ScriptingGatewayNode[DT]` (Digitakt gateway, if connected) |
| 112 | `ScriptingGatewayNode[KS]` (Keystep gateway, if connected) |
| 200 | `ReverbNode` (master bus) |

Constants injected per profile: `LP_DEVICE_ID`, `DT_DEVICE_ID`, `KS_DEVICE_ID`, `CLOCK_ID`, `TRACK_SEQ_IDS`, `TRACK_SAMP_IDS`, `TRACK_DIST_IDS`, `TRACK_FILT_IDS`.

## Sequencer Node Commands

Beyond the universal `CMD_SET_PARAM`/`CMD_BUMP_PARAM`, `Sequencer` handles:

| type_id | Constant | arg0 | arg1 |
|---------|----------|------|------|
| 16 | `CMD_TOGGLE_STEP` | step index | — |
| 17 | `CMD_SET_STEP` | step index | < 0 = deactivate; ≥ 0 = note value |
| 18 | `CMD_CLEAR` | — | — |
| 23 | `CMD_SET_FILL_A` *(P5)* | 1 = active, 0 = inactive | — |
| 24 | `CMD_SET_FILL_B` *(P5)* | 1 = active, 0 = inactive | — |
| 25 | `CMD_SET_STEP_TIMING` *(P5)* | step index | micro_offset (i8, ±47; 1 unit ≈ 1/96 beat) |
| 26 | `CMD_SET_STEP_CONDITION` *(P5)* | step index | packed i64: bits 0–7 probability, 8–15 repeat_n, 16–23 repeat_m, 24–31 fill discriminant |
| 27 | `CMD_SET_PATTERN` *(P5)* | pattern index | — (stub at P5; always plays pattern 0) |

Sequencer publishes to the state bus each cycle:
- `/node/{id}/state/steps` — 16-char ASCII bitfield (`'1'`/`'0'` per step)
- `/node/{id}/state/track_name` — non-empty if set via `with_name()`
- `/node/{id}/state/current_step` — current playback position (0–15)
- `/node/{id}/state/loop_count` *(P5)* — wrapping u32; increments each pattern completion
- `/node/{id}/state/fill_a` *(P5)* — bool as f64
- `/node/{id}/state/fill_b` *(P5)* — bool as f64

P5 also adds a `swing` parameter to the Sequencer ParameterBank (0.0–0.5, default 0.0). Swing is applied at emit time to odd-indexed steps.

**Sequencer as CV source (P9):** `Sequencer::with_cv_outputs(n)` adds `n` CvSignal output ports (`cv_out_0`, `cv_out_1`, … at port IDs `PORT_CV_OUT_BASE + i`, where `PORT_CV_OUT_BASE = 3`; ports 0–2 are the existing clock_in/events_in/events_out). Each `Step` gains `cv_locks: Vec<(u16, f32)>` for per-step CV value locks (sample-and-hold: value held from step fire until next step fire). `"sequencer_cv"` type-tag is `Sequencer::with_cv_outputs(1)`. cv_locks are serialized in the project file (serialization version 2); P5 fields (TrigCondition, StepTiming) are still not serialized.

## Current Phase: Playable-loop arc (P10 + W-track, July 2026)

**If you are starting an implementation session, read `design/handoff.md` first** — it routes tasks by model tier and carries the guardrails. Current sequence (authority: `design/roadmap.md`): ~~P10 C0~~ shipped (BUG-001 re-diagnosed + fixed via measurement — see p10-interfaces amendment; BUG-008 fixed) → **W0** (next: Antiphon server + Theoria browser grid POC, spec: `design/phases/w0-interfaces.md`) → **P10 C1** (Pattern struct + serializer v3, gated) → **W1** (spec: `design/phases/w1-interfaces.md`) → **paired session #1 with the user** → P10 C2+ interleaved with W2.

**The interface track (accepted July 2026, `design/interface-plan.md`, ADR-031):** *Antiphon* (`paraclete-antiphon`) is a presentation-agnostic interface server — WebSocket for the tablet client, in-process transport for the terminal — speaking the existing hardware vocabulary; every connected surface manifests as a `Surface` node + gateway, peer to `LaunchpadNode`. *Theoria* is the view layer (`theoria-web` tablet client — primary control/editing surface; `theoria-term` — the TUI ported to an Antiphon client at milestone WT). Touch encoders emit relative deltas only. **Naming policy is binding:** third-party marks (Elektron, Digitakt, Ableton, Push…) never appear in our feature names, identifiers, or UI strings — house vocabulary is Antiphon, Theoria, kerygma (broadcast module), epiclesis (headless test driver), *pages*, *grid*, *chain*; wire-protocol names stay plain.

**P9.5 closed early** (C1 shipped: full Launchpad emulator via track-cursor keyboard, 402 workspace tests passing). C2/C3 cancelled — superseded by W0/W1; C4 folded into P10 C5 test work; piano mode deferred. **P10.5 dissolved** into a trigger-based bug backlog (see roadmap).

**P7 shipped** (317 tests, 0 failures): `Node::type_name()` — stable string label for project files. `published_state()` push-down (OQ-9 resolved) — nodes push into a caller-owned `Vec`, eliminating per-cycle heap allocation. Per-voice rubato pitch resampling in Sampler — `VoiceResampler` wraps `SincFixedOut<f32>`; `root_note` moved to ParameterBank; serialize v3. Project save/recall — RON format (ADR-025); `save_project()`/`load_project()` in `paraclete-app/src/project.rs`; `--load`/`--save` CLI flags. `paraclete-clap` crate — `ClapParamBridge`, `translate_transport`, `SingleNodePlugin`, `SubgraphPlugin`, machine bank stub binaries. `paraclete-node-api` v0.1.0 publication prep; `id_for_name` promoted to `const fn`. See `design/phases/p7-report.md`.

**P8 shipped** (340 tests, 0 failures): P7 residuals — `InternalClock` transport stop/resume; machine bank real CLAP FFI (`clap-sys`), five workspace crates (`paraclete-machine-kick/snare/fm-kick/fm-bell/fm-bass`); SILENCE buffer 4096 → 65536. `publish_context()` Rhai builtin — writes `/context/{encoder_key}/node` and `param` to state bus. Instrument definition YAML (`serde_yaml`, ADR-026) — `build_from_instrument()`; `Node::set_initial_params()` (DistortionNode, FilterNode); `AudioOutputNode`. `paraclete-tui` crate — `ratatui` terminal UI; transport bar, encoder row, step row; 500 ms encoder highlight decay. `paraclete-clap-host` crate — `PluginLibrary` / `PluginNode` (clap-sys + libloading; `clack` crate was a crates.io placeholder); ParameterBank + `CLAP_EVENT_PARAM_VALUE` flush; `Arc<PluginLibrary>` for multi-plugin bundle safety; generator plugins only. App wiring (`--instrument`, `--no-tui` flags; 14-step startup sequence; instrument-file-driven graph). See `design/phases/p8-report.md`.

**P9 shipped** (391 tests by cargo count / 389 distinct, 0 failures): See `design/phases/p9-report.md`.
- **Commit 1 (GATED):** Encoder value publishing — `ParameterSlot::name`, `ParameterBank::iter_values()`, `publish_bank_state()`, `Node::set_node_id()`. All nodes with ParameterBank override `published_state()` to push live values. TUI encoder row shows real values.
- **Commit 2:** Housekeeping — `cap_doc_cache` in NodeConfigurator (prerequisite for Commit 4); `serde_yaml` → `serde_yml`; `set_initial_params()` coverage for AnalogEngine, FmEngine, Sampler, ReverbNode; effect-type CLAP plugins in `PluginNode`. Added BUG-008 to the tracker.
- **Commit 3:** Feedback loop break — `LoopBreakNode` (CvSignal only), `is_loop_break()`/`loop_break_prev()`/`loop_break_swap()` Node trait methods, executor pre/post phases, amended cycle detection (ADR-028).
- **Commit 4:** Dynamic topology — `NodeRegistry`, `TopologyChange`, `apply_patch()` (pause-rebuild-resume, ~5 ms silence), `AudioEngine::pause()`/`wait_paused()`/`resume_with_executor()`, project file format v2 with `type_tag` (ADR-029).
- **Commit 5:** Sequencer as CV source — `Sequencer::with_cv_outputs(n)`, `cv_out_{i}` ports (CvSignal), `Step::cv_locks`, sample-and-hold output. `"sequencer_cv"` registered in NodeRegistry.
- **Commit 6:** GraphNode — `GraphNode` marker trait (LGPL3), `paraclete-graph-nodes` crate (new), `InnerGraphNode` (inner executor: clock→sequencer→kick→mix→audio_out), `"inner_graph"` registered in NodeRegistry.

**Known deferred items (P10+):**
- Step period is 241 ticks (not 240) — off-by-one in transport start model.
- `ConnectionAgreement::baseline()` hardcodes `sample_rate=44100.0` and `block_size=512`.
- `paraclete-hal` depends on `paraclete-runtime` (layer violation, deferred until StateBusHandle moves to L2).
- AnalogEngine/FmEngine polyphony (monophonic with retrigger).
- Signed micro-timing in Sequencer (negative offset not yet distinct from positive).
- LadderFilter HP/BP modes, LFO tempo sync.
- `Sequencer::serialize()` does not save P5 fields (TrigCondition, StepTiming) or swing.
- `agg_state_buf` in executor does one allocation per cycle via `mem::take()` — full elimination requires a return channel.
- AudioBuffer feedback cycles (requires OQ-7 oversampling strategy).
- Inner GraphNode topology patching at runtime (P9 `apply_patch()` patches the outer graph only).
- CLAP plugin nodes not in NodeRegistry (require PluginLibrary argument).
- `InnerGraphNode::serialize()` returns empty — inner graph state not persisted at P9.
- `SubgraphPlugin` machine slot swapping — depends on stable `GraphNode` API (now P9, scheduled P10).
- Undo/redo for `apply_patch()` — deferred.
- CLAP GUI extension, note expressions, hot-reload (all still deferred).

## Dependencies Added Per Phase

**P3:**
- `hound = "3.5"` (MIT) in `paraclete-nodes` — WAV loading for `Sampler`. `symphonia` is the P6 upgrade path.
- `rtrb = "0.3"` (MIT) in `paraclete-runtime` — lock-free SPSC ring buffer for the StateBus upgrade.

**P4:**
- `midir = "0.9"` (MIT) in `paraclete-hal` — cross-platform MIDI I/O for real hardware nodes.
- `rtrb = "0.3"` (MIT) in `paraclete-hal` — internal SPSC for `LaunchpadNode` output handle.
- `crossterm = "0.27"` in `paraclete-hal` — terminal emulator for `LaunchpadEmulator`.
- `egui`/`eframe` deferred — `--dev-ui` logs to stderr instead; full egui window pushed to P5+.

**P5:**
- `fastrand = "2"` (MIT) in `paraclete-nodes` — no_std-compatible RNG for `TrigCondition` probability. No other new dependencies; Freeverb and delay algorithms are written from scratch per DSP source policy.

**P6:**
- `symphonia = "0.5"` (MIT/Apache) with `features = ["all"]` in `paraclete-nodes` — replaces `hound` for audio file loading. Supports WAV/FLAC/AIFF/OGG/MP3.
- `rubato = "0.15"` (MIT) in `paraclete-nodes` — load-time sinc resampling (SincFixedIn) when sample native rate ≠ output rate.
- `hound` removed from `paraclete-nodes` (retained in `gen-samples` tool only).

**P7:**
- `ron = "0.8"` (MIT/Apache) in workspace + `paraclete-app` — RON serialisation for project save/recall (ADR-025).
- `serde = { version = "1", features = ["derive"] }` in workspace + `paraclete-app` — required by `ron`.
- `paraclete-clap` crate added — `ClapParamBridge`, `translate_transport`, `SingleNodePlugin`, `SubgraphPlugin`; no `clap-sys` yet (added in P8 Commit 1 for real CLAP FFI).

**P8:**
- `clap-sys = "0.5"` (MIT) in `paraclete-clap`, `paraclete-clap-host` — raw CLAP FFI bindings for machine bank and host.
- `libloading = "0.8"` (MIT/ISC) in `paraclete-clap-host` — cross-platform dynamic library loading for `.clap` files.
- `serde_yaml = "0.9"` (MIT/Apache) in workspace + `paraclete-app` — YAML instrument definition parsing (ADR-026).
- `ratatui = "0.26"` (MIT) in workspace + `paraclete-tui`, `paraclete-app` — terminal UI widgets.
- `crossterm` promoted to workspace dependency (was `paraclete-hal`-only; now also used by `paraclete-tui`).
- `paraclete-tui` crate added — `TuiApp`, `TuiState`, `EncoderSlot`, `layout::render()`.
- `paraclete-clap-host` crate added — `PluginLibrary`, `PluginNode`, `HostParamBridge`, `scan_clap_paths()`.
- Five `paraclete-machine-*` workspace crates added (replace `[[bin]]` stubs in `paraclete-clap`).

**P9:**
- `serde_yaml = "0.9"` removed; `serde_yml = "0.0"` (0.0.13 resolved; MIT/Apache) replaces it — upstream deprecation migration (Commit 2).
- `paraclete-graph-nodes` crate added — GPL3; depends on `paraclete-node-api`, `paraclete-nodes`, `paraclete-runtime`; `InnerGraphNode` is the first implementation (Commit 6).

## DSP Source Policy

All DSP algorithm implementations must be sourced from MIT/Apache-licensed code or written from scratch. Mutable Instruments firmware (MIT) is the primary DSP reference. HexoDSP may be studied as a GPL3-to-GPL3 reference but implementations must be independent.

## Cellular Architecture Constraint

**Every component that participates in data flow, state management, or event routing is a `Node`.** There is no legitimate second class of platform object (ADR-018). Any design that appears to require a component outside the graph is a signal to extend the Node API — not a reason to create an exception. The scripting engine is the sole exemption: it is the live environment nodes operate within, not a node itself.

Multi-track is a graph topology, not a node feature (ADR-016): 8 tracks = 8 `Sequencer` node instances connected in parallel to one `InternalClock`. Any hardware control can reach any parameter declared in `capability_document()` generically via `CMD_SET_PARAM`/`CMD_BUMP_PARAM` — type knowledge is not required (ADR-019).

## Main Loop Sequence

The app main loop (`paraclete-app/src/main.rs`) runs at ~1 ms intervals and must execute these steps in order each iteration:

1. `conf.process_main_thread()` — drain state bus SPSC from audio thread; tick all `SurfaceOutputHandle`s
2. Drain per-device gateway SPSCs (`consumer_lp.drain()`, `consumer_dt.drain()`, `consumer_ks.drain()`) into a shared event buffer
3. `scripting.dispatch_surface_event(ev)` — dispatch each event to registered Rhai handlers
4. `scripting.process_subscriptions(&bus)` — fire state bus subscription callbacks for changed paths
5. `conf.send_command(cmd)` — flush `NodeCommand`s produced by scripts to the audio thread ring buffer
6. `conf.deliver_script_output(led_output)` — route LED/output commands from scripts to hardware handles

When adding a new hardware device, add a `ScriptingGatewayNode` for it (with its device ID) and drain its consumer in step 2. The gateway's `device_id` tags events so Rhai handlers can identify the source.

## Dynamic Topology (P9)

**`NodeRegistry`** (in `paraclete-app`) maps type-tag strings to zero-argument constructors. `build_registry()` registers all first-party node types. `registry.build("filter")` returns `Box<dyn Node>`. CLAP plugin nodes are not registered (require `PluginLibrary` argument; inserted directly).

**`apply_patch(changes, engine, conf, registry)`** (in `paraclete-app`) applies a batch of `TopologyChange` variants atomically via pause-rebuild-resume: `engine.pause()` → `engine.wait_paused()` → apply changes to `NodeConfigurator` → `conf.rebuild_executor()` → `engine.resume_with_executor(executor)`. One buffer of silence (~5 ms) on each call. Returns `Vec<u32>` (new node IDs for `AddNode` entries). Fails on unknown type_tag, cycle without `LoopBreakNode`, or unknown node ID. Prior changes in a failed batch are NOT rolled back.

**`TopologyChange` variants:** `AddNode { type_tag, initial_params }`, `RemoveNode { id }`, `AddEdge { src, src_port, dst, dst_port }`, `RemoveEdge { src, src_port, dst, dst_port }`.

**`AudioEngine` pause protocol:** `pause()` (non-blocking signal) → `wait_paused()` (blocks up to 500 ms; panics if exceeded — indicates hung audio thread) → `resume_with_executor(executor)` (swaps executor and resumes).

**Project file format v2 (P9):** `NodeSnapshot` gains `type_tag: String` (construction key). Edges are now authoritative — v2 loads restore topology from the file. v1 files remain loadable (warning emitted; topology unchanged). `save_project()` writes version 2. `load_project()` reads both.

## Design Documents

**Vocabulary note (July 2026):** the `Hardware*` type family was renamed to `Surface*` while the project had zero external users: `HardwareDevice` → `Surface` (method `surface()` → `descriptor()`), `HardwareEvent` → `SurfaceEvent`, `HardwareOutput`/`Handle` → `SurfaceOutput`/`Handle`, `Event::Hardware` → `Event::Surface`, `add_hardware_device` → `add_surface`, Rhai `on_hw_event` → `on_surface_event`, `HardwareMappingNode` → `SurfaceMappingNode`. Phase specs and ADRs written before July 2026 use the old names — map accordingly; do not edit those historical documents.

All design documents live in `design/`. Key documents:

- `design/architecture-core.md` — stable: layer model, Node API, signal types, design principles (binding constraints)
- `design/architecture-evolving.md` — append-only phase log; records what shipped and what was found per phase
- `design/roadmap.md` — living: current phase scope, known provisional implementations, open questions
- `design/handoff.md` — living: task routing by model tier, guardrails, current sequence; read before implementing
- `design/interface-plan.md` — accepted plan: Antiphon interface server + Theoria clients (web/terminal), W0–W4 phases, naming & trademark policy
- `design/sessions/` — paired-usage session notes (append-only; one file per session; each produces an explicit roadmap delta)
- `design/instrument-vision.md` — the concrete instrument being built; design tiebreaker for ambiguous decisions
- `design/bugs.md` — **append-only** known bug tracker; BUG-001 through BUG-008 currently open (BUG-008: `set_initial_params` re-applied on every `activate()`, overwrites deserialized state); add new bugs at the bottom with severity, phase found, location, and fix direction; mark resolved with commit reference
- `design/adr/` — all Architecture Decision Records (**append-only** — never edit a past ADR, add a new one to supersede it)
- `design/adr/ADR-005-scheduler-cycles.md` — why the graph must be a DAG (cycle rejection)
- `design/adr/ADR-008-node-api-level.md` — the three engagement levels
- `design/adr/ADR-016-multi-track-sequencer.md` — multi-track = multiple node instances
- `design/adr/ADR-017-effect-node-architecture.md` — effects are plain nodes; `AudioEffect` is a marker only
- `design/adr/ADR-018-cellular-architecture.md` — no non-node platform components; Node API extension is the answer
- `design/adr/ADR-019-universal-parameter-control.md` — `CMD_SET_PARAM`/`CMD_BUMP_PARAM`; `ParameterBank` spec
- `design/adr/ADR-022-node-portability.md` — portability rule: nodes link L2 only; machine bank architecture; machine variant pattern; signal port portability note (P6.5 amendment)
- `design/adr/ADR-023-instrument-encapsulation.md` — `GraphNode` primitive (P9); composition hierarchy from primitives to instruments; public framing rules
- `design/adr/ADR-024-clap-wrapper.md` — CLAP plugin adapter design (hand-rolled, P7); `paraclete-clap` crate; `ClapParamBridge`; DAW transport → `TransportInfo` translation; `nih-plug` removed
- `design/adr/ADR-025-project-file-format.md` — RON project file format (P7); `NodeSnapshot`; save/load path; versioning; what is not persisted
- `design/adr/ADR-026-instrument-definition-tui.md` — declarative YAML instrument file (P8); terminal UI as primary feedback surface; `publish_context()` Rhai API; macro pre-population
- `design/adr/ADR-027-clap-host.md` — Paraclete as CLAP host (P8); `paraclete-clap-host` crate; `PluginLibrary` / `PluginNode`; generator plugins at P8, effect plugins at P9; `clap-sys` + `libloading` (spec referenced `clack` which is a crates.io placeholder); amended June 2026 to record P8 implementation findings
- `design/adr/ADR-028-loop-break-node.md` — single-sample feedback loop break (P9); `LoopBreakNode`; executor pre/post phases; `MissingLoopBreak`/`TooManyLoopBreaks` error variants; CvSignal only; resolves OQ-5
- `design/adr/ADR-029-dynamic-topology.md` — patchable node graph (P9); pause-rebuild-resume protocol; `NodeRegistry`; `apply_patch()`; `AudioEngine::pause()`/`wait_paused()`/`resume_with_executor()`; project file format v2 with `type_tag`; `cap_doc_cache` prerequisite
- `design/adr/ADR-031-antiphon-interface-server.md` — interface server (W-track); pluggable transports (WebSocket + in-process, no tokio); surface/semantic command planes; surfaces as device nodes; plain wire names
- `design/adr/ADR-030-pattern-engine.md` — pattern engine (P10); `Pattern` struct; `Sequencer` owns `Vec<Pattern>`; multi-page (64-step) patterns + page-loop window; per-track length & speed (polyrhythm); seamless cued switching + chain; serializer v3 (resolves BUG-005); pattern-within-node, not pattern-as-topology
- `design/architecture-evolving-append.md` — retroactive phase log entries for P4–P8 (append to `architecture-evolving.md`)
- `design/phases/` — per-phase interface specs and implementation reports (append-only once a phase ships)
- `design/phases/p3-interfaces.md` — P3 spec: `Negotiable` trait, parameter lock protocol, `Sampler` node, StateBus SPSC upgrade, Rhai bindings
- `design/phases/p4-interfaces.md` — P4 spec: hardware integration, `ParameterBank`, `NodeCommand`, `SurfaceOutputHandle`, effect nodes, profile scripts
- `design/phases/p4-report.md` — P4 implementation report: what shipped, node IDs, test coverage, deferred items, post-commit analysis
- `design/phases/p5-interfaces.md` — P4.5 + P5 spec: foundation fixes + Sequencer v2, ReverbNode, DelayNode, SplitNode
- `design/phases/p5-report.md` — P5 implementation report: what shipped, deviations, test counts
- `design/phases/p6-interfaces.md` — P6 spec: synthesis primitive vocabulary, OscillatorNode, EnvelopeNode, LfoNode, LadderFilterNode, AnalogEngine (Kick/Snare/HiHat machines), FmEngine (Kick/Bell/Bass machines), Sampler audio quality (rubato + symphonia + envelope DSP); includes full DSP pseudocode and per-commit test lists
- `design/phases/p6-report.md` — P6 implementation report: what shipped per commit, post-ship code review findings and fixes, test counts (285 total)
- `design/phases/p6.5-plan.md` — P6.5 scope: 4 commits (Looping AD fix, LadderFilter param rename, signal port executor allocation, docs); pre-P7 gate criteria
- `design/phases/p6.5-report.md` — P6.5 implementation report
- `design/phases/p7-interfaces.md` — P7 spec: type_name(), published_state() push-down, per-voice Sampler resampling, project save/recall, paraclete-clap infrastructure, crates.io prep
- `design/phases/p7-report.md` — P7 implementation report: 317 tests, 0 failures; CLAP flag bug fixes; rubato reset/set_ratio ordering; code review findings
- `design/phases/p8-interfaces.md` — P8 spec: P7 residuals (gate), publish_context(), instrument YAML, paraclete-tui, paraclete-clap-host, app wiring
- `design/phases/p8-report.md` — P8 implementation report: 340 tests, 0 failures; CLAP FFI machine bank split; Arc<PluginLibrary> bundle fix; code review findings per commit
- `design/phases/p9-interfaces.md` — P9 spec: encoder value publishing (gated Commit 1), housekeeping, loop break, dynamic topology, sequencer CV, GraphNode/InnerGraphNode; 389 test target
- `design/phases/p9-report.md` — P9 implementation report: 389/391 tests, 0 failures; per-commit deltas; no new bugs (BUG-008 added in C2); deferred-items list for P10
- `design/phases/p9.5-interfaces.md` — P9.5 spec (Device Emulation & Test Harness): full Launchpad emulator (track-cursor keyboard scheme, all 64 pads + scene + control row, gates p10 C5); RGB LED feedback render; virtual relative encoders + `CMD_BUMP_PARAM` acceleration; piano mode + headless input injection for CI
- `design/phases/w0-interfaces.md` — W0 spec (Antiphon + Theoria grid POC): protocol v0 message schema, `paraclete-antiphon` crate layout, threading model, `TheoriaSurfaceNode`, zero-build client, per-commit test list, exit criteria
- `design/phases/w1-interfaces.md` — W1 spec (Theoria MVP): CMD_TRIGGER, state-path unification (+BUG-007), kerygma state mirror, semantic plane, TS client workspace + encoder row, paired-session exit criteria
- `design/phases/p10-interfaces.md` — P10 spec (Pattern Engine): pre-flight BUG-008 fix; `Pattern` struct + multi-pattern storage + serializer v3 (gated, fixes BUG-005); multi-page patterns + page-loop window; per-track length & speed (polyrhythm); seamless cued switching + chain; state-bus/TUI/Launchpad surface; new command IDs 28–32; ~410 test target
- `design/review/` — post-phase code review reports (`p4-code-review.md`, `p4-fixes-review.md`, etc.)
- `profiles/` — Rhai profile scripts loaded at startup (`launchpad.rhai`, `digitakt.rhai`, `keystep.rhai`, `launchpad_overview.rhai`). Constants `LP_DEVICE_ID`, `DT_DEVICE_ID`, `KS_DEVICE_ID`, `TRACK_SEQ_IDS`, etc. injected per profile.
