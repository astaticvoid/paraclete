# ADR-041: Machine Identity — Runtime Machine Select & Variant Declaration

| Field | Value |
|-------|-------|
| **Status** | ✅ Implemented in MM (2026-08-01) — accepted 2026-07-23 |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Ratified by user 2026-07-23, as written (no amendments) |
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

## Post-ratification hostile review — 2026-07-23 (amendments user-approved)

Findings 1 B / 10 M / 4 m across ADR-041/042/043; this ADR's amendments:

1. **Decision 2 amended — the flat union is insufficient** (review B1).
   `ParameterBank` holds one min/max/default per id, but the real
   machines conflict on shared names (FM `decay` 0.01–2.0 vs 0.05–8.0
   vs 0.05–4.0; `feedback` 0–1 vs 0–0.5; analog `tone` 200–8k vs
   1k–18k; three different defaults). Amended model: **widest-envelope
   storage** in the bank, plus **per-machine descriptor overlays
   (min/max/default, and an `identity` flag) carried in
   `MachineVariant`** — surfaces display/clamp against the active
   overlay; inactive values are never clamped; union docs dedup by id.
   Lossless cross-machine kits hold under this model, not the flat one.
2. **Decision on placement amended — machine-select lives on the TRIG
   page**, not "SRC slot 1" (reviews M2/M8): both engines' SRC slots
   are occupied, FmVoice's pages are exactly 8 wide, and track identity
   belongs with track settings. Prerequisite commit: `merge_page`
   currently **ignores declared slots** (sequential counter) — slot
   honoring in `paraclete-view-assembly` is required before any slot
   convention is real.
3. **Decision 3 extended to the actual wire** (review M4): clients get
   `view_meta` built from the pre-merged `CompositeView`, not `Rule`;
   the extension must carry per-variant pre-merged pages per track and
   populate `stepped`/`options` (currently hardwired `None`) so the
   machine encoder can label variants. Spec'd with the implementing
   phase.
4. **Decision 6's validation site corrected** (review M10): the
   sequencer stores opaque `(node_id, param_id)` locks and cannot know
   a foreign node's params — machine-lock rejection lives **surface/
   app-side**, keyed on the overlay `identity` flag (which scene-assign
   rejection also uses). The performer sees an echo-area rejection.
5. Validation debt (review m15 → BUG-037): a debug-build assertion that
   every page/variant param ref resolves in the union doc.

## Implementation note

```text
ADR-041 implemented in MM (2026-08-01), commits MM-C0 through MM-C6.
```

**Where each decision landed.** `machine` is a stepped bank param on both
engines (MM-C3, MM-C4); the bank stores the **widest-envelope union** and is
never narrowed, with per-machine ranges in `MachineVariant::overlays`
(amendment 1). Variants reach composite assembly and the wire in MM-C5, and
Theotokos in MM-C6. Machine-select sits at TRIG slot 0, declared by the
engines (amendment 2).

**Three things the implementation decided that the ADR left open**, recorded
here because the phase spec is where the reasoning lives (`design/phases/
mm-machine-and-mod.md`, §0 D1–D6):

1. **The declick is a 5 ms fade-out with no fade-in, but the ramp is
   bidirectional.** Decision 4 resets voice state on switch, so there is
   nothing to fade *in* to — but a switch that is cancelled or retargeted
   part-way must return to unity continuously, or it clicks in the other
   direction.
2. **Amendment 2 said TRIG but not who declares it.** The engines do, so every
   surface inherits the control through the merged view rather than each
   surface synthesising its own. Consequence, flagged for the session: TRIG is
   first in `CANONICAL_PAGE_ORDER`, so a page now sits at index 0 ahead of SRC
   and page keys 1-6 select different pages than they did.
3. **Amendment 5's assertion, as literally written, would have passed on
   BUG-037's successor** — "resolves in the union doc" is satisfied by a page
   naming a param the *active* machine lacks. `validate_view`
   (`paraclete-node-api`) therefore checks per variant, and runs over every
   `ViewPlugin` rather than only the machine hosts. That widening is what
   caught #156.

**One hazard this ADR's shape creates, mitigated by assertion rather than
structurally.** §0 A1 puts the `identity` flag on the *overlay*, so it must be
repeated in every variant; miss one and p-lock rejection stops working for that
machine alone. `validate_view` checks the flag is consistent, and Theotokos
reads the union across variants so a miss costs nothing at the point of use. A
`Rule`-level `identity_params` list would remove the hazard structurally; that
deviates from the ratified shape and was not taken.

**Known limitation, filed not worked around:** Antiphon assembles `view_meta`
from a startup cap-doc snapshot, so its `active` and top-level `pages` report
the machine a node was *constructed* with (#157). A client is not stuck — it
watches `/node/{id}/param/machine` and draws the matching pre-merged variant,
which is decision 1's model exactly.
