# ADR-043: FM Voice — Four-Operator Melodic FM (P14 Model)

| Field | Value |
|-------|-------|
| **Status** | ✅ Accepted (2026-07-23) |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Ratified by user 2026-07-23, as written (no amendments) |
| **Scope** | New engine `FmVoice` (L3, `paraclete-nodes`), P14 phase family |
| **Related** | Roadmap P14 ("four-operator melodic FM, macro-first"), ADR-041 (variant-native machines), ADR-042 (MOD page), ADR-032 (`Rule` pages), P13 (voice-model precedents OQ-13/OQ-15), instrument-vision two-tier (macro-first) principle |

> Design-prose analogy: the Digitone page discipline (SYN1/SYN2
> operator model). Marks per house naming policy.

---

## Decision

**A new `FmVoice` engine: four-operator phase-modulation FM with eight
curated algorithms, Digitone-shaped SYN1/SYN2 page discipline, and
machine variants (ADR-041) as the macro tier.** The existing two-op
`FmEngine` is untouched — it remains the lightweight FM *drum* machine
family; `FmVoice` is the melodic instrument.

1. **Operator model.** Four operators in three groups — **C** (carrier),
   **A** (modulator), **B** (a linked pair B1+B2 sharing one ratio
   control with a fixed offset spread) — the grouping that makes 4-op
   playable on 8 encoders. **Eight algorithms** (stepped `algo`)
   covering the canonical shapes: single stack, double stack, Y-split,
   parallel carriers, feedback variants; exact routing table frozen in
   the P14 spec. `feedback` on the algorithm's designated op; `mix`
   crossfades the two output taps (X/Y) each algorithm defines.

2. **SYN1 page (8 params):** `algo` · `ratio_c` · `ratio_a` ·
   `ratio_b` · `harm` (partial-shaping waveform blend on C/A) ·
   `detune` · `feedback` · `mix`. Ratios are **stepped through the
   musical set** (0.25, 0.5, 0.75, 1 … 16 — exact table in the spec);
   `detune` is continuous fine offset.

3. **SYN2 page (8 params):** two modulator envelope groups, A and B —
   `atk_a` · `dec_a` · `end_a` · `lev_a` · `atk_b` · `dec_b` ·
   `end_b` · `lev_b` (attack/decay-to-end-level/modulation-level; the
   envelope shape that makes FM brightness playable without an
   operator matrix). Carrier amplitude uses the engine AMP page
   (attack/decay/sustain/release/volume/pan/drive/overdrive-tone —
   final shape in the spec).

4. **Expression params** (overflow page via FUNC+PG, ADR-038):
   `keytrack` (ratio-independent brightness tracking), `vel_mod`
   (velocity → modulation level), `env_reset` (retrig vs legato
   envelopes), `porta` (glide). Full melodic note handling via the
   existing `note_to_hz` path; **monophonic per track in v1** —
   paraphonic allocation arrives with P13's allocator work (OQ-15),
   shared, not duplicated here.

5. **Macro tier = machine variants, not new plumbing.** `FmVoice` is
   born variant-native (ADR-041): machine 0 = **Full** (SYN1/SYN2/AMP
   as above); curated macro machines (candidates: `Keys`, `Bass`,
   `Bell`, `Perc`, `Pad` — set frozen in the spec) expose a single
   8-slot SRC page of the *most musical* deep params for that
   character, with tuned defaults for the rest. Macros are **curation
   over the same union bank** — no combo-param engine; true
   multi-target macros remain P16 scope. This is the two-tier
   instrument-vision principle made concrete: depth underneath,
   playability on top, one param truth.

6. **MOD page** via ADR-042 `LfoBlock` from day one; envelope/LFO
   `Rule` affordances declared so Theotokos/Theoria render SYN2
   envelopes and LFO shapes without surface work.

7. **DSP ground rules:** pure-Rust PM core in `engine_dsp` (shared
   sine/feedback op primitive, unit-tested at the operator level);
   block-based with per-sample phase accumulation; no allocation in
   `process()`; anti-click on algo/machine switch per ADR-041;
   baseline audio regression per ADR-035 once the spec's reference
   patches exist.

## Alternatives considered

- **Extend the two-op `FmEngine` to four ops** — rejected: its three
  machines are tuned drum instruments with tiny param sets; growing
  them risks their sound and conflates drum-macro and melodic-depth
  tiers. Syntakt-style coexistence (different machines, different
  depth) is the model.
- **Six/eight operators or user-patchable algorithms** — rejected for
  P14: the eight-algorithm, four-op shape is the proven playable
  ceiling for encoder-first FM; open operator matrices belong to a
  hypothetical later machine, not the voice that must be performable
  from 8 encoders.
- **Macro tier as separate combo params (one knob → many targets)** —
  rejected here: that is the P16 macro system's job, instrument-wide;
  duplicating it inside one engine would create two macro planes.

## Consequences

- **New:** `FmVoice` engine + operator primitives in `engine_dsp`;
  ~35 declared params across SYN1/SYN2/AMP/expression/MOD — the first
  Digitone-density track in the instrument; machine variants exercise
  ADR-041 from birth.
