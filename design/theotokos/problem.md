# Theotokos — Problem Statement

> **Stage 1 of a staged design.** This document was written first and states
> the problem only. The design is appended aspect-by-aspect in
> `design/theotokos/design.md` as it is determined, so that a different session
> can pick up the work at any point. Do not write code against this track
> until ADR-036 is ratified.
>
> **Authored:** 2026-07-21. **Status:** problem accepted (user brief);
> design in progress.
>
> **Supersession note (2026-07-23):** the *problem* stands, but the
> concrete keymap requirements below were revised by the Elektron
> convergence redesign (ADR-038, accepted; `design.md` Stage 3):
> requirement 2's home-row split grid and top-row tracks → continuous
> `q…i`/`a…k` grid + TRK hold-chord; requirement 3's major modes →
> screens + REC toggle + hold layers; requirement 5's numpad cluster →
> optional accelerator (OQ-T24). Requirements 1, 4, 6–10 stand
> unchanged.

---

## The gap

Paraclete has three control surfaces today, and none of them is the one a
keyboard-native performer would choose:

1. **Hardware grids** (Launchpad, emulator) — pads and LEDs. Great for step
   entry and mutes; no fine parameter control, no screen.
2. **Theoria (tablet/web)** — the accepted *primary control/editing device*.
   Touch encoders, param pages, chain view. It is a glass editing surface:
   excellent for editing, but touch drags are not a keyboard performer's
   instrument, and it presumes a tablet on the rig.
3. **`paraclete-tui`** — a **display-only** ratatui render of transport,
   encoder context, and steps. It reads the state bus and draws; it has
   **zero input**. You cannot play it.

The one surface every rig already has — the laptop keyboard, attached to
the very machine running the graph — is used only as a Launchpad *emulator*
(a grid of letter keys pretending to be pads). That is the worst of both
worlds: it inherits the grid's poverty (no param control, no display) and
adds none of the keyboard's strengths (100+ keys, two-hand touch-typing
posture, chord/prefix composition, a full text screen inches away).

## The problem

**Paraclete lacks a keyboard-first performance surface.** Specifically, no
existing or planned surface provides:

- **Step entry and track selection at typing speed** — the grid mirrors
  pads with keys instead of using the keyboard's own ergonomics.
- **Performance-grade parameter control** — the ability to move **at least
  two parameters simultaneously** (e.g. cutoff + resonance in a filter
  sweep) with hold-to-ramp and fine/coarse control, and to **re-target what
  is being controlled in one keystroke** without looking away.
- **A modal, power-hub interaction model** — the Emacs lesson: a small set
  of modes, composable prefix keys, a command line for the long tail, and
  every frequent gesture one chord away. The instrument vision already
  cites Emacs as the two-tier reference for *engines*; the same lesson
  applies to *interaction*.
- **Real-time terminal graphics worth looking at** — envelopes, signal
  forms, LFO rate/phase, playheads: the terminal as the instrument's
  *screen*, not a debug readout. ADR-026 declared the terminal "the
  instrument's display"; that intent is currently fulfilled by a read-only
  widget strip.

## Relationship to the existing plan (why this is not WT)

The roadmap's **WT** phase ports `paraclete-tui` to an Antiphon client —
the terminal "coming home" as a peer of the web client, rendering the same
generic views (param pages, grid) for *parity*. WT is editor/parity-first
and unstarted.

This track is different in kind, not degree: a **performance-first**
terminal surface with its own native modal interaction logic, designed to
be the fastest way to *play* Paraclete — targeting "as top-level as the
web GUI in base experience, and ideally the best way to use Paraclete for
performance." It does not replace Theoria (glass editing remains primary
for editors) and it does not preclude WT (generic terminal views remain a
valid goal). The two share one substrate — cap-docs, ADR-032 `Rule` view
data, the state bus — which this track consumes as its **second renderer**
(after the web client), proving the universal node-view contract.

The fork question (does this reorganize the design tree?) is answered in
ADR-036: this is a **new track alongside the W-track**, not a fork of it.
Track-local living documents live in `design/theotokos/`; ADRs, phase specs,
session notes, and the roadmap stay in their canonical locations, exactly
as the W-track does.

## Requirements (from the user brief)

1. **Keyboard-first.** Not a pad emulator. The keyboard is the controller;
   the terminal is the screen.
2. **Home-row steps, top-row tracks.** `asdfjkl;` + `zxcvm,./` = the 16
   steps of the default pattern (home row 1–8, lower row 9–16);
3. **Modal, Emacs-flavored.** Modes with distinct keymaps; prefix/leader
   composition; a command line for everything else.
4. **Two-parameter performance.** At least two params modifiable at once,
   hold-to-ramp, fine/coarse, one-keystroke re-targeting. Filter sweeps
   must be a two-finger gesture.
5. **Numpad as the performance cluster.** Full use of a dedicated numpad
   when present; never required. **Phase 1 may assume it is present**;
   a numpad-less fallback layer is a later phase.
6. **Terminal graphics that render the instrument live** — envelopes,
   signal forms, LFO rate, playheads, value motion.
7. **Iterative delivery.** POC first, then usability-tested iterations that
   converge on decisions by battle-testing. No one-shot delivery. Testing
   cycles are part of the roadmap, not an afterthought.
8. **Minimal new dependencies.** `ratatui` + `crossterm` are already
   workspace-pinned and are the expected whole stack.
9. **No wheel reinvention.** Reuse the semantic plane (ADR-019 commands),
   the sequencer command vocabulary, cap-docs/Rule (ADR-032), the state
   bus, and the existing app wiring. Extend scaffolding only where a real
   gap exists (e.g. live visualization data — see design.md §5).
10. **Composability.** Views are data (`Rule`) + state (state bus) +
    renderer. This track must leave the substrate *more* composable than
    it found it, so future interfaces (WT, hardware screens) are views
    over the same contract.

## Non-goals

- **Not an editor replacement for Theoria.** Deep editing (sample slicing,
  graph patching) stays on glass and on the composition floor.
- **Not a DAW.** No timeline, no mouse-first affordances.
- **Not a re-skin of the Launchpad emulator.** The emulator stays as the
  no-hardware dev tool; this track does not emulate pads.
- **No new protocol, no JSON, no socket.** In-process only (see ADR-036).
- **No audio-thread involvement.** Keys are read on the main thread; the
  emulator's current audio-thread keyboard polling is a known defect this
  track designs around, not copies.

## Success criteria (what "done" feels like)

- A performer can build a pattern, sweep a filter pair, ride mutes/fills,
  cue and chain patterns, and play a full set **without touching a mouse,
  a grid, or a tablet** — and prefer it.
- The terminal screen is worth glancing at mid-performance: envelopes move,
  playheads sweep, values breathe.
- Every usability-session finding either lands in the next iteration or is
  an explicit, recorded "won't fix" — convergence by evidence, not taste.
