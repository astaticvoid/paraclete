# P2 Code Review

Reviewing commit 595f3b2 ("Paraclete — P2 complete"). This commit is the
entire foundation: P0, P1, and P2. All file paths reference that commit;
current-working-tree status is noted where the issue has since been addressed.

---

## Critical (must fix before building further)

### C-1: Audio thread takes an RwLock write every cycle

**File:** `crates/paraclete-runtime/src/executor.rs`, lines 211-217

```rust
if let Ok(mut map) = self.state_bus_snapshot.write() {
    for (k, v) in &local_state_bus {
        map.insert(k.clone(), v.clone());
    }
}
```

The executor comments document this as "the sole exception (lock held for
microseconds)." That is incorrect framing. Taking a mutex/RwLock on the audio
thread violates the hard constraint in `CLAUDE.md` ("never block or take a lock
in `process()`"). The comment elsewhere says the StateBus is "never touched on
the audio thread" — that promise is broken every cycle. If the main thread
holds the read lock for any reason (subscriber poll, scripting read), the audio
thread blocks. At 44100 Hz with a 512-frame block, the callback budget is
~11 ms; a stalled reader can easily blow it.

**Why it matters:** This is not a latency spike risk, it is a correctness
violation of the stated audio-thread contract. It compounds with every new
subscriber added in P3/P4 (Rhai, GUI).

**Fix direction:** Replace with the lock-free SPSC that the report already
identifies as the P4 target. The `rtrb` crate ships in P3 for exactly this
purpose. Executor pushes a `StateBusUpdate` struct at end of cycle; main thread
drains it in `process_state_bus()`. This is the architecture described in
`CLAUDE.md`'s StateBus section. **Status: fixed in P3.**

---

### C-2: LaunchpadEmulator performs blocking I/O inside `process()`

**File:** `crates/paraclete-hal/src/emulator/mod.rs`, lines 167-171

```rust
fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
    self.poll_keyboard();  // calls crossterm::event::poll + ::read → syscalls
    self.maybe_render();   // Instant::now() + writes to stdout
    ...
}
```

`poll_keyboard` performs two syscalls per call: `crossterm::event::poll` and
`crossterm::event::read`. `maybe_render` calls `terminal::render` which issues
many `stdout` writes via `crossterm::execute!`. All of this runs on the audio
thread. Stdout writes can block indefinitely (terminal scroll, pipe buffer full,
SSH latency). `Instant::now` is a syscall on some platforms.

**Why it matters:** This is the most immediate cause of audio glitches in the
P2 demo. Every 16 ms (the render debounce), the audio callback writes to
terminal, potentially stalling for tens of milliseconds.

**Fix direction:** The emulator needs a background thread or main-loop polling
model. `poll_keyboard` should run on the main thread, pushing `HardwareEvent`
into an SPSC. `process()` drains that SPSC and emits events — no syscalls.
Rendering belongs in the main loop, not in `process()`. The P4
`HardwareOutputHandle::tick()` pattern on the main thread is the correct
architecture. **Status: addressed in P4 with `HardwareOutputHandle`.**

---

### C-3: Audio input routing is permanently wired to empty — no node can receive audio from another

**File:** `crates/paraclete-runtime/src/executor.rs`, line 174

```rust
let input = ProcessInput {
    audio_inputs: &[],   // always empty; every node sees no audio inputs
    ...
};
```

The executor always passes `audio_inputs: &[]` regardless of graph connections.
This means any node with an `Audio` input port (a gain stage, compressor,
reverb, or any effect) receives silence instead of the upstream node's output.
The P2 graph happens to work only because the sole audio-producing node
(`SineOscillator`) writes to its `audio_out` buffer and the executor sums all
node `audio_out` buffers into the final output — but inter-node audio routing
is completely absent.

**Why it matters:** Effects, mixers, sends — every downstream audio processing
node is architecturally broken. A Sampler node added in P3 that needed to feed
an effect chain would silently receive no input. This is not marked provisional
in the code, only in the P2 report ("audio buffer wiring at P2" comment in
EdgeMeta is misleading — it never actually happened).

**Fix direction:** The executor must wire audio output buffers of upstream nodes
to the audio input slices of downstream nodes, keyed by the edge's
`src_port`/`dst_port`. `EdgeMeta` already stores these port IDs. The
`audio_inputs` field of `ProcessInput` needs to be populated from the
preceding node's `audio_out` buffer using the edge table built at
`build_executor()` time.

---

### C-4: Sample-rate and block-size mismatch between configurator and audio driver

**File:** `crates/paraclete-app/src/main.rs` lines 16-17; `crates/paraclete-hal/src/audio.rs` lines 37, 42

`main.rs` hardcodes `SAMPLE_RATE = 44100.0` and `BLOCK_SIZE = 512`, then
passes these to `NodeConfigurator::new`. The `AudioBackend` queries cpal for
the device's actual sample rate and uses `BufferSize::Default`, which returns
whatever block size the OS chooses (commonly 256, 512, or 1024 frames, but
variable per callback).

The executor stores `self.block_size = 512` and allocates `AudioBuffer::new(2,
512)` per node. When cpal calls `executor.process(data, channels)` with
`data.len() != 512 * channels`, the executor has a `debug_assert_eq!` that
fires in debug builds and silently reads/writes out-of-bounds in release builds
(UB). The pitch of all oscillators will be wrong if the device runs at 48000 Hz.

**Why it matters:** This is undefined behavior in release mode on any device
that doesn't happen to use a 512-sample block size at exactly 44100 Hz. Most
macOS and Linux systems default to 48000 Hz.

**Fix direction:** `AudioBackend::start` must extract the actual sample rate
and block size from cpal and pass them to the `NodeExecutor` (or better, pass
them at `NodeConfigurator::new` time so `activate()` gets the correct values).
Use `BufferSize::Fixed(BLOCK_SIZE)` if a fixed block size is required, or
design the executor to process an arbitrary number of frames per call.

---

### C-5: Layer violation — L0 HAL depends on L1 Runtime

**File:** `crates/paraclete-hal/Cargo.toml` line 12; `crates/paraclete-hal/src/audio.rs` line 4

```toml
paraclete-runtime = { workspace = true }
```

```rust
use paraclete_runtime::NodeExecutor;
```

The five-layer model in `CLAUDE.md` is explicit: "No layer may reach across
another layer's boundary." L0 HAL should depend only on L2 Node API. Instead,
`AudioBackend` imports `NodeExecutor` from L1 Runtime directly, creating a
circular dependency risk and making the HAL unable to compile without the
runtime.

**Why it matters:** If the App layer is supposed to wire HAL to Runtime (as the
architecture intends), then HAL knowing about `NodeExecutor` breaks the
separation. A CLAP plugin host would need to link L0+L1+L2 together even if
only HAL is needed for MIDI I/O.

**Fix direction:** The audio callback signature should accept a trait object or
closure that the App layer provides — something like `Box<dyn FnMut(&mut [f32],
usize)>` — rather than `NodeExecutor` directly. The App layer owns the wiring
of HAL → Runtime. Alternatively, define a minimal `AudioProcessor` trait in L2
that `NodeExecutor` implements, and have HAL depend on L2 only. **Status:
partially addressed in P4 by restructuring the app layer.**

---

### C-6: `incoming` event buffers allocate on the audio thread every cycle

**File:** `crates/paraclete-runtime/src/executor.rs`, lines 158 and 198

```rust
let events: Vec<TimedEvent> = std::mem::take(&mut self.incoming[slot_idx]);
// ...
self.incoming[dst_idx].extend_from_slice(unsafe { &*outgoing });
```

`std::mem::take` replaces `self.incoming[slot_idx]` with `Default::default()`
(an empty `Vec` with zero capacity) and returns the original `Vec`. The
returned `Vec` is dropped at the end of the loop body — this is a heap
deallocation on the audio thread.

More importantly, `self.incoming[dst_idx].extend_from_slice(...)` pushes events
into a `Vec` that was just replaced with a zero-capacity default. If any events
are routed (which is the entire point), `extend_from_slice` triggers a heap
allocation on the audio thread to grow the vector.

**Why it matters:** Heap allocation on the audio thread causes priority
inversion with the system allocator's internal mutex. This is one of the most
common causes of audio glitches on multi-core systems.

**Fix direction:** Pre-allocate the incoming event vecs with a fixed capacity
(matching `EventOutputBuffer::capacity`) and never let them shrink to zero.
Replace `mem::take` with a pattern that reuses the allocation:

```rust
// Instead of mem::take, swap with a pre-cleared staging buffer
let mut events = /* pre-allocated staging vec */;
std::mem::swap(&mut events, &mut self.incoming[slot_idx]);
// ... process events ...
events.clear(); // retain capacity
std::mem::swap(&mut events, &mut self.incoming[slot_idx]); // put back
```

Or simpler: keep a single `Vec<TimedEvent>` per slot with adequate capacity, and
don't use `mem::take` at all — just pass a slice reference to `process()`.

---

### C-7: `build_hardware_output` allocates a `Vec` on the audio thread every cycle

**File:** `crates/paraclete-runtime/src/executor.rs`, lines 112 and 204

```rust
fn build_hardware_output(&self, ...) -> HardwareOutput {
    let mut led_updates = Vec::new();  // allocation every cycle
    // ...
    HardwareOutput { led_updates, display_updates: vec![] }  // two more
}
```

Three heap allocations per audio cycle, in addition to the `HashMap::new()` on
line 204 (`let mut local_state_bus: HashMap<String, StateBusValue> = HashMap::new()`).
`HashMap::insert` with `String` keys also calls the allocator.

**Fix direction:** `HardwareOutput` and the local StateBus map need pre-allocated
structures with fixed capacity, reused across cycles. With the P4
`HardwareOutputHandle` pattern (tick on main thread), the LED update path moves
off the audio thread entirely, which is the correct solution.

---

## Significant (fix before P5)

### S-1: `negotiate()` is declared but never called by the runtime

**File:** `crates/paraclete-runtime/src/configurator.rs`, `connect()` method

The `Node::negotiate()` method is documented as being called at connection time:
"When two nodes connect, the runtime calls `negotiate()` on both sides." The
`configurator.connect()` method does not call `negotiate()` on either node. The
`ConnectionAgreement` returned by `baseline()` hardcodes `sample_rate = 44100.0`
and `block_size = 512` regardless of the actual runtime configuration.

**Why it matters:** The negotiation protocol is a core L3 engagement mechanism
(ADR-008). Any node that overrides `negotiate()` expecting to be informed of
the connection parameters receives nothing. `ConnectionAgreement.initial_transport`
is never populated, so downstream nodes that rely on it for initial position
sync cannot use it. The entire handshake infrastructure ships dead.

**Fix direction:** `configurator.connect()` should call `negotiate()` on both
nodes after the DAG check passes, reconcile the two `ConnectionAgreement`
values, and call `set_connection_record()` (or similar) to inform both nodes of
the final agreement. The agreed `sample_rate` and `block_size` should come from
the runtime, not from `baseline()` constants.

---

### S-2: `connect()` does not validate port direction or type compatibility

**File:** `crates/paraclete-runtime/src/configurator.rs`, lines 162-170

```rust
let src_port_type = self.nodes[&src]
    .ports()
    .iter()
    .find(|p| p.id == src_port)
    .map(|p| p.port_type)
    .unwrap_or(PortType::Audio);   // silent fallback if port not found
```

Two missing checks:

1. **Port direction**: `connect()` does not verify that `src_port` is an
   Output port and `dst_port` is an Input port. Connecting two outputs or two
   inputs is silently accepted.

2. **Port not found**: If `src_port` ID doesn't exist on the node, it silently
   defaults to `PortType::Audio` rather than returning an error. A typo in a
   port ID produces a valid but incorrect connection.

3. **Type compatibility**: There is no check that `src_port_type` matches the
   `dst_port`'s type. An `Audio` output can be silently connected to an `Event`
   input.

**Why it matters:** These are silent failures. Misconnections produce incorrect
but non-crashing behavior — the executor routes events where audio was expected
or vice versa. With a richer graph (P5+), this will be very hard to debug.

**Fix direction:** `connect()` should look up both ports, verify src is Output
and dst is Input, verify types match (or are compatible), and return `Err` for
any mismatch. The `unwrap_or(PortType::Audio)` fallback should be an error.

---

### S-3: `Sequencer::step_tick` counting is off by one — step 0 never fires

**File:** `crates/paraclete-nodes/src/sequencer.rs`, `handle_transport()`, lines 150-190

After `global_start`, the sequencer sets `current_step = 0, step_tick = 0`. On
the very first tick that arrives with `playing = true`, the code checks if
`step_tick >= ticks_per_step` (0 >= 240 — false), then increments `step_tick`
to 1. After `ticks_per_step` ticks, `step_tick` reaches 240, the sequencer
advances to step 1, and fires step 1's note. Step 0's note is never triggered.

The test `sequencer_advances_step_and_emits_note_on_at_boundary` acknowledges
this by putting the expected note at step 1, with the comment: "(global_start
resets to step 0; first boundary advances to step 1.)" The P2 demo in `main.rs`
puts C4 at step 0 and E4 at step 2 — C4 never sounds; the pattern is silent on
the first beat and plays E4 as its first audible note.

