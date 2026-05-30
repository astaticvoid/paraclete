# Paraclete — P3 Interface Specification

> **Implementation blueprint.** These are the contracts P3 implementation must satisfy.
> Do not deviate without updating the relevant ADR first.
>
> **Deliverable:** Sampler node driving real audio. Full parameter lock protocol.
> State bus SPSC upgrade. Rhai state bus bindings.
> **Phase:** P3 — First Instrument
> **Last updated:** May 2026
> **Depends on:** p0-interfaces.md, p1-interfaces.md, p2-interfaces.md — all prior
> types assumed present. All identifiers are plain English per vocabulary-revert.md.

---

## Overview

P3 delivers four things:

1. **`Negotiable` trait** — full connection handshake. The `Sampler` is the first
   node to declare its lockable parameters at connection time so the `Sequencer`
   can discover them.

2. **Full parameter lock protocol** — the complete path from sequencer step
   parameter lock → `ParamLockEvent` → sampler applies override → gate closes →
   sampler restores base value.

3. **`Sampler` node** — first real instrument. 4-voice polyphonic pool, effectively
   mono by default. Filesystem WAV loading. Single full-sample slice stub,
   expandable. All parameters lockable.

4. **State bus SPSC upgrade + Rhai bindings** — replaces `Arc<RwLock<HashMap>>`
   with lock-free `rtrb` SPSC channel. Rhai scripts gain read/write access to the
   state bus. API designed to be stable across implementation change.

The P3 graph:

```
InternalClock → Sequencer → Sampler → AudioOutput
                    ↑
LaunchpadEmulator → HardwareMappingNode
```

---

## New Crate Dependencies

### `paraclete-nodes` (L3 — GPL3)

```toml
[dependencies]
hound = "3.5"   # WAV file loading — MIT licensed
```

For future format support: `symphonia` (MIT) is the documented upgrade path.
The sample loading function is isolated — swapping `hound` for `symphonia`
touches one function in `sampler.rs`.

### `paraclete-runtime` (L1 — GPL3)

```toml
[dependencies]
rtrb = "0.3"   # Lock-free SPSC ring buffer — MIT licensed
```

---

## Part 1: `Negotiable` Trait

### Purpose

When two nodes connect, the runtime brokers a capability handshake. `Negotiable`
lets a node participate actively — declaring what it exposes, what it needs, and
what parameters it will accept locks for.

At P3 this solves one specific problem: the `Sequencer` needs to know which
parameters the `Sampler` exposes for locking. The sampler declares them in
`negotiate()`. The sequencer reads them from the returned `ConnectionAgreement`.

### Trait definition

```rust
/// A node that participates actively in connection handshakes.
///
/// When two nodes connect, the runtime calls negotiate() on both sides,
/// passing each node the other's CapabilityDocument. The runtime
/// reconciles the two ConnectionAgreements into a ConnectionRecord
/// delivered to both sides.
///
/// Nodes that do not implement this trait use ConnectionAgreement::baseline()
/// for all connections.
pub trait Negotiable: Node {
    fn negotiate(&mut self, their_doc: &CapabilityDocument) -> ConnectionAgreement;

    /// Called by the runtime after both sides have negotiated.
    /// Store the ConnectionRecord to know who you are connected to
    /// and what was agreed.
    fn set_connection_record(&mut self, record: ConnectionRecord) {}
}
```

### `ConnectionAgreement` — updated for P3

```rust
pub struct ConnectionAgreement {
    pub sample_rate: f32,
    pub block_size: usize,
    pub channels: usize,
    pub space_ids: Vec<(String, u16)>,
    pub initial_transport: Option<TransportInfo>,

    /// Parameters this node will accept ParamLockEvent for.
    /// Populated by the receiving node (e.g. Sampler) in negotiate().
    /// The sending node (e.g. Sequencer) reads this to know what it can lock.
    pub lockable_params: Vec<LockableParam>,
}

/// A parameter exposed for locking by a connected sequencer.
#[derive(Clone, Debug)]
pub struct LockableParam {
    /// Hash of parameter name. Matches ParamDescriptor.id.
    pub param_id: u32,
    /// Display name for sequencer UI.
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: ParamUnit,
}
```

### `ConnectionRecord`

Delivered to both nodes after negotiation completes.

```rust
pub struct ConnectionRecord {
    pub agreement: ConnectionAgreement,
    /// The NodeId of the connected partner.
    pub partner_id: NodeId,
    /// Which of this node's ports this record applies to.
    pub local_port_id: u32,
}
```

