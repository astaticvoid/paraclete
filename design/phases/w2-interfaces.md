# Paraclete — W2 Interface Specification (Theoria Editor)

> **Implementation blueprint.** Contracts only — deviations stop, get recorded
> in the phase report, and go to the user.
>
> **W2 deliverable:** the tablet is an editor surface — fixed rail +
> contextual window with cap-doc-driven parameter pages for every engine, a
> chain view, and the frozen ADR-032 view-plugin API. Ends in **paired
> session #2**.
> **Last updated:** 2026-07-13
> **Baseline:** W1 C4 shipped; P10 C5 shipped; debug/test harness (Rank 2) complete.
> **Design authority:** `design/specs/w2-reference-analysis.md` (groundwork, 4
> manuals, 8 convergent patterns) + paired decisions 2026-07-13 (§6.0–6.7, OQ-13,
> OQ-14).
> **Model routing:** ADR-032 + cap-doc + protocol changes Opus-tier (API freeze);
> view renderer code Opus-tier.

---

## Design decisions (ratified 2026-07-13)

| Decision | Resolution |
|---|---|
| §6.0 | **Track-first nav. Chain view = convenience hop** through the same pages in signal-flow order (not a separate mode) |
| §6.1 | **Elektron POC rail layout:** left edge = track selectors; bottom edge = page nav row (TRIG · SRC · FLTR · AMP · FX · MOD) + transport on far right; above bottom = encoder row (8 virtual knobs, already shipped); center = contextual window |
| §6.2 | **2×4 param grid** (~8 params/page) with **graphical affordances** per page type (env curve on AMP, filter shape on FLTR, LFO shape on MOD, waveform/engine diagram on SRC) |
| §6.3 | **Engine-declared page groups** with a default Elektron set (TRIG/SRC/FLTR/AMP/FX/MOD). Engines that fit the mold don't repeat it; engines that don't declare their own |
| §6.4 | **Default views shipped with nodes, overridable** via cap-doc `view_overrides` |
| §6.5 | **Continuous blend send knobs + topology toggle** (Hydrasynth model). Maps onto Paraclete's additive mix model |
| §6.6 | **5 cap-doc extensions** — see ADR-032 |
| §6.7 | **Vertical slice: kick engine end-to-end** (SRC + FLTR + AMP + FX + MOD pages) |
| OQ-13 | **Compose from primitives → compile to monolith, reversible.** Author as graph, ahead-of-time fuse to monolithic engine; always un-compile back. Compiled monolith declares exposed modulation targets via the same cap-doc |
| OQ-14 | **Machine-as-parameter.** Stepped param selects machine variant; live-switchable, p-lockable, kit-savable. No topology change on switch |

---

## Commit sequence

| Commit | Crate(s) | Deliverable |
|---|---|---|
| **0** | `paraclete-node-api` | **ADR-032 freeze:** `ViewPlugin` trait + `Rule`, `PageRef`, `MacroBinding`, `MacroCurve`, `AffordanceHint`, `EnvelopeGroup`, `RoutingSemantics` types. Cap-doc stores `Box<dyn ViewPlugin>`. |
| **1** | `paraclete-nodes` | **Node-side implementations:** `ViewPlugin` on every engine and effect node. Default page conventions. |
| **2** | `paraclete-antiphon` | **View protocol:** `view_meta` composite message (assembled from sub-graph walk), `get_view_meta` req/resp, `engine_diagram` push. |
| **3** | `web/` | **Fixed rail layout:** left track column, bottom page row + transport, encoder row, contextual window frame. |
| **4** | `web/` | **Param page rendering:** param grid cells with affordance renderers (envelope curve, filter shape, LFO shape, waveform). |
| **5** | `web/`, `paraclete-nodes`, `paraclete-antiphon` | **Kick engine vertical slice:** SRC + FLTR + AMP + FX pages, end-to-end. Sub-graph walk + composite `view_meta`. |
| **6** | `web/` | **Chain view:** per-source continuous send sliders + topology toggle. |
| **7** | — | **Exit criteria + paired session #2** (session recording only). |

---

## Commit 0: ADR-032 — `ViewPlugin` trait + support types

