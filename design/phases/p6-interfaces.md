# Paraclete — P6 Interface Specification

> **Implementation blueprint.** These are the contracts P6 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
>
> **P6 deliverable:** The platform synthesizes sounds. Primitive nodes
> (oscillator, envelope, LFO, ladder filter), `AnalogEngine` and `FmEngine`
> generator nodes with machine variants, high-quality resampling and broad
> audio format support in Sampler.
> **Last updated:** June 2026
> **Depends on:** p0–p5 interfaces and reports — all prior types assumed
> present and correct.
> **References:** ADR-022 (node portability), ADR-023 (instrument
> encapsulation), roadmap.md (P6 scope)

-----

## How to use this document

**Read Part 1 before implementing anything.** It is the synthesis primitive
study output. Every node in this document is justified by that vocabulary.
The study precedes design; design precedes code.

**Implement in commit order.** Each commit is independently testable and
merges clean. The L2 API extension (Commit 1) must land before any new
node that writes modulation or logic outputs.

| Commit | Deliverable |
|--------|-------------|
| 1 | L2 API extension — `mod_output_mut`, `logic_output_mut` |
| 2 | `OscillatorNode` |
| 3 | `EnvelopeNode` |
| 4 | `LfoNode` |
| 5 | `LadderFilterNode` |
| 6 | Sampler audio quality — rubato + symphonia + envelope DSP |
| 7 | `AnalogEngine` — KickMachine, SnareMachine, HiHatMachine |
| 8 | `FmEngine` — FmKickMachine, FmBellMachine, FmBassMachine |
| 9 | P6 graph wiring |

**Portability rule applies to every node.** Every new node links only
against `paraclete-node-api`. Verify with `cargo tree -p paraclete-nodes`
before merge. See ADR-022.

**Machine variant pattern applies to all generator nodes.** `AnalogEngine`
and `FmEngine` expose machine-appropriate parameter surfaces via
`capability_document()`. Full synthesis depth is available through
`ConnectionAgreement` negotiation, not by default. See ADR-022.

**Every synthesis node is a future inner-graph primitive.** Ask: could
this node appear inside a `GraphNode` instrument's inner graph at P9?
If the answer is no, the granularity is wrong. See ADR-023.

-----

# Part 1: Synthesis Primitive Vocabulary

This section records the output of the pre-P6 synthesis primitive study.
It establishes the vocabulary all subsequent sections reference.

The study surveyed the synthesis engines and voice architectures present
in hardware step-sequencer instruments targeting the industrial/techno/
hardcore aesthetic (per `instrument-vision.md`). The goal was to extract
a minimal complete set of primitive building blocks from which any
reference voice topology can be composed without gaps.

-----

## 1.1 Oscillator Types

**Analog-modelled oscillators** — direct digital synthesis with polyBLEP
anti-aliasing on discontinuous waveforms:

| Waveform | Character | Anti-aliasing |
|----------|-----------|---------------|
| Sine | Clean, pure | None needed |
| Triangle | Warm, soft harmonics | None needed |
| Sawtooth | Bright, full harmonics | polyBLEP required |
| Square | Hollow, aggressive | polyBLEP required |
| Noise | Inharmonic, broadband | LFSR white noise |

All oscillators use a normalised phase accumulator (0.0 to just below 1.0).
Frequency modulation is additive to phase: `phase += (freq + fm_offset) / sample_rate`.

**FM operator** — a sine oscillator whose output is used to modulate the
phase (not frequency) of another oscillator. Phase modulation and frequency
modulation are equivalent for sine carriers at audio rate but phase
modulation is better-behaved with envelopes. Implemented as a pair of
oscillators (modulator, carrier) with `carrier_phase += (carrier_freq + index × mod_out) / sample_rate`.

**Self-feedback** — the modulator samples its previous output and adds a
fraction of it to its own phase input. Produces progressively distorted
timbres from sine through square-like to noise-like. Essential for
FM kick synthesis.

-----

## 1.2 Envelope Types

| Type | Shape | Trigger behaviour | Primary use |
|------|-------|------------------|-------------|
| AD | Attack → Decay → Idle | Retrigger: restart from current value | Drums |
| ADSR | Attack → Decay → Sustain → Release | Gate high = sustain, gate low = release | Melodic synths |
| Looping AD | Attack → Decay → Attack → ... | Self-retriggering; rate = attack + decay | LFO substitute |

All envelopes share one gate input. Rising edge = trigger. Falling edge
triggers Release (ADSR only). AD/looping AD ignore gate low.

**Exponential decay** is the standard for musical plausibility. Specified
as time to fall 60 dB (to 0.001 of peak). Linear attack is sufficient.

**Retrigger behaviour on AD**: a new NoteOn before the envelope reaches
idle restarts from the current envelope value (not from 0), preventing
the click that would occur from a hard reset.

-----

## 1.3 Filter Topologies

| Topology | Modes | Character |
|----------|-------|-----------|
| SVF (state-variable filter) | LP, HP, BP, Notch | Clean, musical; already in `FilterNode` |
| Ladder (4-pole Moog model) | LP only | Warm, resonant; self-oscillation at max resonance |

The SVF ships at P4 (`FilterNode`). The ladder topology is new at P6
(`LadderFilterNode`). Both operate at audio rate and accept modulation
inputs for cutoff and resonance.

**Comb filter** and **formant filter** (vowel filter) are deferred to P7.

-----

## 1.4 LFO Modes

| Waveform | Range | Notes |
|----------|-------|-------|
| Sine | -1.0..+1.0 | Smooth cyclic modulation |
| Square | -1.0..+1.0 | Stepped, hard transitions |
| Ramp up | -1.0..+1.0 | Rising sawtooth |
| Ramp down | -1.0..+1.0 | Falling sawtooth |
| Random (S&H) | -1.0..+1.0 | New random value each half-cycle |

LFOs run at audio rate (same block processing as audio nodes) for
accurate low-frequency modulation without control-rate stepping artefacts.
Tempo-sync and one-shot modes are deferred to P7.

-----

## 1.5 Machine Vocabulary

The machine variant study yields these six machine targets for P6:

| Engine | Machine | Character | Topology |
|--------|---------|-----------|----------|
| AnalogEngine | KickMachine | Punchy sub-bass kick | Sine osc + pitch env + amp env + SVF |
| AnalogEngine | SnareMachine | Cracking snare with noise | Body osc + noise src + dual env + BP filter |
| AnalogEngine | HiHatMachine | Metallic hi-hat | Noise src + HP filter + amp env |
| FmEngine | FmKickMachine | FM thump with growl | 2-op FM, pitch env, feedback |
| FmEngine | FmBellMachine | Metallic bell / pluck | 2-op FM, high ratio, long decay |
| FmEngine | FmBassMachine | Punchy FM bass | 2-op FM, short attack |

Each machine is a named configuration of its engine node. The engine's
full parameter depth is present in the implementation; the machine's
`capability_document()` exposes only the musically relevant subset.

-----

# Part 2: L2 API Extension (Commit 1)

**Crate:** `paraclete-node-api`
**File:** `src/context.rs`

The primitive nodes `EnvelopeNode` and `LfoNode` output modulation signals.
`OscillatorNode` optionally accepts a logic gate. These require two new
output accessors and one new input accessor on the process context types.

```rust
impl ProcessOutput {
    /// Write a Modulation port output. Returns a mutable slice of
    /// block_size values. Called for ports declared PortType::Modulation
    /// with direction Output. Panics in debug if port_id is not a
    /// registered Modulation output port.
    pub fn mod_output_mut(&mut self, port_id: u32) -> &mut [f32];

    /// Write a Logic port output. Returns a mutable slice of block_size
    /// values. Called for ports declared PortType::Logic with direction
    /// Output. Panics in debug if port_id is not a registered Logic
    /// output port.
    pub fn logic_output_mut(&mut self, port_id: u32) -> &mut [f32];
}
```

