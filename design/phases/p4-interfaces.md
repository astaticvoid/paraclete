# Paraclete — P4 Interface Specification

> **Implementation blueprint.** These are the contracts P4 implementation must satisfy.
> Do not deviate without updating the relevant ADR first.
>
> **Deliverable:** Sit down with the three devices and make a beat. Mouse never touched.
> **Phase:** P4 — Hardware Integration
> **Last updated:** May 2026
> **Depends on:** p0–p3 interfaces and reports — all prior types assumed present and correct.
> **References:** ADR-018 (cellular architecture), architecture-core.md (updated principles)

---

## Overview

P4 is the first phase where the instrument is genuinely playable. Thirteen things are new:

1. **Real MIDI hardware** — `LaunchpadNode`, `DigitaktMidiNode`, `KeystepNode`. Real
   devices as `HardwareDevice: Node` in the graph per ADR-018. `midir` is the MIDI I/O
   layer.

2. **`HardwareOutputHandle` trait in L2** — the standard pattern for hardware nodes
   that need main-thread output (LEDs, motor faders). `take_output_handle()` added to
   `HardwareDevice`. Resolves the P3-deferred "update_output() on audio thread" problem
   without moving devices out of the graph.

3. **8-track drum machine graph** — 8 Sequencer + Sampler + Distortion + Filter chains
   feeding a `MixNode`. ADR-016 topology realized.

4. **Effect nodes** — `DistortionNode`, `FilterNode`, `MixNode`. All plain nodes per
   ADR-017. All use `ParameterBank`.

5. **`ParameterBank` in L2** — pre-allocated parameter storage built from
   `capability_document()` at `activate()` time. Handles `CMD_SET_PARAM` and
   `CMD_BUMP_PARAM` generically and allocation-free. The enabling infrastructure for
   universal hardware→parameter control.

6. **`ScriptingGatewayNode`** — a single fan-in node that hardware devices connect to.
   Bridges the graph to the scripting layer via SPSC. One per instrument graph.

7. **`ScriptContext` with owned registrations** — each profile script runs in an
   isolated Rhai scope. All handlers, subscriptions, and macro bindings are owned by
   the context. `reload_profile(name)` tears down and re-evaluates atomically. Hot-reload
   ships at P4.

8. **State bus subscriptions in Rhai** — `subscribe(path, fn)` builtin. Fires
   immediately with current value on registration, then on every change. Nodes publish
   every cycle; callbacks fire only when value changes. `state_alive(path, timeout_ms)`
   for liveness checking without pings.

9. **Universal parameter control** — `CMD_SET_PARAM` and `CMD_BUMP_PARAM` are universal
   `NodeCommand` type IDs. Any hardware encoder can reach any parameter declared in any
   node's `capability_document()` generically.

10. **Digitakt MIDI mode** — replaces the Launch Control XL as the contextual parameter
    controller. Endless (relative) encoders. No calibration gesture. `EncoderBehaviour::
    Relative` exercised against a real device.

11. **Profile scripts** — `launchpad.rhai`, `digitakt.rhai`, `keystep.rhai`. Each in
    an isolated `ScriptContext`. Cross-script state lives in the state bus.

12. **Surface descriptor iteration fix** — hardcoded `for pad in 0..16` in executor
    replaced by iterating `device.surface().controls`.

13. **egui developer window** — state bus monitor and node graph view. `--dev-ui` flag.

---

## P4 Graph

```
InternalClock
  ├──→ Sequencer[0] (Kick)   → Sampler[0] → Distortion[0] → Filter[0] ──┐
  ├──→ Sequencer[1] (Snare)  → Sampler[1] → Distortion[1] → Filter[1] ──┤
  ├──→ Sequencer[2] (Hat CH) → Sampler[2] → Distortion[2] → Filter[2] ──┤
  ├──→ Sequencer[3] (Hat OH) → Sampler[3] → Distortion[3] → Filter[3] ──┼──→ MixNode → AudioOutput
  ├──→ Sequencer[4] (Perc A) → Sampler[4] → Distortion[4] → Filter[4] ──┤
  ├──→ Sequencer[5] (Perc B) → Sampler[5] → Distortion[5] → Filter[5] ──┤
  ├──→ Sequencer[6] (FX)     → Sampler[6] → Distortion[6] → Filter[6] ──┤
  └──→ Sequencer[7] (Bass)   → Sampler[7] → Distortion[7] → Filter[7] ──┘
         ↑ events_in
  KeystepNode → HardwareMappingNode

  LaunchpadNode ──┐
  DigitaktNode   ─┼──→ ScriptingGatewayNode → SPSC → ScriptingEngine
  KeystepNode    ─┘     (KeystepNode optional — only wire if script needs it)
```

All hardware devices are `HardwareDevice: Node` in the graph per ADR-018. The
`ScriptingGatewayNode` is the single bridge from graph events to the scripting layer.

---

## What P4 Deliberately Defers

- `Scriptable` trait (synchronous node queries from scripts) — P5/P6
- Send routing (SplitNode is P5; MixNode receives track outputs directly)
- LaunchpadEmulator egui rewrite — terminal emulator unchanged
- MIDI 2.0 CI negotiation (OQ-3) — standard MIDI only at P4
- XL Mk2 support — `EncoderBehaviour::Absolute` path deferred; Digitakt is primary
- Sequencer v2 features — P5

---

## New Crate Dependencies

### `paraclete-hal` (GPL-3.0)
```toml
midir = "0.9"   # Cross-platform MIDI I/O — MIT
```

