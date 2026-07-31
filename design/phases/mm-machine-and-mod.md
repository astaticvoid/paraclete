# Paraclete — MM Specification (Machine Identity & the MOD Page)

**Phase tag:** `MM` · **Milestone:** "ADR-041 + ADR-042 implementation"
**Design authority:** ADR-041 (machine identity), ADR-042 (MOD page).
Both ratified 2026-07-23 and **amended by their own post-ratification
hostile reviews** — where an amendment and the ADR body disagree, the
amendment wins (ADR-041 §"Post-ratification", ADR-042 §"Post-ratification").
**Also binding:** ADR-018 (cellular — no node commands another),
ADR-019 (param plane), ADR-032 (view contract), ADR-038 (page keys),
ADR-035 (regression baselines — the tool that makes MM-C7 verifiable).

**Chosen as track 4c of the TK2-exit scheduling pass** (user, 2026-07-30)
because it was the only one of the three with no further design gate.
P14 (FM Voice, ADR-043) depends on both ADRs landing.

---

## §0. What this spec decides that the ADRs deferred

Both ADRs explicitly punt several things to "the implementing phase". They
are decided here. Per `AGENTS.md` guardrail 1, where a spec is silent the
boring option is taken and named as such — each of these is a place a
reviewer should push back if the boring option is wrong.

