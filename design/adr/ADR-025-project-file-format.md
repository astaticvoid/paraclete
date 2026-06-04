# ADR-025: Project File Format

**Date:** June 2026
**Status:** Accepted — P7 implementation target

-----

## Decision

Project files are RON (Rusty Object Notation) documents. They capture
the graph topology, each node's serialised state, the active profile script
binding, and the project metadata. State bus values are not persisted —
they are regenerated from node state on load. The format is versioned.
Unknown node state blobs are skipped with a warning rather than failing
the load.

-----

## Context

P7 delivers project save/recall. OQ-2 has been deferred since P3:
*"serialize() for structured state + StateBus snapshot for observable values.
Boundary unspecified. Deferred from P3 — not blocking until P7 save/recall."*

This ADR resolves OQ-2. The boundary decision is: nodes own their state via
`serialize()` / `deserialize()`; the project file is the aggregation point.
The state bus is observable runtime state, not persistent state.

-----

## Why RON

Three options were considered.

**Binary envelope (custom or MessagePack/bincode).** Compact, fast. Opaque
— not human-readable, not diffable, not version-controllable in a meaningful
way. No tooling without a project viewer. Rejected for a hardware-first
instrument where patch management matters.

**TOML.** Human-readable, widespread, good Rust support (toml crate).
Limitations: no native byte array representation (node `serialize()` blobs
are `Vec<u8>`); awkward for recursive structures; table ordering not
guaranteed. Workable but not the natural fit.

**RON.** Rusty Object Notation — designed for Rust data serialization. Natively
expresses Rust struct and enum shapes. Supports byte arrays. Human-readable,
diffable, and version-controllable. Excellent for a Rust project. The `ron`
crate provides serde integration. Chosen.

An alternative to RON would be a TOML envelope with base64-encoded node blobs.
This is noted as a fallback if RON compatibility proves problematic in practice.

-----

## Format

### Top-level structure

```ron
Project(
    version: 1,
    metadata: ProjectMetadata(
        name: "dawless-set-01",
        bpm: 140.0,
        created: "2026-06-15T00:00:00Z",
    ),
    graph: GraphSnapshot(
        nodes: [
            NodeSnapshot(
                id: 10,
                type_name: "InternalClock",
                state: [],
            ),
            NodeSnapshot(
                id: 20,
                type_name: "AnalogEngine::Kick",
                state: [0x02, 0x1a, ...],   // raw bytes from serialize()
            ),
            // ... all registered nodes
        ],
        edges: [
            Edge(src_node: 10, src_port: 0, dst_node: 30, dst_port: 0),
            // ... all graph connections
        ],
    ),
    profiles: ProfileBinding(
        active: [
            "profiles/launchpad.rhai",
            "profiles/digitakt.rhai",
            "profiles/keystep.rhai",
        ],
    ),
)
```

### `NodeSnapshot`

```rust
#[derive(Serialize, Deserialize)]
struct NodeSnapshot {
    id:        u32,        // NodeId — stable across save/load cycles
    type_name: String,     // used for error messages; not for dispatch
    state:     Vec<u8>,    // verbatim output of Node::serialize()
}
```

`type_name` is for human readability and error messages only. Dispatch
on load is by `id` — the project file's node IDs must match the node IDs
in the runtime graph topology that is constructed at startup. The application
layer is responsible for constructing the graph in the same order so IDs
are stable. This is the simplest load model and is correct for a fixed graph
topology (which Paraclete has through P8).

**Node ID stability contract:** Node IDs are assigned at application startup
from a fixed construction sequence. The sequence is documented in `paraclete-app`
and must not change between versions without a project migration step. A
changed construction sequence is treated as a format version bump.

### Versioning

The top-level `version: u32` field governs the overall project file format.
Node state blobs carry their own internal version byte (already established
in `serialize()` contracts for Sampler, Sequencer, etc.). These are independent:
a project file version bump is needed only if the top-level structure changes,
not when individual node state format changes.

**Version 1** is the initial format as described above.

**Unknown node blobs:** On load, if `Node::deserialize()` receives a blob
with an unrecognised version byte, it must silently ignore it and use defaults
(this is already the contract — Sampler v1 data is silently ignored at v2).
The load does not fail; the node starts with defaults and the user is informed
via a startup warning.

**Unknown node IDs:** If the project file contains a node ID that does not
exist in the current runtime graph (e.g. a node was removed in a code update),
that snapshot is skipped with a warning. The remaining nodes load normally.

