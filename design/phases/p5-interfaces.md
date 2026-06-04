# Paraclete — P4.5 + P5 Interface Specification

> **Implementation blueprint.** These are the contracts P4.5 and P5 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
> 
> **P4.5 deliverable:** Four blocking gaps closed. Integration test passes.
> **P5 deliverable:** Conditional trigs, micro-timing, swing, reverb, delay, split,
> complete audio routing.
> **Last updated:** June 2026
> **Depends on:** p0–p4 interfaces and reports — all prior types assumed present
> and correct.
> **References:** ADR-018 (cellular architecture), ADR-019 (universal parameter
> control), ADR-020 (Launchpad profile), ADR-022 (node portability),
> ADR-023 (instrument encapsulation)

-----

## How to use this document

**Implement P4.5 first.** Work through the four fixes in order. Run the integration
test. All four must pass before P5 implementation begins. P5 depends on correct audio
routing and correct device tagging — building on broken foundations produces
unreliable results.

**Then implement P5.** The P5 section is a complete interface spec assuming P4.5 is
green. The Sequencer v2 section builds on the existing Sequencer — it is an extension,
not a rewrite.

-----

# PART ONE: P4.5 — Foundation Verification

## Overview

Four bugs were identified in the P4 report and code review. None are design questions.
All are implementation gaps with fully specified fixes. The integration test at the
end of this section is the done criterion for P4.5.

-----

## Fix 1 — LED Routing (P4 gap — unnamed)

**Problem:** `ScriptingEngine::take_pending_output()` returns a
`HashMap<u32, HardwareOutput>` (device_id → output), but the main loop never routes
this to the stored `HardwareOutputHandle` instances. The hardware grid is dark.

**Root cause:** The P4 main loop pseudocode left a placeholder comment:

```rust
// routing detail resolved during implementation
let _ = pending;
```

This was not resolved.

**Fix:** `NodeConfigurator` gains a device_id → handle index map built at registration
time. `deliver_script_output()` looks up each device_id in the pending map and pushes
the `HardwareOutput` into that handle’s internal SPSC.

### `NodeConfigurator` additions

```rust
impl NodeConfigurator {
    /// Route script LED output to the correct HardwareOutputHandle.
    /// Called by the app layer after scripting.take_pending_output().
    /// Matches device_id keys to registered handles.
    pub fn deliver_script_output(&mut self, pending: HashMap<u32, HardwareOutput>);
}
```

Internal implementation:

```rust
// Added at registration time in add_hardware_device():
device_id_to_handle_index: HashMap<u32, usize>

// deliver_script_output():
for (device_id, output) in pending {
    if let Some(&idx) = self.device_id_to_handle_index.get(&device_id) {
        self.output_handles[idx].push_output(output);
    }
    // unknown device_id: silently drop (device was removed or never registered)
}
```

### `HardwareOutputHandle` addition

```rust
pub trait HardwareOutputHandle: Send {
    fn tick(&mut self);

    /// Push a HardwareOutput into the handle's internal SPSC for delivery
    /// at next tick(). Called from the main thread by deliver_script_output().
    fn push_output(&mut self, output: HardwareOutput);
}
```

### `LaunchpadOutputHandle` update

`push_output()` pushes to the existing `rtrb::Producer<HardwareOutput>`. `tick()` is
unchanged — it drains the consumer as before.

### Main loop update

```rust
// Step 6 — was a placeholder. Now:
let pending = scripting.take_pending_output();
conf.deliver_script_output(pending);
// tick() is called inside conf.process_main_thread() in step 1 — no change needed
```

### Done criterion

`set_led()` calls in a profile script produce visible LED changes on the Launchpad X
within one main loop iteration (~1 ms). The hardware grid responds to script output.

-----

## Fix 2 — Device ID Tagging (C-004)

**Problem:** `ScriptingGatewayNode` fans all device events into one node. Events are
tagged with `device_ids[0]` regardless of which input port they arrived on. Profile
scripts cannot distinguish which physical device sent an event when multiple devices
share one gateway.

**Architectural fix (ADR-018):** One `ScriptingGatewayNode` per device. Each gateway
is wired to exactly one hardware node. Device ID is unambiguous — it is the single
source device’s ID.

### Graph wiring change

```rust
// Before (P4 — broken):
let (gateway, script_consumer) = ScriptingGatewayNode::new(1024);
let gateway_id = conf.add_node(Box::new(gateway));
conf.connect(launchpad_id, 0, gateway_id, 0).unwrap();
conf.connect(digitakt_id,  0, gateway_id, 1).unwrap();
conf.connect(keystep_id,   0, gateway_id, 2).unwrap();

// After (P4.5 — correct):
let (lp_gateway,  lp_consumer)  = ScriptingGatewayNode::new(1024);
let (dt_gateway,  dt_consumer)  = ScriptingGatewayNode::new(1024);
let (ks_gateway,  ks_consumer)  = ScriptingGatewayNode::new(256);

let lp_gateway_id = conf.add_node(Box::new(lp_gateway));
let dt_gateway_id = conf.add_node(Box::new(dt_gateway));
let ks_gateway_id = conf.add_node(Box::new(ks_gateway));

conf.connect(launchpad_id, 0, lp_gateway_id, 0).unwrap();
conf.connect(digitakt_id,  0, dt_gateway_id, 0).unwrap();
conf.connect(keystep_id,   0, ks_gateway_id, 0).unwrap();
```