```rust
impl ProcessInput {
    /// Read a Logic input port as a slice of block_size values.
    /// Returns a buffer of all zeros if the port is unconnected.
    /// High state = 1.0. Low state = 0.0. Rising edge = transition
    /// from <0.5 to ≥0.5. Called for PortType::Logic input ports.
    pub fn logic(&self, port_id: u32) -> &[f32];
}
```

`input.modulation(port_id)` already exists and returns `&[f32]` for
unconnected ports. `input.logic()` follows the same pattern.

### Implementation notes

`mod_output_mut` and `logic_output_mut` are backed by pre-allocated
`Vec<f32>` buffers in the executor, mirroring the existing audio output
buffer allocation. The executor allocates one buffer per declared
non-audio output port at graph build time. No allocation in `process()`.

`logic()` for an unconnected port returns a reference to a statically
allocated zero buffer (or a per-executor zeroed scratch buffer).
The returned slice is always exactly `block_size` in length.

### Tests

```
logic_input_unconnected_returns_zeros
    ProcessInput with no logic port connected → input.logic(0) → all 0.0
mod_output_mut_returns_block_size_slice
    ProcessOutput with one mod output port → mod_output_mut(0).len() == block_size
```

-----

# Part 3: Primitive Nodes

All primitive nodes live in `paraclete-nodes`. All are portable per
ADR-022. No dependency on `paraclete-runtime`, `paraclete-scripting`, or
any hardware crate.

-----

## Part 3.1: `OscillatorNode`

A band-limited analog-modelled oscillator. Produces mono audio output
from a configurable waveform and pitch.

### Struct

```rust
pub struct OscillatorNode {
    bank:        ParameterBank,
    phase:       f32,            // 0.0..just below 1.0
    noise_state: u32,            // xorshift32 LFSR state; initialised to 1 at new()
    sample_rate: f32,
}
```

### Parameters

```rust
pub const PARAM_OSC_WAVEFORM: u32 = id_for_name("waveform");
// Range: 0.0–4.0 (stepped), default 0.0
// 0 = sine, 1 = triangle, 2 = sawtooth, 3 = square, 4 = noise

pub const PARAM_OSC_PITCH: u32 = id_for_name("pitch");
// Range: -60.0–+60.0 (semitones), default 0.0
// MIDI note 69 + pitch = A4 = 440 Hz

pub const PARAM_OSC_TUNE: u32 = id_for_name("tune");
// Range: -100.0–+100.0 (cents), default 0.0
// Additive fine-tune offset

pub const PARAM_OSC_LEVEL: u32 = id_for_name("level");
// Range: 0.0–2.0, default 1.0
```

### Ports

```
Input:  pitch_cv_in  (id 0, PortType::Modulation, optional)
        — adds to PARAM_OSC_PITCH in semitones; 0.0 if unconnected
        fm_in        (id 1, PortType::Modulation, optional)
        — FM modulation in semitones; 0.0 if unconnected
        gate_in      (id 2, PortType::Logic, optional)
        — rising edge resets phase to 0.0; 0.0 if unconnected

Output: audio_out    (id 3, PortType::Mono)
```

### `activate()`

```rust
fn activate(&mut self, sample_rate: f32, _block_size: usize) {
    self.sample_rate = sample_rate;
    self.bank = ParameterBank::from_capability_document(&self.capability_document());
    // phase and noise_state are preserved across activate/deactivate
    // to avoid discontinuities on hot-reload
}
```

