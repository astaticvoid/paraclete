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