**Why it matters:** This is a correctness bug that makes step-0 programming
permanently unusable. Any user expecting to program a pattern starting on beat 1
will find the first note silently dropped.

**Fix direction:** On `global_start`, immediately check if the current step is
active and fire its note before beginning the tick accumulation. Alternatively,
restructure so that `step_tick >= ticks_per_step` triggers the note of the
*current* step (not the next one) — fire on arrival, not on departure.

---

### S-4: `Sequencer` clones `param_locks` on every active step during `process()`

**File:** `crates/paraclete-nodes/src/sequencer.rs`, lines 168-170

```rust
let (active, locks) = {
    let step = &self.steps[self.current_step];
    (step.active, step.param_locks.clone())
};
```

`step.param_locks` is `Vec<StepParamLock>`. Cloning it on the audio thread
allocates heap memory on every step boundary where the step is active.

**Why it matters:** `process()` must never allocate. This violates the hard
audio-thread constraint. On a 16-step pattern running at 120 BPM with 4
param-locked steps, this allocates 4 times per bar — hundreds of allocations
per minute.

**Fix direction:** Borrow `&self.steps[self.current_step]` directly and iterate
over `step.param_locks` by reference. No clone needed — `StepParamLock` fields
(`node_id: u32`, `param_id: u32`, `value: f64`) are all `Copy` and can be
copied individually into the `ParamLockEvent`.

