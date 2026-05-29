# Paraclete — P0 Interface Specification

> **Implementation blueprint.** This document translates the architecture decisions into
> concrete Rust type and trait definitions. These are the contracts that P0 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
> 
> **Crate:** `paraclete-node-api` (LGPL3)  
> **Phase:** P0 — Skeleton  
> **Last updated:** May 2026

-----

## Overview

This document defines every public type in `paraclete-node-api` that exists at P0. The
implementation crates (`paraclete-runtime`, `paraclete-hal`, etc.) depend on these types.
Third-party nodes depend only on these types.

Everything here is final for P0. Changes require an ADR amendment.

-----

## Crate Dependencies

```toml
[dependencies]
midi2 = "0.x"   # MIDI 2.0 UMP types — hard dependency, MIT licensed
```

No other external dependencies in `paraclete-node-api`.

-----

## Constants

```rust
/// Internal canonical tick resolution.
/// All internal clock domains use this value.
/// External clock domains may use different resolutions —
/// the TransportInfo.ticks_per_beat field always reflects
/// the actual resolution of the current domain.
pub const TICKS_PER_BEAT: u32 = 960;
```

-----

## Buffer Types

### `AudioBuffer`

Primary audio signal. Planar layout — all samples for channel 0, then channel 1, etc.
Interleaved conversion happens at the HAL boundary only. Never on the audio thread.

```rust
pub struct AudioBuffer {
    data: Vec<f32>,
    channels: usize,
    frames: usize,
}

impl AudioBuffer {
    /// Access a single channel as a flat slice.
    /// Panics if index >= channels.
    pub fn channel(&self, index: usize) -> &[f32];
    pub fn channel_mut(&mut self, index: usize) -> &mut [f32];
    
    pub fn channels(&self) -> usize;
    pub fn frames(&self) -> usize;
}
```

### `SignalBuffer` (internal base type)

The underlying allocation shared by all non-audio signal types.
**Never used directly by nodes.** Only appears in runtime internals.

```rust
pub(crate) struct SignalBuffer {
    data: Vec<f32>,
    frames: usize,
}
```

### Typed Signal Wrappers

All signal types are newtype wrappers around `SignalBuffer`. All implement:

- `Deref<Target = [f32]>` — access underlying samples directly
- `DerefMut<Target = [f32]>` — write underlying samples
- `AsSignal` — generic signal trait for DSP utilities
- `AsSignalMut` — mutable variant

```rust
/// General purpose CV. Audio rate, single-sample capable.
/// Convention: bipolar -1.0 to +1.0.
pub struct CvBuffer(SignalBuffer);

/// Phase signal. Unipolar 0.0 to just below 1.0.
/// Drives oscillators, lookup tables, step sequencers.
/// Convention: values outside range are wrapped at input ports.
pub struct PhaseBuffer(SignalBuffer);

/// Logic / gate / trigger signal.
/// High state: 1.0. Low state: 0.0.
/// Convention: threshold at 0.5 for inputs. Edge-sensitive triggers
/// fire on the 0→1 transition only.
pub struct LogicBuffer(SignalBuffer);

/// Pitch signal. Semitone-based convention.
/// 0.0 = middle C (C4). Each 1.0 = one semitone.
/// Typical range: -60.0 to +60.0 (five octaves each way from C4).
pub struct PitchBuffer(SignalBuffer);

/// Modulation rate signal. Sub-audio rate.
/// LFOs, envelopes, slow automation.
/// Convention: bipolar -1.0 to +1.0.
pub struct ModBuffer(SignalBuffer);
```

### `AsSignal` / `AsSignalMut` traits

Used by DSP utilities that operate generically across signal types.

```rust
pub trait AsSignal {
    fn frames(&self) -> usize;
    fn as_slice(&self) -> &[f32];
}

pub trait AsSignalMut: AsSignal {
    fn as_slice_mut(&mut self) -> &mut [f32];
}
```

All five signal wrapper types implement both traits.

-----

## Port Types

### `PortType`

Declared per port in `PortDescriptor`. Enforced at connection time by the runtime.
A `PhaseBuffer` output cannot connect to a `LogicBuffer` input — the runtime rejects it.

