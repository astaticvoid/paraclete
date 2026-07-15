# Paraclete — Known Bugs

Append-only. Add new bugs at the bottom. Mark resolved with **Fixed:** or **RESOLVED** line and the commit that fixed it.

---

## Status (2026-07-14)

**Actively open:** BUG-027 (engine exonerated by measurement — pending user headphone A/B, see addendum), INFRA-004 (no headless mode — emulator requires TTY).
**Partially resolved:** BUG-012 (sample rate detection shipped; output ring buffer not yet implemented — causes distortion on Linux ALSA with non-multiple block sizes).
**Fixed, pending hardware verification:** (none).
**Trigger-based (fix when named trigger fires):** BUG-003, BUG-006.
**Resolved below:** BUG-001, 004, 005, 007, 008, 009, 010, 011, 013, 014, 015, 016, 017, 018, 019, 020, 021, 022, 023, 024, 025, 026, 028, 029, 030, 031, INFRA-001, INFRA-002, INFRA-003.
**Debug harness:** ADR-033 interactive mode **shipped 2026-07-13 (`92b8795`)** —
the harness-gated audit items below are now reachable without hardware (see the
2026-07-13 entry at the bottom).
**Audit validation rounds 1–3 (2026-07-12):** ADR latent-issue items #1–#8,
#10, #11, #16, #24, #27, #28, #30 validated (see round tables below); the
rest were gated on the ADR-033 interactive harness (now available) or hardware.

---

## Open

### BUG-001 — Step period is 241 ticks (should be 240)

**Severity:** Medium — affects timing precision at all tempos  
**Phase found:** P5  
**Description:** `InternalClock` emits `global_start` on its first tick, consuming one tick before the Sequencer starts. Net result: step period is 241 ticks instead of the intended 240 (at 24 PPQN). Manifests as a systematic ~0.4% BPM error.  
**Location:** `crates/paraclete-nodes/src/internal_clock.rs` — transport start model  
**Fix direction:** Emit `global_start` without consuming a tick, or adjust the step count to account for the extra tick.  
**Fixed:** P10 C0 (`b0cf2c8`) — per the s0 re-diagnosis (see below): increment-then-check tick advance (exactly 240 ticks/step), step 0 fired at `global_start`, bar-sync snap made drift-correction-only. Verified by `step_period_is_240_ticks` and the app-level uniformity gate (+0.0000% measured deviation).

---

### BUG-002 — ConnectionAgreement::baseline() hardcodes sample_rate=44100 and block_size=512

**Severity:** Medium — wrong values when engine runs at non-default config  
**Phase found:** P6  
**Description:** `ConnectionAgreement::baseline()` returns a fixed `sample_rate=44100.0` and `block_size=512` regardless of the actual engine configuration. Any node that relies on the baseline agreement for connection negotiation gets wrong values at other sample rates or block sizes.  
**Location:** `crates/paraclete-node-api/src/agreement.rs`  
**Fix direction:** Pass actual sample rate and block size into `baseline()` from the configurator at `connect()` time.

---

### BUG-003 — paraclete-hal depends on paraclete-runtime — L0→L1 layer violation

**Severity:** Low (functional, architecturally wrong)  
**Phase found:** P4  
**Description:** `paraclete-hal` (L0 HAL) imports `paraclete-runtime` (L1 Runtime) for `StateBusHandle`. The five-layer model requires L0 to only depend on L2 (node-api), not L1.  
**Location:** `crates/paraclete-hal/Cargo.toml`  
**Fix direction:** Move `StateBusHandle` to `paraclete-node-api` (L2) so L0 can depend on L2 directly. Blocked until `StateBusHandle` is suitable for LGPL3 publication.

---

### BUG-004 — Sequencer negative micro_offset behaves identically to zero

**Severity:** Medium — signed micro-timing feature silently non-functional  
**Phase found:** P5  
**Description:** `CMD_SET_STEP_TIMING` stores `micro_offset` as `i8` (±47), but negative offsets (pushing a step earlier) require emitting events across the step boundary — i.e. in the previous step's emission window. The current implementation only delays (positive offset); negative values produce no detectable difference from offset=0.  
**Location:** `crates/paraclete-nodes/src/sequencer.rs` — step emission logic  
**Fix direction:** At step N, if step N's `micro_offset < 0`, emit the step event during step N-1's window (wrap around for step 0).

---

### BUG-005 — Sequencer::serialize() does not save P5 fields

**Severity:** Medium — data loss on project save/load  
**Phase found:** P5  
**Description:** `TrigCondition` (probability, repeat, fill), `StepTiming` (micro_offset), and `swing` are all lost on project save/load. A project that programs conditional trigs or timing offsets silently loses that programming across sessions. P9 adds `cv_locks` serialization but P5 fields remain excluded.  
**Location:** `crates/paraclete-nodes/src/sequencer.rs` — `serialize()` / `deserialize()`  
**Fix direction:** Include P5 fields in the serialised `Step` representation; bump serializer version.  
**Fixed:** P10 C1 (`6212242`) — serializer v3 persists every `Step` field (`TrigCondition`, `StepTiming.micro_offset`) and per-pattern `swing`. Step and pattern records are length-prefixed for forward extensibility (v4 can append fields without a rewrite); counts are plain integers so engine caps can grow without a format bump. v1/v2 blobs still load into `patterns[0]` with defaults for the never-saved fields. Verified by `serialize_roundtrip_preserves_{conditions,timing,swing,cv_locks}` and `deserialize_v2_into_single_pattern`.

---

### BUG-006 — agg_state_buf loses capacity on every send via mem::take()

**Severity:** Low — one reallocation per audio callback after every state bus send  
**Phase found:** P7  
**Description:** The executor sends `agg_state_buf` to the main thread via `mem::take()`, which zeros the field, discarding the allocated capacity. The next cycle starts with a zero-capacity `Vec` and grows again when nodes push entries. The comment "capacity grows back within a few cycles" documents but accepts the repeated reallocation.  
**Location:** `crates/paraclete-runtime/src/executor.rs` — `process_cycle()` state bus send  
**Partial mitigation (P9 C1):** Initial capacity increased from 64 to 256 and per-slot from 8 to 16, reducing the growth cost.  
**Fix direction:** Use a return channel (e.g. a second SPSC) to give back the drained `Vec` to the audio thread after the main thread drains it.

---

### BUG-007 — publish_bank_state() allocates String keys on audio thread every cycle

