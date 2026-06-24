# Paraclete — P10 Interface Specification

> **Implementation blueprint.** These are the contracts P10 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
>
> **P10 deliverable:** The `Sequencer` becomes a real pattern engine —
> multiple patterns per track, multi-page (64-step) patterns with page-loop
> windows, per-track length & speed (polyrhythm), seamless cued switching and
> chaining, and full serialization of every sequencer field.
> **Last updated:** June 2026
> **Baseline:** P9 complete — 391 tests (389 distinct), 0 failures
> **Depends on:** p0–p9 interfaces and reports — all prior types assumed
> present and correct.
> **References:** ADR-030 (pattern engine), ADR-016 (multi-track = node
> instances), ADR-018 (cellular architecture), ADR-019 (universal parameter
> control), ADR-025 (project file format), ADR-026 (TUI / context publishing),
> ADR-029 (dynamic topology), ADR-001 (license)

-----

## How to use this document

**Commit 1 is gated.** It is a pure refactor: the single `Vec<Step>` becomes
`patterns: Vec<Pattern>` with exactly one pattern, and serializer v3 lands.
There must be **no behavioral change** — every existing P9 test passes
unchanged, plus the new round-trip serialization tests. No P10 feature work
begins until `cargo test --workspace` is green after Commit 1. This commit also
fixes BUG-005 (state lost on save), so it is gap-closure, not just scaffolding.

**Implement in commit order.** Each commit builds on the prior model. Commit 4
(switching/chaining) depends on Commit 2's page-loop window boundary, which is
where a cued switch lands.

**Portability rule applies throughout.** `cargo tree -p paraclete-nodes` must
show no dependency on `paraclete-runtime` or `paraclete-scripting` before merge.
The same invariant holds for `paraclete-node-api`. All P10 sequencer work lives
in `paraclete-nodes`; TUI/profile surface work touches `paraclete-tui`,
`paraclete-app`, and `profiles/` only.

**`paraclete-node-api` is LGPL3 at v0.1.0.** P10 adds no `Node` trait methods.
The pattern engine is entirely internal to the `Sequencer` node plus new
`NodeCommand` type IDs (which are data, not API surface).

**Parameter names are long-term contracts.** P10 introduces no new ADR-019
parameter names. `speed` and `length` are sequencer *commands*, not bank
parameters; pattern/page indices are positional. Swing stays the existing bank
parameter but moves to per-pattern storage (see 2.3).

**Workflow (per `feedback-p8-workflow` / `feedback-p7-subagent-workflow`):**
code review before every commit; design/doc changes (this file, ADR-030,
bugs.md, CLAUDE.md) commit separately from code (`feedback-commit-discipline`).
Stop at the Commit 4 integration milestone to play-test with hardware before
the surface work in Commit 5.

-----

## Commit Sequence

| Commit | Crate(s) | Deliverable |
|--------|----------|-------------|
| **0** | `paraclete-nodes` | Pre-flight (two independent deck-clearing bug fixes): **BUG-008** (`mem::take` pending params in `activate()`) and **BUG-001** (clock start model, 241→240 tick period — lives in `internal_clock.rs`, must precede per-track speed so polyrhythm has a clean 240-tick base). |
| **1 (GATED)** | `paraclete-nodes` | `Pattern` struct; `Sequencer` owns `Vec<Pattern>` (len 1); serializer **v3** (fixes BUG-005). No *playback*-behavior change; serialization format changes to v3 by design. |
| 2 | `paraclete-nodes` | Multi-page patterns — up to 64 steps; page-loop window `(start_page, end_page)`; per-pattern swing. |
| 3 | `paraclete-nodes` | Per-track length & speed multiplier (1/8×–2×) → polyrhythm. `CMD_SET_LENGTH`, `CMD_SET_SPEED`. **Fixes BUG-004** (signed micro-timing — genuinely in the step-emission path this commit rewrites). |
| 4 | `paraclete-nodes` | Seamless cued switching + volatile chain. `CMD_SET_PATTERN` (real), `CMD_CHAIN_PUSH`, `CMD_CHAIN_CLEAR`. |
| 5 | `paraclete-nodes`, `paraclete-tui`, `paraclete-app`, `profiles/` | State-bus surface: publish active/cued pattern, page, length, speed. TUI page display. Launchpad scene-button page select + cued-pattern blink. **Surface verification depends on P9.5** (full emulator) — see 5.3. |