```rust
pub enum PortType {
    Audio,       // AudioBuffer — stereo or multi-channel
    Mono,        // AudioBuffer with channels == 1
    Cv,          // CvBuffer — audio rate CV
    Phase,       // PhaseBuffer — 0.0 to <1.0 ramp
    Logic,       // LogicBuffer — gate / trigger
    Pitch,       // PitchBuffer — semitone pitch CV
    Modulation,  // ModBuffer — sub-audio modulation
    Event,       // TimedEvent stream
    Clock,       // TransportInfo — tempo and position
}
```

### `PortDirection`

```rust
pub enum PortDirection {
    Input,
    Output,
}
```

### `PortName`

Hybrid static/dynamic port name. Use `Static` for fixed ports (the common case).
Use `Dynamic` for nodes with runtime-determined port names (mixers, modular graph).

```rust
pub enum PortName {
    Static(&'static str),
    Dynamic(String),
}

impl PortName {
    pub fn as_str(&self) -> &str;
}

impl From<&'static str> for PortName {
    fn from(s: &'static str) -> Self { PortName::Static(s) }
}

impl std::fmt::Display for PortName { ... }
```

### `PortDescriptor`

A node’s declaration of a single port. Returned from `Node::ports()`.

```rust
pub struct PortDescriptor {
    pub id: u32,
    pub name: PortName,
    pub direction: PortDirection,
    pub port_type: PortType,
}
```

-----

## Event Types

### `TimedEvent`

The fundamental event unit. A timestamp within the current buffer plus a typed event.
`Copy` — events live in pre-allocated slices, nodes copy them as needed.

```rust
#[derive(Clone, Copy)]
pub struct TimedEvent {
    /// Which sample within the current buffer this event occurs at.
    /// Range: 0 to block_size - 1.
    pub sample_offset: u32,
    pub event: Event,
}
```

### `Event`

Common event types are fixed-size enum variants. The `Extended` variant is a stub
for future custom node-to-node protocols (Phase 3+). The `#[cold]` attribute hints
to the compiler that `Extended` is rarely taken — keeps the hot path fast.

```rust
#[derive(Clone, Copy)]
pub enum Event {
    /// MIDI 2.0 Universal MIDI Packet. Covers all performance events:
    /// note on/off, per-note expression, pitch bend, pressure,
    /// channel controllers, program change, sysex.
    Midi2(midi2::UmpMessage),

    /// Sequencer parameter lock. Overrides a parameter value for one step.
    ParamLock(ParamLockEvent),

    /// Clock domain state change — loop point, stop, start, reposition.
    Transport(TransportEvent),

    /// Tempo change in BPM.
    Tempo(f64),

    /// Custom extended event. Points into the cycle's ExtendedEventSlab.
    /// space_id is negotiated at connection time via ConnectionAgreement.
    /// Stub at P0 — infrastructure ships at Phase 3.
    #[cold]
    Extended(ExtendedEventRef),
}
```

### `ParamLockEvent`

The sequencer’s per-step parameter override. Targets a specific parameter on a
specific node by content-addressed ID (hash of parameter name).

```rust
#[derive(Clone, Copy)]
pub struct ParamLockEvent {
    /// ID of the target node.
    pub node_id: u32,
    
    /// Hash of the parameter name. Stable per node type.
    /// Same parameter always has the same ID regardless of position or session.
    /// Parameter locks targeting IDs not in the current CapabilityDocument
    /// are held in suspension until the parameter returns.
    pub param_id: u32,
    
    /// Parameter value for this step.
    pub value: f64,
}
```

### `TransportEvent`

A clock domain state change occurring at a specific sample within the buffer.
Distinct from `TransportInfo` which is the current state at the start of the buffer.

```rust
#[derive(Clone, Copy)]
pub struct TransportEvent {
    pub domain_id: u32,
    pub bar: i32,
    pub beat: u32,
    pub tick: u32,
    pub ticks_per_beat: u32,
    pub bpm: f64,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub flags: TransportFlags,
}

#[derive(Clone, Copy)]
pub struct TransportFlags {
    pub playing: bool,
    pub recording: bool,
    pub looping: bool,
    /// This event marks a loop start boundary crossing.
    pub loop_start: bool,
    /// This event marks a loop end boundary crossing.
    pub loop_end: bool,
}
```

