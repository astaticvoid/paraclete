# ADR-041: Machine Identity — Runtime Machine Select & Variant Declaration

| Field | Value |
|-------|-------|
| **Status** | 🟡 Proposed (2026-07-23) |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Pending user |
| **Scope** | `paraclete-node-api` (Rule extension), machine-host engines (`AnalogEngine`, `FmEngine`, future `FmVoice`), `paraclete-view-assembly`, surfaces (variant-aware page display), ADR-039 kit apply |
| **Related** | Roadmap D2/OQ-14 (machine-as-parameter — **decided 2026-07-13, implements here**), ADR-019 (param plane), ADR-032 (view contract — additive extension), ADR-038 (page keys), ADR-039 (kits) |

> Design-prose analogy: the Syntakt per-track machine switch. Marks per
> house naming policy.

---

## Decision

**Machine selection becomes a runtime stepped bank param (`machine`) on
machine-host engines, backed by a union parameter bank and
pre-declared per-machine `Rule` variants.** No capability re-query
exists or is added — cap-docs are collected once at startup
(main.rs step 7, before the executor owns the nodes), and that stays
true. Everything a surface needs for every machine ships in the
startup contract.

1. **`machine` is a declared stepped param** (`min 0, max N-1`),
   mutated via ordinary `CMD_SET_PARAM` — ADR-019, no new command. The
   existing fixed constructors (`AnalogEngine::kick()` …) become
   default values of `machine`; `instrument.yaml` keeps working
   unchanged.

2. **Union bank.** The engine's cap-doc declares the **union of all
   its machines' params**. Name-hashed ids make this cheap — the
   current machines already share most names (`tune`, `decay`,
   `tone`…). Params not used by the active machine are *inert but
   retain values*: switching away and back loses nothing, and a kit
   (ADR-039) captured on machine A applies losslessly to a track
   currently on machine B — the values sit in the bank until the
   machine that reads them is selected.

3. **Pre-declared variants in the view contract.** `Rule` gains an
   additive field: `variants: Vec<MachineVariant { value, name,
   page_groups, pages }>` — one entry per machine, carrying that
   machine's page layout. Base-`Rule` fields remain the default (used
   when `variants` is empty — every existing node unaffected; wire
   format additive, ADR-032 revision note). Surfaces already mirror
   every param from the state bus; they watch `machine` and swap the
   displayed variant locally. **Zero runtime negotiation.**

4. **Switch semantics in the engine:** on `machine` change the engine
   swaps its DSP path at block boundary with a short declick fade
   (~5 ms *(tunable)*); voice state (phases, envelopes) resets;
   per-machine state is pre-allocated at build (no `process()`
   allocation).

5. **Kit interaction (ADR-039):** kits include `machine`; the app
   applies `machine` **first**, then the remaining params (a stable
   sort in kit apply — one rule, no races). Scene morphing (ADR-040)
   **must not assign `machine`** — stepped identity is not morphable;
   scene-assign rejects it.

6. **P-locking `machine` is forbidden in v1.** `CMD_SET_LOCK_TARGET`
   validation rejects the `machine` param id (per-step machine
   switching with declick fades inside a step is undesigned; revisit
   with session evidence).

## Alternatives considered

- **Runtime cap-doc re-query** — rejected: nodes are owned by the
  audio-thread executor after startup; adding a query channel builds a
  new plane for a problem pre-declaration solves statically.
- **One node type per machine + topology swap on select** (ADR-029
  patches) — rejected: machine switch is a *performance* gesture; a
  pause-rebuild-resume per twist of an encoder is the wrong tool, and
  track identity (locks, kit refs, edges) would churn.
- **Per-machine banks (no union)** — rejected: value loss on switch,
  and kits/locks would need machine-scoped addressing. The union bank
  makes cross-machine state trivially well-defined.

## Consequences

- **New:** `MachineVariant` in `paraclete-node-api::Rule` (additive);
  variant plumbing through `paraclete-view-assembly` (composite keeps
  per-node variants keyed by the owning node); variant-aware page
  display in Theotokos/Theoria (watch `machine`, swap pages).
- **Engines:** `AnalogEngine`/`FmEngine` gain `machine` param, union
  docs, declick switch; their 3 machines each become runtime-selectable
  — the first Syntakt-style behavior in the instrument. `FmVoice`
  (ADR-043) is born variant-native.
- **Enables:** machine-select on the SRC page slot 1 by convention
  (stepped encoder — the hardware gesture); kit-driven machine changes
  on pattern switch (P11).
- **Open (phase spec):** exact declick constant; whether variant names
  surface in the `:` fuzzy index; per-step machine locks (revisit).

## Implementation note (to be added when ratified)

```text
ADR-041 implemented in [phase] (YYYY-MM-DD).
```
