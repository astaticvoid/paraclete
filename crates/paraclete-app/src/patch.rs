// SPDX-License-Identifier: GPL-3.0-or-later
//! Dynamic topology — `apply_patch()` implements the pause-rebuild-resume
//! protocol described in ADR-029.

use std::collections::HashMap;

use paraclete_runtime::{ConnectError, NodeConfigurator};
use paraclete_hal::AudioEngine;

use crate::registry::NodeRegistry;

/// A single topology mutation. Batched into a `Vec` and applied atomically by
/// `apply_patch()`.
#[derive(Debug)]
pub enum TopologyChange {
    /// Add a new node constructed by the registry.
    AddNode {
        type_tag:       String,
        initial_params: HashMap<String, f64>,
    },
    /// Remove an existing node (severs all connected edges).
    RemoveNode { id: u32 },
    /// Add an edge between two existing nodes.
    AddEdge {
        src:      u32,
        src_port: u32,
        dst:      u32,
        dst_port: u32,
    },
    /// Remove a specific edge.
    RemoveEdge {
        src:      u32,
        src_port: u32,
        dst:      u32,
        dst_port: u32,
    },
}

/// Error returned by `apply_patch()`.
#[derive(Debug)]
pub enum PatchError {
    /// A requested type_tag is not registered in the `NodeRegistry`.
    UnknownTypeTag(String),
    /// A node ID referenced by `RemoveNode` or an edge was not found.
    NodeNotFound(u32),
    /// An `AddEdge` would create an unsanctioned cycle.
    CycleError(ConnectError),
    /// A `RemoveEdge` or `RemoveNode` operation failed.
    ConfigError(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::UnknownTypeTag(t)  => write!(f, "unknown type tag: {t}"),
            PatchError::NodeNotFound(id)   => write!(f, "node {id} not found"),
            PatchError::CycleError(e)      => write!(f, "cycle error: {e}"),
            PatchError::ConfigError(s)     => write!(f, "config error: {s}"),
        }
    }
}

impl std::error::Error for PatchError {}

/// Apply a batch of topology changes atomically via pause-rebuild-resume.
///
/// Protocol (ADR-029):
/// 1. `engine.pause()` — signal audio thread to stop after current buffer.
/// 2. `engine.wait_paused()` — block until audio thread confirms (≤ 500 ms).
/// 3. Drain the old executor's nodes back to the configurator.
/// 4. Apply each `TopologyChange` to the configurator.
/// 5. `conf.rebuild_executor()` — build a new executor with fresh channels.
/// 6. `engine.resume_with_executor(executor)` — swap and resume.
///
/// Returns one new node ID per `AddNode` change, in order. If a change fails,
/// the patch aborts at that step. Prior changes in a failed batch are **not**
/// rolled back, and the engine is rebuilt and resumed with them applied —
/// a failed patch must never leave the audio thread stranded paused
/// (BUG-029; each individual change keeps the graph valid, so the partial
/// state is always buildable).
pub fn apply_patch(
    changes:  Vec<TopologyChange>,
    engine:   &AudioEngine,
    conf:     &mut NodeConfigurator,
    registry: &NodeRegistry,
) -> Result<Vec<u32>, PatchError> {
    engine.pause();
    engine.wait_paused();

    // Return nodes from the old executor to the configurator.
    if let Some(old_executor) = engine.take_executor() {
        let nodes = old_executor.drain_nodes();
        conf.restore_nodes(nodes);
    }

    let mut new_ids = Vec::new();
    let mut error: Option<PatchError> = None;

    for change in changes {
        let step = match change {
            TopologyChange::AddNode { type_tag, initial_params } => {
                match registry.build(&type_tag) {
                    Some(mut node) => {
                        node.set_initial_params(&initial_params);
                        let id = conf.add_node_tagged(node, &type_tag);
                        new_ids.push(id);
                        Ok(())
                    }
                    None => Err(PatchError::UnknownTypeTag(type_tag)),
                }
            }
            TopologyChange::RemoveNode { id } => {
                conf.remove_node(id).map(|_| ()).map_err(|msg| {
                    // A known id whose removal was refused (e.g. a surface
                    // device) must surface the reason, not be masked as
                    // "not found". Only a genuinely absent id is NodeNotFound.
                    if conf.contains_node(id) {
                        PatchError::ConfigError(msg)
                    } else {
                        PatchError::NodeNotFound(id)
                    }
                })
            }
            TopologyChange::AddEdge { src, src_port, dst, dst_port } => {
                conf.connect(src, src_port, dst, dst_port)
                    .map(|_| ()).map_err(PatchError::CycleError)
            }
            TopologyChange::RemoveEdge { src, src_port, dst, dst_port } => {
                conf.disconnect(src, src_port, dst, dst_port)
                    .map_err(PatchError::ConfigError)
            }
        };
        if let Err(e) = step {
            error = Some(e);
            break;
        }
    }

    let new_executor = conf.rebuild_executor();
    engine.resume_with_executor(new_executor);

    match error {
        Some(e) => Err(e),
        None    => Ok(new_ids),
    }
}
