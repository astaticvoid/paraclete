# Paraclete — P14 Specification (FM Voice)

**Phase tag:** `P14` · **Milestone:** "P14 — FM Voice"
**Design authority:** ADR-043 (FM voice, four-op melodic PM, accepted
2026-07-23). Depends on ADR-041 (machine identity) and ADR-042 (MOD page),
both landed in MM.
**Also binding:** ADR-018 (cellular — no node commands another), ADR-019
(param plane), ADR-032 (view contract), ADR-038 (page keys), ADR-035
(regression baselines).

**Chosen as next phase** (user, 2026-08-22) after MM/TK3 closed — the only
phase in the Implementation Order table whose remaining gate ("user freezes
the musical tables") the user delegated to the agent outright: *"I don't
know anything about fm, so don't lock it down, keep it expressive, but
enable human interface level detail potential."* §0 below **is** that
freeze — it is the deliverable the roadmap's ⛔ gate was waiting on, ruled
by the agent per that instruction rather than by the user's ear. The
*second* roadmap gate — "baseline patches + session" — still stands after
implementation, but is expected to be a playability/UI session rather than
a tuning-by-ear one, for the same reason.

---

## §0. The musical tables — ruled here, per user delegation

ADR-043 named six things as "open (P14 spec)": algorithm routing table,
ratio table, `harm` waveform set, AMP page final shape, macro machine set +
curations, baseline reference patches. Ruled below, biased toward **reach**
(the full union bank is always present and editable) over safety —
per the user's "keep it expressive" instruction, macro machines narrow the
*visible SRC page*, never the underlying param range. Machine 0 ("Full")
always exposes everything; nothing is ever removed, only curated.

### D1 — Eight algorithms

Groups per ADR-043 decision 1: **C** (carrier, the only op that can be an
output tap on its own), **A** (modulator), **B** (linked pair B1+B2, one
`ratio_b` control, B2 = B1 + a fixed spread — see D2). Each algorithm names
its serial/parallel routing, its feedback operator, and its X/Y output taps
(`mix` crossfades X→Y). This follows the amendment's normative format
exactly (ADR-043 post-ratification note 4).

| # | Name | Routing | Feedback | X tap | Y tap | Character |
|---|------|---------|----------|-------|-------|-----------|
| 1 | **Stack** | B2→B1→A→C serial | B2 | C | A (pre-carrier) | Deep single voice; Y alone is a plain 2-op tone, X is the full 4-op stack — `mix` sweeps stack depth |
| 2 | **Short Stack** | B1→A→C serial; B2 unused in chain, sums with B1 into A as a fixed-offset unison pair (thickens the modulator, not a separate branch) | A | C | B1+B2 (pre-A, the raw pair) | Simpler/brighter than Stack; Y exposes the raw detuned pair for metallic/chorused modulator tones |
| 3 | **Y-Split** | B1→A and B2→A in parallel (two independent modulators onto one op, offset ratio gives a two-tone modulator complex), A→C | A | C | A alone | Complex, slightly inharmonic modulator; classic "Y" shape |
| 4 | **Branch** | B1→C directly (short path) and B2→A→C (deep path) sum into C | B2 | C (full sum) | A→C branch alone (B1 branch muted) | `mix` blends a bright direct branch against a darker/deeper one |
| 5 | **Parallel Carriers** | B1→C, B2→A; C and A both output directly and sum | B1↔B2 (mutual) | C branch alone | A branch alone | Two independent 2-op voices layered — genuinely two timbres, not one shaped tone; best for keys/pad layering |
| 6 | **Feedback Pair** | B1↔B2 mutual feedback pair → A → C serial | B1/B2 pair (self + cross) | C | A | Same shape as Stack but the feedback lives in the pair, not a single op — noisier, more chaotic modulator content |
| 7 | **Additive Twin** | C (carrier, unmodulated pure tone) and A←B1←B2 (a full 3-op stack that outputs directly as a second carrier) sum, independently leveled | B2 | C alone | stack (A) alone | Pure tone + complex tone as two independently balanced layers — organ/bell hybrids |
| 8 | **Bell** | A→C (feedback-heavy) and B1+B2 (fixed unison pair) →C directly, both into carrier | A (deep) | C | B-pair contribution alone (A branch muted) | Inharmonic partials from the B-pair's own beating land straight on the carrier — metallic/bell default algorithm |

`feedback` (SYN1) always targets the op/pair named in the Feedback column
for the active algorithm — it is not user-routable; that is the "curated,
not a matrix" boundary ADR-043 draws (alternatives-considered §2).

### D2 — Ratio table

Stepped set for `ratio_c`, `ratio_a`, `ratio_b` (20 values — wide enough to
reach both harmonic and bell/inharmonic territory, since the user asked for
reach over safety):

```
0.25, 0.5, 0.75, 1, 1.5, 2, 2.5, 3, 3.5, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16
```

Sub-1 values are subharmonics (sub-bass/growl material on `ratio_c`);
integers give harmonic stacks; the half-steps between 1 and 4 are where
most "FM electric piano/bell" tones live; 5–16 are for bright/metallic
upper partials, mostly useful on `ratio_a`/`ratio_b` in algorithms 3, 6, 8.

**B-pair spread (D2b):** `ratio_b` sets B1's ratio from the table above;
B2's ratio = `ratio_b` + **0.05** (additive, fixed, not user-editable — ADR-043
decision 1 names one shared control with "a fixed offset spread"). 0.05 is
small enough to read as beating/chorus at low ratios and a shimmer at high
ratios, never a separate harmonic partial in its own right.

