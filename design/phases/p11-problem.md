# P11 — Live Performance: Problem Statement

> **Stage 1 of a staged design.** States the problem only; the
> architectural shape is ADR-039 (proposed alongside this document,
> 2026-07-23). The implementation blueprint (`p11-interfaces.md`) is
> written when P11 is next to start, per the house front-load rule.
>
> **Authored:** 2026-07-23. **Status:** drafted at user request; awaiting
> problem-statement acceptance with ADR-039 ratification.

---

## The gap

The instrument vision (§Performance) promises four capabilities that the
engine does not have, and the TK track has now deferred against three of
them repeatedly:

1. **Mute system.** A single `mute` bank param per sequencer exists (TK1,
   trig-gate, click-free). But Elektron-class muting is *tiered*: global
   mutes that persist across pattern switches, pattern mutes that belong
   to the pattern, and **prepared mutes** — staged silently, applied
   together at a musical boundary. None of the tiers is modeled; the TK2
   ALT-layer work is blocked on "the mute system is P11 scope."
2. **Temporary save / reload.** The quick-save safety net ("save before a
   risky tweak; reload if it goes wrong") has no engine mechanism. It
   cannot ride the project serializer: `save_project`/`load_project` run
   only at startup, and bugs.md dormant #2 records that ADR-025's
   "engine need not be stopped" claim is unsafe once invoked live.
   Deferred out of TK2 twice.
3. **Perform Kit mode.** The vision's continuously-evolving-sound
   promise — tweaks survive pattern switches, revert or commit on
   demand — presupposes a **kit**: a named snapshot of the instrument's
   sound (param state) that a pattern can be bound to. **No kit concept
   exists anywhere in the engine.** Today a pattern switch changes steps
   only and never touches params — the instrument is permanently in an
   accidental "perform mode" with nothing to revert *to*. Theotokos's
   KIT button (ADR-038) is reserved with "kit model TBD."
4. **Live record.** Playing notes into the running pattern (Keystep, or
   Theotokos live trigs once TK2 C1 lands) is not possible: the
   sequencer's `events_in` port consumes only `Transport` events —
   incoming note-ons are silently ignored (`sequencer.rs` process loop).
   REC+PLAY live recording, the other half of the Elektron REC grammar,
   has no engine path.

## The problem

**Paraclete has no model for performance state** — the layer of state
that lives *between* the audio graph (bank params, patterns) and the
project file (cold persistence): sound snapshots that patterns bind to,
volatile safety-net copies, mute state with musical scope, and a
recording path from live input into the running pattern. Every surface
(Theotokos, Theoria, Launchpad) will need the same four capabilities;
none can build them surface-side because the state spans nodes and the
gestures need sample-accurate engine timing.

## Requirements (from the vision + accumulated TK/W constraints)

1. **Kits are first-class and pattern-bound.** Switching patterns applies
   the bound kit's sound unless Perform mode is on. Reload and commit are
   single gestures.
2. **Temp save/reload is volatile and cheap.** RAM-only, never touches
   the filesystem or the project serializer, safe while playing.
3. **Mute tiers with deterministic timing.** Global / pattern / prepared;
   prepared mutes apply exactly at the pattern boundary, engine-side.
4. **Live record is sample-accurate.** Micro-timing captured from event
   sample offsets, not from a ~1 ms UI tick.
5. **Semantic plane only.** Surfaces mutate through ADR-019 commands and
   the declared sequencer vocabulary; where an operation spans nodes, the
   app orchestrates (ADR-018 environment exemption), never a side door.
6. **All surfaces inherit the features.** The mechanism must be equally
   reachable from Theotokos (command drain), Antiphon (protocol verbs,
   W-track scope), and profiles/hardware.
7. **No audio-thread allocation or blocking.** Snapshot clones and kit
   replay must respect the process() rules (pre-allocation, command-time
   work on the main thread).

## Non-goals

- **Not song mode / arrangement** — chains (P10) remain the sequencing
  macro-structure; SONG-style arrangement is unscoped.
- **Not sample management** — kits snapshot *params*; sample slot
  assignment joins the kit model only when a sampling workflow exists.
- **Not undo/redo** — temp save is a single explicit safety net, not a
  history stack.
- **Not generative** — retrig/Euclidean/randomness are P12.

## Success criteria

- The vision's session narrative §"A Session" (temp save → risky tweaks →
  reload; Perform mode on → pattern switch keeps the evolving sound →
  commit) is performable end-to-end from Theotokos without a mouse.
- Prepared mutes land on the downbeat every time, at any BPM.
- Playing the Keystep (or REC-off trig keys) into a playing pattern
  records steps with audible micro-timing fidelity.
- All four capabilities reach Theotokos with **zero** Theotokos-specific
  engine paths — the same commands/app-ops serve every surface.