### `process()`

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());

    let waveform  = self.bank.get(PARAM_OSC_WAVEFORM) as u8;  // truncate
    let base_pitch = self.bank.get(PARAM_OSC_PITCH) as f32;
    let tune_cents = self.bank.get(PARAM_OSC_TUNE) as f32;
    let level     = self.bank.get(PARAM_OSC_LEVEL) as f32;

    let pitch_cv = input.modulation(0);
    let fm_in    = input.modulation(1);
    let gate_in  = input.logic(2);
    let out      = output.audio_output_mut(3);

    let mut prev_gate = 0.0f32;
    for i in 0..out.len() {
        // Detect rising gate edge → reset phase
        if gate_in[i] >= 0.5 && prev_gate < 0.5 {
            self.phase = 0.0;
        }
        prev_gate = gate_in[i];

        let semitones = base_pitch + tune_cents / 100.0 + pitch_cv[i] + fm_in[i];
        let freq = 440.0f32 * 2.0f32.powf((semitones - 9.0) / 12.0);
        let phase_inc = (freq / self.sample_rate).clamp(0.0, 0.5);

        let sample = match waveform {
            0 => (self.phase * std::f32::consts::TAU).sin(),
            1 => {
                let p = self.phase;
                4.0 * (p - (p + 0.5).floor() + 0.5).abs() - 1.0
            }
            2 => {
                // Sawtooth with polyBLEP
                let saw = 2.0 * self.phase - 1.0;
                saw - poly_blep(self.phase, phase_inc)
            }
            3 => {
                // Square with polyBLEP
                let sq = if self.phase < 0.5 { 1.0f32 } else { -1.0 };
                let blep0 = poly_blep(self.phase, phase_inc);
                let blep1 = poly_blep((self.phase + 0.5).fract(), phase_inc);
                sq + blep0 - blep1
            }
            _ => {
                // Noise (xorshift32 LFSR)
                self.noise_state ^= self.noise_state << 13;
                self.noise_state ^= self.noise_state >> 17;
                self.noise_state ^= self.noise_state << 5;
                (self.noise_state as i32 as f32) / (i32::MAX as f32)
            }
        };

        out[i] = sample * level;
        self.phase = (self.phase + phase_inc).fract();
    }
}
```

**polyBLEP helper (private module function):**

```rust
fn poly_blep(phase: f32, phase_inc: f32) -> f32 {
    if phase < phase_inc {
        let t = phase / phase_inc;
        2.0 * t - t * t - 1.0
    } else if phase > 1.0 - phase_inc {
        let t = (phase - 1.0) / phase_inc;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}
```

### Tests

```
oscillator_sine_output_is_periodic
oscillator_sawtooth_output_is_nonzero
oscillator_square_output_is_nonzero
oscillator_noise_output_is_nonzero
oscillator_gate_rising_edge_resets_phase
oscillator_pitch_cv_shifts_frequency
oscillator_portability_check   ← cargo tree shows no runtime dependency
```

-----

## Part 3.2: `EnvelopeNode`

A triggered envelope generator. Outputs a modulation signal (0.0–1.0)
suitable for amplitude or pitch modulation. Three modes: ADSR, AD,
and looping AD.

### Struct

```rust
pub struct EnvelopeNode {
    bank:        ParameterBank,
    state:       EnvState,
    phase:       EnvPhase,
    prev_gate:   f32,
    sample_rate: f32,
}

enum EnvPhase { Idle, Attack, Decay, Sustain, Release }

struct EnvState {
    value:         f32,
    attack_inc:    f32,   // pre-computed: 1.0 / (attack_s * sample_rate)
    decay_coeff:   f32,   // pre-computed: 0.001f32.powf(1.0 / (decay_s * sample_rate))
    sustain_level: f32,   // direct from parameter
    release_coeff: f32,   // pre-computed: 0.001f32.powf(1.0 / (release_s * sample_rate))
}
```

`EnvState` is pre-allocated at `activate()` and never reallocates in
`process()`. Coefficients are recomputed when the underlying bank
parameters change — the simplest strategy is to recompute them at the
start of every `process()` call (coefficient computation is cheap
arithmetic; no allocation).

### Parameters

```rust
pub const PARAM_ENV_MODE:    u32 = id_for_name("env_mode");
// Range: 0.0–2.0 (stepped), default 0.0
// 0 = ADSR, 1 = AD, 2 = Looping AD

pub const PARAM_ENV_ATTACK:  u32 = id_for_name("attack");
// Range: 0.001–4.0 (seconds), default 0.01

pub const PARAM_ENV_DECAY:   u32 = id_for_name("decay");
// Range: 0.001–8.0 (seconds), default 0.3

pub const PARAM_ENV_SUSTAIN: u32 = id_for_name("sustain");
// Range: 0.0–1.0, default 0.7
// Only used in ADSR mode; ignored in AD/looping modes

pub const PARAM_ENV_RELEASE: u32 = id_for_name("release");
// Range: 0.001–8.0 (seconds), default 0.5
// Only used in ADSR mode; ignored in AD/looping modes
```

### Ports

```
Input:  gate_in  (id 0, PortType::Logic)
        — rising edge triggers Attack; falling edge triggers Release
          (ADSR mode only; AD and looping AD ignore falling edge)
        — unconnected: all zeros (envelope stays idle)

Output: mod_out  (id 1, PortType::Modulation)
        — range 0.0–1.0; multiply by a signal for AM or add scaled
          to pitch for pitch modulation
```

### `activate()`

```rust
fn activate(&mut self, sample_rate: f32, _block_size: usize) {
    self.sample_rate = sample_rate;
    self.bank = ParameterBank::from_capability_document(&self.capability_document());
    // Do not reset phase or value — preserve state across audio restarts.
}
```

### `process()`

State machine rules:

- **Attack:** `value += attack_inc` per sample until `value >= 1.0` → Decay
- **Decay (ADSR):** `value *= decay_coeff` until `value <= sustain_level` → Sustain
- **Decay (AD / looping):** `value *= decay_coeff` until `value < 1e-5`:
  - AD mode → Idle
  - Looping AD mode → Attack (loop without waiting for gate)
- **Sustain (ADSR):** hold at `sustain_level` until gate falls → Release
- **Release (ADSR):** `value *= release_coeff` until `value < 1e-5` → Idle
- **Retrigger:** NoteOn while not idle restarts Attack from current `value`
  (no hard reset to 0.0 — prevents click on rapid retriggering)

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());

    let mode     = self.bank.get(PARAM_ENV_MODE) as u8;
    let attack_s = self.bank.get(PARAM_ENV_ATTACK) as f32;
    let decay_s  = self.bank.get(PARAM_ENV_DECAY)  as f32;
    let sustain  = self.bank.get(PARAM_ENV_SUSTAIN) as f32;
    let release_s= self.bank.get(PARAM_ENV_RELEASE) as f32;

    // Recompute coefficients from current parameters (cheap, allocation-free)
    let sr = self.sample_rate;
    let attack_inc  = 1.0 / (attack_s * sr).max(1.0);
    let decay_coeff  = 0.001_f32.powf(1.0 / (decay_s  * sr).max(1.0));
    let sustain_lvl  = sustain;
    let release_coeff = 0.001_f32.powf(1.0 / (release_s * sr).max(1.0));

    let gate = input.logic(0);
    let out  = output.mod_output_mut(1);

    for i in 0..out.len() {
        let gate_high = gate[i] >= 0.5;
        let gate_rose = gate_high && self.prev_gate < 0.5;
        let gate_fell = !gate_high && self.prev_gate >= 0.5;
        self.prev_gate = gate[i];

        if gate_rose {
            self.phase = EnvPhase::Attack;
            // retrigger from current value — no reset
        }

        if gate_fell && mode == 0 {
            // ADSR mode: falling gate → release
            self.phase = EnvPhase::Release;
        }

        match self.phase {
            EnvPhase::Idle => {}
            EnvPhase::Attack => {
                self.state.value += attack_inc;
                if self.state.value >= 1.0 {
                    self.state.value = 1.0;
                    self.phase = EnvPhase::Decay;
                }
            }
            EnvPhase::Decay => {
                self.state.value *= decay_coeff;
                match mode {
                    0 => { // ADSR
                        if self.state.value <= sustain_lvl {
                            self.state.value = sustain_lvl;
                            self.phase = EnvPhase::Sustain;
                        }
                    }
                    1 => { // AD — idle at floor
                        if self.state.value < 1.0e-5 {
                            self.state.value = 0.0;
                            self.phase = EnvPhase::Idle;
                        }
                    }
                    _ => { // Looping AD — retrigger at floor
                        if self.state.value < 1.0e-5 {
                            self.state.value = 0.0;
                            self.phase = EnvPhase::Attack;
                        }
                    }
                }
            }
            EnvPhase::Sustain => {
                self.state.value = sustain_lvl;
            }
            EnvPhase::Release => {
                self.state.value *= release_coeff;
                if self.state.value < 1.0e-5 {
                    self.state.value = 0.0;
                    self.phase = EnvPhase::Idle;
                }
            }
        }

        out[i] = self.state.value;
    }
}
```

### Tests

```
envelope_adsr_attack_reaches_one
envelope_adsr_decay_falls_to_sustain
envelope_adsr_gate_low_triggers_release
envelope_adsr_release_reaches_zero
envelope_ad_ignores_gate_low
envelope_ad_completes_without_gate
envelope_looping_ad_restarts_after_decay
envelope_retrigger_from_nonzero_no_click
    — retrigger mid-decay: value must not drop discontinuously
```

-----

## Part 3.3: `LfoNode`

A low-frequency oscillator. Outputs a modulation signal (-1.0 to +1.0).
Runs at audio rate for smooth low-frequency modulation without stepping
artefacts.

### Struct

```rust
pub struct LfoNode {
    bank:        ParameterBank,
    phase:       f32,
    noise_state: u32,
    held_value:  f32,   // for Sample-and-Hold random mode
    sample_rate: f32,
}
```

### Parameters

```rust
pub const PARAM_LFO_WAVEFORM: u32 = id_for_name("lfo_waveform");
// Range: 0.0–4.0 (stepped), default 0.0
// 0 = sine, 1 = square, 2 = ramp up, 3 = ramp down, 4 = random (S&H)

pub const PARAM_LFO_RATE: u32 = id_for_name("lfo_rate");
// Range: 0.01–20.0 (Hz), default 1.0

pub const PARAM_LFO_DEPTH: u32 = id_for_name("lfo_depth");
// Range: 0.0–1.0, default 1.0
// Scales output: out = waveform_value × depth

pub const PARAM_LFO_PHASE_OFFSET: u32 = id_for_name("lfo_phase");
// Range: 0.0–1.0 (normalised phase offset), default 0.0
```

### Ports

```
Input:  sync_in  (id 0, PortType::Logic, optional)
        — rising edge resets phase to lfo_phase offset

Output: mod_out  (id 1, PortType::Modulation)
        — range -1.0..+1.0 (before depth scaling: -depth..+depth)
```

### `process()`

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());

    let waveform     = self.bank.get(PARAM_LFO_WAVEFORM) as u8;
    let rate_hz      = self.bank.get(PARAM_LFO_RATE)  as f32;
    let depth        = self.bank.get(PARAM_LFO_DEPTH) as f32;
    let phase_offset = self.bank.get(PARAM_LFO_PHASE_OFFSET) as f32;

    let phase_inc = rate_hz / self.sample_rate;
    let sync      = input.logic(0);
    let out       = output.mod_output_mut(1);

    let mut prev_sync = 0.0f32;
    for i in 0..out.len() {
        if sync[i] >= 0.5 && prev_sync < 0.5 {
            self.phase = phase_offset.fract();
        }
        prev_sync = sync[i];

        let p = self.phase;
        let sample = match waveform {
            0 => (p * std::f32::consts::TAU).sin(),
            1 => if p < 0.5 { 1.0f32 } else { -1.0 },
            2 => 2.0 * p - 1.0,                 // ramp up
            3 => 1.0 - 2.0 * p,                 // ramp down
            _ => {
                // S&H: new random value at each rising half-cycle
                if p < phase_inc {
                    self.noise_state ^= self.noise_state << 13;
                    self.noise_state ^= self.noise_state >> 17;
                    self.noise_state ^= self.noise_state << 5;
                    self.held_value = (self.noise_state as i32 as f32) / (i32::MAX as f32);
                }
                self.held_value
            }
        };

        out[i] = sample * depth;
        self.phase = (self.phase + phase_inc).fract();
    }
}
```

### Tests

```
lfo_sine_output_oscillates_between_minus_one_and_one
lfo_square_output_is_only_plus_minus_depth
lfo_ramp_up_rises_monotonically_within_cycle
lfo_ramp_down_falls_monotonically_within_cycle
lfo_random_holds_value_between_cycles
lfo_sync_rising_edge_resets_phase
lfo_depth_zero_produces_silence
```

-----

## Part 3.4: `LadderFilterNode`

A 4-pole Moog-style ladder low-pass filter. Warm resonance character
distinct from the SVF in `FilterNode`. Self-oscillates at resonance 1.0.

### Struct

```rust
pub struct LadderFilterNode {
    bank:   ParameterBank,
    stage:  [f32; 4],   // filter stage states; pre-allocated, zeroed at activate()
    sample_rate: f32,
}
```

### Parameters

```rust
pub const PARAM_LADDER_CUTOFF:    u32 = id_for_name("ladder_cutoff");
// Range: 20.0–18000.0 (Hz), default 4000.0

