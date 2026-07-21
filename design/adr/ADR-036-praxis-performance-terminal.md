# ADR-036 — Praxis: Keyboard-First Performance Terminal

**Status:** 🟡 **Proposed (2026-07-21)** — user ratification required before
any code (handoff guardrail: new ADRs gate on the user).

**Context documents:** `design/praxis/problem.md` (problem statement),
`design/praxis/design.md` (full staged design), ADR-018 (cellular
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

1. **A new interface track — Praxis — alongside the W-track, not a fork of
   it.** Track-local living docs in `design/praxis/`; ADRs, phase specs,
   session notes, and roadmap entries stay in their canonical locations
   (the W-track pattern). WT (Theoria/term parity client) is unaffected;
   convergence is revisited with session evidence at PRX3 (OQ-P12).

2. **New crate `paraclete-praxis`** (GPL3 platform crate, same standing as
   `paraclete-tui`). Dependencies: `ratatui`, `crossterm` (already
   workspace-pinned), `paraclete-node-api`. **No new dependencies.**

3. **In-process semantic plane; environment, not a node.** Praxis reads the
   shared `StateBusHandle` + startup cap-doc cache, and mutates only via
   ADR-019 commands and the declared sequencer vocabulary (16–32), drained
   as `Vec<NodeCommand>` by the app each main-loop iteration — the
   established scripting/Antiphon egress pattern. It is **not** a `Surface`
   node, has no gateway, and runs **no Rhai**: its modal state machine is
   native and unit-tested. This is the same ADR-018 exemption shape as
   `paraclete-tui`, the scripting engine, and the Antiphon server.
   **No JSON, no sockets, no new protocol.**

4. **Views = `Rule` + state bus + renderer.** Praxis renders param pages,
   envelope groups, and affordance hints from `Rule` — the second consumer
   of the universal node-view contract. New engines get a terminal view
   with zero Praxis changes. No terminal-only side doors into the engine.

5. **Keyboard-first modal interaction** — home-row steps (`asdfjkl;`),
   top-row tracks (`qweruiop`, invariant across modes), a numpad
   performance cluster with **three always-live focus slots** (two-param
   simultaneous sweeps are the floor), hold-to-ramp jog, param-page
   retargeting in one keystroke, modes with per-mode keymaps, leader
   families, and a `:` command line for the long tail. This ADR freezes the
   **model** (modes, focus slots, semantic-plane-only mutation); **chord
   grammar is deliberately not frozen** — it converges through the
   usability-session protocol in `design/praxis/design.md` §6.

6. **Terminal ownership on the main thread.** Raw mode, alternate screen,
   and keyboard polling live on the main thread (never in `process()`).
   Enabled by `--praxis`; mutually exclusive with the emulator and the old
   TUI (one screen owner). `paraclete-tui` stays in place; its retirement
   is a separate user decision after Praxis reaches parity-plus.

7. **Live visualization extensions, pre-approved in direction, frozen per
   phase spec:** (b) `EnvelopeNode`/`LfoNode` publish
   `/node/{id}/state/{env_level,lfo_phase}` via the existing
   `published_state()` push-down (no trait changes); (c) an optional master
   scope via an SPSC capture ring tapped at the mix output (`try_push`,
   drop-on-full — the established real-time-safe SPSC shape). Static
   `Rule`-derived graphics are the baseline and need no engine changes.

## Alternatives considered

### A. Build it as an Antiphon in-process client now (rejected)

The in-process transport does not exist (WT would create it). Building it
first adds session/serialization machinery for zero in-process benefit —
the semantic plane already exists as plain function calls. Praxis keeps its
`Action`→`NodeCommand` resolution pure and transport-free so a later
in-process Antiphon client remains possible without redesign.

### B. A Surface node + Rhai profile (rejected)

Surface/gateway/profile exists to put *hardware* semantics in scripts.
Modal interaction state in Rhai is the wrong tool: slower, untestable as a
pure function, and it would couple the keymap to profile loading. Profiles
remain the hardware-surface mechanism, untouched.

### C. Extend `paraclete-tui` (rejected)

It is display-only with a different purpose (ADR-026 feedback surface).
Accreting a modal input machine onto it entangles two designs; Praxis
supersedes by replacement (flag-gated), not by growth.

### D. Wait for WT (rejected)

WT is editor/parity-first (generic views in the terminal). The gap is a
*performance* surface; parity work does not close it.

## Consequences

- **New:** `crates/paraclete-praxis`; `--praxis` app flag; phase specs
  `design/phases/prxN-*.md`; session notes `design/sessions/praxis-N.md`;
  `/script/praxis/*` state-bus namespace (in-process writes only).
- **Testing posture:** pure keymap/resolution unit tests, `TestBackend`
  render tests (no insta snapshots — SPIKE-005), test-driver scenarios for
  the command contract, and a **paired usability session gating every PRX
  phase**. No chord grammar is frozen without session evidence.
- **Hard constraints reaffirmed:** no audio-thread input; no alloc/lock/
  block in `process()` (the scope tap uses the existing SPSC idiom);
  layer-clean (`paraclete-praxis` depends on `paraclete-node-api` only);
  no hardcoded counts — track count, page count, and slot bindings come
  from cap-docs/`Rule`, and the POC's hardcoded ids are replaced by
  discovery in PRX1.
- **Naming:** "Praxis" adopts the house vocabulary already assigned in
  `interface-plan.md` ("the controls are praxis"). Third-party marks remain
  design-prose analogy only; the keymap, modes, and UI strings use plain or
  house words.
- **Doc trail:** `design/praxis/problem.md` + `design.md` are the living
  design; this ADR's status line and implementation notes update as PRX
  phases land (append-only body per house rule).

## Relation to other ADRs

- **ADR-018** — Praxis is environment (main-thread plumbing), not a node.
- **ADR-019** — the only mutation path; relative-only `CMD_BUMP_PARAM` for
  all performance motion.
- **ADR-026** — fulfills "the terminal is the instrument's display" with an
  *interactive* terminal; `paraclete-tui` untouched until the retirement
  decision.
- **ADR-031/032** — Praxis is the second `Rule` consumer; no protocol
  changes, no Antiphon dependency.
- **ADR-033/035** — the test-driver verifies the command contract Praxis
  emits; baselines guard the engine beneath it.