### Main loop update

Three consumers are drained independently. The drain loop is otherwise unchanged.

```rust
// In main loop step 2:
lp_consumer.drain(&mut event_buf);
dt_consumer.drain(&mut event_buf);
ks_consumer.drain(&mut event_buf);
// All events land in the same event_buf — dispatch is unchanged.
// device_id on each HardwareEventMsg is now correct.
```

### `ScriptingGatewayNode` simplification

With one source per gateway, `device_ids` is always a single-element vec. The
`process()` implementation simplifies: all incoming events on port 0 are tagged with
`device_ids[0]`. The multi-port fan-in logic can be retained for completeness but is
no longer exercised.

### Done criterion

A profile script that calls `on_hw_event(LP_DEVICE_ID, ...)` receives only Launchpad
events. A handler for `DT_DEVICE_ID` receives only Digitakt events. Pressing a
Launchpad pad does not fire the Digitakt handler.

-----

## Fix 3 — Sampler ParameterBank (P4 gap)

**Problem:** `Sampler` does not use `ParameterBank`. `CMD_BUMP_PARAM` sent from a
Digitakt encoder is a silent no-op. Hardware encoders cannot shape drum sounds.

**Fix:** Add `ParameterBank` to `Sampler`. Declare all tuneable parameters in
`capability_document()`. Call `bank.handle_commands(input.commands())` at the top of
`process()` before any DSP logic.

### `Sampler` additions

```rust
pub struct Sampler {
    // existing fields unchanged
    bank: ParameterBank,   // new
}
```

### Sampler parameter declarations

All parameter IDs use `id_for_name()` content-addressed hashing (same as FilterNode
and DistortionNode). These are the parameters exposed to hardware and automation.

```rust
// In Sampler::capability_document() — declare these parameters:
pub const PARAM_PITCH:    u32 = id_for_name("pitch");      // -24.0–+24.0 semitones, default 0.0
pub const PARAM_VOLUME:   u32 = id_for_name("volume");     // 0.0–1.0, default 1.0
pub const PARAM_PAN:      u32 = id_for_name("pan");        // -1.0–+1.0, default 0.0
pub const PARAM_START:    u32 = id_for_name("start");      // 0.0–1.0 (normalised), default 0.0
pub const PARAM_END:      u32 = id_for_name("end");        // 0.0–1.0 (normalised), default 1.0
pub const PARAM_ATTACK:   u32 = id_for_name("attack");     // 0.0–1.0 (seconds), default 0.005
pub const PARAM_RELEASE:  u32 = id_for_name("release");    // 0.0–4.0 (seconds), default 0.1
```

### `process()` update

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());  // NEW — first line, before all else

    // existing: clear node_locks, process events, render voices
    // existing parameter reads (pitch, volume, etc.) now read from self.bank.get()
    // instead of from previously hardcoded defaults or separate fields
}
```

### `activate()` update

```rust
fn activate(&mut self, sample_rate: f32, block_size: usize) {
    self.bank = ParameterBank::from_capability_document(&self.capability_document());
    // existing: load sample, pre-allocate voices, etc.
}
```

### Parameter lock interaction

Parameter locks (existing `Event::ParamLock`) and `CMD_BUMP_PARAM` (new) both write
to the same underlying `ParameterBank` storage. `ParameterBank` is the reconciliation
point: param locks apply per-step overrides for that step’s duration; `CMD_BUMP_PARAM`
changes the base value persistently. This is the intended model per ADR-019.

The existing `is_known_param()` allowlist (P3-C1 fix) determines which parameters are
lockable. The `ParameterBank` declaration determines which parameters are reachable
from hardware. They overlap but are not required to be identical — some parameters may
be lockable but not hardware-reachable, or hardware-reachable but not lockable.

### Done criterion

A Digitakt encoder sending `CMD_BUMP_PARAM` with `param_id = PARAM_PITCH` audibly
shifts the pitch of the Sampler’s next triggered note.

-----

## Fix 4 — ProcessInput::audio_inputs Wiring (P2-C3)

**Problem:** `ProcessInput::audio_inputs` is always `&[]` in the executor. Audio
effect nodes (FilterNode, DistortionNode, MixNode) receive audio via a separate
buffer-passing mechanism, but the declared API is never populated. Every audio
node currently operates against an empty slice — a hidden contract violation that
will produce incorrect behaviour as soon as any node actually reads from it.

**Fix:** The executor populates `audio_inputs` from connected source node output
buffers before calling each node’s `process()`.

### Executor buffer layout

Audio buffers are pre-allocated at `build_executor()` time, one per audio port per
node slot. They live in a flat `Vec<Vec<f32>>` indexed by a stable `buffer_id`
computed from `(node_slot_index, port_id)`. This allocation already exists — the
fix is threading the correct slices into `ProcessInput`.

### Executor process loop change

```rust
// Before each node's process() call:
let audio_input_slices: Vec<&[f32]> = self
    .audio_edges_into(node_id)           // pre-computed at build_executor()
    .map(|(src_node, src_port)| {
        let buf_id = self.buffer_id(src_node, src_port);
        self.audio_buffers[buf_id].as_slice()
    })
    .collect();

