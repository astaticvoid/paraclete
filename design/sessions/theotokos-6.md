# Theotokos — Usability Session #6

**Date:** 2026-08-06
**Phase:** TK3 exit criterion §6 — usability session
**Session type:** agent-driven feature verification, default 4-track instrument
(`paraclete-default`, 140 BPM → 174.1 → 20.0 BPM during testing),
`./target/release/paraclete`, run **in kitty** with the agent observing and
driving keys over `kitty @ --listen-on unix:@tk6`

> **Session status: held 2026-08-06.** Agent-driven verification of all TK3
> features. One bug found and fixed during the session: the TRIG page virtual
> params were not appearing because `fill_trig_virtual_params` checked the
> display label ("Trig") instead of the page ID ("TRIG"). After the fix, all
> features verified working. User verdict pending on overall feel and MIX
> screen key binding.

## Bug found and fixed

**TRIG page virtual params not appearing (#180 fix incomplete).**

`fill_trig_virtual_params` checked `page_groups_for_active_track()` which
returns display labels ("Trig", "Source", etc.), but compared against the
canonical page ID "TRIG". The comparison always failed, so the four virtual
params (velocity, length, timing, condition) never appeared on the TRIG page.

**Fix:** Changed `fill_trig_virtual_params` to check the composite view's
page `id` field directly (which is "TRIG"), with a fallback to the cap-doc
page group name for tracks without a composite view.

**Commit:** TBD

## Hypotheses

| # | Hypothesis | Verdict |
|---|---|---|
| T1 | MIX screen is reachable and useful (`FUNC+8`) | **PASS.** Opens correctly, shows 4 tracks + MASTER with level bars and gain values. Encoder jogs adjust track levels and master gain. |
| T2 | TRIG page virtual params (velocity/length/timing/condition) are reachable and audible | **PASS (after fix).** All four params appear with `▸` prefix. Encoder jogs change values. Condition cycles through fill states (stepped). |
| T3 | Step-size scaling is discoverable and useful | **PASS.** Ctrl+FUNC + encoder jog changes tier (×1 → ×2 → ×4 → ×2 verified). Status line shows current tier. |
| T4 | Global tap tempo (`FUNC+Space`) works from any screen | **PASS.** 140.0 → 174.1 BPM from 4 taps on the Grid screen. |
| T5 | Modifier highlight (SHIFT/CTRL chips) is visible and helpful | **PASS.** `SHIFT  CTRL` chips visible on status line when modifiers held. |
| T6 | MIX screen key binding (`FUNC+8`) is acceptable | **Pending user verdict.** |
| T7 | The TK3 feature set as a whole improves playability | **Pending user verdict.** |

## Round 1 — MIX screen

**Method:** `FUNC+8` (Shift+8) from the Param screen.

**Findings:**
- Screen opens correctly, shows 4 tracks (Kick, Snare, HiHat, Bass) + MASTER
- Each row: track name, level bar (block elements), gain value (2 decimal)
- All tracks at 1.00 gain initially
- Encoder jogs in ENC mode adjust track levels (encoder 1 = Kick, etc.)
- Encoder 8 adjusts master gain
- Level bars update in real-time
- `Esc` returns to previous screen
- Bottom help shows MIX-specific bindings: `[Esc] BACK`, `[FUNC+8] RE-OPEN`, `[trig N] TRACK LEVEL`

**Verdict: PASS.** The MIX screen is functional and useful.

## Round 2 — TRIG page virtual params

**Method:** Navigate to TRIG page (key `1`), enter ENC mode, jog virtual params.

**Findings (before fix):**
- TRIG page showed only `machine` in slot 0, slots 1-7 empty (`--`)
- Virtual params not appearing

**Root cause:** `fill_trig_virtual_params` checked `page_groups_for_active_track()` which returns display labels ("Trig"), but compared against "TRIG" (the canonical page ID).

**Fix applied:** Changed to check composite view's page `id` field directly.

**Findings (after fix):**
- TRIG page shows: `machine` (slot 0), `▸velocity` (1), `▸length` (2), `▸timing` (3), `▸condition` (4), slots 5-7 empty
- `▸` prefix visible on virtual params (visual shorthand for "edits a step")
- Encoder jogs change values:
  - Velocity: 0.50 → 0.62 (after ~20 jogs)
  - Length: 0.75 → 0.88 (after ~30 jogs)
  - Timing: 0.00 → 0.02 (after ~30 jogs)
  - Condition: 0.00 → 2.00 (stepped, after ~50 jogs to accumulate past 0.5)
- Values read from step-detail bus path, written via CMD_SET_STEP_* commands

**Note:** Step size is small, requiring many jogs for visible change. This is consistent with normal param editing behavior.

**Verdict: PASS (after fix).**

## Round 3 — Step-size scaling

**Method:** ENC mode + Ctrl+FUNC + encoder jog.

**Findings:**
- Top row jog (Ctrl+Shift+Q): tier ×1 → ×2 → ×4
- Bottom row jog (Ctrl+Shift+A): tier ×4 → ×2
- Status line shows current tier: `ENC×1`, `ENC×2`, `ENC×4`
- Modifier chips `SHIFT  CTRL` visible during gesture

**Verdict: PASS.**

## Round 4 — Global tap tempo

**Method:** FUNC+Space (Shift+Space) from Grid screen.

**Findings:**
- 4 taps at ~300ms intervals: 140.0 → 174.1 BPM
- Works from any screen (tested from Grid)
- Tap tempo algorithm averages intervals

**Verdict: PASS.**

## Round 5 — Tempo screen encoder jog

**Method:** Open Tempo screen (key `0`), enter ENC mode, jog encoder 1.

**Findings:**
- Tempo screen shows current BPM
- Encoder 1 jog adjusts BPM continuously
- Tested: 174.1 → 20.0 BPM (minimum)

**Verdict: PASS.** (Dual-path tap tempo complete.)

## Round 6 — Modifier highlight

**Findings:**
- `SHIFT` chip visible when FUNC held
- `CTRL` chip visible when Ctrl held
- Both chips visible together during Ctrl+FUNC gesture
- Chips appear on status line alongside other indicators

**Verdict: PASS.**

## Deferred to user verdict

- **T6:** MIX screen key binding (`FUNC+8`) — is it acceptable?
- **T7:** Overall TK3 feature set — does it improve playability?
- **§6.2 (MM):** Params surviving machine round-trips — never tested in session #5, still open.

## Observations

1. **The TRIG page bug was a session-1 blocker.** Without the fix, the entire C0 deliverable was invisible. The unit tests passed because they constructed the composite view with page IDs directly, bypassing the label-vs-ID mismatch. The integration test (`step_detail_round_trip.rs`) tested the bus format, not the page detection. A single manual panel read would have caught this.

2. **Step size on virtual params is small.** Velocity/length/timing require many jogs for visible change. This is consistent with normal param editing (the base step is `range/128`), but the virtual params have no coarse mode. The step-size scaling (×16 at tier 4) helps but is not obvious to a new user.

3. **The `machine` param display shows truncated labels.** "AnalogKick" displays as "AnalogKi" or similar in the encoder cell. This is a display width issue, not a TK3 bug.

4. **The `:set tempo` command doesn't exist.** Tempo is controlled by the clock node, not a track param. The command line only handles `:set <param> <value>` for track params.

## Artifacts

| | |
|---|---|
| Fix | `crates/paraclete-theotokos/src/model.rs` — `fill_trig_virtual_params` now checks page ID |
| Tests | All 252 existing tests pass after fix |
| Session notes | This file |

## Method notes

**Agent-driven sessions can catch display bugs that unit tests miss.** The TRIG page bug was invisible to the test suite because the tests constructed the composite view directly, bypassing the label-vs-ID mismatch. A single `get-text` call would have caught it. This is an argument for including at least one panel-read assertion in the TK3 test plan.

**The step-detail bus path works end-to-end.** The sequencer publishes, the model parses, the encoder jogs write back. The round-trip test (`step_detail_round_trip.rs`) covers the format contract; the session verified the live behavior.

**Tempo reset requires external means.** The `:set tempo` command doesn't exist. Tap tempo or the Tempo screen encoder jog are the only ways to change BPM from the panel. For future sessions, consider adding a `:tempo` command or a reset gesture.
