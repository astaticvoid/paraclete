# W2 Theoria Native Surface — Reference Analysis (working draft)

> **Status:** GROUNDWORK COMPLETE (2026-07-12). Agent-authored reference analysis
> to front-load the paired W2 spike. **Not a spec.** Design decisions are reserved
> for the user (`handoff.md`: W2 surface + ADR-032 are user-owned, do not
> improvise) — §6 and the P13 appendix are decision menus, not decisions. All
> four `ref/` manuals read (TOC-targeted); sections 1–6 + P13 brief filled.
>
> **Method:** TOC-targeted reads of the four `ref/` manuals (~120 pp each),
> extracting interface structure only — page layout, encoder/param assignment,
> page navigation, contextual display, envelopes/LFOs, FX/routing, sequencer.
> Lens per roadmap: Digitakt II = sample workflow; Syntakt = analog machine
> param pages; Digitone = FM page discipline; Hydrasynth = source→FX signal
> chain. Common views (env/LFO/FX/seq) drawn from whichever does them best.

## What W2 must produce (from roadmap/interface-plan)
- **Fixed input rail + contextual window** (session #2 F1/F7 keystone).
- Cap-doc-driven **parameter pages for every engine**.
- **Chain view** (source → FX routing).
- **View-plugin API (ADR-032)** — the extension contract.
- Envelope, LFO, effects, and sequencer views usable across engines.

---

## 1. Digitakt II — sample workflow + the Elektron page model
_lens: how a track's params are grouped into pages; step/param-lock model; sample assignment_
_source: §6 Interacting, §11 Track Parameters (pp.53–60), TOC_

**The page model (this is the keystone pattern for W2).**
- A track's parameters are split across **six named PARAMETER pages**, each on a
  dedicated hardware key: **TRIG · SRC · FLTR · AMP · FX · MOD**. Fixed set,
  same six across every track — a *stable spatial vocabulary* the player learns
  once.
- Each page presents **exactly 8 parameters** on a fixed **2×4 grid** mapped to
  8 DATA ENTRY knobs (rows A–D / E–H). Every cell = **icon + short label +
  value**. This is the "fixed input rail + contextual window" in physical form:
  the 8 encoders never move; the screen re-labels what they address per page.
- **Sub-pages** within a group: pressing a page key repeatedly (or [UP]/[DOWN])
  cycles FLTR 1/2, MOD 1/2/3 (=LFO1/2/3). Press-and-hold a page key = show all
  values at once. So depth is handled by *paging*, never by a knob-per-param.
- **Graphical affordances, not just numbers:** AMP page draws the envelope curve;
  FLTR page draws the filter shape; SRC page 2 shows the sample waveform; MOD
  shows the LFO shape. The contextual window is *semantic*, not a number list.
- Content is **machine-dependent**: SRC and FLTR page contents change with the
  selected machine (see §A machines) — the same page key, different params. This
  is the cap-doc-driven analogue: the *frame* is fixed, the *fields* come from
  the active engine/machine.

**Parameter locks (the reason the page model matters).** Hold a [TRIG] step key
+ turn any DATA ENTRY knob → that parameter is **locked to that step** (per-step
automation). Because every param lives at a fixed encoder on a fixed page, a
p-lock is a single gesture on the same surface used to set the base value.
[PARAM]+[YES] randomizes a page; [PARAM]+[NO] reloads it to last-saved. (Paraclete
already has ParamLock plumbing — AGENTS.md — so this maps directly.)

**The six pages, concretely:**
| Page | Holds | Notes for Paraclete |
|---|---|---|
| **TRIG** (1/2) | NOTE, VEL, LEN, PROB, LFO.T, FLT.T, FILL, COND / retrig RTRG,VFAD,LEN,RATE,PTIM,PORT | trig-level + conditional; saved with *pattern* not preset. Maps to our Sequencer step/condition/micro-timing model |
| **SRC** (1/2) | machine-dependent sample params; pg2 = waveform view | our Sampler/engine source params, cap-doc-driven |
| **FLTR** (1/2) | filter/EQ machine params / base-width HP+LP, env delay, keytrack | our FilterNode/LadderFilter |
| **AMP** | ATK,HOLD,DEC,SUS,REL,RSET,MODE(AHD/ADSR),PAN,VOL + env curve drawn | generic envelope view |
| **FX** | BR, OVER, SRR + DEL/REV/CHR **sends** + pre/post routing | per-track insert FX + sends = our source→FX story |
| **MOD** (1/2/3) | 3× LFO: SPD,MULT,FADE,DEST,WAVE,SPH,MODE,DEP + LFO shape drawn | generic LFO view; DEST = mod destination picker |

**W2 takeaways (candidate patterns, user to ratify):**
1. A **fixed, named page set** on the rail beats a flat scrolling param list — it
   gives the player a spatial map. Open Q 6.3: do *we* fix the page names
   (TRIG/SRC/FLTR/AMP/FX/MOD-like) or let each engine's cap-doc declare its own
   page groups? Elektron fixes them; that consistency is the whole point.
2. **8 params per page, 2×4, icon+label+value** is a proven density for a small
   contextual window. Our touch window can hold more, but the *grouping into
   ~8-param pages* is the legibility unit.
3. **Graphical affordance per page type** (env curve, filter shape, LFO shape,
   waveform) is what separated "Elektron" from "Behringer" in session #1 — the
   window must draw the thing, not list numbers.
4. **p-lock = same gesture as edit, on the same surface.** Whatever the touch
   equivalent of "hold step + adjust param" is, it should reuse the exact page/
   param widgets (feeds W3 hold-step overlay).

## 2. Syntakt — analog/synthesis machine-per-track param pages
_lens: machine-as-parameter; how a machine's controls map to pages; analog FX block_
_source: §5 Sound Architecture (pp.17–18), §10/§12 TOC_

**Same page family, synthesis slot.** Syntakt uses the identical page grammar as
Digitakt II — TRIG 1/2, **SYN**, FLTR 1/2, AMP 1/2, LFO 1/2 — but the SRC slot is
a **SYN (synthesis) page** holding the voice/machine params instead of sample
params. Confirms the pattern: **the page *frame* is fixed and shared; the engine
fills the fields.** Exactly Paraclete's cap-doc-driven intent.

**Voice type + machine model (relevant to OQ-14, P13).** A track has a *voice
type* (digital, or analog-drum / analog-cymbal) and, within it, a selectable
*machine*. The per-voice signal chain is a **fixed topology** the pages edit:
`Sound Generator → Overdrive → [Base-width Filter] → Multimode Filter (+Filter
Env) → AMP (+Amp Env) → Pan → Voice Output → Effect Sends`. Digital vs analog
voice types differ only in the generator block and whether the base-width filter
is present — i.e. **one page vocabulary spans multiple engine families.** This is
the machine-family-per-track destination in the roadmap's Known-Provisional table.

**Effects use the SAME page grammar (strong universality signal).** The analog FX
block is exposed as its own **FX track** with TRIG/SYN/FLTR/AMP/LFO pages — you
edit an effect with the identical grammar you edit a voice. For W2 this argues:
**do not invent a separate "effect editor" — effects are engines, edited through
the same param-page view.** (Matches ADR-018 "every component is a Node.")

**The source→FX routing view (direct answer to W2 §6.5).** `[FUNC]+[FX]` opens
**ANALOG FX BLOCK ROUTING**: a single screen showing every track (1–12) plus the
send effects and external-in as a **grid of cells**, each toggled to route
*through* the FX block (bright) or *direct to main* (dim). The SAME routing is
also settable per-track via a `ROUT` parameter on AMP PAGE 2. Two coordinated
surfaces for one fact — a **global matrix view** for overview + a **per-track
param** for in-context edits. Send FX are a short serial chain (Delay → Reverb)
with returns.

**W2 takeaways (candidate patterns, user to ratify):**
1. **SYN = SRC slot** confirms a fixed page set works across sample *and* synth
   engines. Strengthens the case for Paraclete fixing the page names.
2. **Effects are edited as engines** through the same page view — one grammar,
   no bespoke FX UI. (Feeds ADR-032: a "param-page view" plugin serves voices
   and effects alike.)
3. **Chain view = a routing grid** (sources × destinations, cells toggle
   routing), mirrored by a per-source routing param. This is the concrete W2
   chain-view candidate. Paraclete's additive MixNode-dry + Reverb-wet send model
   maps onto it directly.
4. **Fixed per-voice topology** (gen→drive→filter→amp→pan→sends) is what makes
   the pages predictable. P13 should define its analog voice's fixed topology up
   front so the pages fall out of it (informs OQ-13).

## 3. Digitone II — FM page discipline
_lens: how a 4-op FM voice is made legible as pages (operators, ratios, envelopes) without a knob-per-parameter_
_source: Appendix B FM Tone Synthesis (pp.106–111), §11 TOC_

**The problem Digitone solves is Paraclete's problem.** A 4-operator FM voice has
a huge raw param count (4 ops × ratio/level/envelope + feedback + harmonics +
algorithm + mix). Exposing that raw would be a knob-per-param nightmare — the
"Behringer, not Elektron" failure. Digitone's answer is a set of **disciplines**
that are directly the answer to W2's page-grouping question and to P14.

**Discipline 1 — macro-group to collapse count.** The four operators are grouped
into **three named groups: C (carrier), A, B**, where **B = B1+B2 controlled by a
single macro** ("the parameter controls for B are macro mapped to both
operators"; envelope B controls both). Four operators → three controllable
groups, one of them 2-for-1. SYN Page 1 is then just **7 params: Ratio C, Ratio
A, Ratio B, Harm, Detune, Feedback, Mix** — fits one page.

**Discipline 2 — diagram-per-parameter.** Every SYN-page param is shown with a
**small operator-graph diagram that greys-in exactly the region it affects**
(B.7). The window doesn't say "Ratio B = 2" — it *shows which operators that
touches*. This teaches the engine's structure while you edit it; it is the
highest form of the "draw the thing, don't list numbers" affordance.

**Discipline 3 — one expressive macro over a complex process.** `HARM` is a
single bipolar knob that sweeps the operators through an **additive harmonic
series** (sine → saw → square → bell), smoothly interpolating between timbres —
huge range, one control. Expose *expressive macros*, not the raw guts.

**Discipline 4 — wrap every engine in the SAME subtractive downstream.** By
design (B.1) the FM engine is "a complex tone generator," then shaped by the
*same* fixed topology as every other voice: `FM → Overdrive → Base-width Filter →
Multimode Filter → Amp`. So the **SYN page is the only FM-specific surface**;
FLTR / AMP / FX / MOD pages are the shared universal vocabulary. Only the
source-slot page changes per engine family.

**Envelopes:** two operator envelopes (group A, group B), not four — the grouping
again halves the surface. Expanded AD with adjustable end-level; trig vs gate.

**W2 / P14 takeaways (candidate patterns, user to ratify):**
1. **Cap-doc must express macros, not only raw params** (§6.3). The page-grouping
   API needs a way for an engine to declare grouped/macro-mapped controls (B =
   two operators, one knob). Without this, deep engines can't be legible. This is
   a concrete input to **ADR-032**.
2. **A view plugin should be able to ship an engine diagram with highlight
   regions** (Discipline 2). ADR-032 view contract candidate: `{param → diagram
   region}` binding, not just `{param → widget}`.
3. **Fix the downstream topology; vary only the source page.** Reinforces §2's
   lesson — Paraclete's FLTR/AMP/FX/MOD views are universal; each engine only
   supplies its SYN/SRC page. Big win for the cap-doc-driven approach.
4. **P14 (and P13) should be "macro-first"** (already the roadmap word): design
   the ~7-param expressive front *before* the raw engine, so the page falls out.

## 4. Hydrasynth — source→effect signal chain view
_lens: signal routing / mod matrix presentation; how a voice's graph is shown and edited_
_source: Contents, Overview, The Mixer Module (pp.43–44), module/shortcut TOC_

**A different organizing axis — by module, not by track.** Hydrasynth navigates
with **Module Select** buttons ([OSC], [MIXER], [FILTER], [AMP], [ENV], [LFO],
[FX], [MOD MATRIX]) — press a module and the fixed knobs + screen address that
module; **PAGE Up/Down** cycles pages within it (Mixer = 3 pages). Same fixed-
knob / paged-screen mechanic as Elektron, but organized by **signal-flow module**
(spanning the whole voice) rather than by **track**. This is the crucial contrast:
Paraclete is a **node graph**, structurally closer to Hydrasynth's module model
than to Elektron's track model.

**Routing as a continuous blend param, not a patch-matrix cell.** Each source
(Osc 1–3, Ring, Noise) has a **`Filter Ratio 100:0 – 0:100`** knob: 100:0 = only
Filter 1, 0:100 = only Filter 2, intermediate = *both in varying amounts*. Plus a
global **Filter Configuration: Series / Parallel** topology toggle. So the
source→destination graph is edited as **per-source continuous send knobs + one
topology switch** — no abstract cable-patching. This is dramatically more touch-
friendly than a grid of on/off cells and maps *exactly* onto Paraclete's additive
mix model (per-source send amount to each destination bus).

**Mod routing has an in-context AND a global surface.** You can **"Create a Mod
route" directly from a source module** (Env/LFO shortcuts) OR build it in the
global **MOD MATRIX**. Same global-view + in-context-edit duality seen in
Syntakt's FX routing — a robust, repeated pattern worth adopting.

**Other transferable bits:** per-source **Solo** (isolate while editing; also a
P11 Perform feature); **MACRO ASSIGN** = a hardware macro knob fanned out to
destinations (the macro-first philosophy from the control side).

**W2 takeaways (candidate patterns, user to ratify):**
1. **THE central W2 navigation decision:** organize Theoria **by track/instrument
   (Elektron)** or **by node/module (Hydrasynth)**? Paraclete's graph can express
   either as a *view*; W2 must pick the default. Elektron's track model won
   session #1/#2's "legibility" framing (a player thinks in tracks); Hydrasynth's
   module model matches the engine's actual structure and the chain-view story.
   **A hybrid is plausible** — track-first for playing, a module/graph view for
   patching — but that multiplies view work. Reserved for the user (§6.1/6.7).
2. **Chain view = per-source continuous send knobs + topology toggle**, not a
   patch grid. Best-fit for touch and for Paraclete's additive send model.
   Combine with Syntakt's per-track `ROUT` mirror: global chain view + in-context
   send param. This is the concrete, cross-validated W2 chain-view recommendation.
3. **Offer both in-context and global routing/mod editing** — create-from-source
   and a global matrix. Cross-validated across two references.
4. **ADR-032 should support macro controls that fan out to multiple
   destinations** (MACRO ASSIGN + Digitone HARM + Hydrasynth macros all point
   here).

---

## 5. Comparative synthesis

**Convergent patterns (multiple references agree → strong W2 candidates):**

| # | Pattern | Seen in | Paraclete implication |
|---|---|---|---|
| C1 | **Fixed input surface + contextual paged window** — knobs never move, screen re-labels per page/module | all 4 | *This is the W2 keystone, validated 4/4.* Fixed rail + contextual window is not a preference, it's the industry consensus |
| C2 | **Depth via paging, never knob-per-param** | all 4 | ~8 params per page is the legibility unit; sub-pages for depth |
| C3 | **Graphical affordance in the window** (env curves, filter shapes, LFO shapes, FM operator diagrams, waveforms) | Elektron + Digitone | the window must *draw the thing*; this is the "Elektron not Behringer" delta from session #1 |
| C4 | **Fixed downstream topology; only the source page varies per engine** | Elektron family | cap-doc supplies the SRC/SYN page; FLTR/AMP/FX/MOD are universal views. Huge reuse win |
| C5 | **Effects edited as engines, same grammar** | Syntakt FX track | no bespoke FX editor; one param-page view serves voices + effects (fits ADR-018) |
| C6 | **Global routing/mod view + in-context per-source edit** (duality) | Syntakt routing, Hydrasynth mod matrix | offer both a chain/matrix overview and a per-source send/route param |
| C7 | **Macros over complex internals** | Digitone HARM, Hydrasynth MACRO ASSIGN | cap-doc must express macro controls, not only raw params |
| C8 | **Step/param-lock reuses the same param widgets** | Elektron | W3 hold-step overlay reuses W2 page widgets; ParamLock plumbing already exists |

**The genuine divergence (the real design fork for the user):**
- **Organizing axis — track vs module.** Elektron organizes the surface **by
  track** (each track carries every page); Hydrasynth organizes **by signal-flow
  module** (each module spans the voice). Paraclete is a **node graph** and can
  express either as a view. Elektron's track model matches how a player *thinks*
  and won the legibility framing; Hydrasynth's module model matches the engine's
  *actual structure* and the chain-view. **A hybrid** (track-first for play, a
  graph/module view for patching) is possible but multiplies view work.
- **Routing representation — discrete cells vs continuous blend.** Syntakt uses a
  toggle grid; Hydrasynth uses per-source continuous blend knobs (100:0–0:100) +
  a series/parallel topology switch. The continuous model is more touch-friendly
  and maps onto Paraclete's additive send model — **recommended**.

**What this asks of the cap-doc / ADR-032 view API.** For the "engine supplies
the fields, the frame is fixed" model to work, an engine's capability document
must be able to declare, per parameter: (a) **page/group membership**, (b)
**macro mapping** (one control → several internal params), (c) a **graphical-
affordance hint** (envelope / filter-shape / lfo-shape / engine-diagram-region /
waveform / plain value), and (d) **routing/send** semantics. These four are the
concrete requirements the reference spike surfaces for **ADR-032**.

---

## 6. W2 spec skeleton — decisions reserved for the user
_each item is a question, NOT an answer. References inform; the user decides in
the paired session. Ordered by how much they gate everything downstream._

### 6.0 ⭐ Organizing axis (decide FIRST — gates all of §6)
- **Question:** Does Theoria organize by **track/instrument** (Elektron), by
  **node/module** (Hydrasynth), or a **hybrid** (track-first play + module/graph
  patch view)?
- **References:** track model = 3/4 (all Elektron), won session #1/#2 legibility;
  module model = Hydrasynth, matches Paraclete's node graph + chain view.
- **Why first:** every other item (rail contents, window modes, chain view)
  inherits from this. Hybrid multiplies view work — scope tradeoff.

### 6.1 Fixed input rail
- **Question:** What lives on the always-present rail — transport, track/module
  select, page-nav (the [TRIG/SRC/FLTR/AMP/FX/MOD]-equivalent keys), master
  encoders?
- **Reference lean:** a **fixed, named page set** on the rail (C1/C2). Name them?
  (Elektron: TRIG/SRC/FLTR/AMP/FX/MOD — do we fix analogous names or let cap-doc
  name pages? Consistency is the whole value.)

### 6.2 Contextual window
- **Question:** What does the window show per mode (grid edit / param page /
  chain / sequencer)? What's the param density (Elektron's 8/page, or more)?
- **Reference lean:** paged param grid with **graphical affordances** (C3), not a
  number list.

### 6.3 Parameter pages (cap-doc-driven)
- **Question:** Page grouping rule — engine-declared groups, or a fixed semantic
  set the engine slots into? Does the cap-doc declare **macros** (C7) and
  **affordance hints** (C3)?
- **Reference lean:** fixed downstream pages (FLTR/AMP/FX/MOD), engine supplies
  only the source page (C4). Requires cap-doc extensions (see §5 a–d).

### 6.4 Envelope / LFO / FX views
- **Question:** One generic view per type (shared across engines), or engine-
  customizable?
- **Reference lean:** **generic views** — env/LFO/filter are universal across all
  4 boxes; only the source page is engine-specific (C4).

### 6.5 Chain / source→FX view
- **Question:** How is routing shown and edited on touch?
- **Reference lean:** **per-source continuous send knobs + a topology toggle**
  (Hydrasynth), mirrored by a per-track `ROUT`-style param (Syntakt) — a global
  chain view + in-context edit (C6). Maps onto Paraclete's additive send model.

### 6.6 ADR-032 view-plugin API (the extension freeze — user-owned)
- **Question:** What can a view plugin declare/bind? Minimum from this spike
  (§5): `{param → page/group}`, `{macro → internal params}`, `{param →
  affordance hint}`, `{param → routing/send semantics}`, and `{param → diagram
  region}` for engine-topology views (Digitone B.7).
- **Guardrail:** `handoff.md` — do not freeze without the user.

### 6.7 Scope for paired session #2
- **Question:** Full editor (every engine's pages + chain + view-plugin API), or
  a narrower first cut (e.g. one engine's pages end-to-end) to reach session #2
  sooner?
- **Reference lean:** C4 means one engine's pages proves most of the machinery
  (downstream views are shared) — a narrow vertical slice is high-leverage.

---

## Appendix: P13 analog-voice decision brief
_Deliverable D2. Two decisions the P13 spec must resolve up front (they shape the
mod-matrix API and CLAP export). Recommendations are for the user to ratify, not
decisions. See roadmap OQ-13/OQ-14; ADR-019 (stepped param), ADR-022 (machine
pattern), ADR-023 (GraphNode), `instrument-vision.md`._

**The voice to build (P13 target, for grounding).** Pro-One primary reference:
dual osc + **hard sync** + **poly-mod routing**, self-oscillating 4-pole ladder
(ZDF is C1 per audio-model review), two envelopes, glide, arp; Model D / MS-20 as
character references. Paraphonic allocation, **per-voice-expression-aware**
(OQ-15). The two decisions below determine *how* this is assembled.

### OQ-13 — composed-from-primitives vs monolithic `AnalogVoice` (decide first)

| | **A. Composed from primitives** (GraphNode / inner graph) | **B. Monolithic `AnalogVoice` engine** (like AnalogEngine) |
|---|---|---|
| Basis | `instrument-vision.md` ("composed from Paraclete primitives"); ADR-023 GraphNode | ADR-022 machine pattern; `SubgraphPlugin` |
| Hard sync / poly-mod | **already expressible** — Phase-port hard sync, topo-order feedforward mod (per OQ-13 note) | hand-coded inside the engine |
| Mod-matrix API | *is* the graph — routing = edges; the mod matrix is the node-graph patching surface (Hydrasynth-like) | needs a bespoke internal mod-matrix param scheme |
| CLAP export | needs subgraph→plugin bundling; heavier | clean — one engine = one machine bank `.clap` |
| Perf / audio-thread | more nodes, more per-cycle overhead; inner-graph patching cost | tight, single-node, cache-friendly |
| Dev cost | reuses existing primitives; new = the wiring/patching UX | new DSP, but self-contained |
| Polyphony (P13 allocator) | voice = a subgraph instance; allocator clones subgraphs | voice = engine instance; simpler pooling |

- **Reference signal:** the C4 pattern (fixed downstream topology, variable
  source) points to a **hybrid**: a *fixed voice scaffold* (osc → filter → amp →
  2 env → LFO in a known topology) with only the character/routing variable —
  buildable either as a curated inner-graph template (A) or a monolithic engine
  with a fixed internal chain (B). Syntakt/Digitone both take **B** (monolithic
  machine, fixed internal topology, exposed via pages + a few macros) and get
  legibility for free.
- **Recommendation to weigh:** **B (monolithic `AnalogVoice`) for the shipping
  P13 voice**, because (i) CLAP export is a P13 goal and B keeps it clean, (ii)
  the page/macro surface (W2) wants a fixed, declarable param set, which a
  monolithic engine's cap-doc gives directly, (iii) audio-thread simplicity for a
  polyphonic allocator. Keep **A available** as the *experimental/patchable* path
  (GraphNode already exists) — but don't gate the flagship voice on inner-graph
  patching UX. **This is a judgment call reserved for the user** — the counter-
  argument (A makes the mod matrix = the graph, unifying with the W2 node view
  from §6.0) is real and ties directly to the W2 navigation-axis decision.
- **Coupling to note:** OQ-13 and §6.0 (W2 axis) are linked. If Theoria goes
  **module/graph-view** (§6.0 Hydrasynth path), option **A** unifies the voice
  internals with the same graph view — one patching surface for the whole system.
  If Theoria goes **track/page** (Elektron path), option **B** fits better. *Ask
  these two together.*

### OQ-14 — drum machine selection: stepped parameter vs `type_tag` swap

| | **A. Machine-as-parameter** (ADR-019 stepped param) | **B. Machine-as-type-tag** (`analog_engine:kick_hard`, apply_patch swap) |
|---|---|---|
| Live switching | **yes** — no topology change, kit-savable, sequenceable/p-lockable | pause-rebuild-resume on every swap (audible gap; the very churn the load test stressed) |
| Save/recall | one param in the kit | a topology edit in the project |
| DSP shape | one engine holds all machine variants; selects internally | one node per machine variant; registry-built |
| Cap-doc / pages | machine = a stepped param on the SRC/SYN page; params below it re-map | each type_tag = its own cap-doc |
| Cost | engine carries every variant's code (bigger node) | smaller nodes, but swap machinery + gap |

- **Reference signal — decisive:** Syntakt's model is exactly **A** — a track has
  a voice *type* and a selectable *machine within it*, switchable live with no
  topology change, and the downstream pages stay fixed. This is the proven live-
  performance model and matches the roadmap's stated default hypothesis
  ("parameter path is the live-performance-friendly default").
- **Recommendation to weigh:** **A (machine-as-parameter)** for drum/machine-
  family selection. It's live-switchable, p-lockable (imagine sequencing a kick
  variant per step), kit-savable, and avoids the apply_patch gap. B's only real
  win (smaller nodes) doesn't justify an audible swap on a performance gesture.
  Reserve **B** for genuinely different *voice types* (analog vs FM vs sample),
  where a topology change is expected and rare. **User to ratify.**
- **Consistency:** this mirrors C4/C5 (fixed frame, variable source; machine is a
  field, not a new frame) — the same principle as the whole W2 analysis.

### What P13 spec still needs from the user beyond OQ-13/14
- Voice **feature set freeze** (which Pro-One features are C0 vs later): dual-osc
  + sync + poly-mod are identity; arp/glide/paraphony scheduling.
- **Voice-allocator** expression-awareness policy (OQ-15 already sets direction:
  per-voice pressure/pitch/timbre from day one).
- ZDF ladder as C1 (audio-model review) — confirm.
