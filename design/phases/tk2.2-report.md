# TK2.2 — Theotokos Session #3 Fix Pass: Report

**Status:** **CLOSED.** Code-complete (C0–C5) and signed off by usability
session #4, held 2026-07-30 (`design/sessions/theotokos-4.md`). Every
hypothesis this phase set out to prove passed under the hand. The session
found four defects beside the fixes — one of them a transport regression in
`InternalClock` that this phase's own ADR-046 specified correctly and the
implementation left one command arm short — plus one test-harness defect and
one user directive that wants an ADR. All are filed as issues against the
code; none block closing the phase.

**Spec:** `design/phases/tk2.2-theotokos.md`
**Design authority:** ADR-044 (amended by session #3), ADR-046 (accepted
2026-07-29, implemented in C5)
**Baseline:** TK2.1 code-complete (C0–C7), reopened by usability session #3
(`design/sessions/theotokos-3.md`)

## Commits

| C0 | `9f62cef` | BUG-044 — `CMD_TRIG_NOW` resolves to the track's `default_note` |
| C1 | `eca103d` | BUG-046 + E4 — one owner for the bare trig; momentary p-lock retired |
| — | `ecf74e4` | C1 nit — comment reworded so the momentary grep is truly zero |
| C2 | `7238e31` | BUG-045 — a hand-written step zeroes its `micro_offset` |
| C3 | `11503de` | E1/E2/E3 — legend/chip placement revision, residual legend, no reflow |
| C4 | `478cc6b` | E5 — a jog says what it is writing |
| C5 | `8595db6` | BUG-043 — pause/stop via ADR-046, three-command transport vocabulary |
| — | `4fe3f5f` | C5 follow-up — ADR-046 T3 blast radius: test-driver and `launchpad.rhai` |
| — | `f5bb007` | Doc sweep — bugs/roadmap/handoff/ADR-046 |
| C6 | *(this report)* | Usability session #4 — `design/sessions/theotokos-4.md` |

## Session #4 outcome

**The fixes hold.** E1/E2/E3's legend and chip revision reads without `?` and
nothing reflows across any screen or mode. E4's one-owner bare trig makes
latched `[m]` sufficient on its own — which settles the near-term half of
OQ-T27 and, on the user's explicit judgment, **keeps ADR-045 parked**. E5's
jog readout names its destination. BUG-044's live pad and its sequenced step
are the same instrument on direct comparison. H8's continuous jog
proportionality, H10/R5's copy/clear/paste mechanics and H11's TRK/PTN feel
and encoder simultaneity all passed.

The panel-grammar argument that dominated sessions #2 and #3 **did not
resurface**. That is what this phase was for.

## What the session found

| Issue | Finding | Where it came from |
|---|---|---|
| #142 | `CMD_CLOCK_REWIND` never resets the clock's own position — `x → c → x` restarts at the top but re-phases the loop to the old boundary | User, by ear |
| #143 | The app boots with the transport running: `profiles/launchpad.rhai:29` sends `CMD_CLOCK_START` in `on_load`, and profiles load unconditionally | Agent measurement |
| #144 | A latched hold-prefix is unrecoverable in the kitty path; `HeldState::on_esc()` has no production caller though D6 specifies it | Agent measurement |
| #145 | Antiphon's initial `TransportSummary` hard-codes `playing: true` on a premise ADR-046 T3 retired | Agent measurement |
| #146 | test-driver name resolution silently misroutes `target: Kick` to the AnalogEngine, not the sequencer | Agent measurement |
| #147 | *(open question)* Every command should acknowledge itself; a held key should highlight | User directive |

### The pattern worth carrying forward

**#142, #143 and #145 are one blast radius, and it is not the one ADR-046
inventoried.** The ADR checked *callers of the clock commands* — thoroughly,
and that inventory was correct. What it missed was *claims about clock
state*: a profile's `on_load` asserting the transport should be running, a
hard-coded hello snapshot asserting `playing: true`, and a comment in
`launchpad.rhai` asserting what Theotokos does in its own `new()` (it does
not; T3 retired exactly that). None of these are callers. All three
contradict T3.

#142 is the sharper version of the same shape: ADR-046 T1 says
`CMD_CLOCK_REWIND` means "set position to the window start", and the command
arm sets a boolean and returns. The decision was right and reviewed; the
implementation satisfied every *caller-side* test while never doing the thing
the ADR named. A migration inventory that enumerates call sites will not
catch this class. Ask instead which **assertions about the changed state**
exist in the tree — in prose, in comments, in hard-coded snapshots, in
profile scripts — because those are what rot.

## Method

Session #4 upgraded the paired-session method materially:
`kitty @ send-key` delivers press *and* release, and composed with
`send-text` it synthesizes hold-chords, so the agent verified step entry,
FUNC chords and copy/clear/paste mechanically and spent the user's time only
on judgment. Session #3's "chords genuinely need the user's hands" is
superseded. Full method note, including two traps that produced retracted
findings, is in `design/sessions/theotokos-4.md`.

One agent finding was **withdrawn in full**: a reported sequencer-silence
regression rested on `parecord` off the default sink monitor reading
`0.0000` while the user could hear the pattern. The user refuted it
directly. `parecord` is not a valid audio oracle for this app; `test-driver`
renders are.

## Carried out of this phase

- **H9** (D11 sticky re-tap + 400 ms guard) — untestable in kitty, which
  delivers releases; needs a release-less terminal that is not tmux
- **H8 stepped half** — needs an engine declaring `stepped: true`; none does
- **F1** (16 named tracks) — needs such an instrument to exist
- **#147** — wants an ADR and the user's sign-off before implementation
- **ADR-045** — stays parked on the user's session #4 judgment, not on
  absence of evidence
