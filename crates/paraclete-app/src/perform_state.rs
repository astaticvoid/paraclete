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
//! bind/unbind, perform-mode toggling, and temp save/reload.
//! `KitCommit`/`KitReload` remain C3 stubs.

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

        // 4. Publish /context/kits — the KIT screen's list source (P11 C6a):
        //    `idx:name` per non-empty slot, semicolon-separated, in slot
        //    order. Empty slots are omitted; the Theotokos resolves which
        //    slot is "loaded" via the selected track's active-pattern
        //    binding (/context/kit_binding).
        let mut kits_line = String::new();
        for (i, kit) in self.kit_store.iter_nonempty() {
            if !kits_line.is_empty() {
                kits_line.push(';');
            }
            let _ = std::fmt::write(&mut kits_line, format_args!("{i}:{}", kit.name));
        }
        conf.state_bus_write("/context/kits", StateBusValue::Text(kits_line));

        // 5. Publish /context/kit_binding — `slot:kit` per bound slot,
        //    semicolon-separated (`slot:-1` = unbound). The KIT screen
        //    marks the slot bound to the selected track's active pattern
        //    as "loaded".
        let mut binding_line = String::new();
        for (slot, kit) in self.kit_binding.iter().enumerate() {
            if !binding_line.is_empty() {
                binding_line.push(';');
            }
            let _ = std::fmt::write(
                &mut binding_line,
                format_args!("{slot}:{}", kit.map_or(-1, |k| k.0 as i32)),
            );
        }
        conf.state_bus_write("/context/kit_binding", StateBusValue::Text(binding_line));
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
            AppOp::KitCommit => self.kit_commit(conf, bus),
            AppOp::KitReload => self.kit_reload(conf, bus),
            AppOp::BindKit { slot, kit } => self.bind_kit(slot, kit),
            AppOp::SetPerformMode(on) => self.set_perform_mode(on),
            AppOp::TempSave => {
                // P11 C3 (ADR-039): snapshot param state and broadcast the
                // engine-side pattern shadow in the same main-loop tick, so
                // both halves of the volatile snapshot are taken together.
                // 1. Capture param state into the temp snapshot (same
                //    cap-doc/bus-read as kit capture).
                self.temp_param_snapshot = Some(capture_kit_entries(conf, bus));
                // 2. Broadcast CMD_TEMP_SAVE to every sequencer (node ids
                //    10-17 — the app graph's sequencer range).
                for seq_id in 10u32..=17 {
                    let cmd = NodeCommand {
                        target_id: seq_id,
                        type_id: paraclete_node_api::command::CMD_TEMP_SAVE as u32,
                        arg0: 0,
                        arg1: 0.0,
                    };
                    if conf.send_command(cmd).is_err() {
                        log::warn!("[temp_save] ring full sending to seq {seq_id}");
                    }
                }
                info!("[temp_save] snapshot saved");
            }
            AppOp::TempReload => {
                // P11 C3: broadcast the engine-side restore, then replay the
                // param snapshot. The pattern restore lands at the node; the
                // params replay chunked (16/tick) via apply_pending.
                // 1. Broadcast CMD_TEMP_RELOAD to every sequencer.
                for seq_id in 10u32..=17 {
                    let cmd = NodeCommand {
                        target_id: seq_id,
                        type_id: paraclete_node_api::command::CMD_TEMP_RELOAD as u32,
                        arg0: 0,
                        arg1: 0.0,
                    };
                    if conf.send_command(cmd).is_err() {
                        log::warn!("[temp_reload] ring full sending to seq {seq_id}");
                    }
                }
                // 2. Replay the param snapshot (if any was saved).
                if let Some(entries) = self.temp_param_snapshot.clone() {
                    self.apply_pending = entries;
                }
                info!("[temp_reload] snapshot reloaded");
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

    /// P11 C2c (the piece C3 deferred): capture the current in_kit param
    /// state back into the kit bound to the Theotokos-selected track's
    /// active pattern slot. The selection is published to
    /// `/script/theotokos/selected` (Int = sequencer id) whenever the
    /// performer changes track; the active pattern is `/node/{id}/state/
    /// active_pattern` (Int = slot). No binding → no-op with a log.
    fn kit_commit(&mut self, conf: &NodeConfigurator, bus: &Rc<RefCell<StateBusHandle>>) {
        let Some(kit_id) = self.bound_kit_for_selected_track(bus) else {
            return;
        };
        let entries = capture_kit_entries(conf, bus);
        // Keep the kit's existing name; only the params are re-captured.
        let name = self
            .kit_store
            .get(kit_id)
            .map(|k| k.name.clone())
            .unwrap_or_else(|| format!("Kit {}", kit_id.0 + 1));
        let entry_count = entries.len();
        self.kit_store.set(kit_id, Kit { name, entries });
        info!(
            "[kit] committed {} params into kit {}",
            entry_count, kit_id.0
        );
    }

    /// P11 C2c: re-apply the kit bound to the Theotokos-selected track's
    /// active pattern slot (the same resolution as `kit_commit`).
    fn kit_reload(&mut self, conf: &mut NodeConfigurator, bus: &Rc<RefCell<StateBusHandle>>) {
        let Some(kit_id) = self.bound_kit_for_selected_track(bus) else {
            return;
        };
        self.kit_load(kit_id, conf);
    }

    /// Resolve the kit bound to the Theotokos-selected track's active
    /// pattern slot: `/script/theotokos/selected` (sequencer id) →
    /// `/node/{id}/state/active_pattern` (pattern index) →
    /// `kit_binding[pattern]`. `None` when any hop is missing (no
    /// selection, no active-pattern publication, or an unbound slot).
    fn bound_kit_for_selected_track(
        &self,
        bus: &Rc<RefCell<StateBusHandle>>,
    ) -> Option<KitId> {
        let seq_id = match bus.borrow().read("/script/theotokos/selected") {
            Some(StateBusValue::Int(i)) if *i >= 0 => *i as u32,
            _ => return None,
        };
        let slot = match bus
            .borrow()
            .read(&format!("/node/{seq_id}/state/active_pattern"))
        {
            Some(StateBusValue::Int(i)) if *i >= 0 => *i as usize,
            _ => return None,
        };
        if slot >= self.kit_binding.len() {
            return None;
        }
        self.kit_binding[slot]
    }

    fn kit_save_as(
        &mut self,
        name: String,
        conf: &NodeConfigurator,
        bus: &Rc<RefCell<StateBusHandle>>,
    ) {
        // Char-safe truncation: `name[..16]` would panic on a multi-byte char.
        let name = truncate_kit_name(&name);
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

/// Truncate a kit name to the 16-char limit at a char boundary (P11 C2 —
/// a byte slice would panic mid-UTF-8).
pub fn truncate_kit_name(name: &str) -> String {
    name.chars().take(16).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::NodeCommand;
    use paraclete_runtime::NodeConfigurator;

    #[test]
    fn truncate_kit_name_caps_at_16_chars() {
        assert_eq!(truncate_kit_name("short"), "short");
        assert_eq!(truncate_kit_name(&"x".repeat(20)), "x".repeat(16));
    }

    #[test]
    fn truncate_kit_name_never_panics_mid_utf8() {
        // 17 chars; the 16-char boundary lands right after a multi-byte
        // char. A byte slice (`name[..16]`) would split ド and panic; the
        // char-wise truncation must stop cleanly on it.
        let name = format!("{}xドy", "a".repeat(14)); // 14 + 1 + 1 + 1 = 17 chars; ド is the 16th
        let t = truncate_kit_name(&name);
        assert_eq!(t.chars().count(), 16);
        assert_eq!(t.chars().last(), Some('ド'));
        assert!(t.is_char_boundary(t.len()));
    }

    #[test]
    fn kit_load_rejects_ids_out_of_range() {
        let mut conf = NodeConfigurator::new(44100.0, 512);
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();
        perform.kit_store.set(
            KitId(0),
            Kit {
                name: "k".into(),
                entries: vec![KitEntry {
                    node_id: 20,
                    param_id: 1,
                    value: 0.7,
                }],
            },
        );

        // KitId ≥ 64 is outside the store — must be a no-op, not a panic
        // or an out-of-bounds index.
        perform.execute(AppOp::KitLoad(KitId(64)), &mut conf, &bus);
        assert!(perform.apply_pending.is_empty());
    }

    /// Build the default instrument graph (real sequencer + engines with
    /// in_kit params) and the selected-track context (`/script/theotokos/
    /// selected` → the first sequencer, active pattern 0) with kit 0 bound
    /// to pattern slot 0 — the context the P11 C2c commit/reload ops
    /// resolve against.
    fn test_graph_with_selection_and_binding() -> (
        NodeConfigurator,
        Rc<RefCell<StateBusHandle>>,
        PerformState,
    ) {
        let instrument = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../instrument.yaml");
        let def =
            crate::builder::load_instrument_definition(&instrument).expect("load instrument.yaml");
        let mut conf = NodeConfigurator::new(44100.0, 512);
        let ids = crate::builder::build_from_instrument(&def, &mut conf, &HashMap::new())
            .expect("build graph");
        let seq_id = ids.sequencers[0];
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();
        perform.kit_binding[0] = Some(KitId(0));
        bus.borrow_mut()
            .write("/script/theotokos/selected", StateBusValue::Int(seq_id as i64));
        bus.borrow_mut()
            .write(&format!("/node/{seq_id}/state/active_pattern"), StateBusValue::Int(0));
        (conf, bus, perform)
    }

    #[test]
    fn kit_commit_captures_into_bound_kit() {
        let (mut conf, bus, mut perform) = test_graph_with_selection_and_binding();
        // A kit in slot 0 first, so commit has a name to keep.
        perform.kit_store.set(
            KitId(0),
            Kit {
                name: "LiveKick".into(),
                entries: vec![],
            },
        );
        // The graph's nodes are un-activated in a unit test (no executor),
        // so their published bus values are absent — simulate the live
        // graph's `/node/{id}/param/{name}` mirrors for one in_kit param.
        bus.borrow_mut()
            .write("/node/20/param/decay", StateBusValue::Float(0.42));

        perform.execute(AppOp::KitCommit, &mut conf, &bus);

        let kit = perform.kit_store.get(KitId(0)).expect("kit 0 exists");
        assert_eq!(kit.name, "LiveKick", "commit keeps the existing name");
        assert!(
            !kit.entries.is_empty(),
            "commit must capture the graph's in_kit params"
        );
        assert!(
            kit.entries
                .iter()
                .any(|e| e.node_id == 20 && e.value == 0.42),
            "the simulated decay value must be captured"
        );
    }

    #[test]
    fn kit_commit_noop_without_binding() {
        let (mut conf, bus, mut perform) = test_graph_with_selection_and_binding();
        // Unbind slot 0 → nothing to commit into.
        perform.kit_binding[0] = None;

        perform.execute(AppOp::KitCommit, &mut conf, &bus);
        assert!(
            perform.kit_store.get(KitId(0)).is_none(),
            "no binding → no kit created"
        );
    }

    #[test]
    fn kit_reload_applies_bound_kit() {
        let (mut conf, bus, mut perform) = test_graph_with_selection_and_binding();
        // A kit whose entries set the kick's decay.
        perform.kit_store.set(
            KitId(0),
            Kit {
                name: "LiveKick".into(),
                entries: vec![KitEntry {
                    node_id: 20,
                    param_id: paraclete_node_api::ParamDescriptor::id_for_name("decay"),
                    value: 0.7,
                }],
            },
        );

        perform.execute(AppOp::KitReload, &mut conf, &bus);
        assert_eq!(
            perform.apply_pending.len(),
            1,
            "reload must queue the bound kit's entries for apply"
        );
    }

    #[test]
    fn chunked_apply_retries_until_ring_drains() {
        let mut conf = NodeConfigurator::new(44100.0, 512);
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();

        // 40 entries > APPLY_CHUNK (16): the apply needs several ticks.
        let entries: Vec<KitEntry> = (0..40)
            .map(|i| KitEntry {
                node_id: 1,
                param_id: i as u32,
                value: 0.5,
            })
            .collect();
        perform.kit_store.set(
            KitId(0),
            Kit {
                name: "big".into(),
                entries: entries.clone(),
            },
        );

        // Fill the command ring so the first chunk cannot be sent.
        let mut filled = 0;
        while conf
            .send_command(NodeCommand {
                target_id: 0,
                type_id: 0,
                arg0: 0,
                arg1: 0.0,
            })
            .is_ok()
        {
            filled += 1;
        }
        assert!(filled > 0, "test premise: ring must actually fill");

        perform.execute(AppOp::KitLoad(KitId(0)), &mut conf, &bus);
        assert_eq!(perform.apply_pending.len(), 40);

        // Tick with the ring still full: nothing may be lost, the whole
        // chunk is re-queued in order (Amd 4 — retry, never drop).
        perform.tick(&mut conf, &bus);
        assert_eq!(perform.apply_pending.len(), 40, "ring-full must re-queue");

        // Drain the ring the way the executor would, then tick until the
        // apply completes (16/tick).
        let mut executor = conf.build_executor();
        let mut block = vec![0.0f32; 512 * 2];
        for _ in 0..16 {
            executor.process(&mut block, 2);
            perform.tick(&mut conf, &bus);
        }
        assert!(
            perform.apply_pending.is_empty(),
            "apply must resume and finish once the ring drains"
        );
    }

    #[test]
    fn pattern_switch_apply_fires_outside_perform_mode_only() {
        let mut conf = NodeConfigurator::new(44100.0, 512);
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();
        perform.kit_store.set(
            KitId(0),
            Kit {
                name: "k".into(),
                entries: vec![KitEntry {
                    node_id: 20,
                    param_id: 1,
                    value: 0.7,
                }],
            },
        );
        perform.execute(
            AppOp::BindKit {
                slot: 0,
                kit: Some(KitId(0)),
            },
            &mut conf,
            &bus,
        );

        let write_pattern = |conf: &mut NodeConfigurator, p: i64| {
            conf.state_bus_write(
                "/node/10/state/active_pattern",
                StateBusValue::Int(p),
            );
        };

        // First observation seeds the cache — no apply on startup.
        write_pattern(&mut conf, 0);
        perform.tick(&mut conf, &bus);
        assert!(perform.apply_pending.is_empty());

        // Pattern 1 is unbound — no apply.
        write_pattern(&mut conf, 1);
        perform.tick(&mut conf, &bus);
        assert!(perform.apply_pending.is_empty());

        // Back to pattern 0 — the bound kit must apply.
        write_pattern(&mut conf, 0);
        perform.tick(&mut conf, &bus);
        assert_eq!(perform.apply_pending.len(), 1);
        perform.tick(&mut conf, &bus);
        assert!(perform.apply_pending.is_empty());

        // Perform mode ON: the same switch must NOT apply.
        perform.execute(AppOp::SetPerformMode(true), &mut conf, &bus);
        write_pattern(&mut conf, 1);
        perform.tick(&mut conf, &bus);
        write_pattern(&mut conf, 0);
        perform.tick(&mut conf, &bus);
        assert!(
            perform.apply_pending.is_empty(),
            "perform mode must suppress kit-apply on pattern switch"
        );
    }
}
