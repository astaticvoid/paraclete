# ADR-030: Pattern Engine — Multi-Pattern, Multi-Page, Per-Track Length & Speed

**Date:** June 2026
**Status:** Accepted — P10 implementation target

---

## Decision

The `Sequencer` node grows from a single 16-step pattern into a **pattern
engine**:

1. A new `Pattern` struct owns the step data, length, and page-loop window. The
   `Sequencer` owns a `Vec<Pattern>` plus an `active_pattern` and an optional
   `cued_pattern`.
2. Patterns span up to **64 steps** (8 pages × 8 steps on the Launchpad grid).
   A **page-loop window** `(start_page, end_page)` selects which contiguous run
   of pages plays.
3. **Pattern switching is seamless**: setting the active pattern *cues* it; the
   switch lands at the end of the current pattern's page-loop window. A volatile
   **chain** queue (≤ 8 entries) advances automatically.
4. Each track has an independent **length** (already `pattern_length`) and a new
   **speed multiplier** (1/8×–2×) applied to `ticks_per_step`. Polyrhythm is the
   emergent result — there is no separate "polyrhythm mode" (consistent with
   ADR-016: structure is topology and per-track parameters, not a feature flag).
5. **All sequencer state serializes** (serializer v3), closing BUG-005:
   conditions, micro-timing, swing, CV locks, page-loop windows, and every
   pattern in the track.

This stays within the existing `Sequencer` node. Multi-track remains multiple
`Sequencer` instances (ADR-016); patterns are *within* a track, not across tracks.

---

## Context

Through P9 the `Sequencer` held one `Vec<Step>` of fixed length 16, a stub
`active_pattern: u8` field, and a `CMD_SET_PATTERN` handler that ignored its
argument and always played pattern 0. The P5 feature set (TrigCondition,
StepTiming, swing, fill) was bolted onto that single pattern.

`instrument-vision.md`'s "A Session" — the project's design tiebreaker — is
written almost entirely around capabilities this model cannot express: scene
buttons paging through a 64-step pattern, the hat track running at 2× while the
kick runs 16ths, cueing pattern C while A plays, chaining A → B → C. None of
these work today. The pattern engine is the precondition for the instrument
being playable as described.

`Sequencer::serialize()` already drops the P5 fields (BUG-005), so a saved
project silently loses conditions and timing. Expanding the model without
fixing serialization would compound the data loss; v3 is therefore in scope.

---

## Why Pattern-Within-Node (Not Pattern-As-Topology)

ADR-016 established multi-track as multiple node instances. A natural question:
should multiple *patterns* also be multiple nodes (e.g. a pattern-bank node)?

**Rejected.** A pattern is not a participant in data flow — it is alternate
*state* for one track's single output stream. Only one pattern of a track plays
at a time; switching is a state transition, not a graph re-route. Modeling
patterns as nodes would force a multiplexer node, per-pattern edges, and a
topology patch on every pattern switch — exactly the ~5 ms silence `apply_patch`
incurs (ADR-029), which is unacceptable for a beat-synchronous, seamless switch.
Pattern switching must be sample-accurate and allocation-free on the audio
thread. That rules out topology changes and rules in in-node state.

This is consistent with ADR-018 (cellular architecture): the Node API is
extended (the `Sequencer` gains pattern state and commands) rather than inventing
a non-node "pattern manager."

---

## Data Model

```rust
pub struct Pattern {
    pub steps:       Vec<Step>,    // up to 64
    pub length:      usize,        // active step count (≤ steps.len())
    pub page_loop:   (u8, u8),     // (start_page, end_page) inclusive, 0-relative
    // swing is per-pattern (vision: "Swing ... Per-pattern.")
    pub swing:       f32,          // 0.0–0.5
}

pub struct Sequencer {
    patterns:        Vec<Pattern>, // replaces the single `steps` Vec
    active_pattern:  usize,
    cued_pattern:    Option<usize>,// applied at end of page-loop window
    chain:           Vec<usize>,   // volatile; advances on cycle boundary
    chain_pos:       usize,
    speed_mult:      f32,          // 1/8 .. 2.0; scales ticks_per_step
    // ... existing playback fields (current_step, step_tick, gate, etc.)
}
```

- **Page** is derived, not stored: `page = current_step / 8`.
- **Page-loop window** advances `current_step` from `start_page*8` to
  `(end_page+1)*8 - 1`, then wraps. A pattern with `length < window` clamps to
  `length`.
