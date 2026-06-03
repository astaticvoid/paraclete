# P4 Bug-Fix Review

**Scope:** Post-P4 fix set applied to the working tree (see git status)
**Reviewer:** Claude Code (claude-sonnet-4-6)
**Date:** 2026-06-02

---

## Critical Findings

### C-001 — `state_write` with `write_sandboxed` silently breaks all profile state

**Severity:** Critical
**Location:** `crates/paraclete-scripting/src/lib.rs:206`, `profiles/launchpad.rhai:24–26,34–35,43,57,95–96`, `profiles/digitakt.rhai:33`

**Description:**

The `state_write` builtin now calls `write_sandboxed()` instead of `write()`. `write_sandboxed()` only allows writes to paths matching `/node/{id}/param/`. Every `state_write` call in `launchpad.rhai` uses `/script/lp/mode`, `/script/lp/selected`, and `/script/lp/shift`. Every `state_write` call in `digitakt.rhai` uses `/script/selected_track`. None of these paths start with `/node/`, so every write silently returns `Err(...)`, the return value is discarded with `let _ = bus.write_sandboxed(...)`, and the value is never stored.

Consequence: the launchpad profile's mode (trigger/sequence), selected track, and shift state are never persisted. Every `state_read` of those paths returns `()`. The mode switch on SEQ_EDIT press appears to work (the LED lights up) but the persisted string never changes, so the next event handler always sees `mode == ()` rather than `"sequence"` or `"trigger"`. The 16-step render and step toggle paths require a valid mode string to branch correctly — they all take the `mode != "sequence"` branch regardless of the user's intention.

The `/script/` namespace is script-internal state that no node writes and that the executor does not need to protect. The sandbox was designed to prevent scripts from overwriting executor-owned state (`/node/*/state/`) and system paths (`/transport/`, `/hw/`). It was never intended to block script-private namespaces.

**Fix:**

Extend `write_sandboxed()` to also permit the `/script/` prefix (or any prefix that is not `/node/*/state/`, `/transport/`, or `/hw/`). A minimal targeted fix:

```rust
let allowed = path.starts_with("/script/")
    || (path.starts_with("/node/") && {
        let after_node = &path["/node/".len()..];
        after_node.find('/').map(|slash| {
            after_node[slash + 1..].starts_with("param/")
        }).unwrap_or(false)
    });
```

Alternatively, invert the logic: block known protected prefixes rather than allowlisting a narrow set. The sandbox comment in `state_bus.rs` already documents the intent: "Scripts may not write to `/node/*/state/` (written by the executor) or to `/transport/` or `/hw/` paths." The fix should implement that stated intent rather than a narrower allowlist.

---

### C-002 — Gateway redesign breaks the Digitakt profile handler permanently

**Severity:** Critical
**Location:** `profiles/digitakt.rhai:23`, `crates/paraclete-app/src/main.rs:111–116`

**Description:**

The gateway redesign correctly assigns each device its true `device_id` — `ID_DIGITAKT` (103) for the Digitakt gateway. The Digitakt profile, however, registers its `on_hw_event` handler under `LP_DEVICE_ID` (102 or 101), not `DT_DEVICE_ID`. The dispatch loop in `ScriptingEngine::dispatch_hardware_event` only delivers an event to handlers whose registered `device_id` matches `msg.device_id`. Digitakt encoder events now arrive tagged with `DT_DEVICE_ID = 103`, and no handler is registered for that ID, so they are silently discarded.

