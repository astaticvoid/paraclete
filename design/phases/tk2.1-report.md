# TK2.1 — Theotokos Panel Redesign: Report

**Status:** Code-complete (C0–C7). **NOT yet signed off** — usability
session #3 (C8, user-paired, no code) is next; this report closes when
`design/sessions/theotokos-3.md` lands with a converged / revise / park
verdict per hypothesis (§5 of the phase spec).
**Spec:** `design/phases/tk2.1-theotokos.md`
**Design authority:** ADR-044 (✅ accepted 2026-07-28)
**Baseline:** TK2 code-complete (C0–C9, `be565b9`), reopened by usability
session #2 (`design/sessions/theotokos-2.md`, `design/phases/tk2-report.md`)

## Commits

| — | `f2576f4` | BUG-041 — `CMD_CLOCK_STOP` now emits `global_stop` |
| C0 | `e9328f8` | Fixed regions, one-track strip, track indicator, display names (D1/D2) |
| C1 | `2028f9d` | `RecMode{Off,Grid,Live}`, pads, silent launch (D5-D7) |
| C2 | `f3053fb` | Key chips + rewritten legend strip (D3/D4) |
| C3b | `c8a9b5b` | Engine-side `live_rec` recording (D8, ADR-039 decision 7 pulled forward) |
| C4 | `87fcbcc` | Descriptor-accurate encoder range/stepped resolution (D10, closes BUG-040) |
| C5 | `311cbad` | ENC mode + p-lock lock target, latched + momentary (D9/D15) |
| C6 | `7e42c0f` | Sticky re-tap reversed, Mute retired, keymap degrades gracefully (D11/D12/D14) |
| C7 | `d1ed585` | BUG-038 disposition (encoder_cursor deleted), help overlay regenerated |
| — | *(this commit)* | Doc sweep: reports, ADR notes, README/AGENTS.md, design.md §5.1/§5.2 |

Each code commit is paired with a same-day `design: TK2.1 C<n> shipped`
doc-only follow-up (`11f9dd5`, `64133bd`, `d45628d`, `13cb18d`,
`56b5938`, `a0ea0a1`, `c9a225a`) recording that commit's hostile-review
findings in `design/roadmap.md` — not re-narrated here.

## Exit criteria

| Criterion | Status |
|---|---|
| `cargo test --workspace` green after every commit | ✅ (160 theotokos tests at C6; workspace fully green throughout) |
| Pre-commit hostile review on every staged commit | ✅ — caught and fixed at least one real defect on every commit except C4 (verified clean) |
| BUG-038 resolved or formally descoped | ✅ **Descoped** (C7) — see below |
| Agent smoke run | ⚠️ **Partial** — see below |
| Usability session #3 sign-off | ❌ Not yet held — this report is not a convergence claim |

## Hostile review findings by commit (summary; see each commit message for detail)

| Commit | Findings folded before landing |
|---|---|
| BUG-041 | Stale `just_stopped` mid-batch flag caused a spurious `global_stop` on `[STOP,START]` in one batch |
| C0 | Transport/status line read engine name not display name; ADR-044 D2 windowing was entirely unimplemented; `for_test` panicked above 16 tracks; test-coverage gaps |
| C1 | Kitty `Repeat` events re-fired `ToggleRec` once per OS auto-repeat pulse |
| C2 | `[^C] QUIT` chip missing from every legend list despite D4 naming it; `for_test` panic recurrence; tail-truncation had zero test coverage |
| C3b | HIGH: no-kitty REC fallback into `Live` never armed `live_rec`. MEDIUM (deferred, filed as BUG-042): live recording on an imminent step boundary can double-trigger |
| C4 | Clean — no findings |
| C5 | HIGH: `:set` routed lock commands to the wrong track's sequencer when the lock target diverged from the active track. HIGH: a TRK-chord track switch could spuriously arm a p-lock. MEDIUM: momentary release used a stale recomputed track instead of the press-time one. LOW: stale status-line label |
| C6 | HIGH: Lock's pending-arm cancel bypassed D11's auto-repeat guard entirely (a bespoke `lib.rs` intercept, not `on_press`), so an OS auto-repeat pulse could wipe a pending p-lock arm. MEDIUM: the auto-repeat guard's clock was captured once per key-event batch, not per event. LOW: two stale post-retirement comments |

## BUG-038 disposition

**Descoped, not implemented**, in this commit. BUG-038 covered two
unwired pieces of the original D13 spec text:

1. **Arrow-cursor navigation of the encoder bank.** Judged genuinely
   **moot**, not merely unimplemented: TK2.1 C5's ENC mode (D9) gives
   every encoder cell a direct physical (key) address, so there is
   nothing left for a cursor to navigate between. The dead
   `Model::encoder_cursor` field — always `0`, never mutated by any code
   path, driving a `>` marker permanently stuck on cell 0 in every real
   run — was deleted in this commit, along with the rendering it drove.