### Handshake protocol

```
Sequencer.ports()[EVENTS_OUT] connects to Sampler.ports()[EVENTS_IN]

  → Runtime calls Sampler.capability_document()    → CapDoc_sampler
  → Runtime calls Sequencer.capability_document()  → CapDoc_sequencer
  → Runtime calls Sequencer.negotiate(&CapDoc_sampler)  → Agreement_seq
  → Runtime calls Sampler.negotiate(&CapDoc_sequencer)  → Agreement_samp
  → Runtime reconciles into ConnectionRecord
  → Sequencer.set_connection_record(record)
  → Sampler.set_connection_record(record)
```

The `Sequencer` stores `agreement.lockable_params` from the sampler's response.
When the user configures a parameter lock on a step, the sequencer UI queries
this list to know what is available.

### Runtime changes for negotiation

`NodeConfigurator::connect()` now invokes the handshake if either node
implements `Negotiable`. Both nodes receive a `ConnectionRecord`.

The `src_port` and `dst_port` fields on `EdgeMeta` (previously dead code) are
used here — the `ConnectionRecord.local_port_id` is populated from these.
Remove the `#[allow(dead_code)]` attributes.

---

## Part 2: Full Parameter Lock Protocol

### The complete path

```
1. User sets a param lock on Sequencer step 3:
   { node_id: sampler_id, param_id: hash("pitch"), value: 2.0 }
   (pitch up 2 semitones for this step)

2. Sequencer reaches step 3 during playback.
   Emits before the NoteOn:
   TimedEvent {
     sample_offset: n,
     event: Event::ParamLock(ParamLockEvent {
       node_id: sampler_id,
       param_id: hash("pitch"),
       value: 2.0,
     })
   }

3. Executor routes ParamLockEvent to Sampler (node_id match).

4. Sampler receives it in process(). It:
   a. Stores the base value if not already locked
   b. Applies the locked value immediately

5. Gate closes (NoteOff or sample end).
   Sampler restores all locked params to their base values.
```

### Ordering guarantee

The sequencer emits `ParamLockEvent` before the `NoteOn` for the same step,
both with the same `sample_offset`. The executor must deliver `ParamLockEvent`
before `Midi2 NoteOn` within the same sample offset. Sorting events by type
priority within the same offset achieves this:

```rust
// Event delivery order within the same sample_offset:
// 1. ParamLockEvent  — apply overrides first
// 2. TransportEvent  — position updates
// 3. Midi2           — notes and controllers
// 4. Hardware        — pad events
// 5. Extended        — custom
```

Add this sort to the executor's event delivery path.

### Active lock tracking

```rust
struct ActiveParamLock {
    param_id: u32,
    base_value: f64,
    locked_value: f64,
}
```

Locks are cleared and base values restored on gate close (NoteOff or sample end).
Multiple locks on one step all restore independently.
A lock for an unknown `param_id` is silently ignored.

---

## Part 3: `Sampler` Node

### Design decisions

**Polyphony:** 4-voice pool, oldest-voice stealing. Effectively mono at default
sequencer settings (short note lengths, one note per step). Overlap is possible
by increasing note length or playing live from the Launchpad.

**Sample loading:** Filesystem path. WAV via `hound`. Path stored in `serialize()`,
reloaded at `activate()`.

**Slice mode:** Stubbed. One slice covers the full sample at P3. The `slice`
parameter is declared and lockable. Slice boundaries are added later without
changing the architecture.

### Port declarations

```rust
impl Node for Sampler {
    fn ports(&self) -> &[PortDescriptor] {
        &[
            PortDescriptor {
                id: Self::PORT_EVENTS_IN,
                name: "events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            },
            PortDescriptor {
                id: Self::PORT_AUDIO_OUT_L,
                name: "audio_out_l".into(),
                direction: PortDirection::Output,
                port_type: PortType::Mono,
            },
            PortDescriptor {
                id: Self::PORT_AUDIO_OUT_R,
                name: "audio_out_r".into(),
                direction: PortDirection::Output,
                port_type: PortType::Mono,
            },
            PortDescriptor {
                id: Self::PORT_PITCH_MOD,
                name: "pitch_mod".into(),
                direction: PortDirection::Input,
                port_type: PortType::Modulation,
            },
            PortDescriptor {
                id: Self::PORT_VOLUME_MOD,
                name: "volume_mod".into(),
                direction: PortDirection::Input,
                port_type: PortType::Modulation,
            },
        ]
    }
}

impl Sampler {
    pub const PORT_EVENTS_IN:   u32 = 0;
    pub const PORT_AUDIO_OUT_L: u32 = 1;
    pub const PORT_AUDIO_OUT_R: u32 = 2;
    pub const PORT_PITCH_MOD:   u32 = 3;
    pub const PORT_VOLUME_MOD:  u32 = 4;
}
```

