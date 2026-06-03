# Paraclete P4 Code Review

Reviewed: 2026-06-02  
Branch: main (post-P4, pre-P5)  
Scope: all modified and new files per git status

---

## Critical Findings (C-series)

### C-001 — Audio-thread filesystem I/O in `Sampler::trigger_voice()`

**Severity:** Critical — guaranteed audio thread violation  
**Location:** `crates/paraclete-nodes/src/sampler.rs:191–196`

`trigger_voice()` is called from `process()` on the audio thread. It opens and writes to `/tmp/paraclete-debug.log` via `std::fs::OpenOptions::new()` on every note trigger. Filesystem I/O is a blocking syscall; on a loaded system it can stall for tens of milliseconds. This will cause audible glitches (xruns) and violates the audio thread contract stated in the CLAUDE.md architecture rules.

```rust
// Inside process() → trigger_voice():
if let Ok(mut f) = std::fs::OpenOptions::new()
    .create(true).append(true).open("/tmp/paraclete-debug.log")
{
    use std::io::Write as _;
    let _ = writeln!(f, "[TRIGGER] ...");
}
```

**Impact:** Xruns under any I/O load. Will manifest as dropouts on real hardware.  
**Fix:** Remove this debug path entirely, or move it to the StateBus (which the main thread can log from). The trigger count is already published via `published_state()` — that is sufficient for observability.

---

### C-002 — `state_write` builtin bypasses sandbox — scripts can write any path

**Severity:** Critical — security/architecture violation  
**Location:** `crates/paraclete-scripting/src/lib.rs:188–208`

The `state_write(path, value)` Rhai builtin calls `bus.write()` (the unsandboxed method) instead of `bus.write_sandboxed()`. This means scripts can write to any path — `/transport/bpm`, `/hw/anything`, or any other path — bypassing the explicit sandbox constraint enforced for the legacy `StateBusProxy::write()` (line 397).

```rust
// Unsandboxed — scripts can write ANYWHERE
bus.write(path, sv);

// Sandboxed — only /node/{id}/param/ allowed
let _ = self.handle.borrow_mut().write_sandboxed(path, StateBusValue::Float(value));
```

The regression tests at lines 751–757 test the legacy `state_bus.write()` path (through `StateBusProxy`), not the new `state_write()` builtin. The sandbox test passes but does not cover the actual builtin that Rhai scripts use.

**Impact:** A profile script can overwrite `/transport/bpm`, `/node/{id}/state/steps`, or any other internal path. This undermines the separation between executor-owned state and script-accessible state.  
**Fix:** Change line 206 from `bus.write(path, sv)` to `let _ = bus.write_sandboxed(path, sv);`. Add a test for `state_write("/transport/bpm", 140.0)` (must not persist) and `state_write("/node/1/param/x", 1.0)` (must persist).

---

### C-003 — `Sampler::release_voice()` clears `node_locks` unconditionally

**Severity:** Critical — state management bug causing param-lock bleeding across notes  
**Location:** `crates/paraclete-nodes/src/sampler.rs:229`

`release_voice()` ends with `self.node_locks.clear()`. This means receiving a NoteOff for *any* note clears the current cycle's `node_locks`, even if other voices are still active with pending locks, or if additional NoteOn events in the same cycle are yet to be processed.

The node-level `node_locks` cleared at the top of `process()` (line 314) is the correct location for cycle-scoped lock clearing. The redundant clear in `release_voice()` is incorrect: if a step fires both a NoteOff (for the previous note) and a ParamLock+NoteOn (for the current note), and events arrive in order (ParamLock → NoteOn → NoteOff), the NoteOff clears the locks that should govern the NoteOn. Events within a cycle are sorted by priority (ParamLock=0 before NoteOff=2 before NoteOn=2), but NoteOff and NoteOn share the same priority bucket (both are Midi2=2).

**Impact:** Intermittent param-lock skipping — noticeably wrong filter cutoff/pitch behavior on polyphonic or tightly-timed patterns.  
**Fix:** Remove `self.node_locks.clear()` from `release_voice()`. The per-cycle clear at line 314 is correct and sufficient.

---

### C-004 — `ScriptingGatewayNode` always tags events with `device_ids[0]` regardless of which port they arrived on