-----

## New `NodeCommand` type IDs

Continuing the Sequencer command table (existing IDs 16–27). All are
allocation-free and clamp/ignore out-of-range arguments (ADR-019 discipline).

| type_id | Constant | arg0 | arg1 | Commit |
|---------|----------|------|------|--------|
| 27 | `CMD_SET_PATTERN` *(redefined)* | pattern index | — | 4 |
| 28 | `CMD_SET_LENGTH` | step count (1–64) | pattern index (≥0) or −1 = active | 3 |
| 29 | `CMD_SET_SPEED` | — | speed multiplier (0.125–2.0) | 3 |
| 30 | `CMD_SET_PAGE_LOOP` | start_page (0–7) | end_page (0–7) | 2 |
| 31 | `CMD_CHAIN_PUSH` | pattern index | — | 4 |
| 32 | `CMD_CHAIN_CLEAR` | — | — | 4 |

`CMD_SET_PATTERN` was a P5 stub that ignored its argument and always played
pattern 0. It is **redefined**, not added: if `!playing`, switch immediately;
if `playing`, cue and switch at the next page-loop window boundary (see 4.1).

-----

# Commit 0: Pre-flight bug fixes (BUG-008, BUG-001)

**Crate:** `paraclete-nodes`

Two independent, deck-clearing fixes that are *not* part of the pattern engine
but must land first. They touch different files and different concerns; keep
them as one small "pre-flight" commit (or two if you prefer — they are
independent of each other and of everything below).

## 0.1 BUG-008 — `set_initial_params` re-applied on re-activate

`set_initial_params` is re-applied on every `activate()`, overwriting
deserialized state when the executor rebuilds (P9 C4 dynamic topology). Fix per
the bug tracker: clear the pending map after first application.

```rust
// At the end of activate() in each of:
//   analog_engine.rs, fm_engine.rs, sampler.rs, reverb.rs, distortion.rs, filter.rs
let pending = std::mem::take(&mut self.pending_initial_params);
for (name, value) in pending { /* apply to bank as before */ }
```

**Tests:** `reactivate_preserves_deserialized_params` — deserialize non-default
values, call `activate()` a second time, assert values unchanged. One per
affected node (or one parametric test). Mark BUG-008 resolved in `bugs.md` with
the commit reference.

## 0.2 BUG-001 — step period 241 → 240 ticks

`InternalClock` emits `global_start` on its **first tick**, consuming one tick
before the Sequencer starts. The realized step period is therefore 241 ticks,
not 240 — a systematic ~0.4% BPM error since P5. This is a **correctness
prerequisite for per-track speed (Commit 3)**: two tracks at different
multipliers must align on a clean 240-tick base, and a 241-tick base produces
audible polyrhythm drift that compounds over a pattern. Fixed here, before the
speed work, rather than bundled into it.