### D3 — `harm` (continuous, not a stepped waveform picker)

ADR-043 calls `harm` a "partial-shaping waveform blend on C/A" but does not
fix the mechanism. Ruled: **continuous additive harmonic stacking**, not a
fixed 4-waveform crossfade — this is what "keep it expressive" costs: no
stepped waveform means no dead zones where turning the knob does nothing.

For operator groups C and A only (B1/B2 stay pure sine — they are meant as
a modulator-color pair, and `harm` on the carrier/primary-modulator gives
the audible brightness control):

```
out(θ, harm) = normalize · [ sin(θ) + Σ_{n=2..5} harm^(n-1) · k_n · sin(nθ) ]
k_2 = 0.60, k_3 = 0.40, k_4 = 0.25, k_5 = 0.15
normalize = 1 / (1 + Σ_{n=2..5} harm^(n-1) · k_n)
```

`harm=0` → pure sine (unity gain, matches every existing engine's default
op). `harm=1` → a fixed 5-partial harmonic stack, saw-adjacent. The
`harm^(n-1)` shaping means higher partials fade in later as the knob turns
up, not all at once — a deliberately "expressive" sweep, not a snap.
`normalize` depends only on `harm`, recomputed once per sub-block (64
samples), not per-sample — no allocation, cheap trig already paid for by
the phase accumulator.

### D4 — AMP page (final 8 slots)

`attack` · `decay` · `sustain` · `release` · `volume` · `pan` · `drive` ·
`tone` (drive's post-clip tilt, matching `ParamDescriptor` naming used
elsewhere for "drive"/"tone" pairs). Driven by the new `AdsrState`
primitive (§ commit C0) — full ADSR, not the two-stage `AdState` the drum
engines use, since a melodic voice must sustain under a held key and
release cleanly on `NoteOff`.

### D5 — Macro machine set (six, plus Full)