// ProcessInput constructed with populated audio_inputs:
let input = ProcessInput::new(
    /* existing fields */
    audio_inputs: &audio_input_slices,
    /* ... */
);
```

`audio_edges_into(node_id)` is a pre-computed adjacency list built at
`build_executor()` time — no graph traversal on the audio thread.

### Lifetime note

The executor processes nodes in topological order. Source nodes always render before
destination nodes. Borrowing a source node’s output buffer as the input to a
downstream node is safe within the cycle — the source buffer is fully written before
the borrow occurs. The `Vec<&[f32]>` is constructed from slices into
`self.audio_buffers` and dropped at the end of each node’s `process()` call.

### `ProcessInput` — no API change

`audio_inputs: &'a [&'a [f32]]` is already declared. No L2 change. This is purely
an executor wiring fix.

### Done criterion

`FilterNode::process()` receives a non-empty `audio_inputs` slice when connected
downstream of a `Sampler`. A test node that asserts `!input.audio_inputs().is_empty()`
passes when wired into an audio chain.

-----

## P4.5 Integration Test

All four fixes must be implemented before running this test. This is the P4.5 done
criterion.

### Test scenario

```
Setup:
  Boot the P4 graph at 140 BPM.
  Load launchpad.rhai and digitakt.rhai profiles.
  Arm step 0 on Sequencer[0] (Kick) with a non-zero distortion drive value
    set via CMD_SET_PARAM to DistortionNode[0].

Assertions:
  1. LED routing — At boot, Launchpad X pad 0 lights according to the profile's
     trigger-mode colours (dim amber for a track with an active step). The grid
     is not dark.

  2. Device ID tagging — Press Launchpad pad 0. The launchpad.rhai on_hw_event
     handler fires. The digitakt.rhai handler does NOT fire. Press a Digitakt
     encoder. The digitakt.rhai handler fires. The launchpad.rhai handler does
     NOT fire.

  3. Sampler ParameterBank — While the clock is running, send CMD_BUMP_PARAM
     with param_id=PARAM_PITCH, arg1=+2.0 to Sampler[0]. The Kick track audibly
     plays at a higher pitch on the next step trigger. The change persists across
     multiple loop iterations.

  4. Audio inputs wiring — FilterNode[0] and DistortionNode[0] receive non-empty
     audio_inputs in their process() calls. Verified by a test-mode assertion flag
     on FilterNode that panics if audio_inputs is empty when the filter is connected.
     No panic occurs during a 10-second run.

  5. End-to-end audio — The Kick track is audible through AudioOutput with
     distortion and filter applied. Confirming the full Sampler → Distortion →
     Filter → Mix chain is functioning.
```

### Automated regression tests to add

- `led_routing_deliver_script_output_reaches_handle` — push a `HardwareOutput` via
  `deliver_script_output()`, assert it arrives in the mock handle’s `push_output()`.
- `one_gateway_per_device_correct_device_id` — wire two mock devices to two separate
  gateways, verify events from each carry the correct device_id.
- `sampler_bump_param_pitch_changes_playback` — send CMD_BUMP_PARAM pitch to Sampler,
  trigger a note, assert rendered frequency has shifted.
- `filter_node_receives_audio_inputs` — wire Sampler → FilterNode in executor test
  harness, assert FilterNode’s audio_inputs is non-empty.

-----

# PART TWO: P5 — Depth

## Overview

P5 adds expressive sequencing (conditional trigs, micro-timing, swing) and a complete
effects palette (reverb, delay, split). Audio routing is complete after P4.5 — P5
nodes depend on correct `audio_inputs` wiring and build on it without workarounds.

### New crate dependencies

```toml
# paraclete-nodes (GPL-3.0)
fastrand = "2"   # MIT — no_std compatible RNG for TrigCondition probability
```

No other new dependencies. Freeverb and the delay algorithm are implemented from
scratch per the DSP source policy (architecture-core.md).

-----

## Part 2.1: Sequencer v2

Sequencer v2 is an extension of the existing `Sequencer` node. The existing step
toggle, clear, and clock logic are unchanged. New capabilities are additive.

### New types in `paraclete-nodes`

These types are internal to the Sequencer — not part of L2.

```rust
/// Per-step timing offset. Resolution: 1/96 of a beat. Range: ±47.
/// At 140 BPM / 44100 Hz: one unit ≈ 198 samples ≈ 4.5 ms.
/// Positive = push forward (later). Negative = pull back (earlier).
/// Swing is applied externally at emit time — not stored here.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepTiming {
    pub micro_offset: i8,   // -47 to +47; 0 = on the grid
}