---

### S-5: `published_state()` called from the audio thread — allocates every cycle

**File:** `crates/paraclete-runtime/src/executor.rs`, lines 204-208;
`crates/paraclete-nodes/src/internal_clock.rs` lines 179-202;
`crates/paraclete-nodes/src/sequencer.rs` lines 268-282

```rust
fn published_state(&self) -> Vec<(String, StateBusValue)> {
    vec![
        (format!("/transport/bpm"), StateBusValue::Float(self.bpm)),
        ...
    ]
}
```

`published_state()` is called from `NodeSlot::published_state()` inside
`NodeExecutor::process()`. Every call allocates a `Vec` of `(String,
StateBusValue)` tuples — every `format!()` call allocates a `String`. The
`node.rs` doc comment says "Allocation is permitted here (not on the hot audio
path)" but the executor calls it from inside the audio callback.

**Why it matters:** N nodes × M keys × allocation per cycle. At 86 cycles/sec
(512 frames at 44100 Hz), this is thousands of allocations per second.

**Fix direction:** With the lock-free SPSC StateBus (C-1 fix), this moves
naturally off the audio thread. Until then: remove the call from `process()`,
or change the signature to `fn push_state(&self, sink: &mut impl StateSink)`
where `StateSink` accepts pre-allocated slots. The cleaner long-term fix is the
lock-free design from P3.

