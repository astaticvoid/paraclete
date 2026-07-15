# W2 Phase Report — Theoria Editor

> **Append-only.** Each commit/landmark appends a section. Never rewrite history.
> **Phase:** W2 (`design/phases/w2-interfaces.md`)
> **Goal:** Tablet as editor surface — cap-doc-driven param pages, chain view,
> ADR-032 view-plugin API.

---

## Commit 0 — 2026-07-13

`Rule` + `ViewPlugin` trait in `paraclete-node-api` (L2, LGPL3 boundary).

**Files:**
- `crates/paraclete-node-api/src/rule.rs` — `Rule`, `PageRef`, `MacroBinding`,
  `MacroCurve`, `AffordanceHint`, `EnvelopeGroup`, `RoutingSemantics`,
  `ViewPlugin` trait. Optional serde behind `serialize` feature flag.
- `crates/paraclete-node-api/src/capability.rs` — `CapabilityDocument.view:
  Option<Rule>` field.
- `crates/paraclete-node-api/src/lib.rs` — `pub mod rule`, re-exports.
- `crates/paraclete-node-api/Cargo.toml` — optional serde dep.

**Tests:** 8 unit tests (rule constructors, envelope group, affordance refs,
macro binding lengths, routing semantics, serialization gating).

**Cross-crate impact:** 29 files updated with `view: None,` on all
`CapabilityDocument` construction sites. `cargo fmt` applied.

---

## Commit 1 — 2026-07-13

`ViewPlugin` implementations on 5 L3 nodes.

**Nodes updated:**
| Node | Pages | Affordances |
|------|-------|-------------|
| `FilterNode` | FLTR | FilterShape on cutoff, resonance |
| `AnalogEngine` | SRC, AMP | EnvelopeCurve(AD) on decay, FilterShape on tone |
| `DistortionNode` | FX | None |
| `ReverbNode` | FX | None |
| `MixNode` | FX | None (dynamic per-num_inputs params) |

**Tests:** 3 new ViewPlugin tests across nodes (FilterNode, DistortionNode,
ReverbNode, AnalogEngine). Total nodes tests: 242.

**Findings:**
- `EnvelopeGroup` with `env_type: "AD"` and `param_ids: [decay, 0, 0, 0]` is
  the pragmatic approach for the fixed `[u32; 4]` layout when a drum voice only
  has a decay param (no full ADSR).
- `FilterNode` uses `"cutoff_hz"` instead of ADR-019 canonical `"cutoff"` —
  pre-existing, not introduced here. ViewPlugin references params by ID so it
  is unaffected.
- Blind agent review caught 3 issues (fmt, missing tests, naming); all fixed.

---

## Commit 2 — 2026-07-13

Antiphon `view_meta` protocol + composite assembly (scaffold).

**Files:**
- `crates/paraclete-antiphon/src/protocol.rs` — `ClientMsg::GetViewMeta`,
  `ServerMsg::ViewMeta` + 7 supporting structs (`ViewMetaPage`,
  `ViewMetaParam`, `ViewMetaEnvelope`, `ViewMetaMacro`, `ViewMetaRouting`,
  `ViewMetaChain`, `ViewMetaChainRoute`). `NodeSummary.has_view` field.
- `crates/paraclete-antiphon/src/view.rs` — `ViewRegistry` + `TrackChain` +
  composite assembly (page merging by group name, affordance/envelope/macro
  conversion, param name resolution from `NodeSummary.params`).
- `crates/paraclete-antiphon/src/server.rs` — `FrameAction::ViewMeta`,
  `GetViewMeta` handler in `route_frame`, dispatch on WS I/O thread.
  `SessionInfo.view_registry` field.
- `crates/paraclete-antiphon/src/lib.rs` — `pub mod view`.
- `crates/paraclete-app/src/main.rs` — `has_view: doc.view.is_some()` on
  `NodeSummary`.

**Tests:** Protocol roundtrip tests for `ViewMeta`/`GetViewMeta` (37 total
Antiphon tests pass).

**Scaffold note:** `ViewRegistry` is constructed with empty `rules`/`chains` —
no real data flows yet. Wire to configurator cap-docs + track definitions in
Commit 5 (vertical slice). Also deferred to Commit 5: `stepped`/`options`
population, `engine_diagram` push message, live routing value reads.

**Subagent review** found 2 HIGH items (no tests, stub ViewRegistry) and 4
MEDIUM items (stepped/options, display_name, engine_diagram, wire tag).
Fixed: roundtrip tests, display_name from NodeSummary, `node_labels` switched
from `HashMap` to `Vec<(u32, String)>` for JSON roundtrip.