impl StepTiming {
    /// Resolve to a sample offset given samples-per-beat.
    /// Always returns 0 for a zero micro_offset (no division performed).
    pub fn to_sample_offset(&self, samples_per_beat: f64) -> u32 {
        if self.micro_offset == 0 {
            return 0;
        }
        let frac = self.micro_offset as f64 / 96.0;
        (frac * samples_per_beat).round().abs() as u32
    }
}

/// Repeat condition for a trig.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RepeatCondition {
    Always,
    NthOfM { n: u8, m: u8 },   // fire on the nth repetition of every m loops
}

/// Fill condition for a trig.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FillCondition {
    Ignore,     // fill state does not affect this trig
    FillA,      // fire only when fill A is active
    FillB,      // fire only when fill B is active
    FillAny,    // fire when either fill is active
    NoFill,     // fire only when no fill is active
    NotFillA,   // fire when fill A is NOT active
    NotFillB,   // fire when fill B is NOT active
}

/// Condition governing whether a trig fires on a given loop iteration.
/// Enum (not struct) so the model can grow without getting stuck flat.
/// Compound and Script variants are reserved for future phases.
#[derive(Clone, Debug)]
pub enum TrigCondition {
    Simple {
        repeat:      RepeatCondition,
        fill:        FillCondition,
        probability: u8,   // 0–100. 100 = always fire (no RNG call). 0 = never.
    },
    // Compound(ConditionExpr),   // reserved — needs audio-thread bytecode (P7+)
    // Script(ConditionId),       // reserved — Rhai-defined condition (P7+)
}

impl TrigCondition {
    /// Evaluate the condition. Returns true if the trig should fire.
    /// Evaluation order: repeat gate → fill gate → probability roll.
    /// Simple { Always, Ignore, 100 } is zero-cost (no RNG, no branch on fill).
    pub fn evaluate(
        &self,
        cycle_state: &SequencerCycleState,
        rng: &mut fastrand::Rng,
    ) -> bool {
        match self {
            TrigCondition::Simple { repeat, fill, probability } => {
                // 1. Repeat gate
                let repeat_pass = match repeat {
                    RepeatCondition::Always => true,
                    RepeatCondition::NthOfM { n, m } => {
                        *m > 0 && (cycle_state.loop_count % *m as u32) == (*n as u32 - 1)
                    }
                };
                if !repeat_pass { return false; }

                // 2. Fill gate
                let fill_pass = match fill {
                    FillCondition::Ignore   => true,
                    FillCondition::FillA    => cycle_state.fill_a,
                    FillCondition::FillB    => cycle_state.fill_b,
                    FillCondition::FillAny  => cycle_state.fill_a || cycle_state.fill_b,
                    FillCondition::NoFill   => !cycle_state.fill_a && !cycle_state.fill_b,
                    FillCondition::NotFillA => !cycle_state.fill_a,
                    FillCondition::NotFillB => !cycle_state.fill_b,
                };
                if !fill_pass { return false; }

                // 3. Probability roll (skip if 100 — no RNG overhead)
                if *probability >= 100 { return true; }
                if *probability == 0   { return false; }
                rng.u8(0..100) < *probability
            }
        }
    }
}

impl Default for TrigCondition {
    fn default() -> Self {
        TrigCondition::Simple {
            repeat:      RepeatCondition::Always,
            fill:        FillCondition::Ignore,
            probability: 100,
        }
    }
}

/// Placeholder for future Script condition variant.
pub struct ConditionId(pub u32);

/// Per-loop state used for condition evaluation.
#[derive(Clone, Copy, Debug, Default)]
pub struct SequencerCycleState {
    pub loop_count: u32,   // increments each time the pattern completes
    pub fill_a:     bool,
    pub fill_b:     bool,
}
```

### `StepData` — extended

```rust
// Existing fields retained. New fields added.
pub struct StepData {
    pub active:       bool,
    pub note:         u8,
    pub velocity:     u8,
    pub param_locks:  Vec<ParamLock>,
    pub condition:    TrigCondition,    // new — default: Simple { Always, Ignore, 100 }
    pub timing:       StepTiming,       // new — default: micro_offset = 0
}

impl Default for StepData {
    fn default() -> Self {
        Self {
            active:      false,
            note:        60,
            velocity:    100,
            param_locks: Vec::new(),
            condition:   TrigCondition::default(),
            timing:      StepTiming::default(),
        }
    }
}
```

### New `Sequencer` fields

```rust
pub struct Sequencer {
    // existing fields unchanged
    cycle_state:    SequencerCycleState,   // new
    rng:            fastrand::Rng,          // new — seeded at activate()
    active_pattern: u8,                     // new — stub; always 0 at P5
    swing_amount:   f32,                    // new — 0.0–0.5; read from ParameterBank
}
```

### New `Sequencer` command constants

Extending the existing set (CMD_TOGGLE_STEP=16, CMD_SET_STEP=17, CMD_CLEAR=18,
CMD_TRIGGER is on Sampler at 19 per ADR-020):

```rust
impl Sequencer {
    // Existing:
    pub const CMD_TOGGLE_STEP:       u32 = 16;  // arg0 = step_index
    pub const CMD_SET_STEP:          u32 = 17;  // arg0 = step, arg1 = note (< 0 = deactivate)
    pub const CMD_CLEAR:             u32 = 18;  // clear all steps