---

### S-6: `ConnectionAgreement::baseline()` hardcodes sample rate and block size

**File:** `crates/paraclete-node-api/src/agreement.rs`, lines 21-27

```rust
pub fn baseline() -> Self {
    Self {
        sample_rate: 44100.0,
        block_size: 512,
        channels: 2,
        ...
    }
}
```

This struct is part of the L2 public API (LGPL-3). Third-party nodes that call
`ConnectionAgreement::baseline()` in their `negotiate()` implementation will
always see 44100/512 regardless of the actual runtime configuration. Since
`negotiate()` is never called (S-1), this is currently inert, but when the
negotiation protocol is activated, baseline must reflect actual runtime values.

**Fix direction:** Remove the constants from `baseline()`. The runtime should
construct a `ConnectionAgreement` seeded with the actual `sample_rate` and
`block_size` from the `NodeConfigurator`, not the other direction.

---

### S-7: `ConfigMessage::SetParam` is received but silently discarded

**File:** `crates/paraclete-runtime/src/executor.rs`, line 100

```rust
ConfigMessage::SetParam { .. } => { /* P2: route to node */ }
```

`ConfigMessage` exposes `SetParam { node_id, param_id, value }` as a public API
in the L1 crate. The executor receives these messages but discards them. Any
code that calls `conf.send(ConfigMessage::SetParam { ... })` will get a
successful return value (`true`) but the parameter change will silently never
be applied.