### `paraclete-app` (GPL-3.0)
```toml
eframe = "0.27"   # MIT/Apache-2.0
egui   = "0.27"   # MIT/Apache-2.0
```

---

## Part 1: `paraclete-node-api` Changes

### `EncoderBehaviour` — new

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EncoderBehaviour {
    /// Sends absolute position. range_max = 127 for standard 7-bit CC.
    Absolute { range_max: u16 },
    /// Sends signed delta. Positive = clockwise. Used by Digitakt and XL Mk3.
    Relative,
}
```

### `EncoderDescriptor` — updated

```rust
pub struct EncoderDescriptor {
    pub id: u32,
    pub name: PortName,
    pub has_push: bool,
    pub behaviour: EncoderBehaviour,   // replaces `endless: bool`
}
```

### `HardwareOutputHandle` — new trait in L2

```rust
/// Main-thread output handler for a hardware device.
///
/// Created by `HardwareDevice::take_output_handle()` at registration time.
/// The configurator holds it and calls `tick()` each main loop iteration.
/// The device's audio-thread side pushes `HardwareOutput` to an internal
/// SPSC; `tick()` drains it and sends to the physical device.
pub trait HardwareOutputHandle: Send {
    /// Called by the configurator each main loop iteration (~1 ms).
    /// Drain pending output and deliver to the physical device.
    /// Must not block.
    fn tick(&mut self);
}
```

### `HardwareDevice` — updated

```rust
pub trait HardwareDevice: Node {
    fn surface(&self) -> &SurfaceDescriptor;

    /// Receive output state for forwarding to physical device.
    /// Called by the executor (audio thread) on existing HardwareDevice
    /// implementations that have not yet been migrated to take_output_handle().
    /// New P4+ devices implement take_output_handle() instead.
    fn update_output(&mut self, output: &HardwareOutput);

    /// Called once by the configurator after registration.
    /// Returns the main-thread output handle if this device has physical output
    /// (LEDs, motor faders, displays). Returns None for receive-only devices.
    ///
    /// When Some is returned, the configurator calls handle.tick() each main
    /// loop iteration instead of calling update_output() from the executor.
    fn take_output_handle(&mut self) -> Option<Box<dyn HardwareOutputHandle>> {
        None   // default: no output handle (uses legacy update_output path)
    }
}
```

Migration path: existing `LaunchpadEmulator` uses `update_output()` unchanged.
New P4 devices (`LaunchpadNode`, `DigitaktMidiNode`) implement `take_output_handle()`.

### `NodeCommand` — new, with universal constants

```rust
/// A command directed at a specific graph node.
/// Fixed-size (24 bytes). Safe in a lock-free ring buffer.
#[derive(Clone, Copy, Debug)]
pub struct NodeCommand {
    pub target_id: u32,
    pub type_id: u32,    // node-defined command type
    pub arg0: i64,       // integer argument (param_id, step index, etc.)
    pub arg1: f64,       // float argument (value, delta, etc.)
}

/// Universal: set a parameter to an absolute value.
/// arg0 = param_id, arg1 = value (clamped to declared range by ParameterBank).
pub const CMD_SET_PARAM: u32 = 0;

/// Universal: adjust a parameter by a signed delta.
/// arg0 = param_id, arg1 = delta (ParameterBank clamps result to range).
pub const CMD_BUMP_PARAM: u32 = 1;

// Node-specific type IDs start at 16 to leave room for future universal commands.
```

### `ParameterBank` — new in L2

```rust
/// Pre-allocated parameter storage built from a node's capability document.
/// Handles CMD_SET_PARAM and CMD_BUMP_PARAM with zero allocation.
/// Build at activate() time; read in process().
///
/// Usage:
///   fn activate(&mut self, sr: f32, block: usize) {
///       self.bank = ParameterBank::from_capability_document(
///           &self.capability_document()
///       );
///   }
///   fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
///       self.bank.handle_commands(input.commands());
///       let drive = self.bank.get(PARAM_DRIVE);
///       // ... DSP using drive
///   }
pub struct ParameterBank {
    slots: Vec<ParameterSlot>,   // pre-allocated; no audio-thread allocation
}

struct ParameterSlot {
    param_id: u32,
    current:  f64,
    min:      f64,
    max:      f64,
    default:  f64,
}

impl ParameterBank {
    /// Build from a capability document. Called at activate() time.
    pub fn from_capability_document(doc: &CapabilityDocument) -> Self;

    /// Handle CMD_SET_PARAM and CMD_BUMP_PARAM from input.commands().
    /// All other command type_ids are silently ignored.
    /// Allocation-free. Call before any DSP in process().
    pub fn handle_commands(&mut self, commands: &[NodeCommand]);

    /// Read the current value of a parameter by ID.
    /// Returns the declared default if param_id is not found.
    pub fn get(&self, param_id: u32) -> f64;

    /// Set a parameter value directly (clamped to declared range).
    pub fn set(&mut self, param_id: u32, value: f64);
}
```

### `ProcessInput` — updated

```rust
impl<'a> ProcessInput<'a> {
    // Signal port views — wired at P4 when connected (was unimplemented!() at P3)
    pub fn modulation(&self, port_id: u32) -> &ModulationBuffer;
    pub fn cv(&self, port_id: u32) -> &CvBuffer;
    pub fn phase(&self, port_id: u32) -> &PhaseBuffer;
    pub fn logic(&self, port_id: u32) -> &LogicBuffer;
    pub fn pitch(&self, port_id: u32) -> &PitchBuffer;

