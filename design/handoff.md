# Paraclete — Implementation Handoff Guide

> **Living document.** Written July 2026 when the specs for the current arc
> were front-loaded. Read this before starting any commit if you are not the
> model/author that wrote the specs.

## The situation

The current arc (P10 + the W-track) was designed and specified in full ahead
of implementation. The specs are the authority; implementation sessions —
possibly by different models with different strengths — execute them. The
design intent is captured in the documents, not in anyone's head.

**Reading order for a fresh session:** `CLAUDE.md` → `design/roadmap.md`
(current sequence) → the phase spec for the commit at hand
(`design/phases/p10-interfaces.md` or `w0-interfaces.md`) →
`design/interface-plan.md` / relevant ADRs only as needed.

## Task routing by tier

Route by the **judgment density** of the task, not its size. When in doubt,
one tier up.

**Sonnet-tier — mechanical, fully specified, verifiable by tests:**
- W1 C0 (CMD_TRIGGER in engines) and W1 C1 (state-path unification) per
  `w1-interfaces.md`
- `web/theoria/` W0 client (zero-build vanilla JS, fully specified in
  `w0-interfaces.md`)
- Marking bugs resolved, report drafting from a commit diff, doc sweeps,
  adding spec'd test lists, BUG-006/BUG-007 when their triggers fire

**Opus-tier — multi-file integration, judgment *within* the spec:**
- W0 Commit 1 (`paraclete-antiphon` crate) and Commit 2 (app wiring)
- P10 C1 (Pattern struct + serializer v3 — data-model restructure, gated)
- P10 C2–C5, W1 implementation
- W1 per `design/phases/w1-interfaces.md` (C0/C1 Sonnet-capable; C2–C4 Opus)
- Post-commit code review passes (the workflow requires them — see below)

**Defer — needs the user, or a frontier-model session, or both:**
- ADR-032 (Theoria view-plugin API freeze at W2) — this is the extension-layer
  contract; do not improvise it
- Protocol v1 freeze (W4); any protocol change beyond the v0 spec
- Re-ordering P10 C2–C5 after paired session #1 (user decision, informed by
  `design/sessions/s1.md`)
- Any deviation from a spec contract; any new ADR

## Guardrails (all tiers)

1. **Specs win.** If the spec and reality conflict, stop, record the conflict
   in the phase report, and ask the user. Do not redesign inline. If a spec is
   silent on a detail, choose the boring option and note it in the report.
2. **Do not revisit named decisions** without the user: no tokio; no Web MIDI
   as primary transport; wire names stay plain; relative-only encoders;
   surfaces are device nodes; DAG + LoopBreakNode; ADR-019 naming contracts;
   the five-layer/license boundaries.
3. **Workflow discipline** (from prior feedback, see memory):
   code review before every commit (subagent); design/doc changes in separate
   commits from code; stop at integration-test milestones to test with the
   user; append-only rules for ADRs, `bugs.md`, phase reports.
4. **Naming policy** (`design/interface-plan.md`): third-party marks never in
   identifiers/features/UI strings; house names (Antiphon, Theoria, kerygma,
   epiclesis, Ordo, Triptych) only in their assigned slots; wire/protocol and
   standard concepts keep plain names.
5. **Audio-thread rules are hard constraints** (CLAUDE.md): no alloc/lock/block
   in `process()`; JSON never touches the audio thread.
6. **Universality check (standing user directive, July 2026):** at every spec
   or implementation pass, ask what is being hard-coded that the vision does
   not require — fixed counts, surface-shaped engine caps, `&'static str` in
   published APIs, single-purpose fields. Flag findings to the user even
   unprompted; file them before format freezes (serializers, wire protocol,
   crates.io APIs), where limitations become permanent.
7. **Every commit:** `cargo test --workspace` green (bare `cargo test` only
   runs the app crate), clippy clean on touched crates, update the phase
   report as you go — not at the end.

## Current sequence (mirror of roadmap, July 2026)

1. ~~P10 C0~~ **shipped** (`b0cf2c8`; BUG-001 re-diagnosed — read the
p10-interfaces amendment before touching sequencer timing) → 2. ~~W0~~
**shipped** (July 2026, both commits; `w0-report.md` has deviations + figures;
tablet-hardware exit checks roll into the next user session) → 3. **P10 C1**
(next; gated; serializer v3 — read the p10-interfaces forward-extensibility
amendment first) → 4. W1
(`w1-interfaces.md`) → 5. **paired session #1** (user; `design/sessions/s1.md`
+ roadmap delta) → 6. P10 C2+ interleaved with W2. Session-0 runbooks
(`s0-launchpad-debug.md`) remain open for the pending human LED/sound
confirmation.

Standing user offers to schedule when hardware is at hand: Digitakt
relative-CC verification (last unverified assumption under the encoder path),
session zero on the current build, BUG-001 before/after audibility check.