The view-plugin API freeze at the LGPL3 L2 boundary. Every L3 node
that has engine/surface presence implements `ViewPlugin`. See
`design/adr/ADR-032-theoria-view-plugin-api.md` for the full ADR.

### 0.1 `Rule` — owns the view data

```rust
/// The concrete type stored on CapabilityDocument.
/// Built from a node's ViewPlugin trait methods at construction time.
/// Antiphon serializes this to the `view_meta` JSON message.
#[derive(Serialize, Clone)]
struct Rule {
    /// Human-readable display name (e.g. "Kick", "Filter", "Reverb").
    name: Cow<'static, str>,
    /// Ordered page group IDs this node contributes params to.
    page_groups: Cow<'static, [Cow<'static, str>]>,
    /// Param → page placement.
    param_pages: Cow<'static, [(u32, PageRef)]>,
    /// Macro bindings (may be empty).
    macros: Cow<'static, [MacroBinding]>,
    /// Per-param affordance hints (may be empty).
    affordances: Cow<'static, [(u32, AffordanceHint)]>,
    /// Envelope groups — sets of params that together form an ADSR curve.
    envelopes: Cow<'static, [EnvelopeGroup]>,
    /// Per-param routing semantics (may be empty).
    routing: Cow<'static, [(u32, RoutingSemantics)]>,
    /// SVG diagram bytes (None if no diagram).
    diagram: Option<Cow<'static, [u8]>>,
    /// Override sub-node views, keyed by node id.
    view_overrides: Cow<'static, [(u64, Rule)]>,
}
```

A node's `ViewPlugin` trait implementation returns the same data piecemeal.
`Rule` is the serializable snapshot — built once per node at construction
time, not on the audio thread. `Rule` is stored on `CapabilityDocument` as
an `Option<Rule>` field. Nodes without surface presence (utility nodes,
internal clock, scripting gateways) set it to `None`.

### 0.2 `PageRef` — param-to-page placement

```rust
#[derive(Serialize, Clone)]
struct PageRef {
    /// Page group key ("SRC", "FLTR", "AMP", "FX", "MOD", or custom).
    page: Cow<'static, str>,
    /// 0-based slot within the page. Pages target 8 slots (2×4 grid).
    /// Slots beyond 7 wrap onto sub-pages of the same group.
    slot: u8,
}
```

### 0.3 `MacroBinding` + `MacroCurve`

```rust
#[derive(Serialize, Clone)]
struct MacroBinding {
    /// Display name for this macro control.
    name: Cow<'static, str>,
    /// Parameter IDs this macro drives.
    targets: Cow<'static, [u32]>,
    /// Mapping per target. Must match targets length.
    curves: Cow<'static, [MacroCurve]>,
    /// Page this macro appears on (None = appears on all pages with targets).
    page: Option<Cow<'static, str>>,
}

#[derive(Serialize, Clone)]
enum MacroCurve {
    Linear,
    Exponential,
    InverseExponential,
}
```

### 0.4 `AffordanceHint` — what to draw beside a param

```rust
#[derive(Serialize, Clone)]
enum AffordanceHint {
    None,
    /// Reference to an EnvelopeGroup by index (see §0.5).
    EnvelopeCurve { group_idx: u8 },
    FilterShape,
    LfoShape,
    Waveform,
    /// Engine diagram region highlight.
    /// `region_id` matches a named region in the engine's diagram SVG.
    DiagramHighlight { region_id: Cow<'static, str> },
}
```

`EnvelopeCurve` now carries a `group_idx` — an index into the `envelopes`
array on the same `Rule`. This is the fix for the single-param affordance
problem: a group of 4 params (attack/decay/sustain/release) is declared
once as an `EnvelopeGroup`, and each individual param's affordance hint
references it by index. The renderer reads the group to know which params
form the curve.

### 0.5 `EnvelopeGroup` — a set of ADSR params

```rust
#[derive(Serialize, Clone)]
struct EnvelopeGroup {
    /// Type: always `"ADSR"` for now (future: AHD, DADSR, etc.).
    env_type: Cow<'static, str>,
    /// Display label for the envelope (e.g. "Amp Envelope", "Filter Env").
    label: Cow<'static, str>,
    /// Ordered param IDs: [attack, decay, sustain, release].
    param_ids: [u32; 4],
}
```

