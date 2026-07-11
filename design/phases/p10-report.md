# Paraclete — P10 Implementation Report

> **Append-only.** One section per commit, written as each commit ships.
> Spec: `design/phases/p10-interfaces.md` (including the two July 2026
> amendments: Commit-0-as-shipped and serializer-v3 forward-extensibility).

---

## Commit 0 — Pre-flight fixes (BUG-001, BUG-008) — shipped `b0cf2c8`

Shipped ahead of W0 with a fix **different from the spec's §0.2 direction**,
based on direct measurement. The full account lives in the
"Commit 0 as actually shipped" amendment appended to `p10-interfaces.md` and
in the BUG-001 tracker entry; it is not repeated here. Key outcome for later
commits: the 240-tick step base holds exactly (+0.0000% measured), and the
bar-sync snap is drift-correction-only — C3 must scale the snap's in-sync
check with the speed multiplier.

---

## Commit 1 — Pattern struct + multi-pattern storage + serializer v3 (GATED)

**Scope shipped:** `crates/paraclete-nodes/src/sequencer.rs`,
`crates/paraclete-nodes/src/lib.rs` (re-export only).

### What shipped

- **`Pattern` struct** (§1.1): `steps` / `length` / `page_loop` / `swing`,
  plus `Pattern::empty(steps)` and a public `PAGE_SIZE = 8`. Exported from
  the crate root alongside `Sequencer`/`Step`.
- **`Sequencer` field changes** (§1.2): `steps: Vec<Step>` and
  `pattern_length: usize` removed; `patterns: Vec<Pattern>` (length 1);
  `active_pattern: u8` → `usize`; inert `cued_pattern` / `chain` /
  `chain_pos` / `speed_mult` fields staged for C3/C4. All step accessors and
  playback go through the active pattern. `Sequencer::STEP_CAPACITY = 64`
  documents the per-pattern runtime step allocation (§2.2 pre-work: the Vec
  is pre-sized at construction on the main thread and never grows in
  `process()`).
- **Serializer v3** (§1.3, fixes BUG-005): header
  (`version`, `ticks_per_step`, `speed_mult`, `active_pattern`,
  len-prefixed `chain`, used-pattern count) + per-pattern records
  (`length`, `page_loop`, `swing`, step count, steps). Every `Step` field is
  now persisted, including `TrigCondition`, `StepTiming.micro_offset`, and
  (per-pattern) swing. v1/v2 blobs still load via a separate legacy reader
  into `patterns[0]` with defaults for the never-saved fields. Truncated or
  malformed input aborts the load without panicking (bounds-checked
  `ByteReader`; the pre-C1 code had the same abort-partway semantics).

### Forward-extensibility amendment — decisions (required record)

1. **Multi-note steps:** chose the amendment's minimum bar — **per-step
   extensible framing**, not widening `note: u8` now. Each v3 step record
   carries a u16 byte-length prefix; readers parse the fields they know and
   skip the remainder, so v4 can append a note list (or any per-step field)
   without a format rewrite. A dedicated test
   (`deserialize_v3_skips_unknown_trailing_step_fields`) locks the skip
   behavior in.
2. **No surface-shaped caps in the format:** all counts are plain integers
   (u16 step/pattern counts, u16 lengths). `STEP_CAPACITY = 64` is a runtime
   constant only; the file format does not encode it, so raising the engine
   cap is a constant change, not a version bump.

### Deviations / boring-option choices (spec silent or in tension)

- **`ticks_per_step` kept in the v3 header.** §1.3's field list omits it, but
  v2 persisted it; dropping it would have been a save/load regression inside
  the commit that exists to end save/load regressions. Recorded here per
  guardrail 1.
- **`CMD_SET_PATTERN` stub semantics preserved exactly.** The P5 stub stored
  any index while playing pattern 0; C1 keeps that (an existing test asserts
  the stored value), and playback clamps through `active_index()` into the
  single-pattern bank. C4 replaces this wholesale with the pre-allocated
  bank + silently-ignore-out-of-range semantics (§4.1).
