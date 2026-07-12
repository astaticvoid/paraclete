# Paraclete — Known Bugs

Append-only. Add new bugs at the bottom. Mark resolved with **Fixed:** line and the commit that fixed it.

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
