# Theotokos — Usability Session #7

**Date:** 2026-08-21
**Phase:** TK3 exit criterion §6 — resolves items deferred by session #6
(`design/sessions/theotokos-6.md`)
**Session type:** paired (user + agent on the same keyboard), driven over
`kitty @ --listen-on unix:@tk4`, `./target/release/paraclete`

> **Session status: held 2026-08-21.** Walked the three items session #6
> left for user verdict (T6, T7, §6.2). T6 accepted with two fixes; §6.2
> verified PASS. T7 (overall feel) was not reached — the session ended
> before that round. Five live bugs found and fixed along the way, none of
> them on the original agenda; all five have new regression tests.

## Hypotheses

| # | Hypothesis | Verdict |
|---|---|---|
| T6 | `FUNC+8`/MIX screen key binding is acceptable | **Accepted**, with two fixes (see below) — see Round 1 |
| §6.2 | Params survive a `machine` round-trip | **PASS** — see Round 2 |
| T7 | The TK3 feature set as a whole improves playability | **Not reached.** Session ended before this round. |

## Round 1 — T6: the `FUNC+8`/MIX binding

**Method:** live keypresses in kitty, both user and agent driving the same
window (see Method notes on the resulting cross-talk).

**Findings:**
- `8` → Settings, `Shift+8` → MIX, `Shift+8` again while on MIX → idempotent
  re-open, `Esc` → back. No collision with existing chords.
- User asked what "FUNC" meant on the physical keyboard mid-session — it's
  Shift (`input::func_held`). That surfaced a real inconsistency: the hint
  bar's other chips all show the literal key printed on the keyboard
  (`[Z]`, `[X]`, `[TAB]`...), but modifier hints said `FUNC` — an
  abstracted name found nowhere on the keyboard. The live SHIFT/CTRL status
  chip (TK3-C6) already said `SHIFT`, not `FUNC`, so the two UI surfaces
  disagreed with each other.
- Separately: `Shift+8 → MIX` had no on-screen referent anywhere except
  inside MIX itself (as `RE-OPEN`). Grid's hint bar already advertises
  every other screen-open chord (`1-6 PAGE`, `8 SET`, `O SONG`) but not
  this one — the same class of gap the code's own `F8` fix once closed for
  `[M] LOCK` in the opposite direction.

**User verdict, both fixes applied:**
1. Every `FUNC` hint-bar/help-overlay string changed to `Shift` (the
   literal key), matching the rest of the hint bar's convention.
2. Grid's hint bar now includes `[Shift+8] MIX`, next to `[8] SET`.

**Verdict: Accepted**, with the above two fixes.

## Round 2 — §6.2: params across a machine round-trip

**Method:** `:set tune 0.35` on Kick (`AnalogKick`) via the command line,
jog `machine` to `AnalogSnare` and back via ENC mode, re-check `tune` on
the Source page.

**Findings:**
- `tune` read `0.35` before the round-trip and `0.35` after — not reset to
  the `AnalogSnare` (or `AnalogKick`) default.
- Consistent with the architecture (`ParameterBank` holds append-only
  per-param-id slots regardless of which machine is currently selected —
  see AGENTS.md's "Bank keeps union width; overlay narrows"), but this had
  never actually been exercised live; session #5 skipped it and session #6
  left it open.

**Verdict: PASS.** Closes the open question.

## Bugs found and fixed (not on the original agenda)

None of these were being hunted for — they surfaced from just using the
panel to test T6 and §6.2.

**1. SHIFT/CTRL status chip stuck lit after releasing a bare modifier key.**
Root cause: crossterm 0.27's CSI-u parser (`parse_csi_u_encoded_key_code`)
sets the modifier's own bit in `ev.modifiers` whenever the *keycode* is
`Modifier(LeftShift/RightShift/...)`, unconditionally on both press and
release — so releasing bare Shift still arrives with `modifiers = SHIFT`.
The app's chip mirror (`self.held.func_held = input::func_held(ev)`)
trusted that bit naively, so the chip only cleared on the next unrelated
keypress. Fix: a release of the modifier key itself is now treated as
authoritative, overriding the stuck bit (`lib.rs`, both `func_held` and
`ctrl_held`). Confirmed on the user's physical keyboard after the fix.

**2. TRK-hold showed the wrong key hints.** While TRK (Tab) is held, its
trig keys silently select a track instead of toggling a step — but the
grid still showed its normal `[Q][W][E]...` step-toggle chips, and the
track line (gated to `RecMode::Off | Live` only) showed none of its own.
User's ask: "keys should show next to the instrument names, and grid keys
should gray out, like the trig mode" — then, after seeing the gray
version, corrected it to match pad mode's *actual* treatment, which fully
**hides** the grid's chips rather than dimming them. Fix: `render.rs`'s
track-line chip visibility and the trig grid's `show_keys` both now also
key off `armed_prefix == Some("TRK…")`, matching `RecMode::Off` exactly.

**3. The `:` command line double-typed every character on a kitty
terminal.** Found while trying to type `:set tune 0.35` for the §6.2 test
— it came out `?:sseett  ttuunnee  00..3355`. Root cause:
`handle_cmdline_key`'s call site had no `KeyEventKind` filter, unlike every
other key handler in the file — kitty's real release events (from
`REPORT_EVENT_TYPES`) inserted the same character a second time on release.
This is a general-purpose bug, not a testing artifact: any user typing a
colon-command on a real kitty terminal would hit it. Fix: gated to
`Press | Repeat`, matching the pattern used everywhere else in
`handle_keys`.

## Method notes

**Cross-talk between the two sets of hands on one keyboard produced
apparent flakiness that had a mundane explanation.** While testing the
TRK-hold fix, `z` (REC toggle) appeared not to respond consistently to the
agent's remote `send-key` calls. This was the user also pressing keys on
the same live window at the same time — exactly the scenario
`kitty-driving`'s "ask, don't bisect" guidance exists for. Once named, it
was immediately obvious and not investigated further as a defect.

**A synthetic remote-control press of a bare modifier key is not a
reliable stand-in for a real one.** The agent tried reproducing the SHIFT
chip's stuck-open bug via `kitty @ send-key`/raw CSI-u escapes before
asking the user to test physically; both synthetic attempts were
inconclusive (no chip appeared at all, held or not), while the user's
single real keypress reproduced it immediately. The eventual fix was
confirmed against crossterm's actual parser source, not against the
inconclusive synthetic test.

**Command-line typing (§6.2's `:set`) had apparently never been exercised
on a kitty terminal before this session** — bug #3 above would double
every character for any user on kitty, and nothing in the existing 253
tests caught it, because the existing cmdline tests all use
`KeyEvent::new` (defaults to `Press`) and never construct a paired
`Release` event the way a real kitty terminal delivers one.

## Artifacts

| | |
|---|---|
| Fixes | `crates/paraclete-theotokos/src/lib.rs`, `crates/paraclete-theotokos/src/render.rs` |
| Tests | 258 passing in `paraclete-theotokos` (253 → 258; 5 new, all covering the bugs above) |
| Session notes | This file |

## Deferred to user verdict

- **T7:** Overall TK3 feature set — does it improve playability? Not
  reached this session.

## Noticed, not addressed this session

`design/phases/tk3-report.md`'s status banner ("code-complete, awaiting
session #6") and its Session #6 gate row ("⏳ pending (needs user)") are
now stale — session #6 was held 2026-08-06 and this session (#7) resolved
two of its three deferred items. Left unedited here per this session's
scope; recorded so the divergence isn't silently carried forward.