**Why it matters:** This is a silent API lie. It's documented as P2
placeholder, which is acceptable, but it should at minimum `log::warn!` or be
hidden behind a feature flag to avoid confusing callers. More importantly, no
mechanism routes `SetParam` to the correct node — there is no node-keyed
dispatch table in the executor.

**Fix direction:** Add a `node_id → slot_index` lookup table to the executor.
When `SetParam` arrives, find the slot and call a new `Node::set_param(param_id,
value)` method (which is what `ParameterBank` in P4 implements). Until then,
log a warning rather than silently discarding. **Status: `ParameterBank` ships
in P4.**

---

### S-8: `add_tempo_source` accepts `Box<dyn Node>` instead of `Box<dyn TempoSource>`

**File:** `crates/paraclete-runtime/src/configurator.rs`, lines 110-116

```rust
pub fn add_tempo_source(
    &mut self,
    user_id: u32,
    node: Box<dyn Node>,
) -> (NodeId, u32) {
```

The `TempoSource` trait exists in L2 and `InternalClock` implements it, but
`add_tempo_source` accepts plain `Box<dyn Node>`. Any node can be registered as
a tempo source regardless of whether it implements `TempoSource`. The runtime
stores `(user_id, domain_id)` pairs but never actually queries the node for
`priority()` or `domain_id()` — the `TempoSource` trait is unused by the
runtime.

**Why it matters:** The clock federation feature is silently non-functional. If
a second tempo source is added (e.g. MIDI clock input), there is no priority
enforcement — the last-registered domain wins by accident of topology, not by
`ClockPriority` ordering.

**Fix direction:** Change the signature to `Box<dyn TempoSource>`. Store the
`Box<dyn TempoSource>` alongside the node registration so the runtime can call
`priority()` during clock federation. Rust's trait object coercion requires
`TempoSource: Node` (already satisfied) and `TempoSource: 'static`.

---

## Minor (worth noting but low urgency)

### M-1: Public enums in L2 LGPL API lack `#[non_exhaustive]`

**Files:** `paraclete-node-api/src/event.rs` (`Event`), `paraclete-node-api/src/hardware.rs`
(`HardwareEvent`, `Control`, `DisplayContent`, `DisplayType`), `paraclete-node-api/src/port.rs`
(`PortType`), `paraclete-node-api/src/state_bus.rs` (`StateBusValue`),
`paraclete-node-api/src/capability.rs` (`ParamUnit`)

None of these public enums are marked `#[non_exhaustive]`. Third-party nodes
that `match` on `Event`, `HardwareEvent`, or `PortType` will fail to compile
when new variants are added (which is certain — `PortType::Midi` and
`Event::Sysex` are obvious future candidates). Since this is an LGPL API
intended for third-party use, adding variants without `#[non_exhaustive]` is a
breaking change.

**Fix direction:** Add `#[non_exhaustive]` to every public enum in
`paraclete-node-api`. Do it before third-party code appears — it's a one-line
fix now, a breaking API change later. Note: `Event` is `Copy`, so downstream
exhaustive matches are currently valid; a new variant added without
`#[non_exhaustive]` breaks compilation in all downstream crates.

---

### M-2: `ParamDescriptor::id_for_name` hash collisions have no detection or mitigation

**File:** `crates/paraclete-node-api/src/capability.rs`, lines 63-72

The FNV-1a 32-bit hash is used as the stable parameter ID. Two parameters with
different names that hash to the same 32-bit value would be indistinguishable
to `SetParam` dispatch and parameter-lock routing. With a 32-bit hash space,
collision probability per pair is ~1-in-4-billion — low but not zero over
thousands of parameters across a plugin ecosystem.

No detection (assert at node registration time) or documentation of the
collision risk exists.

**Fix direction:** Add a debug-mode assertion in `CapabilityDocument::from_ports`
or a dedicated validation function: verify all `param.id` values in a node's
`params` list are distinct. This catches hash collisions at node-construction
time in development, before they cause silent misbehavior at runtime.

---

### M-3: `SineOscillator` ignores `sample_offset` of incoming MIDI events

