// SPDX-License-Identifier: GPL-3.0-or-later
//! Perform-mode state: kit store, pattern→kit bindings, chunked kit apply.
//!
//! Owned by the app main loop (`main.rs`). Each tick:
//!   1. finishes any in-flight chunked kit apply,
//!   2. watches sequencer `active_pattern` state and applies the kit bound
//!      to a pattern when it changes (perform mode),
//!   3. publishes `/context/perform` for surfaces.
//!
//! `AppOp` commands drained from surfaces (`execute`) drive kit save/load,
//! bind/unbind, and perform-mode toggling. `TempSave`/`TempReload` and
//! `KitCommit`/`KitReload` are C3 stubs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::kit::{Kit, KitEntry, KitStore};
use log::info;
use paraclete_node_api::app_op::{AppOp, KitId};
use paraclete_node_api::{NodeCommand, StateBusHandle, StateBusValue, CMD_SET_PARAM};
use paraclete_runtime::NodeConfigurator;

/// How many kit entries are pushed into the command ring per tick.
const APPLY_CHUNK: usize = 16;

pub struct PerformState {
    pub kit_store: KitStore,
    /// Pattern slot → kit binding. Index by active pattern index.
    pub kit_binding: [Option<KitId>; 8],
    pub perform_mode: bool,
    /// Volatile temp snapshot (RAM only, not persisted).
    pub temp_param_snapshot: Option<Vec<KitEntry>>,
    /// Cached active_pattern per sequencer node_id.
    cached_active_patterns: HashMap<u32, usize>,
    /// Residual entries still being applied (chunked apply).
    apply_pending: Vec<KitEntry>,
}

impl PerformState {
    pub fn new() -> Self {
        Self {
            kit_store: KitStore::default(),
            kit_binding: Default::default(),
            perform_mode: false,
            temp_param_snapshot: None,
            cached_active_patterns: HashMap::new(),
            apply_pending: Vec::new(),
        }
    }

    /// Called each main-loop tick with mutable access to conf.
    /// 1. Finishes any pending chunked apply.
    /// 2. Checks for pattern switches → kit-apply trigger.
    /// 3. Publishes /context/perform.
    pub fn tick(&mut self, conf: &mut NodeConfigurator, bus: &Rc<RefCell<StateBusHandle>>) {
        // 1. Continue chunked apply
        self.apply_pending_chunk(conf);

        // 2. Pattern-switch kit-apply trigger (only outside perform mode —
        //    in perform mode the user has taken manual control).
        if !self.perform_mode {
            self.check_pattern_switches(conf, bus);
        }

        // 3. Publish /context/perform
        conf.state_bus_write(
            "/context/perform",
            StateBusValue::Float(if self.perform_mode { 1.0 } else { 0.0 }),
        );
    }

    /// Execute one AppOp.
    pub fn execute(
        &mut self,
        op: AppOp,
        conf: &mut NodeConfigurator,
        bus: &Rc<RefCell<StateBusHandle>>,
    ) {
        match op {
            AppOp::KitLoad(id) => self.kit_load(id, conf),
            AppOp::KitSaveAs(name) => self.kit_save_as(name, conf, bus),
            AppOp::KitCommit => {
                // C3: needs Theotokos-selected track context.
                info!("[kit] commit — stub, needs selected-track context");
            }
            AppOp::KitReload => {
                // C3: needs Theotokos-selected track context.
                info!("[kit] reload — stub, needs selected-track context");
            }
            AppOp::BindKit { slot, kit } => self.bind_kit(slot, kit),
            AppOp::SetPerformMode(on) => self.set_perform_mode(on),
            AppOp::TempSave => {
                // C3.
                info!("[temp] save — stub");
            }
            AppOp::TempReload => {
                // C3.
                info!("[temp] reload — stub");
            }
        }
    }

    fn apply_pending_chunk(&mut self, conf: &mut NodeConfigurator) {
        if self.apply_pending.is_empty() {
            return;
        }
        let n = APPLY_CHUNK.min(self.apply_pending.len());
        let mut chunk: Vec<KitEntry> = self.apply_pending.drain(..n).collect();
        let mut idx = 0;
        while idx < chunk.len() {
            // KitEntry is Copy; the node command is keyed by param_id
            // (CMD_SET_PARAM, matching ParameterBank::handle_commands).
            let entry = chunk[idx];
            let cmd = NodeCommand {
                target_id: entry.node_id,
                type_id: CMD_SET_PARAM,
                arg0: entry.param_id as i64,
                arg1: entry.value,
            };
            match conf.send_command(cmd) {
                // Ring full — re-queue this entry and everything after it,
                // in original order, and retry next tick.
                Err(_) => {
                    for rest in chunk.drain(idx..).rev() {
                        self.apply_pending.insert(0, rest);
                    }
                    return;
                }
                Ok(()) => {}
            }
            idx += 1;
        }
    }