- **Swing (§1.1 says `Pattern::swing` is "INERT" in C1; BUG-005 says swing
  must survive save).** Reconciled with a one-way mirror: the bank slot stays
  authoritative for emission (unchanged behavior); `process()` mirrors the
  bank value into the active pattern's `swing` so `serialize()` captures it;
  `deserialize()` and `activate()` re-apply the pattern value to the bank
  (the `activate()` re-apply covers the project-load path, where
  `deserialize()` runs before `activate()`, and keeps executor-rebuild
  re-activation from resetting a loaded swing — the BUG-008 failure class).
  Net effect: swing survives save/load; emission source unchanged; C2's
  migration (§2.3) flips authority to the pattern as planned.
- **"Used pattern" definition for the trailing-slot cut (§1.3):** a pattern
  is serialized if any step is active, any step carries param/cv locks, or
  swing ≠ 0; always at least one pattern is written. Patterns below the
  highest used index are written even if individually empty (index = file
  position; no index table needed).
- **Whole allocated step bank serialized per used pattern** (64 records),
  not just `length`: from C2 on, trimming `length` must not destroy step
  data beyond the gate, and the used-pattern cut already prevents the ~8×
  bank bloat §1.3 warns about.
- **Step commands now accept indices 0–63** (previously silently ignored
  ≥ 16) because the backing Vec is pre-sized to 64. Playback is still gated
  to `length` (16), so the emitted stream is unchanged; this is the §2.2
  behavior arriving one commit early as a side effect of the storage model.

### Pre-commit review findings (8-angle finder pass + fixes)

Eight review angles (3 correctness, reuse/simplification/efficiency,
altitude, conventions) ran against the working diff before commit; the
correctness-relevant findings were fixed in place and each fix carries a
regression test.

**Fixed before commit:**

1. *(3 angles independently)* `deserialize_v3` accepted blobs whose declared
   pattern `length` exceeded the step count, or with zero steps — a corrupt
   or foreign blob would panic playback on the audio thread (`% plen`,
   out-of-bounds slice). Now: zero-step patterns abort the load; `length`
   clamps into the step Vec; steps are re-padded to `STEP_CAPACITY` so
   post-load step editing matches a fresh sequencer. The legacy reader got
   the same length clamp (the pre-C1 code had the zero-length divide-by-zero
   too).
2. `handle_commands` resolved the active pattern once per batch, so an
   in-batch `CMD_SET_PATTERN` would not have directed later step edits at the
   new pattern (a C4 trap). Now resolved per command.
3. The serializer persisted the stub's raw `active_pattern` (u16-truncating,
   and unhonorable indices would select a different — empty — pattern once
   C4's bank exists: a saved project could reload silent). Now the
   *effective* index is persisted, and the written pattern range always
   includes the active pattern.
4. `pattern_is_used` ignored inactive steps carrying conditions or
   micro-timing — such patterns would be dropped from multi-pattern saves
   (C4 data-loss vector). Predicate extended; `TrigCondition` gained
   `PartialEq`.
5. Pattern records got the same length-prefix framing as step records (u32),
   so future per-pattern fields (name, per-pattern speed) don't force a
   format break; v3 readers also tolerate track-level fields appended after
   the last pattern record. Extends amendment req. 1 one level up.
6. The `activate()` comment asserted an ordering CLAUDE.md's Node contract
   forbids; reworded to name the two real paths (`--load` deserializes
   un-activated nodes before `build_executor` per ADR-025; executor rebuilds
   follow deserialize-after-activate). Code was correct for both; the
   comment was not.
7. The `(n,m)` ⇄ `RepeatCondition` zero-sentinel mapping existed in three
   copies (command decode, record write, record read) — deduplicated into
   `repeat_from_nm` / `nm_from_repeat`. `emit_note_on`'s borrow workaround
   simplified to a single binding.