**D1 — Machine-switch declick is a 5 ms fade-out, then swap. There is no
fade-in.** *(amended by MM-C3's implementation — see below)*

ADR-041 decision 4 says "~5 ms *(tunable)*" without saying between what.
A true crossfade would require rendering both machines simultaneously
through the switch, which means two live voice states and double the DSP
for the duration. But decision 4 *also* says voice state resets on switch —
so there is no "old voice" worth crossfading to. `MACHINE_SWITCH_FADE_SECS
= 0.005`, ramping the voice to silence before the swap.

*As written this said 2.5 ms out + 2.5 ms in. The fade-in is provably a
no-op:* `apply_machine_switch` clears `active`, `render_span` early-returns
while inactive, and the render buffers are zeroed at the top of every block —
so the node emits exact zeros from the swap until the next `retrigger()`,
which starts from zero anyway. There is nothing to fade in.

**But do not read that as "the ramp is one-directional".** The fade-in half
was covering something real, just not post-switch silence: a switch that is
**cancelled or retargeted part-way** must return to unity *continuously*.
Dropping the ramp on cancel snaps the gain from wherever it had reached back
to 1.0 — the same click, in the other direction — and taking a fresh
full-length fade on retarget does likewise. So `SwitchFade` carries a
direction, and cancel/retarget preserve the elapsed portion.

*Reachability of that second case, since it looks theoretical:* it needs the
fade to span more than one block. At 44.1 kHz a 5 ms fade is 220 samples
against a fixed 512-sample block, so it cannot — but it can at 176.4/192 kHz
(cpal takes the device default rate; it is not pinned), and at **any** rate
the moment the constant is raised, which decision 4's "*(tunable)*" and this
very section invite. The guard tests run at 192 kHz for exactly that reason.

**D2 — The 64-sample sub-block loop nests INSIDE event-split spans, and
its boundaries are relative to the span start.** ADR-042 amendment 4 names
the sub-block loop as "new required engine structure" and defers its
interaction with event-split spans to this phase.

Today both engines funnel every render through one function —
`AnalogEngine::render_span` (`crates/paraclete-nodes/src/analog_engine.rs:159`)
and `FmEngine::render_span` (`crates/paraclete-nodes/src/fm_engine.rs:157`) —
which dispatches a whole `[start, end)` span to a per-machine
`process_*(start, end)`. Each of those reads **all** its params once before
the sample loop and derives filter/envelope coefficients from them (e.g.
`analog_engine.rs:180-190`). That is exactly why an LFO cannot simply exist:
nothing re-reads a modulated param within a span.

The decision: `render_span` chunks `[start, end)` into sub-blocks of at most
`LFO_SUB_BLOCK: usize = 64` samples and calls the machine's `process_*` per
chunk. Boundaries are measured **from `start`**, not from the absolute block
position, so a span of 100 samples renders as 64 + 36 regardless of where it
sits in the block.

*Why relative, not absolute:* a span boundary is already a discontinuity
(a note started there), so aligning sub-blocks to it costs nothing and
removes the need to thread absolute block offsets into `render_span`. The
cost is that LFO update instants are not sample-aligned across spans within
one block — inaudible at 64 samples (1.45 ms at 44.1 kHz) and the ADR
already accepted control-rate modulation.

**D3 — `lfo_dest` stores the target param's name-hash id; the stepped
encoder's display index comes from an explicit, append-only per-engine dest
table.** This is ADR-042 amendment 2 restated as a build rule: the table is
a `&'static [u32]` of param ids per hosting node, appended to only at the
end, never reordered. `lfo_*` params are excluded from it (no
self-modulation), and so are `machine`-class identity params (ADR-042
decision 1). A saved `lfo_dest` therefore survives a table append; it does
**not** survive a reorder, which is why the table is append-only and why a
test asserts its head.

**D4 — Dest display labels are built surface-side from the dest table plus
the union cap-doc's param names.** ADR-042 amendment 5's reason is real and
verified: `ParamDisplayAdapter::Dynamic` **panics on clone**
(`crates/paraclete-node-api/src/capability.rs:52-53`), and it sits on
`ParamDescriptor.display` (`capability.rs:98`), which the mainline cap-doc
path clones. So a descriptor-side dynamic display is not usable as specced.
The wire also carries no options today — `stepped: None, options: None` are
hardwired at `crates/paraclete-antiphon/src/view.rs:79-80` and
`crates/paraclete-antiphon/src/protocol.rs:379-380`. MM-C5 populates them;
until then no surface can label a stepped param at all.

**D5 — Kit and scene interaction is OUT of scope, and that is not a
deferral of judgement.** ADR-041 decision 5 (kits apply `machine` first)
and the scene-morph rejection reference ADR-039/ADR-040 machinery that
**does not exist in the codebase**: `grep -rln "KitStore\|apply_kit\|scene_assign" crates/`
returns nothing. P11 and AN are both unscheduled. Those decisions stay
binding as *forward constraints on whoever builds kits* — recorded in §5 —
but nothing in MM can implement or test them.

---

## §1. Scope

**In:**

- `machine` as a runtime stepped bank param on `AnalogEngine` and `FmEngine`
- union parameter bank with per-machine descriptor overlays
- `MachineVariant` on `Rule`, plumbed through composite assembly and the wire
- `LfoBlock` in `engine_dsp`, hosted by both machine engines, `Sampler`
  and `FilterNode`
- the 64-sample sub-block render structure both of the above need
- MOD page content, `LfoShape` affordance, `/node/{id}/state/lfo_phase`
- the debug-build param-ref assertion (ADR-041 amendment 5), which closes
  **#47 / BUG-037** structurally

**Out:**

- Kits, scenes, morphing (D5)
- Tempo-synced LFO — ADR-042 decision 5 stages it deliberately; engines
  receive no tempo today. The param surface is frozen by this phase so
  sync lands later with **no surface change** (OQ-M2)
- LFO2 (OQ-M1) — deferred until a session asks
- Per-sample application for pitch-class dests (OQ-M3)
- `FmVoice` / ADR-043 — that is P14, which depends on this phase
- Machine-select on any surface other than Theotokos; Theoria gets the
  wire fields but its own control is a W-track concern

---

## §2. Commit sequence

Every commit: `cargo test --workspace` green (bare `cargo test` runs only
the app crate), `cargo clippy --workspace` clean on touched crates, no new
dependencies, **no allocation in `process()`**. Each commit compiles and
passes on its own — no commit may reference a type a later commit
introduces (design-process learning 6). Code and doc changes go in separate
commits, and each staged commit gets a fresh-context hostile review before
it lands.

### MM-C0 — Honor declared slots in composite assembly ✅ *(landed)*

**Status: implemented ahead of this spec**, as the prerequisite both ADRs
name (ADR-041 amendment 2, ADR-042 amendment 3).

`merge_page` assigned slots from a sequential counter and never read
`page_ref.slot`, making `PageRef::slot`
(`crates/paraclete-node-api/src/rule.rs:59-63`) documentation-only — while
Theotokos's *non*-composite path (`crates/paraclete-theotokos/src/model.rs:453`)
already sorted by the declared value. Now: declared slot honored, each
contributor padded to a whole number of `SUB_PAGE_SLOTS` (8), params emitted
slot-sorted, `debug_assert` on a node claiming one slot twice.

*Carried finding:* the `make_rule` test helper hardcoded `slot: 0` for every
param and the new assertion fired on it immediately — a fixture describing a
node no real node could be, which passed only because nothing read the field
(design-process learning 4, found in our own tests). Helper fixed to assign
slots sequentially per page.

*Deliberate widening, recorded per guardrail 1:* ADR-042 amendment 3 scopes
8-slot padding to "each node's **MOD** contribution"; MM-C0 applies it to
every merged page. Generalising is the better engineering call — one rule
beats a page-name special case — but it has a UX consequence nobody has
exercised: any page with two contributors now needs ≥2 sub-pages even when
it holds three params total, so the performer pages across to reach the
chain node. Intended for MOD; new and untested for SRC/FX/AMP. No shipped
instrument has a multi-contributor page today (see MM-C1's note), so this
first bites in MM-C11.

### MM-C1 — Carry `slot` to the encoder column ✅ *(landed, `a549768`)*

**MM-C0 is necessary but not sufficient, and this is the other half.**
Review of MM-C0 found that the declared slot stops at the crate boundary:
`Model::resolve_encoder_params`
(`crates/paraclete-theotokos/src/model.rs:394`) returns a tuple that
**does not include `slot`**, and the renderer places positionally —
`(0..8).map(|i| encoder_params.get(i))` (`lib.rs:308-311`), with the jog
dispatch matching (`lib.rs:1081-1082`). So a contributor declaring slots
0, 2, 5 renders on encoders 1, 2, 3.

**Change:** carry `slot` through `resolve_encoder_params` and place by
`slot % SUB_PAGE_SLOTS`, leaving declared gaps as empty columns.

**Why it blocks the ADR-041 work specifically:** amendment 2 puts
machine-select on a TRIG slot *by convention*. Until placement reads the
slot, declaring `machine` at slot 3 does not put it on encoder 4, and
MM-C6 would be building on a convention that is not real.

**What MM-C0 already guarantees, so this is not a regression:** because
`contributor_base` is always a multiple of 8, the window filter
`slot ∈ [8k, 8k+8)` (`model.rs:403`) selects exactly one contributor's
params. Straddling — ADR-042 amendment 3's actual concern — is closed.
Every shipped node also declares densely from 0, so there is no
user-visible change today; the gap first bites when per-machine variants
declare sparse slot sets.

**Also in scope:** `resolve_page_params_n` (`model.rs:320-331`, the numpad
A/B/C bindings) reads `page.params` positionally with `.take(n)`. That path
is formally descoped (BUG-038 / OQ-T24) and is **not** being revived here,
but it must not be left asserting the opposite convention — either route it
through the same placement or comment it as descoped-and-inconsistent.

**Tests:** a rule declaring slots 0, 2, 5 puts params on encoder columns
0, 2, 5 with 1, 3, 4 empty; jogging column 2 moves the param declared at
slot 2, not the second param in the list.

*As landed:* `resolve_encoder_params` returns
`EncoderBank = [Option<EncoderParam>; SUB_PAGE_SLOTS]` rather than a wider
tuple. The bug was that *two* call sites independently interpreted position,
so an eighth tuple field would have left the same drift available; the type
change makes positional indexing unrepresentable, which is a stronger
guarantee than the tests.

*Behaviour change not covered by "no visible change today", recorded per
guardrail 1:* in the engine-local `Rule` branch, a `param_pages` entry whose
id is absent from the cap-doc — BUG-037's exact shape — used to be **dropped**,
closing the gap and shifting every later param one column left. It now leaves
the column empty. The new behaviour is the intended one, but it is a change,
and it is latent rather than dead: the composite branch wins in the shipped
app, so this goes live the moment #152 drops a track's composite view.

*Also filed from this commit's review:* #152 (`main.rs` builds the per-track
composite Vec with `filter_map`, so a track that fails to assemble shifts
every later track's params — and Theotokos indexes that Vec by track).

*Correction carried:* #47 (BUG-037) was closed by accident — the MM spec
commit's body contained the prose "MM-C4 closes #47" and GitHub parsed the
keyword. Reopened, and its body now also names the `AnalogEngine` instance
(`analog_engine.rs:276` places `tune` at SRC slot 0 unconditionally, but the
HiHat cap-doc declares only `tone`/`decay`/`open`), which the original issue
did not mention. MM-C4 must fix both engines, not just `FmEngine`.

### MM-C2 — `MachineVariant` on `Rule` ✅ *(landed, `4da6a01`)*

**Change:** `crates/paraclete-node-api/src/rule.rs` — add

```
pub struct MachineVariant {
    pub value: u32,                                  // the `machine` param value
    pub name: Cow<'static, str>,                     // "AnalogKick"
    pub page_groups: Cow<'static, [Cow<'static, str>]>,
    pub pages: Cow<'static, [(u32, PageRef)]>,       // this machine's param_pages
    pub overlays: Cow<'static, [(u32, ParamOverlay)]>,
}

pub struct ParamOverlay { pub min: f64, pub max: f64, pub default: f64, pub identity: bool }
```

and `Rule.variants: Cow<'static, [MachineVariant]>`, defaulting empty.

**Why a separate commit:** every existing `Rule` literal in the workspace
must gain the field. That is mechanical breadth with no behaviour, and
mixing it into MM-C3 would bury the engine logic in churn.

**Empty `variants` must remain exactly today's behaviour** — base-`Rule`
fields are the default (ADR-041 decision 3). Nothing consumes `variants`
yet; this commit is green because it changes nothing.

**Tests:** a `Rule` with empty `variants` assembles byte-identically to
before (assert against the existing composite tests, unchanged).

### MM-C3 — `AnalogEngine`: union bank, `machine` param, declick switch ✅ *(landed)*

**Two consequences of this commit that a reader — or a paired session — must
not mistake for regressions.**

*The union range is live on the encoder until MM-C6, not just in the bar
fill.* `resolve_encoder_params` takes `min`/`max` from the cap-doc descriptor
(`theotokos/src/model.rs:459-461`), and Theotokos feeds `max - min` into
`jog_step` and then clamps to it (`lib.rs:768`, `:1126`). So between MM-C3 and
MM-C6, Kick's `tone` jogs ~2.25× coarser per detent and can be driven to
18 kHz, and HiHat's `decay` to 2.0 s against its own 1.0 s ceiling. MM-C6
item 3 is the fix — display and input clamp to the **active overlay** while
storage stays union-ranged. **Do not hold a paired session between C3 and C6
without saying this first**; the feel change is real and would read as a
defect.

*HiHat's SRC page loses `tune`, and gains a hole at slot 0.* HiHat never
declared `tune` — the unconditional `tune` page ref was half of #47
(BUG-037), showing a control the engine ignores. Per-machine pages remove it.
The hole is deliberate: `tone` stays on the same encoder across all three
machines, so switching machine does not shuffle the params under the
performer's fingers.

**Changes:** `crates/paraclete-nodes/src/analog_engine.rs`

1. `build_doc` (`:106`) returns the **union** of all three machines'
   params plus `machine` itself (stepped, `min 0, max 2`, default = the
   constructor's machine). Union storage is **widest-envelope**
   (ADR-041 amendment 1), which the current code makes concrete:

   | param | Kick | Snare | HiHat | union |
   |---|---|---|---|---|
   | `tone` | 200–8000 | 200–8000 | 1000–18000 | **200–18000** |
   | `decay` | 0.01–2.0 | 0.01–2.0 | 0.01–1.0 | **0.01–2.0** |
   | `tune` | −24–24 | −24–24 | *absent* | −24–24, inert on HiHat |

   Per-machine `min`/`max`/`default` move into `MachineVariant.overlays`.
   Union docs **dedup by id**.

2. `machine` change is applied at a **block boundary only**, never
   mid-sub-block, with D1's fade. Per-machine state is already all
   pre-allocated in the struct (`:31-41`) — confirm no new allocation.

3. `AnalogEngine::kick()/snare()/hihat()` (`:95-97`) become thin wrappers
   setting the `machine` default. `instrument.yaml` keeps working unchanged
   — that is a hard requirement, not a nicety.

#### The trap here — and where it actually is

`ParameterBank` **already clamps on every write, and never on read**:

| site | `crates/paraclete-node-api/src/parameter.rs` | |
|---|---|---|
| `CMD_SET_PARAM` | `:73` | `s.current = cmd.arg1.clamp(s.min, s.max)` |
| `CMD_BUMP_PARAM` | `:78` | `.clamp(s.min, s.max)` |
| `set()` | `:97` | `s.current = value.clamp(s.min, s.max)` |
| `get()` | `:85-91` | plain read, **no clamp** |

So the dangerous act is **not** adding a clamp to a read path. It is
**narrowing `ParameterSlot.min`/`max` to the active machine's overlay** —
which is the thing that looks like correctly applying the overlay. Because
writes already clamp, narrowing the range silently truncates *storage*.

It is worse than a wrong sound, because of the order the house convention
mandates. `activate()` rebuilds the bank from `build_doc(self.machine)`
(`crates/paraclete-nodes/src/analog_engine.rs:344-345`) and replays through
the clamping `set()` (`:350`); `deserialize()` runs **after** `activate()`
and re-applies saved values through the same `set()`. If the bank's range
is the active machine's rather than the union, **loading a project
truncates every value belonging to a machine that is not currently
selected** — and the truncation persists the next time that project is
saved.

**Recovery differs sharply by site, which is why naming the site matters:**

- *Read-path clamp* (the engine's `get_param` wrapper at `:99`): affects
  only what the DSP hears. Storage is intact. Delete the line and every
  value returns. **Fully recoverable.**
- *Bank-range narrowing*: truncates at write time. In-session edits are
  gone immediately. A project already on disk survives — until it is loaded
  under the bug and saved again. **That load-then-save window is the only
  unrecoverable path in this phase**, and it is the one to guard.

**Three rules that make it hard rather than merely forbidden:**

1. **The engine never holds overlays.** They live in `MachineVariant` on
   the `Rule` (MM-C2) and are consumed by surfaces. The engine has no
   overlay data to clamp *with*, so the wrong turn requires deliberately
   plumbing overlays into the engine — a visible act in review, not a
   one-line edit. This is affordable because ADR-041 decision 2 specifies
   no reset-to-default on switch ("inert but retain values"), so the engine
   genuinely never needs per-machine defaults.
2. **`build_doc` uses `self.machine` for the `machine` param's default and
   for nothing else.** Ranges are union, unconditionally.
3. **A machine switch must not rebuild the bank.** Rebuilding is
   `activate()`'s job and it resets every slot to defaults — the same data
   loss by a different route.

**Tests.** Four, and the first two are the guards:

- **Invariant, shared by both engines:** an
  `assert_union_bank_covers_all_variants(engine)` helper asserting each
  bank slot's `[min, max]` contains every variant overlay's range for that
  id. Fails the instant anyone narrows, in either engine, without needing
  to guess which param they narrowed.
- **The corruption path itself:** serialize with a value that is legal on
  HiHat and out of range on Kick, deserialize with Kick active, assert the
  value is unchanged. This is the load-then-save window as a test.
- **Round trip, derived not literal:** compute the probe value *from the
  declared cap-doc ranges* (a param where one machine's max exceeds
  another's), not from a hardcoded number. Making a failing derived test
  "pass" means changing what it derives, which reads as wrong; retuning a
  literal reads as reasonable.
- Union doc has no duplicate ids; `machine` is stepped with the right
  range; a switch mid-note produces no sample discontinuity above the
  `discontinuity_lt` threshold (test-driver scenario).

### MM-C4 — `FmEngine`: same, and BUG-037 dies structurally

**Changes:** `crates/paraclete-nodes/src/fm_engine.rs` — as MM-C3. The
verified conflicts:

| param | Kick | Bell | Bass | union |
|---|---|---|---|---|
| `decay` | 0.01–2.0 | 0.05–8.0 | 0.05–4.0 | **0.01–8.0** |
| `feedback` | 0–1.0 | 0–0.5 | *absent* | **0–1.0** |
| `ratio` | *absent* | 0.5–8.0 | 0.5–4.0 | **0.5–8.0** |

**This is the commit that closes #47 (BUG-037).** That bug is
`FmEngine::to_rule` (`:287-294`) declaring one machine-invariant page set
referencing `ratio`/`index`/`attack` for all three machines, while FmKick's
doc (`:106-112`) declares none of them and its `punch` appears on no page —
composite assembly silently degrades the unmatched refs to `param_{id}`
placeholders. Per-machine pages in `MachineVariant` make the mismatch
unrepresentable. **Close #47 in this commit's message.**

**Tests:** every param ref in every variant's `pages` resolves in the union
doc, for both engines — the assertion MM-C8 generalises.

### MM-C5 — Variants through composite assembly and the wire

**Changes:**

1. `crates/paraclete-view-assembly/src/lib.rs` — `CompositeView` carries
   per-node variants keyed by owning node; `merge_page` selects the active
   variant's `pages` when `variants` is non-empty, base fields otherwise.
   Depends on MM-C0's slot honoring being real.
2. `crates/paraclete-antiphon/src/view.rs:79-80` and
   `protocol.rs:379-380` — populate `stepped` and `options` instead of
   hardwired `None` (ADR-041 amendment 3). `view_meta` carries per-variant
   pre-merged pages per track.

**Why pre-merged on the wire:** clients build from the merged
`CompositeView`, not from `Rule` (amendment 3). Sending raw variants would
make every client re-implement the merge — including the 8-slot alignment —
and they would drift (design-process learning 5: `PageNav.tsx` already kept
a private copy of the page order).

**`Rule` does not reach the wire through serde, and this is a hand-mapping
commit.** Found while scoping MM-C2: `rule.rs`'s module doc says "the
Antiphon server serializes this to assemble the `view_meta` JSON message",
but the `serialize` feature on `paraclete-node-api`
(`paraclete-node-api/Cargo.toml:20`) is enabled by **no crate in the
workspace** — `grep -rn serialize --include=Cargo.toml` finds only the
declaration. The derive is dead code. Antiphon holds
`HashMap<u32, Rule>` (`crates/paraclete-antiphon/src/view.rs:19`) and builds
its own `ViewMetaParam` by hand.

Two consequences: MM-C2's field addition is wire-inert for free, needing no
`skip_serializing_if`; and the work here is extending Antiphon's mapping,
not adding serde attributes. Do not "enable the serialize feature" as a
shortcut — that would put the whole internal `Rule` shape on the wire as a
side effect, which is a protocol decision nobody has made.

**Tests:** a two-contributor page where the engine has variants and the
chain node does not merges correctly on both; wire round-trip carries
`stepped`/`options` for `machine`; a client rendering the pre-merged pages
gets the same slot layout as Theotokos.

### MM-C6 — Theotokos: variant-aware pages, machine-select, lock rejection

**Changes:** `crates/paraclete-theotokos/`

1. Watch `machine` on the state bus, swap the displayed variant locally —
   **zero runtime negotiation** (ADR-041 decision 3). Cap-docs are still
   collected once at startup (`main.rs` step 7); no query channel is added,
   and none may be.
2. Machine-select lives on the **TRIG page** (ADR-041 amendment 2), *not*
   "SRC slot 1" as the ADR body says — both engines' SRC slots are occupied
   and track identity belongs with track settings.
3. Encoder display and clamping use the **active overlay**; stored values
   stay un-clamped (see MM-C3's trap).
4. **P-locking `machine` is rejected surface/app-side**, keyed on the
   overlay `identity` flag (ADR-041 amendment 4). The sequencer stores
   opaque `(node_id, param_id)` locks and **cannot** know a foreign node's
   params — so decision 6's "`CMD_SET_LOCK_TARGET` validation" cannot live
   in the sequencer. The performer gets an echo-area rejection.

**Tests:** selecting a machine repaints the SRC page to that machine's
params; an inert param retains its value across the switch and is not
displayed; `[m]`+trig on `machine` is refused with a message and sets no
lock.

### MM-C7 — The 64-sample sub-block loop *(pure refactor, no LFO)*

**Changes:** `analog_engine.rs:159` and `fm_engine.rs:157` — `render_span`
chunks into `LFO_SUB_BLOCK` (64) sub-blocks per D2 and calls `process_*`
per chunk. No LFO exists yet; params are simply re-read per sub-block.

**This commit must be output-identical, and that is checkable.** Params are
constant across the span when nothing modulates them, so re-reading them
per sub-block changes nothing *except* where a `process_*` carries state
across the coefficient computation. Run
`--check-baseline` on both ADR-035 baselines
(`kick_reverb_clean.baseline.json`, `plock_authoring.baseline.json`) — they
use a deterministic single-threaded render and are bit-stable, so any drift
here is a real bug in the chunking, not noise.

**The instinct to resist:** if a baseline drifts, do **not** update the
baseline. A pure restructure that changes the output means a `process_*` has
per-span state that the chunking broke — e.g. envelope coefficients that
were computed once per note and are now recomputed per sub-block with a
slightly different starting value. Find that, don't re-fingerprint.

**Tests:** both baselines clean; a span shorter than 64 samples renders as
one sub-block; a span of exactly 64 renders as one, 65 as two.

### MM-C8 — `LfoBlock` in `engine_dsp` (pure, unhosted)

**Changes:** `crates/paraclete-nodes/src/engine_dsp.rs` — `LfoBlock`
following the existing `AdState` shape (`:16-45`): plain struct,
`trigger()`/`tick()`, no allocation, unit-testable with no engine.

Seven params per ADR-042 decision 1: `lfo_shape` (tri/sine/sqr/saw/exp/
ramp/rand), `lfo_speed` (0.01–64 Hz, exponential taper), `lfo_mode`
(free/trig/hold/one/half), `lfo_start_phase`, `lfo_fade`, `lfo_dest`,
`lfo_depth`.

#### The validation assertion, widened *(MM-C2 review)*

ADR-041 amendment 5 asks for a debug-build assertion that "every page/variant
param ref resolves in the union doc". As literally stated it would **pass on
BUG-037's successor**, so it lands here in three parts:

1. **Refs resolve against the active variant's displayed set, not the union
   doc.** `MachineVariant` carries `pages` only; `Rule`'s other
   reference-bearing fields — `affordances`, `envelopes`, `macros`, `routing`
   — have no variant slot and stay machine-invariant. Both engines already
   build them outside the per-machine `match`
   (`analog_engine.rs:280-303`, `fm_engine.rs:296-303`), so a base-`Rule`
   affordance can name a param the active variant does not display. Against
   the *union* doc that resolves fine; against the active variant it does not.
   `AffordanceHint::EnvelopeCurve { group_idx }` indexes `Rule::envelopes`,
   and `EnvelopeGroup::param_ids` is a fixed `[u32; 4]` — this bites as soon
   as machines differ in envelope shape, which ADR-043's variant-native
   FmVoice and MM-C11's first real `LfoShape` declarations both do.
2. **Overlay ids are unique within a variant.** `overlays` is a linear assoc
   list; duplicates are representable and precedence is undefined. Same shape
   as `PageRef::slot` being fiction until MM-C0 (design-process learning 9).
3. **Shared ids agree on `name`, `unit` and `stepped` across machines.** The
   union merge keeps the *first declarer's* non-range fields for a shared id
   and silently drops the rest. Consistent in `AnalogEngine` today (`tone` is
   Hz in all three, `decay` Seconds, `tune` Semitones) and therefore untested;
   MM-C4 runs the same merge over `FmEngine`'s wider conflict set, where a
   disagreement would show a param under the wrong unit with no diagnostic.
4. **A param flagged `identity` in any variant is flagged in all of them.**
   `machine` exists on every machine of a host, so the flag has to be repeated
   per variant; miss one and lock rejection silently stops working *for that
   machine only* — "p-locking machine works on HiHat but not Kick", which no
   test catches by accident.

Point 4 exists because the flag lives on the overlay per ADR-041 §0 A1. A
`Rule`-level `identity_params` list would remove the hazard structurally
rather than by assertion; that deviates from the ratified shape, so it is
**not** taken without the user — but it is the better design if this
assertion ever proves insufficient.

**Tests:** each shape over one cycle; `trig` resets phase to
`lfo_start_phase`, `free` does not; `hold` samples once per trigger;
`one` stops after a cycle; `half` stops at 0.5; fade-in and fade-out both
directions; the dest table's head is asserted so a reorder fails loudly
(D3).

### MM-C9 — Host `LfoBlock` in both machine engines

**Changes:** the 7 params join each engine's union doc; `LfoBlock` ticks
once per sub-block (MM-C7's structure); application is

```
effective = clamp(get_param(dest) + depth × range × lfo(t))
```

per ADR-042 amendment 1 — **base is the `get_param()` result**, which
already resolves `node_locks` before the bank (`analog_engine.rs:99-103`).
So the LFO breathes on top of a p-locked step's value; locks never defeat
the LFO nor vice versa. The bank value is untouched, so p-locks, the state
bus and (later) kits all see the base (ADR-042 decision 3) — and there is
no feedback into `CMD_BUMP_PARAM` reads.

`/node/{id}/state/lfo_phase` is published via the push-down
`published_state(&mut Vec<...>)` signature; paths cached in `OnceLock`, no
`format!` on the audio thread after the first cycle (BUG-007).

**Tests:** `lfo_depth = 0` is bit-identical to no LFO (baseline check);
a p-locked step plus an LFO yields lock+offset, not lock-only or
offset-from-bank; `lfo_dest = 0` is off; a dest pointing at an `lfo_*`
param is impossible by construction.

### MM-C10 — Host in `Sampler` and `FilterNode`

Per ADR-042 decision 6's rollout order. Same structure; `FilterNode` has no
`render_span` equivalent, so its sub-block loop is new here.

### MM-C11 — MOD page display and `LfoShape`

**Changes:** MOD page content in composite views (the canonical order
already reserves the slot — `CANONICAL_PAGE_ORDER` in
`crates/paraclete-view-assembly/src/lib.rs:22`, declared and empty today);
the first real `LfoShape` affordance declarations, closing the known
ADR-032 §2.6.5 gap; dest labels built surface-side per D4.

**The composite payoff, and the thing to actually verify:** on a track with
an engine *and* a filter, the merged MOD page stacks both nodes' LFOs —
engine wobble and filter wobble on one page, more LFOs per track than the
reference hardware (ADR-042 decision 2). MM-C0's 8-slot alignment is what
keeps the second node's block off the first's sub-page. Verify with a real
two-node chain, not a unit fixture.

---

## §3. What must not regress

1. **`instrument.yaml` keeps working unchanged.** Fixed constructors become
   `machine` defaults; no instrument file edit is required by this phase.
2. **No capability re-query exists or is added** (ADR-041 decision 1).
   Cap-docs are collected once at startup, before the executor owns the
   nodes. If a commit seems to need a runtime query, the variant
   declaration is incomplete — fix that instead.
3. **No allocation, lock, or block in `process()`.** Per-machine state is
   pre-allocated at build. The sub-block loop allocates nothing.
4. **The bank's `min`/`max` are the union range for the lifetime of the
   node, whatever machine is active.** Writes already clamp to them
   (`parameter.rs:73,78,97`), so narrowing them truncates storage — and
   because `deserialize()` runs after `activate()` through the same
   clamping `set()`, it truncates on *load*. Overlays are a surface
   concern and never reach the engine. See MM-C3's trap for the recovery
   analysis; this is the phase's only unrecoverable failure.
5. **A machine switch never rebuilds the bank.** That is `activate()`'s
   job and it resets to defaults.
6. **The base/offset split.** The LFO never writes the bank.

---

## §4. Open questions carried

- **OQ-M1** — LFO2 as `lfo2_*` param duplication with an overflow page
  (FUNC+PG per ADR-038). Deferred until a session asks.
- **OQ-M2** — tempo distribution to engines. Staged design: the clock's
  `TransportEvent` stream reaches engines via `events_in` (sequencer
  passthrough, additive), and an `lfo_sync` toggle reinterprets `lfo_speed`
  as a musical multiplier. **The param surface is frozen by this phase so
  sync lands with no surface change.**
- **OQ-M3** — per-sample application for pitch-class dests. 64-sample
  control rate may audibly step a pitch sweep; a session decides.
- **OQ-M4** — *deleted by ADR-042 amendment 2.* Its premise ("dest indices
  stay stable by construction because dest indexes the union bank") was
  wrong; D3's append-only table replaces it. Recorded here so nobody
  reinstates it from the ADR body.
- **OQ-14** — decided 2026-07-13 and implemented here: machine as a
  parameter, not a `type_tag` swap.
- **#149 (INFRA-014)** — test-driver numeric targets are unvalidated. MM
  leans on test-driver scenarios for MM-C3/C7/C9; a typo'd id silently
  no-ops. Worth closing before the scenarios multiply.

## §5. Forward constraints on work this phase cannot do

Binding on whoever builds kits (P11) and scenes (AN), recorded because MM
cannot implement or test them (D5):

- **Kits include `machine`, and kit apply assigns it FIRST**, then the
  remaining params — a stable sort in kit apply, one rule, no races
  (ADR-041 decision 5).
- **Scene morphing must not assign `machine`.** Stepped identity is not
  morphable; scene-assign rejects it, keyed on the same overlay `identity`
  flag MM-C6 uses for lock rejection.
- A kit captured on machine A applies losslessly to a track currently on
  machine B — the values sit in the union bank until the machine that reads
  them is selected. This is only true if MM-C3's un-clamped storage holds.

## §6. Exit criteria

1. Both engines expose `machine` as a stepped param; all six machines are
   runtime-selectable from Theotokos with no audible artefact.
2. A track's params survive machine round-trips losslessly.
3. Both engines, `Sampler` and `FilterNode` host an LFO; a two-node chain's
   merged MOD page stacks both, sub-page aligned.
4. Both ADR-035 baselines clean at MM-C7 and again at MM-C9 with
   `lfo_depth = 0`.
5. #47 (BUG-037) closed by MM-C4, with the debug assertion from MM-C8
   preventing its class.
6. ADR-041 and ADR-042 `Status:` lines updated with implementation notes
   (append-only body).
7. A user-paired session before the milestone closes — machine switch and
   LFO depth are both performance gestures, and this phase has no session
   gate written into it otherwise.