Full (machine 0, everything) is never a "curation" — it is the union bank
with nothing hidden, always reachable regardless of which macro is active
elsewhere (ADR-041's union-bank invariant). Six macros, each a fixed algo +
ratio-table recipe with a curated 8-slot SRC page of the *most musical* deep
params for that character (per-macro SRC contents are chosen during
implementation against these anchors, not frozen numerically here — the
recipe is what's frozen):

| Machine | Algo | Recipe anchor | SRC curation bias |
|---|---|---|---|
| **Keys** | 1 (Stack) | `ratio_c=1, ratio_a=2, harm≈0.3` — clean electric-piano-adjacent stack | `harm`, `feedback`, SYN2 A-group (mod envelope), amp ADSR |
| **Bass** | 2 (Short Stack) | `ratio_c=0.5–1, ratio_a=1–2` — sub-leaning, `keytrack` biased low | `ratio_c`, `drive`, `harm`, amp attack/decay |
| **Bell** | 8 (Bell) | `ratio_b` high (8–14), fixed inharmonic B-pair | `ratio_b`, `feedback`, SYN2 lev/dec, `mix` |
| **Perc** | 4 (Branch) | short amp decay, `env_reset` = retrig | amp decay, `feedback`, `mix`, `harm` |
| **Pad** | 5 (Parallel Carriers) | slow attack, `porta` on | both branch levels (via `mix`), amp attack/release, `detune` |
| **Pluck** | 6 (Feedback Pair) | fast decay, moderate feedback | `feedback`, amp decay, `harm`, `mix` |

**C3 finding carried here:** group A's SYN2 envelope (`atk_a`/`dec_a`/`end_a`/
`lev_a`) is architecturally inert on **Additive Twin** (algo 7) and
**Parallel Carriers** (algo 5) — in both, A is a final output tap, never a
modulator, so nothing in the signal path reads `gain_a`. **Pad's anchor is
Parallel Carriers** — its SRC curation above correctly omits any group-A
control, and C7 must not add one (a knob a performer can turn with zero
audible effect is worse than no knob). If a later curation pass considers
Additive Twin for a seventh character, the same caution applies.

Every macro's non-SRC pages (SYN2, AMP, expression, MOD) stay fully live —
only the *visible curated page* narrows; nothing is clamped or hidden from
the underlying bank. This is the concrete form of "human interface level
detail potential": a performer on a macro machine still has the full
instrument one page-flip away (SRC sub-page 2, or Full).

### D6 — `env_type` tag

`FmEngine::to_rule()` already emits an undocumented `"AD"` env_type
(`fm_engine.rs:910`) alongside the documented `"ADSR"|"AHD"|"DADSR"` set
(`rule.rs:269`). `FmVoice`'s AMP group and both SYN2 mod-envelope groups
use `"ADSR"` proper (true sustain/release semantics, unlike `FmEngine`'s
`"AD"`) — this phase does not touch `FmEngine`, so the pre-existing `"AD"`
tag is left alone, not retroactively fixed (out of scope, see §1).

---

## §1. Scope

**In:**
- `AdsrState` primitive (attack/decay/sustain/release, retrig + legato
  reset modes) in `engine_dsp.rs`, alongside `AdState`.
- `NoteOff` consumption + a fixed-capacity mono note stack (max 8 held
  notes, no heap) — foundational work-package named in ADR-043's
  post-ratification note 3.
- `FmVoice` engine: 4-operator PM core, 8 algorithms (D1), SYN1/SYN2/AMP/
  expression pages, MOD page via `LfoBlock`/`LfoHost`, 7 machine variants
  (Full + 6 macros, D5).
- New node id, registry wiring, a fixture instrument exercising it, one
  ADR-035 baseline scenario.
- Doc updates: `CLAUDE.md` node table, ADR-043 implementation note,
  roadmap P14 line.

**Out:**
- Touching `FmEngine` (node 27) at all — ADR-043 decision 1 keeps it
  untouched; the two-op drum family and the four-op melodic voice are
  siblings, not a shared hierarchy.
- Paraphonic/polyphonic allocation — v1 is **monophonic per track**
  (ADR-043 decision 4); the allocator work is P13/OQ-15 scope, shared
  later, not duplicated here.
- P16 macro system (instrument-wide multi-target macros) — the six
  machines here are curation-over-union-bank, not combo params.
- Retrofitting `"AD"` → `"ADSR"` on `FmEngine` (D6).
- The tuning-by-ear half of the closing session — expected to be a
  UI/performability pass instead, given the user's stated FM unfamiliarity;
  reference patches (C9) are built from stated FM principles, not iterated
  against the user's ear.

---

## §2. Commit sequence

Per-commit invariants (unchanged from MM/TK3 convention): `cargo test
--workspace` and `cargo clippy --workspace` green against the documented
baseline after every commit; no new dependencies; no allocation/blocking/
locking in `process()`; each commit compiles and tests standalone; code and
doc commits stay separate; every commit gets a fresh-context
`code-reviewer` pass before landing (feedback: every AN1 commit's review
caught at least one real issue — do not skip this).

### P14-C0 — `AdsrState` + mono note stack (foundation)

- New `AdsrState`/`AdsrPhase` in `engine_dsp.rs`, mirroring `AdState`'s
  shape (`new()`/`trigger()`/`tick()`), phases
  `Idle→Attack→Decay→Sustain→Release→Idle`. `trigger(reset: bool)`: `reset
  = true` (retrig) restarts phase/level from zero; `reset = false` (legato)
  re-enters `Attack` from the *current* level, no snap.
- `note_off()` transitions `Sustain`→`Release` (or `Attack`/`Decay`→
  `Release` early, using current level as release start).
- Fixed-capacity mono note stack: `struct NoteStack { notes: [Option<u8>;
  8], len: u8 }`, `push`/`pop_matching(note)`/`top()` — last-note priority,
  no heap.
- Unit tests: phase transitions, legato-vs-retrig level continuity,
  bipolarity/boundary tests per `engine_dsp.rs`'s existing convention
  (MM-C8's postmortem: a naive test missed a sign-flip bug — table-driven
  per-phase assertions, not just "it ticks").
- **Tests:** `engine_dsp::tests::adsr_*`, `engine_dsp::tests::note_stack_*`.

### P14-C1 — FM operator core

- `FmOperator` primitive: phase accumulator, `tick(freq_hz, mod_in: f32,
  feedback: f32, harm: f32, sample_rate) -> f32`, implementing D3's
  additive-harmonic sine core with self-feedback (phase offset by
  `feedback * prev_out`, same technique as `FmEngine`'s `prev_mod_out`).
- Unit-tested at the operator level: pure-sine baseline (`harm=0`)
  matches `f32::sin` reference within tolerance; `harm` sweep is monotonic
  in spectral energy; feedback=0 reduces to plain PM; DC-offset check.
- **Tests:** `engine_dsp::tests::fm_operator_*`.

### P14-C2 — `FmVoice` skeleton + SYN1 + algorithm routing

- New file `crates/paraclete-nodes/src/fm_voice.rs`. `struct FmVoice` with
  4×`FmOperator`, `struct Machine { Full, Keys, Bass, Bell, Perc, Pad,
  Pluck }` (only `Full` wired this commit), SYN1 params (`algo, ratio_c,
  ratio_a, ratio_b, harm, detune, feedback, mix`) per D1/D2.
- `render_span` dispatching all 8 algorithms from D1's table; each
  algorithm a small match arm computing operator order + feedback target +
  X/Y taps, matching the amendment's normative description format.
- Anti-click on algo switch: reuses `FmEngine`'s `SwitchFade` pattern
  (5 ms fade, ADR-041 decision 4) — algo switch is a machine-*adjacent*
  discontinuity even within one machine, so it gets the same declick.
- **Tests:** baseline output non-silence + non-finite check per algorithm
  (8 cases), `harm`/`ratio` reachability at table extremes.

### P14-C3 — SYN2 page (mod envelope groups)

- Two `AdsrState`-driven groups A/B: `atk_a/dec_a/end_a/lev_a`,
  `atk_b/dec_b/end_b/lev_b` — "end level" not "sustain": these are
  modulation-index envelopes shaping brightness over the note, not gated
  amplitude (matches ADR-043 decision 3's "attack/decay-to-end-level/
  modulation-level" wording) — `AdsrState` used with `sustain = end_lev`,
  no separate release stage needed here (release comes from the AMP page
  in C4).
- Wires `lev_a`/`lev_b` as modulation-depth multipliers feeding the A/B
  operator's effective `mod_in` gain in the C2 routing.
- **Tests:** envelope shape reaches `end_lev` and holds; `lev_*=0`
  degenerates to pure carrier tone (silence-of-modulation check, not
  silence-of-output).

### P14-C4 — AMP page + `NoteOff`/mono-stack wiring (the load-bearing commit)

- Consumes `ChannelVoice2::NoteOff` (currently discarded, `fm_voice.rs`
  will match it where `FmEngine`/`AnalogEngine` both no-op today).
- `NoteOn` pushes onto the C0 note stack and triggers/legatos the AMP
  `AdsrState` per `env_reset`; `NoteOff` pops the matching note — if the
  stack still holds a note, re-trigger (or glide, if `porta` is set — full
  glide lands in C5) to the new top; if empty, calls `note_off()` on the
  AMP envelope, letting `Release` play out before the voice goes idle.
- AMP page params D4 wired: `drive`/`tone` post-op waveshaper (reuse
  `soft_clip`, `engine_dsp.rs:120`, plus a one-pole tilt for `tone`).
- **This is where C0's foundation work is first exercised end-to-end** —
  flag any NoteOff-timing surprise here, not later.
- **Tests:** held-note sustain never decays on its own; release starts
  exactly at `NoteOff`; legato glide-in note-stack pop doesn't re-trigger
  the amp envelope (level continuity assertion).

### P14-C5 — Expression page + portamento

- FUNC+PG overflow page (ADR-038): `keytrack`, `vel_mod`, `env_reset`,
  `porta`.
- Portamento: linear (or exponential — implementation's call, cheap
  either way) glide of the *carrier frequency target* over `porta` seconds
  on note-stack top-change; `porta=0` is instant (current behavior).
- **Tests:** `porta=0` reachability regression (must match C4's behavior
  exactly); glide reaches target within `porta` seconds ± one block.

**As landed:** `env_reset` governs all three envelopes (`env_a`/`env_b`/
`env_amp`), not just `env_amp` as C4's bullet above literally said
("triggers/legatos the AMP `AdsrState` per `env_reset`") — closing C4's own
punted "env_a/env_b reset policy" question inside this commit rather than
leaving it dangling. Justification: ADR-043 decision 4 names `env_reset` as
one engine-wide expression param ("retrig vs legato **envelopes**", plural),
not an AMP-page-scoped one. Recorded here per house style (spec vs. reality
divergence is recorded, not silently reconciled) since it's a real, if minor,
tension between this spec's own C4 wording and what actually shipped.

Also found and fixed in review: a second stack-pop arriving before an
in-flight glide finished restarted from the outgoing note's *nominal*
frequency rather than the pitch actually sounding mid-glide — an audible
pitch-snap on the one mechanism that exists to avoid one, and reachable
through Pad's own D5 recipe (chords released top-down faster than `porta`).
Fixed by reading the live interpolated frequency when starting a new glide.

### P14-C6 — MOD page

- `LfoBlock`/`LfoHost` integration per the established pattern
  (`fm_engine.rs:361-411`, `:498-576`): one shared `LfoHost`, dest table
  covering SYN1 (8) + SYN2 (8) + AMP (8) + expression (4) = up to 28
  destinations on the Full machine.
- Per-machine dest table narrowing for the six macros (§ D5) — the
  "PER-MACHINE table" pattern (#179), each macro's table restricted to its
  curated + always-safe set (amp, harm, feedback, mix at minimum).
- **Tests:** dest reachability per machine; LFO depth scaling sanity
  (ADR-042's amendment 2 vs body conflict resolved the same way MM-C9
  resolved it for `FmEngine` — one-based index into the *active* machine's
  table).

**As landed:** dest table is 26, not up to 28 — `algo` and `env_reset` are
excluded as topology/behavior selectors, same reasoning `FmEngine` excludes
`machine`. Review caught two real issues fixed before landing: an
uncached `format!` in `published_state()`'s `lfo_phase` publish (BUG-007's
pattern, allocating on the audio thread every cycle — fixed with a
per-node `OnceLock<String>`; the same pre-existing pattern in `FmEngine`/
`AnalogEngine` filed as #188, not fixed here) and `current_sounding_hz()`
reading `detune` unmodulated while `process_algo` reads it LFO-modulated —
an audible pitch-snap at the very first glide whenever `detune` is an LFO
destination, not just the overlapping-glide case P14-C5 fixed. Both fixed;
`current_sounding_hz()` now reads `detune` via `get_param`, matching
`process_algo`.

**OQ-P14-6** — `legato_retarget` never calls `self.lfo.trigger(...)`
(operator-phase-reset-avoidance's reasoning, extended to the LFO — a
note-stack pop's whole point is continuity). Confirmed correct and the
only defensible call, but it has a real, deliberate consequence: with
`lfo_mode = One`/`Half`, once the LFO's one-shot run completes after the
initial `NoteOn`, it never fires again on any note revealed by a later
stack-pop under the same mono voice — only a hard retrigger of a fully-idle
voice restarts it. A performer expecting each stack-revealed note to get a
fresh sweep under `One`/`Half` will not hear one. Not fixed — a genuine
trade-off, not a bug — but named so a later session's surprise isn't a
rediscovery.

### P14-C7 — Machine variants (the six macros)

- `MachineVariant` per macro (D5): `page_groups`, curated SRC `PageRef` +
  `ParamOverlay` (tuned defaults — algo/ratio recipe, not range narrowing,
  per D5's "nothing clamped" rule), `param_labels` for `algo` (readable
  names: "Stack", "Short Stack", "Y-Split", "Branch", "Parallel Carriers",
  "Feedback Pair", "Additive Twin", "Bell") so Theotokos/Theoria render
  algorithm names, not raw indices (carries forward #176's lesson from the
  MM LFO-dest review — bare indices are not performable).
- `union_params()` widest-envelope merge across all 7 machines (ADR-041
  invariant — union bank never narrows).
- **Tests:** each macro's tuned default renders non-silent output; switch
  between any two machines mid-note declicks cleanly (reuses C2's
  `SwitchFade`); Full always exposes all ~35 params regardless of which
  macro was last active (union-bank regression guard, this is the MM-C3
  hazard recurring in a new engine — test it explicitly, don't assume the
  pattern transfers).

**As landed — a real, deliberate gap against §6 exit criterion 4's literal
wording, confirmed correct against precedent, carried here rather than
silently reconciled:** a live `machine` switch does **not** rewrite any
bank param, matching `FmEngine::apply_machine_switch`'s own established
behavior exactly — ADR-041 decision 2's "switching away and back loses
nothing" governs *preserving edited values*, not auto-applying a target
machine's tuned defaults. Consequence: switching to a macro does not make
it sound like that macro's D5 recipe (and does not even change `algo` to
the macro's anchored algorithm) until a performer dials in the values by
hand or a fixture/patch sets `machine` and the underlying params together
— which is exactly what P14-C9's reference-patch scenarios do. So exit
criterion 4 ("tuned, audibly-distinct defaults") is not satisfied by C7
alone; it is satisfied once C9 lands. Not a bug, not fixed here — recorded
so C9 isn't mistaken for optional polish on top of an already-complete
C7.

**Also ruled in C7 review:** `algo`'s `ParamOverlay` is **not** `identity`
(unlike `machine`) — a first draft marked it identity by analogy to
`machine`'s "topology, not a setting" reasoning, but that removes p-lock
and scene-morph capability from `algo` with no ADR or spec text actually
calling for it, and directly against the phase's own delegated mandate to
keep the instrument expressive. `AlgoSwitchFade` already declicks whatever
drives an algo change, p-lock included, so there's no technical reason
(unlike `machine`, which ADR-041 amendment 4 says cannot declick inside a
step) to forbid it.

### P14-C8 — Node registration + fixture

- New node id **28**, single type_tag `"fm_voice"` (defaulting to machine
  `Full`) — a deliberate deviation from `FmEngine`'s per-machine type_tag
  convention (`"fm_engine:kick|bell|bass"`): `FmVoice` is one melodic voice
  per track, not one-instance-per-drum-type, so a single tag with runtime
  machine switching (already how ADR-041 variants work) is the natural fit
  — no need to proliferate `"fm_voice:keys"`/`"fm_voice:bass"`/... tags for
  what the `machine` param already switches at runtime.
- Registry wiring: `registry.rs` constructor map, `builder.rs`
  construction match + `classify_node`'s generator classification,
  `main.rs`'s `engine_type_tags` list (all three sites named in the
  research handoff).
- New fixture — either a new track in `instrument-fx.yaml` or a dedicated
  `instrument-fm.yaml` (implementation's call which reads better); must not
  disturb the four shipped tracks' node ids.
- **Tests:** node loads via registry, `cargo run -p gen-samples`
  (pre-flight) unaffected, project save/load round-trips node 28's state
  (serialize/deserialize per `ParameterBank`, #154's lesson — verify
  engine params are *not* lost on save, the way MM's #154 bug did).

### P14-C9 — Baseline scenario + reference patches

- `tools/test-driver/tests/fm_voice.yaml` + `.baseline.json`, following
  `fm_machines.yaml`'s template: timeline exercising each of the 8
  algorithms, at least 2 machine switches (including mid-note, to catch a
  `SwitchFade` regression), a `porta` glide, MOD dest sweep.
- One reference patch per macro machine (6 total) as `set_param` timelines
  — audibly distinct, built from D5's recipe anchors and D1/D3's stated
  math (not tuned by ear, per §1 scope — this phase's "reference patches"
  are functional/audible-non-silence proofs, not a sound-designed bank).
- **Tests:** `test-driver fm_voice.yaml --check-baseline` green; RMS
  envelope confirms real energy per scenario segment (guard against the
  `fm_machines.yaml` silent-pass failure mode named in its own header).

### P14-C10 — Docs

- `CLAUDE.md` node-ID table: add `28 | FmVoice (id_for_name? no — plain
  node id)`.
- ADR-043 implementation note (template already in the ADR file):
  `ADR-043 implemented across P14 (<date>). See design/phases/p14-*.md.`
- Roadmap P14 row: mark implementation complete, name the outstanding
  gate (playability/UI session, not tuning-by-ear — per this spec's
  framing).

---

## §3. What must not regress

1. `FmEngine` (node 27) — untouched. Any diff touching `fm_engine.rs` in
   this phase's commits is a scope violation unless it's a shared-primitive
   extraction explicitly called out (e.g. if `AdState` and the new
   `AdsrState` end up sharing a phase-transition helper — fine; changing
   `FmEngine`'s envelope *behavior* — not fine).
2. Union-bank invariant (ADR-041): switching machines never narrows what
   `Full` can reach. Test this explicitly per C7, not by inference from MM.
3. `process()` hard constraints: no allocation anywhere in the new engine,
   including the note stack (fixed `[Option<u8>; 8]`, never a `Vec`) and
   the `harm` normalization (recomputed per sub-block, not per-sample, but
   still no heap either way).
4. Existing four tracks' node ids (10–13, 20–22, 27, 60) — the new fixture
   must not renumber or reuse any of them.
5. `porta=0` must reproduce pre-portamento (instant-retune) behavior
   exactly — C5's regression guard.

---

## §4. Open questions carried

- **OQ-P14-1** — Should `FmVoice`'s `harm` additive-stack technique replace
  `FmEngine`'s plain-sine ops in a *future* phase (shared brightness
  control across both FM families)? Explicitly out of scope here (§1); a
  spec-gap ticket if it resurfaces.
- **OQ-P14-2** — Paraphonic allocation (OQ-15) will eventually want the
  mono note stack generalized to N voices; C0's fixed-capacity design
  should be revisited then, not preemptively generalized now (no
  speculative depth, per house style).
- **OQ-P14-3** — The six macro machines' SRC curations (D5) are anchors,
  not frozen slot-by-slot layouts; if a session later finds a curation
  unplayable, that's a defect against this phase, not a spec violation.
- **OQ-P14-5** — D1's `ShortStack` and `YSplit` rows both reduce to the same
  linear system with `FmOperator`'s single scalar `mod_in` (both compute
  `pair = (b1+b2)*0.5` fed into `A`, differing only in which signal becomes
  the Y tap) — at `mix=0.0` (the default), the two algorithms are audibly
  identical, only diverging once `mix > 0` exposes the differing Y taps.
  Found in the C2 review. Not fixed here (would mean giving Y-Split a
  mechanism Short Stack doesn't have — a routing-table change, out of C2's
  scope); if a later session finds these two indistinguishable in practice,
  that's the defect this note predicts, not a surprise.
- **OQ-P14-4** — D3's `harm` normalization is peak-safe (bounded by the
  triangle inequality) but not loudness-preserving: verified in the C1
  review that total output power falls from ~1.0 at `harm=0` to ~0.28 at
  `harm=1`. Several macros (D5) put `harm` on the curated SRC page as a
  headline "brightness" control, so turning it up will read as "brighter
  but quieter" unless something compensates. Decide in C2/C4 whether AMP
  gain should compensate for `harm`, or whether "brighter and a little
  quieter" is accepted as the tone (a real trade-off some additive designs
  make deliberately) — not silently discovered at the playability session.

---

## §5. Forward constraints on work this phase cannot do

- P16 (macro system) must not duplicate the six `FmVoice` machines as
  macro-system targets — they are curation-over-union-bank, a different
  mechanism, and should compose with P16 rather than be reimplemented by
  it.
- P13's allocator (OQ-15), when it lands, should be able to *host*
  `FmVoice` as a paraphonic voice without changing SYN1/SYN2/AMP param
  semantics — the mono note stack is an implementation detail this phase
  owns, not a contract P13 needs to preserve bit-for-bit.

---

## §6. Exit criteria

1. `cargo test --workspace` and `cargo clippy --workspace` green at the
   documented baseline, all ten commits landed.
2. `tools/test-driver/tests/fm_voice.yaml --check-baseline` green.
3. All 8 algorithms (D1), the full ratio table (D2), and the `harm` sweep
   (D3) reachable and non-silent via machine `Full`.
4. All 6 macro machines (D5) load with tuned, audibly-distinct defaults;
   machine switching (including mid-note) declicks cleanly.
5. `NoteOff` is consumed; sustain holds under a held key; release plays out
   after `NoteOff`; legato (`env_reset=false`) does not re-trigger the AMP
   envelope from zero.
6. Project save/load round-trips node 28's full state (no #154-style loss).
7. `CLAUDE.md`, ADR-043, and the roadmap P14 row updated.
8. Playability/UI session held (or explicitly deferred by the user) before
   the milestone closes — this phase's version of the roadmap's "session"
   gate.