### `ExtendedEventRef`

Reference into the cycle’s `ExtendedEventSlab`. Stub at P0.

```rust
#[derive(Clone, Copy)]
pub struct ExtendedEventRef {
    /// Negotiated at connection time. Local to the connection.
    pub space_id: u16,
    pub event_type: u16,
    pub slab_offset: u32,
    pub size: u32,
}
```

### `ExtendedEventSlab`

Pre-allocated byte slab for rich custom events. Stub at P0 — present in
`ProcessInput` but empty. Full implementation at Phase 3.

```rust
pub struct ExtendedEventSlab {
    data: Box<[u8]>,
}

impl ExtendedEventSlab {
    /// Returns None if space_id does not match known_space_id.
    /// Use this to safely ignore extended events from other protocols.
    pub fn read_if_known(
        &self,
        event_ref: &ExtendedEventRef,
        known_space_id: u16,
    ) -> Option<&[u8]>;
}
```

-----

## Transport

### `TransportInfo`

Current clock state at the start of the processing buffer. Always present in
`ProcessInput`. Clock domain changes *within* the buffer arrive as `TimedEvent::Transport`.

```rust
#[derive(Clone, Copy)]
pub struct TransportInfo {
    pub domain_id: u32,
    pub bar: i32,
    pub beat: u32,
    pub tick: u32,
    /// For internal clock domain this is always TICKS_PER_BEAT.
    /// For external domains this reflects the domain's actual resolution.
    pub ticks_per_beat: u32,
    pub bpm: f64,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub playing: bool,
    pub recording: bool,
    pub looping: bool,
}
```

-----

## Process Context

### `ProcessInput`

Everything a node reads during `process()`. Immutable — nodes cannot modify
transport, events, or input buffers. Enforced structurally by the type system.

```rust
pub struct ProcessInput<'a> {
    /// Audio input buffers. One per connected audio input port,
    /// in port declaration order.
    pub audio_inputs: &'a [&'a AudioBuffer],
    
    /// Signal input buffers. Accessed by port id via typed accessors.
    signal_inputs: &'a [SignalInputSlot<'a>],
    
    /// Events for this node. Pre-filtered by the executor.
    /// Sorted by sample_offset ascending.
    pub events: &'a [TimedEvent],
    
    /// Current clock state at the start of this buffer.
    pub transport: &'a TransportInfo,
    
    pub sample_rate: f32,
    pub block_size: usize,
    
    /// Extended event slab. Stub at P0.
    pub extended_events: &'a ExtendedEventSlab,
}

impl<'a> ProcessInput<'a> {
    /// Access a CV input by port id.
    /// Panics (debug) / undefined behaviour (release) if port is wrong type.
    pub fn cv(&self, port_id: u32) -> &CvBuffer;
    pub fn phase(&self, port_id: u32) -> &PhaseBuffer;
    pub fn logic(&self, port_id: u32) -> &LogicBuffer;
    pub fn pitch(&self, port_id: u32) -> &PitchBuffer;
    pub fn modulation(&self, port_id: u32) -> &ModBuffer;
}
```

### `ProcessOutput`

Everything a node writes during `process()`. The only things a node is allowed
to modify. Enforced structurally.

```rust
pub struct ProcessOutput<'a> {
    /// Audio output buffers. One per connected audio output port,
    /// in port declaration order.
    pub audio_outputs: &'a mut [&'a mut AudioBuffer],
    
    /// Signal output buffers. Accessed by port id via typed accessors.
    signal_outputs: &'a mut [SignalOutputSlot<'a>],
    
    /// Outgoing events. Node writes here; executor routes to downstream nodes.
    pub events_out: &'a mut EventOutputBuffer,
}

impl<'a> ProcessOutput<'a> {
    pub fn cv_mut(&mut self, port_id: u32) -> &mut CvBuffer;
    pub fn phase_mut(&mut self, port_id: u32) -> &mut PhaseBuffer;
    pub fn logic_mut(&mut self, port_id: u32) -> &mut LogicBuffer;
    pub fn pitch_mut(&mut self, port_id: u32) -> &mut PitchBuffer;
    pub fn modulation_mut(&mut self, port_id: u32) -> &mut ModBuffer;
}
```