    // New at P4
    pub fn commands(&self) -> &[NodeCommand];
}
```

---

## Part 2: `paraclete-runtime` Changes

### `NodeCommand` SPSC

`rtrb::RingBuffer<NodeCommand>` with capacity 512. Configurator holds producer,
executor holds consumer. Executor drains at start of each cycle into a
`HashMap<u32, Vec<NodeCommand>>` keyed by `target_id`. Each node's `ProcessInput`
receives a slice from this map. Pre-allocated accumulator — no audio-thread allocation.

### `NodeConfigurator` additions

```rust
impl NodeConfigurator {
    /// Send a NodeCommand to a graph node. Delivered on the next audio cycle.
    /// Returns Err if ring buffer is full — caller retries next main loop tick.
    pub fn send_command(&mut self, cmd: NodeCommand) -> Result<(), NodeCommand>;

    /// Register a hardware device's output handle.
    /// Called internally after add_hardware_device() if take_output_handle()
    /// returns Some. The handle is stored and ticked each main loop iteration.
    fn register_output_handle(&mut self, handle: Box<dyn HardwareOutputHandle>);

    /// Main-thread callback. Call once per main loop iteration (~1 ms).
    /// Drains the StateBus SPSC and ticks all registered HardwareOutputHandles.
    /// Does NOT involve the scripting engine — app layer handles dispatch.
    pub fn process_main_thread(&mut self);

    /// Drain the ScriptingGateway's event consumer.
    /// Returns events collected since last call. Pre-allocated output vec
    /// passed in by caller to avoid allocation in the hot path.
    pub fn drain_script_events(&mut self, out: &mut Vec<HardwareEventMsg>);
}
```

`process_main_thread()` does not take a `ScriptingEngine` parameter. The app layer
(GPL3, can reference all layers) orchestrates the dispatch sequence. See Part 6.

### Signal Port Buffer Wiring

At P3, signal port views returned `unimplemented!()` when connected. At P4 the
executor populates `signal_inputs` in `ProcessInput` from per-connection buffers
allocated at `build_executor()` time. No API change for nodes.

### Surface Descriptor Iteration Fix

```rust
// executor.rs — replaces `for pad in 0..16`
for control in device.surface().controls.iter() {
    match control {
        Control::Pad(pad)     => { /* build LedUpdate for pad.id */ }
        Control::Button(btn)  => { /* build LedUpdate for btn.id if rgb */ }
        _                     => {}
    }
}
```

---

## Part 3: `paraclete-hal` Changes

### MIDI Device Registry

```rust
// paraclete-hal::midi — internal utility, not public API
pub struct MidiDeviceRegistry {
    midi_in:  midir::MidiInput,
    midi_out: midir::MidiOutput,
}

impl MidiDeviceRegistry {
    pub fn new() -> Result<Self, midir::InitError>;
    pub fn input_ports(&self) -> Vec<String>;
    pub fn output_ports(&self) -> Vec<String>;
    /// Open input port by substring match.
    pub fn open_input(
        &self, name: &str,
        cb: impl Fn(u64, &[u8]) + Send + 'static,
    ) -> Result<midir::MidiInputConnection<()>, MidiDeviceError>;
    pub fn open_output(&self, name: &str)
        -> Result<midir::MidiOutputConnection, MidiDeviceError>;
}
```

### `LaunchpadNode` — `HardwareDevice: Node`

```rust
pub struct LaunchpadNode {
    device_id: u32,
    _conn_in:  midir::MidiInputConnection<()>,
    // output handle takes ownership of conn_out at take_output_handle() time
    conn_out:  Option<midir::MidiOutputConnection>,
    incoming:  Arc<Mutex<VecDeque<HardwareEvent>>>,
    hw_out_tx: rtrb::Producer<HardwareOutput>,
    surface:   SurfaceDescriptor,
}

impl LaunchpadNode {
    pub fn open() -> Result<Self, MidiDeviceError>;
}

impl Node for LaunchpadNode {
    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        // drain incoming (midir callback thread → audio thread)
        // emit Event::Hardware for each received event
    }
}

impl HardwareDevice for LaunchpadNode {
    fn surface(&self) -> &SurfaceDescriptor { &self.surface }

    fn update_output(&mut self, _: &HardwareOutput) { /* unused — handle path */ }

    fn take_output_handle(&mut self) -> Option<Box<dyn HardwareOutputHandle>> {
        // Move conn_out and hw_out_rx into LaunchpadOutputHandle
        // Returns None if called a second time (take semantics)
        let conn_out = self.conn_out.take()?;
        let (_, rx) = /* previously created SPSC pair */ ...;
        Some(Box::new(LaunchpadOutputHandle { conn_out, rx }))
    }
}

// Main-thread output handle
struct LaunchpadOutputHandle {
    conn_out: midir::MidiOutputConnection,
    rx:       rtrb::Consumer<HardwareOutput>,
}