2. **Numpad slot A/B/C jog.** Stays **formally open** (OQ-T24), not
   resolved — deliberately reserved for usability session #3 as a free
   choice, now that D9 discharges the TK2 spec's §0 A7 modifier-floor
   condition that used to constrain it. Wiring it in this "no new
   features" commit would have preempted a decision the live session
   exists to make.

See `design/bugs.md` BUG-038 and
`design/adr/ADR-044-theotokos-fixed-panel.md`'s implementation note for
the full reasoning.

## Agent smoke run — partial, environment-limited

The TK2 C7 precedent ("run the app on the default 4-track instrument,
fix paper cuts the suites cannot see") assumes an interactive terminal.
This session ran fully autonomously with no attached TTY:
`cargo run -- --no-emulator --no-antiphon` against `instrument.yaml` (the
default 4-track instrument) starts cleanly — audio runs, the profile
loads, no panic — but Theotokos itself reports `terminal setup failed:
No such device or address (os error 6)` (crossterm cannot enter raw mode
without a real controlling terminal) and never renders a frame. This is
an environment limitation, not a code finding: there is no pty/tty
available to this session to drive the actual interactive render loop,
key-by-key, the way the TK2 C7 precedent's live agent pass did.

**What substituted for it:** the render test suite is unusually
thorough for exactly this reason — `TestBackend`-driven assertions cover
every screen, the legend/chip system, track-indicator windowing, the
encoder bank, and region-height invariants across all screens
(`crates/paraclete-theotokos/src/render.rs`'s test module, ~40 render
tests among the 160 theotokos tests at C6). This gives strong
*rendering-doesn't-panic-and-shows-the-right-text* confidence but does
**not** substitute for a human (or live agent) actually pressing keys in
a real terminal and judging feel — that is exactly what usability
session #3 is for. Flagging this explicitly rather than claiming a smoke
pass that didn't happen.

## Doc sweep

- `design/theotokos/design.md` §5.1/§5.2 rewritten as **DETERMINED**
  against the accepted ADR-044, superseding session #2's REOPENED status
  (kept below each as history, per the doc's own never-delete
  convention); new **Stage 5** closes out Stage 4's redesign call.
- `design/phases/tk2-theotokos.md` §0 gained an appended (never
  rewritten) note: A9 and A16 superseded, A14 half-stale, A7's condition
  discharged by D9, D13 disposition cross-referenced to BUG-038.
- `design/adr/ADR-044-theotokos-fixed-panel.md` gained an implementation
  note recording C0–C7 and the two decisions not implemented as literally
  specced (D13's numpad half, cross-surface lock capture per R6/ADR-045).
- `design/adr/ADR-039-performance-state.md` gained a note that decision
  7's `live_rec` slice shipped early (TK2.1 C3b), implemented as written,
  not superseded — the rest of that ADR (kits, temp save, perform mode)
  remains P11-scoped and unimplemented.
- `AGENTS.md` and `README.md` key-control tables rewritten for the
  shipped TK2.1 grammar (REC toggle + REC+PLAY live rec, pad-mode bare
  trig, `n` = ENC, `m` = LOCK, no Mute screen, numpad jog descoped).
- The in-app help overlay (`render.rs::render_help`) regenerated to match:
  rec-mode toggle, pad-mode bare-trig behavior, ENC mode, LOCK, no Mute
  screen.

## Open questions after C7

| OQ | Status |
|---|---|
| OQ-T22 (mute chord vs. screen) | **Resolved** — screen retired (C6), chord is the only gesture |
| OQ-T24 (numpad slot cluster fate) | **Still open**, now unconstrained (A7 discharged by D9) — session #3 |
| OQ-T27 (p-lock authoring gesture) | **Resolved** — ADR-044 D15, shipped C5b |
| OQ-T23b (tap tempo behind a screen) | Open — session #3 |
| OQ-T28 (cross-surface lock capture) | Deferred — ADR-045 (parked, not TK2.1 scope) |
| OQ-T4 (design.md §4.2 step-size scaler) | Open, unchanged by this phase |
| OQ-T21 (KEYBD chromatic grammar) | Open — TK3 |
| OQ-T12 (WT convergence) | Open — after session #3 |

## Implementation notes for the next builder

- `Model::encoder_cursor` and the `>` cursor-prefix rendering it drove
  are gone as of this commit — do not reintroduce them under D13's
  original wording without first checking whether ENC mode still makes
  the concept moot (it does, structurally, as long as D9 stands).
- `Action::Jog`, `Slot`, and the numpad-slot value-routing logic remain
  live, tested, engine-side code (also now reachable via the C5b lock-
  target routing) — only a numpad key that *produces* an `Action::Jog`
  is missing. If session #3 decides to keep the numpad cluster, the jog
  plumbing it would drive already exists; the gap is purely input-side
  (`KeyEventState::KEYPAD` detection, a `PanelButton` variant per slot).
- The `keymap.yaml` degradation (D14) means a stale hand-edited file no
  longer explains a confusing "my bindings vanished" report as loudly as
  it used to — check `cmdline_status`/the startup status line first,
  which now names exactly which entries were skipped.