    // New at P5:
    pub const CMD_SET_FILL_A:        u32 = 23;  // arg0 = 1 (active) or 0 (inactive)
    pub const CMD_SET_FILL_B:        u32 = 24;  // arg0 = 1 (active) or 0 (inactive)
    pub const CMD_SET_STEP_TIMING:   u32 = 25;  // arg0 = step_index, arg1 = micro_offset (i8 cast to i64)
    pub const CMD_SET_STEP_CONDITION:u32 = 26;  // arg0 = step_index, arg1 = encoded condition (see below)
    pub const CMD_SET_PATTERN:       u32 = 27;  // arg0 = pattern_index — stub at P5; no-op if > 0
}
```

### Condition encoding for CMD_SET_STEP_CONDITION

`arg1` (i64) packs the Simple variant fields into a single integer:

```
bits  0– 7: probability (0–100)
bits  8–15: repeat_n    (NthOfM.n; 0 = Always)
bits 16–23: repeat_m    (NthOfM.m; 0 = Always)
bits 24–31: fill_condition discriminant:
              0 = Ignore
              1 = FillA
              2 = FillB
              3 = FillAny
              4 = NoFill
              5 = NotFillA
              6 = NotFillB
```

Decoder in `handle_commands()`:

```rust
Sequencer::CMD_SET_STEP_CONDITION => {
    let step_idx = cmd.arg0 as usize;
    if step_idx >= self.steps.len() { continue; }
    let enc = cmd.arg1 as i64 as u64;
    let probability   = (enc & 0xFF) as u8;
    let repeat_n      = ((enc >> 8) & 0xFF) as u8;
    let repeat_m      = ((enc >> 16) & 0xFF) as u8;
    let fill_disc     = ((enc >> 24) & 0xFF) as u8;

    let repeat = if repeat_n == 0 || repeat_m == 0 {
        RepeatCondition::Always
    } else {
        RepeatCondition::NthOfM { n: repeat_n, m: repeat_m }
    };

    let fill = match fill_disc {
        1 => FillCondition::FillA,
        2 => FillCondition::FillB,
        3 => FillCondition::FillAny,
        4 => FillCondition::NoFill,
        5 => FillCondition::NotFillA,
        6 => FillCondition::NotFillB,
        _ => FillCondition::Ignore,
    };

    self.steps[step_idx].condition = TrigCondition::Simple {
        repeat, fill, probability
    };
}
```

### New Sequencer ParameterBank entries

```rust
// Declared in Sequencer::capability_document():
pub const PARAM_SWING: u32 = id_for_name("swing");
// 0.0–0.5, default 0.0
// 0.0 = no swing. 0.5 = maximum swing (odd steps pushed by half a beat).
// Intermediate values scale linearly.
```

### Swing implementation

Swing is applied at event emit time — not stored per step. It affects all odd-indexed
steps (steps 1, 3, 5, …).

```rust
fn step_sample_offset(&self, step_idx: usize, samples_per_beat: f64) -> u32 {
    let micro_offset = self.steps[step_idx].timing.to_sample_offset(samples_per_beat);
    let swing_offset = if step_idx % 2 == 1 {
        (self.swing_amount as f64 * samples_per_beat) as u32
    } else {
        0
    };
    micro_offset + swing_offset
}
```

`swing_amount` is read from `self.bank.get(PARAM_SWING)` at the start of each
`process()` call, after `bank.handle_commands()`.

### Micro-timing and event emission

Events are emitted with `sample_offset` populated. The executor’s existing event sort
(by `sample_offset`) handles delivery correctly — no executor changes needed.

```rust
// When a step fires:
let spb = input.samples_per_beat();    // provided by TransportEvent
let offset = self.step_sample_offset(step_idx, spb);

