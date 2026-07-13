// SPDX-License-Identifier: GPL-3.0-or-later
//! Project save/recall — RON format. Implements ADR-025.
//!
//! Version 1: node state only (no type_tag, no edge topology).
//! Version 2: adds `type_tag` per node; edges are authoritative on load.

use serde::{Deserialize, Serialize};
use paraclete_runtime::NodeConfigurator;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Top-level project file.
#[derive(Serialize, Deserialize)]
pub struct Project {
    pub version:  u32,
    pub metadata: ProjectMetadata,
    pub graph:    GraphSnapshot,
    pub profiles: ProfileBinding,
}

#[derive(Serialize, Deserialize)]
pub struct ProjectMetadata {
    /// Human-readable project name (filename stem by default).
    pub name:    String,
    /// BPM at save time. Informational only; the InternalClock node's
    /// serialised state is authoritative on load.
    pub bpm:     f32,
    /// ISO 8601 timestamp (UTC) recording when the file was created.
    pub created: String,
}

#[derive(Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub nodes: Vec<NodeSnapshot>,
    pub edges: Vec<EdgeRecord>,
}

/// Per-node serialised state.
#[derive(Serialize, Deserialize)]
pub struct NodeSnapshot {
    /// Stable NodeId — must match the runtime graph's node ID on load.
    pub id:        u32,
    /// Construction key registered in `NodeRegistry`. Empty for v1 files and
    /// for nodes registered via `add_node()` (no tag stored).
    #[serde(default)]
    pub type_tag:  String,
    /// Human-readable label for diagnostics. Not used for dispatch.
    pub type_name: String,
    /// Raw output of `Node::serialize()`. Empty if the node has no state.
    pub state:     Vec<u8>,
}

/// One edge record. Stored for validation (v1) or topology restoration (v2).
#[derive(Serialize, Deserialize)]
pub struct EdgeRecord {
    pub src_node: u32,
    pub src_port: u32,
    pub dst_node: u32,
    pub dst_port: u32,
}

#[derive(Serialize, Deserialize)]
pub struct ProfileBinding {
    /// Paths (relative to project root) of active Rhai profile scripts.
    pub active: Vec<String>,
}

#[derive(Debug)]
pub enum ProjectError {
    Io(std::io::Error),
    Parse(ron::error::SpannedError),
    UnknownVersion(u32),
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::Io(e)              => write!(f, "I/O error: {e}"),
            ProjectError::Parse(e)           => write!(f, "Parse error: {e}"),
            ProjectError::UnknownVersion(v)  => write!(f, "Unknown project version: {v}"),
        }
    }
}

impl From<std::io::Error> for ProjectError {
    fn from(e: std::io::Error) -> Self { ProjectError::Io(e) }
}

impl From<ron::error::SpannedError> for ProjectError {
    fn from(e: ron::error::SpannedError) -> Self { ProjectError::Parse(e) }
}

// ── save / load ───────────────────────────────────────────────────────────────

/// Serialise all node states to a RON project file (version 1).
///
/// Called on the main thread only. Must be called before `build_executor()`
/// (after that call, nodes have been moved to the executor and `all_nodes()`
/// returns nothing).
pub fn save_project(
    path:     &std::path::Path,
    conf:     &NodeConfigurator,
    metadata: ProjectMetadata,
    profiles: ProfileBinding,
) -> Result<(), ProjectError> {
    let nodes: Vec<NodeSnapshot> = conf.all_nodes()
        .map(|(id, node)| NodeSnapshot {
            id,
            type_tag:  conf.type_tag_for(id).unwrap_or("").to_string(),
            type_name: node.type_name().to_string(),
            state:     node.serialize(),
        })
        .collect();

    let edges: Vec<EdgeRecord> = conf.all_edges()
        .map(|e| EdgeRecord {
            src_node: e.src_node,
            src_port: e.src_port,
            dst_node: e.dst_node,
            dst_port: e.dst_port,
        })
        .collect();

    let project = Project {
        version: 1,
        metadata,
        graph: GraphSnapshot { nodes, edges },
        profiles,
    };

    let ron_str = ron::ser::to_string_pretty(&project, ron::ser::PrettyConfig::default())
        .map_err(|e| ProjectError::Io(
            std::io::Error::other(e.to_string())
        ))?;
    std::fs::write(path, ron_str)?;
    Ok(())
}

/// Restore node states from a RON project file (version 1).
///
/// Called on the main thread only. Must be called before `build_executor()`.
/// Node IDs in the file are matched against the runtime graph; unknown IDs
/// are skipped and a warning string is added to the returned list.
/// Returns `Ok(warnings)` — a non-empty warning list is not an error.
pub fn load_project(
    path: &std::path::Path,
    conf: &mut NodeConfigurator,
) -> Result<Vec<String>, ProjectError> {
    let ron_str = std::fs::read_to_string(path)?;
    let project: Project = ron::de::from_str(&ron_str)?;

    if project.version != 1 {
        return Err(ProjectError::UnknownVersion(project.version));
    }

    let mut warnings = Vec::new();

    for snap in &project.graph.nodes {
        match conf.node_mut(snap.id) {
            Some(node) => node.deserialize(&snap.state),
            None => warnings.push(format!(
                "Project file references unknown node id {} ({}); skipping.",
                snap.id, snap.type_name
            )),
        }
    }

    let runtime_edge_count = conf.all_edges().count();
    if runtime_edge_count != project.graph.edges.len() {
        warnings.push(format!(
            "Saved edge count ({}) differs from runtime topology ({}). \
             Runtime topology in use.",
            project.graph.edges.len(),
            runtime_edge_count,
        ));
    }

    Ok(warnings)
}