**Severity:** Critical — multi-device event routing is broken by design  
**Location:** `crates/paraclete-nodes/src/gateway.rs:103–105`

With multiple hardware devices connected to the gateway, all events — from Launchpad, Digitakt, and Keystep — are tagged with `self.device_ids.first().copied()`, i.e. the first registered device's ID. This means Digitakt encoder events dispatched to the scripting engine will carry the Launchpad's device_id. Profile handlers that filter by `device_id` (as all three profiles do) will receive events intended for other devices.

The comment at line 102 acknowledges this limitation ("Full per-port routing requires ProcessInput to carry port_id metadata (P5)") but the current behavior is silently wrong rather than producing an error or a warning.

**Impact:** In a multi-device configuration (Launchpad + Digitakt both connected), all hardware events are dispatched as if they came from the Launchpad. Digitakt encoder movements go unrouted. Keystep notes fire Launchpad script handlers.  
**Fix (for P4 without ProcessInput port metadata):** Remove the multi-device fan-in from ScriptingGatewayNode. Instead, create one gateway node per device, each with a dedicated SPSC. The scripting engine receives from each independently, with the source device_id set on construction. This is architecturally correct per ADR-018 (each node in the graph handles one concern). If a single fan-in gateway is desired for P5+, require ProcessInput to carry port_id and implement routing then.

---

## Significant Findings (S-series)

### S-001 — Audio-thread filesystem I/O in `send_cmd` Rhai builtin

**Severity:** Significant — audio-adjacent blocking I/O  
**Location:** `crates/paraclete-scripting/src/lib.rs:216–229`

The `send_cmd` Rhai builtin (called from the main thread) writes to `/tmp/paraclete-debug.log` on every invocation — every pad press that triggers a sequencer command. This is main-thread, not audio-thread, so it will not cause xruns. However, at 140 BPM with 8 tracks, `send_cmd` is called hundreds of times per second during active use. File I/O that happens on every call will cause the main loop to become I/O-bound, delaying `process_main_thread()` → LED ticking and state bus draining.

**Impact:** Main loop jitter causing delayed LED feedback and sluggish script response.  
**Fix:** Gate behind a compile-time feature flag (`#[cfg(feature = "debug-log")]`) or remove entirely. The production binary should not write to the filesystem in a hot path.

### S-002 — `set_constant()` is a no-op with a misleading implementation

**Severity:** Significant — dead/misleading public API  
**Location:** `crates/paraclete-scripting/src/lib.rs:489–511`

`ScriptingEngine::set_constant()` pushes a dummy NodeCommand (target 0xFFFF_FFFE, all zeros) into `pending_commands`, then calls `let _ = value; let _ = key;`, completely ignoring its arguments. A private method `pending_constants()` is marked `unreachable!()`. The actual constant injection works through `eval_file()`'s `constants: &[(String, Dynamic)]` parameter.

The presence of the public `set_constant()` method on the L4 boundary is a trap for contributors: calling it appears to set a constant but does nothing except poison the pending_commands queue with a bogus entry (which the test at line 783 has to filter out).

**Impact:** Any caller of `set_constant()` will be silently ignored. The dead dummy command in pending_commands is wasteful and confusing.  
**Fix:** Either implement `set_constant()` correctly (store to a `Vec<(String, Dynamic)>` field, inject in next `eval_file()` call), or remove it entirely and document that constants are injected via `eval_file(constants)`. Remove `pending_constants()` and the dead NodeCommand push.

### S-003 — `MixNode::published_state()` returns nothing — step positions invisible to scripts

**Severity:** Significant — missing observable state  
**Location:** `crates/paraclete-nodes/src/mix.rs` (no `published_state` implementation)

`MixNode` has per-input gain parameters controllable via `CMD_SET_PARAM`, but publishes no state to the StateBus. Scripts have no way to read current gain values or confirm a `send_cmd` took effect. This is a design gap rather than a bug, but for a mixing node that profile scripts are expected to control, it matters.

**Fix:** Add `published_state()` to `MixNode` reporting the current per-input gain and master gain.

### S-004 — `ProcessInput::cv()` / `phase()` / `logic()` / `pitch()` / `modulation()` always return empty buffers

