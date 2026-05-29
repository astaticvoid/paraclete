# Paraclete — P2 Interface Specification

> **Implementation blueprint.** These are the contracts P2 implementation must satisfy.
> Do not deviate without updating the relevant ADR first.
>
> **Deliverable:** 16-step sequencer (*Sequencer*) driving the sine oscillator.
> Clock domain established. Basic parameter locks.
> **Phase:** P2 — Sequencer v1
> **Last updated:** May 2026
> **Depends on:** p0-interfaces.md, p1-interfaces.md — all prior types assumed present.

---

## Overview

P2 introduces three new subsystems:

1. **`TempoSource` trait and node** — clock domain ownership. The `TempoSource` node
   is the internal clock master. Any node that presides over a clock domain
   implements the `TempoSource` trait.

2. **`StateBus`** — the state bus. A publish/subscribe namespace. Nodes publish
   named values. Subscribers read them. The mechanism by which the sequencer
   exposes its current step to the Launchpad LEDs and the GUI.

3. **`Sequencer`** — the 16-step sequencer node. The P2 primary deliverable and
   the framework's first stress test. Every design decision made so far is
   validated against this node.

The P2 graph:

```
TempoSource → Sequencer → SineOscillator → AudioOutput
              ↑
         LaunchpadEmulator → HardwareMappingNode
```

The `Sequencer` receives clock ticks from the `TempoSource` and note events from
the mapping node. It emits note events to the oscillator on each active step.

---

## New Crate Dependencies

### `paraclete-node-api` (L2 — LGPL3)

No new external dependencies.

### `paraclete-runtime` (L1 — GPL3)

No new external dependencies. The `StateBus` is implemented entirely within
the runtime using standard library collections.

---

## Part 1: `TempoSource` — Clock Domain

### `trait TempoSource`

Any `Node` that can preside over a clock domain implements this trait.
The runtime registers `TempoSource` nodes and gives them authority over their domain.

Simple by default. A node that does not implement this trait does not own a
clock domain — it simply receives `TransportEvent` from its event stream.

```rust
/// A node that provides a clock domain.
pub trait TempoSource: Node {
    /// The domain identifier assigned by the runtime at registration.
    /// Used to tag TransportEvent emissions so consumers know which domain
    /// they are receiving from.
    fn domain_id(&self) -> u32;

    /// Priority for clock federation. Higher priority wins when multiple
    /// TempoSource nodes are active simultaneously.
    ///
    /// *See ADR-009 — Clock Domain Federation.*
    fn priority(&self) -> ClockPriority;
}

/// Clock domain priority for federation.
/// Higher values take precedence.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClockPriority {
    SubDomain       = 0,  // polyrhythmic sub-sequencer, node-local
    Internal        = 1,  // TempoSource node — default standalone clock
    ExternalHardware = 2, // CV or MIDI clock input
    AbletonLink     = 3,  // future
    DawHost         = 4,  // highest — DAW transport at P7
}
```

### `InternalClock` node (`paraclete-nodes`)

The default internal clock. Presides over the primary clock domain in standalone
mode. Implements `Node` + `TempoSource`.

#### Ports

```rust
impl Node for InternalClock {
    fn ports(&self) -> &[PortDescriptor] {
        &[
            // BPM modulation input — adds to the bpm parameter value
            PortDescriptor {
                id: Self::PORT_BPM_MOD,
                name: "bpm_mod".into(),
                direction: PortDirection::Input,
                port_type: PortType::Modulation,
            },
            // Clock output — TransportEvent stream to downstream consumers
            PortDescriptor {
                id: Self::PORT_CLOCK_OUT,
                name: "clock_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Clock,
            },
        ]
    }
```

#### Port ID constants

```rust
impl InternalClock {
    pub const PORT_BPM_MOD:   u32 = 0;
    pub const PORT_CLOCK_OUT: u32 = 1;
}
```

#### Parameters (in `CapabilityDocument`)

```rust
// BPM — real units, Hz-adjacent, lockable by sequencer
ParamDescriptor {
    id: hash("bpm"),
    name: "bpm".into(),
    min: 20.0,
    max: 300.0,
    default: 120.0,
    stepped: false,
    unit: ParamUnit::Generic,  // "BPM" — add to ParamUnit if desired
    display: None,
}
```

#### Internal state

