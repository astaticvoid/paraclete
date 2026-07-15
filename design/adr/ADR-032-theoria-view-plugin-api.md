# ADR-032 — Theoria View-Plugin API

**Status:** Accepted (2026-07-13)

**Context documents:** ADR-014 (capability document), ADR-018 (cellular
architecture), ADR-019 (universal parameter control), ADR-031 (Antiphon
interface server), `design/specs/w2-reference-analysis.md` (4-manual
groundwork), `design/phases/w2-interfaces.md` (W2 implementation spec).

---

## Context

ADR-031 established that Theoria views are schema-driven. The W2 milestone
adds editor-grade parameter pages, a chain view, and graphical affordances
to Theoria. To render an engine without hardcoded knowledge — including
future third-party engines — each node must declare how its parameters
are presented.

The W2 reference spike (`w2-reference-analysis.md`) analyzed four hardware
manuals and found 8 convergent patterns. Four key requirements emerged:

1. **Named page groups** (TRIG/SRC/FLTR/AMP/FX/MOD) — every box groups
   params into ~8-param pages with a fixed spatial vocabulary.
2. **Graphical affordances** — envelope curves, filter shapes, LFO shapes,
   waveform overviews, engine diagrams. The window draws the thing, not a
   number list.
3. **Macros** — one expressive control fanning out to several internal
   params (Digitone HARM, Hydrasynth MACRO ASSIGN).
4. **Dual routing surfaces** — a global chain view and per-param send
   semantics (Syntakt routing grid + per-track ROUT param).

## Decision

### `Rule` — the serializable view-data struct

A concrete, serializable struct stored on `CapabilityDocument` as an
`Option<Rule>`. Nodes without surface presence (InternalClock,
ScriptingGatewayNode) leave it `None`. Built once at construction time
from the node's `ViewPlugin` trait method — never on the audio thread.
Antiphon serializes this directly to the `view_meta` JSON message.

```rust
#[derive(Serialize, Clone)]
struct Rule {
    name: Cow<'static, str>,
    page_groups: Cow<'static, [Cow<'static, str>]>,
    param_pages: Cow<'static, [(u32, PageRef)]>,
    macros: Cow<'static, [MacroBinding]>,
    affordances: Cow<'static, [(u32, AffordanceHint)]>,
    envelopes: Cow<'static, [EnvelopeGroup]>,
    routing: Cow<'static, [(u32, RoutingSemantics)]>,
    diagram: Option<Cow<'static, [u8]>>,
    view_overrides: Cow<'static, [(u64, Rule)]>,
}
```

### 1. `PageRef`

```rust
struct PageRef {
    page: Cow<'static, str>,  // "SRC", "FLTR", "AMP", "FX", "MOD", or custom
    slot: u8,                 // 0-based. Slots 0–7 = page 1, 8–15 = sub-page, etc.
}
```

### 2. `MacroBinding` + `MacroCurve`

```rust
struct MacroBinding {
    name: Cow<'static, str>,
    targets: Cow<'static, [u32]>,
    curves: Cow<'static, [MacroCurve]>,  // must match targets length
    page: Option<Cow<'static, str>>,
}

enum MacroCurve { Linear, Exponential, InverseExponential }
```

### 3. `AffordanceHint`

```rust
enum AffordanceHint {
    None,
    EnvelopeCurve { group_idx: u8 },  // index into Rule.envelopes
    FilterShape,
    LfoShape,
    Waveform,
    DiagramHighlight { region_id: Cow<'static, str> },
}
```

`EnvelopeCurve` references an `EnvelopeGroup` by index rather than being a
bare tag. This is the architectural fix for the single-param affordance
problem: an ADSR curve requires 4 params, and the group declaration tells
the renderer which four.

### 4. `EnvelopeGroup`

```rust
struct EnvelopeGroup {
    env_type: Cow<'static, str>,   // "ADSR", "AHD", "DADSR"
    label: Cow<'static, str>,
    param_ids: [u32; 4],
}
```

A node can declare multiple envelope groups (amp env, filter env).
All `param_ids` must belong to the declaring node.

### 5. `RoutingSemantics`

```rust
struct RoutingSemantics {
    destination: Cow<'static, str>,
    source_label: Cow<'static, str>,
}
```

`destination` uses logical names (`"filter"`, `"reverb"`) that map to
the destinations in the chain view's sub-graph walk.

### 6. `ViewPlugin` trait

```rust
trait ViewPlugin {
    fn to_rule(&self, node_id: u64, sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule;
}
```

A single builder method. Nodes construct a complete `Rule` at once rather
than implementing one accessor per field. `sub_nodes` provides access to
internal/sub-node `ViewPlugin`s for composition (e.g. a GraphNode with
internal filter + amp nodes). `view_overrides` on `Rule` stores pre-built
`Rule`s for sub-nodes the engine customizes.

### 7. Composite assembly (Antiphon server)

A track's complete `view_meta` is assembled by Antiphon, not by any
single node. The server walks the audio graph downstream from the engine
node, collecting `Rule` data from every node in the sub-graph. Pages are
merged by group name:

| Page group | Source |
|---|---|
| SRC | engine node |
| AMP | engine node |
| TRIG | Sequencer node in sub-graph |
| FLTR | first FilterNode in sub-graph |
| FX | DistortionNode, ReverbNode, send params — merged |
| MOD | LFO/modulation nodes (future) |

Params from different nodes on the same page are concatenated with
re-assigned sequential slot numbers. Sub-pages are created when a page
exceeds 8 params.

### 8. Engine-declared pages with default convention

The default convention is the Elektron set: `["SRC", "FLTR", "AMP",
"FX", "MOD", "TRIG"]`. Engines matching this simply declare their page
groups; the view fills downstream pages from sub-graph nodes. Engines
with custom pages (future plugins) declare their own full set. A node
can use 4 defaults + 1 custom page.

### 9. Views are overridable per sub-node

A node's `view_overrides` field provides pre-built `Rule`s for internal
sub-nodes. The override replaces the sub-node's own `Rule` when Antiphon
composes the composite view. This allows an engine to customize how its
downstream nodes appear (e.g. a kick engine with a custom filter view).

## Consequences

- **Third-party extensibility** — a new engine ships a `ViewPlugin`
  implementation. Theoria renders its pages without client-side changes.
- **Reuse** — the downstream pages are assembled once from shared
  FilterNode/DistortionNode/ReverbNode `ViewPlugin`s. No per-engine
  duplication of the filter page.
- **No client hardcoding** — the web client is a generic page-grid
  renderer driven by `view_meta` JSON.
- **Serializable by construction** — `Rule` is `#[derive(Serialize)]`
  plain data. No runtime trait dispatch in the serialization path.
  Antiphon's `view_meta` assembly is a Rust-struct → JSON conversion.
- **Macros make deep engines legible** — the `MacroBinding` mechanism
  collapses complex internals (4-operator FM) into expressive surfaces.
- **Envelope groups fix the ADSR rendering problem** — the renderer has
  the full set of params needed to draw the curve.

### Risks

- **`view_meta` message size** — a full composite view for one track
  might be ~3–5 KB JSON. Sent on track select and machine swap. Over
  localhost/USB-C this is negligible.
- **Sub-graph walk caching** — the walk result must be invalidated on
  `apply_patch`. A stale cache produces a wrong composite view.
- **Stepped param options** — `ParamDescriptor` currently has no
  `options` field. The machine-as-parameter feature needs a way to
  carry option labels. This is an implementation detail for the
  stepped-param representation, not a new ADR concept.

## Alternatives Considered

### A. Fixed pages in the client (rejected)

Hardcode the 6 Elektron page names. Rejected: locks out future plugins.

### B. Multi-method ViewPlugin trait (rejected)

One method per field (`page_groups()`, `param_pages()`, ...) with
`Cow<'static, [Box<...>]>` return types. Rejected: awkward trait
signatures, N method calls to serialize, and the "owned trait objects
in borrowed slices" antipattern. The single `to_rule()` method avoids
all of this.

### C. Module-first navigation (rejected)

Organize by signal-flow node instead of by track. Rejected by the user:
track-first won session #1/#2 on legibility. The chain view provides
module-style navigation as a convenience hop within the track framework.

## Relation to other ADRs

- **ADR-014** — `CapabilityDocument` gains `view: Option<Rule>`.
- **ADR-018** — every component is a node; `ViewPlugin` is the node's
  view contract.
- **ADR-019** — `set_param`/`bump_param` remain the edit path; page
  widgets emit the same commands.
- **ADR-031** — `view_meta` and `get_view_meta` are new Antiphon
  messages.

## Implementation

Staged as 7 commits per `design/phases/w2-interfaces.md`. Commit 0
freezes the `ViewPlugin` trait and `Rule` struct. Commits 1–2 build
node implementations and protocol. Commits 3–6 build the web client.
Commit 7 is the paired session.

**2026-07-13 — Commits 0–2 shipped.**
- **Commit 0** — `Rule`, `ViewPlugin` trait, `PageRef`, `MacroBinding`,
  `MacroCurve`, `AffordanceHint`, `EnvelopeGroup`, `RoutingSemantics` types
  in `paraclete-node-api`; `CapabilityDocument.view: Option<Rule>` field;
  optional `serde` behind `serialize` feature flag.
- **Commit 1** — `ViewPlugin` implemented on 5 nodes: `FilterNode`,
  `AnalogEngine`, `DistortionNode`, `ReverbNode`, `MixNode`. Each declares
  page groups, param placements, and affordances per the Elektron default
  convention.
- **Commit 2** — Antiphon protocol extended: `ClientMsg::GetViewMeta`,
  `ServerMsg::ViewMeta` + 7 supporting structs; composite `ViewRegistry`
  assembly (sub-graph walk, page merging by group name, affordance/envelope/
  macro conversion); `FrameAction::ViewMeta` dispatched on WS I/O thread;
  `NodeSummary.has_view` field.
- All 557 workspace tests pass. ViewRegistry is scaffolding — rules/chains
  are empty stubs until Commit 5 (vertical slice wiring).