**Severity:** Significant — declared-but-unimplemented public API in L2  
**Location:** `crates/paraclete-node-api/src/context.rs:71–119`

All five signal input accessors iterate the `signal_inputs` slot list but discard the found slot (`let _ = slot;`) and always return the `OnceLock` silent buffer. Signal ports are declared and type-checked at connection time, but reading them in `process()` always returns silence. Similarly, `ProcessOutput::cv_mut()` etc. call `find_output()` but then call `unimplemented!()`.

These stubs are on the LGPL3 L2 boundary. Third-party node authors who try to use signal ports will get silent/panicking behavior with no compile-time indication that the feature is not yet implemented.

**Impact:** Any third-party node written against the current L2 API that uses signal ports will silently receive zeros or panic. The API surface implies functionality that does not exist.  
**Fix:** Two options: (a) Add `#[doc = "Not yet implemented — returns silence until P5."]` comments and stabilize as a known P4 limitation; or (b) gate these methods behind a `#[cfg(feature = "signal-ports")]` feature flag so callers get a compile error until P5. At minimum, document the stubs explicitly in the type-level doc.

### S-005 — `LaunchpadNode::process()` takes a `Mutex` on the audio thread

**Severity:** Significant — Mutex contention risk on audio thread  
**Location:** `crates/paraclete-hal/src/launchpad/mod.rs:342`

`LaunchpadNode::process()` calls `self.incoming.try_lock()`. If the MIDI callback thread holds the lock at the same moment, `try_lock()` returns `Err` and the incoming event is silently dropped. This is the standard SPSC-via-Mutex pattern for MIDI input, and `try_lock()` is correct (non-blocking). The risk is event loss under high MIDI load, not blocking.

However, the use of a `Mutex<VecDeque>` is architecturally inferior to an `rtrb` SPSC (already in the dependency tree for `hw_out_tx`). The MIDI callback thread pushes; the audio thread pops — a lock-free SPSC is strictly better here.

Same pattern applies to `DigitaktMidiNode` and `KeystepNode`.

**Impact:** Rare event loss at the cost of a trylock on every audio cycle. Not a P4 blocker, but worth tracking for P5.  
**Fix (P5):** Replace `Arc<Mutex<VecDeque>>` with an `rtrb` SPSC in all three hardware nodes. The callback pushes; `process()` pops. Matches the `hw_out_tx` pattern already in `LaunchpadNode`.

### S-006 — `FilterNode::process()` calls `self.bank.get()` inside the sample loop

**Severity:** Significant — unnecessary linear scan per sample  
**Location:** `crates/paraclete-nodes/src/filter.rs:100`

`svf_sample()` calls `self.bank.get(PARAM_FILTER_TYPE)` on every sample in the block (up to 512 times per cycle). `ParameterBank::get()` does a linear scan over the slots vec. The filter type is constant within a block and already known before the loop; computing it per-sample is wasteful.

**Impact:** Wasted CPU — minor for `< 32` params (as noted in `ParameterBank` docs), but unnecessary given the structure.  
**Fix:** Hoist `let filter_type = self.bank.get(PARAM_FILTER_TYPE) as u32;` before the per-channel loops, pass it to `svf_sample()` as a parameter.

### S-007 — `Sequencer::handle_transport()` fires step on `sync_pulse` without checking `gate_open` / emitting `note_off` first

**Severity:** Significant — voice steal not handled at sync boundary  
**Location:** `crates/paraclete-nodes/src/sequencer.rs:159–174`

When `sync_pulse` fires at `new_tick == 0` with an active step, the code calls `emit_note_on()` without first checking if `self.gate_open` is true and emitting a `note_off`. If a note is currently sounding when the bar-boundary sync occurs (e.g. a long gate from step 15 is still open), the sampler receives a second `NoteOn` without a preceding `NoteOff` for the old note.

**Impact:** For the current sampler, this triggers an extra voice (voice steal behavior), which may be acceptable, but it leaves the `gate_open` state tracking inconsistent — the sequencer will not send a `NoteOff` for the original note until the *next* `step_tick >= gate_ticks` check in the normal tick path, meaning a doubled NoteOn with a late/missing NoteOff.  
**Fix:** Before calling `emit_note_on()` in the sync-pulse branch, check `if self.gate_open { self.emit_note_off(sample_offset, output); }`.