**File:** `crates/paraclete-nodes/src/oscillator.rs`, lines 80-97

```rust
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
    for timed in input.events {
        if let Event::Midi2(ump) = timed.event {
            self.handle_ump(ump);  // applies state change immediately
        }
    }
    // ...then renders all block_size frames from updated state
}
```

All MIDI events are applied before any audio is rendered. A `NoteOn` at
`sample_offset = 32` in a 64-frame block causes the oscillator to start
sounding from frame 0, not frame 32. The timing resolution promised by the
`TimedEvent.sample_offset` field is not honored.

**Why it matters:** Sub-block timing is a key feature of the `TimedEvent`
design. Not using it means the sequencer's precise per-tick timing (up to
960 ppq) is thrown away at the synthesis layer. This is acceptable for P2 but
becomes a correctness issue in P3/P4 when the sampler needs accurate slice
triggering.

**Fix direction:** Split the render loop: iterate frames, applying events at
the correct `sample_offset` and rendering audio between event application
points. This is the standard sample-accurate synthesis pattern.

---

### M-4: `ring_buffer::Inner<T>` is declared `Sync` but SPSC cannot safely be used concurrently

**File:** `crates/paraclete-runtime/src/ring_buffer.rs`, line 24

```rust
unsafe impl<T: Send> Sync for Inner<T> {}
```

`Inner` holds `head` (written by `Sender`) and `tail` (written by `Receiver`).
The `Sync` impl means `&Inner<T>` can be shared across threads. If two threads
hold `&Sender<T>` (which is possible since `Sender: Sync` via auto-impl because
`Arc<Inner<T>>: Sync`), they can call `try_send` concurrently, producing a data
race on `head`.

In practice, `ring_buffer` is `pub(crate)`, `Sender` is not `Clone`, and the
only use is single-producer single-consumer — so the unsoundness is not
exploitable. But a future refactor that stores `Sender` in an `Arc` for sharing
would silently become unsound.

**Fix direction:** Replace `unsafe impl<T: Send> Sync for Inner<T>` with more
targeted implementations: `unsafe impl<T: Send> Send for Sender<T>` and
`unsafe impl<T: Send> Send for Receiver<T>`, without making `Inner` `Sync`.
This expresses the correct invariant: each half can be *moved* to a thread, but
they cannot be *shared* across threads.

---

### M-5: I16/U16 audio format path allocates on every callback with variable-size buffers

**File:** `crates/paraclete-hal/src/audio.rs`, lines 57-83

```rust
let mut f32_buf: Vec<f32> = Vec::new();
device.build_output_stream(
    &config,
    move |data: &mut [i16], _| {
        if f32_buf.len() != data.len() {
            f32_buf.resize(data.len(), 0.0);  // allocates on audio thread
        }
```

The I16 and U16 format callbacks allocate `f32_buf` lazily and resize it if
`data.len()` changes. The first call always allocates. If cpal returns variable-
sized buffers (common with `BufferSize::Default`), every differently-sized
callback allocates.

**Fix direction:** Pre-allocate `f32_buf` with `Vec::with_capacity(max_block *
channels)` before calling `build_output_stream`. Since `BufferSize::Default`
can be queried for a maximum, pre-allocate to that. Better: use
`BufferSize::Fixed` and pre-allocate exactly.

---

### M-6: `SurfaceDescriptor` uses `Vec<Control>` — allocated at construction but never needed at audio rate

**File:** `crates/paraclete-node-api/src/hardware.rs`, `SurfaceDescriptor` struct

`SurfaceDescriptor::controls` is `Vec<Control>`. Each `Control` variant owns a
`PadDescriptor`, `ButtonDescriptor`, etc. — all heap-allocated. This is fine
since `SurfaceDescriptor` is constructed once at init time and never mutated.
However, `PortName` inside each descriptor is `PortName::Dynamic(String)` in
practice, causing many small `String` allocations at startup.

Not a runtime issue but noted since the pattern of using `&'static str` for
descriptor names is inconsistent with `PortName::Dynamic(String)` in tests.

---

### M-7: Duplicate `user_id` registration silently corrupts the graph

**File:** `crates/paraclete-runtime/src/configurator.rs`, `register()`, lines 84-95