### Parameters

All parameters are lockable. Declared in `capability_document()` and in the
`ConnectionAgreement.lockable_params` returned from `negotiate()`.

```rust
fn capability_document(&self) -> CapabilityDocument {
    CapabilityDocument {
        name: "Sampler",
        vendor: "Paraclete",
        version: (0, 3, 0),
        ports: self.ports().to_vec(),
        params: vec![
            ParamDescriptor {
                id: param_hash("pitch"),
                name: "pitch".into(),
                min: -24.0, max: 24.0, default: 0.0,
                stepped: false,
                unit: ParamUnit::Semitones,
                display: None,
            },
            ParamDescriptor {
                id: param_hash("volume"),
                name: "volume".into(),
                min: 0.0, max: 1.0, default: 0.8,
                stepped: false,
                unit: ParamUnit::Generic,
                display: None,
            },
            ParamDescriptor {
                id: param_hash("pan"),
                name: "pan".into(),
                min: -1.0, max: 1.0, default: 0.0,
                stepped: false,
                unit: ParamUnit::Generic,
                display: None,
            },
            ParamDescriptor {
                id: param_hash("start"),
                name: "start".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false,
                unit: ParamUnit::Percent,
                display: None,
            },
            ParamDescriptor {
                id: param_hash("end"),
                name: "end".into(),
                min: 0.0, max: 1.0, default: 1.0,
                stepped: false,
                unit: ParamUnit::Percent,
                display: None,
            },
            ParamDescriptor {
                id: param_hash("loop"),
                name: "loop".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: true,
                unit: ParamUnit::Generic,
                display: Some(ParamDisplayAdapter::Static(&ON_OFF_DISPLAY)),
            },
            // Slice index — stub at P3, always 0
            ParamDescriptor {
                id: param_hash("slice"),
                name: "slice".into(),
                min: 0.0, max: 127.0, default: 0.0,
                stepped: true,
                unit: ParamUnit::Generic,
                display: None,
            },
        ],
        extensions: vec!["paraclete.instrument"],
    }
}
```

### Voice model

```rust
/// One voice in the sampler voice pool.
struct Voice {
    /// Whether this voice is currently active.
    active: bool,
    /// MIDI note number that triggered this voice.
    note: u8,
    /// Current playback position within the sample (fractional frame index).
    playback_pos: f64,
    /// Per-voice parameter lock overrides.
    /// These are distinct from the node-level base params — each voice
    /// inherits the node's effective params at trigger time, then applies
    /// its own locks independently.
    active_locks: HashMap<u32, ActiveParamLock>,
    /// When this voice was triggered (cycle counter).
    /// Used for oldest-voice stealing.
    triggered_at: u64,
}

struct ActiveParamLock {
    param_id: u32,
    base_value: f64,
    locked_value: f64,
}
```

### Internal state

```rust
pub struct Sampler {
    node_id: NodeId,

    // Sample data — loaded at activate()
    sample_data: Vec<f32>,   // mono PCM, f32 normalised -1.0 to +1.0
    sample_rate: f32,        // native sample rate of loaded file
    sample_frames: usize,

    // Slice table — one entry at P3: (0, sample_frames)
    // Each entry is (start_frame, end_frame)
    slices: Vec<(usize, usize)>,

    // Base parameters (user-set values, pre-lock)
    base_pitch:  f64,   // semitone offset
    base_volume: f64,   // 0.0–1.0
    base_pan:    f64,   // -1.0–+1.0
    base_start:  f64,   // 0.0–1.0 fraction of sample
    base_end:    f64,   // 0.0–1.0 fraction of sample
    base_loop:   bool,
    base_slice:  usize, // index into slices vec

    // Node-level active locks (applied before voice trigger,
    // affect the effective params voices inherit)
    node_locks: HashMap<u32, ActiveParamLock>,

    // Voice pool
    voices: [Voice; 4],
    cycle_counter: u64,

    // Runtime
    output_sample_rate: f32,   // set in activate()
    root_note: u8,             // MIDI note at which sample plays at native pitch (default 60)

    // Sample path — persisted in serialize()
    sample_path: Option<String>,

    // Connection records from Negotiable
    connection_records: Vec<ConnectionRecord>,
}
```

