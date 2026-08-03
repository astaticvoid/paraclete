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

    /// Iterate non-empty slots in slot order as `(slot_index, &Kit)` — the
    /// KIT screen's list source (P11 C6a).
    pub fn iter_nonempty(&self) -> impl Iterator<Item = (usize, &Kit)> {
        self.kits
            .iter()
            .enumerate()
            .filter_map(|(i, o)| o.as_ref().map(|k| (i, k)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P11 C2 spec format: a project's `kits` section is a 64-slot
    /// `Vec<Option<Kit>>` serialized as RON `Some(("Name", [(node, param,
    /// value), ...]))` — must round-trip byte-for-byte so kits survive
    /// save/load. `KitEntry` order is preserved (sorted at capture).
    #[test]
    fn kits_section_ron_round_trip() {
        let mut kits: Vec<Option<Kit>> = vec![None; 64];
        kits[0] = Some(Kit {
            name: "Kick Basic".into(),
            entries: vec![
                KitEntry {
                    node_id: 20,
                    param_id: 3541427549, // decay
                    value: 0.5,
                },
                KitEntry {
                    node_id: 20,
                    param_id: 1234567890,
                    value: 0.3,
                },
            ],
        });
        kits[2] = Some(Kit {
            name: "Snare Tight".into(),
            entries: vec![KitEntry {
                node_id: 21,
                param_id: 3541427549,
                value: 0.7,
            }],
        });

        let ron_str = ron::ser::to_string(&kits).expect("serialize");
        let back: Vec<Option<Kit>> = ron::de::from_str(&ron_str).expect("deserialize");

        assert_eq!(back.len(), 64, "64 slots must survive the round trip");
        let (Some(k0), Some(k2)) = (&back[0], &back[2]) else {
            panic!("filled slots must survive: {:?}", &back[..3]);
        };
        assert_eq!(k0.name, "Kick Basic");
        assert_eq!(k0.entries.len(), 2);
        assert_eq!(k0.entries[0].node_id, 20);
        assert_eq!(k0.entries[0].param_id, 3541427549);
        assert_eq!(k0.entries[0].value, 0.5);
        assert_eq!(k2.name, "Snare Tight");
        // Empty slots stay empty.
        assert!(back[1].is_none() && back[63].is_none());
    }

    /// The `kit_binding` section is `[Option<u8>; 8]` — round-trips too.
    #[test]
    fn kit_binding_ron_round_trip() {
        let binding: [Option<u8>; 8] = [Some(0), None, Some(2), None, None, None, None, None];
        let ron_str = ron::ser::to_string(&binding).expect("serialize");
        let back: [Option<u8>; 8] = ron::de::from_str(&ron_str).expect("deserialize");
        assert_eq!(back, binding);
    }

    /// KitId is u8-constrained by the store's 64 slots; `set` must silently
    /// ignore out-of-range ids rather than panic.
    #[test]
    fn set_ignores_out_of_range_ids() {
        let mut store = KitStore::default();
        store.set(
            KitId(63),
            Kit {
                name: "edge".into(),
                entries: vec![],
            },
        );
        store.set(
            KitId(64),
            Kit {
                name: "must-not-land".into(),
                entries: vec![],
            },
        );
        assert!(store.get(KitId(63)).is_some());
        assert!(store.get(KitId(64)).is_none());
    }
}