output.push_event(Event::NoteOn {
    note:          step.note,
    velocity:      step.velocity,
    sample_offset: offset,    // was always 0 before P5
    // ... other fields
});
```

`TransportEvent` already carries enough clock information to derive `samples_per_beat`.
If not currently exposed on `ProcessInput`, add:

```rust
impl<'a> ProcessInput<'a> {
    /// Samples per beat at the current tempo.
    /// Derived from the most recent TransportEvent in this buffer.
    /// Returns 0.0 if no TransportEvent has been received yet.
    pub fn samples_per_beat(&self) -> f64;
}
```

### Loop count and cycle state

`loop_count` increments each time `current_step` wraps from the last step back to 0.

```rust
// In Sequencer::process(), at step wrap:
if self.current_step == 0 && self.prev_step == self.pattern_length - 1 {
    self.cycle_state.loop_count = self.cycle_state.loop_count.wrapping_add(1);
}
```

`fill_a` and `fill_b` are set directly by `CMD_SET_FILL_A` / `CMD_SET_FILL_B` in
`handle_commands()`.

### New Sequencer published state

```
/node/{id}/state/loop_count  — u32 (wrapping)
/node/{id}/state/fill_a      — bool (0 or 1 as f64)
/node/{id}/state/fill_b      — bool (0 or 1 as f64)
```

Existing published state unchanged:

```
/node/{id}/state/steps        — 16-char ASCII bitfield
/node/{id}/state/current_step — u32
/node/{id}/state/track_name   — String
/node/{id}/state/last_trig    — u64 timestamp (ADR-020)
```

### Multi-pattern stub

`active_pattern: u8` field is stored. `CMD_SET_PATTERN` is accepted and stored but
has no effect — the Sequencer always plays pattern 0 at P5. Bank storage is P6.
This stubs the API so profile scripts can issue the command without error.

-----

## Part 2.2: `ReverbNode`

Algorithm: Freeverb (Jezar at Dreampoint, public domain / MIT). A well-known,
thoroughly documented algorithm: 8 parallel comb filters + 4 series allpass filters
per channel. All delay line lengths are determined at `activate()` from the sample
rate — not hardcoded.

### Struct

```rust
pub struct ReverbNode {
    bank:        ParameterBank,
    // Freeverb internal state — pre-allocated at activate()
    comb_l:      [CombFilter; 8],
    comb_r:      [CombFilter; 8],
    allpass_l:   [AllpassFilter; 4],
    allpass_r:   [AllpassFilter; 4],
    pre_delay_l: DelayLine,   // max 100ms
    pre_delay_r: DelayLine,
    pre_delay_pos: usize,
    pre_delay_len: usize,
}

struct CombFilter {
    buf:      Vec<f32>,   // pre-allocated at activate()
    pos:      usize,
    feedback: f32,
    damp1:    f32,
    damp2:    f32,
    filterstore: f32,
}

struct AllpassFilter {
    buf: Vec<f32>,   // pre-allocated at activate()
    pos: usize,
}

struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}
```

### Parameters

```rust
pub const PARAM_REVERB_ROOM_SIZE:  u32 = id_for_name("room_size");
pub const PARAM_REVERB_DAMPING:    u32 = id_for_name("damping");
pub const PARAM_REVERB_WET:        u32 = id_for_name("wet");
pub const PARAM_REVERB_DRY:        u32 = id_for_name("dry");
pub const PARAM_REVERB_WIDTH:      u32 = id_for_name("width");
pub const PARAM_REVERB_PRE_DELAY:  u32 = id_for_name("pre_delay_ms");

// Ranges and defaults:
// room_size:   0.0–1.0,   default 0.5
// damping:     0.0–1.0,   default 0.5
// wet:         0.0–1.0,   default 0.3
// dry:         0.0–1.0,   default 0.7
// width:       0.0–1.0,   default 1.0
// pre_delay_ms: 0.0–100.0, default 0.0
```

### Ports

```
Input:  audio_in_l (id 0), audio_in_r (id 1)
Output: audio_out_l (id 2), audio_out_r (id 3)
```

Mono input is accepted: if `audio_in_r` is unconnected, `audio_in_l` is used for
both channels. Checked via `input.audio_inputs()` length.

### `activate()`

Freeverb comb filter delay line lengths (in samples) are proportional to the sample
rate. The canonical Freeverb lengths are defined relative to 44100 Hz:

```
Comb lengths at 44100:  1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617
Allpass lengths at 44100: 556, 441, 341, 225
Stereo offset: +23 samples per channel for R channel
```

Scale all lengths by `sample_rate / 44100.0` and round to nearest integer.

```rust
fn activate(&mut self, sample_rate: f32, _block_size: usize) {
    let scale = sample_rate / 44100.0;
    let comb_base = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    let allpass_base = [556, 441, 341, 225];
    let stereo_spread = 23;

    for i in 0..8 {
        let len_l = ((comb_base[i] as f32 * scale).round() as usize).max(1);
        let len_r = ((( comb_base[i] + stereo_spread) as f32 * scale).round() as usize).max(1);
        self.comb_l[i] = CombFilter::new(len_l);
        self.comb_r[i] = CombFilter::new(len_r);
    }
    for i in 0..4 {
        let len_l = ((allpass_base[i] as f32 * scale).round() as usize).max(1);
        let len_r = (((allpass_base[i] + stereo_spread) as f32 * scale).round() as usize).max(1);
        self.allpass_l[i] = AllpassFilter::new(len_l);
        self.allpass_r[i] = AllpassFilter::new(len_r);
    }

    let max_pre_delay = ((sample_rate * 0.1) as usize).max(1);  // 100ms
    self.pre_delay_l = DelayLine::new(max_pre_delay);
    self.pre_delay_r = DelayLine::new(max_pre_delay);

    self.bank = ParameterBank::from_capability_document(&self.capability_document());
}
```

### `process()`

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());

    let room_size   = self.bank.get(PARAM_REVERB_ROOM_SIZE) as f32;
    let damping     = self.bank.get(PARAM_REVERB_DAMPING) as f32;
    let wet         = self.bank.get(PARAM_REVERB_WET) as f32;
    let dry         = self.bank.get(PARAM_REVERB_DRY) as f32;
    let width       = self.bank.get(PARAM_REVERB_WIDTH) as f32;
    let pre_delay_s = self.bank.get(PARAM_REVERB_PRE_DELAY) as f32 / 1000.0;

    // Freeverb tuning constants
    let feedback  = room_size * 0.28 + 0.7;   // maps 0–1 to 0.7–0.98
    let damp      = damping * 0.4;
    let wet1 = wet * (width / 2.0 + 0.5);
    let wet2 = wet * ((1.0 - width) / 2.0);

    let in_l = input.audio_inputs().get(0).copied().unwrap_or(&[]);
    let in_r = input.audio_inputs().get(1).copied().unwrap_or(in_l);

    let out_l = output.audio_output_mut(2);
    let out_r = output.audio_output_mut(3);

    for i in 0..in_l.len() {
        // Pre-delay
        let pd_len = (pre_delay_s * sample_rate) as usize;
        // ... write to pre_delay, read with offset

        let input_mixed = (pre_delayed_l + pre_delayed_r) * 0.015;

        // 8 parallel comb filters
        let mut out_l_s = 0.0f32;
        let mut out_r_s = 0.0f32;
        for c in 0..8 {
            out_l_s += self.comb_l[c].process(input_mixed, feedback, damp);
            out_r_s += self.comb_r[c].process(input_mixed, feedback, damp);
        }

        // 4 series allpass filters
        for a in 0..4 {
            out_l_s = self.allpass_l[a].process(out_l_s);
            out_r_s = self.allpass_r[a].process(out_r_s);
        }

        out_l[i] = out_l_s * wet1 + out_r_s * wet2 + in_l[i] * dry;
        out_r[i] = out_r_s * wet1 + out_l_s * wet2 + in_r[i] * dry;
    }
}
```

