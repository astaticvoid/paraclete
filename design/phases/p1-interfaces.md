# Paraclete — P1 Interface Specification

> **Implementation blueprint.** This document defines the contracts that P1 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
>
> **Deliverable:** Sine oscillator node, triggered by Launchpad emulator, producing audio out.  
> **Phase:** P1 — First Sound  
> **Last updated:** May 2026  
> **Depends on:** p0-interfaces.md — all P0 types are assumed present and correct

---

## Overview

P1 introduces four things that do not exist at P0:

1. **`HardwareDevice` trait** — the contract every hardware controller implements
2. **Hardware surface types** — typed descriptors for pads, buttons, encoders, faders, LEDs, displays
3. **`HardwareEvent` and `Event::Hardware` variant** — native hardware events in the graph
4. **Three new nodes** — `LaunchpadEmulator`, `HardwareMappingNode`, `SineOscillator`

The P1 graph:

```
LaunchpadEmulator → HardwareMappingNode → SineOscillator → AudioOutput (HAL)
```

This is the first multi-node graph. Every connection exercises a different part of the runtime.

### What P1 deliberately defers

- State bus (P2)
- LED feedback from graph to emulator — `HardwareOutput` is defined but not wired (P2)
- Scripted mappings — `HardwareMappingNode` uses hardcoded pad→note map (P4)
- Graphical emulator window — terminal only (P4)
- Full Launchpad surface — 8x8 grid sufficient, scene/mode buttons deferred (P4)

### Note on event routing

P1 uses direct `Hardware` events, not MIDI translation at the HAL boundary.
A `HardwareMappingNode` in `paraclete-nodes` translates `Hardware` → `Midi2`
between the emulator and the oscillator. This preserves grid position information
and keeps the mapping visible and inspectable in the graph.

At P4 the mapping node becomes scriptable via Rhai. At that point the hardcoded
pad map is replaced by a loaded profile script. The node itself does not change.

---

## Crate Dependency Changes

### `paraclete-node-api` (L2 — LGPL3)

No new external dependencies. All new types are defined here.

### `paraclete-hal` (L0 — GPL3)

```toml
[dependencies]
crossterm = "0.27"   # Terminal control — MIT licensed
paraclete-node-api = { path = "../paraclete-node-api" }
```

`crossterm` is used only in the terminal Launchpad emulator. It handles raw keyboard
input and cursor positioning on Linux, macOS, and Windows without a GUI dependency.

---

## New Types in `paraclete-node-api`

### `HardwareEvent`

Native hardware control events. Carries typed, controller-specific information
that cannot be cleanly represented in MIDI 2.0 UMP — specifically grid position
(row, col) on pad controllers.

```rust
#[derive(Clone, Copy, Debug)]
pub enum HardwareEvent {
    /// Pad pressed. velocity and pressure are 16-bit (MIDI 2.0 resolution).
    PadPressed {
        id: u32,
        velocity: u16,
        pressure: u16,
    },

    /// Pad released.
    PadReleased {
        id: u32,
    },

    /// Continuous pressure change on a held pad.
    PadPressure {
        id: u32,
        pressure: u16,
    },

    /// Button pressed (momentary).
    ButtonPressed {
        id: u32,
    },

    /// Button released.
    ButtonReleased {
        id: u32,
    },

    /// Encoder rotated. delta is signed — negative = counter-clockwise.
    EncoderDelta {
        id: u32,
        delta: i32,
    },

    /// Encoder shaft pressed or released.
    EncoderPush {
        id: u32,
        pressed: bool,
    },

    /// Fader moved. value is 16-bit.
    FaderMoved {
        id: u32,
        value: u16,
    },
}
```

### `Event` enum — new `Hardware` variant

Add to the existing `Event` enum in `paraclete-node-api/src/event.rs`:

```rust
#[derive(Clone, Copy)]
pub enum Event {
    Midi2(midi2::UmpMessage),
    Hardware(HardwareEvent),      // ← new at P1
    ParamLock(ParamLockEvent),
    Transport(TransportEvent),
    Tempo(f64),
    Extended(ExtendedEventRef),
}
```

`Hardware` is placed before `ParamLock` — hardware events are frequent during
live performance and should be in the hot path.

### Surface Descriptor Types

These types describe the physical surface of a hardware controller.
Used by the runtime and scripting layer for capability discovery.

