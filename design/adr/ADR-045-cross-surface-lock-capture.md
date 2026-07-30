# ADR-045 — Cross-surface parameter-lock capture

| Field | Value |
|-------|-------|
| **Status** | 🟡 Proposed (2026-07-28) — **premise empirically confirmed 2026-07-29** by Theotokos usability session #3; recommend unparking (see "Session #3 evidence" below) |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Awaits user decision on R1–R3 below |
| **Scope** | `paraclete-app` (main-loop command drain), `paraclete-theotokos`, `paraclete-scripting` (one state-bus path), no engine change |
| **Related** | ADR-019 (semantic plane), ADR-044 D15 (the lock target), ADR-018 (environment exemption), ADR-032/`Rule` (per-track node ownership), ADR-031 (Antiphon), TK1 D3 (CMD 33–35) |

Third-party marks appear per house naming policy: design prose only, never
identifiers or UI strings.

---

## Context

ADR-044 D15 gave Theotokos a p-lock gesture: a **lock target**
(`track`, `step`), set momentarily by holding a trig where the terminal
reports key releases, or latched otherwise. While it is armed, Theotokos
rewrites *its own* parameter motion into the shipped lock pair —
`CMD_SET_LOCK_TARGET` (33) + `CMD_SET_STEP_LOCK` (34) — on that track's
sequencer.

That covers the keyboard. It does not cover the workflow the design is
actually reaching for, which is the reference box's: **hold the step with
one hand, move a value with the other.** On a keyboard the two hands
compete for the same rows — in ENC mode the trig rows *are* the encoders,
so the holding hand has nowhere to be. The way out is that the value does
not have to come from the keyboard at all:

- **Theoria's touch encoders are relative today** (W1 C0 →
  `CMD_BUMP_PARAM`). Hold the step in the terminal, dial the value on the
  tablet. This is testable with hardware already in the room.
- A **Launchpad pad** can hold the step while the value comes from
  anywhere else.
- A future controller with real encoders is the eventual shape, but see
  "Hardware reality" — it does not exist here yet, and this decision does
  not depend on it.

### Session #3 evidence — 2026-07-29

**This document's central claim was confirmed by play**, independently and
before this note was written. The paragraph above predicted that "on a
keyboard the two hands compete for the same rows — in ENC mode the trig
rows *are* the encoders, so the holding hand has nowhere to be." In session
#3 the user reached exactly that conclusion unprompted, from the keyboard:
*"how would holding q for plock? how would you jog q on step q?"*

The sharpened form: to p-lock encoder *N* on step *N*, the hand must hold
trig *N* (to address the step) and jog encoder *N* (`Shift`+trig *N* with
ENC off, bare trig *N* with ENC on) — **the same physical key**. The
momentary gesture therefore has a hole in its domain, not a tuning problem.
ADR-044 D15's momentary half is consequently **retired** in TK2.2 E4, and
latched `[m]` becomes the only keyboard-local p-lock gesture. **OQ-T27 is
reopened**, which is what this ADR would close properly: the value does not
have to come from the keyboard at all.

Recommendation: unpark on this evidence. It is no longer a speculative
ergonomics improvement — it is the only route that restores the reference
workflow the keyboard structurally cannot express. See
`design/sessions/theotokos-3.md` (F7/F11) and
`design/phases/tk2.2-theotokos.md` §6 OQ-T27.