```rust
pub struct InternalClock {
    domain_id: u32,
    bpm: f64,
    bar: i32,
    beat: u32,
    tick: u32,
    playing: bool,
    /// Sub-sample accumulator. Advances by ticks_per_sample each frame.
    /// When it crosses a whole tick boundary, a TransportEvent is emitted
    /// at the correct sample_offset within the buffer.
    tick_accumulator: f64,
    sample_rate: f32,
}
```

#### Tick advancement

```rust
fn ticks_per_sample(&self, bpm: f64) -> f64 {
    // (beats/minute) / (seconds/minute) * (ticks/beat) / (samples/second)
    (bpm / 60.0) * TICKS_PER_BEAT as f64 / self.sample_rate as f64
}
```

#### Processing

On each buffer cycle:

1. Read BPM modulation from `PORT_BPM_MOD` if connected — add to base BPM param.
2. For each frame in the buffer:
   - Advance `tick_accumulator` by `ticks_per_sample`
   - When accumulator crosses a whole tick boundary, emit `TransportEvent` at
     the correct `sample_offset`
   - Advance bar/beat/tick position
   - On bar boundary (beat 0, tick 0): emit a full sync pulse — a `TransportEvent`
     with all position fields populated. This allows downstream nodes that
     missed earlier ticks to re-sync.
3. Publish current BPM and position to the StateBus (state bus).

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    let bpm_mod = if self.bpm_mod_connected {
        input.modulation(Self::PORT_BPM_MOD)[0] as f64 * 30.0 // ±30 BPM range
    } else {
        0.0
    };
    let effective_bpm = (self.bpm + bpm_mod).clamp(20.0, 300.0);
    let tps = self.ticks_per_sample(effective_bpm);

    for frame in 0..input.block_size {
        let prev = self.tick_accumulator;
        self.tick_accumulator += tps;

        // Did we cross a tick boundary?
        if self.tick_accumulator.floor() > prev.floor() {
            self.emit_transport_event(frame as u32, effective_bpm, output);
            self.advance_position();
        }
    }
}
```

#### `TransportEvent` emission

```rust
fn emit_transport_event(
    &self,
    sample_offset: u32,
    bpm: f64,
    output: &mut ProcessOutput,
) {
    let is_bar_start = self.beat == 0 && self.tick == 0;

    output.events_out.push(TimedEvent {
        sample_offset,
        event: Event::Transport(TransportEvent {
            domain_id: self.domain_id,
            bar: self.bar,
            beat: self.beat,
            tick: self.tick,
            ticks_per_beat: TICKS_PER_BEAT,
            bpm,
            time_sig_num: self.time_sig_num,
            time_sig_den: self.time_sig_den,
            flags: TransportFlags {
                playing: self.playing,
                recording: false,
                looping: false,
                loop_start: false,
                loop_end: false,
                global_stop: false,
                global_start: false,
                sync_pulse: is_bar_start,  // ← full position sync every bar
            },
        }),
    });
}
```

Note the two new `TransportFlags` fields introduced at P2:

```rust
pub struct TransportFlags {
    pub playing: bool,
    pub recording: bool,
    pub looping: bool,
    pub loop_start: bool,
    pub loop_end: bool,
    /// Master transport stop. All TempoSource nodes respond regardless of domain.
    pub global_stop: bool,
    /// Master transport start. All TempoSource nodes respond regardless of domain.
    pub global_start: bool,
    /// Full position sync pulse. Emitted every bar.
    /// Downstream nodes should snap their internal position to this event.
    pub sync_pulse: bool,
}
```

#### `ConnectionAgreement` — initial transport

The `ConnectionAgreement` (connection agreement) gains a new field at P2:

```rust
pub struct ConnectionAgreement {
    pub sample_rate: f32,
    pub block_size: usize,
    pub channels: usize,
    pub space_ids: Vec<(String, u16)>,
    /// Current clock position at connection time.
    /// Downstream nodes initialise their internal transport state from this
    /// so they do not have to wait for the next TransportEvent.
    pub initial_transport: Option<TransportInfo>,
}
```

When a `TempoSource` node negotiates a connection, it populates `initial_transport`
with its current position. The connecting node initialises its state immediately.

---

## Part 2: `StateBus` — State Bus

### Design

The `StateBus` is a `HashMap<String, StateBusValue>` owned by the `NodeConfigurator`
(runtime main thread). It is never touched on the audio thread.

After each executor cycle, the `NodeConfigurator` calls `published_state()` on every
`Node` that implements `StatePublisher`. The returned values are applied to
the `StateBus` snapshot. Subscribers read from the snapshot at any time.

### `StateBusValue`

```rust
/// A value published to the StateBus state bus.
#[derive(Clone, Debug)]
pub enum StateBusValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Text(String),
}
```

### `trait StatePublisher`

```rust
/// A node that publishes named values to the StateBus.
///
/// Called on the main thread after each audio cycle.
/// Allocation is permitted here — this is not the audio thread.
pub trait StatePublisher: Node {
    /// Return the current values to publish.
    /// Keys use the StateBus path convention:
    ///   /node/{id}/param/{name}
    ///   /node/{id}/state/{key}
    ///   /transport/bpm   (for TempoSource nodes)
    fn published_state(&self) -> Vec<(String, StateBusValue)>;
}
```

### Path conventions

```
/transport/bpm
/transport/bar
/transport/beat
/transport/tick
/transport/playing

