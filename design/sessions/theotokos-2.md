# Theotokos — Usability Session #2

**Date:** 2026-07-27
**Phase:** TK2 C10 (exit criterion) — also closes the outstanding TK1 C8 obligation
**Session type:** hands-on, default 4-track instrument, `cargo run -- --theotokos`, live in a shared tmux session (`tk2-session2`)

## Verdict: NOT a clean sign-off. TK2 code-complete but the panel does not read as the intended Elektron-class surface. Reopens two DETERMINED design.md sections (§5.1/§5.2) on new session evidence, per §6's convergence rule.

The underlying command/state plumbing (live-trig, encoder resolution,
tempo derivation, key remapping, mute state) all work. What doesn't
work is the **default rendering and mode model** — it reads as a
Launchpad-style LED-grid emulator (all tracks stacked as rows), not a
fixed hardware panel (one always-visible trig strip + contextual
window above it), and the **default transport/REC posture inverts the
Digitakt-style finger-drum model** design.md §3.A itself already
specifies (point 3/4: "trigs are trigs everywhere," "on hardware,
trigs always play"). This session is the evidence that reopens those
two DETERMINED items — it does not invalidate the input-grammar layer
(TRK/PTN hold, REC toggle, FUNC plane) underneath.

## Converged hypotheses

| Hypothesis | Verdict |
|---|---|
| FUNC+transport copy/clear/paste (D7) | **Converged (provisional).** Works; ergonomics ("crazy workflow") not optimized — revisit inside the general redesign pass, not urgent standalone. |
| Tempo screen + YES-tap (D12/OQ-T23) | **Converged**, with reservation — tap-tempo mechanic itself is good; gating it behind a dedicated screen nav feels like friction. Consider a global tap-tempo chord later. |
| Yellow-flash legibility (TK1 carry-over) | **Converged.** Visible and well-timed as-is. |
| Composite page order legibility (TK1 carry-over) | **Converged** for now — not confusing, not a priority to revisit. |

## Revision findings (redesign scope)

| Finding | Source | Action |
|---|---|---|
| **GRID screen renders as an all-tracks-stacked LED grid** (4 rows × 8 cols, one 2-row block per track), reading as Launchpad, not Elektron | H1 (D1) | Reopens design.md §5.1/§5.2. Redesign to: one always-visible 2×8 trig strip (selected track only) + single track/pattern indicator + a **contextual window above** that changes with editing mode — matches the *already-DETERMINED* §5.1 skeleton (`contextual window / mode line / echo area`) more literally than the current implementation does. |
| **Trig cells don't show their mapped key label** | H1 | Print the bound key character inside each trig cell (e.g. `q`, `w`…) for discoverability. |
| **Sticky-prefix re-tap should toggle off, not no-op** | H2 (D6) | **Overrides §0 A9** ("same-prefix re-press is a no-op") from the TK2 spec's amendment section. Re-tapping an armed prefix should disarm it (return to previous state). Flagging explicitly since A9 was itself a hostile-review-driven decision — this session's live-play evidence reverses it. |
| **No persistent key-legend / labeled function strip** | H1, H2 | The bottom grey help line doesn't serve as a legend. Want an Elektron-style labeled column/strip (e.g. `[TAB] TRK`) always visible, not a scrolling hint line. Same root cause as the two rendering findings above — fold into one layout redesign, not three separate fixes. |
| **Held-FUNC for encoder access feels wrong** | H4 (D13/OQ-T24) | Consider a **toggle key** that switches the trig grid into "encoder mode," replacing the held-`Shift` gesture. |
| **Encoder jog has no variable step size** | H4 | Params with different ranges need proportionally different jump distances per jog tick, not one flat increment. |
| **Live viz (env/LFO) should surface inside the encoder view** | H4 | Ties to already-shipped C9 (`env_level`/`lfo_phase` publish) — route that into whatever the encoder-mode contextual window becomes. |
| **Trigs disappearing in encoder mode** | H4 | Reinforces the H1 finding: trig strip should stay visible regardless of mode. |
| **Mute quick-chord wins outright over the Mute screen** | H6 (OQ-T22) | **Resolved, not just a preference — scope cut.** Retire the dedicated Mute screen (`m`); TRK+FUNC+trig is the only mute mechanism going forward. |
| **`grid_rec` should NOT default armed; trig should default to live-play** | H8 (D12) | **Significant mode-model change**, and arguably closes a gap against design.md §3.A points 3–4 rather than opening a new one: default screen = finger-drum/trig-mode (tapping a trig **plays** that track live *and* switches the contextual display to its context); REC arms step-record (only then does tapping a trig write/clear a step); transport must **not auto-play on launch**; PLAY+REC together = real-time/live record into the pattern. |

## Parked / deferred (no session verdict)

| Item | Reason |
|---|---|
| TRK/PTN hold mechanics — physical feel (D1 input side) | Visual confusion (H1's layout finding) made this hard to judge cleanly this session; revisit once the layout redesign lands. |
| Encoder bank simultaneity + numpad cluster fate (OQ-T24) | User needs more time; "not intuitive anyway" but not enough signal yet for a verdict either way. Still genuinely open. |
| `:` runtime remap discoverability (D11) | Explicitly deferred by user request — "just want it there, can be designed later." Functionally present, UX not evaluated. |

## Roadmap deltas

- TK2 is **not** exiting to "code-complete, proceed to TK2-exit scheduling pass" as the spec assumed. A **redesign pass** is needed first, covering: fixed hardware-style layout (persistent trig strip + contextual window + key legend), trig-mode-default/REC-arms-step-entry mode model, sticky-prefix toggle behavior, and Mute-screen retirement.
- Concrete scope cut: **retire the Mute screen** (`m`).
- design.md §5.1/§5.2 reopened (were DETERMINED); §3.A points 3–4 flagged as an implementation gap against already-accepted intent, not a new design question.
- TK1 C8 obligations formally closed by this session — all live items (`\` leader, Shift+track mute, number-row pattern select) were already superseded by ADR-038 and are dropped without further action; yellow-flash and composite-page-order carried over cleanly and converged.
- Next: a short design pass (new ADR or TK2.5 addendum) to freeze the redesigned layout + mode model, then implement, then a **session #3** to re-judge (this session's open/parked items — TRK/PTN feel, encoder ergonomics, numpad fate — need re-testing against the new layout, not against this one).