The `param_ids` array identifies which four parameters form this envelope.
All four must belong to the same node. A node can declare multiple
envelope groups (e.g. amp envelope + filter envelope). The renderer draws
the ADSR curve from the live values of these params.

### 0.6 `RoutingSemantics` — send/destination binding

```rust
#[derive(Serialize, Clone)]
struct RoutingSemantics {
    /// Logical destination name shown in the chain view.
    destination: Cow<'static, str>,
    /// Human-readable source label (e.g. "Kick", "Bass").
    source_label: Cow<'static, str>,
}
```

The `destination` field uses logical names (`"filter"`, `"reverb"`,
`"delay"`) that map onto the destinations listed in the chain view's
sub-graph walk, not hardcoded node IDs.

### 0.7 `ViewPlugin` trait

```rust
trait ViewPlugin {
    fn to_rule(&self, node_id: u64, sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule;
}
```

**Design note:** a single builder method, not one per field. Nodes
construct a `Rule` struct with all view data at once. This avoids
the N-method trait with awkward return types (`&[(u64, Box<...>)]`)
and gives the node access to sub-node `ViewPlugin`s for composition.
`sub_nodes` is the list of this node's internal/sub nodes (e.g. a
GraphNode's children). The `view_overrides` field on `Rule` stores
pre-built `Rule`s for any sub-node the engine wants to customize.

Not on the audio thread — `to_rule()` is called once at construction
time (or on `activate()` for dynamic graphs).

### 0.8 Test contract

- Every engine and effect node implements `ViewPlugin`.
- `test_viewplugin_defaults_elektron_pages` — FilterNode returns FLTR group,
  AnalogEngine returns SRC+AMP groups.
- `test_macro_binding_fan_out` — a node declaring `MacroBinding { targets:
  [C_RATIO, A_RATIO, B_RATIO] }` can be queried.
- `test_envelope_group_param_ids_match_node` — group's `param_ids` all
  exist in the node's parameter bank.

---

## Commit 1: Node-side `ViewPlugin` implementations

Implement `ViewPlugin::to_rule()` on every existing engine and effect
node. The `Rule` is built at construction time; stored on the cap-doc.

### 1.1 Default page conventions

| Node type | `page_groups` | Notes |
|---|---|---|
| `AnalogEngine` (kick/snare/hihat) | `["SRC", "AMP"]` | SRC = engine controls. AMP = internal envelope (decay/attack/release). FLTR/FX/MOD come from downstream nodes via composite assembly (Commit 2). |
| `FmEngine` | `["SRC", "AMP"]` | Same. MOD page (FM modulation) goes on SRC or a custom FM page via macros. |
| `Sampler` | `["SRC", "AMP"]` | SRC = sample select + playback params. |
| `FilterNode` | `["FLTR"]` | cutoff, resonance, mode, drive, env_amount, keytrack. |
| `DistortionNode` | `["FX"]` | drive, type, tone. |
| `ReverbNode` | `["FX"]` | decay, wet, dry, pre_delay, size. |
| `Sequencer` | `["TRIG"]` | NOTE, VEL, LEN, PROB, COND, etc. |
| `MixNode` | `["FX"]` (routing only) | No param pages — provides chain routing data. |
| `InternalClock` | `None` (no `Rule`) | No surface presence. |
| `ScriptingGatewayNode` | `None` | No surface presence. |

### 1.2 Affordance hints per engine

| Engine | Param | Affordance | EnvelopeGroup needed? |
|---|---|---|---|
| `AnalogEngine` kick | `decay` | `EnvelopeCurve { group_idx: 0 }` | Yes — `EnvelopeGroup { env_type: "AHD", param_ids: [delay, attack, decay, hold] }` if kick uses AHD, else ADSR |
| `AnalogEngine` kick | `tune` | `None` | — |
| `AnalogEngine` kick | `drive` | `None` | — |
| `AnalogEngine` kick | `cutoff` | `FilterShape` | — |
| `AnalogEngine` kick | `resonance` | `FilterShape` | — |
| `FmEngine` bass | `ratio`, `feedback`, `mod_depth` | `DiagramHighlight { region_id }` | — |
| `Sampler` | sample waveform | `Waveform` | — |
| `FilterNode` | `cutoff` | `FilterShape` | — |
| `FilterNode` | `resonance` | `FilterShape` | — |