- **Cued switch / chain advance** is evaluated at the wrap boundary only, so a
  switch is always musically aligned and never tears mid-pattern.

`active_pattern` (the existing stub `u8`) is widened to `usize` and made real.

### Per-track speed: step-division, not a new clock domain

Architecture-core principle 5 frames polyrhythm as a **clock-domain** concern
(nodes declare clock relationships; clock federation, ADR-009). This ADR
deliberately chooses the simpler alternative: a per-track **step-division**
multiplier that scales the Sequencer's local `ticks_per_step`, rather than
registering a sub-divided clock domain per track. Rationale — it needs no new
domain, no federation handshake, and no executor change; the track stays on the
one `InternalClock` domain and simply consumes its ticks faster or slower. The
clock-domain route remains available later if a track ever needs an *independent
tempo* (not just a division of the master), which step-division cannot express.

`effective_ticks_per_step = round(ticks_per_step / speed_mult)` is exact for
integer and 1/2/1/4/1/8 multipliers. Fractional multipliers (e.g. 1.5×) incur
per-step rounding; accumulate the remainder across steps to avoid long-pattern
drift (the same drift class BUG-001 caused). The p10 spec (Commit 3) carries the
implementation note.

**Intra-step timing composes proportionally with speed (BUG-031).** Swing and
signed micro-timing are *fractions of a step*, not absolute musical times: an
intra-step offset scales by `1 / speed_mult` so it stays the same proportion of
the (speed-scaled) step at any per-track speed. `step_sample_offset` computes
`swing_amount * samples_per_beat / speed_mult` and
`(micro_offset / 96) * samples_per_beat / speed_mult`. This keeps the swing
*feel* constant across speeds and guarantees a per-step nudge can never overshoot
into the following step (the offset shrinks with the step). The rejected
alternative — beat-anchored offsets independent of speed — was the pre-fix
behavior (audit item #27): it made swing drift from intent and could overshoot
the shortened step at high speed. Pinned by
`swing_offset_scales_with_step_speed`.

---

## Serialization (v3)

`Sequencer::serialize()` bumps to version 3 and writes: format version, speed
multiplier, active pattern index, chain, a **used-pattern count**, then that many
`Pattern` records (length, page-loop, swing, and every `Step` including
`condition`, `timing`, `param_locks`, `cv_locks`). Only patterns up to the
highest non-empty index are written — **not** the full pre-allocated bank — so an
8-slot bank with one used pattern does not bloat the file 8×. `deserialize()`
reads v1/v2 (single pattern → `patterns[0]`, defaulting speed = 1.0, page-loop =
full) and v3. This resolves **BUG-005** and supersedes the P9 note that "P5
fields are still not serialized."

Project file format (ADR-025/029) is unaffected: the node blob is opaque to the
RON layer; only the node's internal byte format changes.

---

## Consequences

**Positive**
- The vision's "A Session" pattern/page/polyrhythm gestures become expressible.
- BUG-005 (silent state loss on save) is fixed.
- No new node types, no topology changes on switch, no audio-thread allocation.
- Per-track speed gives polyrhythm for free, with no mode flag (ADR-016 intact).

**Negative / deferred**
- The Launchpad surface and TUI must learn pages and cued/chained patterns
  (OQ-11); the LED/profile work is part of P10, not free.
- Mute, temporary save/reload, and Perform Kit mode are **not** in this ADR.
  They are P11 and layer on top of this model (a snapshot is a clone of the
  active `Pattern`; Perform Kit changes which state pattern-switch overwrites).
- Signed micro-timing (OQ-6 / BUG-004) interacts with per-track speed; it is
  **fixed in P10 Commit 3**, not deferred.
- The 241-vs-240 tick step period (BUG-001) is a prerequisite for per-track speed
  (clean polyrhythm base); it is **fixed in P10 Commit 0**, ahead of the speed
  work, since it lives in `internal_clock.rs` and is a separable concern.

---

## Alternatives Considered

- **Pattern-as-topology (multiplexer node).** Rejected — see above; incompatible
  with seamless, allocation-free, sample-accurate switching.
- **Fixed 64-step always, no pages.** Rejected — the Launchpad shows 8 steps per
  row per page; pages are the hardware-native unit and the page-loop window is a
  named vision feature ("loop pages 1–2 while composing").
- **Patterns shared across tracks (song-global pattern bank).** Rejected — tracks
  are independent nodes (ADR-016); a global bank reintroduces cross-track
  coupling the cellular architecture rejects. Pattern chaining is the
  song-structure tool, kept per-track and volatile.
