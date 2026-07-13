---
name: layer-boundary
description: Use when adding or changing crate dependencies, creating new crates, or auditing imports across paraclete's 5-layer architecture. Enforces that no layer may reach across another.
---

# Layer Boundary Rules

Paraclete enforces strict 5-layer architecture. No layer may reach across
another. Violations produce a dirty runtime dependency graph.

## Layer dependency table

| Layer | Crate | License | May depend on |
|-------|-------|---------|---------------|
| L0 HAL | `paraclete-hal` | GPL3 | L1 (special case: owns audio callback → needs executor) |
| L1 Runtime | `paraclete-runtime` | GPL3 | Nothing above L0 |
| L2 Node API | `paraclete-node-api` | **LGPL3** | Nothing internal — this is the third-party boundary |
| L3 Nodes | `paraclete-nodes` | GPL3 | L2 only |
| L4 Scripting | `paraclete-scripting` | GPL3 | L2, L1 (via StateBusHandle) |

## Platform crate rules

| Crate | Allowed dependencies |
|-------|---------------------|
| `paraclete-graph-nodes` | **Only** crate allowed to depend on both `paraclete-nodes` AND `paraclete-runtime` |
| `paraclete-tui` | `paraclete-node-api` only (no runtime access — uses StateBusHandle) |
| `paraclete-antiphon` | `paraclete-node-api`, `paraclete-scripting` |
| `paraclete-clap` | `paraclete-node-api`, `paraclete-runtime` |
| `paraclete-clap-host` | `paraclete-node-api`, `paraclete-runtime`, `clap-sys`, `libloading` |

## Verification

```bash
# Check that L2 (node-api) has no runtime dependency
cargo tree -p paraclete-node-api -i paraclete-runtime --depth 1
# Should print nothing

# Check that only graph-nodes depends on both nodes and runtime
cargo tree -p paraclete-graph-nodes -i paraclete-nodes,paraclete-runtime --depth 0
```

## Hard constraints from design docs

- **ADR-018:** Every component is a Node — no non-node platform objects
- **ADR-022:** Node portability — L2-only linking for third-party nodes
- **ADR-023:** GraphNode for instrument encapsulation
- **No tokio** — blocking tungstenite + rtrb only
- **No Web MIDI** as primary transport

## Known violations (tracked, not to be replicated)

- `paraclete-hal` depends on `paraclete-runtime` (BUG-003) — ack'd, fixed before LGPL3 publication