pub const PARAM_LADDER_RESONANCE: u32 = id_for_name("ladder_resonance");
// Range: 0.0–1.0, default 0.0
// 0.0 = no feedback; 1.0 = self-oscillation threshold

pub const PARAM_LADDER_DRIVE:     u32 = id_for_name("ladder_drive");
// Range: 0.0–1.0, default 0.0
// Soft-clips input before filter stages; adds saturation/warmth
```

### Ports

```
Input:  audio_in       (id 0, PortType::Mono)
        cutoff_mod     (id 1, PortType::Modulation, optional)
        — additive cutoff modulation in Hz; 0.0 if unconnected
        resonance_mod  (id 2, PortType::Modulation, optional)
        — additive resonance modulation 0.0–1.0; 0.0 if unconnected

Output: audio_out      (id 3, PortType::Mono)
```

### `process()`

The algorithm is the 1-sample-delay feedback ladder. Each pole is a
1-pole LP section; the four poles give an 80 dB/decade slope at high
frequencies.

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());

    let base_cutoff    = self.bank.get(PARAM_LADDER_CUTOFF)    as f32;
    let base_resonance = self.bank.get(PARAM_LADDER_RESONANCE) as f32;
    let drive          = self.bank.get(PARAM_LADDER_DRIVE)     as f32;

    let cutoff_mod    = input.modulation(1);
    let resonance_mod = input.modulation(2);
    let in_buf        = input.audio_inputs().get(0).copied().unwrap_or(&[]);
    let out_buf       = output.audio_output_mut(3);

    for i in 0..in_buf.len() {
        let cutoff_hz = (base_cutoff + cutoff_mod[i]).clamp(20.0, self.sample_rate * 0.49);
        let r         = (base_resonance + resonance_mod[i]).clamp(0.0, 1.0);

        // Pre-warped frequency coefficient (Euler approximation)
        let fc = (std::f32::consts::PI * cutoff_hz / self.sample_rate).sin().clamp(0.0, 0.99);

        // Input drive and soft-clip
        let drive_gain = 1.0 + drive * 4.0;
        let x = (in_buf[i] * drive_gain - 4.0 * r * self.stage[3]).tanh();

        // 4 cascaded 1-pole LP sections
        self.stage[0] += fc * (x            - self.stage[0]);
        self.stage[1] += fc * (self.stage[0] - self.stage[1]);
        self.stage[2] += fc * (self.stage[1] - self.stage[2]);
        self.stage[3] += fc * (self.stage[2] - self.stage[3]);

        out_buf[i] = self.stage[3];
    }
}
```

Note: the `tanh()` at the input provides soft-clipping which prevents
instability at high drive + high resonance. The 4-pole cascade without
nonlinearity can blow up; the tanh keeps it bounded.

### Tests

```
ladder_attenuates_above_cutoff
    — sine at 2× cutoff is attenuated by >10 dB at resonance 0.0
ladder_resonance_peak_at_cutoff
    — sine at cutoff is amplified relative to resonance 0.0 when resonance 0.8
ladder_does_not_blow_up_at_max_resonance_and_drive
    — noise input at resonance 1.0, drive 1.0: output magnitude stays finite
ladder_cutoff_mod_shifts_frequency
ladder_portability_check
```

-----

# Part 4: Generator Nodes

Generator nodes are synthesis engines. They receive `Event` input from a
`Sequencer` and produce stereo audio output. They follow the same event
interface as `Sampler` — identical wiring works.

Generator nodes are **monophonic with retrigger** at P6. A new NoteOn
immediately restarts all envelopes and oscillators at the new pitch.
This is the correct model for drum synthesis. Polyphonic voice management
is deferred to P7+ for melodic machines.

**Internal helper types** defined in a private `engine_dsp` module within
`paraclete-nodes`, shared by `AnalogEngine` and `FmEngine`:

```rust
// paraclete-nodes/src/engine_dsp.rs  (not public, not exported)

struct AdState {
    phase:         AdPhase,
    value:         f32,
}

#[derive(Clone, Copy)]
enum AdPhase { Idle, Attack, Decay }

impl AdState {
    fn new() -> Self {
        AdState { phase: AdPhase::Idle, value: 0.0 }
    }

    /// Trigger (or retrigger) the envelope.
    fn trigger(&mut self) {
        self.phase = AdPhase::Attack;
        // retrigger from current value — no reset to 0
    }

    /// Advance one sample. Returns current envelope value.
    fn tick(&mut self, attack_inc: f32, decay_coeff: f32) -> f32 {
        match self.phase {
            AdPhase::Idle => 0.0,
            AdPhase::Attack => {
                self.value += attack_inc;
                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.phase = AdPhase::Decay;
                }
                self.value
            }
            AdPhase::Decay => {
                self.value *= decay_coeff;
                if self.value < 1.0e-5 {
                    self.value = 0.0;
                    self.phase = AdPhase::Idle;
                }
                self.value
            }
        }
    }

    fn is_idle(&self) -> bool {
        matches!(self.phase, AdPhase::Idle)
    }
}

/// XOR-shift noise generator. Period = 2^32 - 1.
fn xorshift(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as i32 as f32) / (i32::MAX as f32)
}

/// Map MIDI note + semitone offset to frequency in Hz.
fn note_to_hz(note: u8, tune_semitones: f32) -> f32 {
    440.0 * 2.0f32.powf((note as f32 - 69.0 + tune_semitones) / 12.0)
}

/// Soft-clip: tanh approximation, bounded to -1.0..+1.0.
fn soft_clip(x: f32) -> f32 { x.tanh() }

/// Single 1-pole SVF LP section (reused from FilterNode internals).
/// state_low and state_band are persistent filter state.
fn svf_lp_sample(input: f32, f: f32, q: f32,
                 state_low: &mut f32, state_band: &mut f32) -> f32 {
    *state_low  += f * *state_band;
    *state_band += f * (input - *state_low - q * *state_band);
    *state_low
}
```

-----

## Part 4.1: `AnalogEngine`

An analog drum voice synthesizer. Three machine variants with distinct
parameter surfaces. All share one synthesis engine implementation; the
`machine` field selects the DSP path and `capability_document()` surface.

### Machine enum

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AnalogMachine { Kick, Snare, HiHat }
```

### Struct

```rust
pub struct AnalogEngine {
    machine:     AnalogMachine,
    bank:        ParameterBank,
    sample_rate: f32,

    // Shared oscillator state
    osc_phase:   f32,

    // Kick/Snare body envelopes
    pitch_env:   AdState,   // kick: pitch drop envelope
    amp_env:     AdState,   // kick + snare body amp
    body_env:    AdState,   // snare body decay (snap)

    // Snare-specific
    noise_env:   AdState,   // snare noise amp
    noise_state: u32,

    // Hihat-specific
    hihat_noise: u32,

    // SVF filter state (used by all machines)
    svf_low:     f32,
    svf_band:    f32,

