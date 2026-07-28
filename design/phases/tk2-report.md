# TK2 — Performance Layer: Report

**Status:** Code-complete, **NOT signed off** — session #2 reopened the panel layout and default mode model. See `design/sessions/theotokos-2.md`.
**Spec:** `design/phases/tk2-theotokos.md`
**Baseline:** TK1 code-complete (`design/phases/tk1-report.md`, session #2 for TK1 never separately run — folded into this session per D4)

## Commits

| C0 | `040a45f` | Canonical page order → TRIG first (ADR-038 D2b) |
| — | `c39fa60` | Hostile review cycle: findings folded across ADR-038…043 + TK2 spec |
| C1 | `186d780` | Live-trig engine command (D5, `CMD_TRIG_NOW=38`) |
| C2 | `3e55b26` | Panel model — pure types + mapping (D6/D8/D11/D12) |
| C3 | `b631595` | Wiring + render migration (D6/D9/D11/D12) |
| C4 | `26a30b4` | FUNC transport chords + mute chord (D7/A8/A10/A14/A16) |
| C5 | `7e64ea7` | Encoder bank (D8/D13/§0 A11) |
| C6 | `d1164df` | Tempo, Settings, Chain screens (D12) |
| C7 | `5f89c2c` | Agent smoke + polish gate |
| C8 | `5d08fd2`, `e6fe7e3` | Key remapping (ADR-037/D11/D14); ADR-037 accepted |
| C9 | `0c03b24`, `ce067a4` | Live visualization (env_level, lfo_phase); review findings M2–M5 fixed |
| C10 | `theotokos-2.md` | Usability session #2 — **reopens §5.1/§5.2, NOT a clean sign-off** |

## Exit criteria

| Criterion | Status |
|---|---|
| `cargo test --workspace` green through C9 | ✅ full workspace green at each commit |
| Agent smoke run | ✅ (C7) |
| Live-trig, encoder bank, screens, key remapping, live viz | ✅ all functionally shipped |
| Usability session #2 sign-off | ❌ **Did not converge.** Layout + default mode model reopened. |

TK2's code-complete state stands — nothing here is a bug in the shipped
commits. The gap is between the *implemented* rendering/mode-default
choices (all-tracks-stacked grid; REC-armed-by-default) and what design.md
§3.A/§5.1 already intended (trigs always live; one contextual window).
Session #2 is the hands-on evidence the design.md convergence rule (§6)
requires to reopen a DETERMINED item — it supplies that evidence for both.

## Spec/design divergences surfaced by session #2

| design.md said (DETERMINED) | What shipped | Session #2 finding |
|---|---|---|
| §5.1: transport header / **one** contextual window per mode / mode line with live bindings / echo area | GRID screen renders **all tracks simultaneously**, stacked as repeated 2-row blocks — no distinct contextual window separate from the trig display | Reads as a Launchpad LED-grid emulator, not the intended fixed hardware panel. Redesign to a single always-visible 2×8 strip (selected track only) + a genuinely separate contextual pane above it. |
| §3.A point 3/4: "trigs are trigs everywhere"; "on hardware, trigs always play" (motivated the REC-toggle + live-trig command in the first place) | Default launch state is REC-armed; tapping a trig always writes/clears a step, never live-plays | Default should be trig/finger-drum mode (trig = audition + context-switch); REC arms step-entry; transport should not auto-play on launch; PLAY+REC = live record. This is closing a gap against the box's own stated intent, not opening a new one. |
| §0 A9 (TK2 spec amendment): sticky-prefix re-press is a no-op | Implemented as spec'd | Session found this wrong in practice — re-tap should toggle the prefix off. Overrides A9. |

## Open questions resolved / status after session #2

| OQ | Resolution |
|---|---|
| OQ-T22 (mute chord vs. screen) | **Resolved — chord wins.** Mute screen to be retired. |
| OQ-T23 (tempo/tap grammar) | **Converged**, with a friction note (screen-gating) for later. |
| OQ-T24 (numpad slot cluster fate) | **Still open.** No verdict; needs more time and a redesigned context to judge fairly. |
| OQ-T21 (KEYBD chromatic grammar) | Untouched this session; remains TK3-scoped. |

## TK2.x / redesign scope (re-cut by session #2)

Before any TK2-exit scheduling pass (roadmap step 3) can proceed as
originally planned, a short redesign pass is needed:

- Fixed hardware-style layout: persistent 2×8 trig strip (selected track
  only) + track/pattern indicator + contextual window above (mode-driven)
  + a labeled key-legend strip (replacing the scrolling grey hint line)
- Trig cells show their bound key character
- Default-state mode model: trig/finger-drum default, REC arms
  step-entry, no auto-play on launch, PLAY+REC = live record
- Sticky-prefix (D6) toggles off on re-tap (reverses §0 A9)
- Retire the Mute screen; quick-chord only
- Encoder-mode access reconsidered (toggle key vs. held-FUNC),
  variable-rate jog, live viz surfaced in that view, trigs stay visible

This becomes either a TK2.1 addendum to the existing spec or a short new
ADR, whichever the next design pass decides — recommend drafting it before
touching code, per the house front-load rule. A **session #3** should
re-judge the parked/open items (TRK/PTN physical feel, encoder ergonomics,
numpad fate) against the *redesigned* layout, since this session's
confusion on those points was partly caused by the layout itself.

## Implementation notes for the next builder

- The four numbered rows seen in the live session (`1:`/`2:`/`3:`/`4:`,
  each two text-lines tall) are **one block per track**, stacked — this
  matches §5.2's literal "two rows per track" wording taken at face
  value across *all* tracks simultaneously, not the single-selected-track
  strip the redesign now wants. The render code to change is
  `paraclete-theotokos/src/render.rs`'s GRID-screen path (see the
  `RenderData` step-state rendering block referenced in TK0/TK1 reports).
- `keymap.yaml` was absent (clean default state) at session start — no
  stale bindings to account for.
- Session was run live via a detached tmux session
  (`cargo run -- --theotokos`) with the user attaching directly, so key
  events (including any kitty-protocol behavior) came from the user's own
  terminal, not the agent's shell.
