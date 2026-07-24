# ADR-042: The MOD Page — Per-Node LFO Block

| Field | Value |
|-------|-------|
| **Status** | ✅ Accepted (2026-07-23) |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Ratified by user 2026-07-23, as written (no amendments) |
| **Scope** | `paraclete-nodes/engine_dsp` (new `LfoBlock`), machine engines + `Sampler` + `FilterNode`, `Rule` MOD pages, `LfoShape` affordance |
| **Related** | ADR-018 (cellular — dest stays node-local), ADR-019 (params), ADR-032 (`Rule`; closes the §2.6.5 `LfoShape` gap), ADR-038 (MOD page key `6`), CANONICAL_PAGE_ORDER (MOD is declared but empty today) |

> Design-prose analogy: the Elektron per-track LFO page. Marks per house
> naming policy.

---

## Decision

**Modulation becomes a page, not just a patch: a reusable `LfoBlock`
(L2 helper in `engine_dsp`) gives any node a MOD page whose LFO targets
that node's own params.** The existing modulation-by-wiring plane
(`LfoNode`/`EnvelopeNode` → Modulation ports on oscillator/ladder/
sampler) is untouched — it remains the modular-synthesis path; the
MOD page is the groovebox path.

1. **One `LfoBlock` per hosting node (v1), seven bank params** — one
   full encoder-bank page:

   | Param | Range | Notes |
   |---|---|---|
   | `lfo_shape` | stepped: tri, sine, sqr, saw, exp, ramp, rand (S&H) | drawn via `LfoShape` affordance |
   | `lfo_speed` | 0.01–64 Hz, exponential taper | free-running Hz in v1; tempo-sync staged (see 5) |
   | `lfo_mode` | stepped: free, trig, hold, one, half | the Elektron mode set: free-run; retrig on note; sample-and-hold on note; one-shot; half-cycle |
   | `lfo_start_phase` | 0–1 | applied in trig modes |
   | `lfo_fade` | −1…+1 | fade-in (+) / fade-out (−) on trig |
   | `lfo_dest` | stepped: 0 = off, 1..N = index into the node's own declared params (stable declaration order, `machine`-class identity params excluded) | display strings from `ParamDescriptor` names |
   | `lfo_depth` | −1…+1 (bipolar, fraction of the dest param's range) | |

2. **Dest is node-local, by design.** A node modulates only its own
   params (ADR-018 — no node commands another). The Elektron
   divergence is deliberate and stronger: on a Paraclete track the
   *composite* MOD page (canonical order already reserves the slot)
   stacks each chain node's own LFO — engine wobble *and* filter
   wobble, one page, more LFOs per track than the hardware. Cross-node
   modulation remains the wiring plane.

3. **Application is an offset, never a rewrite:**
   `effective = clamp(base + depth × range × lfo(t))` computed
   per block (64-sample rate; per-sample for pitch-class dests is a
   phase-spec option). The bank value is untouched — p-locks, kits,
   scenes, and the state bus all see the *base*; the LFO breathes on
   top. No feedback into `CMD_BUMP_PARAM` reads.

4. **Fully lockable and performable:** `lfo_depth`, `lfo_speed`,
   `lfo_dest` are ordinary bank params — p-lockable per step (the
   dest-lock trick), kit-captured, scene-morphable (continuous ones),
   encoder-bank-joggable.

5. **Tempo sync is staged, surface-first** (the ADR-040 §7 pattern):
   `lfo_speed` in Hz ships now. BPM-multiplier sync requires tempo at
   the node — engines currently receive no tempo. The staged design:
   the clock's existing `TransportEvent` stream reaches engines via
   their `events_in` (sequencer passthrough — additive), and a
   `lfo_sync` toggle reinterprets `lfo_speed` as a musical multiplier.
   Param surface frozen now; sync lands with the passthrough
   (OQ-M2). No surface changes when it does.

6. **Rollout order:** machine engines (`AnalogEngine`, `FmEngine`) and
   `Sampler` first, `FilterNode` second, FX nodes on demand. `FmVoice`
   (ADR-043) ships with it. `Rule` gains the MOD page group + the
   first real `LfoShape` affordance declarations (closing the known
   §2.6.5 gap); Theotokos's live-viz phase animates them via the
   published `lfo_phase` state (design.md §5.3(b), TK2 C9 — same
   push-down, now per-engine).

## Alternatives considered

- **Track-level LFO entity (exact Elektron shape)** — rejected: a
  track is an emergent chain, not an object (ADR-018); a track-scoped
  modulator would need cross-node command authority that nothing has.
  The per-node stack on a merged page keeps cellularity and gives
  strictly more capability.
- **Route MOD-page modulation through `LfoNode` + auto-wired
  Modulation ports** — rejected: auto-adding graph nodes/edges per
  MOD page entangles topology with UI state, makes p-locking the LFO
  params a cross-node affair, and the destinations (arbitrary bank
  params) mostly have no Modulation port to receive on.
- **Modulate by emitting `CMD_SET_PARAM` at control rate from the
  app** — rejected: command-plane spam, no sample-block coherence,
  and it would fight the performer's own encoder writes (the
  base/offset split in decision 3 is precisely what avoids this).

## Consequences

- **New:** `LfoBlock` (pure, unit-testable — trigger/tick like
  `AdState`); 7 params × hosting node; MOD page content in composite
  views; `LfoShape` affordances; `/node/{id}/state/lfo_phase` on hosts.
- **Second LFO per node** is a param-surface duplication
  (`lfo2_*`, overflow page via FUNC+PG per ADR-038) — deliberately
  deferred until a session asks (OQ-M1).
- **Open:** OQ-M1 (LFO2), OQ-M2 (tempo distribution / sync stage),
  OQ-M3 (per-sample application for pitch dests), OQ-M4 (dest
  indexing stability across machine variants — dest indexes the union
  bank, ADR-041, so indices stay stable by construction; confirm in
  spec).

## Post-ratification hostile review — 2026-07-23 (amendments user-approved)

1. **Decision 3 disambiguated** (review M5): engines render p-locked
   steps from `node_locks`, not the bank — "base" in the formula is the
   **`get_param()` result**: `effective = clamp(lock_value_or_base +
   depth × range × lfo(t))`. The LFO breathes on top of a locked step's
   value; locks never defeat the LFO nor vice versa.
2. **`lfo_dest` storage amended** (review M6): declaration-order indices
   are not stable and nothing addresses params by index — dest is
   stored as the target param's **name-hash id**, with the stepped
   encoder's display index derived at build from an explicit,
   append-only dest table per engine. OQ-M4's "stable by construction"
   is deleted. `lfo_*` params are excluded from the dest set (review
   m14 — no self-modulation).
3. **Composite alignment** (review M3): each node's MOD contribution is
   **padded to 8-slot alignment** so a second node's LFO never
   straddles a sub-page boundary (depends on the ADR-041 slot-honoring
   prerequisite).
4. **Control-rate structure named** (review m13): per-block application
   at host block size (512) aliases a 64 Hz LFO; the **64-sample
   sub-block loop is new required engine structure**, spec'd with its
   interaction with event-split spans in the implementing phase.
5. **Dest display labels are surface-derived** from the known param-name
   list (review M11): `ParamDisplayAdapter::Dynamic` panics on clone
   along the mainline cap-doc path, and the wire never carries options —
   descriptor-side dynamic display is not usable as specced.

## Implementation note (to be added when implemented)

```text
ADR-042 implemented in [phase] (YYYY-MM-DD).
```