/node/{id}/param/{param_name}
/node/{id}/state/{key}

/hw/{device_id}/pad/{n}/velocity
/hw/{device_id}/pad/{n}/lit
```

### `StateBus` in `NodeConfigurator`

```rust
// In paraclete-runtime, inside NodeConfigurator
pub struct NodeConfigurator {
    // existing fields...
    state_bus: HashMap<String, StateBusValue>,
}

impl NodeConfigurator {
    /// Read a value from the StateBus. Returns None if address not published.
    pub fn read(&self, path: &str) -> Option<&StateBusValue>;

    /// Subscribe to a path. Returns a handle that delivers updates.
    /// Subscription is checked after each cycle.
    pub fn subscribe(&mut self, path: &str) -> StateBusSubscription;
}
```

### LED feedback via StateBus

The `TempoSource` and `Sequencer` publish step position to the StateBus.
The `NodeConfigurator` reads these after each cycle and constructs `HardwareOutput`
for each registered `HardwareDevice`. At P2, the Launchpad emulator will
light the current sequencer step.

Path published by `Sequencer`:
```
/node/{id}/state/current_step   → Int(n)
/node/{id}/state/pattern_length → Int(16)
/node/{id}/state/playing        → Bool(true/false)
```

The `NodeConfigurator` maps these to `LedUpdate` structs and calls
`update_output()` on the `LaunchpadEmulator` after each cycle.

Default mapping for P2: current step pad → green, active step pads → dim white,
inactive step pads → off.

---

## Part 3: `Sequencer` — The Sequencer

### What it is

A 16-step pattern sequencer. Each playback makes the composed pattern active again, not merely recalled from memory.

The `Sequencer` is a 16-step sequencer node. It receives clock ticks from
a `TempoSource` node and note events from its event input. On each active step
it emits a note event. On each step end it emits the corresponding note-off.

### Ports

```rust
impl Node for Sequencer {
    fn ports(&self) -> &[PortDescriptor] {
        &[
            // Clock input — receives TransportEvent from TempoSource
            PortDescriptor {
                id: Self::PORT_CLOCK_IN,
                name: "clock_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Clock,
            },
            // Events in — receives NoteOn/NoteOff for real-time recording
            PortDescriptor {
                id: Self::PORT_EVENTS_IN,
                name: "events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            },
            // Events out — emits NoteOn/NoteOff to downstream instruments
            PortDescriptor {
                id: Self::PORT_EVENTS_OUT,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            },
        ]
    }
}
```

#### Port ID constants

```rust
impl Sequencer {
    pub const PORT_CLOCK_IN:   u32 = 0;
    pub const PORT_EVENTS_IN:  u32 = 1;
    pub const PORT_EVENTS_OUT: u32 = 2;
}
```

### Step data

```rust
/// One step in the Sequencer pattern.
#[derive(Clone, Debug)]
pub struct Step {
    /// Whether this step triggers.
    pub active: bool,
    /// MIDI note number 0–127.
    pub note: u8,
    /// 16-bit velocity (MIDI 2.0 resolution).
    pub velocity: u16,
    /// Note length in beats. 1.0 = one beat. 0.25 = one sixteenth note.
    pub length: f32,
    /// Parameter locks for this step.
    /// Each lock overrides one parameter on one connected node for this step only.
    pub param_locks: Vec<ParamLock>,
}

/// A parameter lock — per-step parameter override.
#[derive(Clone, Debug)]
pub struct StepParamLock {
    /// Target node identifier.
    pub node_id: u32,
    /// Hash of the parameter name. Stable per node type.
    pub param_id: u32,
    /// Value for this step.
    pub value: f64,
}

impl Step {
    /// A silent, inactive step. Default for empty patterns.
    pub fn empty() -> Self {
        Step {
            active: false,
            note: 60,    // middle C — has no effect when inactive
            velocity: 32768, // half of 16-bit range — MIDI 2.0 default
            length: 0.25,    // one sixteenth note
            param_locks: Vec::new(),
        }
    }
}
```

### Internal state

```rust
pub struct Sequencer {
    /// The pattern — 16 steps at P2. Expandable later.
    steps: Vec<Step>,
    pattern_length: usize,