// ── Version 2 save/load ───────────────────────────────────────────────────────

/// Serialise all node states to a RON project file (version 2).
///
/// Version 2 adds a `type_tag` per node (the `NodeRegistry` key needed to
/// reconstruct the node) and treats the edge list as authoritative on load.
///
/// Called on the main thread only. Must be called before `build_executor()`.
pub fn save_project_v2(
    conf: &NodeConfigurator,
    path: &std::path::Path,
) -> Result<(), ProjectError> {
    let nodes: Vec<NodeSnapshot> = conf.all_nodes()
        .map(|(id, node)| NodeSnapshot {
            id,
            type_tag:  conf.type_tag_for(id).unwrap_or("").to_string(),
            type_name: node.type_name().to_string(),
            state:     node.serialize(),
        })
        .collect();

    let edges: Vec<EdgeRecord> = conf.all_edges()
        .map(|e| EdgeRecord {
            src_node: e.src_node,
            src_port: e.src_port,
            dst_node: e.dst_node,
            dst_port: e.dst_port,
        })
        .collect();

    let project = Project {
        version: 2,
        metadata: ProjectMetadata {
            name:    String::new(),
            bpm:     0.0,
            created: String::new(),
        },
        graph: GraphSnapshot { nodes, edges },
        profiles: ProfileBinding { active: vec![] },
    };

    let ron_str = ron::ser::to_string_pretty(&project, ron::ser::PrettyConfig::default())
        .map_err(|e| ProjectError::Io(
            std::io::Error::other(e.to_string())
        ))?;
    std::fs::write(path, ron_str)?;
    Ok(())
}

/// Restore node states from a RON project file (version 1 or 2).
///
/// - Version 1: nodes are matched by ID against the existing graph; topology
///   is unchanged. A warning is emitted if the saved edge count differs.
/// - Version 2: nodes are reconstructed from `type_tag` via the registry.
///   Edges are restored from the file (authoritative).
///
/// Returns `Ok(warnings)` — a non-empty list is informational, not an error.
pub fn load_project_v2(
    path:     &std::path::Path,
    conf:     &mut NodeConfigurator,
    registry: &crate::registry::NodeRegistry,
) -> Result<Vec<String>, ProjectError> {
    let ron_str = std::fs::read_to_string(path)?;
    let project: Project = ron::de::from_str(&ron_str)?;

    let mut warnings = Vec::new();

    match project.version {
        1 => {
            warnings.push(
                "Project file is version 1; topology will not be restored from the file. \
                 Upgrade by re-saving."
                    .to_string(),
            );
            for snap in &project.graph.nodes {
                match conf.node_mut(snap.id) {
                    Some(node) => node.deserialize(&snap.state),
                    None => warnings.push(format!(
                        "Project file references unknown node id {} ({}); skipping.",
                        snap.id, snap.type_name
                    )),
                }
            }
        }
        2 => {
            // Build nodes from type_tags, then restore their serialised state.
            for snap in &project.graph.nodes {
                if snap.type_tag.is_empty() {
                    warnings.push(format!(
                        "Node id {} ({}) has no type_tag; cannot reconstruct.",
                        snap.id, snap.type_name
                    ));
                    continue;
                }
                match registry.build(&snap.type_tag) {
                    Some(node) => {
                        // add_node activates the node (sets sample_rate/block_size).
                        // deserialize() must follow activate() so ParameterBank values
                        // are applied on top of defaults rather than before the reset.
                        conf.add_node(snap.id, node);
                        conf.set_type_tag_for(snap.id, &snap.type_tag);
                        if let Some(n) = conf.node_mut(snap.id) {
                            n.deserialize(&snap.state);
                        }
                    }
                    None => warnings.push(format!(
                        "Unknown type_tag {:?} for node id {} ({}); skipping.",
                        snap.type_tag, snap.id, snap.type_name
                    )),
                }
            }
            // Restore edges.
            for edge in &project.graph.edges {
                if let Err(e) = conf.connect(
                    edge.src_node, edge.src_port,
                    edge.dst_node, edge.dst_port,
                ) {
                    warnings.push(format!(
                        "Failed to restore edge {}:{} → {}:{}: {e}",
                        edge.src_node, edge.src_port, edge.dst_node, edge.dst_port
                    ));
                }
            }
        }
        v => return Err(ProjectError::UnknownVersion(v)),
    }

    Ok(warnings)
}
