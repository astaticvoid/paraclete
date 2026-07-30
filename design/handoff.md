# Paraclete — Implementation Handoff Guide

> **Living document.** Written July 2026 when the specs for the current arc
> were front-loaded. Read this before starting any commit if you are not the
> model/author that wrote the specs.

## ▶ START HERE — active implementation phase (2026-07-30)

**TK2.2 is CODE-COMPLETE (C0–C5 shipped `9f62cef`…`8595db6`, plus same-day
follow-up `4fe3f5f`) but NOT CLOSED.** Do not re-implement any of C0–C5 —
read `design/phases/tk2.2-theotokos.md`'s "Implementation report" section
(appended 2026-07-30, at the end of the file) before touching this phase
at all. The only remaining step is **C6 — usability session #4**, a
user-paired session with no code component. If you are a cold-start agent
and no user session is in progress, there is **no autonomous next-phase
work queued** in this track; see "If you have no user session" below.

Design authority (both now **implemented**, not just accepted): ADR-044
(amended by session #3 — see its "Session #3 outcome" note), ADR-046 (C5;
see its own "Implementation note" appended 2026-07-30 — it documents one
real gap the ADR's migration inventory missed, found and fixed same-day).

**If you have no user session:** do not invent Theotokos work. Options,
in priority order: (1) INFRA-008 (`design/bugs.md`) — gate confirmed
lifted 2026-07-30 (Theotokos has been the default surface since TK0/TK1),
fix direction is concrete (move `LaunchpadEmulator`'s keyboard poll off
the audio thread into a main-thread-ticked handle, mirroring the existing
`SurfaceOutputHandle` pattern), not yet started; (2) BUG-042 (live_rec
double-trigger) — fix direction filed, touches the same delicate
`early_fired` boundary logic as BUG-004, so budget care not speed;
(3) BUG-047 (`kick_reverb_clean.yaml` dropout) — not yet investigated
even at the "is this real" level, start there before touching DSP. None
of these are Theotokos/TK2.2 work — do not fold them into this phase's
commit sequence.

**Historical note, kept for the next hostile-review cycle in this
family:** every one of C0–C5's reviews found something, from a wording
nit to (at C5) a real blast-radius miss — the ADR's own "migration cost is
small" claim missed two consumers (`tools/test-driver`, `launchpad.rhai`)
that had never sent a clock command at all, relying on the old auto-start
default implicitly. The gate caught it after the commit landed, not
before — a reminder that "who commands X" is not the same question as
"who currently depends on X's default behavior without commanding it"
(AGENTS.md design-process-learning #2).

**Deliberately parked, not forgotten:** ADR-045 (cross-surface lock
capture) — its premise was empirically confirmed by session #3, but session
#4 should first judge whether TK2.2 E4's latched-only p-lock suffices in
play. Do not unpark it until C6 happens.

## The situation

The current arc (P10 + the W-track) was designed and specified in full ahead
of implementation. The specs are the authority; implementation sessions —
possibly by different models with different strengths — execute them. The
design intent is captured in the documents, not in anyone's head.

**Reading order for a fresh session:** `AGENTS.md` → `design/roadmap.md`
(current sequence) → the phase spec for the commit at hand
(`design/phases/p10-interfaces.md` or `w0-interfaces.md`) →
`design/interface-plan.md` / relevant ADRs only as needed.

For new tools/components outside the phase structure: find the ADR in
`design/adr/` (e.g. ADR-033 for the headless test driver). ADRs are the spec;
implementation follows in a separate commit.

For the Theotokos performance-terminal track (ADR-036 **accepted
2026-07-21**): `design/theotokos/problem.md` → `design/theotokos/design.md`
→ the current phase spec. Track history: TK0 and TK1 shipped. **TK2** was
code-complete (C0–C9) but did not sign off — usability session #2 reopened
the panel layout and the default mode model (`theotokos-2.md`,
`tk2-report.md`). **TK2.1** (ADR-044, ratified 2026-07-28) rebuilt the panel
and is **code-complete C0–C7**; usability **session #3 held 2026-07-29**
signed off the redesign but found four bugs and two structural design
collisions inside it (`design/sessions/theotokos-3.md`,
`design/phases/tk2.1-report.md` — that report is **closed**). **TK2.2**
(ADR-046, accepted + implemented 2026-07-30) fixed those four bugs and the
legend/chip/jog-feedback follow-ups and is **code-complete C0–C5**; only
usability session #4 (C6) remains.
**→ See "START HERE" at the top of this file — do not re-implement C0–C5.**

## Task routing by tier

Route by the **judgment density** of the task, not its size. When in doubt,
one tier up.

**Session orchestrator is Opus** (decided July 2026; Fable/frontier tier is
off the table). The orchestrator holds session context, makes delegation
calls, verifies returned work for correctness, and writes gated commit
messages and spec-conflict reconciliations — it is never delegated down.
Delegate *to* a subagent with an explicit model param when there is a real
batch to amortize the cold start (a spec'd test-stub list, a multi-file doc
sweep); do trivial single actions (one commit, marking one bug resolved)
inline. See `[[feedback-model-routing-subagents]]`.

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

**Defer — needs the user (with Opus), not a higher tier:**
(These once read "frontier-model session"; with Fable off the table the top
available tier is Opus, so these gate on the *user*, not on a bigger model.
They are judgment calls to make *with* the user in session, not solo.)
- ~~ADR-032 (Theoria view-plugin API freeze at W2) — this is the extension-layer
  contract; do not improvise it~~ **accepted 2026-07-13. W2 implementation can proceed.**
- Protocol v1 freeze (W4); any protocol change beyond the v0 spec
- Re-ordering P10 C2–C5 after paired session #1 (user decision, informed by
  `design/sessions/s1.md`)
- Any deviation from a spec contract; any new ADR

## Guardrails (all tiers)

1. **Specs win.** If the spec and reality conflict, stop, record the conflict
   in the phase report, and ask the user. Do not redesign inline. If a spec is
   silent on a detail, choose the boring option and note it in the report.
   **New tools/components outside existing phases: write an ADR first, get
   approval, then implement. Never jump to code.**
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
5. **Audio-thread rules are hard constraints** (AGENTS.md): no alloc/lock/block
   in `process()`; JSON never touches the audio thread.
6. **Universality check (standing user directive, July 2026):** at every spec
   or implementation pass, ask what is being hard-coded that the vision does
   not require — fixed counts, surface-shaped engine caps, `&'static str` in
   published APIs, single-purpose fields. Flag findings to the user even
   unprompted; file them before format freezes (serializers, wire protocol,
   crates.io APIs), where limitations become permanent.
7. **Defect-filing (standing user directive, 2026-07-21):** architectural
   defects found during design or review work are **filed in `bugs.md`
   against the code** and the design is adjusted — never silently worked
   around in prose. (Filed under it so far: BUG-032, BUG-033, INFRA-008.)
8. **Every commit:** `cargo test --workspace` green (bare `cargo test` only
   runs the app crate), clippy clean on touched crates, update the phase
   report as you go — not at the end.

## Current sequence (mirror of roadmap, updated 2026-07-10 evening)

1. ~~P10 C0 / W0 / P10 C1 / W1 / session #1~~ shipped (roadmap history) →
2. ~~Theoria legibility phase~~ **shipped 2026-07-10**
   (`theoria-legibility-report.md`; BUG-016…021; open-by-default + `--token`;
   USB-C tablet link @ 3.0 ms) → 3. ~~paired session #2~~ **held 2026-07-10**
   (`design/sessions/s2.md` — READ IT: it re-founds the W-track) →
4. **next, in order:**
   - ~~**(a) Theoria baseline interaction wins**~~ — **shipped 2026-07-11**
     (`e1f86cf` + `3356d60`, `theoria-baseline-interactions-report.md`):
     drag-draw paints via the new `/script/lp/steps_n` mask mirror; encoder
     row at the bottom with value bars; dead grid gone. BUG-024 (state_write
     in subscriptions panicked the app) found + fixed. One Chrome pass on
     the final bundle + tablet judgment still pending.
   - ~~**(b) P10 C2–C3 pattern depth**~~ — **C2–C5 all shipped 2026-07-11**
     (`0a8116b`/`e8f7718`/`50ef64b`/`5306674`; `p10-report.md`). BUG-004
     fixed in C3; BUG-013 fully resolved 2026-07-11 (engines `309a9e6`;
     Sampler: SincFixedOut → load-time resample + Hermite playback with
      span-split voice starts — see bugs.md). BUG-025 (executor defers
      cross-block offsets) and BUG-026 (stable sort) both fixed 2026-07-11;
      BUG-014 (emulator ButtonPressed) fixed same session. §5.3 Launchpad
      surface parked per s2; the P10 play-test gates on a paired session
      (TUI + command injection, or Theoria post-W2).
     - **(c) W2 re-scoped: reference-design spike** — **paired session held 2026-07-13.** Decisions §6.0–6.7 + OQ-13/OQ-14 ratified. W2 spec drafted (`design/phases/w2-interfaces.md`), ADR-032 proposed (`design/adr/ADR-032-theoria-view-plugin-api.md`). **Next: user ratifies the spec + ADR freeze, then W2 implementation begins.**
     - **(d) Headless test driver** — **ADR-033 batch mode shipped 2026-07-11**
       (`e265666`). Loads instrument YAML + test scenario YAML, builds graph,
       runs audio capture, executes timed commands, checks assertions, writes
       WAV. Interactive debug REPL deferred to v2. 4 audit tests pass
       validating 10 ADR findings. See `design/review/adr-latent-issues.md`.
5. **Parked/waiting:** Launchpad track frozen (s2 F2; s1-F7 cleanup in the
    trigger backlog); BUG-023 resolved (macOS speaker protection confirmed via
    headphone A/B); BUG-012 queued for a hardware session; BUG-027 engine
    exonerated by measurement — pending user headphone A/B on afplay start,
    same protocol as BUG-023 (see bugs.md addendum).
6. **Autonomous session 2026-07-12:** INFRA-001 (artifact assertions) and
    INFRA-002 (test-driver quick mode `--trigger kick --at 1.0 -d 3`)
    shipped. Found+fixed: BUG-028 (omitted trigger note sent 0 — all
    test-driver trigger renders were mistuned), BUG-029 (failed apply_patch
    stranded the engine paused; audit item #4), BUG-030 (remove_node
    destroyed devices on refusal). ADR latent-issue audit items #1, #2, #5,
    #6, #7, #10, #11 validated — see the round-2 tables in bugs.md.
    Remaining audit items need the interactive harness (ADR-033 v2), stress
    runs, or hardware.

7. **Session 2026-07-12 (late):** vision crystallized
    (`instrument-vision.md` — "Performance Meets Limitless Composability") and the
    W2 reference spike finished (`design/specs/w2-reference-analysis.md`, all four
    manuals, 8 convergent patterns, decision menus). Roadmap **"Active
    Priorities"** is now the current ranked view; start there. Runtime
    observability measured quiet (ADR-034 counters + live-audio load test);
    BUG-012 confirmed a hard crash on buffer-size mismatch. **Active agent task:
    the pre-W2 universality / hardcoded-count audit** (must land before the
    interface/protocol/serializer freeze).

Standing design tiebreakers (s2): synthesis voices are the emotional core —
"sit down with a nice kick engine and tune it"; interface work starts from
studied reference manuals, never improvised. Naming policy: reference marks
(Digitakt, Syntakt, Hydrasynth…) live in design docs only.

Standing design tiebreakers (added 2026-07-12): **one graph, layered surfaces**
(hardware-style performance / signal-flow graph / mouse+keyboard floor); **every
node has a graceful-degrading view** (ADR-032 = the *universal node-view
contract*, not just engine param pages); **two-tier engines** ("elisp of
machines" — fast monolithic *and* graph-composed, never forced to choose);
**modulation is graph edges** (limitless LFOs/envs, no slot count); **no
hardcoded counts in any frozen format**. **Tone (public repo):** *do* name
Elektron / Hydrasynth as the aspirational bar — they set the standard, built by
teams who love the craft, and we keep them in view. Frame humbly and clear-eyed:
a small open project may not match their per-surface polish. The pitch is the
*combination* (performance immediacy + open composability, one graph, any
controller, free license), **never** superiority over a named product.

Standing user offer to schedule when hardware is at hand: BUG-001
before/after audibility check.

**Do not revive the Digitakt-as-encoder-controller idea.** It was checked
on hardware 2026-07-04 and disqualified: the Digitakt II transmits
**absolute** CC (knob position, mirrored from the internal parameter) and
Elektron's MIDI implementation has no relative mode
(`design/sessions/s0-hardware-checks.md`, Check 1; side finding BUG-009).
It survived in later docs as a "standing offer" and an "unverified
assumption" long after it was settled — it is neither. Paraclete's
relative-only encoder contract needs a controller that transmits knob
*deltas*; no such device is on hand (SPIKE-006 explores what the
LaunchControl XL's pots can and cannot do instead). Digitakt remains a
*design reference* for workflow, per naming policy — never a control
surface for parameter editing.