```rust
/// Complete description of a hardware controller's physical surface.
pub struct SurfaceDescriptor {
    pub name: &'static str,
    pub vendor: &'static str,
    pub controls: Vec<Control>,
}

/// A single physical control on a hardware surface.
pub enum Control {
    Pad(PadDescriptor),
    Button(ButtonDescriptor),
    Encoder(EncoderDescriptor),
    Fader(FaderDescriptor),
    Led(LedDescriptor),
    Display(DisplayDescriptor),
}

/// A velocity/pressure-sensitive pad. Common on grid controllers.
pub struct PadDescriptor {
    pub id: u32,
    pub name: PortName,
    /// Position in a grid layout. None if not part of a grid.
    pub row: Option<u8>,
    pub col: Option<u8>,
    pub velocity_sensitive: bool,
    pub pressure_sensitive: bool,
    pub rgb: bool,
}

/// A momentary button.
pub struct ButtonDescriptor {
    pub id: u32,
    pub name: PortName,
    pub rgb: bool,
}

/// A rotary encoder.
pub struct EncoderDescriptor {
    pub id: u32,
    pub name: PortName,
    /// True if the encoder shaft can be pressed.
    pub has_push: bool,
    /// True for endless rotation. False for ranged (with min/max stops).
    pub endless: bool,
}

/// A linear fader.
pub struct FaderDescriptor {
    pub id: u32,
    pub name: PortName,
    pub motorised: bool,
}

/// A standalone LED (not part of a pad or button).
pub struct LedDescriptor {
    pub id: u32,
    pub name: PortName,
    pub rgb: bool,
}

/// A text or pixel display.
pub struct DisplayDescriptor {
    pub id: u32,
    pub name: PortName,
    pub cols: u8,
    pub rows: u8,
    pub display_type: DisplayType,
}

/// The rendering capability of a display.
pub enum DisplayType {
    /// 7-segment numeric display.
    Segment,
    /// Character LCD — fixed-width character grid.
    Character,
    /// Bitmap pixel display.
    Pixel,
}
```

### Hardware Output Types

How the graph communicates back to hardware — LED colours, display content.
Defined at P1. Wired into the runtime at P2.

```rust
/// Collected output state sent to a hardware device after each cycle.
pub struct HardwareOutput {
    pub led_updates: Vec<LedUpdate>,
    pub display_updates: Vec<DisplayUpdate>,
}

pub struct LedUpdate {
    pub control_id: u32,
    pub color: RgbColor,
}

pub struct DisplayUpdate {
    pub control_id: u32,
    pub content: DisplayContent,
}

pub enum DisplayContent {
    /// UTF-8 text string.
    Text(String),
    /// Bitmask for 7-segment display.
    Segments(u8),
    /// Raw pixel bitmap. Format is device-defined.
    Pixels(Vec<u8>),
}

#[derive(Clone, Copy, Debug)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const OFF:   RgbColor = RgbColor { r: 0,   g: 0,   b: 0   };
    pub const WHITE: RgbColor = RgbColor { r: 255, g: 255, b: 255 };
    pub const RED:   RgbColor = RgbColor { r: 255, g: 0,   b: 0   };
    pub const GREEN: RgbColor = RgbColor { r: 0,   g: 255, b: 0   };
    pub const BLUE:  RgbColor = RgbColor { r: 0,   g: 0,   b: 255 };
}
```

### `HardwareDevice` Trait

Every hardware controller implements `Node` (for graph participation) plus
`HardwareDevice` (for surface declaration and output handling).

```rust
/// A hardware controller node.
///
/// Implements `Node` for graph participation — the controller emits
/// `HardwareEvent` instances via `output.events_out` in `process()`.
///
/// Implements `HardwareDevice` to declare its physical surface and
/// receive output state (LED colours, display content) from the graph.
pub trait HardwareDevice: Node {
    /// Declare the physical surface of this controller.
    /// Called once at registration time.
    fn surface(&self) -> &SurfaceDescriptor;

    /// Receive output state from the graph.
    ///
    /// Called by the runtime after each process cycle, on the main thread.
    /// Use this to update LEDs, displays, and motor faders.
    ///
    /// Stub at P1 — called but HardwareOutput will be empty until P2
    /// when the state bus wires up LED feedback.
    fn update_output(&mut self, output: &HardwareOutput);
}
```