### 1.3 Machine-as-parameter (OQ-14)

`AnalogEngine` declares a stepped param with option labels. The param
descriptor's `stepped` field is augmented with an `options` array in
the underlying representation (implementation detail for Commit 1):

- Stepped param `"machine"` with options `["kick_hard", "kick_soft",
  "kick_808", "snare_crisp", "snare_fat", ...]`.
- When the machine value changes, the engine's `param_pages` and
  `affordances` for the SRC page re-map. This is handled by the
  engine's internal state, not by a graph change.
- `view_meta` is re-sent with updated SRC page params after a machine
  switch. The Antiphon server detects the change via the existing state
  mirror (param value change on the machine param) and re-assembles
  the composite view.

**Tests:**
- `test_analog_engine_machine_param_has_options` — the machine param
  descriptor carries option labels.
- `test_viewplugin_page_groups_never_empty_for_engines` — every engine
  node has at least `["SRC"]`.
- `test_viewplugin_affordance_hints_all_params` — every published param
  has an affordance hint entry (None is valid; the test ensures the
  map exists).

---

## Commit 2: Antiphon view protocol + composite assembly

### 2.1 Sub-graph walk

Antiphon constructs a composite view for a track by walking the audio
graph downstream from the engine node. The walk follows audio connections
from the engine's output port, collecting every node encountered:

```rust
fn walk_track_graph(engine_node_id: u64) -> Vec<u64>;
```

Implementation: follows `EdgeData::Audio` edges in topological order
from the engine to the MixNode. The walk stops at the MixNode (or
any node with no further audio output edges). The resulting node list
is the track's signal chain.

The walk result is cached until graph topology changes (`apply_patch`).

### 2.2 Composite `Rule` assembly

For each page group in the Elektron set (TRIG/SRC/FLTR/AMP/FX/MOD),
Antiphon collects params from all nodes in the sub-graph walk:

| Page | Source | Rule |
|---|---|---|
| `SRC` | engine node | engine's `Rule.param_pages` for page "SRC" |
| `AMP` | engine node | engine's `Rule.param_pages` for page "AMP" + `envelopes` |
| `TRIG` | Sequencer node (if in walk) | sequencer's `Rule.param_pages` for page "TRIG" |
| `FLTR` | first FilterNode in walk | filter's `Rule.param_pages` for page "FLTR" |
| `FX` | DistortionNode + ReverbNode + send params | merged from all nodes contributing to "FX" page group |
| `MOD` | any LFO/modulation node | future |

Params from different nodes on the same page group are **concatenated**:
engine params first, then downstream node params. The slot numbers are
re-assigned sequentially across the merged list so the encoder row maps
contiguously. Sub-pages (slots 8–15, 16–23, ...) are created if a page
group has more than 8 params.

Macros and envelope groups are passed through from their owning node.

### 2.3 `view_meta` message (push)