---

## Commit 4 — 2026-07-14

Param page rendering in theoria-web.

**Files:**
- `web/packages/app/src/views/ParamPage.tsx` — 2-column grid of param cells.
  Each cell shows name + value + value bar. Touch-drag sends `bump_param`.
  Reuses `DragToDetents` from `@paraclete/core`. EnvelopeCurve SVG for ADSR/AD
  shapes with per-envelope-group rendering. Stepped param option hints displayed.
  Macro displays with target list.
- `web/packages/app/src/app.tsx` — renders ParamPage for non-GRID pages.
- `web/packages/app/src/styles.css` — param grid, envelope, cell, bar,
  macro CSS.

**Subagent review** found duplicate DragTracker (fixed — now uses core
DragToDetents), private field access (fixed), missing affordance renderers
(stubbed for Commit 5), EnvelopeCurve ignoring env_group (fixed).

---

## Commit 5 — 2026-07-14

ViewRegistry wired with real data. Kick engine vertical slice.

**Files:**
- `crates/paraclete-app/src/main.rs` — `build_view_registry()`: collects
  `Rule`s from configurator cap-doc cache, builds `TrackChain` per engine,
  passes to `AntiphonServer::spawn`.
- `crates/paraclete-antiphon/src/lib.rs` — `spawn()` accepts `ViewRegistry`
  parameter.
- All 7 engine/effect nodes: `capability_document()` sets `view: Some(...)`
  from `self.to_rule(0, &[])`.

**Critical fix (B1):** `view: None` on every `CapabilityDocument` prevented
any data flow. Fixed by wiring `self.to_rule()` into each node's
`capability_document()` method.

**Type_tag mismatches fixed:** `build_view_registry` now matches
`builder.rs` tags (`"analog_engine:kick"`, `"filter"`, etc.).

**ViewPlugin added to FmEngine and Sampler** (were missing).

---

## Commit 6 — 2026-07-14

Chain view in theoria-web.

**Files:**
- `web/packages/app/src/views/ChainView.tsx` — renders signal-chain nodes in
  order; tap navigates to first available page. Routing destinations shown.
  Uses `view_meta.pages` for node→page navigation.
- `web/packages/app/src/app.tsx` — renders ChainView for CHAIN page.
- `web/packages/app/src/styles.css` — chain node, edge, routing CSS.

**Fix:** Removed broken label-matching logic; nodes now navigate by available
page IDs from view_meta.

---

## Session summary

All 6 implementation commits shipped. ADR-032 accepted. W2 spec ratified.
557 tests pass, workspace compiles clean, web builds clean (24 modules,
36 KB JS, 8 KB CSS).

Remaining for Commit 7 (paired session): user validation on tablet,
spec divergence recording, roadmap delta.

---

## Commit 3 — 2026-07-13

Theoria web client four-zone editor rail layout.

**Files:**
- `web/packages/core/src/protocol.ts` — W2 types: `GetViewMetaMsg`,
  `ViewMetaMsg`, `ViewMetaParam`, `ViewMetaPage`, `ViewMetaEnvelope`,
  `ViewMetaMacro`, `ViewMetaChain`, `ViewMetaChainRoute`. `ClientMsg` and
  `ServerMsg` unions extended.
- `web/packages/app/src/views/PageNav.tsx` — bottom-edge page nav row
  (GRID · TRIG · SRC · AMP · FLTR · FX · MOD · CHAIN). Renders custom
  engine-declared pages beyond the default set. Active page highlighted,
  empty pages dimmed.
- `web/packages/app/src/views/TrackSelect.tsx` — left-edge vertical track
  select column. Each button shows track index + name. Tap selects track +
  sends `get_view_meta` request.
- `web/packages/app/src/app.tsx` — restructured layout: left rail (track
  select), main area (contextual window above encoder row above bottom
  rail). Handles `view_meta` response from server. `activePage` controls
  what renders in the contextual window (GRID = performance surface,
  others = param pages placeholder).
- `web/packages/app/src/styles.css` — four-zone CSS grid replacement for
  the vertical column layout. Left rail 90px, encoder 96px, bottom rail
  auto, center flex-fills.
- `web/packages/app/src/views/TrackBar.tsx` — **removed** (replaced by
  TrackSelect).

**Build:** `npm run build` produces clean bundle (22 modules, 31 KB JS, 5 KB CSS).

**Subagent review** found 2 HIGH (dead TrackBar, custom page nav missing),
2 MEDIUM (spec wire tag discrepancy, node_labels shape), 2 LOW. Fixed:
TrackBar removed, PageNav renders custom pages, `aria-label` for
accessibility.