impl HardwareOutputHandle for LaunchpadOutputHandle {
    fn tick(&mut self) {
        while let Ok(output) = self.rx.pop() {
            for update in &output.led_updates {
                self.send_note_on(update.control_id as u8,
                                  update.color.to_launchpad_index());
            }
        }
    }
}
```

**MIDI translation — Launchpad MK2:**
- Note On ch 0, vel > 0 → `PadPressed { id: note, velocity: vel << 9, pressure: 0 }`
- Note On ch 0, vel = 0 / Note Off ch 0 → `PadReleased { id: note }`
- CC ch 0 (scene/mode buttons) → `ButtonPressed` / `ButtonReleased`

**Output:**
- `LedUpdate { control_id, color }` → Note On ch 0, note = id, vel = nearest palette index

**Surface — Launchpad MK2:**
- 64 pads (ids 0–63, row-major), velocity-sensitive, RGB
- 8 scene buttons (ids 64–71), RGB
- 8 top buttons (ids 72–79), RGB

### `DigitaktMidiNode` — `HardwareDevice: Node`

Replaces `LaunchControlXlNode`. Digitakt in MIDI mode with relative encoders.

```rust
pub struct DigitaktMidiNode {
    device_id: u32,
    _conn_in:  midir::MidiInputConnection<()>,
    incoming:  Arc<Mutex<VecDeque<HardwareEvent>>>,
    surface:   SurfaceDescriptor,
    // Digitakt has no LED output in standard MIDI mode — no output handle
}

impl DigitaktMidiNode {
    pub fn open() -> Result<Self, MidiDeviceError>;
}

impl Node for DigitaktMidiNode {
    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        // drain incoming MIDI
        // CC relative values → EncoderChanged { id, value: 0, delta: decoded_delta }
        // Track buttons → ButtonPressed / ButtonReleased
        // Transport buttons → ButtonPressed / ButtonReleased
    }
}

impl HardwareDevice for DigitaktMidiNode {
    fn surface(&self) -> &SurfaceDescriptor { &self.surface }
    fn update_output(&mut self, _: &HardwareOutput) {}
    fn take_output_handle(&mut self) -> Option<Box<dyn HardwareOutputHandle>> {
        None   // Digitakt has no LED feedback in standard MIDI mode
    }
}
```

**Relative encoder decoding (Digitakt default):**
- CC value 1–63: `delta = value as i16` (clockwise)
- CC value 65–127: `delta = (value as i16) - 128` (counter-clockwise)
- CC value 64: `delta = 0` (center detent, if any)

**Surface — Digitakt MIDI mode:**
- 8 track encoders (ids 0–7), `EncoderBehaviour::Relative`
- 8 track buttons (ids 8–15)
- Transport buttons: Play (id 16), Stop (id 17), Record (id 18)

### `KeystepNode` — unchanged from earlier spec

`HardwareDevice: Node`. `take_output_handle()` returns `None` (no LED output).
MIDI Note On/Off → `Event::Midi2`. Standard MIDI, no special protocol.

---

## Part 4: `paraclete-nodes` Changes

### `ScriptingGatewayNode` — new

Single fan-in node. Hardware devices connect their event outputs here.
Bridges graph events to the scripting layer via SPSC.

```rust
pub struct ScriptingGatewayNode {
    ports:      Vec<PortDescriptor>,   // dynamic — one per connected device
    device_ids: Vec<u32>,              // device_ids[port_idx] = source node id
    tx:         rtrb::Producer<HardwareEventMsg>,
}

impl ScriptingGatewayNode {
    /// capacity: ring buffer size (default 1024 events)
    pub fn new(capacity: usize) -> (Self, ScriptEventConsumer);
}

pub struct ScriptEventConsumer {
    rx: rtrb::Consumer<HardwareEventMsg>,
}

impl ScriptEventConsumer {
    pub fn drain(&mut self, out: &mut Vec<HardwareEventMsg>) {
        while let Ok(msg) = self.rx.pop() { out.push(msg); }
    }
}
```

**`set_connection_record()`** — when a hardware device connects to the gateway,
the runtime calls `set_connection_record()` on the gateway with the src node ID
and dst port ID. The gateway stores `device_ids[dst_port_id] = src_node_id`.

**`capabilities_changed()`** — returns true when a new port is added (i.e. after
`set_connection_record()` fires for a new source). The configurator re-queries the
capability document and updates the graph's port table.

**`process()`** — iterates input events by port index, tags with `device_ids[port_idx]`,
pushes `HardwareEventMsg` to SPSC.

### `MixNode` — new

N audio inputs → 1 stereo output. Uses `ParameterBank`.

```rust
pub struct MixNode {
    num_inputs: usize,
    bank:       ParameterBank,
    render_l:   Vec<f32>,   // pre-allocated at activate()
    render_r:   Vec<f32>,
}
impl MixNode { pub fn new(num_inputs: usize) -> Self; }
```

Ports: audio_in_0 … audio_in_{N-1} (ids 0..N-1), audio_out (id N).
Parameters: per-input gain (param_id = input_index, 0.0–2.0, default 1.0/N),
master gain (param_id = N, 0.0–2.0, default 1.0).

### `DistortionNode` — new

```rust
pub struct DistortionNode {
    bank: ParameterBank,
}
```

Ports: audio_in, audio_out.
Parameters: drive (0: 0.0–1.0, default 0.0), output_level (1: -24–+6 dB, default 0.0),
blend (2: 0.0–1.0, default 1.0).
DSP: `tanh(input * exp(drive * 4.0)) * db_to_linear(output_level)`, lerped with dry.

### `FilterNode` — new

```rust
pub struct FilterNode {
    bank:    ParameterBank,
    low_l:   f32, band_l: f32,
    low_r:   f32, band_r: f32,
    f_coeff: f32,
    q_coeff: f32,
    sr:      f32,
}
```

Ports: audio_in, audio_out.
Parameters: cutoff_hz (0: 20–20000, default 1000), resonance (1: 0.1–4.0, default 0.7),
filter_type (2: 0–3 as LowPass/HighPass/BandPass/Notch, default 0).
DSP: Chamberlin SVF. `f_coeff` recomputed at activate() and when cutoff changes.

### `Sequencer` — updated

Node-specific command constants start at 16 (below universal CMD_SET_PARAM/CMD_BUMP_PARAM):

```rust
impl Sequencer {
    pub const CMD_TOGGLE_STEP: u32 = 16;  // arg0 = step_index
    pub const CMD_SET_STEP:    u32 = 17;  // arg0 = step, arg1 = note (< 0 = deactivate)
    pub const CMD_CLEAR:       u32 = 18;  // clear all steps
}
```

New state bus publications:
```
/node/{id}/state/steps       — 16-char ASCII bitfield e.g. "1010000010100000"
/node/{id}/state/track_name  — e.g. "Kick"
```

`process()` drains `input.commands()` before clock/event logic. Also uses
`ParameterBank` for lockable parameters (pitch, velocity_scale, pattern_length).

---

## Part 5: `paraclete-scripting` Changes

### `ScriptContext` — new

Each profile script runs in a named `ScriptContext` with an isolated Rhai scope.
All registrations made during evaluation are owned by the context.

```rust
pub struct ScriptContext {
    name:          String,
    scope:         rhai::Scope<'static>,
    hw_handlers:   HashMap<u32, rhai::FnPtr>,
    subscriptions: Vec<OwnedSubscription>,
    macros:        HashMap<String, rhai::FnPtr>,
    bindings:      HashMap<EventSignature, String>,
}

