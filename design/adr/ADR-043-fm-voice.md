# ADR-043: FM Voice — Four-Operator Melodic FM (P14 Model)

| Field | Value |
|-------|-------|
| **Status** | 🟡 Proposed (2026-07-23) |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Pending user |
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

## Implementation note (to be added when ratified)

```text
ADR-043 implemented across P14 (YYYY-MM-DD).
See design/phases/p14-*.md for the commit plans.
```
