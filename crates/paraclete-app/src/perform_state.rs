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
use std::sync::Arc;

use crate::kit::{Kit, KitEntry, KitStore};
use crate::sample_pool::SamplePool;
use log::{info, warn};
use paraclete_node_api::app_op::{AppOp, KitId};
use paraclete_node_api::{NodeCommand, SampleData, StateBusHandle, StateBusValue, SwapSample, CMD_SET_PARAM};
use paraclete_nodes::{CMD_RECORDER_ARM_THRESHOLD, CMD_RECORDER_CAPTURE, CMD_RECORDER_DISARM};
use paraclete_runtime::NodeConfigurator;

/// How many kit entries are pushed into the command ring per tick.
const APPLY_CHUNK: usize = 16;

/// R2's frozen `FUNC+9` defaults (tunable) — AN1 D-decisions inherited
/// straight from `design.md` §4's transition-trick spec, not re-derived
/// here.
const TRANSITION_TRICK_BARS: f32 = 2.0;
const TRANSITION_TRICK_SLICES: usize = 16;

/// Context kept between issuing `CMD_RECORDER_CAPTURE` and the fulfilled
/// `RecorderCapture` arriving one or more main-loop ticks later
/// (`an1-anamnesis.md` §2.3) — which sampler (if any) to assign the
/// result to, and whether to apply the transition trick's slice grid.
struct PendingCapture {
    assign_to_sampler: Option<u32>,
    apply_transition_slices: bool,
    /// The bar count the request was issued with — carried through to
    /// `SamplePool::record_capture`'s `bars` parameter. Review finding:
    /// an earlier version of this struct dropped this on the floor
    /// (`record_capture` was always called with `None`), silently
    /// discarding real metadata the pool entry exists to hold.
    bars: f32,
}

pub struct PerformState {
    pub kit_store: KitStore,
    /// Pattern slot → kit binding. Index by active pattern index.
    pub kit_binding: [Option<KitId>; 8],
    /// Project-level sample pool (AN0, ADR-040 decision 3).
    pub sample_pool: SamplePool,
    /// Every live Sampler node's id (AN0 C5) — `AppOp::PoolLoadFile`
    /// broadcasts a `SwapSample` to each of these. Set once by `main.rs`
    /// from `InstrumentIds::samplers` after the graph is built.
    pub sampler_ids: Vec<u32>,
    pub perform_mode: bool,
    /// Volatile temp snapshot (RAM only, not persisted).
    pub temp_param_snapshot: Option<Vec<KitEntry>>,
    /// Cached active_pattern per sequencer node_id.
    cached_active_patterns: HashMap<u32, usize>,
    /// Residual entries still being applied (chunked apply).
    apply_pending: Vec<KitEntry>,
    /// AN1 C4 (§2.3): outstanding `CMD_RECORDER_CAPTURE` requests, keyed
    /// by the `request_id` stamped on the command and echoed back on the
    /// `RecorderCapture` that eventually arrives. No timeout — an entry
    /// for a capture that never completes (the recorder was removed
    /// mid-flight) is inert and harmless, same tolerance
    /// `drain_asset_swaps` already documents for undelivered messages.
    pending_captures: HashMap<u32, PendingCapture>,
    next_request_id: u32,
}

impl PerformState {
    pub fn new() -> Self {
        Self {
            kit_store: KitStore::default(),
            kit_binding: Default::default(),
            sample_pool: SamplePool::default(),
            sampler_ids: Vec::new(),
            perform_mode: false,
            temp_param_snapshot: None,
            cached_active_patterns: HashMap::new(),
            apply_pending: Vec::new(),
            pending_captures: HashMap::new(),
            next_request_id: 0,
        }
    }