impl ScriptContext {
    pub fn teardown(&mut self) {
        // Remove all owned registrations
        self.hw_handlers.clear();
        self.subscriptions.clear();
        self.macros.clear();
        self.bindings.clear();
        self.scope = rhai::Scope::new();
    }
}
```

Cross-script state (e.g. `selected_track`) lives in the state bus, not in script
variables. Isolated scopes are safe because scripts don't own platform state.
Constants (node IDs) are injected into each scope by the app layer before evaluation.

### `ScriptingEngine` — updated

```rust
impl ScriptingEngine {
    /// Evaluate a file into a named ScriptContext.
    /// If a context with this name already exists, it is torn down first.
    /// Calls on_load() in the script if defined.
    pub fn eval_file(&mut self, name: &str, path: &str,
                     state_bus: &StateBusHandle) -> Result<(), ScriptError>;

    /// Inject a constant into the next eval_file() call's scope.
    pub fn set_constant(&mut self, key: &str, value: impl Into<rhai::Dynamic>);

    /// Dispatch a hardware event to registered handlers and macro bindings.
    pub fn dispatch_hardware_event(
        &mut self,
        msg: &HardwareEventMsg,
        state_bus: &mut StateBusHandle,
        cmd_tx: &mut dyn Fn(NodeCommand),
        led_out: &mut HashMap<u32, HardwareOutput>,
    );

    /// Fire subscription callbacks for any state bus paths that changed.
    /// Called after drain_state_bus() updates the StateBusHandle.
    pub fn process_subscriptions(&mut self, state_bus: &StateBusHandle);

    /// Collect pending LED output accumulated by set_led() calls.
    pub fn take_pending_output(&mut self) -> HashMap<u32, HardwareOutput>;
}
```

`dispatch_hardware_event()` and `process_subscriptions()` are called by the app
layer, not by `NodeConfigurator`. L1 does not reference L4.

### State Bus Subscriptions

```rust
// Rhai builtin — subscribe(path, fn)
// Fires immediately with current value on registration.
// Fires again whenever the value changes (not on every write).
subscribe("/node/42/state/current_step", |new_val| {
    refresh_track_leds(42, new_val);
});