    // Pitch set by most recent NoteOn
    current_hz:  f32,
    active:      bool,      // true when any envelope is non-idle
}
```

### Constructors

```rust
impl AnalogEngine {
    pub fn new(machine: AnalogMachine) -> Self { ... }
    pub fn kick()  -> Self { Self::new(AnalogMachine::Kick) }
    pub fn snare() -> Self { Self::new(AnalogMachine::Snare) }
    pub fn hihat() -> Self { Self::new(AnalogMachine::HiHat) }
}
```

### Parameters and `capability_document()`

All parameter IDs use `id_for_name()`. Parameters shared between machines
(e.g. `"tune"`, `"decay"`) hash to the same ID — this is intentional:
a hardware encoder mapped to `id_for_name("decay")` controls the decay
of any machine that declares it.

#### KickMachine parameters

```rust
pub const PARAM_TUNE:  u32 = id_for_name("tune");
// Range: -24.0..+24.0 (semitones), default 0.0
// Offsets the base pitch from C2 (MIDI note 36)

pub const PARAM_PUNCH: u32 = id_for_name("punch");
// Range: 0.0..1.0, default 0.7
// Pitch envelope depth. At 1.0: 24-semitone drop over ~80 ms.

pub const PARAM_DECAY: u32 = id_for_name("decay");
// Range: 0.01..2.0 (seconds), default 0.5
// Amplitude envelope decay time.

pub const PARAM_DRIVE: u32 = id_for_name("drive");
// Range: 0.0..1.0, default 0.0

pub const PARAM_TONE:  u32 = id_for_name("tone");
// Range: 200.0..8000.0 (Hz), default 4000.0
// SVF LP filter cutoff.
```

KickMachine `capability_document()` declares: `tune`, `punch`, `decay`, `drive`, `tone`.

#### SnareMachine parameters

SnareMachine declares: `tune`, `snap`, `noise`, `decay`, `tone`.

```rust
pub const PARAM_SNAP:  u32 = id_for_name("snap");
// Range: 0.005..0.3 (seconds), default 0.05
// Body oscillator decay time.

pub const PARAM_NOISE: u32 = id_for_name("noise");
// Range: 0.0..1.0, default 0.5
// Mix level of the noise component.
```

`decay` here controls the noise envelope decay time (the longer tail).
`tone` controls the bandpass filter centre frequency on the noise path.
`tune` offsets the body oscillator from C3 (MIDI note 48).

#### HiHatMachine parameters

HiHatMachine declares: `tone`, `decay`, `open`.

```rust
pub const PARAM_OPEN: u32 = id_for_name("open");
// Range: 0.0..1.0, default 0.0
// 0.0 = closed hihat (short decay); 1.0 = open hihat (8× longer decay).
// Actual decay time = decay × (1.0 + open × 7.0).
```

`tone` controls the highpass filter cutoff (noise colouration).
`decay` is the base closed decay time.
No `tune` — no pitched oscillator.

### Ports

```
Input:  events_in  (id 0, PortType::Event)
Output: audio_out_l (id 1, PortType::Mono)
        audio_out_r (id 2, PortType::Mono)
```

Stereo output mirrors the Sampler interface. Both channels carry the same
mono signal at P6. Stereo panning is a P7 concern.

### `activate()`

```rust
fn activate(&mut self, sample_rate: f32, _block_size: usize) {
    self.sample_rate = sample_rate;
    self.bank = ParameterBank::from_capability_document(&self.capability_document());
    // Reset all DSP state
    self.osc_phase  = 0.0;
    self.svf_low    = 0.0;
    self.svf_band   = 0.0;
    self.pitch_env  = AdState::new();
    self.amp_env    = AdState::new();
    self.body_env   = AdState::new();
    self.noise_env  = AdState::new();
    self.noise_state = 1;
    self.hihat_noise = 1;
    self.current_hz  = 65.41; // C2
    self.active      = false;
}
```

### `process()`

The process loop mirrors `Sampler::process()` in structure:

1. Handle parameter commands via `bank.handle_commands()`
2. Read current parameter values
3. Scan events, precompute effective params using parameter locks
4. Render per-sample DSP using the active machine's path
5. Write both output channels

**Event handling** (identical to Sampler):

```rust
for event in input.events() {
    match event.event {
        Event::Midi2(msg) => {
            if let Some(note_on) = msg.as_note_on() {
                // retrigger all envelopes
                let tune = effective_tune(note_on.note(), &self.bank);
                self.current_hz = note_to_hz(note_on.note(), tune);
                self.pitch_env.trigger();
                self.amp_env.trigger();
                self.body_env.trigger();
                self.noise_env.trigger();
                self.osc_phase = 0.0;
                self.active = true;
            }
        }
        Event::ParamLock(pl) => {
            // handled via bank precompute before DSP loop — same as Sampler
        }
        _ => {}
    }
}
```

**KickMachine per-sample DSP:**

```rust
// Pre-computed from bank (with ParamLock overrides applied):
let punch      = bank_get(PARAM_PUNCH);
let decay_s    = bank_get(PARAM_DECAY);
let drive      = bank_get(PARAM_DRIVE);
let tone_hz    = bank_get(PARAM_TONE);

// Pitch envelope: fixed 2ms attack, 80ms decay scaled by punch
let pitch_attack_inc  = 1.0 / (0.002 * sample_rate);
let pitch_decay_coeff = 0.001f32.powf(1.0 / (0.08 * sample_rate));
let amp_attack_inc    = 1.0 / (0.001 * sample_rate);
let amp_decay_coeff   = 0.001f32.powf(1.0 / (decay_s * sample_rate));

// Per sample:
let pitch_env_val = pitch_env.tick(pitch_attack_inc, pitch_decay_coeff);
let freq = current_hz * 2.0f32.powf(pitch_env_val * punch * 24.0 / 12.0);
let phase_inc = (freq / sample_rate).clamp(0.0, 0.5);
osc_phase = (osc_phase + phase_inc).fract();
let osc = (osc_phase * TAU).sin();
let amp = amp_env.tick(amp_attack_inc, amp_decay_coeff);

// SVF LP filter
let f_svf = (PI * tone_hz / sample_rate).sin().clamp(0.0, 0.99);
let out = svf_lp_sample(osc * amp, f_svf, 0.7, &mut svf_low, &mut svf_band);
let out = soft_clip(out * (1.0 + drive * 9.0));
```

**SnareMachine per-sample DSP:**

```rust
// Body: sine oscillator at current_hz (base = C3), snap decay
let snap_s   = bank_get(PARAM_SNAP);
let noise_lvl= bank_get(PARAM_NOISE);
let decay_s  = bank_get(PARAM_DECAY);
let tone_hz  = bank_get(PARAM_TONE);

let body_attack_inc  = 1.0 / (0.001 * sample_rate);
let body_decay_coeff = 0.001f32.powf(1.0 / (snap_s * sample_rate));
let noise_attack_inc = 1.0 / (0.001 * sample_rate);
let noise_decay_coeff= 0.001f32.powf(1.0 / (decay_s * sample_rate));

// Body oscillator
let body_amp = body_env.tick(body_attack_inc, body_decay_coeff);
let freq = current_hz;
osc_phase = (osc_phase + freq / sample_rate).fract();
let body = (osc_phase * TAU).sin() * body_amp;

// Noise through bandpass SVF
let noise_raw = xorshift(&mut noise_state);
let noise_amp = noise_env.tick(noise_attack_inc, noise_decay_coeff);
// BP filter: run SVF in bandpass mode (use band output)
let f_svf = (PI * tone_hz / sample_rate).sin().clamp(0.0, 0.99);
let q_bp  = 1.0;
svf_low  += f_svf * svf_band;
svf_band += f_svf * (noise_raw - svf_low - q_bp * svf_band);
let noise_out = svf_band * noise_amp * noise_lvl;