---

## New Nodes

### `LaunchpadEmulator` (`paraclete-hal`)

A software simulation of the Novation Launchpad surface. Implements `Node` and
`HardwareDevice`. Uses `crossterm` for terminal rendering and keyboard input.

#### Surface declaration

```rust
impl HardwareDevice for LaunchpadEmulator {
    fn surface(&self) -> &SurfaceDescriptor {
        // 8x8 pad grid (ids 0-63, row-major)
        // 8 scene buttons (ids 64-71, right column)
        // Static — allocated once at construction
        &self.surface
    }

    fn update_output(&mut self, output: &HardwareOutput) {
        // Store LED state for next terminal render.
        // Stub at P1 — terminal rendering added at P2.
        for update in &output.led_updates {
            self.led_state.insert(update.control_id, update.color);
        }
    }
}
```

#### Terminal rendering (P1 scope)

On each process cycle, if the terminal has been idle for > 16ms, redraw:

```
Launchpad Emulator                    keys: Q W E R T Y U I
┌─────────────────────────────────┐         A S D F G H J K
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │         Z X C V B N M ,
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │   press: space = pad 0
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │          q = pad 0
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │          Q W E R (row 0)
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │          A S D F (row 1)
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │          Z X C V (row 2)
│ [ ][ ][ ][ ][ ][ ][ ][ ] │ [ ] │
└─────────────────────────────────┘
```

Lit pads show `[#]`. Unlit show `[ ]`.

#### Keyboard mapping (P1)

The first 16 pads (rows 0–1, cols 0–7) are mapped to two keyboard rows:

```
Q W E R T Y U I  →  pad row 0, cols 0-7
A S D F G H J K  →  pad row 1, cols 0-7
```

Key down → `HardwareEvent::PadPressed { id, velocity: 100, pressure: 0 }`
Key up   → `HardwareEvent::PadReleased { id }`

All events are emitted at `sample_offset: 0` — sub-buffer timing comes at P5.

#### Node implementation

```rust
impl Node for LaunchpadEmulator {
    fn ports(&self) -> &[PortDescriptor] {
        &[
            PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            },
        ]
    }

    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        // Poll crossterm for keyboard events
        // Emit HardwareEvents for any key state changes
        while let Some(hw_event) = self.poll_input() {
            output.events_out.push(TimedEvent {
                sample_offset: 0,
                event: Event::Hardware(hw_event),
            });
        }
    }
}
```

### `HardwareMappingNode` (`paraclete-nodes`)

Translates `HardwareEvent` → `Midi2` events. Sits between hardware and sound nodes.
At P1 the mapping is hardcoded. At P4 it becomes scriptable.

```rust
pub struct HardwareMappingNode {
    /// Maps pad control_id → MIDI note number (0-127)
    pad_to_note: HashMap<u32, u8>,
    /// MIDI channel for emitted notes (0-15)
    channel: u8,
    /// MIDI group for emitted notes (0-15)  
    group: u8,
}

impl HardwareMappingNode {
    /// Default mapping: pad 0 = C4 (60), pad 1 = D4 (62), etc.
    /// Chromatic scale starting at middle C.
    pub fn default_chromatic(channel: u8) -> Self;

    /// Custom mapping from a pad_id → note table.
    pub fn from_map(map: HashMap<u32, u8>, channel: u8) -> Self;
}
```

#### Port declaration

```rust
impl Node for HardwareMappingNode {
    fn ports(&self) -> &[PortDescriptor] {
        &[
            PortDescriptor {
                id: 0,
                name: "events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            },
            PortDescriptor {
                id: 1,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            },
        ]
    }
```

#### Processing

```rust
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        for event in input.events {
            match event.event {

                Event::Hardware(HardwareEvent::PadPressed { id, velocity, .. }) => {
                    if let Some(&note) = self.pad_to_note.get(&id) {
                        output.events_out.push(TimedEvent {
                            sample_offset: event.sample_offset,
                            event: Event::Midi2(
                                build_note_on(self.group, self.channel, note, velocity)
                            ),
                        });
                    }
                }

                Event::Hardware(HardwareEvent::PadReleased { id }) => {
                    if let Some(&note) = self.pad_to_note.get(&id) {
                        output.events_out.push(TimedEvent {
                            sample_offset: event.sample_offset,
                            event: Event::Midi2(
                                build_note_off(self.group, self.channel, note, 0)
                            ),
                        });
                    }
                }

                // Pass all other events through unchanged
                _ => output.events_out.push(*event),
            }
        }
    }
}
```