// Rhai builtin — state_alive(path, timeout_ms)
// Returns true if the path has been written within timeout_ms.
// Nodes publish every cycle — silence means the node is gone.
if !state_alive("/node/42/state/current_step", 500) {
    // node has not published in 500ms — something is wrong
}
```

`OwnedSubscription` stored in `ScriptContext` tracks `path`, `last_value`,
`last_written_at: Instant`, and the callback `FnPtr`. `process_subscriptions()`
iterates all active subscriptions across all contexts, checks for changed values,
and fires callbacks.

`last_written_at` is updated by the state bus drain each main loop tick.
`StateBusHandle` gains a `last_written: HashMap<String, Instant>` field alongside
the existing value map.

### Macro System v1

Macros are `FnPtr` values stored in the active `ScriptContext`. Bindings map
`EventSignature { device_id, event_type, control_id }` to a macro name.

```rhai
// Lifecycle hook — called after script evaluation
fn on_load() {
    def_macro("play_stop", || {
        let p = state_read("/transport/playing");
        state_write("/transport/playing", !p);
    });
    bind_macro(LP_DEVICE_ID, "ButtonPressed", 72, "play_stop");
    on_hw_event(LP_DEVICE_ID, |event| { handle_launchpad(event); });
}
```

### New Rhai Builtins

| Builtin | Description |
|---|---|
| `subscribe(path, fn)` | Subscribe to a state bus path |
| `state_alive(path, ms)` | Check if path was written within ms |
| `on_hw_event(device_id, fn)` | Register hardware event handler |
| `def_macro(name, fn)` | Define a named macro |
| `bind_macro(dev, type, id, name)` | Bind macro to hardware event |
| `fire_macro(name)` | Manually fire a macro |
| `send_cmd(node_id, type_id, a0, a1)` | Send NodeCommand |
| `set_led(device_id, id, r, g, b)` | Accumulate LED update |
| `reload_profile(name)` | Tear down and re-evaluate a ScriptContext |
| `get_step_state(node_id)` | Return Sequencer steps bitfield string |

---

## Part 6: `paraclete-app` Changes

### Graph Wiring

```rust
fn build_graph(conf: &mut NodeConfigurator) -> Ids {
    let clock_id  = conf.add_tempo_source(Box::new(InternalClock::new(140.0)));
    let mix_id    = conf.add_node(Box::new(MixNode::new(8)));
    let output_id = conf.add_node(Box::new(AudioOutput::new()));
    conf.connect(mix_id, MixNode::PORT_AUDIO_OUT,
                 output_id, AudioOutput::PORT_AUDIO_IN).unwrap();

    let mut ids = Ids::default();

    for i in 0..8usize {
        let seq_id  = conf.add_node(Box::new(Sequencer::with_name(TRACK_NAMES[i])));
        let samp_id = conf.add_node(Box::new(Sampler::new()));
        let dist_id = conf.add_node(Box::new(DistortionNode::new()));
        let filt_id = conf.add_node(Box::new(FilterNode::new()));

        conf.connect(clock_id, InternalClock::PORT_CLOCK_OUT,
                     seq_id,   Sequencer::PORT_CLOCK_IN).unwrap();
        conf.connect(seq_id,  Sequencer::PORT_EVENTS_OUT,
                     samp_id, Sampler::PORT_EVENTS_IN).unwrap();
        conf.connect(samp_id, Sampler::PORT_AUDIO_OUT,
                     dist_id, DistortionNode::PORT_AUDIO_IN).unwrap();
        conf.connect(dist_id, DistortionNode::PORT_AUDIO_OUT,
                     filt_id, FilterNode::PORT_AUDIO_IN).unwrap();
        conf.connect(filt_id, FilterNode::PORT_AUDIO_OUT,
                     mix_id,  i as u32).unwrap();

        ids.seq[i] = seq_id; ids.samp[i] = samp_id;
        ids.dist[i] = dist_id; ids.filt[i] = filt_id;
    }

    // Keystep → HardwareMappingNode → Bass Sequencer
    let keystep_id = conf.add_hardware_device(Box::new(
        KeystepNode::open().expect("Keystep not found")
    ));
    let mapper_id = conf.add_node(Box::new(
        HardwareMappingNode::default_chromatic(0)
    ));
    conf.connect(keystep_id, 0, mapper_id, 0).unwrap();
    conf.connect(mapper_id, 1, ids.seq[7], Sequencer::PORT_EVENTS_IN).unwrap();

    // ScriptingGatewayNode — hardware events to scripting
    let (gateway, script_consumer) = ScriptingGatewayNode::new(1024);
    let gateway_id = conf.add_node(Box::new(gateway));
    ids.gateway = gateway_id;

    let launchpad_id = conf.add_hardware_device(Box::new(
        LaunchpadNode::open().expect("Launchpad not found")
    ));
    let digitakt_id = conf.add_hardware_device(Box::new(
        DigitaktMidiNode::open().expect("Digitakt not found")
    ));

    conf.connect(launchpad_id, 0, gateway_id, 0).unwrap();
    conf.connect(digitakt_id,  0, gateway_id, 1).unwrap();
    conf.connect(keystep_id,   0, gateway_id, 2).unwrap();

    ids
}
```

### Profile Loading

```rust
fn load_profiles(scripting: &mut ScriptingEngine, ids: &Ids,
                 state_bus: &StateBusHandle) {
    // Inject constants into each profile's scope before eval
    for profile in &["launchpad", "digitakt", "keystep"] {
        scripting.set_constant("LP_DEVICE_ID",   ids.launchpad as i64);
        scripting.set_constant("DT_DEVICE_ID",   ids.digitakt  as i64);
        scripting.set_constant("KS_DEVICE_ID",   ids.keystep   as i64);
        scripting.set_constant("CLOCK_ID",       ids.clock     as i64);
        scripting.set_constant("TRACK_SEQ_IDS",
            ids.seq.iter().map(|&i| i as i64).collect::<Vec<_>>());
        scripting.set_constant("TRACK_SAMP_IDS",
            ids.samp.iter().map(|&i| i as i64).collect::<Vec<_>>());
        scripting.set_constant("TRACK_DIST_IDS",
            ids.dist.iter().map(|&i| i as i64).collect::<Vec<_>>());
        scripting.set_constant("TRACK_FILT_IDS",
            ids.filt.iter().map(|&i| i as i64).collect::<Vec<_>>());

        scripting.eval_file(
            profile,
            &format!("profiles/{profile}.rhai"),
            state_bus,
        ).unwrap_or_else(|e| eprintln!("Profile {profile} error: {e}"));
    }
}
```

### Main Loop

App layer orchestrates L1 and L4 — no L1/L4 coupling in either crate.

```rust
fn main_loop(
    conf:            &mut NodeConfigurator,
    scripting:       &mut ScriptingEngine,
    script_consumer: &mut ScriptEventConsumer,
    output_handles:  &mut Vec<Box<dyn HardwareOutputHandle>>,
) {
    let mut event_buf: Vec<HardwareEventMsg> = Vec::with_capacity(64);
    let mut cmd_buf:   Vec<NodeCommand>       = Vec::with_capacity(32);

    loop {
        std::thread::sleep(Duration::from_millis(1));

        // 1. Drain state bus SPSC → update StateBusHandle
        //    Tick all HardwareOutputHandles (send LEDs etc.)
        conf.process_main_thread();

        // 2. Drain ScriptingGatewayNode SPSC
        event_buf.clear();
        script_consumer.drain(&mut event_buf);

        // 3. Dispatch hardware events → fire Rhai handlers and macro bindings
        //    cmd_buf collects NodeCommands from send_cmd() calls in scripts
        let state_bus = conf.state_bus_handle_mut();
        for event in &event_buf {
            scripting.dispatch_hardware_event(
                event, state_bus,
                &mut |cmd| { cmd_buf.push(cmd); },
                &mut HashMap::new(),   // led_out — collected separately below
            );
        }

        // 4. Fire subscription callbacks for changed state bus values
        scripting.process_subscriptions(conf.state_bus_handle());

        // 5. Flush NodeCommands produced by scripts
        for cmd in cmd_buf.drain(..) {
            conf.send_command(cmd).ok();
        }

        // 6. Deliver LED output from scripts to device output handles
        let pending = scripting.take_pending_output();
        for handle in output_handles.iter_mut() {
            // handled inside tick() via internal SPSC — already called in step 1
            // set_led() output is routed to handles via device_id lookup
            let _ = pending;  // routing detail resolved during implementation
        }
    }
}
```

---

## Part 7: Profile Scripts

### `profiles/launchpad.rhai`

```rhai
fn on_load() {
    on_hw_event(LP_DEVICE_ID, |event| {
        if event.type == "PadPressed" {
            if event.row < 8 {
                // Toggle step on the selected track's sequencer
                let seq_id = TRACK_SEQ_IDS[event.row];
                send_cmd(seq_id, 16, event.col, 0.0); // CMD_TOGGLE_STEP
                state_write("/script/selected_track", event.row);
                // Subscribe fires update automatically — no explicit refresh needed
            }
        }
    });

    // Subscribe to step state for all 8 tracks — fires immediately with current state
    for track in 0..8 {
        let seq_id = TRACK_SEQ_IDS[track];
        subscribe(`/node/${seq_id}/state/steps`, |steps| {
            update_track_leds(track, steps);
        });
        subscribe(`/node/${seq_id}/state/current_step`, |step| {
            update_step_indicator(track, step);
        });
    }

    def_macro("play_stop", || {
        let p = state_read("/transport/playing");
        state_write("/transport/playing", !p);
    });
    bind_macro(LP_DEVICE_ID, "ButtonPressed", 72, "play_stop");
}

