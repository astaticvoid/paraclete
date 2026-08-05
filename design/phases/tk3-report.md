# TK3 — Theotokos Phase Report

> **Status: code-complete, awaiting session #6.**
>
> **Baseline:** P11 closed, MM closed, TK2.2 closed. `cargo test --workspace`
> green at baseline.
>
> **Spec:** `design/phases/tk3-theotokos.md` (execution-ready 2026-08-05).
>
> **Design authority:** ADR-044 (accepted), ADR-037 (accepted), ADR-038
> (accepted). 14 open questions pre-resolved 2026-08-03.

---

## Summary

TK3 closed visible gaps (#180, #163, #184), added the one missing screen
(MIX), and implemented three resolved OQs (OQ-T4 step-size scaling, OQ-T23b
dual-path tap tempo, OQ-T31 modifier highlight). Eight commits (C0–C6 +
timing fix), 2918 insertions / 425 deletions across 12 files. All tests
green (252 in `paraclete-theotokos`), clippy clean on touched crates.

The phase is breadth and polish — no new architecture, no new modes, no
WT convergence code, no chromatic grammar. The one architectural addition
(`EncoderTarget::VirtualStep`) is model-side only; the composite view
assembly is untouched.

---

## Commits

| # | SHA | Title | Issues |
|---|-----|-------|--------|
| C0 | `0e8de8c` | TRIG page screens the four sequencer per-step params | #180 |
| C1 | `77e4ec4` | Extract RenderData assembly into a testable free function | #163 |
| C2 | `3f06deb` | MIX screen — per-track level view + encoder gain jogs | — |
| C3 | `4d1bdfc` | Expose pending app ops for headless testing | #184 |
| C4 | `9d16b52` | ENC + jog step-size scaling | OQ-T4 #126 |
| C5 | `6bfb638` | Dual-path tap tempo — global FUNC+Space + Tempo encoder | OQ-T23b #125 |
| C6 | `56ebdfa` | Modifier-highlight chips on the status line | OQ-T31 #147 |
| fix | `e377a9a` | Denormalize timing encoder jog to raw micro_offset | #180 |

---

## What shipped

### C0 — TRIG virtual params (#180)

The TRIG page had 7 empty slots and the sequencer's four per-step params
(velocity/length/timing/condition) had no encoder home. They are not bank
params — each is step-internal data accessed via dedicated `CMD_SET_STEP_*`
commands — so the sequencer is not in the `TrackChain` and the composite
view assembly cannot reach them.

**The fix is model-side.** `EncoderParam` gains `target: EncoderTarget`
(`Real` | `VirtualStep { seq_id, kind }`). `resolve_encoder_params()`
detects the TRIG page and appends four virtual entries at slots 1–4. The
jog dispatch matches on `target`: `VirtualStep` emits the dedicated
`CMD_SET_STEP_*` command targeting the focused step (p-lock target, else
current playing step). Condition is a read-modify-write preserving
probability/repeat and swapping only the fill field.

The sequencer publishes a new `/node/{id}/state/step_detail` bus path —
one semicolon-delimited tuple per step carrying all seven fields
(velocity, length, timing, probability, repeat_n, repeat_m, fill). This
is the **only** structured-text bus path (documented as not a pattern to
copy). A `ViewPlugin` impl on `Sequencer` declares the four synthetic
params for cap-doc completeness but is deliberately not wired into
`capability_document` (the sequencer's bank is built from its cap-doc
params, and these are not bank params — wiring would create phantom bank
slots).

Cross-crate round-trip test (`step_detail_round_trip.rs`) proves the
format contract end-to-end.

**Timing bug found in review:** the timing encoder jog sent the
normalized -1..1 value directly as `CMD_SET_STEP_TIMING` arg1, but the
sequencer handler casts arg1 to `i8` — so `~0.016` truncated to 0 and
the timing encoder was effectively a no-op. Fixed in `e377a9a`: multiply
by 47.0 and round before sending.

### C1 — RenderData extraction (#163)

`RenderData` was assembled inline in `render_if_needed` (~290 lines of
bus reads and model queries mixed with the draw call). Extracted into a
module-level free function `build_render_data(model, bus, held, keymap,
tuning, last_jog_param, debug_event) -> RenderData`. Takes `&mut Model`
for the slot/encoder flash side effects; no I/O. Five unit tests cover
default state, param page encoder cells, mute reads, armed prefix, and
KIT screen state.

#138/#139 (TK1 carryover — 7 unwritten tests + missing 8-track fixture)
not folded in — non-trivial per spec §C1.

### C2 — MIX screen

One new screen: `Screen::Mix`. Opened by `FUNC+8` (Settings is on bare
8). Contextual window shows one row per track (`model.tracks.len()`, not
MixNode's input count): name, block-element level bar over the 0.0–2.0
gain range, and the two-decimal value, then a MASTER row. Encoder
columns 1..N map to `input_gain_{n}` on MixNode (node 2); column 8 maps
to `master_gain`.

`MIX_NODE_ID = 2` mirrors the hard-coded app graph convention. The
MixNode may be absent from `model.caps`, so gains are read by name
directly from the bus.

### C3 — Expose pending_app_ops() (#184)

Added `pub fn pending_app_ops(&self) -> &[AppOp]` on `Model` for
non-destructive inspection. The existing destructive
`take_pending_app_ops()` drain is unchanged. Test verifies the observer
does not consume ops and that the drain still works after reads.

### C4 — Step-size scaling (OQ-T4, #126)

`Model.step_size_tier` (0..=4 = ×1, ×2, ×4, ×8, ×16), persisted until
changed. `Action::SetStepSizeTier(i8)` produced by `button_to_action` in
ENC mode when Ctrl+FUNC are both held — Fine/Coarse are suppressed
simultaneously. Outside ENC mode, Ctrl+FUNC keeps its existing Fine-jog
meaning.

The tier scales the base jog step by `2^tier` by scaling the range fed
to `Tuning::jog_step` — the base is `max(0.001, range/128)`, so scaling
`range` scales the base before the ramp, leaving the acceleration curve
shape unchanged. Applies to real, VirtualStep, and MIX jogs; stepped
params unchanged. Status line shows `ENC×{mult}`.

### C5 — Dual-path tap tempo (OQ-T23b, #125)

Two paths to set tempo:
1. **Global chord:** `FUNC+Space` taps tempo from any screen. Companion
   intercept to ADR-044 A12 — fires before it, uses raw key + FUNC to
   distinguish Space from `x`, so `TapTempo` is produced instead of the
   collapsed `ClearLane`. A12's ClearLane protection is preserved: the
   destructive clear still requires the literal `x`.
2. **Encoder jog:** Tempo screen + encoder column 1 = continuous BPM jog
   via `CMD_SET_PARAM` on the clock (absolute value, not bump).

### C6 — Modifier highlight (OQ-T31, #147)

`HeldState` gains `func_held: bool` and `ctrl_held: bool` persistent
fields, tracked in the kitty key loop by mirroring each event's
modifiers. `build_render_data` surfaces these as `held_modifiers`; the
status line renders bright SHIFT/CTRL chips while held.

Sticky-fallback terminals have no releases, so `func_held`/`ctrl_held`
stay false and only the existing armed prefix shows (unchanged
behaviour). TRK…/PTN…/LOCK… continue to ride the existing
`armed_prefix` chip; REC keeps its three-state indicator.

---

## Review findings

**Timing encoder no-op (fixed in `e377a9a`):** The timing virtual
encoder jog sent the normalized -1..1 value (`micro_offset / 47.0`)
directly as `CMD_SET_STEP_TIMING` arg1, but the sequencer handler casts
arg1 to `i8` — so `~0.016` truncated to 0 and the timing encoder was
effectively a no-op. Fixed by multiplying by 47.0 and rounding before
sending. Test added: `encoder_jog_on_virtual_timing_denormalizes_to_raw_micro_offset`
(mutation-checked: reverting the fix → test fails with `arg1 = 0.015625`).

**MIX screen tier scaling (noted, not fixed):** The step-size tier
multiplier applies to MIX jogs as well as param jogs. At tier 4 (×16),
a MIX jog jumps by `2.0 * 16 / 128 = 0.25` per tick — a quarter of the
full gain range. This may be intentional (the performer asked for big
steps), but MIX gains are a different domain from param editing. Flag
for session #6.

**Tempo encoder uses absolute set, not relative bump (noted, not fixed):**
The Tempo encoder reads the current BPM, adds the delta, and sends
`CMD_SET_PARAM` with the absolute new value. This is correct (it needs
to accumulate the delta against the live BPM), but it's a different
pattern from every other encoder jog (which use `CMD_BUMP_PARAM`). If
the BPM changes between the read and the command being processed (e.g.,
from a tap tempo), the jog overwrites the change. Low risk — the audio
thread processes commands in order — but worth noting.

---

## Test coverage

| Layer | Tests | Catches |
|---|---|---|
| Unit (sequencer) | `ViewPlugin` impl, `published_state` step-detail | wrong page placement, malformed bus data |
| Unit (Theotokos model) | `parse_step_detail` (7 fields / step order / malformed), `pack_condition` preserves prob+repeat, TRIG resolves `VirtualStep` 1-4, non-TRIG leaks none, velocity jog emits `CMD_SET_STEP_VELOCITY` (not `BUMP`), condition jog read-modify-write, timing jog denormalizes to raw micro_offset | parse errors, wrong command mapping, virtual leakage, repeat/probability clobber, timing no-op |
| Unit (Theotokos render) | MIX screen layout, modifier chips, virtual param glyph | layout breakage, missing chips, missing `▸` prefix |
| Unit (RenderData) | `build_render_data()` with synthetic inputs (5 tests) | data assembly regressions |
| Integration (cross-crate) | `step_detail_round_trip.rs` — real Sequencer publishes, `parse_step_detail` reads all seven fields back | format drift between producer and consumer |
| Mutation | velocity jog → `CMD_BUMP_PARAM` test fails; condition RMW → prob/repeat clobber test fails; timing jog → arg1 range test fails; step-detail bus path → parse test fails; tier multiplier → scaling test fails; `FUNC+Space` intercept → tap-tempo test fails | all key paths mutation-checked |

**Harness live test (Leg 2):** waived per spec TK3 §5.4 — the unit
round-trip covers the step-detail format contract; a test-driver
scenario is explicitly optional in the spec.

**Live session (Leg 3):** rolled into the phase's session #6 gate (C7),
which needs the user; the ENC-jog surface gesture exists so this is a
tracked milestone, not a silent skip.

---

## Exit criteria status

| # | Criterion | Status |
|---|-----------|--------|
| 1 | `cargo test --workspace` green after every commit | ✅ |
| 2 | `cargo clippy --workspace` clean on touched crates | ✅ |
| 3 | Each commit carries unit tests for the new logic | ✅ |
| 4 | C0's step-detail bus path verified by unit test | ✅ |
| 5 | Code review (Flash) on each commit or logical batch | ✅ full-round review completed |
| 6 | **Usability session #6** | ⏳ pending (needs user) |

---

## Deferred items

| Item | Why deferred | Trigger for re-evaluation |
|---|---|---|
| OQ-T21 (#128, chromatic grammar) | No melodic engine exists | P13 or P14 starts |
| Ordo profile switching/wizard | No session has produced a "I wish I could switch layouts" moment | A session asks for it |
| ADR-045 (#127, cross-surface lock capture) | Parked per session #4 judgment | User decides to unpark |
| #140 (yank lossy — note) | C0 adds velocity/length/timing/condition to the bus but not note; yank still cannot copy the note | A phase needs deep step inspection including note |
| Chain view | Was in original TK3 vision; no session has asked for it | A session asks for it |
| Macro support from `Rule` | Assembly merges macros but no shipped node declares any | A node produces macros |
| WT convergence decision | Resolved by decision in §0 (OQ-T12); no implementation needed | — |
| #138/#139 (TK1 carryover) | Non-trivial — depends on gen-samples output and a new instrument-8track.yaml | A phase owns TK1 test debt |

---

## Session #6 gate

The phase closes with usability session #6
(`design/sessions/theotokos-6.md`). The user plays the instrument with
the MIX screen, modifier highlight, step-size scaling, and global tap
tempo. Findings recorded in the session notes. The MIX screen key
binding (`FUNC+8`) is tunable — session #6 may reassign.

After session #6 concludes and the phase is signed off, close milestone
18 (`TK3 — Theotokos session track`).
