# ADR-046 — Transport command vocabulary: pause, stop, and who owns the rewind

| Field | Value |
|-------|-------|
| **Status** | ✅ **Accepted (2026-07-29)** |
| **Author** | Agent (drafted from Theotokos usability session #3 evidence) |
| **Ratification** | **R1–R4 settled as recommended, 2026-07-29** — see "Ratification" below. No accepted ADR is superseded; ADR-044 D7 keeps its intent and loses its workaround (T3) |
| **Scope** | `paraclete-nodes` (`internal_clock`, `sequencer` reset path), `paraclete-theotokos` (transport actions), no new crate, no audio-thread allocation |
| **Related** | BUG-043 (no pause, no stop), BUG-039 (`InternalClock` boots `playing: true`), BUG-041 (`global_stop`, fixed), ADR-044 D7 (no transport at launch — a surface-side workaround this would retire), OQ-16/OQ-T30 (multi-surface state agreement), ADR-031 (Antiphon) |

Third-party marks appear per house naming policy: design prose only, never
identifiers or UI strings.

---

## Context

Theotokos usability session #3 (2026-07-29) found the transport unusable
within minutes: `c` (STOP) does nothing at all, and `x` pauses but always
resumes from the start of the page-loop window. Measured, then root-caused
(BUG-043):

- `input.rs:857` has no arm for a bare `PanelButton::Stop` — its own
  comment says "Bare STOP has no meaning yet" — and no `Action::Stop`
  variant exists.
- `x` = `Action::PlayToggle` emits `CMD_CLOCK_START`/`CMD_CLOCK_STOP`.
- `CMD_CLOCK_START` sets `first_tick = true`
  (`internal_clock.rs:150-154`), which becomes `global_start: true` on the
  next transport event (`:117`).
- On `global_start` the sequencer does `current_step = wstart;
  step_tick = 0` (`sequencer.rs:925-931`).

**The real defect is not the missing match arm.** It is that
`CMD_CLOCK_START` means two things at once — *begin running* **and**
*rewind to the window start* — while `CMD_CLOCK_STOP` means only *halt*.
With that vocabulary, "resume from where I paused" is **not expressible to
the engine by any surface**, so the panel could not offer pause even if it
wanted to. STOP, meanwhile, is advertised in the legend strip, the `?`
overlay, and README while doing nothing.

Two other open items sit on the same seam:

- **BUG-039**: `InternalClock` constructs with `playing: true`, so every
  surface auto-starts. ADR-044 D7 works around this *surface-side* by
  having Theotokos push a `CMD_CLOCK_STOP` at startup — which is why the
  panel boots silent, verified in session #3. The emulator, the CLAP
  subgraph and headless runs inherit the original behaviour.
- **OQ-16/OQ-T30** (raised by the user in the same session): "we should
  consider what happens with multiple interfaces ie terminals and web etc.
  There needs to be listeners to engine bi directional." Compound commands
  are precisely what makes concurrent surfaces disagree — a surface cannot
  express "resume" without also asserting a position, so two surfaces
  cannot both be right. Antiphon already *publishes* transport state
  (`TransportSummary`, `/transport/*` paths) — the gap is on the write
  side and in the vocabulary, not in the mirror.

**Migration cost is small, and this was checked rather than assumed.** The
only production callers of `CMD_CLOCK_START`/`CMD_CLOCK_STOP` are
Theotokos (`action.rs:147-162`, `lib.rs:98` startup). `launchpad.rhai`
sends no clock commands; the Theoria/web client has no transport control;
the CLAP host path drives the clock through host transport, not these
commands. Everything else touching them is a test.

## Decision

### T1 — Three commands, one meaning each

| Command | Meaning after this ADR |
|---|---|
| `CMD_CLOCK_START` (16) | **Run from the current position.** No implicit rewind. |
| `CMD_CLOCK_STOP` (17) | **Halt in place.** Position is retained. (Unchanged; already emits `global_stop` per BUG-041.) |
| `CMD_CLOCK_REWIND` (new) | **Set position to the window start.** Independent of whether the clock is running. |

Pause becomes expressible (`STOP`, then `START`), and "stop" in the
musical sense becomes a *composition* of two intents (`STOP` + `REWIND`)
rather than a hidden side effect of one.

### T2 — `global_start` becomes a rewind signal, decoupled from starting

Today `first_tick` is set by `CMD_CLOCK_START` and read by downstream
nodes as `global_start`, which sequencers treat as "reset your position".
That conflation is the bug. After this ADR, the transport flag that
carries "reset your position" is raised by `CMD_CLOCK_REWIND`, not by
starting. Sequencers keep resetting on that flag — `sequencer.rs:925-931`
is *correct behaviour for a rewind*, it was simply being told about the
wrong event.

Naming is a ratification question (R2): keep the wire name `global_start`
with new semantics, or rename to `global_rewind` and update every
construction site.

### T3 — The engine boots stopped (retires D7's workaround)

`InternalClock` constructs with `playing: false`, closing BUG-039 at the
source. ADR-044 D7's startup `CMD_CLOCK_STOP` from Theotokos becomes
redundant and should be removed in the same change, not left as a
belt-and-braces duplicate — two mechanisms for one invariant is how the
original confusion arose. Note the honest bound recorded in `lib.rs:88-93`
(the executor ticks before commands drain, so frame 1 paints
`playing = true`) disappears with this change rather than needing its
caveat.

### T4 — Transport state is published for agreement, not inferred

The engine publishes `playing` and the current position as state-bus
values so any surface can render the transport without having tracked the
commands it or anyone else sent. This is the minimum that makes OQ-16
tractable: surfaces send **intents** and render **published state**, never
their own optimistic guess. Position is already published per sequencer
(`current_step`); what is missing is a clock-domain-level `playing` +
position pair. Scope note: this ADR only *publishes*; reconciliation
policy for competing surfaces stays OQ-16.

### T5 — Theotokos mapping

| Key | Action | Emits |
|---|---|---|
| `x` PLAY | `PlayToggle` — start / **pause in place** | `CMD_CLOCK_START` or `CMD_CLOCK_STOP` |
| `c` STOP | `Action::Stop` (**new variant**) | `CMD_CLOCK_STOP` + `CMD_CLOCK_REWIND` |

This is the reference box's grammar and what the session expected. Until
this lands, the legend strip should **not** advertise `[c] STOP` (see
TK2.2).

## Ratification — 2026-07-29

All four settled as recommended, by user decision:

- **R1 → the three-command split.** `CMD_CLOCK_START` runs from the current
  position, `CMD_CLOCK_STOP` halts in place, `CMD_CLOCK_REWIND` sets
  position. The `CMD_CLOCK_RESUME` alternative is rejected: it would have
  left `START` compound, which is the property that makes multi-surface
  agreement hard.
- **R2 → rename** `TransportFlags::global_start` to `global_rewind`.
- **R3 → `CMD_CLOCK_REWIND` is valid while running** — a musical "return to
  top" that keeps playing.
- **R4 → publish clock-level `playing`/position in this ADR** (T4);
  reconciliation policy for competing surfaces remains OQ-16.

### Implementation hazard R2 creates — read before renaming

`sequencer.rs:925`'s `global_start` branch currently does **four** things at
once, and only one of them belongs to a rewind:

1. `self.playing = true`
2. position reset (`current_step = wstart`, `step_tick = 0`)
3. `reset_period()`
4. **fires the entry step** — the BUG-001 fix, which emits the note-on for
   the step being entered

A mechanical rename would therefore make `CMD_CLOCK_REWIND` start playback
*and* emit a note; and because R3 permits rewind while running, a mid-play
rewind would double-fire against the ordinary boundary path. Required
decomposition:

- **`playing`** derives from `flags.playing` (the clock's own state), not
  from the rewind flag. `global_stop` already clears it at `:908-909`, so
  the two sides must stay symmetric.
- **Position reset + `reset_period()`** happen on `global_rewind`.
- **The entry-step fire (BUG-001)** happens on the *transition into
  playing*, not on rewind — otherwise rewind-while-stopped emits an audible
  note from a stopped instrument, and rewind-while-running fires twice.

Pin all three with tests: rewind while stopped is silent and moves position;
rewind while running relocates without double-firing; a normal start still
fires its entry step exactly once (the BUG-001 regression must survive).

## Ratification questions *(historical — answered above)*

- **R1** — Adopt T1's three-command split (recommended), or the
  lower-touch alternative: keep `CMD_CLOCK_START` compound and add
  `CMD_CLOCK_RESUME` alongside it? The alternative needs no migration of
  existing callers, but leaves the compound meaning in place, which is the
  thing that makes multi-surface agreement hard (OQ-16). Given the
  migration inventory above is essentially "Theotokos plus tests", the
  recommendation is the clean split.
- **R2** — Rename `TransportFlags::global_start` → `global_rewind`, or keep
  the name with revised semantics? Rename is clearer and touches
  ~6 construction sites plus test helpers; keeping it risks a future reader
  re-conflating the two.
- **R3** — Does `CMD_CLOCK_REWIND` while *running* re-enter at the window
  start (a musical "return to top" that keeps playing), or is it only
  meaningful while stopped? Recommendation: allow it while running — it is
  a useful performance gesture and costs nothing to permit.
- **R4** — Should T4's published `playing`/position be part of this ADR or
  deferred to OQ-16's own design? Recommendation: publish here (it is
  small and makes the fix verifiable from outside), decide reconciliation
  there.

## Consequences

- BUG-043 becomes implementable; BUG-039 closes at the source; ADR-044 D7
  keeps its *intent* ("no transport at launch") while losing its
  workaround.
- Every existing "start" that relied on the implicit rewind must send
  `REWIND` + `START`. Inventory above; the affected production sites are
  `theotokos/action.rs:147-162` and `theotokos/lib.rs:98`. Tests asserting
  the old compound behaviour (`sequencer.rs:2210-2245`,
  `theotokos/lib.rs:1878`, `:2171`) must be updated deliberately — each one
  is a statement about semantics, so "make it green" is the wrong instinct
  there.
- A sequencer that resets only on rewind means a **pause/resume mid-pattern
  keeps micro-timing and page-window position**, which is what makes pause
  musically useful.
- Risk: a surface that pauses and never rewinds can leave the instrument
  parked mid-pattern with no visible cue. T4's published state is what lets
  a panel show that honestly.

## Cross-references

- `design/sessions/theotokos-3.md` — the evidence (measured transport
  sampling, root-cause chain)
- `design/bugs.md` — BUG-043, BUG-039, BUG-041
- `design/phases/tk2.2-theotokos.md` — the commit that implements this
- ADR-044 D7 (no transport at launch), ADR-031 (Antiphon), ADR-039
  (performance state)