let out = soft_clip(body + noise_out);
```

**HiHatMachine per-sample DSP:**

```rust
let tone_hz   = bank_get(PARAM_TONE);
let decay_s   = bank_get(PARAM_DECAY);
let open      = bank_get(PARAM_OPEN);

let effective_decay = decay_s * (1.0 + open * 7.0);
let amp_attack_inc  = 1.0 / (0.0005 * sample_rate);
let amp_decay_coeff = 0.001f32.powf(1.0 / (effective_decay * sample_rate));

// Noise through HP filter (SVF HP mode = input - LP output)
let noise_raw = xorshift(&mut hihat_noise);
let f_svf = (PI * tone_hz / sample_rate).sin().clamp(0.0, 0.99);
svf_low  += f_svf * svf_band;
svf_band += f_svf * (noise_raw - svf_low - 0.5 * svf_band);
let hp_out = noise_raw - svf_low;

let amp = amp_env.tick(amp_attack_inc, amp_decay_coeff);
let out = hp_out * amp;
```

**Note:** `effective_decay` in HiHatMachine depends on `open`, which may
change per-step via ParamLock. Recompute `amp_decay_coeff` each process
block from the current effective bank values (same as other coefficients).

### Tests

```
analog_kick_produces_audio_on_note_on
analog_kick_is_silent_before_any_note_on
analog_kick_punch_zero_has_no_pitch_drop
analog_snare_body_and_noise_both_present
analog_snare_noise_zero_silences_noise_path
analog_hihat_open_extends_decay
analog_hihat_closed_short_decay
analog_bump_param_decay_changes_output_length
analog_param_lock_drive_overrides_base_drive
analog_portability_check
```

-----

## Part 4.2: `FmEngine`

A two-operator FM synthesizer (phase modulation variant). Three machine
variants. Topology: a modulator operator whose output scales the phase
increment of the carrier operator. Modulator has optional self-feedback.

```
modulator ──(feedback)──┐
    ↑                   ↓
    └──── index × mod_env × modulator_out ──→ [+] carrier_phase
                                              ↓
                                          carrier_out × amp_env → output
```

### Machine enum

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FmMachine { Kick, Bell, Bass }
```

### Struct

```rust
pub struct FmEngine {
    machine:     FmMachine,
    bank:        ParameterBank,
    sample_rate: f32,

    // Oscillator state
    carrier_phase:    f32,
    modulator_phase:  f32,
    prev_mod_out:     f32,   // modulator self-feedback one-sample delay

    // Envelopes
    pitch_env:  AdState,   // kick only: pitch drop
    mod_env:    AdState,   // modulator amplitude (punch character)
    amp_env:    AdState,   // carrier amplitude (note duration)

    current_hz: f32,
    active:     bool,
}
```

### Parameters and `capability_document()`

#### FmKickMachine declares: `tune`, `punch`, `decay`, `feedback`, `drive`

```rust
// tune:     -24.0..+24.0 semitones, default 0.0
// punch:    0.0..1.0, default 0.7 — pitch env depth AND mod env decay
// decay:    0.01..2.0 s, default 0.5 — carrier amp decay
// feedback: 0.0..1.0, default 0.2 — modulator self-feedback
// drive:    0.0..1.0, default 0.0 — output soft-clip gain
```

#### FmBellMachine declares: `tune`, `ratio`, `index`, `decay`, `feedback`

```rust
// tune:     -24.0..+24.0 semitones, default 0.0
// ratio:    0.5..8.0, default 3.5 — modulator/carrier freq ratio
//           Non-integer ratios produce inharmonic, metallic spectra
// index:    0.0..8.0, default 2.0 — modulation depth
// decay:    0.05..8.0 s, default 2.0 — long bell sustain
// feedback: 0.0..0.5, default 0.1
```

#### FmBassMachine declares: `tune`, `ratio`, `index`, `attack`, `decay`, `drive`

```rust
// tune:     -24.0..+24.0 semitones, default 0.0
// ratio:    0.5..4.0, default 1.0 — integer ratios = harmonic bass
// index:    0.0..8.0, default 2.0
// attack:   0.001..0.5 s, default 0.01
// decay:    0.05..4.0 s, default 0.5
// drive:    0.0..1.0, default 0.0
```

### Ports

Identical to `AnalogEngine`:

```
Input:  events_in   (id 0, PortType::Event)
Output: audio_out_l (id 1, PortType::Mono)
        audio_out_r (id 2, PortType::Mono)
```

### `activate()`

```rust
fn activate(&mut self, sample_rate: f32, _block_size: usize) {
    self.sample_rate = sample_rate;
    self.bank = ParameterBank::from_capability_document(&self.capability_document());
    self.carrier_phase    = 0.0;
    self.modulator_phase  = 0.0;
    self.prev_mod_out     = 0.0;
    self.pitch_env  = AdState::new();
    self.mod_env    = AdState::new();
    self.amp_env    = AdState::new();
    self.current_hz = 65.41; // C2
    self.active     = false;
}
```

### `process()` — FM per-sample DSP

**Event handling** — identical to `AnalogEngine`. NoteOn triggers all
three envelopes and resets phases.

**FmKickMachine per-sample DSP:**

```rust
// punch drives both the pitch envelope depth and the mod envelope speed
let punch    = bank_get(PARAM_PUNCH);
let decay_s  = bank_get(PARAM_DECAY);
let feedback = bank_get(PARAM_FEEDBACK);
let drive    = bank_get(PARAM_DRIVE);

let pitch_attack_inc  = 1.0 / (0.002 * sample_rate);
let pitch_decay_coeff = 0.001f32.powf(1.0 / ((0.05 + punch * 0.1) * sample_rate));
let mod_attack_inc    = 1.0 / (0.001 * sample_rate);
let mod_decay_coeff   = 0.001f32.powf(1.0 / ((0.02 + punch * 0.05) * sample_rate));
let amp_attack_inc    = 1.0 / (0.001 * sample_rate);
let amp_decay_coeff   = 0.001f32.powf(1.0 / (decay_s * sample_rate));

let pitch_env_val = pitch_env.tick(pitch_attack_inc, pitch_decay_coeff);
let carrier_hz    = current_hz * 2.0f32.powf(pitch_env_val * punch * 24.0 / 12.0);

// Modulator: ratio = 1.0 for kick (operator in unison with carrier)
let mod_hz        = carrier_hz;
let mod_env_val   = mod_env.tick(mod_attack_inc, mod_decay_coeff);
let mod_out = {
    let mod_phase_inc = mod_hz / sample_rate;
    modulator_phase   = (modulator_phase + mod_phase_inc).fract();
    let fb_term = feedback * prev_mod_out;
    let m = (modulator_phase * TAU + fb_term * TAU).sin();
    prev_mod_out = m;
    m * mod_env_val
};

// Carrier
let carrier_inc = carrier_hz / sample_rate;
let index = 2.0 + punch * 4.0;  // deeper index at higher punch
carrier_phase = (carrier_phase + carrier_inc + index * mod_out / TAU).fract();
let carrier_out = (carrier_phase * TAU).sin();
let amp = amp_env.tick(amp_attack_inc, amp_decay_coeff);
let out = soft_clip(carrier_out * amp * (1.0 + drive * 9.0));
```

**FmBellMachine per-sample DSP:**

```rust
let ratio    = bank_get(PARAM_RATIO);
let index    = bank_get(PARAM_INDEX);
let decay_s  = bank_get(PARAM_DECAY);
let feedback = bank_get(PARAM_FEEDBACK);

// Bell: no pitch envelope; long amp decay; mod env same duration as amp env
let decay_coeff = 0.001f32.powf(1.0 / (decay_s * sample_rate));
let attack_inc  = 1.0 / (0.001 * sample_rate);

let mod_env_val = mod_env.tick(attack_inc, decay_coeff);
let amp_val     = amp_env.tick(attack_inc, decay_coeff);

let mod_hz       = current_hz * ratio;
let mod_phase_inc = mod_hz / sample_rate;
modulator_phase  = (modulator_phase + mod_phase_inc).fract();
let fb_term = feedback * prev_mod_out;
let mod_out = (modulator_phase * TAU + fb_term * TAU).sin();
prev_mod_out = mod_out;

let carrier_inc = current_hz / sample_rate;
carrier_phase   = (carrier_phase + carrier_inc + index * mod_out * mod_env_val / TAU).fract();
let out = (carrier_phase * TAU).sin() * amp_val;
```