-----

## Part 2.3: `DelayNode`

A stereo tape-style delay with low-pass filtering on the feedback path.

### Struct

```rust
pub struct DelayNode {
    bank:       ParameterBank,
    buf_l:      Vec<f32>,   // pre-allocated at activate(); max 2000ms
    buf_r:      Vec<f32>,
    write_pos:  usize,
    max_len:    usize,      // sample_rate * 2 — computed at activate()
    // SVF state for feedback LP filter (reuses FilterNode approach)
    low_l:      f32,
    band_l:     f32,
    low_r:      f32,
    band_r:     f32,
}
```

### Parameters

```rust
pub const PARAM_DELAY_TIME_MS:  u32 = id_for_name("delay_time_ms");
pub const PARAM_DELAY_FEEDBACK: u32 = id_for_name("feedback");
pub const PARAM_DELAY_WET:      u32 = id_for_name("wet");
pub const PARAM_DELAY_DRY:      u32 = id_for_name("dry");
pub const PARAM_DELAY_FILTER_HZ:u32 = id_for_name("filter_hz");

// Ranges and defaults:
// delay_time_ms: 1.0–2000.0,   default 250.0
// feedback:      0.0–0.95,     default 0.4  (capped at 0.95 — no runaway)
// wet:           0.0–1.0,      default 0.5
// dry:           0.0–1.0,      default 1.0
// filter_hz:     200.0–8000.0, default 4000.0
```

### Ports

```
Input:  audio_in_l (id 0), audio_in_r (id 1)
Output: audio_out_l (id 2), audio_out_r (id 3)
```

Mono input: if `audio_in_r` unconnected, `audio_in_l` feeds both channels.

### `activate()`

```rust
fn activate(&mut self, sample_rate: f32, _block_size: usize) {
    self.max_len = (sample_rate * 2.0) as usize;   // 2000ms max
    self.buf_l = vec![0.0f32; self.max_len];
    self.buf_r = vec![0.0f32; self.max_len];
    self.write_pos = 0;
    self.bank = ParameterBank::from_capability_document(&self.capability_document());
}
```

### `process()`

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());

    let delay_samples = (self.bank.get(PARAM_DELAY_TIME_MS) as f32
                         / 1000.0 * sample_rate) as usize;
    let delay_samples = delay_samples.clamp(1, self.max_len - 1);
    let feedback   = self.bank.get(PARAM_DELAY_FEEDBACK) as f32;
    let wet        = self.bank.get(PARAM_DELAY_WET) as f32;
    let dry        = self.bank.get(PARAM_DELAY_DRY) as f32;
    let filter_hz  = self.bank.get(PARAM_DELAY_FILTER_HZ) as f32;

    // Chamberlin SVF coefficients for feedback LP (same as FilterNode)
    let f = (std::f32::consts::PI * filter_hz / sample_rate).sin();
    let q = 0.7f32;

    let in_l = input.audio_inputs().get(0).copied().unwrap_or(&[]);
    let in_r = input.audio_inputs().get(1).copied().unwrap_or(in_l);

    let out_l = output.audio_output_mut(2);
    let out_r = output.audio_output_mut(3);

    for i in 0..in_l.len() {
        let read_pos = (self.write_pos + self.max_len - delay_samples) % self.max_len;

        let delayed_l = self.buf_l[read_pos];
        let delayed_r = self.buf_r[read_pos];

        // LP filter on feedback path
        self.low_l  += f * self.band_l;
        self.band_l += f * (delayed_l - self.low_l - q * self.band_l);
        self.low_r  += f * self.band_r;
        self.band_r += f * (delayed_r - self.low_r - q * self.band_r);

        self.buf_l[self.write_pos] = in_l[i] + self.low_l * feedback;
        self.buf_r[self.write_pos] = in_r[i] + self.low_r * feedback;

        out_l[i] = in_l[i] * dry + delayed_l * wet;
        out_r[i] = in_r[i] * dry + delayed_r * wet;

        self.write_pos = (self.write_pos + 1) % self.max_len;
    }
}
```

-----

## Part 2.4: `SplitNode`

Routes one audio stream to two independent outputs with per-output gain control.
Enables send/return topologies. Two SplitNodes in series give three outputs.

### Struct

```rust
pub struct SplitNode {
    bank: ParameterBank,
}
```

### Parameters

```rust
pub const PARAM_SPLIT_GAIN_0: u32 = id_for_name("gain_0");
pub const PARAM_SPLIT_GAIN_1: u32 = id_for_name("gain_1");