**Severity:** Low (pre-existing pattern; Sequencer does the same)  
**Phase found:** P9 C1  
**Description:** `publish_bank_state` calls `format!("/node/{}/{}", node_id, name)` per parameter per cycle on the audio thread. `state_bufs.clear()` drops these `String`s each cycle; they are re-created from scratch on the next cycle. The `Vec` outer capacity is retained but String heap data is not.  
**Location:** `crates/paraclete-node-api/src/parameter.rs` — `publish_bank_state()`  
**Note:** The Sequencer's `published_state()` has used the same `format!` pattern since P5. This is an accepted pattern — `published_state()` is not held to the same no-alloc rule as `process()` — but is worth eliminating for large graphs.  
**Fix direction:** Cache formatted path strings in a `Vec<String>` built once at `activate()` time. `publish_bank_state` zips cached paths with `iter_values()` to push pairs with no allocation.  
**Fixed:** W1 C1 (`c9468b9`) — path strings cached in a per-`ParameterBank` `std::sync::OnceLock<Vec<String>>`, built lazily on the first `publish_bank_state()` call and cloned thereafter; `format!` no longer runs on the audio thread after the first cycle. `Sequencer::published_state()` got the same treatment (`OnceLock<[String; 9]>` for its fixed `/state/*` keys; the conditional `track_name` entry stays inline). The residual per-entry `String::clone` is retained by design — the buffer ships owned strings to the main thread (full elimination needs BUG-006's return channel). Verified by `publish_bank_state_allocates_no_paths_after_first_call` (pointer-stability across calls). Note the paths also changed to `/node/{id}/param/{name}` in the same commit (state-path unification).

---

### BUG-008 — set_initial_params re-applied on every activate() — overwrites deserialized state on re-activate

**Severity:** Medium — data loss on project load after dynamic topology change  
**Phase found:** P9 C2  
**Description:** All six ParameterBank nodes that implement `set_initial_params()` store the params map in `pending_initial_params` and apply it inside `activate()`. `activate()` is called again whenever dynamic topology rebuilds the executor (P9 C4). After a project load, `deserialize()` sets the bank to saved values; if `activate()` fires a second time before the next save, `pending_initial_params` is non-empty and overwrites the deserialized values. The pending map is never cleared after first use.  
**Location:** `crates/paraclete-nodes/src/analog_engine.rs`, `fm_engine.rs`, `sampler.rs`, `reverb.rs`, `distortion.rs`, `filter.rs` — `activate()` in each  
**Fix direction:** Call `std::mem::take(&mut self.pending_initial_params)` at the end of `activate()` so the map is cleared after first application. Subsequent `activate()` calls (re-activate) apply no params, leaving deserialized values intact.  
**Fixed:** P10 C0 (`b0cf2c8`) — `mem::take` in all six nodes; regression test `reactivate_does_not_reapply_initial_params`.

---

## Resolved

_(none yet)_

### BUG-009 — DigitaktMidiNode decodes CC as relative; Digitakt II sends absolute

**Severity:** Low (device demoted from encoder role by session-0 finding)  
**Phase found:** Session 0 (July 2026), hardware verification  
**Description:** `decode_relative_delta()` interprets CC values as signed
offsets around 64 (1–63 = CW, 65–127 = CCW). Verified against a real
Digitakt II: encoder output is absolute position CC (values ramp 0–127 and
mirror the internal parameter). The decoder therefore emits large spurious
deltas (e.g. position 0x3B → delta +59). Elektron's MIDI implementation has no
relative encoder mode.  
**Location:** `crates/paraclete-hal/src/digitakt/mod.rs` — `decode_relative_delta()`, `parse_digitakt_midi()`  
**Fix direction:** stop emitting `EncoderChanged` from DT CC input (keep
buttons/transport). If the DT II is ever used as an *absolute* fader-style
source, that is a new, explicitly-absolute mapping decision — not a decoder
fix. Do not add soft-takeover (rejected in controller strategy).

### BUG-001 — Re-diagnosis (Session 0, July 2026)

Measured directly (`paraclete-app/tests/timing_measure.rs`): the claimed
systematic ~0.4% tempo error **does not exist** — long-term mean step period
deviates only +0.011% from nominal. The real shape: the 15 in-pattern steps
each run ~0.39% long (the 241/240 tick period), and the pattern-wrap step
fires ~5.85% EARLY (−322 samples ≈ 7 ms at 120 BPM/44.1 kHz), snapping the
pattern back on-grid. Net tempo is correct; the groove limps once per pattern.
P10 C0's fix target is the per-step 241→240 period; the gated test
`step_intervals_are_uniform_within_pattern` (#[ignore]) is the acceptance
gate.  
**Fixed:** P10 C0 (`b0cf2c8`) — gate enabled and passing; measured +0.0000%.

### BUG-010 — Script errors silently swallowed at three layers (FIXED in session 0)

**Severity:** High (masked every other profile bug)  
**Phase found:** Session 0 (July 2026)  
**Description:** `dispatch_surface_event` handler calls, `on_load()`
invocation, and unmatched device ids in `deliver_script_output()` all
discarded errors (`let _ =` / silent no-match). A crashing profile registered
its early handlers, died mid-`on_load`, and the surface simply went dark with
no diagnostic. Root cause of the "Launchpad shows nothing" session-0 symptom
(combined with BUG-011).  
**Fixed:** session-0 commit — all three sites now `eprintln!` the error.

### BUG-011 — Profile/app assume 8 tracks; 4-track default instrument crashes launchpad.rhai (FIXED in session 0)

**Severity:** High (default out-of-box configuration was broken)  
**Phase found:** Session 0 (July 2026)  
**Description:** `TRACK_SAMP_IDS` is populated only from literal `sampler`
type-tags; the default instrument.yaml uses engine voices, so the array was
empty and `launchpad.rhai` indexed it unguarded (trigger branch + three
`0..8` loops), killing `on_load` mid-registration. Also: `CMD_TRIGGER` (19)
sent by trigger mode is implemented by no engine — trigger-mode sound has
never worked (still open; see fix direction).  
**Fixed (session-0 commit):** `TRACK_GEN_IDS` injected from `ids.generators`;
all profile track loops guarded by `TRACK_SEQ_IDS.len()`.  
**Still open from this finding:** implement `CMD_TRIGGER` in engine nodes (or
a `send_note` Rhai builtin) so pads can live-trigger voices — needed for the
W-track pad-performance path; schedule with W1.

### BUG-012 — Device sample rate and buffer size are assumed, not negotiated

**Severity:** High — wrong pitch/tempo on 48 kHz devices; OOB index or stale
samples on buffer sizes ≠ 512 (release builds have only a debug_assert)  
**Phase found:** Audio-model review (July 2026); escalates/subsumes BUG-002  
**Description:** app hardcodes 44100/512 into `NodeConfigurator::new`; cpal
opens the stream at the device default rate; `NodeExecutor::process()` renders
`self.block_size` frames regardless of the callback buffer length
(`executor.rs:578`).  
**Location:** `paraclete-app/src/main.rs:23`, `paraclete-hal/src/audio.rs`,
`paraclete-runtime/src/executor.rs:578`  
**Fix direction:** query `default_output_config()` before graph build and pass
the real rate through; chunk the cpal callback into internal fixed blocks via
a small ring buffer. Set FTZ/DAZ on the audio thread in the same commit.
Schedule before the first paired session on external audio hardware.

**Empirically confirmed (2026-07-12).** The new load test
(`patch_tests.rs::loadtest_topology_churn_under_live_audio`, run against real
CoreAudio) first ran with `block_size = 256` while this MacBook Air's cpal
default buffer is 512 frames. Result: the audio thread **panicked immediately**
at `executor.rs:612` (`debug_assert_eq!(out_interleaved.len(), block_size *
channels)`) — a hard crash, not graceful degradation. `audio.rs:137` uses
`BufferSize::Default` (device-native) and `:132` takes the device rate, neither
pinned to `block_size`; the app only survives because its hardcoded
`BLOCK_SIZE = 512` happens to equal this device's default. Confirms the
predicted failure mode and raises confidence that a 48 kHz / non-512 interface
at a paired session would crash (debug) or OOB/half-fill (release). The trigger
"first non-44.1 kHz/512 deployment" (roadmap backlog) is therefore a crash, not
a mistuning — worth pulling forward if any external interface is in play.

**Linux ALSA confirmed (2026-07-14).** Running on Linux (wlp4s0, ALSA backend),
the device delivers 9408 samples per callback against the hardcoded 1024-sample
block size. In debug mode this panics at `executor.rs:695`
(`debug_assert_eq!(out_interleaved.len(), frames * channels)`, left=9408 right=2048).
In release mode the mismatch causes audible distortion (audio-rate LFO effect,
confirmed by user in paired W2 session). The server and web UI remain functional
but audio output is unusable on this hardware. Same root cause — `BLOCK_SIZE`
hardcoded, cpal uses device-default buffer. Confirms this is not macOS-specific;
any non-512 device triggers it.

### BUG-013 — Engines ignore event sample_offset: voice starts quantized to block boundaries

**Severity:** High for the instrument's identity — ±11.6 ms trigger jitter at
512/44.1k; P5/P10 micro-timing is inaudible in audio (event timestamps only)  
**Phase found:** Audio-model review (July 2026)  
**Description:** Sequencer emits sample-accurate offsets (verified by the s0
timing harness) but AnalogEngine/FmEngine/Sampler start voices at block start
(`_sample_offset` ignored in `trigger_voice`; retrigger applied before
whole-block render).  
**Location:** `analog_engine.rs`, `fm_engine.rs`, `sampler.rs` process loops  
**Fix direction:** split the render block at event offsets (render pre-event
span with old state, apply event, render remainder). Replace the Sampler's
per-voice `SincFixedOut` with load-time resample + playback Hermite in the
same change (sinc group delay compounds the jitter). **Schedule with P10 C3**
— signed micro-timing must ship audible.

### BUG-014 — Emulator emits PadPressed for scene/control ids where hardware emits ButtonPressed

**Severity:** Medium — emulator-only; SHIFT (65) and SEQ_EDIT (64) are dead
under the terminal emulator because the profile keys them on `ButtonPressed`  
**Phase found:** W0 Commit 1 (July 2026), while defining Theoria's event
mapping  
**Description:** the physical `LaunchpadNode` parses scene (64–71) and
control-row (72–79) presses as `ButtonPressed`/`ButtonReleased`, and
`launchpad.rhai` matches on those event types. The P9.5 `LaunchpadEmulator`
emits `PadPressed`/`PadReleased` for *all* 80 ids (`emulator/mod.rs`
`apply_press`), so scene/control handlers never fire when driving the app
from the keyboard. `TheoriaSurfaceNode` (W0) follows the hardware convention,
which is the correct one — the emulator is the odd one out.  
**Location:** `paraclete-hal/src/emulator/mod.rs` (`apply_press`,
`apply_release`)  
**Fix direction:** emit `ButtonPressed`/`ButtonReleased` for ids ≥ 64 in the
emulator; update the P9.5 emulator tests that assert `PadPressed` for scene/
control ids. One-file change + test sweep; fold into the next emulator-touching
commit or W0 C2 verification if the emulator path blocks the exit criteria.

**RESOLVED** (2026-07-11): `apply_press` and `apply_release` now branch on
`id >= SCENE_BASE` to emit `ButtonPressed`/`ButtonReleased` for scene (64-71)
and control (72-79) ids, matching physical Launchpad behavior. Test
`scene_and_control_emit_button_events` updated to assert `ButtonPressed`.

---

### BUG-015 — FmEngine routes ParamLock through bank.handle_commands (p-lock bleed)

**Severity:** Medium — per-step parameter locks on an FM voice permanently
mutate the ParameterBank, so a locked value bleeds into every subsequent step
until another lock or `CMD_SET_PARAM` overwrites it  
**Phase found:** W1 C0 (July 2026), while adding CMD_TRIGGER — noticed the
existing event loop  
**Description:** CLAUDE.md's ParamLock pattern (ADR-019) is explicit: nodes
supporting per-step locks "must NOT route `ParamLock` events through
`bank.handle_commands()` — that permanently mutates the bank and bleeds the
locked value into subsequent steps." `AnalogEngine` follows the prescribed
`node_locks` pattern (per-cycle overrides cleared each `process()`), but
`FmEngine` converts each `ParamLock` event into a synthetic `CMD_SET_PARAM`
and feeds it to `self.bank.handle_commands(...)` (`fm_engine.rs` ~305–306),
the exact anti-pattern. An FM track with a p-lock on one step hears that value
persist on the following unlocked steps.  
**Location:** `crates/paraclete-nodes/src/fm_engine.rs` — `process()` event
loop (`Event::ParamLock` arm)  
**Fix direction:** mirror `AnalogEngine` — add a `node_locks: Vec<(u32, f64)>`
cleared at the top of `process()`, push `(param_id, value)` in the `ParamLock`
arm, and read via a `get_param()` helper that checks `node_locks` before the
bank. Pre-existing (not introduced by W1 C0); left untouched there to keep the
commit scoped. Fold into the next `fm_engine.rs`-touching commit or a small
standalone fix; add a regression test asserting a locked step does not affect
the next unlocked step.

---

### BUG-016 — State mirror had no on-connect replay (RESOLVED `7e7a39a`)

**Severity:** High — any Theoria client connecting after app startup showed no
BPM, no mode, no param values until each path happened to change again  
**Phase found:** Theoria legibility session (2026-07-10, autonomous)  
**Description:** `AntiphonHandle::pump()` is diff-only (shadow map); kerygma
replays the LED shadow to new clients but nothing replayed the *state* mirror,
so a fresh client's `StateStore` stayed empty for every already-settled path.
Explains the empty transport bar in the session-#1 baseline screenshots.  
**Resolution:** `needs_state_replay` flag per `ClientSlot`; `pump()` services a
full state + context replay before diffing. Regression test
`late_client_receives_full_state_and_context_replay`.

### BUG-017 — enc/enc_push frames dropped: W0 gate never opened at W1 (RESOLVED `7e7a39a`)

**Severity:** High — the W1 touch-encoder row was dead on the wire; every
`{"t":"enc"}` frame died at `route_frame` with `Drop("encoders are W1")`  
**Phase found:** Theoria legibility session (2026-07-10)  
**Description:** W0 gated encoder delivery "until W1"; W1 C3/C4 shipped the
client encoder row and the semantic plane but never lifted the surface-plane
gate, and no profile ever handled `EncoderChanged` from Theoria either.  
**Resolution:** Enc/EncPush route to surface events like pads (test
`route_frame_emits_encoder_events`). The client now prefers the semantic
`bump_param` for context-mapped encoders (see phase report for the spec
conflict on where detent scaling lives).

### BUG-018 — BUMP_DELTA_CAP froze wide-range params (RESOLVED `7e7a39a`)

**Severity:** Medium — `bump_param` deltas were clamped to an absolute ±0.5 in
param units; a 200–8000 Hz cutoff could move at most 0.5 Hz per message  
**Phase found:** Theoria legibility session (2026-07-10)  
**Description:** the cap assumed normalized 0–1 params (universality-directive
class: hardcoded range assumption in a validation path).  
**Resolution:** cap is range-relative — |delta| ≤ (max−min) × 0.5. Test
`bump_param_delta_cap_is_range_relative`.

### BUG-019 — ContextSlot key mismatch: slots 0–7 vs client lookups 90–97 (RESOLVED `7e7a39a`/`e553c62`)

**Severity:** High — the encoder row could never resolve a context slot, so
cells showed "–" even when context was published  
**Phase found:** Theoria legibility session (2026-07-10)  
**Description:** `build_context_frame` emits `enc` = trailing int of the
profile's `encoder_{i}` key (0–7); the web `EncoderRow` looked up slots by
surface control id (90–97). This was the "boring documented choice… may need a
defined map once the web encoder row binds ids 90–97" flagged in W1 C2 — C4
never validated it.  
**Resolution:** semantics defined and documented on both protocol structs:
`ContextSlot.enc` is the encoder SLOT INDEX 0–7; each client maps its own
controls onto slot indexes.

### BUG-020 — Web client layout race: canvas sizing fed back into flex (RESOLVED `e553c62`)

**Severity:** Medium — at desktop viewport the encoder row locked at full
viewport height and the grid rendered 0 px tall; tablet worked only because the
stylesheet won the race  
**Phase found:** Theoria legibility session (2026-07-10)  
**Description:** `EncoderRow.resize()` set an explicit pixel height from
`parent.clientHeight` measured before the stylesheet applied; `min-height:auto`
then kept the flex item at content size forever (ResizeObserver never re-fired
because nothing ever resized).  
**Resolution:** canvases are absolutely positioned inside their containers
(`.encoder-row-container`/`.grid-container` overflow hidden) so JS-set pixel
sizes can never influence flex layout.

### BUG-021 — Hard-error status stomped to "stale": no error screen could ever show (RESOLVED `2daceac`)

**Severity:** Medium — bad-token and protocol-mismatch states rendered as a
permanent STALE overlay instead of their error message; a tablet at the bare
URL had no path forward (driver report 2026-07-10: "if I go to root I just
see stale")  
**Phase found:** Theoria legibility session (2026-07-10), while adding the
code-entry gate  
**Description:** two paths in `connection.ts` overwrote the hard-error
status: the WS `close` handler set "stale" *before* checking `hardError`, and
the 500 ms stale timer armed in `connect()` (cleared only on `welcome`) fired
after the `bye "bad token"` sequence. Verified live: server sent the bye,
client showed STALE.  
**Resolution:** both paths respect `hardError`; `closeHard()` also clears the
stale timer. The bare-URL flow now reaches the TokenGate code-entry screen.

---

### BUG-022 — Sequencer default step note (60) is two octaves above engine trigger reference (36)

**Severity:** High — a step toggled from any surface plays the kick at C4
while a direct CMD_TRIGGER plays C2; the same track sounds like two different
instruments ("seq is very high pitch, params totally disconnected" — s2.md F6)  
**Phase found:** Paired session #2 (2026-07-10)  
**Description:** `Sequencer` steps created by `CMD_TOGGLE_STEP` default to
`note: 60` (`sequencer.rs:141`); `AnalogEngine` initializes `last_note = 36`
("C2 — matches current_hz's initial value", `analog_engine.rs:79`), so
CMD_TRIGGER with arg0 < 0 fires the tuned base while sequenced NoteOns
transpose +24 semitones through `note_to_hz(note, tune)`.  
**Location:** `crates/paraclete-nodes/src/sequencer.rs` (default `Step.note`),
`crates/paraclete-nodes/src/analog_engine.rs` / `fm_engine.rs` / `sampler.rs`
(trigger reference notes)  
**Fix direction:** make the toggle-created default note configurable per
`Sequencer` instance (instrument-file field, e.g. `default_note: 36` for the
drum tracks in `instrument.yaml`) rather than hardcoding either constant —
per-step notes stay full-range (universality). Audit all three engines'
default/reference notes against it in the same commit; add a regression test
that a toggled step and a default CMD_TRIGGER produce the same `current_hz`.

### BUG-023 — Kick ducks on fast re-trigger (strong → quiet → recovers after a pause)

**Severity:** High — live finger drumming on the strongest voice is unusable
("loud strong first hit… quiet if I hit it right after… strong again if I
wait" — s2.md F5; no compressor exists in the graph)  
**Phase found:** Paired session #2 (2026-07-10)  
**Description:** symptom reproducible by ear via CMD_TRIGGER on glass.
Suspects, unranked: `AdEnvelope::trigger()` deliberately retriggers from the
*current* value (`engine_dsp.rs:18-21`, anti-click) so a re-hit mid-decay
skips part of the attack/pitch trajectory; residual `pitch_env` value
shifting the punch sweep window; un-reset SVF state (`svf_low`/`svf_band`)
after a loud hit; interaction with the master `ReverbNode` tail. "Maybe
compression" is confirmed absent — nothing dynamic sits in the path except
`soft_clip`.  
**Fix direction:** measure before touching DSP — headless A/B harness (P10 C0
timing-harness precedent): render two triggers 100 ms apart vs 1 s apart,
compare peak/RMS of the second hit. Fix whatever the measurement convicts;
keep the anti-click property (short linear ramp-down + hard env reset is the
usual correct form). Regression test on the measured ratio.

**BUG-022 RESOLVED** (`5071c9a`): `Sequencer::with_default_note(n)` +
per-sequencer `default_note` in the instrument file (36 on the default
instrument's four tracks). Regression test asserts a sequenced note-36 and a
bare CMD_TRIGGER land on identical `current_hz`.

**BUG-023 STATUS** (`5071c9a`): measured — **the engine is exonerated**.
`probe_rehit_energy_vs_gap` (ignored test, `--nocapture`): re-hit RMS/peak =
99.2–100% of a rested hit at gaps 11 ms → 1 s. Everything downstream in the
graph (MixNode, Freeverb) is linear. Prime suspect is now the playback
environment: macOS built-in-speaker protection limits bass transients with
exactly the reported signature (full → thin on fast re-hit → recovers).
**Next action (driver): A/B on headphones or an interface.** If ducking still
reproduces off-speaker, next step is a full-graph render harness.
`fast_retrigger_is_not_ducked` stays as a permanent engine gate; keep this
bug open until the headphone A/B.

**RESOLVED** (2026-07-11): headphone A/B confirmed — kicks are clean through
headphones, same volume across re-triggers. Engine gate verified by
`fast_retrigger_is_not_ducked` (P10 C0). Speakers produce "POP pow pow pow"
signature consistent with macOS bass transient protection. Not an engine
bug — playback environment artifact.

---

### BUG-024 — state_write inside a subscribe callback panicked the main thread (RESOLVED `3356d60`)

**Severity:** High (latent) — any profile writing state from a subscription
brought the whole app down with "RefCell already borrowed"  
**Phase found:** Theoria baseline-interactions session (2026-07-11)  
**Location:** `crates/paraclete-scripting/src/lib.rs` `process_subscriptions()`
+ the `main.rs` call site  
**Description:** `process_subscriptions()` took `&StateBusHandle`, so the app
held `bus_handle.borrow()` across the entire dispatch. The first subscription
callback that called `state_write` (a `borrow_mut` of the same `RefCell`)
panicked. Latent since P4 — every shipped profile had only ever written state
from event handlers; surfaced the moment `launchpad.rhai` published
`/script/lp/steps_n` from its steps subscription.  
**Fix:** signature takes `&Rc<RefCell<StateBusHandle>>`; the bus is borrowed
only around each path read and dropped before the callback fires. Verified by
a headless WS client observing the steps_n mirror update across live toggles.

---

**BUG-004 RESOLVED** (`e8f7718`, P10 C3): negative `micro_offset` fires the
step during the previous step's emission window, `|offset|` × 10 ticks before
its grid boundary (tick-exact; wrap handled for step 0 via the page-loop
advance). Steps fired directly at their grid position (transport start,
resync) treat "on the grid" as the closest playable time — a negative offset
contributes nothing there. Regression tests
`negative_micro_offset_emits_early` / `negative_micro_offset_step_zero_wraps`.
Partially resolves OQ-6 (signed representation unchanged: i8 ±47).

---

**BUG-013 PARTIAL — engines resolved** (`309a9e6`, 2026-07-11):
AnalogEngine + FmEngine render in spans split at NoteOn sample offsets —
voice starts are sample-accurate (regression tests
`note_on_mid_block_starts_at_its_sample_offset` per engine), velocity is
per-span (`two_notes_in_one_block_keep_their_own_velocities`), and
ParamLocks apply from their offset. **Sampler half remains open:** its
per-voice `SincFixedOut` yields fixed-size chunks and cannot render
arbitrary spans — the fix direction's load-time-resample + playback-Hermite
replacement is the remaining work, a real resampler redesign that should
not be rushed; keep this entry open until it lands.

---

**BUG-015 RESOLVED** (`0ff4fc7`, 2026-07-11): FmEngine now uses the ADR-019
`node_locks` per-cycle override pattern (mirrors AnalogEngine); all
render-path reads go through `get_param()`. Regression test
`param_lock_does_not_bleed_into_bank`.

---

**BUG-013 RESOLVED — Sampler half** (2026-07-11): per-voice `SincFixedOut`
replaced with load-time resample (data always at output rate;
`sample_rate_native` renamed `sample_data_rate` and now correctly tracks the
output rate after load — previously stale at 44100 on non-44.1k devices) +
4-point Hermite playback interpolation (`hermite4`, standard de Soras form,
from scratch). `process()` renders in spans split at NoteOn/NoteOff **and
ParamLock** offsets — the Sampler goes one step beyond the engines here: a
p-lock without a co-timed trig applies from its own offset instead of
leaking backward into the pre-event span (the engines still apply such locks
from block start; latent only, since the Sequencer always emits locks
co-timed with their trig — fold engine parity into the next engine-touching
commit). Hermite taps are clamped to the played region (no slice/loop-edge
bleed); loop wrap emits its post-wrap sample in the same frame (the old
one-sample-per-cycle dropout in the linear path is gone); playback rate
keeps the old resampler's ±4-octave bound and rejects non-finite modulation.
Accepted tradeoff per the fix direction: no playback-side anti-aliasing on
pitch-up (Hermite interpolates, it does not band-limit; the sinc's group
delay was the reason it had to go). Regression tests:
`note_on_mid_block_starts_at_its_sample_offset`,
`note_off_mid_block_stops_at_its_sample_offset`,
`two_notes_in_one_block_keep_their_own_velocities`,
`param_lock_mid_block_applies_from_its_offset`,
`loop_wrap_emits_on_every_frame`, `loaded_sample_rate_tracks_output_rate`.

---

### BUG-025 — Swing/positive micro-timing offsets exceed block_size and lose their sub-block position

**Severity:** High for groove — swing and positive micro-offsets are
quantized to a block boundary, the exact jitter class BUG-013 removed  
**Phase found:** BUG-013 Sampler code review (2026-07-11, verified against
executor + sequencer source)  
**Description:** the Sequencer is clock-tick driven, not block-window gated:
it emits a step's NoteOn in the block where the step's *grid tick* falls,
with `sample_offset = tick_offset + step_sample_offset(step, spb)` — swing
(`swing × samples_per_beat`, e.g. 22 050 samples at 60 BPM/0.5 swing) and
positive micro-offsets (≈2 756 samples for +12 at 120 BPM) routinely exceed
any real block size. The executor delivers events same-block with no
cross-block deferral, and all three instrument nodes clamp with
`sample_offset.min(block_size)` — so a swung note renders nothing in its
delivery block and starts at the *next* block's sample 0, discarding the
sub-block placement the feature exists to produce. Pre-existing (the
sequencer's own test `micro_offset` asserts an emitted offset ~43× the block
size); the engines inherited the clamp in `309a9e6`, the Sampler has had it
since P3.  
**Location:** `crates/paraclete-runtime/src/executor.rs` (event delivery, no
deferral), `crates/paraclete-nodes/src/sequencer.rs` (`step_sample_offset`
emission paths), instrument nodes' `.min(block_size)` clamps  
**Fix direction (decided 2026-07-11): executor defers.** The executor is
the single point of truth for event timing — it will carry future-offset
events across block boundaries via a fixed-capacity per-node ring buffer
(allocation-free; ~16 slots). Events are consumed progressively: each cycle
subtracts `block_size` from deferred offsets until they fit, then delivered
with residual sub-block position. No node needs block awareness. The
`TimedEvent` doc comment ("0..block_size−1") will be corrected — offsets
represent samples from current block start, with cross-block spans legal.
Instrument nodes' `.min(block_size)` clamps become harmless (sub-block
offsets always < block_size after executor defers). The negative-micro path
(BUG-004) is unaffected (block-local early fire).

**RESOLVED** (2026-07-11): implemented per the decision.
- Executor: `deferred_events` per-slot ring buffer; merge at cycle start
  subtracts block_size and delivers events that fit; routing splits
  oversized events into deferred at emit time (no same-cycle pickup).
- Instrument nodes: `.min(block_size)` removed from AnalogEngine, FmEngine,
  Sampler (offsets always < block_size after executor defers).
- Regression tests: `executor_defers_cross_block_offset_event_to_next_block`
  (offset 522 → deferred → delivered at offset 10 in next block),
  `executor_delivers_inblock_event_immediately` (offset 100 delivered in
  current block).

---

### BUG-027 — Reverb init crackle on first audio block

**Severity:** Medium — audible click/pop at session start
**Phase found:** Test-driver session (2026-07-11)
**Description:** Initial crackle heard at the very start of playback ("might be
reverb starting"). Suspect: Freeverb's internal delay-line buffers are
uninitialized (contain whatever was on the heap) and the first `process()` call
reads stale data, producing a click. The buffers should be zeroed in
`activate()`.
**Location:** `crates/paraclete-nodes/src/reverb.rs` — `activate()`, `process()`
**Fix direction:** zero the delay-line buffers in `activate()` or in the first
`process()` call. Small standalone fix; verify with the test driver's
`peak_lt` assertion on the first ~50ms of audio.

---

### INFRA-001 — No automated artifact detection

**Severity:** Medium — pop/click/dropout detection is manual (listen)
**Found:** Test-driver session (2026-07-11)
**Description:** The test driver can assert on peak levels and state bus values
but has no check for audio artifacts: sample-to-sample discontinuities
(>threshold), DC offset, zero-crossing glitches, or repeated-sample dropout.
An agent can verify "audio exists" but not "audio is clean."
**Fix direction:** Add `discontinuity_gte <samples>` assertion to the test
driver that scans the captured buffer for adjacent samples with absolute
difference exceeding a threshold. Also add `dc_offset_lt` and `dropout_gte`
(for detecting repeated zero/hold values). Deferred to ADR-033 v2.

---

### INFRA-002 — Test scenario YAML too verbose for inline testing

**Severity:** Low — developer ergonomics
**Found:** Test-driver session (2026-07-11)
**Description:** Writing a full YAML file for every test scenario is verbose.
The one-liner CLI mode (`--trigger kick --at 1.0 -d 3`) is not yet
implemented. A Rust builder API (`Scenario::new().trigger("kick").at(1.0).run()`)
would enable inline `#[test]` functions without YAML files.
**Fix direction:** Implement CLI one-liner mode (ADR-033 § Quick mode). Rust
builder API deferred.

---

### AUDIT VALIDATION (2026-07-11)

Results from running the test driver against the ADR latent-issue list
(`design/review/adr-latent-issues.md`):

| Finding | Test | Result |
|---|---|---|
| Param setting via ring buffer | `set_param` + state bus probe | **PASS** — decay=0.5, drive=0.0 confirmed |
| Trigger produces audio | `trigger` + `peak_gte` | **PASS** — peak > 0.01 after trigger |
| Audio returns to silence | `peak_lt` between triggers | **PASS** — peak < 0.01 during silence |
| Sequencer step advance | `toggle_step` + probe `current_step` | **PASS** — steps advance correctly |
| Duplicate display_name resolution | "kick" → engine (id 20), not sequencer (id 10) | **PASS** — short type_tag name wins |
| Pattern cue during playback | `set_pattern` + probe `cued_pattern` | **PASS** — cued_pattern = -1 → 1, active stays at 0 |
| Speed multiplier | `set_speed` 0.5 | **PASS** — speed_mult shows 0.5, step timing slows |
| Full signal chain clock→output | Multi-track, 2 sequencers, params, audio peak | **PASS** — audio present across 5s, both tracks advance |
| Parameter persistence via state bus | `set_param` decay=0.3 + probe | **PASS** — value 0.3 confirmed between 0.25-0.35 |

### BUG-026 — Executor event sort is unstable; same-offset NoteOff/NoteOn order is unguaranteed

**Severity:** Medium (latent) — a reorder silently drops a repeated note  
**Phase found:** BUG-013 Sampler code review (2026-07-11)  
**Description:** `executor.rs` sorts each node's incoming events with
`sort_unstable_by_key((sample_offset, priority))`; NoteOn and NoteOff are
both `Event::Midi2` (priority 2), so at an equal offset their relative order
is unspecified. The Sequencer emits `NoteOff` then `NoteOn` at the identical
offset for back-to-back steps (full gate or same-offset early fire); if the
sort ever swaps them, `release_voice` (max `triggered_at`) kills the note
just triggered — the repeated step goes silent. Currently masked by
`sort_unstable`'s small-slice insertion-sort fallback (order-preserving in
practice), which is an implementation detail, not a contract.  
**Location:** `crates/paraclete-runtime/src/executor.rs` event sort  
**Fix direction:** make the intended order explicit — stable sort is the
minimal fix; better, give the priority key a NoteOff-before-NoteOn
tie-break (release-then-retrigger is the musical contract) or a monotonic
sequence field. Small standalone commit + a regression test that feeds the
pair pre-swapped.

**RESOLVED** (2026-07-11): `sort_unstable_by_key` → `sort_by_key` in
`executor.rs:453`. Regression test `stable_sort_preserves_noteoff_before_noteon_at_same_offset`
feeds NoteOff then NoteOn at same offset and asserts delivery order.

---

### INFRA-001 — RESOLVED (`9655cd0`, 2026-07-12)

Artifact-detection assertions landed in the test driver: `discontinuity_lt`
(max adjacent-sample |diff|), `dc_offset_lt` (|mean| of window),
`dropout_lt_ms` (longest bitwise-identical sample run), each windowed by
optional `from`/`until` seconds and evaluated post-capture on the full
buffer. Any non-finite sample (NaN/Inf) in a scanned window fails outright —
NaN defeats ordered comparisons, so it is checked first (pre-commit review
finding). Failure messages report the worst offender's sample index and time.

Naming deviates deliberately from this entry's sketch (`discontinuity_gte`,
`dropout_gte`): the shipped names follow the existing pass-condition
convention (`peak_gte`/`peak_lt`), and the dropout threshold is milliseconds,
not samples. Semantics are otherwise as specified.

Known limitation, documented in `kick_reverb_clean.yaml`: timeline actions
dispatch on wall clock while capture time runs slower (block processing +
sleep per block, ~27% slow in debug builds), so `from`/`until` windows need
margin around wall-clock action times. A captured-samples action clock would
remove the skew — fold into ADR-033 v2 alongside the interactive REPL.

---

### BUG-027 — Addendum: engine exonerated by measurement (2026-07-12)

The suspected cause is wrong: Freeverb's delay lines are already
zero-initialized (`vec![0.0; len]` in `CombFilter::new`, `AllpassFilter::new`,
and the pre-delay buffers in `activate()` — `reverb.rs`). There is no stale
heap data to read.

Measured with the new INFRA-001 assertions (`9655cd0`):

| Scenario | Result |
|---|---|
| Silent 1.5s session start (`session_start_clean.yaml`) | peak < 0.001, max jump < 0.005, DC < 0.001 — **clean** |
| First kick through mix → reverb (`kick_reverb_clean.yaml`) | pre-trigger window clean; no post-trigger dropouts — **clean** |
| Sequencer-driven start, strict thresholds | first 50ms: max jump < 0.0001; worst jump anywhere = 0.0276 at 1.72s (inside a kick transient — musical slope, not a click) |

The crackle is not present in the captured buffer under any of the repro
conditions. Conclusion: playback-side — `afplay`/CoreAudio output-stream
start transient, the same macOS device-behavior class that resolved BUG-023
(speaker protection). The WAV writer was also inspected: header and i16
conversion are correct.

**Status: engine exonerated; pending user confirmation.** Same protocol as
BUG-023: headphone A/B on `afplay` start (does the crackle follow the
speakers or the file?). If confirmed device-side, close as environmental.

---

### BUG-028 — Test-driver omitted trigger note sent 0 instead of engine default

**Severity:** Medium — every trigger render to date was mistuned
**Phase found:** INFRA-002 implementation (2026-07-12), reading the scenario
schema against the CMD_TRIGGER contract
**Description:** `TimelineAction::Trigger` used `#[serde(default)]` for
`note`, so YAML triggers omitting it sent `arg0 = 0` — a valid MIDI note that
retunes the voice (kick: `note_to_hz(0)` ≈ 8 Hz, three octaves below the C2
reference) and poisons `last_note` for subsequent playback. The contract
(ADR-033; `analog_engine.rs`, `sampler.rs`, `fm_engine.rs` all agree) is
`arg0 < 0` = engine default. Every existing test YAML omits `note`, so all
audit/kick trigger renders were mistuned — assertions passed because they
only checked `peak_gte`. Same defect class as BUG-022 (seq-vs-trigger pitch
mismatch).
**Location:** `tools/test-driver/src/scenario.rs`
**RESOLVED** (`4841c55`, 2026-07-12): omitted note now defaults to -1;
verified -1 means "default" in all three trigger-capable nodes. Regression
test `trigger_without_note_defaults_to_engine_default`.

---

### INFRA-002 — RESOLVED (`27974fe`, 2026-07-12)

Quick mode shipped per ADR-033 § Quick mode:
`test-driver --trigger kick --at 1.0 -d 3 [--instrument p] [--output p]
[--no-play]`. Pairs `--trigger`/`--at` positionally; mismatched counts,
flag-like values, negative/NaN times, and unknown flags error with exit 2
before execution; duration defaults to last trigger + 2s.

Spec conflict recorded: ADR-033 claims clap is "already a workspace
dependency via crossterm's transitive deps" — it is not (`clap-sys` is the
CLAP plugin ABI, unrelated). Parsing is hand-rolled to honor the ADR's
no-new-crate intent; CLI shape matches the spec. The Rust builder API
(`Scenario::new().trigger(...)`) remains deferred as this entry stated.

---

### BUG-029 — apply_patch failure stranded the engine paused with no executor

**Severity:** High — one bad TopologyChange permanently silenced audio
**Phase found:** ADR latent-issue audit item #4 (predicted by the review;
confirmed by code reading 2026-07-12)
**Description:** `apply_patch` (ADR-029 pause-rebuild-resume) returned early
via `?` on any failed change — after `take_executor()` had drained the old
executor, but before `rebuild_executor()`/`resume_with_executor()`. The
engine stayed paused with no executor; audio never resumed. No live call
site existed yet (Antiphon wiring pending), so this was latent, not field.
**Location:** `crates/paraclete-app/src/patch.rs`
**RESOLVED** (`32f34a6`, 2026-07-12): the change loop records the first
error and breaks; rebuild + resume always run, then the error is returned.
Partial changes stay applied (documented). Every abort path leaves a valid
DAG — `connect()` removes the offending edge before erroring. Regression
test `apply_patch_failed_change_still_installs_executor`.

---

### BUG-030 — remove_node on a device destroyed it while reporting failure

**Severity:** Medium (latent) — device dropped, error references nonexistent API
**Phase found:** Validating audit item #5 (2026-07-12)
**Description:** `NodeConfigurator::remove_node` removed the slot from
`id_to_index`/`nodes`/graph/`cap_doc_cache` and *then* returned `Err` for
`Device` slots — the surface was silently dropped while the caller was told
to "use remove_surface", a function that does not exist.
**Location:** `crates/paraclete-runtime/src/configurator.rs`
**RESOLVED** (`32f34a6`, 2026-07-12): devices are refused before any
mutation. Regression test `remove_node_on_surface_is_nondestructive`.

---

### AUDIT VALIDATION — round 2 (2026-07-12)

Code-level validation of ADR latent-issue items not needing the interactive
harness (`design/review/adr-latent-issues.md`):

| Item | Finding | Result |
|---|---|---|
| #1 (ADR-025 deserialize ordering) | Both load paths deserialize AFTER activate (`register()` activates at add_node; v2 load documents the order; rebuild_executor does NOT re-activate) | **Implementation correct** — ADR-025's "deserialize before activate" claim is a dead letter; code follows AGENTS.md |
| #2 (ADR-025 hot-reload data race) | `load_project`/`save_project` run only at startup, before `build_executor()` — no audio thread exists yet | **Unreachable today** — becomes live when P11 temp save/reload ships; ADR-025's "engine need not be stopped" claim must be qualified before then |
| #4 (ADR-029 failed patch strands graph) | Confirmed real | **BUG-029, fixed** |
| #5 (ADR-029 cap_doc_cache stale) | `remove_node` evicts the cache | **Clean** — but exposed BUG-030 (destroy-then-error on devices), fixed |
| #8 (event sort) | — | Already resolved as BUG-026 |

Noted for the tracker, ~~not fixed~~ **both RESOLVED 2026-07-12** (see below):
a device-targeted `RemoveNode` inside `apply_patch` reported
`PatchError::NodeNotFound` rather than the device-refusal reason (error
mapping discarded the message); and `remove_node` on an id whose slot is
currently drained into a live executor errored after partially mutating
`id_to_index` (same shape as BUG-030, unreachable via apply_patch which
restores nodes first).

**RESOLVED** (commit follows this doc): `apply_patch` now surfaces the real
refusal reason — a known id whose removal is refused returns
`PatchError::ConfigError(msg)`; only a genuinely absent id is
`NodeNotFound`. The new `NodeConfigurator::contains_node()` distinguishes the
two without string-matching. `remove_node` validates slot residency before
touching `id_to_index`, so a refused removal never half-edits the id map.
Regression tests: `apply_patch_remove_device_reports_refusal_reason`,
`apply_patch_remove_unknown_id_is_node_not_found` (paraclete-app),
`remove_node_on_drained_slot_leaves_id_map_intact` (paraclete-runtime).

---

### AUDIT VALIDATION — round 2 addendum (2026-07-12)

| Item | Finding | Result |
|---|---|---|
| #7 (ADR-007 Rhai infinite loop) | `set_max_operations(500_000)` + call-level/expr-depth caps configured in `paraclete-scripting` — a `while true {}` script aborts with a Rhai error; error surfacing was fixed in BUG-010 | **Clean** — bounded, recoverable |
| #10 (ADR-023 GraphNode inner ParamLock) | `InnerGraphNode::process` ignores `input.commands` and forwards events only to the inner sequencer; outer ParamLock routes by node_id, which never matches inner ids | **Confirmed limitation, already tracked** — roadmap Known Provisional "Inner GraphNode runtime patching", P11+ |
| #16 (ADR-012 output buffers not zeroed) | `slot.audio_out.clear()` (= `fill(0.0)`) runs every cycle before each node processes; master interleaved buffer and signal scratch likewise | **Clean** |

---

### BUG-031 — Swing/micro offsets not scaled by per-track speed

**Severity:** Low (latent) — timing feel drifts from intent only when
per-track speed (P10 C3) and swing/micro-timing (P10 C2) are combined
**Phase found:** ADR latent-issue audit item #27 (2026-07-12)
**Description:** The step *period* (boundary spacing) is speed-scaled —
`exact_period() = ticks_per_step / speed_mult` — but the swing and micro-timing
sample offsets are **beat-anchored**: `step_sample_offset()` computes swing as
`swing_amount * samples_per_beat` and micro as `micro_offset/96 * samples_per_beat`,
both independent of `speed_mult`. So at `speed_mult != 1.0` the offset is no
longer a fixed proportion of the (now shorter/longer) step. At high `speed_mult`
with non-trivial swing the odd-step offset can exceed the shortened step period
and push the swung note past the following step's boundary (ordering overshoot).
No ADR/spec documented the intended composition, so this is a design gap, not a
regression. Both features are live (`swing_amount` refreshed every `process()`
from the active pattern; `speed_mult` set via `CMD_SET_SPEED`).
**Location:** `crates/paraclete-nodes/src/sequencer.rs` — `step_sample_offset()`
vs `exact_period()`
**RESOLVED** (2026-07-12): chose candidate (a) — intra-step offsets are
fractions of a step and scale by `1.0 / speed_mult`. `step_sample_offset` now
computes `swing_amount * samples_per_beat / speed_mult` (and the same for
micro-timing), so the swing feel is constant across per-track speeds and a nudge
can never overshoot the shortened step. Rule documented in ADR-030 (Per-track
speed section); pinned by `swing_offset_scales_with_step_speed`.

---

### AUDIT VALIDATION — round 3 (2026-07-12)

Code-level validation of the remaining ADR latent-issue items reachable
without the interactive harness (`design/review/adr-latent-issues.md`):

| Item | Finding | Result |
|---|---|---|
| #3 (ADR-024 `translate_transport` emits GlobalStart every buffer) | The event is gated on the `!prev_playing && playing` transition; `SubgraphPlugin::process_block` writes back `self.prev_playing = transport.playing` on both branches, and the CLAP `ffi.rs` caller threads it via `prev_playing()`. GlobalStart fires exactly once per start. | **Implementation correct** — the "every buffer" concern does not reproduce |
| #24 (ADR-028 LoopBreakNode → non-cycle consumers get wrong-cycle data) | The executor pre-phase (`back_edges`) writes `loop_break_prev()` into the LB output buffer before the main loop, and the node's own `process()` writes `prev` too; `loop_break_swap()` runs once post-execution. Every consumer — cycle or not — reads the same previous-cycle slice within a cycle. | **Clean** |
| #27 (ADR-030 speed × swing composition) | Step period is speed-scaled; swing/micro offsets are beat-anchored and speed-independent. No documented rule; overshoot possible at high speed. | **Finding — BUG-031 filed** (pinned by test) |
| #28 (ADR-030 cue vs chain precedence) | Boundary logic is `if let Some(cue) = cued_pattern.take() { … } else if !chain.is_empty() { … }` — an explicit cue wins for that one boundary and the chain does not advance (spec 4.2). | **Clean** — deterministic |
| #30 (ADR-031 protocol version mismatch) | `ClientMsg::Hello` carries no protocol field — version is server-authored (`Welcome.protocol`), so a client cannot declare `protocol: 999`. Malformed/unknown client frames hit `evaluate_hello`'s `Reject` path or are dropped, never panic (`protocol_unknown_tag_is_error_not_panic`). | **Not applicable / clean by design** — client-side check of `Welcome.protocol` is a Theoria-client responsibility; note for the W4 freeze |

Remaining harness-gated items (#9, #12, #13–#15, #17–#23, #25, #26, #29) need
the ADR-033 interactive debug harness or hardware and are deferred to it.

---

### INFRA-003 — Self-inflicted audio dropouts are invisible at runtime

**Severity:** High (observability) — the most likely dropout source in our own
code produces silence with no counter, log, or meter
**Phase found:** Debug-posture review (2026-07-12)
**Description:** The audio callback recovers from two conditions by filling the
buffer with silence — an audible dropout — and records nothing:
`audio.rs:159–167` / `188–196`, `if let Ok(guard) = exec_cell.try_lock() { … }
else { data.fill(0.0) }` (lock contended with the executor swap) and the inner
`else { data.fill(0.0) }` (no executor installed). The same silent-drop shape
recurs at `executor.rs:588` (`let _ = state_bus_producer.push(…)` — SPSC full,
state update dropped) and the deferred-event carry at `executor.rs:400`. Only
LED delivery logs a drop (`configurator.rs:607`), and only the first one.
Consequence: a `try_lock` miss (the Mutex→arc-swap trigger in the audio-model
review) or an SPSC overflow (BUG-006, audit #9/#12) can happen in a live session
and leave no trace — so "none of these triggers have fired" is unproven, not
confirmed. cpal *device* xruns are logged via `err_fn`; our own are not.
**Location:** `crates/paraclete-hal/src/audio.rs` (callback), plus the
executor/configurator drop sites above
**Fix direction (minimal, lock-free):** an `AtomicU64` dropout/underrun counter
on `AudioEngine` incremented on each `fill(0.0)` recovery path, plus a
buffers-processed counter, exposed via the deferred `/engine/cpu` state path
(audio-model-review). Same pattern for the state-bus overflow counter (folds in
the BUG-006 profiling need). This is the concrete, buildable half of the
roadmap's "CPU/xrun meter" gap — it does not require the full ADR-033 harness
and unblocks confirming (rather than assuming) the trigger-based backlog.
**Status:** **Fixed (ADR-034 shipped 2026-07-12).** `RuntimeCounters` struct
with four `AtomicU64` counters (`buffers_processed`, `dropout_lock_miss`,
`dropout_no_executor`, `state_bus_overflows`) shared between audio callback and
executor via `Arc`. Published to state bus as `/engine/*` paths each cycle and
mirrored to Antiphon clients at ~30 Hz. Unit-tested in `runtime_integration.rs`.
The deferred-event-carry and LED-drop counters are deferred to the structured-log
channel (separate ADR/infra item).

**Measurement (2026-07-12).** First live read of the counters, closing the D4
"assumed quiet vs measured quiet" gap for the executor path. Scenario
`tools/test-driver/tests/engine_counters_quiet.yaml` runs the full default
instrument busy for 8 s (all four tracks stepping + a mid-run polyrhythm speed
change) and reads `/engine/*` at 7.5 s:

| Counter | Value | Read as |
|---|---|---|
| `buffers_processed` | 459 | counter live and advancing |
| `state_bus_overflows` | **0** | SPSC never overflowed under full 4-track state churn — **measured**, no longer assumed |
| `dropout_lock_miss` | 0 | trivially 0 — see caveat |
| `dropout_no_executor` | 0 | trivially 0 — see caveat |

The two dropout counters are NOT exercised by the test-driver — they increment
only inside the cpal callback (`audio.rs`) on a `try_lock` miss or an empty
executor cell, and the test-driver drives `ex.process()` directly (holding the
lock itself), so they read 0 by construction there.

**Load test — callback path measured (2026-07-12).** The two dropout counters
are now measured under real audio via
`crates/paraclete-app/tests/patch_tests.rs::loadtest_topology_churn_under_live_audio`
(`#[ignore]`, opt-in — needs an output device). It boots the default instrument
through the **real cpal callback** and hammers `apply_patch` (topology swaps) for
~15 s, driving the ADR-029 pause-rebuild-resume cycle against the live audio
thread, then reads the counters straight off the executor's shared `Arc`:

| Counter | Value | Read as |
|---|---|---|
| topology swaps applied | 703 (0 failures) | heavy live churn |
| `buffers_processed` | 731 | real cpal callback ran |
| `dropout_lock_miss` | **0** | pause protocol never lost a lock race across 703 executor swaps — **measured** |
| `dropout_no_executor` | **0** | callback never caught the cell empty — **measured** |
| `state_bus_overflows` | 0 | SPSC kept up under churn |

**Conclusion:** all four counters are now **measured quiet** — the executor path
under an 8 s busy render (test-driver) and the audio-callback path under 703 live
topology swaps (load test). The pause-rebuild-resume protocol holds under stress:
zero self-inflicted dropouts. (The load test also surfaced BUG-012 as a hard
crash on buffer-size mismatch — see that entry's 2026-07-12 confirmation.)

---

### DEBUG HARNESS — ADR-033 interactive mode shipped (2026-07-13, `92b8795`)

The interactive debug harness (roadmap Rank 2 keystone) now exists:
`test-driver --interactive` opens a JSON-lines REPL over the null-backend engine
stack — `set_param`/`trigger`/sequencer/chain mutations plus `read` / `dump` /
`peak` / `render` / `quit`. Live engine interrogation with no hardware, no TUI.

**Unblocks the harness-gated audit items.** Round 3 (2026-07-12) deferred items
#9, #12, #13–#15, #17–#23, #25, #26, #29 to "the ADR-033 interactive debug
harness or hardware." The ones that needed *the harness* (not physical hardware)
are now reachable — walk them with the REPL: set the relevant params, trigger,
and `read`/`peak` the state and audio to confirm or file. Only genuinely
hardware-gated items (e.g. BUG-012, device rate/buffer negotiation) still wait on
a hardware session.

**Still open in the debug-posture push (roadmap Rank 2):** audio diff/snapshot
baselines, the structured per-node log channel, and the CPU-% meter to
`/engine/cpu`. These build on the harness and are the next increments.

---

### BUG-012 — PARTIAL (2026-07-14, corrected 2026-07-14)

**Fix shipped (commit `6bd2129`):** sample-rate auto-detection (`query_sample_rate()`
in `paraclete-hal/src/audio.rs`) + `audio_callback_f32` chunking that splits the
cpal callback into internal `block_size` chunks. A `work_buf` handles the final
partial chunk (render full block, copy leading frames).

**Correction (2026-07-14):** the `design/bugs.md` resolution description (above,
as written 2026-07-14) claimed a ring buffer (`fill_output_ring()`) that does
not exist in the codebase. The actual implementation (`audio_callback_f32`,
`audio.rs:354`) is a simpler chunk-and-discard:
- Full chunks: `executor.process(chunk, channels)` — renders directly into the
  cpal buffer slice.
- Partial chunk: renders a full block into `work_buf`, copies only the leading
  frames, and **discards the rest**. No carryover.

**Consequences on Linux ALSA (9408-sample callback, 512-frame block, stereo):**
- 9 full chunks (9×512 = 4608 frames) + 1 partial chunk (96 frames output;
  416 frames discarded) = 5120 frames rendered, 4704 delivered per callback.
- ~416 frames of rendered audio silently dropped every callback (~9.4 Hz).
- Engine state (oscillators, envelopes, filters) advances for the discarded
  frames — next callback starts from a desynchronized state.
- Clock advances by `block_size` per `process()` call (5120 frames/callback
  vs 4704 delivered), drifting ~8.8% fast. After 10 callbacks (~1s), the
  sequencer is ~94 ms ahead.

The discarded-audio discontinuities, state/output desync, and clock drift
combine to produce the "very distorted" kick reported in paired session #3.

**Remaining work:**
1. **Output ring buffer** — accumulate executor output in a pre-allocated ring;
   drain in the callback. No frames discarded, timing error bounded to ±½ block.
2. **FTZ/DAZ** — set flush-to-zero + denormals-are-zero on the audio thread for
   x86 Linux (`_MM_SET_FLUSH_ZERO_MODE`, `_MM_SET_DENORMALS_ZERO_MODE`). The
   design review directed this but it was never implemented.
3. **BufferSize::Fixed retry** — attempt `BufferSize::Fixed(block_samples)` first;
   fall back to Default only if the device rejects it (EINVAL on some ALSA
   hardware).

---

### INFRA-004 — No headless mode; LaunchpadEmulator requires a TTY

**Severity:** Medium — cannot run the app in background without a PTY shim
**Phase found:** W2 paired session #3 (2026-07-14)
**Description:** `LaunchpadEmulator::activate()` calls
`crossterm::enable_raw_mode()` which fails without a real TTY, and the emulator
continuously writes ANSI escape sequences to stdout. Starting the app with
`cargo run -- --no-tui ... &` kills the process when the shell exits. Workaround
is `setsid` (new session) but the emulator still spews grid output.
**Location:** `crates/paraclete-hal/src/emulator/mod.rs` — `activate()`, `process()`
**Fix direction:** Add a `--headless` flag (or `--no-emulator`) that skips
`LaunchpadEmulator` creation entirely. In headless mode the app runs as a pure
Antiphon server with no TUI and no emulator — stdin/stdout are unused. The
emulator's `--no-tui` flag already controls the ratatui layer; this is a separate
concern (the HAL-layer hardware node itself).