The comment at the top of `digitakt.rhai` (lines 13–17) documents this as "a P5 fix" and instructs "register under `LP_DEVICE_ID` to catch them until the gateway fix lands." That workaround was written for the old broken gateway (where all events arrived tagged with the Launchpad's ID). The gateway fix has now landed, so the workaround must be removed and the handler must be updated to use `DT_DEVICE_ID`.

**Fix:**

Update `profiles/digitakt.rhai` line 23:
```
on_hw_event(DT_DEVICE_ID, |event| {
```

and remove the outdated explanatory comments (lines 13–17).

---

### C-003 — Sampler subscriptions in launchpad profile use stale `/samp/` paths

**Severity:** Critical
**Location:** `profiles/launchpad.rhai:141–142`

**Description:**

The M-010 fix renamed sampler published state paths from `/node/{id}/samp/trig` and `/node/{id}/samp/last_note` to `/node/{id}/state/trig` and `/node/{id}/state/last_note`. The launchpad profile subscribes to the old paths and reads the old paths (lines 141–142). Since the sampler no longer publishes to `/samp/`, the subscription callback is never fired and the `state_read` always returns `()`. The sampler play log (`SAMPLER_PLAY track=...`) is never emitted, and the trigger-mode flash that was to be driven by these subscriptions never occurs (it is currently driven by the sequencer `last_trig` subscription instead, but the sampler note information is lost).

**Fix:**

Update `profiles/launchpad.rhai` lines 141–142:
```
subscribe(`/node/${samp_id}/state/trig`, |count| {
    let note = state_read(`/node/${samp_id}/state/last_note`);
```

---

## Significant Findings

### S-001 — Filter `update_coefficients` not triggered on resonance change

**Severity:** Significant
**Location:** `crates/paraclete-nodes/src/filter.rs:131–135`

**Description:**

`process()` detects parameter changes by comparing only `PARAM_CUTOFF` before and after `bank.handle_commands()`. If only `PARAM_RESONANCE` changes (e.g., from a Digitakt encoder turn on enc_id=2 sending `CMD_BUMP_PARAM`), `update_coefficients()` is never called, so `q_coeff` is stale from the previous `activate()` or cutoff-triggered update. The resonance encoder effectively does nothing until the next cutoff change. This is a regression introduced by the cutoff-hoisting fix (the old per-sample coefficient recalculation computed both `f` and `q` each sample).

**Fix:**

Track the previous value of both parameters:
```rust
let prev_cutoff = self.bank.get(PARAM_CUTOFF);
let prev_res    = self.bank.get(PARAM_RESONANCE);
self.bank.handle_commands(input.commands);
if (self.bank.get(PARAM_CUTOFF) - prev_cutoff).abs() > 0.5
   || (self.bank.get(PARAM_RESONANCE) - prev_res).abs() > 1e-4
{
    self.update_coefficients();
}
```

---

### S-002 — Gateway tests do not exercise the `process()` path

**Severity:** Significant
**Location:** `crates/paraclete-nodes/src/gateway.rs:100–123`

**Description:**

Both `gateway_tags_events_with_configured_device_id` and `consumer_drain_collects_pushed_events` push events directly into `gw.tx` (bypassing `process()`) and then drain via the consumer. This does not test the actual integration path: a `HardwareEvent` arriving via `ProcessInput::events` being tagged with `self.device_id` and forwarded through the SPSC. The tests would pass even if `process()` contained a bug or was entirely empty.

A real integration test should construct a `TimedEvent` containing `Event::Hardware(hw_ev)`, feed it through `process()`, and assert the consumer drains an `HardwareEventMsg` with the correct `device_id`.

**Fix:**

Add a test that calls `gw.process(&input, &mut output)` with a fabricated `ProcessInput` containing an `Event::Hardware` event, then asserts the consumer drains an event with the expected `device_id` and content.

---

### S-003 — Sequencer sync-pulse NoteOff emitted before gate close guard runs

**Severity:** Significant
**Location:** `crates/paraclete-nodes/src/sequencer.rs:159–165`

**Description:**

In `handle_transport`, the sync-pulse branch (lines 145–177) is evaluated before the main gate-close guard (lines 198–202). When `new_tick == 0 && self.playing && step.active && self.gate_open`, the code correctly emits NoteOff then NoteOn (via `emit_note_off` at line 163, then `emit_note_on` at line 165). However, the main-loop gate-close check at line 198–202 (`if self.gate_open { if self.step_tick >= gate_ticks { emit_note_off }`) then runs immediately after `handle_transport` returns for the same event. Because `emit_note_on` sets `self.gate_open = true` and `self.step_tick` is `0` at that point, the gate-close check evaluates `0 >= gate_ticks`. For a step with `length = 0.75` and `ticks_per_step = 240`, `gate_ticks = 180` — so `0 >= 180` is false and no spurious NoteOff is emitted. This appears safe for typical step lengths, but for a step with `length = 0.0` (`gate_ticks = 0`), `0 >= 0` is true and a spurious NoteOff immediately follows the NoteOn in the same cycle.

More importantly: `handle_transport` is called for every `TransportEvent` in the event loop, which is the correct path. The code structure is sound for the non-zero-length case, but the edge case with `length = 0.0` steps produces a NoteOn/NoteOff pair in the same cycle, which is acoustically equivalent to silence. This was likely not the intent.

**Fix:**

Guard `gate_ticks = 0` at the close check, or document that `length = 0.0` is a silent/muted step and validate at `set_step()` time.

---

### S-004 — `debug_print` builtin opens a file on the audio-adjacent main thread every call

**Severity:** Significant
**Location:** `crates/paraclete-scripting/src/lib.rs:322–331`

**Description:**

The `debug_print` Rhai builtin opens `/tmp/paraclete-debug.log` with `OpenOptions::new().create(true).append(true)` on every invocation. The launchpad profile calls `debug_print` from inside hardware event handlers and subscription callbacks (lines 40, 44–45, 121, 127, 154). These run on the main loop thread each 1 ms tick. Each `debug_print` call performs a syscall (open + write + implicit close) on the main thread. While not an audio-thread violation, it is blocking I/O on the real-time-adjacent loop that drives `process_main_thread()`, `tick()` handles, and subscription callbacks. Under sustained pad input it will noticeably slow the main loop. The fix to `send_cmd` (removing its file I/O) was correctly applied, but `debug_print` remains.

**Fix:**

Replace the per-call `open()` with either a one-time file handle kept in `ScriptState`, or redirect to `eprintln!` (as the `--dev-ui` path already does). Alternatively, gate `debug_print` behind a compile-time or runtime flag so it is a no-op in production.

---

## Minor Findings

### M-001 — `node_id` field in `ScriptingGatewayNode` is stored but never used

**Severity:** Minor
**Location:** `crates/paraclete-nodes/src/gateway.rs:36,51,63–65`

**Description:**

`ScriptingGatewayNode` stores `node_id` (set via `set_node_id()`) but never reads it. Since `process()` does not publish state and does not use the node ID for routing, the field is dead. This is not a correctness issue, but it is dead code that will generate a Clippy warning.

**Fix:**

Prefix the field with `_node_id` or remove it and implement `set_node_id` as a no-op (`fn set_node_id(&mut self, _id: u32) {}`).

---

### M-002 — Stale comment in `digitakt.rhai` documents the old broken gateway behaviour

**Severity:** Minor
**Location:** `profiles/digitakt.rhai:13–17,20–22`

**Description:**

The comments at lines 13–17 and 20–22 describe a workaround for the old fan-in gateway bug ("all gateway events are currently tagged with the first registered device's node_id"). This workaround is what causes C-002 above, and the comment misleads future readers into thinking the workaround is still appropriate. After fixing C-002 by registering the handler under `DT_DEVICE_ID`, the comments should be removed.

---

### M-003 — `ProcessOutput::cv_mut` and siblings call `find_output` before panicking

**Severity:** Minor
**Location:** `crates/paraclete-node-api/src/context.rs:126–149`

**Description:**

The signal output helpers (`cv_mut`, `phase_mut`, `logic_mut`, `pitch_mut`, `modulation_mut`) each call `self.find_output(port_id, kind)` (which may panic if no slot exists), and then immediately call `unimplemented!("signal output views land at P5")`. The `find_output` call is wasted work — if no slot is wired, it panics before reaching the `unimplemented!`. If a slot is wired, it panics at `unimplemented!`. The code can never succeed. A node author attempting to call `modulation_mut()` on a genuinely wired output port will get a panic with a misleading message ("no signal output with port_id N") rather than the intended "not yet implemented" message.

**Fix:**

Replace the body of each method with just `unimplemented!("signal output views land at P5")`, removing the `find_output` call entirely. This is cleaner and gives the correct error message.

---

### M-004 — `StateBusProxy::write` uses `write_sandboxed` but takes `f64` only

**Severity:** Minor
**Location:** `crates/paraclete-scripting/src/lib.rs:381–383`

**Description:**

The legacy `StateBusProxy::write` method (used by old-style scripts that receive `state_bus` as a scope variable) wraps its `f64` argument as `StateBusValue::Float` and calls `write_sandboxed`. This is correct for `/node/{id}/param/` paths, but the method signature takes only `f64`, preventing the legacy API from writing `Bool` or `Int` values. This is a pre-existing limitation but worth noting since the new `state_write` builtin gained full type support in this change set.

---

### M-005 — Digitakt `decode_relative_delta` match has unreachable arm

**Severity:** Minor
**Location:** `crates/paraclete-hal/src/digitakt/mod.rs:94–99`

**Description:**

The match arms cover `1..=63`, `65..=127`, `0 | 64`, and `_`. Since the input is `u8` (range 0–255) and the first three arms cover 0–127 exhaustively, the `_ => 0` arm is only reachable for values 128–255. A MIDI CC value is 7-bit and should never exceed 127, but the raw byte from the MIDI callback is not validated before reaching this function — if the MIDI driver ever delivers a malformed byte > 127 (which some hardware does), `_ => 0` silently swallows it. The comment says "values > 127 are not valid 7-bit MIDI CC" which is correct, but silent acceptance is less debuggable than a `debug_assert!(value <= 127)` before the match.

---

## Summary

### Fix Correctness

| Change | Correct? | Notes |
|--------|----------|-------|
| Gateway redesign: one-per-device | Architecturally correct | Breaks digitakt.rhai handler registration (C-002) |
| `state_write` → `write_sandboxed` | Incorrectly narrow allowlist | Breaks all `/script/` path writes (C-001) |
| Sampler `/samp/` → `/state/` paths | Correct in node | launchpad.rhai still uses old paths (C-003) |
| Sampler: remove blocking file I/O from `trigger_voice` | Correct | |
| Sampler: remove `node_locks.clear()` from `release_voice` | Correct | Node-level locks are now cleared at the top of each `process()` cycle, which is the right place |
| Sequencer sync-pulse NoteOff | Correct for normal step lengths | Edge case with `length=0.0` produces silent NoteOn/NoteOff pair (S-003) |
| Filter sin() coefficient | Correct — Chamberlin SVF textbook form | |
| Filter `filter_type` hoisted before loop | Correct | Resonance changes do not trigger `update_coefficients()` (S-001) |
| `#[derive(Clone, Debug)]` on hardware types | Correct |  |
| Signal port stub documentation | Clear — "Not yet implemented (P5)" | Output stubs still call `find_output` before panicking (M-003) |
| Digitakt `decode_relative_delta` fallback arm | Correct | See M-005 |
| Distortion comment on sequential param IDs | Correct and useful | |

### Priority Order for Follow-Up

1. **C-001** — `write_sandboxed` must permit `/script/` paths or the entire launchpad profile mode-switching is broken.
2. **C-002** — `digitakt.rhai` must register its handler under `DT_DEVICE_ID` now that the gateway assigns the correct ID.
3. **C-003** — Sampler subscription paths in `launchpad.rhai` must be updated from `/samp/` to `/state/`.
4. **S-001** — Resonance parameter changes must trigger `update_coefficients()`.
5. **S-002** — Gateway tests must exercise the `process()` path.
6. **S-004** — `debug_print` file I/O is blocking the main loop.