### What is not persisted

**State bus values.** The state bus is observable runtime state. Values are
published by nodes each process cycle and consumed by subscribers
(profile scripts, UI). On load, nodes start processing and immediately
publish their initial values; the state bus is populated within the first
cycle. No snapshot needed.

**Hardware state.** LED patterns, encoder positions, and MIDI state are
ephemeral. Profile scripts reconstruct hardware state from the state bus
on startup (this is already how they work — see `state_alive()` and
`subscribe()` in the scripting layer).

**Graph topology.** The graph topology (which nodes exist, how they connect)
is fixed at application startup and is not loaded from the project file.
The project file carries edge data for validation only — if the saved edges
differ from the runtime topology, a warning is emitted and the runtime
topology takes precedence. This approach is robust for P7 where the graph
is fixed code; a full topology-from-file model is a P9+ concern.

-----

## Save and Load Path

### Save

```rust
// In paraclete-app, on Cmd+S or equivalent:
fn save_project(path: &Path, conf: &NodeConfigurator) -> Result<(), ProjectError> {
    let nodes: Vec<NodeSnapshot> = conf.all_nodes().map(|(id, node)| {
        NodeSnapshot {
            id,
            type_name: node.type_name().to_string(),
            state: node.serialize(),
        }
    }).collect();

    let edges: Vec<Edge> = conf.all_edges().map(|e| Edge {
        src_node: e.src_node,
        src_port: e.src_port,
        dst_node: e.dst_node,
        dst_port: e.dst_port,
    }).collect();

    let project = Project { version: 1, metadata, graph: GraphSnapshot { nodes, edges },
                            profiles: current_profiles() };
    let ron_str = ron::ser::to_string_pretty(&project, Default::default())?;
    std::fs::write(path, ron_str)?;
    Ok(())
}
```

### Load

```rust
fn load_project(path: &Path, conf: &mut NodeConfigurator) -> Result<(), ProjectError> {
    let ron_str = std::fs::read_to_string(path)?;
    let project: Project = ron::de::from_str(&ron_str)?;

    if project.version != 1 {
        return Err(ProjectError::UnknownVersion(project.version));
    }

    let mut warnings = Vec::new();

    for snapshot in &project.graph.nodes {
        match conf.node_mut(snapshot.id) {
            Some(node) => node.deserialize(&snapshot.state),
            None => warnings.push(format!(
                "Project file references unknown node id {}; skipping.",
                snapshot.id
            )),
        }
    }

    for warning in &warnings {
        eprintln!("WARN: {}", warning);
    }

    Ok(())
}
```

`load_project()` calls `deserialize()` before `activate()` is called for
that cycle. This matches the `Node` trait contract: *"Called on the main
thread before `activate()`."* The audio engine need not be stopped to load
a project; `deserialize()` is called on the main thread, and the updated
state is picked up by the next `process()` cycle. For a running engine, this
produces a near-instantaneous patch change without audio dropout.

-----

## Consequences

**P7:**
- `ron` crate added to workspace dependencies (MIT/Apache-2)
- `ProjectSnapshot`, `NodeSnapshot`, `Edge`, `ProjectMetadata`,
  `ProfileBinding` types added to `paraclete-app` (not to `paraclete-node-api`
  — project format is platform-level, not node-level)
- `NodeConfigurator` gains `all_nodes()` and `all_edges()` iterators
- `Node::type_name()` default implementation added to the trait
  (returns `std::any::type_name::<Self>()` — for display only)
- Save/load wired to a keyboard shortcut (main thread only)
- OQ-2 marked resolved

**P8+:**
- If graph topology becomes dynamic (runtime node add/remove), the
  topology-from-file model will need revisiting
- CLAP plugin state extension (`clap_plugin_state`) maps to
  `Node::serialize()` / `Node::deserialize()` directly — no new
  serialization format needed for CLAP state

**Known limitation:** The fixed-topology load model means project files
are not portable across binary versions that change the node construction
sequence. Version 1 files from one build may not load cleanly into a build
that reorders node registration. This is acceptable through P8; it becomes
an issue at P9 if graph topology becomes user-configurable at runtime.

-----

## References

- ADR-014 — Capability Document (node introspection)
- ADR-010 — State Model (state bus; observable vs persistent state)
- `p0-interfaces.md` — `Node::serialize()` / `Node::deserialize()` contract
- `ron` crate — https://crates.io/crates/ron
- OQ-2 — State bus persistence boundary (now resolved by this ADR)