### `EventOutputBuffer`

Pre-allocated buffer for events emitted by a node during processing.

```rust
pub struct EventOutputBuffer {
    events: Vec<TimedEvent>,  // pre-allocated, cleared each cycle
    capacity: usize,
}

impl EventOutputBuffer {
    /// Push an outgoing event. Panics if capacity exceeded.
    /// sample_offset must be within current block_size.
    pub fn push(&mut self, event: TimedEvent);
    
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

-----

## Capability Document

### `ParamUnit`

Units for parameter display. Common cases get smart formatting.
Custom cases display the raw value and the unit string.

```rust
pub enum ParamUnit {
    Generic,
    Hz,
    Decibels,
    Milliseconds,
    Seconds,
    Semitones,
    Cents,
    Percent,
    Beats,
    /// Compile-time custom unit label.
    Custom(&'static str),
    /// Runtime-generated unit label. Allocates — only use when necessary.
    CustomDynamic(String),
}
```

### `ParamDisplay`

Override default unit formatting. Used for stepped parameters with named values
(waveform selectors, algorithm pickers, etc.) and any case where raw numbers
are not human-readable.

```rust
pub trait ParamDisplay: Send + Sync {
    fn format(&self, value: f64) -> String;
    fn parse(&self, s: &str) -> Option<f64>;
}

pub enum ParamDisplayAdapter {
    /// Baked into binary at compile time. Zero allocation.
    Static(&'static dyn ParamDisplay),
    /// Runtime-constructed. Use for dynamic label sets (e.g. sample slice names).
    Dynamic(Box<dyn ParamDisplay>),
}
```

### `ParamDescriptor`

Describes one parameter exposed by a node. Used by the sequencer for parameter
lock discovery, by the GUI for display, and by the scripting layer for automation.

```rust
pub struct ParamDescriptor {
    /// Hash of `name`. Stable per node type — same name always produces same id.
    /// Parameter locks targeting this id survive capability renegotiation
    /// as long as the parameter name is present in the new document.
    pub id: u32,
    
    pub name: PortName,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    
    /// True for integer-stepped parameters (algorithm selectors, etc.)
    pub stepped: bool,
    
    pub unit: ParamUnit,
    
    /// Override display formatting. None = use ParamUnit formatting.
    pub display: Option<ParamDisplayAdapter>,
}
```

### `CapabilityDocument`

A node’s complete self-description. Returned by `Node::capability_document()`.
Built on the main thread — allocation is fine here. Never built on the audio thread.

```rust
pub struct CapabilityDocument {
    pub name: &'static str,
    pub vendor: &'static str,
    /// Semantic versioning: (major, minor, patch)
    pub version: (u32, u32, u32),
    pub ports: Vec<PortDescriptor>,
    pub params: Vec<ParamDescriptor>,
    
    /// Extension identifiers this node implements.
    /// e.g. "paraclete.instrument", "paraclete.sequencer",
    ///      "com.yourcompany.custom_protocol"
    /// Used by other nodes and tooling for capability discovery.
    pub extensions: Vec<&'static str>,
}

impl CapabilityDocument {
    /// Build a minimal document from port declarations only.
    /// Used as the default implementation in Node::capability_document().
    pub fn from_ports(ports: &[PortDescriptor]) -> Self;
}
```

-----

## Connection Agreement

Produced by the runtime when two nodes connect. Stub at P0 — baseline only.
Full negotiation protocol at Phase 3.

```rust
pub struct ConnectionAgreement {
    /// Agreed audio buffer format.
    pub sample_rate: f32,
    pub block_size: usize,
    pub channels: usize,
    