**Fix (per BUG-001 direction):** emit `global_start` without consuming a tick (or
adjust the Sequencer's first-step accounting so the realized period is 240).
Touches `crates/paraclete-nodes/src/internal_clock.rs` (`global_start: is_first`)
and the Sequencer start handling.

**Required existing-test change (flagged):** `sequencer_loop_count_increments_on_pattern_wrap`
(`sequencer.rs`) currently encodes the bug — its `wrap_tick = 16 * (tps + 1)`
assumes the 241-tick period. This test **must be updated** to `16 * tps` as part
of this commit; it is not a new test but a corrected one. Any other test that
hard-codes `tps + 1` for a wrap boundary must be audited the same way.

**New test:** `step_period_is_240_ticks` — measure the realized tick count
between successive step fires; assert exactly 240 at 1×.

Mark BUG-001 resolved in `bugs.md` with the commit reference.

Both fixes are independent of the pattern engine and should merge first.

-----

# Commit 1: Pattern struct + multi-pattern storage (GATED)

**Crate:** `paraclete-nodes`

**Problem:** `Sequencer` holds one fixed `Vec<Step>` of length 16 and a stub
`active_pattern: u8`. `serialize()` drops the P5 fields (BUG-005). Neither
multiple patterns nor durable state is possible.

**Fix:** Introduce `Pattern`; the `Sequencer` owns a pre-allocated bank of
`Pattern`s (only index 0 populated with the P9 preset after this commit). All
playback reads `&self.patterns[self.active_pattern]`. Serializer bumps to v3.
**No playback-behavior change** — the emitted event/audio stream is identical to
P9. The **serialization format does change** to v3 by design (this is the BUG-005
fix); v1/v2 blobs still load. "No behavior change" therefore scopes to playback,
not to the serialized byte output, which is deliberately different.

## 1.1 `Pattern`

```rust
#[derive(Clone, Debug)]
pub struct Pattern {
    pub steps:     Vec<Step>,   // pre-sized to 64; length 16 played after this commit
    pub length:    usize,       // active step count, <= steps.len()
    pub page_loop: (u8, u8),    // (start_page, end_page) inclusive, 0-relative; (0,1) for 16 steps
    pub swing:     f32,         // 0.0..=0.5; INERT in Commit 1 (bank.swing is still authoritative);
                                //   becomes authoritative in Commit 2's migration (2.3)
}

impl Pattern {
    pub fn empty(steps: usize) -> Self { /* steps Vec of Step::empty(), length = steps, page_loop spanning all pages, swing 0.0 */ }
}
```

## 1.2 `Sequencer` field changes

- Remove `steps: Vec<Step>`; add `patterns: Vec<Pattern>`.
- `active_pattern: u8` → `active_pattern: usize` (real, defaults 0).
- Add `cued_pattern: Option<usize>` (None), `chain: Vec<usize>` (empty),
  `chain_pos: usize` (0), `speed_mult: f32` (1.0). Fields are present but inert
  until Commits 2–4 wire them.
- All step accessors (`set_step`, `CMD_TOGGLE_STEP`, etc.) operate on
  `self.patterns[self.active_pattern]`. With one pattern this is identical to P9.

## 1.3 Serializer v3 (resolves BUG-005)

`serialize()` writes: `version = 3`, `speed_mult`, `active_pattern`,
`chain` (len-prefixed), then a **used-pattern count** followed by that many
`Pattern` records — `length`, `page_loop`, `swing`, and each `Step` in full
(`active`, `note`, `velocity`, `length`, `param_locks`, `condition`, `timing`,
`cv_locks`).

**Do not serialize the full pre-allocated bank** (4.3). Write only patterns up to
the highest non-empty index (default 1). Writing all `PATTERN_BANK_SIZE` empty
patterns would bloat every project file ~8× for no benefit. On load, the bank is
re-allocated to `PATTERN_BANK_SIZE` and the saved patterns fill the low indices;
the rest are `Pattern::empty(64)`.

`deserialize()`:
- **v1/v2:** read the legacy single-pattern body into `patterns[0]`;
  `speed_mult = 1.0`, `page_loop` spans the loaded length, `cued`/`chain` empty;
  remaining bank slots `Pattern::empty(64)`.
- **v3:** read as written above (used-pattern count, then that many patterns).

Project RON format (ADR-025/029) is unaffected — the node blob is opaque to it.

## 1.4 Gate criteria

- `cargo test --workspace` — **zero failures** (this is the gate).
- New tests:
  - `serialize_roundtrip_preserves_conditions` — a step with a non-default
    `TrigCondition` survives serialize → deserialize.
  - `serialize_roundtrip_preserves_timing` — `StepTiming` micro-offset survives.
  - `serialize_roundtrip_preserves_swing` — swing value survives.
  - `serialize_roundtrip_preserves_cv_locks` — `cv_locks` survive (regression
    guard; v2 already did these).
  - `deserialize_v2_into_single_pattern` — a v2 blob loads with one pattern,
    speed 1.0, full page-loop.
  - `single_pattern_playback_unchanged` — the emitted step-fire / event sequence
    for the default preset is identical to P9 (step-fire sequence assertion).
    Note: this asserts *playback*, not serialized bytes — the v3 blob is
    deliberately not byte-identical to v2 (that is the BUG-005 fix).
- `cargo tree -p paraclete-nodes` shows no `paraclete-runtime` dependency.

-----

# Commit 2: Multi-page patterns

**Crate:** `paraclete-nodes`

**Goal:** Patterns up to 64 steps; a page-loop window selects which contiguous
run of 8-step pages plays. Page is derived: `page = current_step / 8`.

## 2.1 Page-loop playback

- A pattern of `length` steps spans `ceil(length / 8)` pages (max 8).
- `page_loop = (start, end)` (inclusive). Playback advances `current_step`
  across `[start*8, min((end+1)*8, length))` and wraps to `start*8`.
- The wrap boundary is the **cycle boundary** — `loop_count` increments here,
  and (Commit 4) cued switches / chain advances are evaluated here only.
- `CMD_SET_PAGE_LOOP` (30): clamp `start ≤ end`, both within the pattern's page
  count; ignore otherwise. Default `(0, page_count-1)` for a fresh pattern.

## 2.2 Step storage (no audio-thread growth)

- `set_step` / step commands accept indices 0–63. The `steps` Vec is **pre-sized
  to 64 at pattern creation** (main thread, at construction); `length` gates how
  many are played. Changing `length` (Commit 3) never reallocates — it only moves
  the gate. No `steps` Vec ever grows on the audio thread.

## 2.3 Swing → per-pattern (resolve the dual source of truth)

- Move the authoritative swing value from the `ParameterBank` into
  `Pattern::swing` (the vision specifies swing is per-pattern). The bank `swing`
  parameter remains a declared ADR-019 name **purely for encoder routing** — a
  `CMD_SET_PARAM` on `swing` writes through to the *active* pattern's
  `Pattern::swing`, and emit-time reads come from the active pattern, not the
  bank slot. The bank slot is a write-conduit and a publish-mirror, never the
  source of truth, to avoid two stores disagreeing.
- **On pattern switch (Commit 4):** the bank `swing` slot and `/state/swing` must
  be refreshed from the newly-active pattern's `Pattern::swing`, so the encoder
  and TUI reflect the pattern you switched to. State this dependency in Commit 4.
- Keep `/state/swing` publishing (now sourced from the active pattern).

## 2.4 Tests

- `page_loop_window_wraps_within_window` — a 32-step pattern with `page_loop=(0,1)`
  plays steps 0–15 and wraps, never reaching step 16.
- `page_loop_opens_to_full` — setting `(0,3)` on a 32-step pattern plays 0–31.
- `set_page_loop_clamps_start_le_end` — `(2,1)` is rejected, window unchanged.
- `page_derivation` — `current_step` 0–7 → page 0, 8–15 → page 1.
- `swing_is_per_pattern` — two patterns hold independent swing values.

-----

# Commit 3: Per-track length & speed (polyrhythm)

**Crate:** `paraclete-nodes`

**Goal:** Independent per-track length and a speed multiplier. Polyrhythm is the
emergent result — no mode flag (ADR-016).

## 3.1 Length

- `CMD_SET_LENGTH` (28): arg0 = step count 1–64; arg1 = pattern index (≥0) or −1
  for the active pattern. Sets `Pattern::length`; re-derives page count and
  clamps `page_loop` into range. No reallocation (steps pre-sized to 64).

## 3.2 Speed multiplier

- `CMD_SET_SPEED` (29): arg1 = multiplier in `[0.125, 2.0]` (1/8×–2×), clamped.
- Applied to the effective step period: `effective_ticks_per_step =
  round(ticks_per_step / speed_mult)`. At 2× the track advances twice as fast.
- Speed is **per-track** (per Sequencer node), not per-pattern — it follows the
  vision's "set that track's speed to 2×."

> **BUG-001 (240-tick period) is fixed in Commit 0**, not here — it lives in
> `internal_clock.rs` and is a prerequisite for the speed multiplier below, so it
> lands before this commit. This section assumes a clean 240-tick base.

**Rounding note for fractional multipliers:** `effective_ticks_per_step =
round(ticks_per_step / speed_mult)` introduces per-step rounding error at
non-integer multipliers (e.g. 1.5×). Over a long pattern this accumulates the
same class of drift BUG-001 caused. Either accumulate the fractional remainder
across steps (preferred) or document that fractional multipliers are
nearest-tick approximations. Integer and 1/2/1/4/1/8 multipliers are exact.

## 3.3 BUG-004 — signed micro-timing (committed in this commit)

Negative `micro_offset` (pushing a step earlier) currently behaves identically to
zero — the signed half of the P5 micro-timing feature is silently non-functional.
P10 is already in the step-emission path for per-track speed, so fix it here
rather than carrying it forward.

**Fix (per BUG-004 direction):** at step N, if step N's `micro_offset < 0`, emit
the step event during step N-1's emission window (wrap around for step 0).
Touches `crates/paraclete-nodes/src/sequencer.rs` step emission. This partially
resolves OQ-6 (signed micro-timing representation); note the resolution in the
roadmap. Mark BUG-004 resolved in `bugs.md` with this commit's reference.

## 3.4 Tests

- `set_length_changes_cycle_length` — length 8 wraps after 8 steps.
- `set_length_targets_specific_pattern` — arg1 = 1 sets pattern 1's length,
  leaving the active pattern untouched.
- `speed_2x_doubles_rate` — at 2×, the track fires twice per base-clock cycle.
- `speed_half_halves_rate` — at 0.5×, the track fires every other base cycle.
- `speed_clamped_to_range` — 4.0 clamps to 2.0; 0.0 clamps to 0.125.
- `two_tracks_polyrhythm` — integration: track A length 16 @ 1×, track B length
  12 @ 1× from one clock; assert relative fire positions realign only at LCM.
  (Depends on the Commit-0 240-tick fix — would fail on a 241-tick base.)
- `negative_micro_offset_emits_early` — **BUG-004 regression** — a step with
  `micro_offset < 0` fires before its grid position (earlier sample_offset than
  offset = 0); offset = 0 and offset = +N remain distinct.
- `negative_micro_offset_step_zero_wraps` — step 0 with negative offset emits in
  the previous cycle's final window (boundary wrap).

-----

# Commit 4: Seamless switching + chaining

**Crate:** `paraclete-nodes`

**Goal:** Cue a pattern while another plays; the switch lands at the cycle
boundary. A volatile chain advances automatically.

## 4.1 `CMD_SET_PATTERN` (27, redefined)

- arg0 = target pattern index. Command handling runs inside `process()` on the
  **audio thread**, where allocation is forbidden — so patterns are **never grown
  on demand**. Instead, **pre-allocate a fixed bank** of `PATTERN_BANK_SIZE`
  patterns (see 4.3) at construction. An index `≥ PATTERN_BANK_SIZE` is silently
  ignored (ADR-019 unknown-arg discipline). All valid indices already exist, so
  no audio-thread allocation ever occurs.
- If `!playing`: switch `active_pattern` immediately (editing while stopped).
- If `playing`: set `cued_pattern = Some(index)`; the switch applies at the next
  page-loop wrap boundary, then `cued_pattern = None`.
- **On every switch (immediate or boundary):** refresh the bank `swing` slot and
  `/state/swing` from the newly-active pattern's `Pattern::swing` (per 2.3), so
  the encoder mapping and TUI reflect the pattern now playing.

## 4.2 Chain

- `CMD_CHAIN_PUSH` (31): append a pattern index to `chain` (cap 8; ignore beyond).
- `CMD_CHAIN_CLEAR` (32): empty `chain`, reset `chain_pos`.
- When `chain` is non-empty and no explicit `cued_pattern` is set, the cycle
  boundary advances `chain_pos` (wrapping) and switches to `chain[chain_pos]`.
- An explicit `CMD_SET_PATTERN` cue takes precedence over chain advance for that
  one boundary.

## 4.3 Construction

- `Sequencer::new()` / `with_name()` allocate a fixed pattern bank
  (`PATTERN_BANK_SIZE`, suggest 8) of `Pattern::empty(64)`, with `active = 0`,
  `length = 16`, `page_loop = (0,1)` on pattern 0 to preserve the P9 default
  preset. Document `PATTERN_BANK_SIZE` as the per-track pattern count.
- **Allocation budget:** 8 patterns × 64 `Step`s = 512 pre-allocated `Step`s per
  Sequencer (each with empty `param_locks` / `cv_locks` Vecs); ×8 tracks ≈ 4096
  Steps in the app graph. All allocated **once, on the main thread, at
  construction** — never on the audio thread. This is the cost that buys
  audio-thread-allocation-free pattern switching. Acceptable; state it so it is a
  deliberate choice, not an accident.

## 4.4 Tests

- `set_pattern_while_stopped_switches_immediately`.
- `set_pattern_while_playing_cues_until_boundary` — assert `active` unchanged
  until the wrap, then changed.
- `cued_pattern_clears_after_switch`.
- `chain_advances_on_boundary` — chain `[0,1,2]` visits patterns in order across
  three cycles, then wraps.
- `explicit_cue_overrides_chain_for_one_boundary`.
- `chain_push_caps_at_eight`.

-----

# Commit 5: State-bus surface (TUI + Launchpad)

**Crates:** `paraclete-nodes`, `paraclete-tui`, `paraclete-app`, `profiles/`

**Goal:** The operator sees and drives the pattern engine. No new state survives
a power cycle (state bus is not persisted, ADR-025).

## 5.1 New state-bus paths (published by `Sequencer::published_state()`)

| Path | Variant | Meaning |
|------|---------|---------|
| `/node/{id}/state/active_pattern` | `Int` | current pattern index |
| `/node/{id}/state/cued_pattern` | `Int` | cued index, or −1 if none |
| `/node/{id}/state/current_page` | `Int` | `current_step / 8` |
| `/node/{id}/state/page_count` | `Int` | `ceil(length / 8)` |
| `/node/{id}/state/page_loop_start` | `Int` | window start page |
| `/node/{id}/state/page_loop_end` | `Int` | window end page |
| `/node/{id}/state/pattern_length` | `Int` | active pattern length |
| `/node/{id}/state/speed_mult` | `Float` | track speed multiplier |
| `/node/{id}/state/chain_len` | `Int` | chain length (0 = no chain) |

- `/state/steps` — **decided convention (do not defer):** publish the **full
  active pattern** as a `length`-character bitfield (`'1'`/`'0'`), exactly as
  today but up to 64 chars instead of always 16. Consumers (TUI, `launchpad.rhai`)
  slice the 8 chars for the displayed page using `current_page`. This is the
  least-breaking choice: a 16-step pattern still yields the identical 16-char
  string, so the existing test `sequencer_published_state_includes_steps_bitfield`
  (asserts `starts_with("10100")`) keeps passing unchanged. Do **not** redefine
  `/state/steps` to mean "current page only" — that would break every existing
  consumer.
- **Fix the stale docstring:** `steps_bitfield()` in `sequencer.rs` is documented
  as "16-character … padded to pattern_length," which is already inaccurate (it is
  `pattern_length`-length, not padded to 16). Correct the comment to describe the
  variable-length (1–64) active-pattern bitfield as part of this commit.
- These use `published_state()` push (OQ-9 pattern) — no per-cycle allocation
  beyond the accepted `format!` path cost (BUG-007 class). Cache the path strings
  at `set_node_id()` time if convenient.

## 5.2 TUI (`paraclete-tui`)

- Add a pattern/page indicator to the step row: `P{active}` (blink when cued
  differs), `pg {current_page+1}/{page_count}`, and the speed multiplier when ≠ 1.
- Read the new paths each frame (TUI already polls the StateBus per ADR-026).

## 5.3 Launchpad profile (`profiles/launchpad.rhai`)

> **Depends on P9.5 (full Launchpad emulator).** The features below are bound to
> scene buttons, the control row, and tracks 3–7 — none reachable from today's
> emulator (pads 0–23 only, no scene/control keys). P9.5's full emulator is the
> prerequisite for verifying any of this without physical hardware. If P9.5 has
> not landed, this commit's surface behavior can only be tested on a real
> Launchpad; the state-bus paths (5.1) and TUI (5.2) remain emulator-independent.

- Scene buttons select the active page (write `CMD_SET_PAGE_LOOP` or a page-view
  offset; per OQ-11 decide whether scene = view-only page scroll or page-loop
  edit — recommend: tap = scroll view, hold-page + step = set page-loop window,
  per the vision's grid-edit description).
- Cued pattern: blink the corresponding pattern-select pad until the switch lands.
- Current-step indicator already sweeps; ensure it tracks the displayed page.
- Per `rhai-scripting` memory: inline-only, no `chars()`, watch char literals and
  for-loop capture.

## 5.4 Tests

- `published_state_includes_pattern_paths` — assert all new paths present with
  correct variants after a process cycle.
- `steps_bitfield_reflects_current_page` — on page 1, the bitfield matches steps
  8–15 (per the chosen convention).
- Profile/TUI behavior is verified by the **play-test** (see milestone): on real
  hardware, or via the **P9.5 full emulator** (scene buttons + all 8 tracks
  reachable) once it lands. With scripted input injection (P9.5 #4), the page-
  select and cued-switch flows can additionally be covered by an automated
  integration test rather than only by hand.

-----

## Definition of Ready (this spec is "ready to implement" when)

- [x] `design/roadmap.md` reflects P9 shipped and P10 current.
- [x] `README.md` reflects P9 and points to this spec.
- [x] ADR-030 (pattern engine data model) written and accepted.
- [x] This interface spec exists with per-commit contracts, command IDs, state
      paths, and test lists.
- [x] BUG-005, BUG-008, BUG-001, and BUG-004 are referenced with fix direction
      (done in tracker; resolution lands in Commits 1, 0, 0, and 3 respectively).
- [x] OQ-11 (Launchpad page/pattern surface) is named in the roadmap; the
      grid-edit convention is decided in Commit 5.
- [x] The four infrastructure bugs not in P10 (BUG-002, 003, 006, 007) are owned
      by a scheduled P10.5 Cleanup row in the roadmap — nothing is unscheduled.

## Definition of Done (P10 ships when)

- All commits merged in order; code review before each (`feedback-p8-workflow`).
- `cargo test --workspace` green; target ≈ 415+ tests (391 baseline + ~24 new).
- `cargo clippy` clean; portability check passes for `paraclete-nodes` and
  `paraclete-node-api`.
- BUG-005, BUG-008, BUG-001, and BUG-004 marked resolved in `bugs.md` with
  commit references.
- A play-test confirms (on hardware **or** the P9.5 full emulator): paging a
  64-step pattern via scene buttons, the hat track at 2× against the kick,
  cueing + chaining patterns live — all mouse-free, per `instrument-vision.md`'s
  "A Session." Commits 0–4 are verifiable by unit tests + `NodeCommand`
  injection alone; only Commit 5's surface needs the emulator/hardware.
- `design/phases/p10-report.md` written (append-only); `architecture-evolving.md`
  appended; CLAUDE.md "Current Phase" and Sequencer command table updated.

-----

## Out of Scope (deferred to P11+)

- Mute system (pattern + global + prepared) — P11.
- Temporary save / reload (volatile snapshot) — P11.
- Perform Kit mode (parameter state persists across pattern switch) — P11.
- Live record from the Keystep into the active pattern — P11.
- Retrig and Euclidean mode — P12.
- Inner GraphNode pattern support — the `InnerGraphNode` sequencer stays single
  pattern at P10.
- **Infrastructure bugs BUG-002 (agreement baseline), BUG-003 (hal→runtime layer
  violation), BUG-006 (agg_state_buf realloc), BUG-007 (publish_bank_state alloc)
  — P10.5 Cleanup**, not P10. They are unrelated to the pattern engine and
  bundling them here would make the phase incoherent. BUG-003 is additionally
  blocked on moving `StateBusHandle` to L2 (LGPL3).

Note: signed micro-timing (OQ-6 / BUG-004) and the 240-tick period (BUG-001) are
**no longer deferred** — BUG-001 is fixed in Commit 0 (§0.2, ahead of the speed
work) and BUG-004 in Commit 3 (§3.3).