- **P14 phase family** becomes concrete: spec (front-load rule) →
  DSP core with operator unit tests → pages/variants → baseline
  patches → session. Depends on ADR-041 + ADR-042 landing first.
- **P13 unaffected** in scope but shares precedents (allocator OQ-15,
  the compile-to-monolith pattern from OQ-13 applies to the PM core).
- **Open (P14 spec):** algorithm routing table; ratio table; `harm`
  waveform set; AMP page final shape; macro machine set + their SRC
  curations; baseline reference patches.

## Post-ratification hostile review — 2026-07-23 (amendments user-approved)

1. **SYN1/SYN2 are SRC sub-pages 1/2** (slots 0–7 / 8–15) — review M7:
   non-canonical page groups sort after MOD, the opposite of the
   intended discipline; sub-paging keeps ADR-038's six page keys intact
   and rides the TK2 §0 A1 sub-page gesture. Macro machines' curated
   page is SRC sub-page 1.
2. **Machine-select moves to the TRIG page** (with ADR-041 review
   note 2) — resolving review M8's 9-params-into-8-slots arithmetic:
   every FmVoice page is exactly 8 wide again.
3. **The gated voice model is a named P14 dependency** (review M9): AMP
   sustain/release, `env_reset` legato, and `porta` require an
   `AdsrState` primitive, NoteOff consumption (engines currently
   discard it), and a mono note stack — a foundational work-package
   listed as its own commit in the P14 spec, not an AMP-page line item.
   The sequencer already emits NoteOff with computed gate times, so the
   upstream exists.
4. **One normative algorithm example** (review m12) so the frozen SYN1
   params have fixed meaning ahead of the full routing table: *algo 1
   "stack":* C←A←B1←B2 serial; feedback on B2; X tap = C output, Y tap
   = A output (pre-carrier); `harm` shapes C and A oscillators; `mix`
   crossfades X/Y. All eight algorithms follow this description format
   in the P14 spec.

## Implementation note

```text
ADR-043 implemented across P14 (2026-08-22), commits P14-C0 through P14-C9.
See design/phases/p14-fm-voice.md for the commit plans and §0 for the
musical tables (algorithm routing, ratio set, harm formula, macro
recipes) the user delegated to the agent rather than freezing by ear.
```

**Where each decision landed.** The operator model (decision 1) is
`FmOperator` (`engine_dsp.rs`, P14-C1) — four operators (C/A/B1/B2)
composed per algorithm in `FmVoice::process_algo` (P14-C2), the eight
algorithms exactly as named in the spec's §0 D1 table (a superset of the
one normative example this ADR's amendment 4 required). SYN1/SYN2 (decisions
2-3) are SRC sub-pages 1/2 as amendment 1 requires (P14-C2/C3); the gated
voice model amendment 3 named as a foundational work-package is `AdsrState`
+ `NoteStack` (P14-C0, unhosted per MM-C8's precedent) plus `NoteOff`
consumption and the retrig/legato split (P14-C4/C5). Macro tier (decision
5) is ADR-041 `MachineVariant`s over the same union bank, six macros plus
Full (P14-C7) — machine-select sits on TRIG per amendment 2. MOD page
(decision 6) is `LfoBlock`/`LfoHost` from P14-C6, a 26-entry dest table
excluding `algo`/`env_reset` as topology/behavior selectors. DSP ground
rules (decision 7): no allocation anywhere in the render path; `harm`'s
additive-harmonic formula (not a fixed waveform crossfade — the spec's own
§0 D3 ruling, made in the user's stead per their delegation) is peak-safe
by the triangle inequality but not loudness-preserving (OQ-P14-4, unfixed,
named for a future gain-compensation decision); anti-click on algo/machine
switch via two independent `SwitchFade`-style mechanisms (P14-C2/C7).

Two real bugs surfaced and fixed during implementation are worth recording
here since they bear on the "gated voice model" this ADR named as risky:
self-feedback folded into a *running* phase accumulator converges to a
fixed point and goes silent (P14-C1, fixed by keeping the phase clock
purely frequency-driven); `FmVoice` shipped through P14-C4/C8 with no
`CMD_TRIGGER` handler and a NoteOff dispatch bug that re-attacked a
still-sounding chord note when an *inner* note released (both found while
wiring P14-C9's baseline scenario, fixed as a standalone prerequisite —
see that commit). Node id **28**, single type_tag `"fm_voice"` — a
deliberate deviation from this engine family's own `fm_engine:kick|bell|
bass` per-machine-tag convention, named and justified in the P14 spec's
C8 bullet.

**Outstanding, by design:** a live `machine` switch does not auto-apply a
macro's recipe (matches `FmEngine`'s own precedent exactly) — the six
macros only sound distinct via P14-C9's reference-patch timelines, not a
bare switch; the roadmap's "baseline patches + session" gate is a
playability/UI session, not a tuning-by-ear one, since the user delegated
the musical judgment outright rather than freezing it by ear.