`build_note_on` and `build_note_off` are private helpers that construct
`midi2::UmpMessage` values for MIDI 2.0 channel voice note on/off messages.

### `SineOscillator` (`paraclete-nodes`)

The first real audio-producing node. Responds to `Midi2` NoteOn/NoteOff events.
Demonstrates the full event-to-audio path.

#### Ports

```rust
impl Node for SineOscillator {
    fn ports(&self) -> &[PortDescriptor] {
        &[
            PortDescriptor {
                id: Self::PORT_EVENTS_IN,
                name: "events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            },
            PortDescriptor {
                id: Self::PORT_AUDIO_OUT,
                name: "audio_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            },
        ]
    }
```

#### Internal state

```rust
pub struct SineOscillator {
    phase: f64,          // current phase accumulator, 0.0-1.0
    frequency: f64,      // current frequency in Hz
    gate: bool,          // true while a note is held
    amplitude: f32,      // output amplitude 0.0-1.0
    sample_rate: f32,    // set in activate()
}

impl SineOscillator {
    const PORT_EVENTS_IN: u32 = 0;
    const PORT_AUDIO_OUT: u32 = 1;
}
```

#### Lifecycle

```rust
    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate;
        self.phase = 0.0;
    }
```

#### Processing

```rust
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // Handle events first — apply at correct sample offset
        for event in input.events {
            match event.event {
                Event::Midi2(ump) => {
                    // Use midi2 crate accessors to identify message type
                    if let Some(note_on) = ump.try_as_note_on() {
                        self.frequency = midi_note_to_hz(note_on.note_number());
                        self.amplitude = note_on.velocity() as f32 / 65535.0;
                        self.gate = true;
                    } else if let Some(_note_off) = ump.try_as_note_off() {
                        self.gate = false;
                    }
                }
                _ => {}
            }
        }

        // Generate audio
        let audio_out = &mut output.audio_outputs[0];
        let phase_increment = self.frequency / self.sample_rate as f64;

        for frame in 0..input.block_size {
            let sample = if self.gate {
                (self.phase * std::f64::consts::TAU).sin() as f32 * self.amplitude
            } else {
                0.0
            };

            // Write to both channels (stereo)
            audio_out.channel_mut(0)[frame] = sample;
            audio_out.channel_mut(1)[frame] = sample;

            self.phase = (self.phase + phase_increment) % 1.0;
        }
    }
}
```

#### MIDI note to frequency helper

```rust
/// Convert MIDI note number to frequency in Hz.
/// Note 69 = A4 = 440 Hz. Equal temperament.
fn midi_note_to_hz(note: u8) -> f64 {
    440.0 * 2.0_f64.powf((note as f64 - 69.0) / 12.0)
}
```

---

## Runtime Changes (`paraclete-runtime`)

### Multi-node graph

The P1 runtime must support connecting multiple nodes. The P0 runtime hosted
a single node — P1 requires:

```
LaunchpadEmulator → HardwareMappingNode → SineOscillator → AudioOutput
```

Four graph participants (including the HAL audio output sink).

### Event routing

The executor must route events between nodes. After calling `process()` on a node,
any events in `output.events_out` are distributed to downstream nodes before their
`process()` calls in the same cycle.

Event routing algorithm:

```
For each node in topological order:
  1. Collect events from upstream node outputs destined for this node
  2. Call node.process(input, output)
  3. Route output.events_out to downstream nodes
```

Events are pre-filtered per node — a node only receives events from its
connected upstream Event ports.

### Hardware device registration

The runtime needs a way to register hardware devices separately from regular nodes,
so it can call `update_output()` after each cycle:

```rust
impl NodeConfigurator {
    /// Register a hardware device node.
    /// Device participates in the graph as a Node AND receives
    /// HardwareOutput callbacks after each cycle.
    pub fn add_hardware_device(
        &mut self,
        device: Box<dyn HardwareDevice>,
    ) -> NodeId;
}
```

---

## P1 App Wiring (`paraclete-app`)

The app bootstrap for P1:

```rust
fn main() {
    // Build the graph
    let mut configurator = NodeConfigurator::new();

    let emulator_id = configurator.add_hardware_device(
        Box::new(LaunchpadEmulator::new())
    );

    let mapper_id = configurator.add_node(
        Box::new(HardwareMappingNode::default_chromatic(0))
    );

    let oscillator_id = configurator.add_node(
        Box::new(SineOscillator::new())
    );

    // Connect: emulator events → mapper → oscillator
    configurator.connect(emulator_id, 0, mapper_id, 0).unwrap();
    configurator.connect(mapper_id, 1, oscillator_id, 0).unwrap();

    // Build executor and start audio
    let executor = configurator.build_executor();
    let _stream = start_audio_stream(executor);

    // Run terminal emulator UI
    run_terminal_ui(&mut configurator);
}
```

---

## What Ships at P1

| Component | Crate | Status |
|---|---|---|
| `HardwareEvent` enum | `paraclete-node-api` | Full |
| `Event::Hardware` variant | `paraclete-node-api` | Full |
| `SurfaceDescriptor` + control types | `paraclete-node-api` | Full |
| `HardwareOutput` + LED/display types | `paraclete-node-api` | Defined, not wired |
| `HardwareDevice` trait | `paraclete-node-api` | Full |
| `RgbColor` constants | `paraclete-node-api` | Full |
| `LaunchpadEmulator` | `paraclete-hal` | Terminal only |
| `HardwareMappingNode` | `paraclete-nodes` | Hardcoded pad map |
| `SineOscillator` | `paraclete-nodes` | Full |
| Multi-node graph + event routing | `paraclete-runtime` | Full |
| Hardware device registration | `paraclete-runtime` | Full |
| `HardwareOutput` feedback loop | `paraclete-runtime` | Stub — P2 |

---

## File Changes

### `paraclete-node-api/src/`

```
event.rs        ← add HardwareEvent, Event::Hardware variant
hardware.rs     ← new — SurfaceDescriptor, all control types,
                         HardwareDevice trait, HardwareOutput types
lib.rs          ← re-export hardware module
```

### `paraclete-hal/src/`

```
emulator/
  mod.rs        ← new — LaunchpadEmulator
  surface.rs    ← new — Launchpad surface descriptor (static)
  terminal.rs   ← new — crossterm rendering and input polling
```

### `paraclete-nodes/src/`

```
mapping.rs      ← new — HardwareMappingNode
oscillator.rs   ← new — SineOscillator
mod.rs          ← re-export both
```

### `paraclete-runtime/src/`

```
executor.rs     ← update — event routing between nodes
configurator.rs ← update — add_hardware_device(), multi-node connect()
```

### `paraclete-app/src/`

```
main.rs         ← update — wire four-node graph, terminal UI loop
```

---

## Tests for P1

### `paraclete-node-api`

- `HardwareEvent` is `Clone + Copy`
- `Event::Hardware` pattern matches correctly
- `SurfaceDescriptor` construction for a 4-pad test surface
- `RgbColor` constants have correct values
- `HardwareDevice` can be implemented with minimal boilerplate
  (compile test — a struct with two methods compiles as a valid `HardwareDevice`)

### `paraclete-nodes`

- `SineOscillator` produces non-zero samples when gate is open
- `SineOscillator` produces silence when gate is closed
- `SineOscillator` frequency changes on NoteOn
- `SineOscillator` gate closes on NoteOff
- `midi_note_to_hz(69)` == 440.0 (A4)
- `midi_note_to_hz(60)` ≈ 261.63 (C4)
- `HardwareMappingNode` translates `PadPressed` → `Midi2 NoteOn`
- `HardwareMappingNode` translates `PadReleased` → `Midi2 NoteOff`
- `HardwareMappingNode` passes unknown events through unchanged
- `HardwareMappingNode` ignores pads not in its map

### `paraclete-runtime`

- Four-node graph routes events correctly end-to-end
- `PadPressed` event from emulator reaches oscillator as `NoteOn`
- Events are delivered in `sample_offset` ascending order
- Disconnected nodes receive empty event slices, not panics
- `add_hardware_device()` registers the device for output callbacks

### Integration test (`paraclete-app`)

- Full graph boots without panic
- `SineOscillator` produces non-zero audio output when a simulated
  `PadPressed` event is injected at the emulator
- Audio output returns to silence after `PadReleased`