### `activate()` — sample loading

```rust
fn activate(&mut self, sample_rate: f32, _block_size: usize) {
    self.output_sample_rate = sample_rate;

    if let Some(ref path) = self.sample_path {
        match load_wav(path, sample_rate) {
            Ok(data) => {
                self.sample_frames = data.len();
                self.sample_data = data;
                // Reset slice table to single full-sample slice
                self.slices = vec![(0, self.sample_frames)];
            }
            Err(e) => {
                // Never panic on load failure — audio must continue
                // Set a silent single-sample buffer
                self.sample_data = vec![0.0; 1];
                self.sample_frames = 1;
                self.slices = vec![(0, 1)];
                eprintln!("Sampler: failed to load {:?}: {}", path, e);
            }
        }
    }
}
```

### WAV loading helper

```rust
/// Load a WAV file, resampling to target_rate if needed.
/// Returns mono f32 PCM normalised to -1.0..+1.0.
fn load_wav(path: &str, target_rate: f32) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("hound: {}", e))?;

    let spec = reader.spec();
    let native_rate = spec.sample_rate as f32;

    // Read samples — handle i16, i32, f32 formats
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader.samples::<f32>()
                .map(|s| s.unwrap_or(0.0))
                .collect()
        }
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample as f32;
            let max = 2.0_f32.powf(bits - 1.0);
            reader.samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / max)
                .collect()
        }
    };

    // Mix to mono if stereo
    let mono: Vec<f32> = if spec.channels == 2 {
        samples.chunks(2)
            .map(|c| (c[0] + c.get(1).copied().unwrap_or(0.0)) * 0.5)
            .collect()
    } else {
        samples
    };

    // Resample if needed — simple linear interpolation for P3
    // TODO: replace with high-quality resampler (rubato crate) at P6
    if (native_rate - target_rate).abs() < 1.0 {
        Ok(mono)
    } else {
        let ratio = native_rate / target_rate;
        let output_len = (mono.len() as f32 / ratio) as usize;
        let mut resampled = Vec::with_capacity(output_len);
        for i in 0..output_len {
            let pos = i as f32 * ratio;
            let idx = pos as usize;
            let frac = pos - idx as f32;
            let s0 = mono.get(idx).copied().unwrap_or(0.0);
            let s1 = mono.get(idx + 1).copied().unwrap_or(0.0);
            resampled.push(s0 + (s1 - s0) * frac);
        }
        Ok(resampled)
    }
}
```

Note the resampling TODO — linear interpolation is acceptable for P3, but
`rubato` (MIT licensed) should replace it at P6 when audio quality matters more.