    /// Negotiated space_ids for custom extended events.
    /// Empty at P0.
    pub space_ids: Vec<(String, u16)>,
}

impl ConnectionAgreement {
    /// Produces a baseline agreement with no custom extensions.
    /// Used as the default return value from Node::negotiate().
    pub fn baseline() -> Self;
}
```

-----

## The Node Trait

The universal contract. Every Paraclete node implements this trait and nothing else
is required. Three implicit levels of engagement — see ADR-008.

```rust
pub trait Node: Send {

    // ── Required ─────────────────────────────────────────────────────────────

    /// Declare this node's ports.
    /// Called once at node registration time.
    /// Must return a stable slice for the lifetime of the node.
    fn ports(&self) -> &[PortDescriptor];

    /// Process one buffer cycle.
    ///
    /// This is the hot path. Called once per buffer by the NodeExecutor
    /// on the audio thread. Must never allocate, block, or take locks.
    ///
    /// `input`  — everything the node reads (immutable)
    /// `output` — everything the node writes (audio buffers, signal buffers, events out)
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput);


    // ── Level 2 — override for instruments ───────────────────────────────────

    /// Describe this node's capabilities.
    ///
    /// Default builds a minimal document from ports() only.
    /// Override to declare parameters, extensions, and negotiation preferences.
    ///
    /// May be called at any time on the main thread.
    /// Must reflect current node state — see capabilities_changed().
    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument::from_ports(self.ports())
    }

    /// Return true if capability_document() would return a different document
    /// from the last time the runtime queried it.
    ///
    /// The runtime polls this at safe reconfiguration points (main thread only).
    /// When true, the runtime re-queries capability_document(), updates the
    /// state bus, and triggers renegotiation on affected connections.
    ///
    /// Static nodes never need to implement this.
    fn capabilities_changed(&self) -> bool { false }

    /// Called when the audio engine starts or the sample rate / block size changes.
    ///
    /// Use to allocate and initialise DSP state that depends on sample_rate
    /// or block_size (delay lines, filter coefficients, etc.)
    ///
    /// Called on the main thread before any process() calls.
    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {}

    /// Called when the audio engine stops.
    ///
    /// Use to release resources or flush state.
    /// Called on the main thread after the last process() call.
    fn deactivate(&mut self) {}

    /// Serialise private state for project save.
    ///
    /// Return an opaque byte blob. The runtime stores it and passes it back
    /// to deserialize() when the project is loaded. Format is node-defined.
    ///
    /// Return empty Vec if the node has no persistent state.
    fn serialize(&self) -> Vec<u8> { vec![] }

    /// Restore private state from a project load.
    ///
    /// `data` is the blob previously returned by serialize().
    /// Called on the main thread before activate().
    fn deserialize(&mut self, _data: &[u8]) {}


    // ── Level 3 — override for smart nodes ───────────────────────────────────

    /// Participate in the connection handshake.
    ///
    /// When two nodes connect, the runtime calls negotiate() on both sides,
    /// passing each node the other's CapabilityDocument. The runtime reconciles
    /// the two ConnectionAgreements into a final agreement shared by both nodes.
    ///
    /// Default returns ConnectionAgreement::baseline() — core event types only,
    /// no custom extensions, standard audio format.
    ///
    /// Override to negotiate custom event spaces, agree on channel counts,
    /// or configure connection-specific behaviour.
    ///
    /// Called on the main thread at connection time.
    fn negotiate(
        &mut self,
        _their_doc: &CapabilityDocument,
    ) -> ConnectionAgreement {
        ConnectionAgreement::baseline()
    }
}
```

-----

## Template Nodes

Pre-built `Node` implementations for common archetypes. The primary contribution
path for third-party developers. Ships at Phase 1 alongside the first real nodes.

```rust
/// For pure signal processors: oscillators, filters, effects, math modules.
/// Requires implementing only the DSP.
pub trait SignalNodeTemplate: Node {
    fn dsp(&mut self, input: &ProcessInput, output: &mut ProcessOutput);
}

/// For MIDI-driven instruments: synths, samplers.
pub trait InstrumentTemplate: Node {
    fn handle_note_on(&mut self, event: &midi2::NoteOn);
    fn handle_note_off(&mut self, event: &midi2::NoteOff);
    fn render(&mut self, output: &mut [&mut AudioBuffer]);
}