```json
{
  "type": "view_meta",
  "track_id": 0,
  "engine_node_id": 20,
  "engine_name": "AnalogEngine",
  "display_name": "Kick",
  "pages": [
    {
      "id": "SRC",
      "label": "Source",
      "params": [
        {"id": "machine", "node_id": 20, "label": "Machine", "affordance": "None", "slot": 0, "stepped": true, "options": ["kick_hard", "kick_soft", "kick_808"]},
        {"id": "decay",   "node_id": 20, "label": "Decay",   "affordance": "EnvelopeCurve", "env_group": 0, "slot": 1},
        {"id": "tune",    "node_id": 20, "label": "Tune",    "affordance": "None", "slot": 2},
        {"id": "drive",   "node_id": 20, "label": "Drive",   "affordance": "None", "slot": 3},
        {"id": "click",   "node_id": 20, "label": "Click",   "affordance": "None", "slot": 4},
        {"id": "noise",   "node_id": 20, "label": "Noise",   "affordance": "None", "slot": 5},
        {"id": "pitch_env","node_id": 20, "label": "Pitch Env","affordance": "None", "slot": 6},
        {"id": "bend",    "node_id": 20, "label": "Bend",    "affordance": "None", "slot": 7}
      ],
      "envelopes": [
        {"id": 0, "type": "AHD", "label": "Amp Envelope", "param_ids": ["delay", "attack", "decay", "hold"]}
      ],
      "macros": []
    },
    {
      "id": "AMP",
      "label": "Amp",
      "params": [
        {"id": "attack",  "node_id": 20, "label": "Attack",  "affordance": "EnvelopeCurve", "env_group": 0, "slot": 0},
        {"id": "decay",   "node_id": 20, "label": "Decay",   "affordance": "EnvelopeCurve", "env_group": 0, "slot": 1},
        {"id": "sustain", "node_id": 20, "label": "Sustain", "affordance": "EnvelopeCurve", "env_group": 0, "slot": 2},
        {"id": "release", "node_id": 20, "label": "Release", "affordance": "EnvelopeCurve", "env_group": 0, "slot": 3},
        {"id": "volume",  "node_id": 20, "label": "Volume",  "affordance": "None", "slot": 4},
        {"id": "pan",     "node_id": 20, "label": "Pan",     "affordance": "None", "slot": 5}
      ]
    },
    {
      "id": "FLTR",
      "label": "Filter",
      "params": [
        {"id": "cutoff",    "node_id": 40, "label": "Cutoff",    "affordance": "FilterShape", "slot": 0},
        {"id": "resonance", "node_id": 40, "label": "Resonance", "affordance": "FilterShape", "slot": 1},
        {"id": "env_amount","node_id": 40, "label": "Env Amt",   "affordance": "None", "slot": 2},
        {"id": "drive",     "node_id": 40, "label": "Drive",     "affordance": "None", "slot": 3},
        {"id": "mode",      "node_id": 40, "label": "Mode",      "affordance": "None", "slot": 4, "stepped": true, "options": ["LP", "BP", "HP"]}
      ]
    },
    {
      "id": "FX",
      "label": "Effects",
      "params": [
        {"id": "dist_drive", "node_id": 30, "label": "Dist Drive", "affordance": "None", "slot": 0},
        {"id": "dist_tone",  "node_id": 30, "label": "Dist Tone",  "affordance": "None", "slot": 1},
        {"id": "reverb_send","node_id": 20, "label": "Reverb Send","affordance": "None", "slot": 2, "routing": {"dest": "reverb"}},
        {"id": "filter_send","node_id": 20, "label": "Filter Send","affordance": "None", "slot": 3, "routing": {"dest": "filter"}}
      ]
    },
    {
      "id": "TRIG",
      "label": "Trig",
      "params": [
        {"id": "note",  "node_id": 10, "label": "Note",  "affordance": "None", "slot": 0},
        {"id": "vel",   "node_id": 10, "label": "Velocity","affordance": "None", "slot": 1},
        {"id": "len",   "node_id": 10, "label": "Length", "affordance": "None", "slot": 2},
        {"id": "prob",  "node_id": 10, "label": "Probability","affordance": "None", "slot": 3},
        {"id": "cond",  "node_id": 10, "label": "Condition","affordance": "None", "slot": 4, "stepped": true, "options": ["FILL", "PRE", "1ST", "A:B", "NEI"]}
      ]
    }
  ],
  "chain": {
    "nodes": [20, 30, 40, 200],
    "node_labels": {"20": "AnalogEngine", "30": "Distortion", "40": "Filter", "200": "Reverb"},
    "routing": [
      {"source": 20, "dest": "filter", "param_id": "filter_send", "value": 1.0},
      {"source": 20, "dest": "reverb", "param_id": "reverb_send", "value": 0.3}
    ]
  }
}
```

Every param carries `node_id` — the `set_param`/`bump_param` command
plane targets the owning node, not the composite view.

### 2.4 `get_view_meta` request/response

```json
{"type": "get_view_meta", "track_id": 0}
```

Response: the `view_meta` message above. The request carries a
`nonce` field for client-side matching against the async push:
```json
{"type": "get_view_meta", "track_id": 0, "nonce": "req-1"}
```
The response includes `"nonce": "req-1"`. If concurrent requests are rare
given the single-tablet interaction model, the nonce is included but
matching is optional.

