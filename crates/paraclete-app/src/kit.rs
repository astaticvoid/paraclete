// SPDX-License-Identifier: GPL-3.0-or-later
//! Kit store: named parameter snapshots that can be applied at any time.
//!
//! A kit is a sorted list of `(node_id, param_id, value)` entries captured
//! from every `in_kit` parameter in the graph. Kits are stored in a fixed
//! 64-slot store and can be bound to sequencer pattern slots so a pattern
//! switch applies the bound kit automatically (perform mode).

use paraclete_node_api::app_op::KitId;
use serde::{Deserialize, Serialize};

/// A saved kit: a name and sorted (node_id, param_id) entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kit {
    /// Max 16 chars, enforced on save.
    pub name: String,
    /// Sorted by (node_id, param_id).
    pub entries: Vec<KitEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct KitEntry {
    pub node_id: u32,
    pub param_id: u32,
    pub value: f64,
}

/// 64-kit store. Slot `None` = empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KitStore {
    pub kits: Vec<Option<Kit>>,
}

impl Default for KitStore {
    fn default() -> Self {
        Self { kits: vec![None; 64] }
    }
}

impl KitStore {
    pub fn get(&self, id: KitId) -> Option<&Kit> {
        self.kits.get(id.0 as usize).and_then(|o| o.as_ref())
    }

    pub fn set(&mut self, id: KitId, kit: Kit) {
        if (id.0 as usize) < self.kits.len() {
            self.kits[id.0 as usize] = Some(kit);
        }
    }
}