### `process()` — main audio loop

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.cycle_counter += 1;

    // 1. Handle events — param locks first, then notes
    for event in input.events {
        match event.event {
            Event::ParamLock(ref lock) if lock.node_id == self.node_id => {
                self.apply_node_lock(lock);
            }
            Event::Midi2(ref ump) => {
                if let Some(note_on) = ump.try_as_note_on() {
                    self.trigger_voice(
                        note_on.note_number(),
                        note_on.velocity(),
                        event.sample_offset,
                    );
                } else if let Some(note_off) = ump.try_as_note_off() {
                    self.release_voice(note_off.note_number(), event.sample_offset);
                }
            }
            _ => {}
        }
    }

    // 2. Compute effective node-level params (base + node locks + mod inputs)
    let pitch_mod = input.modulation(Self::PORT_PITCH_MOD)
        .first().copied().unwrap_or(0.0) as f64 * 12.0; // ±12 semitone range
    let volume_mod = input.modulation(Self::PORT_VOLUME_MOD)
        .first().copied().unwrap_or(0.0) as f64;

    let effective_volume = (self.effective_param(param_hash("volume")) + volume_mod)
        .clamp(0.0, 1.0);
    let effective_pan = self.effective_param(param_hash("pan")).clamp(-1.0, 1.0);

    // 3. Render all active voices
    let out_l = output.audio_outputs[0].channel_mut(0);
    let out_r = output.audio_outputs[1].channel_mut(0);

    // Zero outputs first
    for frame in out_l.iter_mut() { *frame = 0.0; }
    for frame in out_r.iter_mut() { *frame = 0.0; }

    for voice in self.voices.iter_mut().filter(|v| v.active) {
        // Per-voice effective pitch (node base + node lock + voice lock + mod)
        let voice_pitch = self.base_pitch
            + voice.locked_value_or_base(param_hash("pitch"), self.base_pitch)
            + pitch_mod;

        let note_diff = voice.note as f64 - self.root_note as f64 + voice_pitch;
        let playback_rate = 2.0_f64.powf(note_diff / 12.0)
            * self.sample_rate as f64 / self.output_sample_rate as f64;

        // Determine active slice bounds
        let slice_idx = self.effective_param(param_hash("slice")) as usize;
        let (slice_start, slice_end) = self.slices
            .get(slice_idx)
            .copied()
            .unwrap_or((0, self.sample_frames));

        // Apply start/end params within slice bounds
        let start_frame = slice_start +
            (self.effective_param(param_hash("start"))
             * (slice_end - slice_start) as f64) as usize;
        let end_frame = slice_start +
            (self.effective_param(param_hash("end"))
             * (slice_end - slice_start) as f64) as usize;

        let looping = self.effective_param(param_hash("loop")) >= 0.5;

        for frame in 0..input.block_size {
            let abs_pos = start_frame as f64 + voice.playback_pos;

            let sample = if abs_pos < end_frame as f64 {
                // Linear interpolation
                let idx = abs_pos as usize;
                let frac = (abs_pos - idx as f64) as f32;
                let s0 = self.sample_data.get(idx).copied().unwrap_or(0.0);
                let s1 = self.sample_data.get(idx + 1).copied().unwrap_or(0.0);
                voice.playback_pos += playback_rate;
                s0 + (s1 - s0) * frac
            } else if looping {
                voice.playback_pos = 0.0;
                0.0
            } else {
                // Sample ended
                voice.active = false;
                self.restore_voice_locks(voice);
                0.0
            };

            // Accumulate into outputs with volume and pan
            let vol = effective_volume as f32;
            let pan_l = ((1.0 - effective_pan as f32) * 0.5 + 0.5).sqrt();
            let pan_r = ((1.0 + effective_pan as f32) * 0.5 + 0.5).sqrt();
            out_l[frame] += sample * vol * pan_l;
            out_r[frame] += sample * vol * pan_r;
        }
    }
}
```

### Voice management

```rust
fn trigger_voice(&mut self, note: u8, velocity: u16, _sample_offset: u32) {
    // Find a free voice, or steal the oldest active one
    let voice_idx = self.voices.iter().position(|v| !v.active)
        .unwrap_or_else(|| {
            // Steal oldest
            self.voices.iter()
                .enumerate()
                .min_by_key(|(_, v)| v.triggered_at)
                .map(|(i, _)| i)
                .unwrap_or(0)
        });

    let voice = &mut self.voices[voice_idx];
    voice.active = true;
    voice.note = note;
    voice.playback_pos = 0.0;
    voice.triggered_at = self.cycle_counter;
    voice.active_locks.clear();

    // Inherit current node-level locks at trigger time
    for (param_id, lock) in &self.node_locks {
        voice.active_locks.insert(*param_id, lock.clone());
    }
}

fn release_voice(&mut self, note: u8, _sample_offset: u32) {
    // Release the most recently triggered voice for this note
    if let Some(voice) = self.voices.iter_mut()
        .filter(|v| v.active && v.note == note)
        .max_by_key(|v| v.triggered_at)
    {
        voice.active = false;
        self.restore_node_locks_from_voice(voice);
    }
}
```

### `Negotiable` implementation

```rust
impl Negotiable for Sampler {
    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        let mut agreement = ConnectionAgreement::baseline();

        agreement.lockable_params = self.capability_document().params
            .iter()
            .map(|p| LockableParam {
                param_id: p.id,
                name: p.name.as_str().to_string(),
                min: p.min,
                max: p.max,
                default: p.default,
                unit: p.unit.clone(),
            })
            .collect();