fn update_track_leds(track, steps) {
    for col in 0..8 {
        let active = steps[col] == '1';
        let color  = if active { [64, 64, 255] } else { [8, 8, 8] };
        set_led(LP_DEVICE_ID, track * 8 + col, color[0], color[1], color[2]);
    }
}

fn update_step_indicator(track, step) {
    set_led(LP_DEVICE_ID, track * 8 + step, 0, 255, 0); // green = current
}
```

### `profiles/digitakt.rhai`

```rhai
fn on_load() {
    // Subscribe to selected track so encoder routing stays current
    subscribe("/script/selected_track", |track| {
        // XL encoder routing already uses state_read — nothing to do here
    });

    on_hw_event(DT_DEVICE_ID, |event| {
        if event.type == "EncoderChanged" {
            let track = state_read("/script/selected_track").to_int();
            route_encoder(event.id, event.delta, track);
        }
        if event.type == "ButtonPressed" && event.id >= 8 && event.id <= 15 {
            // Track select buttons
            let track = event.id - 8;
            state_write("/script/selected_track", track);
        }
    });
}

fn route_encoder(enc_id, delta, track) {
    let normalized = delta / 64.0; // scale relative delta to approx -1..+1

    if      enc_id == 0 { send_cmd(TRACK_SAMP_IDS[track], 0, 0, normalized * 24.0); } // pitch
    else if enc_id == 1 { send_cmd(TRACK_DIST_IDS[track], 0, 1, normalized); }         // drive
    else if enc_id == 2 { send_cmd(TRACK_FILT_IDS[track], 1, 0, normalized * 4000.0); }// cutoff
    else if enc_id == 3 { send_cmd(TRACK_FILT_IDS[track], 1, 1, normalized * 0.5); }  // resonance
    // enc 4–7: available
}
```

*Note: `send_cmd` calls above use `CMD_BUMP_PARAM (type_id=1)` — arg0=param_id, arg1=delta.*

### `profiles/keystep.rhai`

```rhai
fn on_load() {
    // Keystep note events route through the graph directly (KeystepNode → HardwareMappingNode → Seq[7])
    // This profile handles mod wheel → filter cutoff on the bass track only
    on_hw_event(KS_DEVICE_ID, |event| {
        if event.type == "FaderMoved" && event.id == 1 {
            let cutoff = 200.0 + (event.value / 65535.0) * 4000.0;
            send_cmd(TRACK_FILT_IDS[7], 0, 0, cutoff); // CMD_SET_PARAM, cutoff_hz
        }
    });
}
```

---

## Decisions Recorded

| # | Decision | Notes |
|---|---|---|
| D1 | HardwareController trait rejected | ADR-018 — devices stay in graph as HardwareDevice: Node |
| D2 | HardwareOutputHandle::tick() in L2 | take_output_handle() on HardwareDevice; default returns None |
| D3 | ScriptingGatewayNode — one per graph | Fan-in from all hardware nodes; one SPSC to scripting layer |
| D4 | subscribe() fires immediately on registration | Current value delivered; no init boilerplate needed |
| D5 | Publish every cycle; fire on change | Continuous publish = liveness signal; state_alive() for health checks |
| D6 | ParameterBank in L2 at P4 | CMD_SET_PARAM/CMD_BUMP_PARAM universal; all P4 nodes use ParameterBank |
| D7 | Isolated ScriptContext scopes | Cross-script state lives in state bus; hot-reload of one script is clean |
| D8 | ScriptContext + reload_profile() at P4 | Infrastructure built once; hot-reload is a direct consequence |
| D9 | Digitakt MIDI mode replaces XL | Endless encoders; no calibration; EncoderBehaviour::Relative exercised |
| D10 | Scriptable trait deferred | P5/P6; NodeCommand + state bus covers P4 scripting needs |
| D11 | process_main_thread() takes no scripting param | App layer orchestrates L1 and L4; no layer violation |
| D12 | state_alive(path, ms) builtin | last_written: Instant per path in StateBusHandle |

---

## Test Coverage Targets

### `paraclete-node-api`
- `ParameterBank::handle_commands`: CMD_SET_PARAM clamps to declared range
- `ParameterBank::handle_commands`: CMD_BUMP_PARAM applies delta and clamps
- `ParameterBank::handle_commands`: unknown type_id silently ignored
- `ParameterBank::get`: returns default for unknown param_id
- `NodeCommand` is 24 bytes (size_of assertion)
- `HardwareOutputHandle` is object-safe

### `paraclete-hal`
- `LaunchpadNode` MIDI translation: Note On → PadPressed, Note Off → PadReleased
- `LaunchpadNode::take_output_handle()`: returns Some on first call, None on second
- `DigitaktMidiNode` relative decode: CC=1 → delta=+1, CC=127 → delta=-1, CC=65 → delta=-63
- `KeystepNode`: Note On ch 0 → Event::Midi2 NoteOn

### `paraclete-nodes`
- `ScriptingGatewayNode`: events tagged with correct device_id per port
- `ScriptingGatewayNode`: set_connection_record populates device_ids correctly
- `MixNode(8)`: 8 inputs sum to correct stereo output with unity gain
- `DistortionNode` at drive=0: output ≈ input
- `DistortionNode` at drive=1: output clamped near ±1.0
- `FilterNode` at high cutoff: signal passes with low attenuation
- `FilterNode` state zeroed between activate() calls
- `Sequencer` CMD_TOGGLE_STEP: step toggles correctly
- `Sequencer` CMD_CLEAR: all steps become inactive
- `Sequencer` publishes correct steps bitfield after toggle

### `paraclete-scripting`
- `subscribe()` fires immediately with current value on registration
- `subscribe()` fires again only when value changes, not on every write
- `state_alive()` returns false when path not written within timeout
- `reload_profile()` tears down old context completely before re-evaluation
- ScriptContext teardown removes all handlers and subscriptions
- Isolated scopes: variable defined in launchpad context not visible in digitakt context

### `paraclete-runtime`
- `send_command()` delivers to target node within one audio cycle
- `send_command()` returns Err when full, no panic
- Signal port views: connected port returns non-empty buffer
- Signal port views: unconnected port returns &[]

### Integration (`paraclete-app`)
- Graph builds and activates: 35 nodes, 0 panics
- Clock fan-out: all 8 Sequencers receive clock ticks
- CMD_TOGGLE_STEP via scripting → step active → Sampler fires on next clock tick
- subscribe() → Launchpad LED updates fire on step change

---

## File Changes Summary

```
paraclete-node-api/src/
  hardware.rs    ← EncoderBehaviour, EncoderDescriptor, HardwareDevice updated,
                    HardwareOutputHandle trait, HardwareEventMsg
  command.rs     ← new — NodeCommand, CMD_SET_PARAM, CMD_BUMP_PARAM
  parameter.rs   ← new — ParameterBank
  context.rs     ← signal port views wired, commands() accessor
  lib.rs         ← re-export new types