### 2.5 `engine_diagram` push

```json
{"type": "engine_diagram", "node_id": 20, "svg_base64": "<svg>...</svg>"}
```

Sent once per engine type on connect; client caches. SVG elements with
`id` attributes matching `DiagramHighlight.region_id` values are the
highlight regions.

### 2.6 Triggers

`view_meta` is re-sent when:
- Active track changes (user taps track select)
- Machine param changes value (different variant = different SRC page params)
- Graph topology changes (`apply_patch` — invalidates cached sub-graph walk)

### 2.7 Changes to existing messages

- `topology` gains `has_view: bool` — whether the node has a `Rule`
  (non-None `view_plugin` on the cap-doc).
- `state` has no changes — param values are already mirrored.

### 2.8 Protocol version note

All W2 messages are part of protocol v0 (unstable, per ADR-031). No
version bump needed since the protocol is explicitly pre-freeze. W1
clients that parse `topology` loosely (ignoring unknown fields) are
unaffected by the `has_view` addition.

**Tests:**
- `test_view_meta_kick_composite` — `view_meta` for track 0 (kick) has
  SRC page from node 20, FLTR page from node 40, AMP page from node 20.
- `test_view_meta_affordance_hints` — `view_meta` carries correct
  affordance strings (`"EnvelopeCurve"`, `"FilterShape"`, `"None"`).
- `test_sub_graph_walk_kick` — `walk_track_graph(20)` returns
  `[20, 30, 40, 200]` (engine, distortion, filter, reverb).
- `test_view_meta_resends_on_machine_switch` — changing the machine
  param triggers a `view_meta` re-send with updated SRC page.
- `test_view_meta_resends_on_apply_patch` — graph topology change
  triggers `view_meta` re-send with updated chain+pages.

---

## Commit 3: Fixed rail layout (theoria-web)

The always-present chrome. Four zones:

- **Left edge** — track select column. 8 buttons, stacked vertically.
  Active track highlighted. Tap selects track → `get_view_meta` request
  → contextual window repaints.
- **Bottom edge** — page nav row (TRIG · SRC · FLTR · AMP · FX · MOD)
  + transport buttons (Play/Pause, Stop, Record) on the right.
  Active page highlighted. Pages with content lit; empty pages dimmed.
  Engine-declared pages beyond the 6 defaults append after MOD.
- **Encoder row** — between bottom edge and contextual window. 8 encoders
  map to slots 0–7 of the active page. Re-labeled on page switch.
  Already shipped from W1 baseline interactions.
- **Center** — contextual window. Content determined by active page.

Transport state mirrored from `/transport/*` bus paths (W1).

**Tests:** None (visual — validated in paired session).

---

## Commit 4: Param page rendering (theoria-web)

The contextual window renders the active page's parameter grid from
`view_meta`.

### 4.1 Grid cell

Each cell shows: icon + short label, value bar + numeric value, and
the graphical affordance (if any) below the bar.

### 4.2 Graphical affordance renderers

| Affordance | Renderer | Data sources |
|---|---|---|
| `EnvelopeCurve` (`env_group` present) | SVG ADSR curve | Reads `env_group.param_ids` from the `view_meta` page's `envelopes` array. Fetches live values for those 4 params via state mirror. Draws attack/decay/sustain/release curve. Multiple `EnvelopeCurve` params sharing the same `env_group` index all reference the same curve. |
| `FilterShape` | SVG filter response | Live cutoff + resonance values → dB-vs-frequency plot. |
| `LfoShape` | SVG waveform | Live wave type + rate. |
| `Waveform` | Canvas waveform overview | Sample waveform data (future: sent as base64 from server). |
| `DiagramHighlight` | SVG overlay on engine diagram | Highlights the named region on the engine's diagram SVG. |
| `None` | No drawing | Value bar + number only. |

### 4.3 Stepped params

Rendered as a popup option list on tap. Also adjustable via encoder
(before/after cycling).

### 4.4 Macros

Appear as additional rows within their declared page. Macro cell shows
name + value bar. Editing a macro sends `set_param` for each target.