The blocker is structural, not ergonomic. A surface's parameter write is
addressed to the **engine node** (`CMD_BUMP_PARAM` → node 20's `decay`).
Theotokos never sees it; the sequencer that owns the step never sees it
either, because the executor delivers commands per-node. So today the
value lands in the live bank and the armed lock target is simply ignored —
the same press produces a lock from the keyboard and a live edit from the
tablet, which is the kind of split behaviour that makes a surface feel
untrustworthy.

## Decision

**1. The lock target is app-owned state, not Theotokos-private.**
`LockCapture { track, step, node_ids }` lives beside the app's other
main-loop state. Theotokos sets it in-process. Other surfaces set it
through the semantic plane by writing `/context/lock_target` on the state
bus (scripting already has `state_write`; Antiphon already mirrors
`/context/*`), and the app watches that path. It is published back on
`/context/lock_target` so **every** surface can display what is armed —
a lock you cannot see armed is a lock you will write by accident.

**2. Capture happens at the command drain, the one place all surfaces
converge.** The app's main loop already drains commands from scripting,
Antiphon and Theotokos before `conf.send_command()` (AGENTS.md's main-loop
sequence, steps 2–5). While a target is armed, a `CMD_SET_PARAM` or
`CMD_BUMP_PARAM` addressed to a node **owned by that track** is rewritten
into the lock pair on that track's sequencer. Everything else passes
through untouched.

**3. Ownership is the track's node chain, already computed.**
`CompositeView.chain` (`paraclete-view-assembly`) is the per-track,
engine-first list of `Rule`-bearing nodes the view layer assembles for
exactly this "what belongs to this track" question. A parameter on a node
outside the chain — a master effect, the clock — is **never** captured; it
writes live, because a per-step lock on a shared node is meaningless.

**4. Relative writes are resolved to absolute at capture time.**
`CMD_BUMP_PARAM` carries a delta; a lock stores a value. Capture reads the
current value the same way Theotokos does today — existing lock for that
step if present, else the live bank value from the state bus — applies the
delta, clamps to the descriptor's real range (ADR-044 D10), and writes the
absolute result. This runs on the main thread with the bus already in
hand; nothing new touches the audio thread.

**5. Capture is visible in the echo/status plane, not silent.** Every
captured write emits the same feedback path a keyboard-sourced lock does,
so the surface that *sent* the value and the surface that *armed* the
target both show that a lock was written rather than a live edit.

## What this is not

- **Not an engine change.** The sequencer's lock commands are unchanged;
  it cannot see other nodes' traffic and is not being taught to.
- **Not a new mutation verb.** Captured writes become the *declared*
  lock commands (33/34) — the semantic plane's vocabulary is untouched,
  which is what keeps ADR-019's "no side doors" property true.
- **Not automation recording.** One armed step, one value per parameter.
  Live-record (ADR-039 decision 7) is the time-varying path and is
  independent of this.

## Alternatives considered

- **Engine-side capture** (arm the sequencer, let it absorb param
  changes). Rejected: the executor delivers commands per-node, so the
  sequencer never observes writes to the engine nodes it owns. Making it
  do so means fanning every param command out to owning sequencers — a
  hot-path change for a main-thread problem.
- **Per-surface capture** (each surface learns the lock rules). Rejected:
  duplicates the resolve/clamp/read-current logic into every surface and
  every profile, and guarantees drift — the failure mode AGENTS.md
  learning 5 names.
- **Scripting-level capture** (a Rhai profile rewrites commands).
  Rejected: profiles see their own device's events only; nothing gives one
  profile authority over another surface's writes.
- **Do nothing; keep locks keyboard-only.** Rejected as the *default*, but
  it is the honest fallback if R1 says the app should not rewrite commands
  — in which case D15's keyboard-local half stands alone and the
  hold-elsewhere/dial-elsewhere workflow does not exist.

## Hardware reality (read before scheduling this)

Paraclete's encoder contract is **relative-only** (a named decision), and
what is on hand does not satisfy it:

- The **Digitakt II is disqualified and must not be revisited**: checked
  on hardware 2026-07-04, it transmits absolute CC (knob position mirrored
  from the internal parameter) and Elektron's implementation has no
  relative mode (`design/sessions/s0-hardware-checks.md`, Check 1; side
  finding BUG-009). It remains a *design reference* for workflow only.
- The **LaunchControl XL has pots, not encoders** — absolute positions.
  Its useful shape is fixed macro bindings (roadmap **W5** + P16's macro
  system), wired once and left, not a contextual bank that jumps.
  **SPIKE-006** scopes what it can actually provide, including prior art
  on firmware modification.
- **Theoria's touch encoders are relative and exist now.** They are the
  value source this ADR should be *tested* with, and they are enough to
  prove or kill the workflow without buying anything.

## Consequences

- The app gains a `LockCapture` module and one new watched bus path; the
  main-loop sequence gains a rewrite step between drain and send.
- Theotokos's D15 implementation becomes the *setter* of shared state
  rather than the owner of the behaviour; its local rewrite path collapses
  into the shared one, so there is one implementation of "what is a lock
  write", not two.
- Any surface that can move a parameter gains p-lock authoring for free,
  including ones not written yet — the property that makes this worth an
  ADR rather than a Theotokos patch.
- A rewrite layer in the command path is a real cost: commands no longer
  mean exactly what they say while a target is armed. Decision 5's
  visibility requirement is the mitigation, and R2 asks whether it is
  enough.
- **Scheduling:** not TK2.1. It wants D15 shipped and session #3's verdict
  on whether the keyboard-local half is even the ergonomics wanted, before
  the cross-surface generalization is worth building.

## Ratification questions

| # | Question | Recommendation |
|---|---|---|
| **R1** | Is an app-level rewrite of surface commands acceptable at all, or should a captured write stay the sending surface's own responsibility? | Accept it — it is the only place all surfaces converge, and the alternative is the same logic copied into every surface and profile |
| **R2** | Is bus-published `/context/lock_target` plus echo feedback enough to keep the rewrite from feeling like magic, or should a captured write be confirmable (e.g. the sending surface must opt in to capture)? | Publish + echo first; opt-in is a bigger contract and can follow evidence |
| **R3** | Schedule: draft-and-park until after TK2.1 session #3, or implement alongside the AN/P11 work once TK2.1 lands? | Park. It is cheap to hold and expensive to build against unproven ergonomics |

## Test seams

- **Pure:** ownership resolution (in-chain vs out-of-chain node), delta →
  absolute resolution against fixture descriptors, clamping to real ranges.
- **Integration:** a scripted parameter write with a target armed produces
  the lock pair on the right sequencer and no live bank change; the same
  write with no target armed passes through byte-for-byte.
- **Engine effect:** `tools/test-driver` scenario asserting the step's
  locked value takes effect on the next pass of that step.
- **Feel:** a paired session with the terminal holding the step and
  Theoria supplying the value — the cheapest real test available, needing
  no new hardware.