paraclete-hal/src/
  midi/mod.rs         ← new — MidiDeviceRegistry
  launchpad/mod.rs    ← new — LaunchpadNode, LaunchpadOutputHandle
  digitakt/mod.rs     ← new — DigitaktMidiNode
  keystep/mod.rs      ← new — KeystepNode
  emulator/           ← unchanged

paraclete-nodes/src/
  gateway.rs     ← new — ScriptingGatewayNode, ScriptEventConsumer
  mix.rs         ← new — MixNode
  distortion.rs  ← new — DistortionNode
  filter.rs      ← new — FilterNode
  sequencer.rs   ← ParameterBank, CMD_TOGGLE_STEP/SET_STEP/CLEAR, steps bitfield
  mod.rs         ← re-export

paraclete-runtime/src/
  configurator.rs ← send_command(), register_output_handle(), process_main_thread(),
                     drain_script_events(), NodeCommand SPSC producer,
                     state_bus last_written timestamps
  executor.rs     ← NodeCommand SPSC consumer, per-node dispatch,
                     signal port buffer wiring, surface descriptor iteration fix

paraclete-scripting/src/
  engine.rs      ← dispatch_hardware_event(), process_subscriptions(),
                    take_pending_output(), eval_file() with ScriptContext
  context.rs     ← new — ScriptContext, OwnedSubscription
  macros.rs      ← new — MacroRegistry, EventSignature, bind_macro()
  builtins.rs    ← subscribe(), state_alive(), on_hw_event(), def_macro(),
                    bind_macro(), fire_macro(), send_cmd(), set_led(),
                    reload_profile(), get_step_state()

paraclete-app/src/
  main.rs         ← updated — graph build, profile load, main loop
  graph.rs        ← new — build_graph(), Ids struct
  profile.rs      ← new — load_profiles(), constant injection
  dev_ui.rs       ← new — egui window (--dev-ui flag)

profiles/
  launchpad.rhai  ← new
  digitakt.rhai   ← new
  keystep.rhai    ← new
```