**Tests:** None (visual).

---

## Commit 5: Kick engine vertical slice

End-to-end wiring: track select → `get_view_meta` → composite
`view_meta` → page nav → param edits via existing command plane →
state mirror updates display.

### 5.1 What each page shows

| Page | Source node(s) | Key params |
|---|---|---|
| SRC | AnalogEngine (kick, id 20) | machine (stepped), decay, tune, drive, click, noise, pitch_env, bend |
| AMP | AnalogEngine (kick, id 20) | attack, decay, sustain, release, volume, pan — with envelope curve affordance |
| FLTR | FilterNode (id 40) | cutoff, resonance, env_amount, drive, mode (stepped LP/BP/HP) — with filter shape affordance |
| FX | DistortionNode (id 30) + AnalogEngine sends | dist_drive, dist_tone, reverb_send, filter_send |
| TRIG | Sequencer (id 10) | note, velocity, length, probability, condition |

### 5.2 What this proves

- Composite assembly: one `view_meta` message aggregates params from 4
  different nodes (engine, filter, distortion, sequencer).
- Page nav works generically — switching to snare re-uses the same
  FLTR/FX view code (only SRC/AMP change).
- EnvelopeCurve affordance works — the AMP page's `envelopes` array
  groups decay/attack/sustain/release; the renderer draws the curve.
- Machine-as-parameter: changing "machine" on SRC page re-maps
  params; `view_meta` re-sends; page updates.
- No hardcoded engine knowledge in the client — all driven by
  `view_meta`.

**Tests:**
- `test_view_meta_kick_src_page` — composite `view_meta` has SRC page
  with decay/tune/drive/machine params from node 20.
- `test_view_meta_kick_filter_page` — same message has FLTR page from
  node 40.
- `test_envelope_group_on_amp_page` — AMP page has an `envelopes` array
  entry with `param_ids: [attack, decay, sustain, release]`.
- `test_machine_switch_remaps_src_page` — changing machine re-sends
  `view_meta` with different SRC params.
- `test_view_meta_snare_reuses_filter_page` — snare's composite
  `view_meta` has the same FLTR page from node 40 (shared filter).

---

## Commit 6: Chain view

A convenience projection of the track's signal graph. Accessible from a
"CHAIN" button in the page nav (rightmost slot, or appended after MOD).

### 6.1 Content

- Node blocks shown in signal-flow order (from `view_meta.chain.nodes`).
- Edges drawn between blocks.
- Send sliders from each source to each routing destination.
- Topology toggle (Series/Parallel) if the chain has parallel paths.

### 6.2 Navigation

Tapping a node block in the chain view navigates to that node's primary
page group (same as tapping FLTR/AMP/FX/SRC in the page nav). This is
the "convenience hop" — chain view re-orders pages by signal flow.

### 6.3 Send sliders

Per-destination continuous sliders. Adjusting a slider sends `set_param`
for the corresponding `param_id` on the owning `node_id`, using the
`routing` data from `view_meta.chain`.

**Tests:**
- `test_chain_view_nodes_in_signal_flow_order` — chain node list
  matches topological order from graph.
- `test_chain_view_routing_sends` — routing entries list correct
  `param_id` and `source` for each destination.

---

## Commit 7: Exit criteria + paired session #3

### 7.1 Exit criteria

| Criterion | Check |
|---|---|
| Tablet connects via USB-C (or localhost) | — |
| Fixed rail visible: track select, page nav, transport | — |
| Kick engine: SRC page shows params; FLTR page shows filter shape affordance; AMP page shows envelope curve | — |
| Editing a param updates sound in real time | — |
| Switching pages works | — |
| Machine param switches kick variants; SRC page updates | — |
| Chain view shows nodes + send sliders | — |
| Encoder row re-labels per page | — |
| Custom page groups (beyond the 6 defaults) appear on the rail | — |
| No hardcoded engine assumptions in the client — all views driven by `view_meta` | — |

### 7.2 Session recording

Output: `design/sessions/s3.md` (session #1 was W1, #2 was legibility).
Captures: tablet judgment, spec divergence, roadmap deltas or
explicit "no change."

**No code changes in this commit — session recording only.**
