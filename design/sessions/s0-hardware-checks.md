# Session 0 — Hardware Verification Protocol

> **Runbook.** Written July 2026 so any implementation session (any model tier)
> can run these checks with the user. Results go in this file's "Findings"
> section (append-only). Each check has a decision tree — record the branch
> taken; do not improvise beyond it.

## Gear needed

Digitakt + USB, Launchpad + USB, audio out. (iPad not needed until W0 exit
criteria; Keystep not needed until P11 live-record.)

---

## Check 1 — Digitakt relative-CC verification (highest value, ~15 min)

**Why:** the contextual-encoder model requires true relative deltas
(`CMD_BUMP_PARAM`). The Digitakt-as-encoder-stopgap assumption has never been
verified; Elektron gear commonly sends **absolute** CC.

**Procedure:**
1. Connect the Digitakt over USB. Confirm `DigitaktMidiNode` opens (stderr log
   at startup; device id 103).
2. Create `profiles/_diag_dt.rhai` (temporary, do not commit):
   ```rhai
   on_surface_event(DT_DEVICE_ID, |ev| {
       print(`DT ev: ${ev}`);   // want: id, value, delta per encoder turn
   });
   ```
   If the event object doesn't stringify usefully, print fields explicitly
   (id / value / delta per the `SurfaceEvent::EncoderChanged` shape).
3. Run `cargo run -- --no-tui`. Turn one Digitakt encoder slowly clockwise,
   8–10 detents; then quickly; then counter-clockwise.

**Read the output:**
- **Relative:** repeated small signed deltas (+1/+2 per detent, negative
  counter-clockwise), `value` static or irrelevant → **encoder path
  validated.** Digitakt is a workable encoder surface until a dedicated box.
- **Absolute:** `value` climbs/falls 0–127, deltas computed only from value
  differences (jumping on fast turns, pegging at 0/127) → check Digitakt MIDI
  settings for a relative/inc-dec CC mode first (SETTINGS → MIDI CONFIG). If
  none exists: **record "Digitakt = absolute only"** in roadmap provisional
  table; the touch encoders (W1) become the only relative path until a
  true-relative box (Intech EN16 Smooth / MFT) is purchased. Do not build
  soft-takeover as a workaround — explicitly rejected in the controller
  strategy.

## Check 2 — BUG-001 audibility (~5 min, piggybacks on Check 1)

**Only meaningful BEFORE P10 C0 lands; skip afterwards (or use as the
after-fix confirmation).**

1. Paraclete: default instrument, four-on-the-floor kick, 120 BPM. Digitakt:
   any 120 BPM kick pattern, started in sync by hand.
2. Listen ~2 minutes. Expected with the bug: ~0.4% tempo deficit → drift of a
   full beat in roughly 50–60 s at 120 BPM.
3. Record: audible drift yes/no, approximate time-to-one-beat.
4. After P10 C0: repeat; expect no systematic drift (hand-start error only).

## Check 3 — Session zero UX baseline (~25 min)

Run on the current build (pre-W0), Launchpad connected. The point is a
friction baseline before the tablet exists — capture, don't fix.

Tasks (user drives, session-runner observes and takes notes here):
1. Program a beat across **all 8 tracks** (kick/snare/hat/samples/FM bass).
2. Tweak per-track params (encoders if Check 1 passed; otherwise note the gap).
3. Use fills (`CMD_SET_FILL_A/B` gestures if the profile exposes them) and
   whatever performance gestures exist.
4. `--save`, quit, `--load`, verify the beat and params survived.
5. Free play, 5 min.

Capture per task: time taken, wrong-turns, "where do I look" moments, anything
the user says out loud. Close with the user's top-3 frustrations, verbatim.

---

## Findings

_(append results here, dated, with the branch taken per check)_

### 2026-07-04 — Check 1: Digitakt relative-CC (VERDICT: ABSOLUTE — disqualified)

- Hardware is a **Digitakt II** (single USB MIDI port, "Elektron Digitakt II";
  name-substring match in `DigitaktMidiNode::open()` works unchanged).
- Default config transmits nothing (Encoder Dest = INT). With INT+EXT +
  Param Output = CC, encoder A produced `B9 03 <value>` — CC 3, auto-channel
  (10), **value tracking knob position** (0x3B→0x36 down, →0x3E up). Absolute,
  and mirrored from the internal parameter.
- Branch taken per decision tree: **absolute only.** No relative mode exists in
  the Elektron MIDI implementation (MIDI-machine CC knobs are also absolute).
  Digitakt II removed from the encoder-controller role; W1 touch encoders are
  the only relative path until a true-relative box. No soft-takeover.
- Side finding → **BUG-009**: `decode_relative_delta()` in
  `paraclete-hal/src/digitakt/mod.rs` assumes relative encoding; against real
  DT II output it yields garbage deltas (position 0x3B → "delta +59").

### 2026-07-04 — Check 2: replaced by direct measurement (BUG-001 re-diagnosed)

The Digitakt-reference listening test was dropped (user call: no second sound
generator as reference). Replaced with an offline harness
(`paraclete-app/tests/timing_measure.rs`) driving clock→sequencer and
measuring note-on intervals in samples. Verdict: **no systematic tempo error**
(+0.011% mean). Real defect: 15 steps at +0.39% (the 241/240 period), wrap
step −5.85% (≈7 ms early once per pattern). Details under BUG-001 re-diagnosis
in bugs.md; gated test is the P10 C0 acceptance gate.

### 2026-07-04 — Check 3 partial: Launchpad dark-grid root cause

"Launchpad shows nothing" traced to BUG-010 (three silent error swallows) +
BUG-011 (empty TRACK_SAMP_IDS on the 4-track default → profile crash
mid-on_load). Both fixed; LED bytes confirmed flowing ([lpx] note=.. lines).
Human confirmation of visible LEDs/sound pending → runbook
`s0-launchpad-debug.md` (Haiku-executable).