    /// Called each main-loop tick with mutable access to conf.
    /// 1. Finishes any pending chunked apply.
    /// 2. Checks for pattern switches → kit-apply trigger.
    /// 3. Publishes /context/perform.
    pub fn tick(&mut self, conf: &mut NodeConfigurator, bus: &Rc<RefCell<StateBusHandle>>) {
        // 1. Continue chunked apply
        self.apply_pending_chunk(conf);

        // 1.5. AN1 C4: collect any capture(s) that finished since the
        // last tick (§2.3) — may be zero, one, or several if the audio
        // thread fulfilled more than one request between ticks.
        self.drain_finished_captures(conf);

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

        // 3.5. AN1 D11: the SAMPLING capability, derived from graph
        // presence — gated on a live "recorder" node existing (a bare
        // AudioInputNode with no recorder can't capture anything, so its
        // presence alone doesn't make the screen useful; simplified from
        // D11's original "input OR recorder" OR-check during C5
        // implementation — see an1-anamnesis.md). Also publishes which
        // node id the recorder is, since it's registry-constructed
        // (unlike AudioInputNode's fixed id) and Theotokos has no other
        // way to discover it.
        let recorder_id = conf
            .all_nodes()
            .find(|(id, _)| conf.type_tag_for(*id) == Some("recorder"))
            .map(|(id, _)| id);
        conf.state_bus_write(
            "/context/sampling_available",
            StateBusValue::Float(if recorder_id.is_some() { 1.0 } else { 0.0 }),
        );
        if let Some(id) = recorder_id {
            conf.state_bus_write("/context/recorder_id", StateBusValue::Int(id as i64));
        }

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
            AppOp::PoolLoadFile { slot, path } => {
                let rate = conf.sample_rate();
                match self.sample_pool.load_file(slot as usize, &path, rate) {
                    Ok((entry, samples)) => {
                        let data = Arc::new(SampleData {
                            samples,
                            rate: entry.rate,
                            frames: entry.frames,
                        });
                        for &node_id in &self.sampler_ids {
                            let msg = SwapSample { node_id, pool_slot: slot, data: data.clone() };
                            if let Err(_msg) = conf.send_swap_sample(msg) {
                                warn!("[pool_load] asset ring full delivering slot {slot} to node {node_id}");
                            }
                        }
                        info!("[pool_load] slot {slot} <- {path} ({} frames)", entry.frames);
                    }
                    Err(e) => warn!("[pool_load] slot {slot}: failed to load {path:?}: {e}"),
                }
            }
            AppOp::RecorderArmThreshold { recorder_id, db } => {
                let cmd = NodeCommand {
                    target_id: recorder_id,
                    type_id: CMD_RECORDER_ARM_THRESHOLD,
                    arg0: 0,
                    arg1: db,
                };
                if conf.send_command(cmd).is_err() {
                    warn!("[recorder] command ring full arming {recorder_id}");
                }
            }
            AppOp::RecorderDisarm { recorder_id } => {
                let cmd = NodeCommand {
                    target_id: recorder_id,
                    type_id: CMD_RECORDER_DISARM,
                    arg0: 0,
                    arg1: 0.0,
                };
                if conf.send_command(cmd).is_err() {
                    warn!("[recorder] command ring full disarming {recorder_id}");
                }
            }
            AppOp::RecorderCapture { recorder_id, bars } => {
                // Plain capture (manual/threshold/retro, design.md §4):
                // populates the pool only. "assign to track N" is a
                // separate, later gesture the performer makes explicitly
                // — unlike PoolLoadFile, this does not broadcast to every
                // live Sampler.
                self.issue_capture(conf, recorder_id, bars, None, false);
            }
            AppOp::CaptureTransition { recorder_id, sampler_id } => {
                // R2's frozen requirement: the FUNC+9 one-gesture chord.
                self.issue_capture(
                    conf,
                    recorder_id,
                    TRANSITION_TRICK_BARS,
                    Some(sampler_id),
                    true,
                );
            }
        }
    }

    /// Shared by `AppOp::RecorderCapture` and `AppOp::CaptureTransition`
    /// (§2.3): converts `bars` to frames against the current tempo,
    /// issues `CMD_RECORDER_CAPTURE`, and registers the request so
    /// `drain_finished_captures` knows what to do once it completes.
    fn issue_capture(
        &mut self,
        conf: &mut NodeConfigurator,
        recorder_id: u32,
        bars: f32,
        assign_to_sampler: Option<u32>,
        apply_transition_slices: bool,
    ) {
        let bpm = match conf.state_bus_read("/transport/bpm") {
            Some(StateBusValue::Float(b)) => b as f32,
            _ => {
                // Review finding: silently guessing a tempo is exactly
                // the "using a fallback, something's off" case CLAUDE.md's
                // logging convention exists for — a capture issued before
                // any clock has published would otherwise span the wrong
                // wall-clock duration with zero trace in the logs.
                warn!(
                    "[recorder] /transport/bpm not yet published — falling back to 120 BPM for \
                     capture on {recorder_id}; the captured span's length may not match {bars} bars"
                );
                120.0
            }
        };
        let seconds = bars * 4.0 * 60.0 / bpm.max(1.0);
        let frames = (seconds * conf.sample_rate()) as usize;
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let cmd = NodeCommand {
            target_id: recorder_id,
            type_id: CMD_RECORDER_CAPTURE,
            arg0: request_id as i64,
            arg1: frames as f64,
        };
        if conf.send_command(cmd).is_err() {
            warn!("[recorder] command ring full issuing capture on {recorder_id}");
            return;
        }
        self.pending_captures.insert(
            request_id,
            PendingCapture { assign_to_sampler, apply_transition_slices, bars },
        );
    }

    /// AN1 C4 (§2.3): collect every `RecorderCapture` the audio thread has
    /// fulfilled since the last call, assemble each into a `SamplePool`
    /// slot, distribute it if the request asked for that (the
    /// transition-trick composite), and return the drained buffer for
    /// reuse. A `request_id` with no matching `pending_captures` entry
    /// (already handled, or not one of ours) still gets its buffer
    /// returned — never left dangling.
    fn drain_finished_captures(&mut self, conf: &mut NodeConfigurator) {
        while let Some(capture) = conf.poll_recorder_capture() {
            let Some(pending) = self.pending_captures.remove(&capture.request_id) else {
                let _ = conf.return_capture_buffer(paraclete_node_api::CaptureBufferReturn {
                    node_id: capture.node_id,
                    buf: capture.buf,
                });
                continue;
            };
            let rate = conf.sample_rate();
            let samples = &capture.buf[..capture.frames];
            let (slot, owned) = self.sample_pool.record_capture(samples, rate, Some(pending.bars));
            if pending.apply_transition_slices {
                if let Some(entry) = self.sample_pool.slots.get_mut(slot).and_then(|s| s.as_mut())
                {
                    entry.slice_table = even_slice_grid(entry.frames, TRANSITION_TRICK_SLICES);
                }
            }
            if let Some(sampler_id) = pending.assign_to_sampler {
                let data = Arc::new(SampleData { samples: owned, rate, frames: capture.frames });
                let msg = SwapSample { node_id: sampler_id, pool_slot: slot as u32, data };
                if let Err(_msg) = conf.send_swap_sample(msg) {
                    warn!("[capture] asset ring full delivering slot {slot} to node {sampler_id}");
                }
                // Caching the buffer (SwapSample, above) is not the same
                // as playing it — the Sampler's `sample` param still
                // selects whatever slot it was pointed at before. "Assign
                // to track N" means the target actually starts playing
                // the new slot, so this must also move its `sample` param
                // (the AN0 pool-indexed param, CMD 33-35's lock target)
                // to the freshly assigned slot.
                let set_sample_cmd = NodeCommand {
                    target_id: sampler_id,
                    type_id: CMD_SET_PARAM,
                    arg0: paraclete_node_api::ParamDescriptor::id_for_name("sample") as i64,
                    arg1: slot as f64,
                };
                if conf.send_command(set_sample_cmd).is_err() {
                    warn!("[capture] command ring full setting sample param on {sampler_id}");
                }
            }
            info!("[capture] slot {slot} <- recorder {} ({} frames)", capture.node_id, capture.frames);
            let _ = conf.return_capture_buffer(paraclete_node_api::CaptureBufferReturn {
                node_id: capture.node_id,
                buf: capture.buf,
            });
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

/// R2's transition trick: `n` evenly-sized slices spanning `[0, frames)`.
/// The last slice absorbs any remainder from integer division (`frames *
/// (i+1) / n` for each boundary, not `frames / n` per slice — this
/// avoids uneven slices dropping a few trailing samples off the pool
/// entry's frame count when `frames` doesn't divide evenly by `n`).
fn even_slice_grid(frames: usize, n: usize) -> Vec<(usize, usize)> {
    if n == 0 || frames == 0 {
        return vec![(0, frames)];
    }
    (0..n).map(|i| (frames * i / n, frames * (i + 1) / n)).collect()
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

    /// AN0 C5: `AppOp::PoolLoadFile` decodes the file into `sample_pool`
    /// and does not panic when broadcasting to `sampler_ids` — the ring
    /// push itself doesn't validate node existence (that's the executor's
    /// job at delivery time, already covered by
    /// paraclete-runtime's asset_plane_tests.rs); this test covers the
    /// app-op handler's own logic, not end-to-end delivery.
    #[test]
    fn pool_load_file_populates_the_pool_and_does_not_panic_broadcasting() {
        let mut conf = NodeConfigurator::new(44100.0, 512);
        conf.add_node(23, Box::new(paraclete_nodes::Sampler::new()));
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();
        perform.sampler_ids = vec![23, 999]; // 999 is not in the graph — must not panic

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/kick.wav");
        perform.execute(
            AppOp::PoolLoadFile { slot: 3, path: path.to_string_lossy().into_owned() },
            &mut conf,
            &bus,
        );

        let entry = perform.sample_pool.get(3).expect("slot 3 should be populated");
        assert!(entry.frames > 0);
        assert_eq!(entry.rate, 44100.0);
    }

    /// AN0 C5 (review finding): the previous version of this test only
    /// checked pool metadata and could pass even if the broadcast loop
    /// were deleted entirely. This one drives a real executor: two
    /// Sampler nodes, each pre-pointed at pool slot 5 (`set_base_sample`),
    /// both silent before the slot is delivered — `PoolLoadFile` must
    /// reach *both* via the asset-delivery plane before either produces
    /// audio on trigger. Mirrors `an0-anamnesis.md` §4's "construct a
    /// small graph with 2 Sampler nodes... assert both nodes'
    /// pool_cache... contain the new slot" via observable behavior
    /// (there's no cross-crate `pool_cache` accessor, so "produces audio"
    /// stands in for "the slot arrived") rather than a field peek.
    #[test]
    fn pool_load_file_delivers_to_every_live_sampler_node_via_the_asset_plane() {
        let sr = 44100.0;
        let block = 64usize;
        let mut conf = NodeConfigurator::new(sr, block);

        let mut s1 = paraclete_nodes::Sampler::new();
        s1.set_base_sample(5);
        conf.add_node(1, Box::new(s1));

        let mut s2 = paraclete_nodes::Sampler::new();
        s2.set_base_sample(5);
        conf.add_node(2, Box::new(s2));

        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();
        perform.sampler_ids = vec![1, 2];

        let mut exec = conf.build_executor();
        let mut out = vec![0.0f32; block * 2];

        // Before delivery: pool slot 5 is uncached on both — silence.
        for &id in &[1u32, 2u32] {
            let _ = conf.send_command(NodeCommand {
                target_id: id,
                type_id: paraclete_node_api::CMD_TRIGGER,
                arg0: -1,
                arg1: 0.0,
            });
        }
        exec.process(&mut out, 2);
        assert!(out.iter().all(|&x| x == 0.0), "uncached slot must be silent before PoolLoadFile");

        // Deliver: execute() sends SwapSample to both node ids; one
        // process() cycle drains the asset-delivery ring into each node's
        // pool_cache (see asset_plane_tests.rs — one cycle is sufficient).
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/kick.wav");
        perform.execute(
            AppOp::PoolLoadFile { slot: 5, path: path.to_string_lossy().into_owned() },
            &mut conf,
            &bus,
        );
        exec.process(&mut out, 2); // drains the SwapSample ring, no trigger this cycle

        // After delivery: trigger both again — both must now produce audio.
        for &id in &[1u32, 2u32] {
            let _ = conf.send_command(NodeCommand {
                target_id: id,
                type_id: paraclete_node_api::CMD_TRIGGER,
                arg0: -1,
                arg1: 0.0,
            });
        }
        out.iter_mut().for_each(|x| *x = 0.0);
        exec.process(&mut out, 2);
        assert!(
            out.iter().any(|&x| x != 0.0),
            "both samplers should be playing slot 5's audio after PoolLoadFile delivers it"
        );
    }

    #[test]
    fn pool_load_file_missing_path_does_not_panic_or_populate() {
        let mut conf = NodeConfigurator::new(44100.0, 512);
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();

        perform.execute(
            AppOp::PoolLoadFile { slot: 3, path: "does/not/exist.wav".into() },
            &mut conf,
            &bus,
        );

        assert!(perform.sample_pool.get(3).is_none());
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
        let ids = crate::builder::build_from_instrument(&def, &mut conf, &HashMap::new(), &mut Default::default())
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

    #[test]
    fn even_slice_grid_covers_the_whole_range_with_no_gaps_or_overlap() {
        let slices = even_slice_grid(100, 16);
        assert_eq!(slices.len(), 16);
        assert_eq!(slices[0].0, 0);
        assert_eq!(slices[15].1, 100);
        for w in slices.windows(2) {
            assert_eq!(w[0].1, w[1].0, "adjacent slices must be contiguous");
        }
    }

    #[test]
    fn even_slice_grid_handles_a_frame_count_not_divisible_by_slice_count() {
        // 100 / 16 = 6 remainder 4 — must still cover exactly [0, 100)
        // with no gaps, not silently drop the remainder.
        let slices = even_slice_grid(97, 16);
        assert_eq!(slices.first().unwrap().0, 0);
        assert_eq!(slices.last().unwrap().1, 97);
    }

    /// AN1 C4 (§2.3): `AppOp::CaptureTransition` end to end through a real
    /// executor — a source Sampler plays a known non-silent sample into a
    /// wired RecorderNode; the transition trick captures it, assembles a
    /// new pool slot with a 16-slot grid, and delivers it to a *different*
    /// target Sampler via SwapSample — mirrors
    /// `pool_load_file_delivers_to_every_live_sampler_node_via_the_asset_plane`'s
    /// silent-before/audio-after proof technique, since that's the only
    /// way to observe "the slot truly arrived" without a cross-crate
    /// pool_cache accessor.
    #[test]
    fn capture_transition_delivers_captured_audio_to_the_target_sampler() {
        let sr = 44100.0;
        let block = 64usize;
        let mut conf = NodeConfigurator::new(sr, block);

        let mut source = paraclete_nodes::Sampler::new();
        source.set_base_sample(5); // will be populated by PoolLoadFile below
        conf.add_node(1, Box::new(source));
        conf.add_node(2, Box::new(paraclete_nodes::RecorderNode::new()));
        // Default-constructed: base_sample 0, uncached — silent until the
        // transition trick both delivers *and* re-points its `sample`
        // param at the newly captured slot.
        conf.add_node(3, Box::new(paraclete_nodes::Sampler::new()));
        conf.connect(1, paraclete_nodes::Sampler::PORT_AUDIO_OUT_L, 2, 0 /* RecorderNode::PORT_AUDIO_IN */)
            .expect("wiring source -> recorder must succeed");

        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();

        let mut exec = conf.build_executor();
        let mut out = vec![0.0f32; block * 2];

        // Load a real, non-silent sample into pool slot 5 and deliver it
        // to the source Sampler (node 1, already pointed at slot 5 via
        // set_base_sample above), then trigger it playing.
        perform.sampler_ids = vec![1]; // must be set before execute() — PoolLoadFile reads it
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/kick.wav");
        perform.execute(
            AppOp::PoolLoadFile { slot: 5, path: path.to_string_lossy().into_owned() },
            &mut conf,
            &bus,
        );
        exec.process(&mut out, 2); // deliver slot 5's SwapSample to node 1

        // The transition trick captures the last TRANSITION_TRICK_BARS
        // bars (at the fallback 120 BPM, no clock node in this minimal
        // graph — 4s = 176400 frames @ 44.1kHz). Retrigger the source
        // periodically across enough cycles to cover that whole window
        // with real, non-silent audio — a single kick decays long before
        // 4s, so one trigger alone would leave most of the captured
        // buffer silent and this test would fail for a scale reason
        // having nothing to do with correctness.
        let trigger_source = || NodeCommand {
            target_id: 1,
            type_id: paraclete_node_api::CMD_TRIGGER,
            arg0: -1,
            arg1: 0.0,
        };
        for i in 0..2800 {
            if i % 50 == 0 {
                let _ = conf.send_command(trigger_source());
            }
            exec.process(&mut out, 2);
        }

        // Target (node 3) is silent before delivery — proves whatever
        // arrives later didn't come from anywhere else.
        let _ = conf.send_command(NodeCommand {
            target_id: 3,
            type_id: paraclete_node_api::CMD_TRIGGER,
            arg0: -1,
            arg1: 0.0,
        });
        exec.process(&mut out, 2);
        assert!(out.iter().all(|&x| x == 0.0), "target sampler must be silent before the transition trick");

        // The transition trick: capture recorder 2's last ~2 bars, assign
        // to sampler 3, with a 16-slot grid.
        perform.execute(AppOp::CaptureTransition { recorder_id: 2, sampler_id: 3 }, &mut conf, &bus);
        exec.process(&mut out, 2); // RecorderNode fulfills CMD_RECORDER_CAPTURE
        perform.tick(&mut conf, &bus); // drains it, assembles the pool slot, sends SwapSample
        exec.process(&mut out, 2); // delivers SwapSample to node 3

        let entry = perform
            .sample_pool
            .slots
            .iter()
            .flatten()
            .find(|e| matches!(&e.source, crate::sample_pool::SampleSource::Ram { .. }))
            .expect("a Ram pool entry must exist after the transition trick");
        assert_eq!(entry.slice_table.len(), 16, "R2's frozen default: a 16-slot even grid");
        assert_eq!(
            entry.bars,
            Some(TRANSITION_TRICK_BARS),
            "review finding: the requested bar count must reach PoolEntry.bars, not be dropped to None"
        );

        let _ = conf.send_command(NodeCommand {
            target_id: 3,
            type_id: paraclete_node_api::CMD_TRIGGER,
            arg0: -1,
            arg1: 0.0,
        });
        exec.process(&mut out, 2);
        assert!(
            out.iter().any(|&x| x != 0.0),
            "target sampler must produce audio after the transition trick delivers the captured slot"
        );
    }

    /// A capture whose `request_id` has no matching `pending_captures`
    /// entry (already handled, or spurious) must still return its buffer
    /// rather than leaving it stuck in the executor forever — verified
    /// indirectly: a second, real capture on the same recorder must still
    /// succeed afterward (proving the return-leg ring wasn't starved of
    /// scratch-buffer turnover by the orphaned one).
    #[test]
    fn drain_finished_captures_returns_the_buffer_for_an_unmatched_request_id() {
        let sr = 44100.0;
        let block = 64usize;
        let mut conf = NodeConfigurator::new(sr, block);
        conf.add_node(2, Box::new(paraclete_nodes::RecorderNode::new()));
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();
        let mut exec = conf.build_executor();
        let mut out = vec![0.0f32; block * 2];

        // A capture command with a request_id perform_state never
        // registered (simulating an already-handled or foreign request).
        let _ = conf.send_command(NodeCommand {
            target_id: 2,
            type_id: paraclete_nodes::CMD_RECORDER_CAPTURE,
            arg0: 12345,
            arg1: 4.0,
        });
        exec.process(&mut out, 2);
        perform.tick(&mut conf, &bus); // must not panic; must return the buffer

        // A real, tracked capture afterward must still work — proves the
        // scratch pool wasn't left exhausted by the untracked one.
        perform.execute(AppOp::RecorderCapture { recorder_id: 2, bars: 0.001 }, &mut conf, &bus);
        exec.process(&mut out, 2);
        perform.tick(&mut conf, &bus);
        let entry = perform
            .sample_pool
            .slots
            .iter()
            .flatten()
            .next()
            .expect("a real capture after an unmatched one must still populate the pool");
        assert_eq!(
            entry.bars,
            Some(0.001),
            "the plain RecorderCapture path (not just CaptureTransition) must also carry bars through"
        );
    }
}