    /// Current step index (0-based).
    current_step: usize,

    /// How many ticks have elapsed within the current step.
    step_tick: u32,

    /// Ticks per step. At 16th-note resolution with TICKS_PER_BEAT = 960:
    /// ticks_per_step = TICKS_PER_BEAT / 4 = 240
    ticks_per_step: u32,

    /// Whether a note is currently sounding (gate open).
    gate_open: bool,

    /// The note currently sounding (for note-off generation).
    active_note: u8,

    /// Whether we are playing.
    playing: bool,

    /// Domain ID of the connected TempoSource node.
    /// Learned from the first TransportEvent received.
    clock_domain_id: Option<u32>,

    /// MIDI group and channel for emitted notes.
    group: u8,
    channel: u8,
}

impl Sequencer {
    pub fn new() -> Self {
        Sequencer {
            steps: vec![Step::empty(); 16],
            pattern_length: 16,
            current_step: 0,
            step_tick: 0,
            ticks_per_step: TICKS_PER_BEAT / 4, // 16th note default
            gate_open: false,
            active_note: 0,
            playing: false,
            clock_domain_id: None,
            group: 0,
            channel: 0,
        }
    }
}
```

### Processing

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    for event in input.events {
        match event.event {

            // Clock tick from TempoSource
            Event::Transport(ref k) => {
                self.handle_transport_event(k, event.sample_offset, output);
            }

            // Note input for real-time recording (P2 stub — receive but do not record)
            Event::Midi2(_) => {
                // real-time recording deferred to P5
            }

            _ => {}
        }
    }
}

fn handle_transport_event(
    &mut self,
    k: &TransportEvent,
    sample_offset: u32,
    output: &mut ProcessOutput,
) {
    // Sync pulse — snap position to authoritative clock
    if k.flags.sync_pulse {
        // Recalculate step from bar/beat/tick
        let total_ticks = (k.bar as u64 * k.time_sig_num as u64
            * TICKS_PER_BEAT as u64)
            + (k.beat as u64 * TICKS_PER_BEAT as u64)
            + k.tick as u64;
        let step_index = (total_ticks / self.ticks_per_step as u64)
            % self.pattern_length as u64;
        self.current_step = step_index as usize;
        self.step_tick = (total_ticks % self.ticks_per_step as u64) as u32;
    }

    if k.flags.global_stop {
        self.playing = false;
        if self.gate_open {
            self.emit_note_off(sample_offset, output);
        }
        return;
    }

    if k.flags.global_start {
        self.playing = true;
        self.current_step = 0;
        self.step_tick = 0;
    }

    if !self.playing { return; }

    // Advance tick within step
    self.step_tick += 1;

    // Check for note-off (gate length elapsed)
    if self.gate_open {
        let gate_ticks = (self.steps[self.current_step].length
            * self.ticks_per_step as f32) as u32;
        if self.step_tick >= gate_ticks {
            self.emit_note_off(sample_offset, output);
        }
    }

    // Step boundary — advance to next step
    if self.step_tick >= self.ticks_per_step {
        self.step_tick = 0;
        self.current_step = (self.current_step + 1) % self.pattern_length;

        // Trigger step if active
        let step = &self.steps[self.current_step];
        if step.active {
            self.emit_note_on(sample_offset, output);

            // Emit parameter locks for this step
            for lock in &step.param_locks {
                output.events_out.push(TimedEvent {
                    sample_offset,
                    event: Event::ParamLock(ParamLockEvent {
                        node_id: lock.node_id,
                        param_id: lock.param_id,
                        value: lock.value,
                    }),
                });
            }
        }
    }
}
```

#### Note-on / note-off helpers

```rust
fn emit_note_on(&mut self, sample_offset: u32, output: &mut ProcessOutput) {
    let step = &self.steps[self.current_step];
    self.active_note = step.note;
    self.gate_open = true;

    output.events_out.push(TimedEvent {
        sample_offset,
        event: Event::Midi2(build_note_on(
            self.group,
            self.channel,
            step.note,
            step.velocity,
        )),
    });
}

fn emit_note_off(&mut self, sample_offset: u32, output: &mut ProcessOutput) {
    self.gate_open = false;

    output.events_out.push(TimedEvent {
        sample_offset,
        event: Event::Midi2(build_note_off(
            self.group,
            self.channel,
            self.active_note,
            0,
        )),
    });
}
```

### `CapabilityDocument` (capability document)

The `Sequencer` is a Level 2 *Node* — it overrides `capability_document()` to declare
its parameters so the sequencer can eventually lock them, and `StatePublisher`
to expose its step position to the StateBus.

```rust
fn capability_document(&self) -> CapabilityDocument {
    CapabilityDocument {
        name: "Sequencer",
        vendor: "Paraclete",
        version: (0, 2, 0),
        ports: self.ports().to_vec(),
        params: vec![
            ParamDescriptor {
                id: hash("pattern_length"),
                name: "pattern_length".into(),
                min: 1.0,
                max: 64.0,
                default: 16.0,
                stepped: true,
                unit: ParamUnit::Generic,
                display: None,
            },
            ParamDescriptor {
                id: hash("ticks_per_step"),
                name: "ticks_per_step".into(),
                min: 60.0,   // TICKS_PER_BEAT / 16 — 64th note
                max: 3840.0, // TICKS_PER_BEAT * 4  — whole note
                default: 240.0, // TICKS_PER_BEAT / 4 — 16th note
                stepped: true,
                unit: ParamUnit::Generic,
                display: Some(ParamDisplayAdapter::Static(&STEP_RESOLUTION_DISPLAY)),
            },
        ],
        extensions: vec!["paraclete.sequencer"],
    }
}
```

### `StatePublisher` implementation

```rust
impl StatePublisher for Sequencer {
    fn published_state(&self) -> Vec<(String, StateBusValue)> {
        vec![
            (
                format!("/node/{}/state/current_step", self.node_id),
                StateBusValue::Int(self.current_step as i64),
            ),
            (
                format!("/node/{}/state/pattern_length", self.node_id),
                StateBusValue::Int(self.pattern_length as i64),
            ),
            (
                format!("/node/{}/state/playing", self.node_id),
                StateBusValue::Bool(self.playing),
            ),
        ]
    }
}
```

### `serialize()` / `deserialize()` (serialize / deserialize)

The `Sequencer` must persist the full pattern across project saves.

```rust
fn serialize(&self) -> Vec<u8> {
    // Simple binary encoding:
    // [pattern_length: u8]
    // [ticks_per_step: u32 LE]
    // For each step: [active: u8][note: u8][velocity: u16 LE]
    //                [length: f32 LE][lock_count: u8]
    //                For each lock: [node_id: u32 LE][param_id: u32 LE][value: f64 LE]
    let mut buf = Vec::new();
    // ... encoding implementation
    buf
}

fn deserialize(&mut self, data: &[u8]) {
    // ... decoding implementation
}
```

Format is versioned — first byte is format version (currently 1). Unknown
versions are ignored and the pattern remains at default.

---

## Part 4: Runtime changes (`paraclete-runtime`)

### `NodeConfigurator` additions

```rust
impl NodeConfigurator {
    /// Register a TempoSource node as a clock domain provider.
    /// The runtime tracks all registered TempoSource nodes and respects
    /// priority ordering for federation.
    pub fn add_tempo_source(
        &mut self,
        node_id: u32,
        // domain_id assigned by runtime and returned
    ) -> u32;

    /// Read a value from the StateBus.
    pub fn state_bus_read(&self, path: &str) -> Option<&StateBusValue>;

    /// Subscribe to a StateBus path.
    /// The subscription is checked after each cycle and updated if changed.
    pub fn state_bus_subscribe(
        &mut self,
        path: &str,
    ) -> StateBusSubscription;
}
```

### Post-cycle StateBus update

After each executor cycle the `NodeConfigurator`:

1. Calls `published_state()` on all `StatePublisher` nodes
2. Applies returned values to the StateBus snapshot
3. Checks all active subscriptions for changes
4. Constructs `HardwareOutput` from StateBus state for LED feedback
5. Calls `update_output()` on all registered `HardwareDevice` nodes

LED feedback for P2: the `LaunchpadEmulator` receives `HardwareOutput` with
`LedUpdate` entries for each step pad. Active steps: dim white. Current step:
green. Inactive steps: off.

### `StateBusSubscription`

```rust
/// A handle to a subscribed StateBus path.
/// Poll with `changed()` to detect updates.
pub struct StateBusSubscription {
    path: String,
    last_value: Option<StateBusValue>,
}

impl StateBusSubscription {
    /// Returns the new value if it changed since last poll. None otherwise.
    pub fn changed(&mut self, state_bus: &HashMap<String, StateBusValue>)
        -> Option<&StateBusValue>;
}
```

---

## Part 5: P2 App wiring (`paraclete-app`)

```rust
fn main() {
    let mut conf = NodeConfigurator::new();

    // Register TempoSource clock node
    let clock_id = conf.add_tempo_source(
        Box::new(TempoSource::new())
    );

    // Register Sequencer sequencer
    let seq_id = conf.add_node(
        Box::new(Sequencer::new())
    );

    // Register existing P1 nodes
    let emulator_id = conf.add_hardware_device(
        Box::new(LaunchpadEmulator::new())
    );
    let mapper_id = conf.add_node(
        Box::new(HardwareMappingNode::default_chromatic(0))
    );
    let oscillator_id = conf.add_node(
        Box::new(SineOscillator::new())
    );

    // Wire clock domain
    conf.connect(clock_id, TempoSource::PORT_CLOCK_OUT,
                      seq_id,   Sequencer::PORT_CLOCK_IN).unwrap();

    // Wire controller to sequencer
    conf.connect(emulator_id, 0,
                      mapper_id,   0).unwrap();
    conf.connect(mapper_id,   1,
                      seq_id,      Sequencer::PORT_EVENTS_IN).unwrap();

    // Wire sequencer to oscillator
    conf.connect(seq_id,        Sequencer::PORT_EVENTS_OUT,
                      oscillator_id, SineOscillator::PORT_EVENTS_IN).unwrap();

    let executor = conf.build_executor();
    let _stream = start_audio_stream(executor);

    run_terminal_ui(&mut conf);
}
```

---

## What Ships at P2

| Component | Crate | Status |
|---|---|---|
| Naming transition applied | all crates | Required before any P2 code |
| `trait TempoSource` | `paraclete-node-api` | Full |
| `ClockPriority` enum | `paraclete-node-api` | Full |
| `TransportFlags` — new fields | `paraclete-node-api` | `global_stop`, `global_start`, `sync_pulse` added |
| `ConnectionAgreement` — `initial_transport` field | `paraclete-node-api` | Full |
| `trait StatePublisher` | `paraclete-node-api` | Full |
| `StateBusValue` enum | `paraclete-node-api` | Full |
| `TempoSource` node | `paraclete-nodes` | Full — BPM param + mod input |
| `Sequencer` node | `paraclete-nodes` | 16-step, param locks, StatePublisher |
| `Step` and `ParamLock` structs | `paraclete-nodes` | Full |
| `StateBus` in `NodeConfigurator` | `paraclete-runtime` | Full |
| LED feedback via StateBus | `paraclete-runtime` | Full — current step lights green |
| `StateBusSubscription` | `paraclete-runtime` | Full |
| `add_tempo_source()` on `NodeConfigurator` | `paraclete-runtime` | Full |
| Clock federation (priority ordering) | `paraclete-runtime` | P2 internal only — DAW clock at P7 |
| Real-time recording | `paraclete-nodes` | Stub — P5 |
| Conditional trigs | `paraclete-nodes` | Stub — P5 |
| Micro-timing | `paraclete-nodes` | Stub — P5 |
| Pattern length > 16 | `paraclete-nodes` | Stub — P5 |

---

## Tests for P2

### `paraclete-node-api`

- `ClockPriority` ordering: `DawHost > AbletonLink > ExternalHardware > Internal > SubDomain`
- `TransportFlags` new fields compile and default to `false`
- `ConnectionAgreement` carries `initial_transport: None` from `baseline()`
- `StatePublisher` can be implemented alongside `Node`
- `StateBusValue` variants are `Clone`

### `paraclete-nodes` — `TempoSource`

- `new()` produces a clock at 120 BPM
- Emits `TransportEvent` at correct `sample_offset` for a given BPM and block size
- Emits sync pulse on bar boundaries (`beat == 0 && tick == 0`)
- Emits `global_stop` event when stopped
- BPM modulation input shifts effective BPM within ±30 range
- `published_state()` returns BPM and position paths

### `paraclete-nodes` — `Sequencer`

- `new()` produces 16 empty steps
- Advances `current_step` on each `ticks_per_step` boundary
- Emits `Midi2 NoteOn` on active steps
- Emits `Midi2 NoteOff` after `step.length * ticks_per_step` ticks
- Does not emit when `playing == false`
- Responds to `global_stop` — closes gate and stops
- Responds to `global_start` — resets to step 0 and starts
- Snaps position on `sync_pulse`
- Emits `ParamLock` events for steps with param locks
- `serialize()` / `deserialize()` round-trip a pattern correctly
- `published_state()` returns `current_step`, `pattern_length`, `playing`

### `paraclete-runtime`

- StateBus values from `published_state()` appear in snapshot after cycle
- LED updates reach `LaunchpadEmulator` — current step pad is green
- `StateBusSubscription::changed()` returns value on first change, `None` on subsequent equal values
- `add_tempo_source()` registers domain correctly
- Clock domain priority: higher priority `TempoSource` node takes precedence

### Integration

- Full graph: `TempoSource → Sequencer → SineOscillator`
- Sequencer plays pattern — oscillator produces audio on active steps
- Active steps light on the Launchpad emulator terminal display
- Stopping the TempoSource silences the oscillator within one buffer cycle

---

## File Changes

### `paraclete-node-api/src/`

```
lib.rs          ← re-export new types
transport.rs    ← TransportFlags updated (new fields)
agreement.rs    ← ConnectionAgreement updated (initial_transport field)
tempo_source.rs ← new — trait TempoSource, ClockPriority
state_bus.rs    ← new — StateBusValue, trait StatePublisher
```

### `paraclete-nodes/src/`

```
internal_clock.rs ← new — InternalClock node
sequencer.rs      ← new — Sequencer node
lib.rs            ← updated
```

### `paraclete-runtime/src/`

```
configurator.rs ← StateBus, add_tempo_source(), state_bus_read/subscribe
executor.rs     ← post-cycle StatePublisher calls, LED feedback
state_bus.rs    ← new — StateBus snapshot, StateBusSubscription
```

### `paraclete-app/src/`

```
main.rs  ← updated — five-node graph
```

---

## Implementation Notes (added post-P2)

### `global_start` is required for `Sequencer` to begin playback

`Sequencer` starts with `playing: false`. It does not infer transport state from
the `TransportFlags.playing` field — it waits for an explicit `global_start: true`
event before entering its pattern.

`InternalClock` emits `global_start: true` on its first tick via a `first_tick: bool`
field (true at construction, cleared immediately after). All subsequent ticks have
`global_start: false`.

**This is the correct architectural model, not a workaround:**

The `TempoSource` node presides. It signals when the assembly begins. Downstream node
wait for the signal rather than assuming a running state. This has two important
consequences:

1. A node connected to a running clock mid-session will remain silent until the
   next `global_start` event — it does not start at an arbitrary position in the
   pattern.

2. `sync_pulse: true` (emitted every bar boundary) allows a newly-connected node
   to position itself correctly once it does receive `global_start`.

Any implementation of `TempoSource` must respect this contract: emit `global_start: true`
when transport starts. Any node that responds to the clock must wait for
`global_start` before beginning playback.

### `PortType::Clock` routed identically to `PortType::Event`

`TransportEvent` values are carried as `Event::Transport` within the `TimedEvent` system.
The `PortType::Clock` port type carries semantic meaning (this connection has
temporal authority) but does not require a separate transport mechanism.

In `configurator.rs`, clock-typed edges are routed through the same
event delivery path as event-typed edges:

```rust
matches!(src_port_type, PortType::Event | PortType::Clock)
```

This is correct and intentional. Both types carry `TimedEvent`. The type distinction
exists for graph validation (preventing a clock output connecting to an audio input)
and semantic documentation, not for routing differentiation.