### S-008 — `executor.rs`: `published_state()` allocates on every audio cycle

**Severity:** Significant — per-cycle allocation on the audio thread  
**Location:** `crates/paraclete-runtime/src/executor.rs:231–233`

```rust
let entries: Vec<(String, StateBusValue)> = self.nodes.iter()
    .flat_map(|s| s.published_state())
    .collect();
```

`published_state()` is called on every node every cycle, and the results are `flat_map` + `collect()`ed into a new `Vec<(String, StateBusValue)>`. Each call to `Sequencer::published_state()` constructs 6–7 `String` keys via `format!()` and `StateBusValue::Text(steps_bitfield())`. At 44100/512 = ~86 cycles/second with 8 sequencers, this is ~600 heap allocations per second from `published_state()` alone, plus the flat_map Vec.

**Impact:** Steady-state heap allocation on the audio thread. This will cause jitter on constrained allocators (real-time kernels, embedded future targets).  
**Fix (P5):** Two approaches: (a) Only publish state when something changed (add a dirty flag per node, similar to `capabilities_changed()`); (b) Pre-allocate the entries Vec and have nodes fill a `&mut Vec` rather than returning a new one. Approach (a) is simpler and sufficient.

### S-009 — `ScriptingEngine::dispatch_hardware_event()` allocates per-event

**Severity:** Significant — main-loop allocation on every hardware event  
**Location:** `crates/paraclete-scripting/src/lib.rs:572–618`

`dispatch_hardware_event()` calls `self.contexts.keys().cloned().collect()` to get `ctx_names: Vec<String>`. For N active contexts and M events per tick, this is N×M String clones per main loop iteration. At 1ms loop with active Launchpad use (many presses), this adds up.

Additionally, the `handlers: Vec<FnPtr>` inside the loop also collects.

**Impact:** Steady GC pressure on the main thread. Not a correctness issue.  
**Fix:** Cache the context name list; only rebuild when contexts are added/removed. Or iterate `self.contexts.iter_mut()` directly (may require restructuring to avoid the borrow conflict with `script_state`).

### S-010 — `HardwareOutput::empty()` allocates on every audio cycle for legacy-path devices

**Severity:** Significant — unnecessary audio-thread allocation  
**Location:** `crates/paraclete-runtime/src/executor.rs:137–141, 237–239`

`build_hardware_output()` returns `HardwareOutput::empty()` which constructs two empty `Vec`s via `vec![]`. This is then passed to `slot.hw_update(&hw_out)` for every device slot. The P4 comment says "LED output is now owned by the scripting layer" — so for all P4 devices (which use the new output handle path), this `HardwareOutput::empty()` is constructed and passed to `update_output()` where it is a no-op.

**Impact:** Two `Vec::new()` allocations per audio cycle (inside `HardwareOutput::empty()`).  
**Fix:** Pre-allocate a single `static_empty_hw_out: HardwareOutput` in the executor and reuse it, or check whether any device uses the legacy path before constructing the struct.

---

## Minor Findings (M-series)

### M-001 — `LaunchpadOutputHandle::Drop` calls `thread::sleep` — may block runtime teardown

**Severity:** Minor  
**Location:** `crates/paraclete-hal/src/launchpad/mod.rs:213–218`

The `Drop` impl sleeps for 30ms between SysEx messages. This is fine for graceful shutdown but will block any thread that drops the handle. Since the handle is stored in `NodeConfigurator::output_handles` on the main thread, it blocks main-thread teardown for at least 30ms. Not a correctness issue, but worth noting.

### M-002 — `LaunchpadNode::open()` calls `thread::sleep` during construction

**Severity:** Minor  
**Location:** `crates/paraclete-hal/src/launchpad/mod.rs:267–284`

Three `thread::sleep` calls during `open()`: 50ms + 50ms + 100ms = at minimum 200ms of blocking during startup. This is expected for hardware handshake but not documented at the call site in `main.rs`.

### M-003 — `Sampler::release_voice()` uses `node_locks.clear()` as a semantic for "end of this step's locks"

**Severity:** Minor (see also C-003)  
**Location:** `crates/paraclete-nodes/src/sampler.rs:229`

