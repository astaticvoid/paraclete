# ADR-036 — Theotokos: Keyboard-First Performance Terminal

**Status:** ✅ **Accepted (2026-07-21)** — ratified by the user after the
pre-ratification design review (findings folded same day, see below).
Standing user directive issued at ratification: **architectural defects
found during design/review work are filed in `bugs.md` against the code,
with design adjustments as necessary — not silently worked around.**
(BUG-032 and BUG-033 were filed under it immediately.)

**Context documents:** `design/theotokos/problem.md` (problem statement),
`design/theotokos/design.md` (full staged design), ADR-018 (cellular
architecture), ADR-019 (universal parameter control), ADR-026 (terminal as
instrument display), ADR-031/ADR-032 (Antiphon; the universal node-view
contract), `design/interface-plan.md` (W-track and WT).

---

## Context

Every control surface Paraclete has or plans is either a pad grid
(Launchpad, emulator), a touch editor (Theoria, primary editing device), or
a display-only terminal (`paraclete-tui`, zero input). None is a
**performance instrument for the keyboard** — the one controller attached
to every rig. The instrument vision's performance ambition ("the immediacy
of a dedicated groovebox… mouse-free") is currently unreachable without
buying hardware or carrying a tablet.

Meanwhile ADR-032 made node views a serializable data contract (`Rule` on
the cap-doc) with exactly one consumer (the web client). A second,
independent renderer is the cheapest possible proof that the universal
node-view contract is real — and the terminal was declared "the
instrument's display" as far back as ADR-026.

Two scaffolding facts shape the decision (full inventory in the design
session notes, 2026-07-21):

1. The emulator reads the keyboard **on the audio thread** inside
   `process()` — a defect the new surface must design around, not copy.
2. No live visualization data (envelope level, LFO phase, scope) is
   published anywhere today; only static `Rule` affordance metadata exists.

## Decision

1. **A new interface track — Theotokos — alongside the W-track, not a fork of
   it.** Track-local living docs in `design/theotokos/`; ADRs, phase specs,
   session notes, and roadmap entries stay in their canonical locations
   (the W-track pattern). WT (Theoria/term parity client) is unaffected;
   convergence is revisited with session evidence at TK3 (OQ-T12).

2. **New crate `paraclete-theotokos`** (GPL3 platform crate, same standing as
   `paraclete-tui`). Dependencies: `ratatui`, `crossterm` (already
   workspace-pinned), `paraclete-node-api`. **No new dependencies.**

3. **In-process semantic plane; environment, not a node.** Theotokos reads the
   shared `StateBusHandle` + startup cap-doc cache, and mutates only via
   ADR-019 commands and the declared sequencer vocabulary (16–32), drained
   as `Vec<NodeCommand>` by the app each main-loop iteration — the
   established scripting/Antiphon egress pattern. It is **not** a `Surface`
   node, has no gateway, and runs **no Rhai**: its modal state machine is
   native and unit-tested. This is the same ADR-018 exemption shape as
   `paraclete-tui`, the scripting engine, and the Antiphon server.
   **No JSON, no sockets, no new protocol.**

4. **Views = `Rule` + state bus + renderer.** Theotokos renders param pages,
   envelope groups, and affordance hints from `Rule` — the second consumer
   of the universal node-view contract. New engines get a terminal view
   with zero Theotokos changes. No terminal-only side doors into the engine.
   **Scope note (review M2):** TK0 consumes *engine-local* Rules; the
   track-*composite* assembly (ADR-032 §7) currently lives in
   `paraclete-antiphon`, is protocol-shaped, and is built only when
   Antiphon is enabled. Hoisting it to a location both renderers share is a
   TK1 decision (OQ-T13), not a new architecture.

5. **Keyboard-first modal interaction** — home-row steps (`asdfjkl;`),
   top-row tracks (`qweruiop`, invariant across modes), a numpad
   performance cluster with **three always-live focus slots** (two-param
   simultaneous sweeps are the floor), hold-to-ramp jog, param-page
   retargeting in one keystroke, modes with per-mode keymaps, leader
   families, and a `:` command line for the long tail. This ADR freezes the
   **model** (modes, focus slots, semantic-plane-only mutation); **chord
   grammar is deliberately not frozen** — it converges through the
   usability-session protocol in `design/theotokos/design.md` §6.

6. **Terminal ownership on the main thread.** Raw mode, alternate screen,
   and keyboard polling live on the main thread (never in `process()`).
   Enabled by `--theotokos`; mutually exclusive with the emulator and the old
   TUI (one screen owner). `paraclete-tui` stays in place; its retirement
   is a separate user decision after Theotokos reaches parity-plus.

7. **Platform extensions, pre-approved in direction, frozen per phase
   spec:**
   (a) **Transport control (TK0).** `InternalClock` gains node-specific
   command handling (start/stop; `bpm` becomes a real bank param) —
   verified 2026-07-21 that nothing on any surface can play/stop the
   standalone instrument today (the clock handles no commands;
   `ConfigMessage::SetPlaying` injects no `TransportEvent`s; scripting
   sandbox rejects `/transport/*` writes; Antiphon has no transport
   message). TK0's `Space` depends on it.
   (b) **Live visualization (TK2).** `EnvelopeNode`/`LfoNode` publish
   `/node/{id}/state/{env_level,lfo_phase}` via the existing
   `published_state()` push-down (no trait changes).
   (c) **Scope tap (TK2).** Optional master scope via an rtrb SPSC ring
   tapped at the mix output (`try_push`, drop-on-full — the established
   real-time-safe idiom; the test-driver's `CaptureRing` is harness-grade
   and not the model).
   Static `Rule`-derived graphics are the baseline and need no engine
   changes (noting `LfoNode`/`EnvelopeNode` declare no `Rule` today — a
   small L3 addition, TK-scoped).