If `add_node` is called twice with the same `user_id`, `id_to_index.insert`
overwrites the previous `NodeIndex` with the new one. The old `NodeIndex` is
still present in the petgraph `RuntimeGraph` and the `nodes` HashMap, but it
can no longer be reached via `id_to_index`. The node is leaked (never
deactivated), and if the graph was built with edges referencing the old index,
`build_executor()` includes the orphan node in execution order with no way to
remove it.

**Fix direction:** `register()` should return an error (or panic) if `user_id`
is already registered.

---

### M-8: `PortType::Mono` has no distinct handling anywhere

**File:** `crates/paraclete-node-api/src/port.rs`, line 8

`PortType::Mono` is declared alongside `PortType::Audio`, but the executor
always creates `AudioBuffer::new(2, block_size)` (stereo) regardless of port
type. `Mono` has no distinct treatment in `connect()`, `build_executor()`, or
the audio summing loop. It is either unused or silently treated as stereo.

**Fix direction:** Either remove `PortType::Mono` (use `Audio` with a
`channels` field on `PortDescriptor`), or implement it distinctly. Dead API
surface in L2 that will confuse third-party node authors.

---

### M-9: Executor LED feedback hard-codes assumption that all sequencers control pad 0-15

**File:** `crates/paraclete-runtime/src/executor.rs`, `build_hardware_output()`, lines 121-129

```rust
for pad in 0u32..16 {
    led_updates.push(LedUpdate {
        control_id: pad,
        color: if pad == current_step { RgbColor::GREEN } else { RgbColor::OFF },
    });
}
```

The LED feedback assumes that pad control IDs 0-15 directly map to sequencer
steps 0-15, and that every hardware device (regardless of its `SurfaceDescriptor`)
should receive this update. Multiple hardware devices (e.g., a Launchpad and a
MIDI Fighter) would both receive 16 LED updates intended for a Launchpad grid
layout. A sequencer with `pattern_length > 16` would have steps silently
clipped.

**Why it matters:** This is hardcoded display logic that belongs in a profile
script (L4), not in the L1 executor. The executor should have no knowledge of
pad layouts or visual representation. **Status: addressed in P4 with profile
scripts.**

---

## Confirmed correct (things that look suspicious but are intentional)

**Event routing via `PortType::Clock` treated identically to `PortType::Event`:**
The executor routes both Clock and Event typed edges through the same
`incoming` event vec. This is correct — `TransportEvent` is carried as
`Event::Transport`, not a distinct transport channel. Clock-typed ports are a
semantic annotation (for capability negotiation), not a distinct transport.

**The `unsafe` raw-pointer aliasing in the executor's process loop:**
The `transport_ref` and `slab_ref` raw pointer casts exist to allow `slot.process()`
to be called while `self.transport` and `self.extended_event_slab` are
simultaneously referenced. Since these two fields are not modified during
`process()` (only read), and the borrow checker cannot reason about
field-disjoint borrows through `&mut self`, this is a legitimate and documented
use of raw pointers. The `SAFETY` comments are accurate.

**`EventOutputBuffer` panics instead of reallocating when capacity is exceeded:**
The `assert!` before `Vec::push` ensures the pre-allocated capacity is never
exceeded and thus `push` never reallocates. This is the correct pattern for
audio-thread event buffers — fail loudly in development rather than silently
allocating. The capacity (256 events per node per cycle) is generous enough for
real use.

**`InternalClock` starts with `playing: true` and emits `global_start` on first tick:**
This is the Transport Start Model documented in `CLAUDE.md`. The clock emits
`global_start: true` exactly once on the first tick, and `sync_pulse: true`
simultaneously (since bar=1, beat=0, tick=0). The sequencer handles both in
order: `sync_pulse` snaps position (no-op on first tick since position is 0),
then `global_start` sets `playing = true`. Correct.

**FNV-1a for `param_id_for_name`:** 32-bit FNV-1a is deterministic and fast for
the short strings used as parameter names. The collision risk is documented as a
minor finding (M-2) but the algorithm choice itself is appropriate for this use.

**`StateBusSubscription` polling model:** The subscription returns `Option<&Value>`
only when the value has changed since last poll. There is no waker/notify
mechanism — callers must poll. This is intentional for simplicity at P2; async
notification is a future enhancement.

**`midi2` dependency in `paraclete-node-api` (LGPL3 crate):** `midi2` is
MIT-licensed. The LGPL3 boundary at L2 is clean — `paraclete-node-api`'s sole
dependency carries no GPL contamination.