This is covered by C-003. The additional minor observation: even if the correctness issue from C-003 is accepted as "by design," the clear in `release_voice()` is not documented and its semantics are unclear relative to the process-level clear at line 314.

### M-004 — `DistortionNode` param IDs use raw integer literals, not `id_for_name()`

**Severity:** Minor — inconsistency with the rest of the codebase  
**Location:** `crates/paraclete-nodes/src/distortion.rs:17–19`

```rust
const PARAM_DRIVE:        u32 = 0;
const PARAM_OUTPUT_LEVEL: u32 = 1;
const PARAM_BLEND:        u32 = 2;
```

All other nodes (FilterNode, Sequencer, Sampler) use `ParamDescriptor::id_for_name("name")` for param IDs, which produces stable hashes that survive document reordering and allow generic routing. `DistortionNode` uses raw sequential integers. If the param order changes, or if a profile script targets params by the hash-based ID convention, it will compute a different ID than the node expects.

**Fix:** Use `ParamDescriptor::id_for_name("drive")` etc., and define constants as `pub const PARAM_DRIVE: u32 = ParamDescriptor::id_for_name("drive");` (requires `const fn`, which `id_for_name()` is not currently). Alternatively, use the same `const u32 = 0` pattern but document it as a deliberate departure and add a comment explaining why.

### M-005 — `ParameterBank::from_capability_document()` called at `activate()` with a freshly-allocated `CapabilityDocument`

**Severity:** Minor  
**Location:** `crates/paraclete-nodes/src/distortion.rs:87`, `filter.rs:123`, `sequencer.rs:282`

Each node calls `ParameterBank::from_capability_document(&Self::default_doc())` or `&self.capability_document()` inside `activate()`. `capability_document()` allocates a new `Vec<PortDescriptor>`, `Vec<ParamDescriptor>`, and `Vec<&'static str>` each time it is called. This is fine for `activate()` (called on the main thread, not audio-thread hot path), but it is worth documenting that `capability_document()` allocates.

### M-006 — `decode_relative_delta()` in Digitakt returns 0 for value=64 — center detent is silently dropped

**Severity:** Minor — intentional or bug?  
**Location:** `crates/paraclete-hal/src/digitakt/mod.rs:92–98`

```rust
1..=63  => value as i16,
65..=127 => (value as i16) - 128,
_ => 0,   // catches 0 and 64
```

Value 64 maps to 0 delta. The Digitakt uses 64 as "no movement" (center of relative range), so this is intentional. However, value 0 also maps to 0, which the Digitakt should never send in relative mode. The `_` arm catching both 0 and 64 could be made explicit: `0 | 64 => 0` with a comment, or `64 => 0, _ => 0 /* should not occur */` to make the intent visible.

### M-007 — `FilterNode::update_coefficients()` uses `PI * cutoff / sr` without pre-gain compensation

**Severity:** Minor — DSP note  
**Location:** `crates/paraclete-nodes/src/filter.rs:86`

```rust
self.f_coeff = 2.0 * std::f32::consts::PI * cutoff / self.sr;
```

The Chamberlin SVF `f` coefficient should be `2.0 * sin(pi * cutoff / sr)` for stability at high cutoff frequencies. The linear approximation `2 * pi * cutoff / sr` diverges from `2 * sin(...)` above about `sr/4`, causing the filter to become unstable near Nyquist. The `f_coeff.min(1.0)` clamp in `svf_sample()` provides a crude stability guard, but the filter response at high cutoff will be incorrect.

**Fix:** Use `self.f_coeff = 2.0 * (std::f32::consts::PI * cutoff / self.sr).sin();`

### M-008 — `ScriptingGatewayNode::capabilities_changed()` uses `Cell::replace` side-effect for read

**Severity:** Minor — non-obvious stateful read  
**Location:** `crates/paraclete-nodes/src/gateway.rs:63–65`

```rust
fn capabilities_changed(&self) -> bool {
    self.capabilities_dirty.replace(false)
}
```

`replace()` reads the current value AND resets it to false in one operation. This is a correct pattern for a "take and reset" flag, but it means calling `capabilities_changed()` twice in sequence will return different values — the second call always returns false regardless of the actual state. If the runtime ever calls `capabilities_changed()` more than once per reconfiguration cycle (e.g. for logging or debugging), the flag will be consumed silently.