**Deliberate, no change (recorded so they aren't re-litigated):**

- `ByteReader` duplicates the hand-rolled cursor reads in
  `sampler.rs::deserialize` — hoisting a shared reader and migrating Sampler
  is real cleanup but out of this gated commit's scope; queue for the next
  housekeeping pass.
- 64 step records serialized per used pattern (≈4× the v2 blob for a default
  track): the price of preserving beyond-`length` step data once C2/C3 make
  `length` mutable; used-pattern trimming already bounds the bank cost.
- Step commands accept indices 0–63 (P9 ignored ≥ 16): §2.2 storage-model
  consequence, playback unchanged.
- Swing lives in three places during C1 (bank slot authoritative,
  `swing_amount` cache, per-pattern mirror): the C2 migration (§2.3)
  collapses this; the mirror direction is what makes `serialize()` correct
  even before `activate()`.
- TUI still renders a fixed 16-step row (`app.rs` `.take(16)`,
  `layout.rs` "/ 16"): pre-existing; P10 C5 owns the TUI pattern/page
  surface.

### Gate results (§1.4)

- `cargo test --workspace`: **441 passed, 0 failures** (427 baseline + 7
  spec'd C1 tests + 7 review-fix regression tests).
- Spec'd C1 tests (§1.4), all passing: `serialize_roundtrip_preserves_conditions`,
  `serialize_roundtrip_preserves_timing`,
  `serialize_roundtrip_preserves_swing` (asserts both pattern and bank),
  `serialize_roundtrip_preserves_cv_locks`,
  `deserialize_v2_into_single_pattern`, `single_pattern_playback_unchanged`
  (exact step-fire tick sequence over two pattern cycles), plus
  `deserialize_v3_skips_unknown_trailing_step_fields` (amendment guard).
- Review-fix regression tests, all passing:
  `deserialize_v3_skips_unknown_trailing_pattern_fields`,
  `deserialize_v3_rejects_zero_step_pattern`,
  `deserialize_v3_clamps_length_to_step_count`,
  `deserialize_v3_pads_steps_to_capacity`,
  `serialize_persists_effective_pattern_index`,
  `pattern_is_used_counts_condition_and_timing_only_steps`,
  `set_pattern_then_step_edit_in_same_batch`.
- `cargo clippy -p paraclete-nodes --all-targets`: warning set identical to
  the pre-change baseline (no new warnings).
- `cargo tree -p paraclete-nodes`: no `paraclete-runtime` /
  `paraclete-scripting` dependency.
- Project-file byte-for-byte roundtrip test (`project_tests.rs`) passes
  against the v3 writer unchanged.

### Review findings

(Pre-commit review pass; findings and dispositions recorded before the code
commit.)

## Commit 2 — Multi-page patterns (page-loop window + per-pattern swing) — shipped `0a8116b`

Implemented 2026-07-11 (autonomous session, spec: `p10-interfaces.md` §2).

- **Page-loop playback (§2.1):** `window()` maps `page_loop` + `length` to a
  `[start, end)` step range; `advance_step()` wraps at the window end — the
  wrap is the cycle boundary (loop_count; C4 will evaluate cued switches /
  chain advances there only). Transport `global_start` enters at the window's
  first step. The bar-sync snap maps transport position into the window
  (`wstart + ticks % wlen`), so a mid-session join lands inside the run that
  plays. Both helpers clamp defensively; the audio thread cannot panic on a
  transiently inconsistent window/length pair.
- **`CMD_SET_PAGE_LOOP` (30):** validate-or-ignore (the §2.4 test wants
  `(2,1)` rejected outright, so "clamp" from §2.1's prose reads as
  validation, not coercion — boring option chosen). Negative fractional
  `arg1` is rejected before the truncating cast (review finding).
- **Steps 0–63 (§2.2):** already satisfied by C1 (steps pre-sized to 64;
  commands bound-check against `steps.len()`); no change needed.
- **Swing per-pattern (§2.3):** `Pattern::swing` is authoritative for
  emission + serialization. The bank slot is a write-through conduit
  (`last_bank_swing` detects encoder writes). Conduit refresh happens at
  `CMD_SET_PATTERN` (C4's stated dependency, implemented early at the switch
  site because a live switch is reachable today via a multi-pattern v3
  blob), `activate()`, and `deserialize()`.

### Review findings (pre-commit, 2 agents: correctness + spec compliance)

1. **Applied — conduit stale on live pattern switch** (correctness,
   CONFIRMED): without the refresh, the first post-switch encoder nudge
   snapped the new pattern to the old pattern's swing, and a write of
   exactly the stale value was dropped by the change guard. Fixed at the
   `CMD_SET_PATTERN` site + regression test
   `pattern_switch_refreshes_swing_conduit`.
2. **Applied — deserialize/page_loop asymmetry** (correctness): a corrupt or
   foreign v3 blob with an invalid window (`(4,5)` on length 16, or
   reversed) loaded into degenerate single-step playback. deserialize_v3 now
   sanitizes to the full span, matching CMD_SET_PAGE_LOOP's validation.
   Regression test `deserialize_sanitizes_invalid_page_loop`.
3. **Applied — negative `arg1` truncation** (both agents): `-0.5 as i64 = 0`
   slipped through validation; rejected before the cast now.
4. **Applied — page_derivation test half-missing** (spec): the §2.4 test
   named "0–7 → page 0, 8–15 → page 1"; the first draft asserted only the
   page-1 half. Both halves now asserted, including through playback.

### Spec conflict record (handoff guardrail 1)

`p10-interfaces.md` §2.3: "Keep `/state/swing` publishing (now sourced from
the active pattern)." **No `/state/swing` path has ever existed** —
`published_state()` never published swing, and nothing else does. The
sentence cannot be "kept"; nothing was added (the C5 state-bus commit is the
natural home if a swing path is wanted — decide there). Recorded rather than
silently implemented.

### Gate results

- `cargo test --workspace`: **480 passed, 0 failures** (473 baseline + 5
  spec tests + 2 review regression tests).
- Clippy: changed hunks clean (crate warning set unchanged from baseline).

## Commit 3 — Per-track length & speed (polyrhythm) + BUG-004 — shipped `e8f7718`

Implemented 2026-07-11 (autonomous session, spec: `p10-interfaces.md` §3).

- **`CMD_SET_LENGTH` (28):** clamps arg0 to 1..=capacity (clamp chosen over
  reject for a scalar range, matching speed's contract; note: SET_PAGE_LOOP
  validates-or-ignores because its test demands rejection); arg1 targets a
  pattern index or −1 = active; unknown index ignored. Page count re-derived,
  window clamped.
- **`CMD_SET_SPEED` (29):** per-track, clamped [0.125, 2.0]. `step_period`
  is computed per step; `period_frac` carries the fractional remainder so
  non-integer multipliers alternate floor/ceil periods with an exact long-run
  average (spec's preferred option; `fractional_speed_carries_remainder_
  without_drift` holds 1.3× within 1 tick over 100 boundaries). Gate length
  scales with the effective period. The bar-sync snap uses the exact
  fractional period — nearest-tick at fractional speeds, documented.
- **BUG-004 (§3.3):** negative micro_offset fires during the previous step's
  window, tick-exact (1/96 beat = 10 ticks). The condition rolls once at
  early-fire time (pre-wrap `loop_count` for a window-wrapping early step —
  documented choice). Direct-at-grid fires (start/resync) clamp a negative
  offset to "on the grid". Marked resolved in `bugs.md`.
- **Polyrhythm:** emergent from per-track length/speed on one clock
  (`two_tracks_polyrhythm`: 16 vs 12 realign only at LCM 48). No mode flag
  (ADR-016).

### Review findings (pre-commit, 2 agents: correctness + spec)

Spec agent: fully compliant, no findings. Correctness agent, all three
applied with regression tests:

1. **Gate truncation for early-fired notes** (CONFIRMED): the old
   compare-against-step_tick gate cut an early-fired note at the *previous*
   step's gate-close tick (~1-tick gate in the common case). Gate is now an
   absolute countdown from note-on (`gate_ticks_left`), preserving the
   firing step's gate across the boundary. Test
   `early_fired_note_keeps_its_gate_length`.
2. **early_fired bool swallowed a different step's fire** when a
   length/window edit landed between the early fire and the boundary. Now
   stores the fired step index; the boundary skips only that exact step.
   Test `window_edit_between_early_fire_and_boundary_does_not_swallow`.
3. **Fractional-speed snap could strand `step_tick == step_period`**
   (period rounds down, `new_tick` derived from the unrounded period),
   skipping the joined step. Clamped inside the step.

### Gate results

- `cargo test --workspace`: **491 passed, 0 failures** (480 after C2 + 8
  spec tests + 1 drift guard + 2 review regression tests).
- Clippy: changed hunks clean (crate warning set unchanged).

## Commit 4 — Seamless switching + volatile chain — shipped `50ef64b`

Implemented 2026-07-11 (autonomous session, spec: `p10-interfaces.md` §4).

- **Pattern bank:** `PATTERN_BANK_SIZE = 8`, allocated at construction on
  the main thread (§4.3's stated budget). `CMD_SET_PATTERN` validates
  against the bank — patterns are never grown on demand, so no audio-thread
  allocation exists anywhere in the switch path (`ParameterBank::set` is
  find+clamp, verified by review).
- **Cue semantics (§4.1):** stopped → immediate; playing → cued, lands at
  the page-loop wrap (the C2 cycle boundary), clears after landing. Every
  switch refreshes the swing conduit and enters the new pattern at its
  window start; `early_fired` clears on switch.
- **Chain (§4.2):** capacity 8, Vec pre-reserved (deserialize rebuilds at
  the same capacity and drops excess). Read-then-advance at the boundary;
  explicit cue wins one boundary without advancing the chain.

### Review findings (pre-commit, 2 agents)

1. **Applied — deserialize bank clamp** (correctness): a foreign v3 blob's
   u16 pattern count was honored verbatim — unbounded memory and a broken
   bank invariant. Now clamped to `PATTERN_BANK_SIZE` (excess records fall
   into the format's ignored-trailing-bytes region); `active_pattern`
   clamped into the bank. Test `deserialize_clamps_pattern_bank_and_active_index`.
2. **Applied — cue survived global_stop** (correctness): a cue pending at
   stop now collapses to an immediate switch (stopped switches are
   immediate by contract; restart plays the last selection, not one
   surprise cycle of the old pattern). Test `stop_applies_pending_cue`.
3. **Recorded — spec-internal conflict** (spec agent): §4.2's literal
   "advances `chain_pos` (wrapping) and switches to `chain[chain_pos]`"
   (advance-then-read) contradicts §4.4's own ordering test (a fresh chain
   `[0,1,2]` must visit 0,1,2). Implementation uses read-then-advance,
   which the test demands. The spec sentence should read "switches to
   `chain[chain_pos]` and then advances it (wrapping)". Spec doc not edited
   (append-only discipline; conflict recorded here).

### Test updates required by the bank

`deserialize_v2_into_single_pattern` asserted `patterns.len() == 1` (true
under C1's single-slot construction); §1.3 says the bank is restored to
`PATTERN_BANK_SIZE` on load, which C4 makes true — the assertion now checks
bank size + "only pattern 0 used". `serialize_persists_effective_pattern_index`
asserted the C1 stub's clamp; the property it protected (a save can never
select a different pattern on load) is now enforced by command-site
validation, and the test asserts that directly.

### Gate results

- `cargo test --workspace`: **499 passed, 0 failures**.
- Clippy: changed hunks clean (crate warning set unchanged).