## Alternatives considered

### A. Build it as an Antiphon in-process client now (rejected)

The in-process transport does not exist (WT would create it). Building it
first adds session/serialization machinery for zero in-process benefit —
the semantic plane already exists as plain function calls. Theotokos keeps its
`Action`→`NodeCommand` resolution pure and transport-free so a later
in-process Antiphon client remains possible without redesign.

### B. A Surface node + Rhai profile (rejected)

Surface/gateway/profile exists to put *hardware* semantics in scripts.
Modal interaction state in Rhai is the wrong tool: slower, untestable as a
pure function, and it would couple the keymap to profile loading. Profiles
remain the hardware-surface mechanism, untouched.

### C. Extend `paraclete-tui` (rejected)

It is display-only with a different purpose (ADR-026 feedback surface).
Accreting a modal input machine onto it entangles two designs; Theotokos
supersedes by replacement (flag-gated), not by growth.

### D. Wait for WT (rejected)

WT is editor/parity-first (generic views in the terminal). The gap is a
*performance* surface; parity work does not close it.

## Consequences

- **New:** `crates/paraclete-theotokos`; `--theotokos` app flag; phase specs
  `design/phases/tkN-*.md`; session notes `design/sessions/theotokos-N.md`;
  `/script/theotokos/*` state-bus namespace (in-process writes only).
- **Testing posture:** pure keymap/resolution unit tests, `TestBackend`
  render tests (no insta snapshots — SPIKE-005), test-driver scenarios for
  the command contract, and a **paired usability session gating every TK
  phase**. No chord grammar is frozen without session evidence.
- **Hard constraints reaffirmed:** no audio-thread input; no alloc/lock/
  block in `process()` (the scope tap uses the existing SPSC idiom);
  layer-clean (`paraclete-theotokos` depends on `paraclete-node-api` only);
  no hardcoded counts — track count, page count, and slot bindings come
  from cap-docs/`Rule`, and the POC's hardcoded ids are replaced by
  discovery in TK1.
- **Naming:** "Theotokos" (Θεοτόκος, "the bearer") continues the liturgical
  register (Antiphon, Theoria, kerygma, epiclesis); it is the shipped
  embodiment of the `interface-plan.md` vocabulary ("the controls are
  praxis") without spending the plain word. Third-party marks remain
  design-prose analogy only; the keymap, modes, and UI strings use plain or
  house words.
- **Doc trail:** `design/theotokos/problem.md` + `design.md` are the living
  design; this ADR's status line and implementation notes update as TK
  phases land (append-only body per house rule).

## Relation to other ADRs

- **ADR-018** — Theotokos is environment (main-thread plumbing), not a node.
- **ADR-019** — the only mutation path; relative-only `CMD_BUMP_PARAM` for
  all performance motion.
- **ADR-026** — fulfills "the terminal is the instrument's display" with an
  *interactive* terminal; `paraclete-tui` untouched until the retirement
  decision.
- **ADR-031/032** — Theotokos is the second `Rule` consumer; no protocol
  changes, no Antiphon dependency.
- **ADR-033/035** — the test-driver verifies the command contract Theotokos
  emits; baselines guard the engine beneath it.

---

## Implementation notes

**2026-07-23 — interaction model superseded in part by ADR-038 (🟡
proposed).** User-directed Elektron-convergence redesign: decision 5's
concrete keymap (invariant `qweruiop` track row, home-row split step grid,
SEQ/PERF modes) is replaced by a virtual front panel — TRK/PTN hold-chords,
REC grid-recording toggle, continuous two-row trig grid, FUNC(Shift)-layer
8-encoder bank. The *model-level* commitments here (modal-state machine as
pure native code, focus-slot floor of two-param simultaneity, semantic-
plane-only mutation, session-gated chord convergence) stand; the two-param
floor is now met by the encoder bank. See ADR-038 and `design/theotokos/
design.md` Stage 3.

**2026-07-21 — accepted.** Ratified by the user; the review amendments
below were folded pre-ratification. BUG-032 (transport) and BUG-033 (TUI
stale path) filed at acceptance under the standing defect-filing
directive. Next artifact: `design/phases/tk0-theotokos.md` (POC spec);
TK0 Commit 0 implements decision 7(a).

**2026-07-21 — pre-ratification design review (subagent).** All factual
code claims verified (9/10 lettered claims line-for-line; layer story,
dependency claims, reuse inventory, test seams confirmed). Findings folded
into this ADR before ratification:

- **B1 (blocker):** transport control gap → new decision 7(a); TK0-scoped;
  shape question logged as OQ-T14.
- **M1:** the default `instrument.yaml` is 4-track (seqs 10–13, voices
  20–22/27), not the 8-track graph in AGENTS.md's ID table → TK0 scope
  corrected in `design.md`; AGENTS.md annotated.
- **M2:** composite `Rule` assembly location → scope note on decision 4;
  OQ-T13 for TK1.
- **M3:** p-lock authoring is a *design* task (packed CMD 33+), not a
  confirmation → OQ-T8 sharpened.
- **m7:** ADR-018's "sole exemption" wording — implementation note appended
  to ADR-018 recording the Antiphon/TUI/Theotokos environment standing.

Full findings: `design/theotokos/design.md` amendment log.