    fn kit_load(&mut self, id: KitId, _conf: &mut NodeConfigurator) {
        if id.0 >= 64 {
            return;
        }
        if let Some(kit) = self.kit_store.get(id) {
            info!("[kit] loading kit {}: {}", id.0, kit.name);
            self.apply_pending = kit.entries.clone();
        }
    }

    fn kit_save_as(
        &mut self,
        name: String,
        conf: &NodeConfigurator,
        bus: &Rc<RefCell<StateBusHandle>>,
    ) {
        // Char-safe truncation: `name[..16]` would panic on a multi-byte char.
        let name: String = name.chars().take(16).collect();
        let entries = capture_kit_entries(conf, bus);
        // Find first empty slot.
        if let Some(slot) = self.kit_store.kits.iter().position(|k| k.is_none()) {
            self.kit_store.kits[slot] = Some(Kit {
                name: name.clone(),
                entries,
            });
            info!("[kit] saved kit {name} in slot {slot}");
        } else {
            log::warn!("[kit] kit store is full (64/64) — cannot save {name}");
        }
    }

    fn bind_kit(&mut self, slot: usize, kit: Option<KitId>) {
        if slot < 8 {
            self.kit_binding[slot] = kit;
        }
    }

    fn set_perform_mode(&mut self, on: bool) {
        self.perform_mode = on;
    }

    fn check_pattern_switches(
        &mut self,
        conf: &mut NodeConfigurator,
        bus: &Rc<RefCell<StateBusHandle>>,
    ) {
        // For now, scan a fixed range of potential sequencer node ids (10-17).
        // A proper implementation would discover sequencers from cap-docs.
        let bus_ref = bus.borrow();
        for seq_id in 10u32..=17 {
            let path = format!("/node/{seq_id}/state/active_pattern");
            if let Some(StateBusValue::Int(pattern)) = bus_ref.read(&path) {
                let pattern = *pattern as usize;
                let prev = self.cached_active_patterns.get(&seq_id).copied();
                if prev.is_none() {
                    // First observation — seed the cache, don't apply.
                    if pattern < 8 {
                        self.cached_active_patterns.insert(seq_id, pattern);
                    }
                } else if prev != Some(pattern) && pattern < 8 {
                    // Pattern switch detected.
                    self.cached_active_patterns.insert(seq_id, pattern);
                    if let Some(kit_id) = self.kit_binding[pattern] {
                        info!(
                            "[kit] pattern switch seq={seq_id} pattern={pattern} → kit {}",
                            kit_id.0
                        );
                        drop(bus_ref);
                        self.kit_load(kit_id, conf);
                        return;
                    }
                }
            }
        }
    }
}

/// Capture current param values for all in_kit params across all nodes.
fn capture_kit_entries(
    conf: &NodeConfigurator,
    bus: &Rc<RefCell<StateBusHandle>>,
) -> Vec<KitEntry> {
    let mut entries: Vec<KitEntry> = Vec::new();
    let bus_ref = bus.borrow();
    // Scan nodes 1-200 — the app graph node id range.
    for node_id in 1u32..=200 {
        if let Some(doc) = conf.get_node_cap_doc(node_id) {
            for p in &doc.params {
                if p.in_kit {
                    let path = format!("/node/{node_id}/param/{}", p.name.as_str());
                    if let Some(StateBusValue::Float(v)) = bus_ref.read(&path) {
                        entries.push(KitEntry {
                            node_id,
                            param_id: p.id,
                            value: *v,
                        });
                    } else if let Some(StateBusValue::Int(v)) = bus_ref.read(&path) {
                        entries.push(KitEntry {
                            node_id,
                            param_id: p.id,
                            value: *v as f64,
                        });
                    }
                }
            }
        }
    }
    entries.sort_by(|a, b| a.node_id.cmp(&b.node_id).then(a.param_id.cmp(&b.param_id)));
    entries
}