/// For event-generating sequencer nodes.
pub trait SequencerTemplate: Node { ... }

/// For hardware controller nodes.
pub trait ControllerTemplate: Node { ... }
```

-----

## Port ID Conventions

Nodes define their port IDs as `u32` constants. Convention: declare them as
associated constants or module-level consts near the node struct.

```rust
// Example: a simple stereo filter node
pub struct MyFilter { ... }

impl MyFilter {
    pub const PORT_AUDIO_IN:  u32 = 0;
    pub const PORT_AUDIO_OUT: u32 = 1;
    pub const PORT_CUTOFF:    u32 = 2;  // ModBuffer input
    pub const PORT_RESONANCE: u32 = 3;  // ModBuffer input
}

impl Node for MyFilter {
    fn ports(&self) -> &[PortDescriptor] {
        &[
            PortDescriptor { id: Self::PORT_AUDIO_IN,  name: "audio_in".into(),  direction: Input,  port_type: PortType::Audio },
            PortDescriptor { id: Self::PORT_AUDIO_OUT, name: "audio_out".into(), direction: Output, port_type: PortType::Audio },
            PortDescriptor { id: Self::PORT_CUTOFF,    name: "cutoff".into(),    direction: Input,  port_type: PortType::Modulation },
            PortDescriptor { id: Self::PORT_RESONANCE, name: "resonance".into(), direction: Input,  port_type: PortType::Modulation },
        ]
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let audio_in  = &input.audio_inputs[0];
        let cutoff    = input.modulation(Self::PORT_CUTOFF);
        let resonance = input.modulation(Self::PORT_RESONANCE);
        let audio_out = &mut output.audio_outputs[0];
        
        // DSP here — cutoff and resonance are &ModBuffer, deref to &[f32]
        for frame in 0..input.block_size {
            let cut = cutoff[frame];
            let res = resonance[frame];
            // ... filter DSP
        }
    }
}
```

-----

## What Ships at P0

P0 implementation delivers:

|Component                 |Crate                |Status                       |
|--------------------------|---------------------|-----------------------------|
|All types in this document|`paraclete-node-api` |Full                         |
|`ConnectionAgreement`     |`paraclete-node-api` |Baseline stub only           |
|`ExtendedEventSlab`       |`paraclete-node-api` |Stub — present but empty     |
|Template node traits      |`paraclete-node-api` |Signatures only, no impl     |
|`NodeConfigurator`        |`paraclete-runtime`  |Minimal — register one node  |
|`NodeExecutor`            |`paraclete-runtime`  |Minimal — execute one node   |
|Lock-free ring buffer     |`paraclete-runtime`  |Full                         |
|HAL audio backend         |`paraclete-hal`      |cpal integration, one device |
|Launchpad emulator        |`paraclete-hal`      |Basic surface, keyboard input|
|Rhai sandbox              |`paraclete-scripting`|Sandboxed engine, no bindings|

P0 does not ship:

- State bus (P1)
- Multiple connected nodes (P1)
- Event routing between nodes (P2)
- Full capability negotiation (P3)
- Extended event slab (P3)

-----

## File Structure

```
paraclete-node-api/
├── Cargo.toml
├── src/
│   ├── lib.rs              ← re-exports everything public
│   ├── buffer.rs           ← AudioBuffer, SignalBuffer, typed wrappers, AsSignal
│   ├── port.rs             ← PortDescriptor, PortType, PortDirection, PortName
│   ├── event.rs            ← TimedEvent, Event, all event structs, ExtendedEventSlab
│   ├── transport.rs        ← TransportInfo, TransportEvent, TransportFlags
│   ├── context.rs          ← ProcessInput, ProcessOutput, EventOutputBuffer
│   ├── capability.rs       ← CapabilityDocument, ParamDescriptor, ParamUnit, ParamDisplay
│   ├── agreement.rs        ← ConnectionAgreement
│   ├── node.rs             ← Node trait
│   ├── templates.rs        ← SignalNodeTemplate, InstrumentTemplate, etc.
│   └── constants.rs        ← TICKS_PER_BEAT and other constants
└── README.md               ← LGPL3 notice and quick-start guide
```