        agreement
    }

    fn set_connection_record(&mut self, record: ConnectionRecord) {
        self.connection_records.push(record);
    }
}
```

### `serialize()` / `deserialize()`

Persists sample path and all base parameter values. Does not persist sample
data — reloaded from path at `activate()`.

```rust
fn serialize(&self) -> Vec<u8> {
    // Format version 1 (u8)
    // path_len (u16 LE) + path bytes (UTF-8)
    // root_note (u8)
    // pitch, volume, pan, start, end (f64 LE each)
    // loop (u8 — 0 or 1)
    // slice (u8)
    let mut buf = vec![1u8]; // version
    let path = self.sample_path.as_deref().unwrap_or("");
    let path_bytes = path.as_bytes();
    buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(path_bytes);
    buf.push(self.root_note);
    for &val in &[self.base_pitch, self.base_volume, self.base_pan,
                   self.base_start, self.base_end] {
        buf.extend_from_slice(&val.to_le_bytes());
    }
    buf.push(self.base_loop as u8);
    buf.push(self.base_slice as u8);
    buf
}

fn deserialize(&mut self, data: &[u8]) {
    if data.is_empty() || data[0] != 1 { return; } // unknown version — ignore
    // ... decode matching serialize() format
}
```

---

## Part 4: State Bus SPSC Upgrade

### Why at P3, not P4

The Rhai bindings added at P3 must be written against the stable API, not the
provisional `RwLock` backing store. Correct order: upgrade the implementation
first, then add Rhai bindings on top.

### Architecture

```
Audio thread (NodeExecutor):
  After all process() calls complete:
  → Collect published_state() from all StatePublisher nodes into Vec
  → Push one StateBusUpdate to rtrb producer — non-blocking
  → No lock held at any point on the audio thread

Main thread (NodeConfigurator):
  Between cycles (in process_state_bus()):
  → Drain rtrb consumer into owned HashMap
  → Notify subscriptions
  → Build HardwareOutput from updated values
  → Call update_output() on all hardware devices
```

### `rtrb` integration

```rust
// In NodeConfigurator setup
use rtrb::RingBuffer;

pub struct NodeConfigurator {
    // existing fields...
    state_bus: StateBusHandle,
    state_bus_consumer: rtrb::Consumer<StateBusUpdate>,
}

pub struct NodeExecutor {
    // existing fields...
    state_bus_producer: rtrb::Producer<StateBusUpdate>,
}

/// One cycle's worth of state bus updates.
pub struct StateBusUpdate {
    pub entries: Vec<(String, StateBusValue)>,
}
```

Ring buffer capacity: 256 updates (one per cycle, with headroom). If the
producer finds the buffer full (main thread is behind), it drops the update
for that cycle. State bus values are not critical-path — a missed update means
one cycle of stale LED state, not a crash.

### `StateBusHandle` — stable API

```rust
/// The stable state bus access API.
/// All consumers — Rhai scripts, GUI, hardware LED feedback — use this.
/// The backing implementation (RwLock → SPSC) is invisible to callers.
pub struct StateBusHandle {
    store: HashMap<String, StateBusValue>,
    subscriptions: Vec<StateBusSubscription>,
}

impl StateBusHandle {
    pub fn read(&self, path: &str) -> Option<&StateBusValue>;
    pub fn write(&mut self, path: &str, value: StateBusValue);
    pub fn subscribe(&mut self, path: &str) -> StateBusSubscription;

    /// Called by NodeConfigurator between cycles.
    /// Drains the SPSC consumer and applies updates to the store.
    pub fn drain_updates(&mut self, consumer: &mut rtrb::Consumer<StateBusUpdate>);

    /// Notify all active subscriptions of any changed values.
    /// Called after drain_updates().
    pub fn notify_subscriptions(&mut self);
}
```

### `NodeConfigurator::process_state_bus()`

```rust
/// Call from the main loop between audio cycles.
pub fn process_state_bus(&mut self) {
    self.state_bus.drain_updates(&mut self.state_bus_consumer);
    self.state_bus.notify_subscriptions();

    let hw_output = self.build_hardware_output();
    for device in &mut self.hardware_devices {
        device.update_output(&hw_output);
    }
}
```

---

## Part 5: Rhai State Bus Bindings

### Design principle

Scripts access the state bus through a stable simple API. The SPSC backing store
is completely invisible. Scripts run on the main thread — they read from and write
to the `StateBusHandle` directly, no cross-thread concerns.

### Rhai API

```rhai
// Read — returns the value or () if not found
let bpm = state_bus.read("/transport/bpm");
let step = state_bus.read("/node/3/state/current_step");

// Write — sets a parameter value
state_bus.write("/node/3/param/pitch", 2.0);