**Impact:** Low — the runtime currently calls this once per cycle. But it is a surprising API.  
**Fix:** Rename to `take_capabilities_changed()` to signal the consuming nature, or use an `AtomicBool` with `swap(false, Ordering::Relaxed)` if the semantics need to be explicit.

### M-009 — `SurfaceDescriptor`, `Control`, `PadDescriptor` and related hardware types lack `Clone` and `Debug` derives

**Severity:** Minor — L2 ergonomics  
**Location:** `crates/paraclete-node-api/src/hardware.rs:38–116`

`SurfaceDescriptor`, `Control`, `PadDescriptor`, `ButtonDescriptor`, `EncoderDescriptor`, `FaderDescriptor`, `LedDescriptor`, `DisplayDescriptor`, `DisplayType`, `HardwareOutput`, `LedUpdate`, `DisplayUpdate`, `DisplayContent` all lack `#[derive(Clone, Debug)]`. These types appear in `HardwareDevice::surface()` return values, LED output construction, and potentially in GUI code.

The absence of `Clone` means these types cannot be stored in data structures that require it. `DisplayContent::Pixels(Vec<u8>)` in particular would benefit from `Clone`. `Debug` is universally expected on public API types.

**Fix:** Add `#[derive(Clone, Debug)]` to all hardware descriptor and output types. `DisplayContent` may need manual `Clone` if `Box<dyn ...>` fields appear later.

### M-010 — `Sequencer` `published_state()` returns `/node/{id}/state/` paths inconsistently with `Sampler` which uses `/node/{id}/samp/`

**Severity:** Minor — path convention inconsistency  
**Location:** `crates/paraclete-nodes/src/sequencer.rs:301–311`, `sampler.rs:257–260`

The Sequencer uses `/node/{id}/state/{key}` and the Sampler uses `/node/{id}/samp/{key}`. Both claim to be state. The CLAUDE.md spec says the convention is `/node/{id}/state/{key}`. The Sampler's paths should use `/node/{id}/state/trig` and `/node/{id}/state/last_note` to match. Profile scripts that read sampler state will need to use a different prefix, which is undocumented.

**Fix:** Change Sampler paths to `/node/{id}/state/samp_trig` and `/node/{id}/state/last_note`.

---

## Summary

**Overall assessment:** The P4 implementation is functionally sound for the single-device happy path but has two correctness bugs that will surface immediately in multi-device and multi-script configurations (C-002, C-004), one critical performance violation that will cause audio glitches under I/O load (C-001), and one state-management bug that will cause intermittent wrong-note behavior on complex patterns (C-003).

**Must fix before P5:**
- C-001: Remove filesystem I/O from `Sampler::trigger_voice()` — will cause xruns
- C-002: Fix `state_write` to call `write_sandboxed()` — security/architecture violation
- C-003: Remove `node_locks.clear()` from `release_voice()` — param-lock bleed
- C-004: Fix or document `ScriptingGatewayNode` single-device-id routing — all events are tagged as device 0 in multi-device config

**Fix in P5:**
- S-001: Remove `send_cmd` debug logging from main loop hot path
- S-002: Remove or implement `set_constant()`
- S-004: Document signal port stubs as unimplemented
- S-005: Replace `Mutex<VecDeque>` with SPSC in all three hardware nodes
- S-007: Fix sync-pulse NoteOn without preceding NoteOff in Sequencer
- S-008: Make `published_state()` allocation-free or dirty-flagged
- M-007: Fix Chamberlin SVF `f_coeff` formula to use `sin()` instead of linear approximation

**Defer:**
- S-003, S-006, S-009, S-010, M-001 through M-006, M-008, M-009, M-010: cosmetic and ergonomic issues with no correctness impact

**Test coverage:** The P4 test suite is strong. Each new node has unit tests covering DSP basics, parameter clamping, and command handling. The integration test (`eight_track_pattern.rs`) provides meaningful end-to-end coverage. Major gaps: no test for the `state_write` sandbox bypass (C-002), no test for multi-device gateway routing (C-004), no test for `release_voice` + param-lock interaction across NoteOn/NoteOff ordering (C-003). These three test gaps directly correspond to the three critical bugs.