**FmBassMachine per-sample DSP:**

```rust
let ratio    = bank_get(PARAM_RATIO);
let index    = bank_get(PARAM_INDEX);
let attack_s = bank_get(PARAM_ATTACK);
let decay_s  = bank_get(PARAM_DECAY);
let drive    = bank_get(PARAM_DRIVE);

let attack_inc  = 1.0 / (attack_s * sample_rate).max(1.0);
let decay_coeff = 0.001f32.powf(1.0 / (decay_s * sample_rate).max(1.0));

// Mod env: faster than amp env (controls harmonic character at note start)
let mod_decay_coeff = 0.001f32.powf(1.0 / ((decay_s * 0.3) * sample_rate).max(1.0));
let mod_env_val = mod_env.tick(attack_inc, mod_decay_coeff);
let amp_val     = amp_env.tick(attack_inc, decay_coeff);

let mod_hz = current_hz * ratio;
modulator_phase = (modulator_phase + mod_hz / sample_rate).fract();
let mod_out = (modulator_phase * TAU).sin() * mod_env_val;

carrier_phase = (carrier_phase + current_hz / sample_rate
                 + index * mod_out / TAU).fract();
let out = soft_clip((carrier_phase * TAU).sin() * amp_val * (1.0 + drive * 9.0));
```

### Tests

```
fm_kick_produces_audio_on_note_on
fm_kick_feedback_zero_vs_nonzero_timbre_differs
fm_bell_decays_longer_than_kick_at_same_decay_param
fm_bell_noninteger_ratio_differs_from_integer_ratio
fm_bass_attack_param_changes_onset_slope
fm_bass_drive_increases_rms_output
fm_param_lock_ratio_overrides_base_ratio
fm_portability_check
```

-----

# Part 5: Sampler Audio Quality (Commit 6)

Three improvements ship together in one commit: rubato resampling, symphonia
format loading, and activation of the `attack`/`release` envelope stub.

-----

## Part 5.1: High-Quality Resampling (rubato)

**Problem:** Sampler uses linear interpolation for pitch-shifted playback.
Linear interpolation has severe high-frequency distortion at large
pitch ratios (notably +/-12 semitones = 2× or 0.5× playback rate).

**Fix:** Replace the linear interpolation loop with a rubato
`SincFixedIn` resampler per voice. The rubato resampler is initialised
at `activate()` time — no allocation in `process()`.

### Cargo.toml additions

```toml
[workspace.dependencies]
rubato = "0.15"

[paraclete-nodes dependencies]
rubato = { workspace = true }
```

### Sampler changes

```rust
use rubato::{Resampler, SincFixedIn, SincInterpolationType, SincInterpolationParameters, WindowFunction};

// Voice struct gains:
struct SamplerVoice {
    // existing fields unchanged
    resampler: Option<SincFixedIn<f32>>,
    resample_buf_in:  Vec<Vec<f32>>,   // pre-allocated at activate()
    resample_buf_out: Vec<Vec<f32>>,   // pre-allocated at activate()
}
```

