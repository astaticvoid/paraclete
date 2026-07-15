// SPDX-License-Identifier: GPL-3.0-or-later
//! ClapParamBridge — translates between CLAP sequential param IDs and
//! Paraclete content-addressed param IDs. See ADR-024.

use paraclete_node_api::{CapabilityDocument, NodeCommand, CMD_SET_PARAM};

/// Translates CLAP parameter IDs (sequential u32 assigned at plugin
/// instantiation) to Paraclete parameter IDs (id_for_name() hash).
///
/// CLAP IDs are stable for the lifetime of a plugin instance and across
/// versions — they are assigned in the order parameters appear in the
/// CapabilityDocument. Parameter additions must be append-only.
pub struct ClapParamBridge {
    entries: Vec<ClapParamEntry>,
}

#[derive(Clone)]
pub struct ClapParamEntry {
    pub clap_id: u32,
    pub paraclete_id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default_val: f64,
}

impl ClapParamBridge {
    /// Empty bridge — no parameters. Used before the capability document is known.
    pub fn empty() -> Self {
        ClapParamBridge { entries: vec![] }
    }

    /// Build from a node's capability document.
    /// CLAP IDs are assigned sequentially (0, 1, 2, …) in parameter declaration order.
    pub fn from_capability_document(doc: &CapabilityDocument) -> Self {
        let entries = doc
            .params
            .iter()
            .enumerate()
            .map(|(clap_id, param)| ClapParamEntry {
                clap_id: clap_id as u32,
                paraclete_id: param.id,
                name: param.name.to_string(),
                min: param.min,
                max: param.max,
                default_val: param.default,
            })
            .collect();
        ClapParamBridge { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up the Paraclete param_id for a CLAP param ID.
    /// Returns None if clap_id is out of range.
    pub fn paraclete_id_for(&self, clap_id: u32) -> Option<u32> {
        self.entries.get(clap_id as usize).map(|e| e.paraclete_id)
    }

    /// Entry for a CLAP param ID. Used when filling CLAP param_info.
    pub fn entry(&self, clap_id: u32) -> Option<&ClapParamEntry> {
        self.entries.get(clap_id as usize)
    }

    /// Build a `CMD_SET_PARAM` NodeCommand from a CLAP param value event.
    /// `target_id` must be set to the target node's ID by the caller.
    pub fn make_set_param_command(
        &self,
        clap_id: u32,
        value: f64,
        target_id: u32,
    ) -> Option<NodeCommand> {
        let paraclete_id = self.paraclete_id_for(clap_id)?;
        Some(NodeCommand {
            target_id,
            type_id: CMD_SET_PARAM,
            arg0: paraclete_id as i64,
            arg1: value,
        })
    }
}