// Ranges and defaults:
// gain_0: 0.0–2.0, default 1.0
// gain_1: 0.0–2.0, default 1.0
```

### Ports

```
Input:  audio_in (id 0)
Output: audio_out_0 (id 1), audio_out_1 (id 2)
```

### `process()`

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    self.bank.handle_commands(input.commands());

    let gain_0 = self.bank.get(PARAM_SPLIT_GAIN_0) as f32;
    let gain_1 = self.bank.get(PARAM_SPLIT_GAIN_1) as f32;

    let in_buf = input.audio_inputs().get(0).copied().unwrap_or(&[]);
    let out_0  = output.audio_output_mut(1);
    for (o, &s) in out_0.iter_mut().zip(in_buf.iter()) {
        *o = s * gain_0;
    }
    let out_1 = output.audio_output_mut(2);
    for (o, &s) in out_1.iter_mut().zip(in_buf.iter()) {
        *o = s * gain_1;
    }
}
```

No state. `activate()` builds `ParameterBank` only.

-----

## Part 2.5: P5 Graph

The P5 graph extends the P4 graph. The eight existing chains gain optional reverb
and delay inserts. `SplitNode` enables a send topology if desired. The base graph
is unchanged — new nodes are opt-in.

Minimal P5 graph (adds reverb on master bus):

```
InternalClock
  ├──→ Sequencer[0..7] → Sampler[0..7] → Distortion[0..7] → Filter[0..7] ──┐
  └──→ (as P4)                                                               │
                                                                             ↓
                                                                         MixNode
                                                                             │
                                                                        ReverbNode
                                                                             │
                                                                        AudioOutput

LaunchpadNode  ──→ ScriptingGatewayNode[LP]  ──→ ScriptEventConsumer[LP]
DigitaktNode   ──→ ScriptingGatewayNode[DT]  ──→ ScriptEventConsumer[DT]
KeystepNode    ──→ ScriptingGatewayNode[KS]  ──→ ScriptEventConsumer[KS]
```

-----

## Part 2.6: Profile Script Updates

### Fill control from Rhai

```rhai
// In launchpad.rhai — SHIFT held on a specific button activates fill A
on_hw_event(LP_DEVICE_ID, |event| {
    if event.type == "ButtonPressed" && event.id == FILL_A_BUTTON_ID {
        for seq_id in TRACK_SEQ_IDS {
            send_cmd(seq_id, 23, 1, 0.0);  // CMD_SET_FILL_A active
        }
    }
    if event.type == "ButtonReleased" && event.id == FILL_A_BUTTON_ID {
        for seq_id in TRACK_SEQ_IDS {
            send_cmd(seq_id, 23, 0, 0.0);  // CMD_SET_FILL_A inactive
        }
    }
});
```

### Micro-timing from Rhai

```rhai
// Set step 3 timing to +8 micro-steps (push slightly late) on track 0
send_cmd(TRACK_SEQ_IDS[0], 25, 3, 8.0);   // CMD_SET_STEP_TIMING

// Set conditional trig: step 5 fires on 1st of every 2 loops, fill ignored, 75%
// Encoding: probability=75, repeat_n=1, repeat_m=2, fill=Ignore(0)
// bits 0–7: 75, bits 8–15: 1, bits 16–23: 2, bits 24–31: 0
let enc = 75 | (1 << 8) | (2 << 16) | (0 << 24);
send_cmd(TRACK_SEQ_IDS[0], 26, 5, enc);    // CMD_SET_STEP_CONDITION
```

-----

## P5 Done Criteria

- `TrigCondition::evaluate()` is tested against all combinations of repeat,
  fill, and probability=0/50/100
- Swing at 0.5 audibly displaces odd steps by approximately half a beat
- `ReverbNode` output is non-silent for non-silent input with wet > 0
- `DelayNode` produces an echo at the configured delay time
- `SplitNode` output_0 and output_1 both carry the input signal
- All new nodes pass the portability check: `cargo tree -p paraclete-nodes`
  shows no dependency on `paraclete-runtime` or `paraclete-scripting`
- Test count at P5 ship: existing 188 + minimum 20 new tests covering the
  above assertions