The resampler is constructed at note-on time with the current pitch ratio
(`sample_rate / (playback_rate × voice_sample_rate)`). Construction of
`SincFixedIn` allocates — it must happen in `process()` but only at
note-on events (rare path, acceptable per ADR-005 note on "bounded
allocation on event").

If `rubato` construction during `process()` is unacceptable, an
alternative strategy is to pre-construct resamplers at voice
pre-allocation time (in `activate()`) for a set of discrete pitch values.
The implementer chooses. Either approach must be documented in the P6
report.

**Sinc parameters (recommended defaults):**

```rust
SincInterpolationParameters {
    sinc_len: 256,
    f_cutoff: 0.95,
    interpolation: SincInterpolationType::Linear,
    oversampling_factor: 256,
    window: WindowFunction::BlackmanHarris2,
}
```

These are the rubato "quality" defaults. They can be tuned for
CPU budget. Document chosen parameters in the P6 report.

-----

## Part 5.2: Broad Audio Format Support (symphonia)

**Problem:** Sampler loads WAV files only (via hound). FLAC and AIFF are
common sample formats; OGG and MP3 support is expected.

**Fix:** Replace `hound` with `symphonia` for all audio file decoding.
`hound` dependency is removed from `paraclete-nodes` and workspace.

### Cargo.toml changes

```toml
# Remove: hound = ...

# Add:
[workspace.dependencies]
symphonia = { version = "0.5", features = ["all"] }

[paraclete-nodes dependencies]
symphonia = { workspace = true }
```

### Load path changes

The sample loading function (`load_sample_from_path` or equivalent) is
rewritten to use the symphonia probe/demux/decode pipeline:

```rust
fn load_sample(path: &std::path::Path) -> Result<SampleData, SamplerError> {
    let src = std::fs::File::open(path)?;
    let mss = symphonia::core::io::MediaSourceStream::new(Box::new(src), Default::default());
    let hint = symphonia::core::probe::Hint::new();  // symphonia probes by magic bytes
    let probed = symphonia::default::get_probe().format(&hint, mss, &Default::default(), &Default::default())?;
    let mut reader = probed.format;
    let track = reader.default_track().ok_or(SamplerError::NoTrack)?;
    let codec_params = track.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs().make(&codec_params, &Default::default())?;

    let mut samples: Vec<f32> = Vec::new();
    let mut channels = 1usize;

    loop {
        match reader.next_packet() {
            Ok(packet) => {
                let decoded = decoder.decode(&packet)?;
                // Convert to f32 planar — use symphonia's SampleBuffer
                let spec = *decoded.spec();
                channels = spec.channels.count();
                let mut sample_buf = symphonia::core::audio::SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sample_buf.copy_interleaved_ref(decoded);
                samples.extend_from_slice(sample_buf.samples());
            }
            Err(symphonia::core::errors::Error::IoError(_)) => break,  // end of stream
            Err(e) => return Err(SamplerError::Decode(e)),
        }
    }

    // Deinterleave if stereo — Sampler works in planar mono internally
    Ok(SampleData { samples, channels, sample_rate: codec_params.sample_rate.unwrap_or(44100) })
}
```

Supported formats (via `features = ["all"]`): WAV, AIFF, FLAC, OGG
Vorbis, MP3. The existing test WAV files continue to load unchanged.

**Error handling:** `SamplerError` gains `Decode(symphonia::core::errors::Error)`.
File-not-found and decode errors produce an idle voice with zero output
(same as before). The error is logged to stderr in debug builds.

-----

## Part 5.3: Sampler Envelope DSP Activation

**Problem:** The `attack` and `release` parameters have been in
`Sampler`'s `ParameterBank` since P4.5 but no envelope DSP exists.
A non-zero `attack` has no audible effect. See p5-report.md.

**Fix:** Add per-voice AD/ADSR envelope state to `SamplerVoice` and
apply it in the render loop.

### `SamplerVoice` additions

```rust
struct SamplerVoice {
    // existing fields unchanged
    env_value:   f32,
    env_phase:   EnvPhaseSimple,
}

#[derive(Clone, Copy)]
enum EnvPhaseSimple { Attack, Release, Done }
```

The Sampler uses a simplified 2-stage model (Attack + Release) because:
- Decay and Sustain don't apply to one-shot sample playback
- Attack shapes the onset; Release shapes the tail

`EnvPhaseSimple` is a Sampler-local enum (not `EnvPhase` from
`EnvelopeNode`). No shared type needed — they share no crate boundary.

### Render loop changes

```rust
// In SamplerVoice::render_sample() (or the per-sample render path):
let attack_inc    = 1.0 / (attack_s * sample_rate).max(1.0);
let release_coeff = 0.001_f32.powf(1.0 / (release_s * sample_rate).max(1.0));

let env = match self.env_phase {
    EnvPhaseSimple::Attack => {
        self.env_value += attack_inc;
        if self.env_value >= 1.0 {
            self.env_value = 1.0;
            self.env_phase = EnvPhaseSimple::Release;  // one-shot: go straight to release
        }
        self.env_value
    }
    EnvPhaseSimple::Release => {
        self.env_value *= release_coeff;
        if self.env_value < 1.0e-5 {
            self.env_value = 0.0;
            self.env_phase = EnvPhaseSimple::Done;
        }
        self.env_value
    }
    EnvPhaseSimple::Done => {
        self.active = false;
        0.0
    }
};

output_sample = resampled_sample * volume * env;
```

**NoteOn** resets `env_value = 0.0` and `env_phase = Attack`.

**Default attack** = 0.005 s (already declared in capability_document).
At this default, the attack is inaudible on most samples — backwards
compatible with all existing P5 behaviour.

**Done criterion:** `attack = 0.1` produces an audible slow onset on a
sustained sample (drum sample with attack tail at 100ms). `release = 0.01`
produces a click-free short tail.

### Tests

```
sampler_attack_nonzero_delays_onset
sampler_release_nonzero_extends_tail_vs_zero
sampler_default_attack_0005_is_audibly_instantaneous
sampler_flac_loads_and_plays       ← requires a test FLAC fixture
sampler_rubato_pitch_up_12_semitones_produces_audio
```

-----

# Part 6: P6 Graph (Commit 9)

The P6 graph replaces the Sampler-based drum tracks 0–2 with synthesis
engines. Tracks 3–7 remain Sampler-based but use the audio quality
improvements from Commit 6.

```
InternalClock
  ├──→ Sequencer[0] (Kick)   → AnalogEngine::kick()  → Distortion[0] → Filter[0] ──┐
  ├──→ Sequencer[1] (Snare)  → AnalogEngine::snare() → Distortion[1] → Filter[1] ──┤
  ├──→ Sequencer[2] (HiHat)  → AnalogEngine::hihat() → Distortion[2] → Filter[2] ──┤
  ├──→ Sequencer[3]          → Sampler[3]            → Distortion[3] → Filter[3] ──┤
  ├──→ Sequencer[4]          → Sampler[4]            → Distortion[4] → Filter[4] ──┼──→ MixNode → ReverbNode → AudioOutput
  ├──→ Sequencer[5]          → Sampler[5]            → Distortion[5] → Filter[5] ──┤
  ├──→ Sequencer[6]          → Sampler[6]            → Distortion[6] → Filter[6] ──┤
  └──→ Sequencer[7] (Bass)   → FmEngine::bass()      → Distortion[7] → Filter[7] ──┘

LaunchpadNode  ──→ ScriptingGatewayNode[LP]
DigitaktNode   ──→ ScriptingGatewayNode[DT]
KeystepNode    ──→ ScriptingGatewayNode[KS]
```

**Track 7 (Bass):** FmEngine with FmBassMachine. Keystep drives
melodic input. The Keystep's NoteOn events arrive as MIDI2 events
via `KeystepNode` → `HardwareMappingNode` → Sequencer[7]
(same wiring as P4/P5).

**Alternative topology for validation:** For easier integration testing,
the graph can initially keep all 8 tracks as Sampler-based and add
one `AnalogEngine::kick()` as a 9th track feeding the MixNode directly.
This validates the AnalogEngine wiring without disrupting the existing
drum machine. The swap to the diagram above comes after the single-track
test passes.

### Profile script changes

The launchpad profile requires no changes — it already sends
`CMD_BUMP_PARAM` with param IDs from `id_for_name("decay")` etc. These
IDs match the AnalogEngine parameter declarations by design.

One optional change: the profile script can display different LED colours
for synthesis tracks vs sampler tracks to give visual feedback about
which engine is active. This is a profile-level concern, not a node
concern.

-----

# P6 Done Criteria

All criteria must pass before P6 is marked complete.

## Functionality

- `AnalogEngine::kick()` produces non-silent audio on NoteOn with default parameters
- `AnalogEngine::snare()` produces non-silent audio with audible body + noise mix
- `AnalogEngine::hihat()` with `open = 0.0` decays faster than with `open = 1.0`
- `FmEngine::kick()` produces non-silent audio; timbre differs visibly from
  `AnalogEngine::kick()` (different spectral character)
- `FmEngine::bell()` has a long audible decay at default parameters (≥ 1.5 s)
- `FmEngine::bass()` responds to Keystep melodic input: different MIDI notes
  produce different pitches
- `CMD_BUMP_PARAM` on `id_for_name("decay")` audibly changes decay time on
  all machines that declare `decay`
- Parameter locks on `drive` produce a louder/more saturated hit on the
  locked step
- Sampler loads a FLAC file without error
- Sampler with `attack = 0.1` has an audibly slower onset than `attack = 0.005`

## Synthesis Primitive Nodes

- `OscillatorNode` outputs audio for all 5 waveform settings
- `EnvelopeNode` in ADSR mode transitions correctly through all four stages
- `EnvelopeNode` in looping AD mode runs continuously without a gate
- `LfoNode` output oscillates within `[-depth, +depth]` range
- `LadderFilterNode` attenuates a sine wave at 2× cutoff by more than 10 dB

## Architecture

- All new nodes pass portability check:
  `cargo tree -p paraclete-nodes` shows no dependency on
  `paraclete-runtime`, `paraclete-scripting`, or any L0 crate
- Machine variant pattern verified: `AnalogEngine::kick().capability_document()`
  declares exactly `tune`, `punch`, `decay`, `drive`, `tone` — no more
- `FmEngine::bell().capability_document()` declares exactly `tune`, `ratio`,
  `index`, `decay`, `feedback` — no more

## Tests

| State | Count |
|-------|-------|
| P5 ship | 232 |
| Commit 1 (L2 extension) | +2 |
| Commit 2 (OscillatorNode) | +7 |
| Commit 3 (EnvelopeNode) | +8 |
| Commit 4 (LfoNode) | +7 |
| Commit 5 (LadderFilterNode) | +4 |
| Commit 6 (Sampler quality) | +5 |
| Commit 7 (AnalogEngine) | +10 |
| Commit 8 (FmEngine) | +8 |
| **P6 target** | **≥ 283** |

All 232 existing tests continue to pass unchanged through every commit.

-----

## P6 Known Gaps and Deferred Items

**OQ-9 (published_state() allocation):** With 8 synthesis nodes publishing
state each cycle, the per-call allocation in `published_state()` is more
significant than at P3 scale. Addressing this requires pre-allocated
push-down in the state bus publisher — a runtime change, not a node change.
Deferred to P7 when the full node count is known.

**CMD_SET_PATTERN (Sequencer):** The Sequencer always plays pattern 0.
`active_pattern` is stored but unused. Deferred; not blocking P6 synthesis
work.

**Signed micro-timing:** Negative `micro_offset` pushes events later
(same as positive). Full signed micro-timing requires splitting emission
across the step boundary. Deferred from P5; still deferred.

**AnalogEngine polyphony:** FmBassMachine is monophonic with retrigger.
A melodic bass line with overlapping notes (legato) will retrigger.
Polyphonic voice management deferred to P7.

**LFO tempo sync:** `LfoNode` runs free at all times. Beat-synced LFO
rates require `TransportInfo` subscription. Deferred to P7.

**Ladder filter HP/BP modes:** `LadderFilterNode` provides LP only.
HP and BP modes are derivable from the ladder state but are not commonly
found in analog hardware; deferred.

**FmEngine 4-operator algorithms:** Two-operator FM covers kick, bell, and
bass adequately. Four-operator algorithms (enabling the full Digitone
voice topology) require richer algorithm routing. Deferred to P7.