// Subscribe — returns a handle, check in script loop
let sub = state_bus.subscribe("/node/3/state/current_step");
if sub.changed() {
    let new_step = sub.value();
}
```

### Rust registration

```rust
pub fn register_state_bus_api(
    engine: &mut rhai::Engine,
    handle: Rc<RefCell<StateBusHandle>>,
) {
    engine.register_type_with_name::<StateBusProxy>("StateBus");

    engine.register_fn("read", |proxy: &mut StateBusProxy, path: &str|
        -> rhai::Dynamic {
            proxy.read(path).into()
        }
    );

    engine.register_fn("write", |proxy: &mut StateBusProxy, path: &str, value: f64| {
        proxy.write(path, StateBusValue::Float(value));
    });

    engine.register_fn("subscribe",
        |proxy: &mut StateBusProxy, path: &str| -> StateBusSubscriptionProxy {
            proxy.subscribe(path)
        }
    );
}
```

`Rc<RefCell>` not `Arc<Mutex>` — scripts run on the main thread only. No cross-thread
access needed. `RefCell` gives interior mutability with no synchronisation overhead.

### Script sandbox

Scripts can:
- `read()` any path
- `write()` to `/node/{id}/param/*` paths
- `subscribe()` to any path

Scripts cannot (runtime enforcement via path prefix check):
- `write()` to `/transport/*`
- `write()` to `/hw/*`

---

## Part 6: Signal Port Stubs — Fix

The `modulation()`, `cv()`, `phase()`, `logic()`, `pitch()` accessors on
`ProcessInput` currently `unimplemented!()`. The `Sampler` uses `modulation()`.
These must return a silent buffer for unconnected ports rather than panic.

```rust
impl<'a> ProcessInput<'a> {
    pub fn modulation(&self, port_id: u32) -> &ModBuffer {
        match self.signal_slot(port_id) {
            Some(SignalInputSlot::Mod(b)) => b,
            Some(_) => panic!("Port {} is not a Modulation port", port_id),
            None => &SILENT_MOD_BUFFER, // unconnected — return silence
        }
    }
    // same pattern for cv(), phase(), logic(), pitch()
}

/// A static zeroed ModBuffer returned for unconnected modulation ports.
static SILENT_MOD_BUFFER: LazyLock<ModBuffer> =
    LazyLock::new(|| ModBuffer::zeroed(MAX_BLOCK_SIZE));
```

Panic on wrong type (a node requesting a `ModBuffer` for a port declared as
`PortType::Phase`) — this is a programming error. Return silence on unconnected —
this is normal operation.

---

## Part 7: P3 App Wiring

```rust
fn main() {
    let mut conf = NodeConfigurator::new();

    let clock_id    = conf.add_tempo_source(Box::new(InternalClock::new()));
    let seq_id      = conf.add_node(Box::new(Sequencer::new()));
    let sampler_id  = conf.add_node(Box::new(Sampler::with_path("samples/kick.wav")));
    let emulator_id = conf.add_hardware_device(Box::new(LaunchpadEmulator::new()));
    let mapper_id   = conf.add_node(Box::new(HardwareMappingNode::default_chromatic(0)));

    // Clock
    conf.connect(clock_id,  InternalClock::PORT_CLOCK_OUT,
                 seq_id,    Sequencer::PORT_CLOCK_IN).unwrap();

    // Controller path
    conf.connect(emulator_id, 0, mapper_id, 0).unwrap();
    conf.connect(mapper_id,   1, seq_id, Sequencer::PORT_EVENTS_IN).unwrap();

    // Sequencer → sampler (triggers Negotiable handshake)
    conf.connect(seq_id,     Sequencer::PORT_EVENTS_OUT,
                 sampler_id, Sampler::PORT_EVENTS_IN).unwrap();

    // Sampler → audio out
    conf.connect(sampler_id, Sampler::PORT_AUDIO_OUT_L, audio_l_id, 0).unwrap();
    conf.connect(sampler_id, Sampler::PORT_AUDIO_OUT_R, audio_r_id, 0).unwrap();

    let executor = conf.build_executor();
    let _stream = start_audio_stream(executor);

    loop {
        conf.process_state_bus();
        std::thread::sleep(Duration::from_millis(1));
    }
}
```

Note: `Sampler::with_path()` is a named constructor (not `new()`) that sets the
initial sample path. `new()` produces a sampler with no loaded sample (silent).

---

## What Ships at P3

| Component | Crate | Status |
|---|---|---|
| `trait Negotiable` | `paraclete-node-api` | Full |
| `LockableParam` | `paraclete-node-api` | Full |
| `ConnectionAgreement::lockable_params` | `paraclete-node-api` | Full |
| `ConnectionRecord` | `paraclete-node-api` | Full |
| `StateBusHandle` stable API | `paraclete-runtime` | Full |
| SPSC upgrade via `rtrb` | `paraclete-runtime` | Full — replaces RwLock |
| `NodeConfigurator::process_state_bus()` | `paraclete-runtime` | Full |
| `ParamLockEvent` priority ordering | `paraclete-runtime` | Full |
| Negotiable handshake at connect() | `paraclete-runtime` | Full |
| `EdgeMeta` port fields used | `paraclete-runtime` | `#[allow(dead_code)]` removed |
| Rhai state bus bindings | `paraclete-scripting` | Full |
| `Sampler` node — 4-voice pool | `paraclete-nodes` | Full |
| `Sampler` — filesystem WAV loading | `paraclete-nodes` | Full (`hound`) |
| `Sampler` — parameter locks | `paraclete-nodes` | Full |
| `Sampler` — slice stub | `paraclete-nodes` | Single full-sample slice |
| `Sampler` — serialize/deserialize | `paraclete-nodes` | Full |
| Signal port silence on unconnected | `paraclete-node-api` | Full |
| `build_note_on/off` moved to lib.rs | `paraclete-nodes` | Already done in audit |
| Hardcoded `for pad in 0..16` | `paraclete-runtime` | TODO P4 comment added |

---

## Tests for P3

### `paraclete-node-api`
- `ConnectionAgreement` carries `lockable_params` correctly
- `LockableParam` is `Clone + Debug`
- `ConnectionRecord` stores `partner_id` and `local_port_id`
- Unconnected modulation port returns zeroed buffer, does not panic
- Wrong port type returns appropriate panic message

### `paraclete-nodes` — `Sampler`
- `Sampler::new()` produces silent output (no sample loaded)
- `Sampler::with_path()` loads WAV at `activate()` — non-zero output on NoteOn
- `negotiate()` returns `ConnectionAgreement` with all 7 parameters as `lockable_params`
- `ParamLock` for `pitch` changes playback rate
- `ParamLock` for `volume` changes output level
- `ParamLock` for `slice` is accepted (stub — always slice 0)
- Multiple locks on one step all apply independently
- All locks restore to base values on NoteOff
- All locks restore to base values when sample ends naturally
- Lock for unknown `param_id` is silently ignored
- 4-voice pool: 5th simultaneous NoteOn steals oldest voice
- Stereo output: left and right channels reflect `pan` parameter
- `serialize()`/`deserialize()` round-trip: path, all base params, root_note
- `deserialize()` with unknown version leaves state at defaults

### `paraclete-runtime`
- SPSC: values published via `published_state()` appear in state bus within one cycle
- `StateBusHandle.read()` returns correct values
- `StateBusHandle.write()` updates store
- `StateBusSubscription.changed()` fires once on change, not on stable value
- `Negotiable` handshake invoked at `connect()` — both sides receive `ConnectionRecord`
- `ParamLockEvent` delivered before `Midi2 NoteOn` at same `sample_offset`
- `ParamLockEvent` only routed to node whose `node_id` matches

### `paraclete-scripting`
- Rhai script can `state_bus.read()` a value published by a node
- Rhai script can `state_bus.write()` to a `/node/{id}/param/` path
- Rhai script writing to `/transport/` is rejected by sandbox
- `state_bus.subscribe()` returns subscription that detects change

### Integration
- Full graph: `InternalClock → Sequencer → Sampler`
- Active sequencer steps trigger sampler — audio output
- Pitch parameter lock changes audible pitch for that step only
- Pitch restores on next step
- State bus step position updates reach Launchpad emulator LEDs

---

## Open Design Notes for P4

- **Hardcoded `for pad in 0..16`** in `build_hardware_output()` — replace with
  iteration over `HardwareDevice::surface()` descriptor at P4.
- **Linear interpolation resampler** — replace with `rubato` crate at P6 when
  audio quality matters.
- **`StateBus::published_state()` allocates per cycle** — pre-allocated push-down
  approach needed at scale. See OQ-9 in architecture-evolving.md.
- **Sample format support** — `hound` handles WAV only. Add `symphonia` for AIFF,
  FLAC, MP3 at P6.
