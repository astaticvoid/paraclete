// SPDX-License-Identifier: GPL-3.0-or-later
//! Launch Control XL 8-track drum machine controller (`LcxlDrumController`).
//!
//! Provides an out-of-the-box drum machine surface over up to 8 tracks:
//! - **8 Faders (`id: 24..=31`):** Per-track mixer level (`MixNode` `input_gain_0..7`).
//! - **3×8 Pots (`id: 0..=23`):**
//!   - Normal layer: Row 1 = Pitch/Tune, Row 2 = Decay/End, Row 3 = Filter Cutoff / Timbre.
//!   - FX layer (`Device` button `id: 16`): Row 1 = Distortion Drive / Voice Drive,
//!     Row 2 = Filter Resonance / Sweep, Row 3 = Master Reverb (`wet`, `room_size`, `damping`, `dry`).
//! - **Bottom 16 Buttons (`id: 0..=15`):**
//!   - `BottomMode::StepSequencer` (default): 16-step drum sequencer for the selected track
//!     (Amber = active step, Green = playhead, Red = active step under playhead).
//!   - `BottomMode::TrackControl` (toggled via `Mute` button `id: 17`):
//!     Top row `0..=7` selects track (or live-triggers drum voices when `Record Arm` `id: 19`
//!     is latched); Bottom row `8..=15` toggles track mute (immediate or pattern-wrap prepared
//!     when `Solo` `id: 18` is latched).
//! - **Directional Buttons (`id: 20..=23`):**
//!   - `Left`/`Right` (`22`/`23`): Select previous/next track (bidirectionally synced with
//!     `/script/theotokos/selected` on the StateBus).
//!   - `Up`/`Down` (`20`/`21`): Select sequencer step page (`0..=3`).

use std::collections::HashMap;

use paraclete_node_api::command::CMD_PREPARE_MUTE;
use paraclete_node_api::{
    LedUpdate, NodeCommand, ParamDescriptor, RgbColor, StateBusValue, SurfaceEvent, SurfaceOutput,
    CMD_BUMP_PARAM_NORM, CMD_SET_PARAM, CMD_SET_PARAM_NORM,
};
use paraclete_runtime::NodeConfigurator;

use crate::builder::InstrumentIds;
use crate::instrument::{LcxlDef, ParamTargetDef};

/// Sequencer command type IDs (`paraclete-nodes::Sequencer`).
const CMD_TOGGLE_STEP: u32 = 16;
const CMD_SET_LOCK_TARGET: u32 = 33;
const CMD_SET_STEP_LOCK: u32 = 34;
const CMD_TRIG_NOW: u32 = 38;

/// Internal clock command type IDs (`paraclete-nodes::InternalClock`).
const CMD_CLOCK_START: u32 = 16;
const CMD_CLOCK_STOP: u32 = 17;
const CMD_CLOCK_REWIND: u32 = 18;

/// Surface node ID reserved for Launch Control XL.
pub const ID_LCXL: u32 = 105;

/// Parameter synchronization policy for non-motorized pots (per SPIKE-006 / #96).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Physical pot position is authoritative (1:1 normalized `[0.0, 1.0]` mapped to `[min, max]`).
    HardwareAuthoritative,
    /// Physical pot motion generates relative deltas from its last observed position.
    PseudoRelative,
}

/// Operating mode for the 2×8 bottom button matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomMode {
    /// Buttons `0..=15` toggle steps `1..=16` (on `active_page`) of `active_track`.
    StepSequencer,
    /// Top row `0..=7` selects/triggers tracks `1..=8`; bottom row `8..=15` mutes tracks `1..=8`.
    TrackControl,
}

/// Target `(node_id, param_id)` resolved for a knob in a track column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnobTarget {
    pub node_id: u32,
    pub param_id: u32,
}

/// Per-track resolved knob targets for fast lookup.
#[derive(Clone, Copy, Debug)]
struct TrackTargets {
    seq_id: u32,
    mute_param_id: u32,
    gain_param_id: u32,
    row1_normal: KnobTarget,
    row2_normal: KnobTarget,
    row3_normal: KnobTarget,
    row1_fx: KnobTarget,
    row2_fx: KnobTarget,
}

fn resolve_param_id(conf: &NodeConfigurator, node_id: u32, param_name: &str) -> u32 {
    conf.get_node_cap_doc(node_id)
        .and_then(|doc| {
            doc.params
                .iter()
                .find(|p| p.name.as_str() == param_name)
                .map(|p| p.id)
        })
        .unwrap_or_else(|| ParamDescriptor::id_for_name(param_name))
}

/// Stateful controller bridging `LcxlNode` surface events, `NodeConfigurator`, and `StateBus`.
pub struct LcxlDrumController {
    clock_id: u32,
    mix_id: u32,
    tracks: Vec<TrackTargets>,
    master_fx_row: [Option<KnobTarget>; 8],
    active_track: usize,
    active_page: usize,
    bottom_mode: BottomMode,
    fx_layer: bool,
    prepare_mute_mode: bool,
    trigger_latch: bool,
    mute_held: bool,
    mute_action_during_hold: bool,
    knob_policy: SyncPolicy,
    held_step: Option<usize>,
    held_step_was_active: bool,
    held_step_locked: bool,
    last_fader_norm: [Option<f64>; 32],
    last_leds: [RgbColor; 48],
    leds_dirty: bool,
}

impl LcxlDrumController {
    /// Build a controller bound to the active instrument graph in `conf` using graph discovery.
    pub fn new(ids: &InstrumentIds, conf: &mut NodeConfigurator) -> Self {
        Self::from_definition(ids, None, conf)
    }

    /// Build a controller bound to the active instrument graph, applying any declarative
    /// `lcxl:` knob/FX mappings from the `InstrumentDefinition` and falling back to
    /// edge-derived track chain discovery when omitted.
    pub fn from_definition(
        ids: &InstrumentIds,
        lcxl_def: Option<&LcxlDef>,
        conf: &mut NodeConfigurator,
    ) -> Self {
        let edges: Vec<(u32, u32, u32)> = conf
            .all_edges()
            .map(|e| (e.src_node, e.dst_node, e.dst_port))
            .collect();
        let resolve_def = |def: &ParamTargetDef| KnobTarget {
            node_id: def.node,
            param_id: resolve_param_id(conf, def.node, &def.param),
        };

        // Follow primary event edge (dst_port == 0) from `seq_id` to find its target generator,
        // falling back to positional `ids.generators[col]`.
        let mut tracks = Vec::with_capacity(ids.sequencers.len().min(8));
        for (col, &seq_id) in ids.sequencers.iter().take(8).enumerate() {
            let gen_id = edges
                .iter()
                .find(|(src, _, dst_port)| *src == seq_id && *dst_port == 0)
                .map(|(_, dst, _)| *dst)
                .or_else(|| ids.generators.get(col).copied())
                .unwrap_or(0);

            let gen_tag = conf.type_tag_for(gen_id).unwrap_or("analog_engine");

            // Discover insert filter and distortion by walking downstream edges from `gen_id`
            // until `ids.mix` or `ids.output`, falling back to `ids.filters`/`ids.distortions`.
            let mut filter_id = None;
            let mut dist_id = None;
            let mut cur = gen_id;
            for _ in 0..8 {
                let Some(&(_, next, _)) = edges
                    .iter()
                    .find(|(src, dst, _)| *src == cur && *dst != ids.mix && *dst != ids.output)
                else {
                    break;
                };
                match conf.type_tag_for(next) {
                    Some("filter" | "ladder_filter") if filter_id.is_none() => {
                        filter_id = Some(next);
                    }
                    Some("distortion") if dist_id.is_none() => {
                        dist_id = Some(next);
                    }
                    _ => {}
                }
                cur = next;
            }
            if filter_id.is_none() {
                filter_id = ids.filters.get(col).copied();
            }
            if dist_id.is_none() {
                dist_id = ids.distortions.get(col).copied();
            }

            let (p1, p2, p3, fx1, fx2) = match gen_tag {
                "sampler" => ("pitch", "end", "start", "loop_enable", "pitch"),
                "fm_engine" | "fm_engine:kick" | "fm_engine:bell" | "fm_engine:bass" => {
                    ("tune", "decay", "index1", "feedback", "ratio1")
                }
                "fm_voice" => ("tune", "decay", "timbre", "color", "attack"),
                _ => ("tune", "decay", "tone", "drive", "sweep"),
            };

            let default_r1 = KnobTarget {
                node_id: gen_id,
                param_id: resolve_param_id(conf, gen_id, p1),
            };
            let default_r2 = KnobTarget {
                node_id: gen_id,
                param_id: resolve_param_id(conf, gen_id, p2),
            };
            let default_r3 = if let Some(fid) = filter_id {
                KnobTarget {
                    node_id: fid,
                    param_id: resolve_param_id(conf, fid, "cutoff"),
                }
            } else {
                KnobTarget {
                    node_id: gen_id,
                    param_id: resolve_param_id(conf, gen_id, p3),
                }
            };

            let default_fx1 = if let Some(did) = dist_id {
                KnobTarget {
                    node_id: did,
                    param_id: resolve_param_id(conf, did, "drive"),
                }
            } else {
                KnobTarget {
                    node_id: gen_id,
                    param_id: resolve_param_id(conf, gen_id, fx1),
                }
            };
            let default_fx2 = if let Some(fid) = filter_id {
                KnobTarget {
                    node_id: fid,
                    param_id: resolve_param_id(conf, fid, "resonance"),
                }
            } else {
                KnobTarget {
                    node_id: gen_id,
                    param_id: resolve_param_id(conf, gen_id, fx2),
                }
            };

            let track_override = lcxl_def.and_then(|d| d.tracks.get(col));
            let row1_normal = track_override
                .and_then(|t| t.normal.first())
                .map(&resolve_def)
                .unwrap_or(default_r1);
            let row2_normal = track_override
                .and_then(|t| t.normal.get(1))
                .map(&resolve_def)
                .unwrap_or(default_r2);
            let row3_normal = track_override
                .and_then(|t| t.normal.get(2))
                .map(&resolve_def)
                .unwrap_or(default_r3);
            let row1_fx = track_override
                .and_then(|t| t.fx.first())
                .map(&resolve_def)
                .unwrap_or(default_fx1);
            let row2_fx = track_override
                .and_then(|t| t.fx.get(1))
                .map(&resolve_def)
                .unwrap_or(default_fx2);

            let gain_name = format!("input_gain_{col}");
            tracks.push(TrackTargets {
                seq_id,
                mute_param_id: resolve_param_id(conf, seq_id, "mute"),
                gain_param_id: resolve_param_id(conf, ids.mix, &gain_name),
                row1_normal,
                row2_normal,
                row3_normal,
                row1_fx,
                row2_fx,
            });
        }

        let mut master_fx_row: [Option<KnobTarget>; 8] = [None; 8];
        if let Some(defs) = lcxl_def.filter(|d| !d.master_fx.is_empty()) {
            for (col, def) in defs.master_fx.iter().take(8).enumerate() {
                master_fx_row[col] = Some(resolve_def(def));
            }
        } else {
            // Auto-discover master Reverb (cols 0..4) and Delay (cols 4..8) by type_tag.
            let known: Vec<u32> = conf.known_node_ids().collect();
            let reverb_id = known
                .iter()
                .copied()
                .find(|&id| conf.type_tag_for(id) == Some("reverb"));
            if let Some(rid) = reverb_id {
                for (i, name) in ["wet", "room_size", "damping", "dry"].into_iter().enumerate() {
                    master_fx_row[i] = Some(KnobTarget {
                        node_id: rid,
                        param_id: resolve_param_id(conf, rid, name),
                    });
                }
            }
            let delay_id = known
                .iter()
                .copied()
                .find(|&id| conf.type_tag_for(id) == Some("delay"));
            if let Some(did) = delay_id {
                for (i, name) in ["wet", "delay_time_ms", "feedback", "filter_hz"]
                    .into_iter()
                    .enumerate()
                {
                    master_fx_row[4 + i] = Some(KnobTarget {
                        node_id: did,
                        param_id: resolve_param_id(conf, did, name),
                    });
                }
            }
        }

        let ctrl = Self {
            clock_id: ids.clock,
            mix_id: ids.mix,
            tracks,
            master_fx_row,
            active_track: 0,
            active_page: 0,
            bottom_mode: BottomMode::StepSequencer,
            fx_layer: false,
            prepare_mute_mode: false,
            trigger_latch: false,
            mute_held: false,
            mute_action_during_hold: false,
            knob_policy: SyncPolicy::HardwareAuthoritative,
            held_step: None,
            held_step_was_active: false,
            held_step_locked: false,
            last_fader_norm: [None; 32],
            last_leds: [RgbColor::OFF; 48],
            leds_dirty: true,
        };
        ctrl.publish_surface_state(conf);
        ctrl
    }

    pub fn active_track(&self) -> usize {
        self.active_track
    }

    pub fn active_page(&self) -> usize {
        self.active_page
    }

    pub fn bottom_mode(&self) -> BottomMode {
        self.bottom_mode
    }

    pub fn fx_layer(&self) -> bool {
        self.fx_layer
    }

    pub fn set_knob_policy(&mut self, policy: SyncPolicy) {
        self.knob_policy = policy;
    }

    /// Resolve the `(node_id, param_id)` target for a knob (`id` in `0..24`).
    pub fn resolve_knob_target(&self, id: u32) -> Option<KnobTarget> {
        let row = (id / 8) as usize;
        let col = (id % 8) as usize;
        if self.fx_layer && row == 2 {
            return self.master_fx_row.get(col).copied().flatten();
        }

        let t = self.tracks.get(col)?;
        Some(match (self.fx_layer, row) {
            (false, 0) => t.row1_normal,
            (false, 1) => t.row2_normal,
            (false, _) => t.row3_normal,
            (true, 0) => t.row1_fx,
            (true, _) => t.row2_fx,
        })
    }

    /// Map a normalized `[0.0, 1.0]` knob value to the physical range declared on `target`.
    /// Returns `None` if the parameter is an identity parameter (`machine`).
    fn physical_value_for_norm(conf: &NodeConfigurator, target: KnobTarget, norm: f64) -> Option<f64> {
        let doc = conf.get_node_cap_doc(target.node_id)?;
        let param = doc.params.iter().find(|p| p.id == target.param_id)?;
        if param.name.as_str() == "machine" {
            return None;
        }
        Some(param.norm_to_value(norm))
    }

    /// Handle a single `SurfaceEvent` from `LcxlNode` and emit any resulting `NodeCommand`s.
    pub fn handle_event(&mut self, ev: SurfaceEvent, conf: &mut NodeConfigurator) {
        self.sync_selected_track_from_bus(conf);

        match ev {
            SurfaceEvent::FaderMoved { id, value } => {
                let norm = (value as f64 / 65535.0).clamp(0.0, 1.0);
                let prev = self
                    .last_fader_norm
                    .get(id as usize)
                    .copied()
                    .flatten();
                if let Some(slot) = self.last_fader_norm.get_mut(id as usize) {
                    *slot = Some(norm);
                }

                if (24..32).contains(&id) {
                    // Channel Fader 0..7 -> MixNode input_gain_N (always HardwareAuthoritative)
                    let col = (id - 24) as usize;
                    if let Some(t) = self.tracks.get(col) {
                        let _ = conf.send_command(NodeCommand {
                            target_id: self.mix_id,
                            type_id: CMD_SET_PARAM_NORM,
                            arg0: t.gain_param_id as i64,
                            arg1: norm,
                        });
                    }
                } else if id < 24 {
                    let Some(target) = self.resolve_knob_target(id) else {
                        return;
                    };

                    // Hold-step + knob turn parameter lock authoring (#190 / #127):
                    // In StepSequencer mode, holding any of the 16 step buttons while turning
                    // a knob locks that parameter on the held step instead of mutating the base track.
                    if self.bottom_mode == BottomMode::StepSequencer {
                        if let Some(step) = self.held_step {
                            if let (Some(t), Some(phys)) = (
                                self.tracks.get(self.active_track),
                                Self::physical_value_for_norm(conf, target, norm),
                            ) {
                                let _ = conf.send_command(NodeCommand {
                                    target_id: t.seq_id,
                                    type_id: CMD_SET_LOCK_TARGET,
                                    arg0: target.node_id as i64,
                                    arg1: target.param_id as f64,
                                });
                                let _ = conf.send_command(NodeCommand {
                                    target_id: t.seq_id,
                                    type_id: CMD_SET_STEP_LOCK,
                                    arg0: step as i64,
                                    arg1: phys,
                                });
                                self.held_step_locked = true;
                                self.leds_dirty = true;
                                return;
                            }
                        }
                    }

                    match self.knob_policy {
                        SyncPolicy::HardwareAuthoritative => {
                            let _ = conf.send_command(NodeCommand {
                                target_id: target.node_id,
                                type_id: CMD_SET_PARAM_NORM,
                                arg0: target.param_id as i64,
                                arg1: norm,
                            });
                        }
                        SyncPolicy::PseudoRelative => {
                            if let Some(p) = prev {
                                let delta = norm - p;
                                if delta.abs() > 1e-6 {
                                    let _ = conf.send_command(NodeCommand {
                                        target_id: target.node_id,
                                        type_id: CMD_BUMP_PARAM_NORM,
                                        arg0: target.param_id as i64,
                                        arg1: delta,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            SurfaceEvent::ButtonPressed { id } => {
                match id {
                    // Bottom 16 buttons: Step Sequencer or Track Control
                    0..=15 => {
                        let idx = id as usize;
                        if self.mute_held {
                            // Momentary hold-to-mute on ANY pad column (col = idx % 8)
                            let col = idx % 8;
                            if let Some(t) = self.tracks.get(col) {
                                let currently_muted = Self::read_track_muted(conf, t.seq_id);
                                let next_mute = if currently_muted { 0.0 } else { 1.0 };
                                if self.prepare_mute_mode {
                                    let _ = conf.send_command(NodeCommand {
                                        target_id: t.seq_id,
                                        type_id: CMD_PREPARE_MUTE as u32,
                                        arg0: if currently_muted { 0 } else { 1 },
                                        arg1: 0.0,
                                    });
                                } else {
                                    let _ = conf.send_command(NodeCommand {
                                        target_id: t.seq_id,
                                        type_id: CMD_SET_PARAM,
                                        arg0: t.mute_param_id as i64,
                                        arg1: next_mute,
                                    });
                                }
                                self.mute_action_during_hold = true;
                                self.leds_dirty = true;
                                return;
                            }
                        }
                        match self.bottom_mode {
                            BottomMode::StepSequencer => {
                                if let Some(t) = self.tracks.get(self.active_track) {
                                    let step = self.active_page * 16 + idx;
                                    let mask = Self::read_step_mask(conf, t.seq_id);
                                    let is_active = (mask >> step) & 1 != 0;
                                    self.held_step = Some(step);
                                    self.held_step_was_active = is_active;
                                    self.held_step_locked = false;
                                    if !is_active {
                                        // Activate step immediately on press; holding it now allows p-locking
                                        let _ = conf.send_command(NodeCommand {
                                            target_id: t.seq_id,
                                            type_id: CMD_TOGGLE_STEP,
                                            arg0: step as i64,
                                            arg1: 0.0,
                                        });
                                    }
                                }
                            }
                            BottomMode::TrackControl => {
                                if idx < 8 {
                                    // Top row 0..7: Select track or live-trigger
                                    if idx < self.tracks.len() {
                                        self.active_track = idx;
                                        conf.state_bus_write(
                                            "/script/theotokos/selected",
                                            StateBusValue::Int(self.tracks[idx].seq_id as i64),
                                        );
                                        if self.trigger_latch {
                                            let _ = conf.send_command(NodeCommand {
                                                target_id: self.tracks[idx].seq_id,
                                                type_id: CMD_TRIG_NOW,
                                                arg0: 0,
                                                arg1: 0.85,
                                            });
                                        }
                                    }
                                } else {
                                    // Bottom row 8..15: Mute / Prepare-mute track 0..7
                                    let col = idx - 8;
                                    if let Some(t) = self.tracks.get(col) {
                                        let currently_muted =
                                            Self::read_track_muted(conf, t.seq_id);
                                        let next_mute = if currently_muted { 0.0 } else { 1.0 };
                                        if self.prepare_mute_mode {
                                            let _ = conf.send_command(NodeCommand {
                                                target_id: t.seq_id,
                                                type_id: CMD_PREPARE_MUTE as u32,
                                                arg0: if currently_muted { 0 } else { 1 },
                                                arg1: 0.0,
                                            });
                                        } else {
                                            let _ = conf.send_command(NodeCommand {
                                                target_id: t.seq_id,
                                                type_id: CMD_SET_PARAM,
                                                arg0: t.mute_param_id as i64,
                                                arg1: next_mute,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Device button (16): Toggle FX knob layer
                    16 => {
                        self.fx_layer = !self.fx_layer;
                        self.publish_surface_state(conf);
                    }
                    // Mute button (17): Momentary hold-to-mute or mode toggle
                    17 => {
                        self.mute_held = true;
                        self.mute_action_during_hold = false;
                    }
                    // Solo button (18): Toggle Immediate vs Pattern-Wrap Prepared Mute
                    18 => {
                        self.prepare_mute_mode = !self.prepare_mute_mode;
                        self.publish_surface_state(conf);
                    }
                    // Record Arm button (19): In StepSequencer mode, immediate trigger of active track;
                    // in TrackControl mode, latches live-trigger pad mode on top row 0..7.
                    19 => {
                        match self.bottom_mode {
                            BottomMode::StepSequencer => {
                                if let Some(t) = self.tracks.get(self.active_track) {
                                    let _ = conf.send_command(NodeCommand {
                                        target_id: t.seq_id,
                                        type_id: CMD_TRIG_NOW,
                                        arg0: 0,
                                        arg1: 0.85,
                                    });
                                }
                            }
                            BottomMode::TrackControl => {
                                self.trigger_latch = !self.trigger_latch;
                                self.publish_surface_state(conf);
                            }
                        }
                    }
                    // Up button (20): Stop / Rewind transport (or previous step page in FX layer)
                    20 => {
                        if self.fx_layer {
                            if self.active_page > 0 {
                                self.active_page -= 1;
                                self.publish_surface_state(conf);
                            }
                        } else {
                            let _ = conf.send_command(NodeCommand {
                                target_id: self.clock_id,
                                type_id: CMD_CLOCK_STOP,
                                arg0: 0,
                                arg1: 0.0,
                            });
                            let _ = conf.send_command(NodeCommand {
                                target_id: self.clock_id,
                                type_id: CMD_CLOCK_REWIND,
                                arg0: 0,
                                arg1: 0.0,
                            });
                        }
                    }
                    // Down button (21): Play / Pause transport (or next step page in FX layer)
                    21 => {
                        if self.fx_layer {
                            if self.active_page < 3 {
                                self.active_page += 1;
                                self.publish_surface_state(conf);
                            }
                        } else {
                            let playing = matches!(
                                conf.state_bus_read("/transport/playing"),
                                Some(StateBusValue::Bool(true))
                            );
                            let _ = conf.send_command(NodeCommand {
                                target_id: self.clock_id,
                                type_id: if playing { CMD_CLOCK_STOP } else { CMD_CLOCK_START },
                                arg0: 0,
                                arg1: 0.0,
                            });
                        }
                    }
                    // Left button (22): Previous drum track
                    22 if !self.tracks.is_empty() => {
                        self.active_track = if self.active_track == 0 {
                            self.tracks.len() - 1
                        } else {
                            self.active_track - 1
                        };
                        conf.state_bus_write(
                            "/script/theotokos/selected",
                            StateBusValue::Int(self.tracks[self.active_track].seq_id as i64),
                        );
                        self.publish_surface_state(conf);
                    }
                    // Right button (23): Next drum track
                    23 if !self.tracks.is_empty() => {
                        self.active_track = (self.active_track + 1) % self.tracks.len();
                        conf.state_bus_write(
                            "/script/theotokos/selected",
                            StateBusValue::Int(self.tracks[self.active_track].seq_id as i64),
                        );
                        self.publish_surface_state(conf);
                    }
                    _ => {}
                }
                self.leds_dirty = true;
            }
            SurfaceEvent::ButtonReleased { id } => {
                match id {
                    0..=15 if self.bottom_mode == BottomMode::StepSequencer && !self.mute_held => {
                        let step = self.active_page * 16 + (id as usize);
                        if self.held_step == Some(step) {
                            if self.held_step_was_active && !self.held_step_locked {
                                if let Some(t) = self.tracks.get(self.active_track) {
                                    let _ = conf.send_command(NodeCommand {
                                        target_id: t.seq_id,
                                        type_id: CMD_TOGGLE_STEP,
                                        arg0: step as i64,
                                        arg1: 0.0,
                                    });
                                }
                            }
                            self.held_step = None;
                            self.held_step_was_active = false;
                            self.held_step_locked = false;
                            self.leds_dirty = true;
                        }
                    }
                    17 => {
                        self.mute_held = false;
                        if !self.mute_action_during_hold {
                            // Quick tap with no pads touched: toggle persistent mode
                            self.bottom_mode = match self.bottom_mode {
                                BottomMode::StepSequencer => BottomMode::TrackControl,
                                BottomMode::TrackControl => BottomMode::StepSequencer,
                            };
                            self.publish_surface_state(conf);
                        }
                        self.leds_dirty = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Synchronize `active_track` with `/script/theotokos/selected` on the StateBus
    /// and deliver incremental LED updates to `ID_LCXL` when state changes.
    pub fn tick_leds(&mut self, conf: &mut NodeConfigurator) {
        self.sync_selected_track_from_bus(conf);
        let desired = self.compute_leds(conf);
        let mut updates = Vec::new();
        for (i, (&new_c, old_c)) in desired.iter().zip(self.last_leds.iter_mut()).enumerate() {
            if self.leds_dirty || new_c != *old_c {
                *old_c = new_c;
                updates.push(LedUpdate {
                    control_id: i as u32,
                    color: new_c,
                });
            }
        }
        self.leds_dirty = false;
        if !updates.is_empty() {
            let mut map = HashMap::with_capacity(1);
            map.insert(
                ID_LCXL,
                SurfaceOutput {
                    led_updates: updates,
                    display_updates: Vec::new(),
                },
            );
            conf.deliver_script_output(map);
        }
    }

    /// Compute the 48 LED colors from current controller state and StateBus values.
    pub fn compute_leds(&self, conf: &NodeConfigurator) -> [RgbColor; 48] {
        let mut leds = [RgbColor::OFF; 48];
        let amber = RgbColor {
            r: 255,
            g: 128,
            b: 0,
        };
        let yellow = RgbColor {
            r: 180,
            g: 255,
            b: 0,
        };
        let green_dim = RgbColor { r: 0, g: 64, b: 0 };

        let show_track_control = self.mute_held || self.bottom_mode == BottomMode::TrackControl;
        if !show_track_control {
            if let Some(t) = self.tracks.get(self.active_track) {
                let mask = Self::read_step_mask(conf, t.seq_id);
                let playhead = Self::read_current_step(conf, t.seq_id);
                let base = self.active_page * 16;
                for (i, led) in leds.iter_mut().enumerate().take(16) {
                    let step = base + i;
                    let is_active = (mask >> step) & 1 != 0;
                    let is_head = playhead == Some(step);
                    *led = match (is_head, is_active) {
                        (true, true) => RgbColor::RED,
                        (true, false) => RgbColor::GREEN,
                        (false, true) => amber,
                        (false, false) => RgbColor::OFF,
                    };
                }
            }
        } else {
            for (col, t) in self.tracks.iter().enumerate().take(8) {
                    leds[col] = if col == self.active_track {
                        RgbColor::GREEN
                    } else if self.trigger_latch {
                        yellow
                    } else {
                        amber
                    };

                    let muted = Self::read_track_muted(conf, t.seq_id);
                    let pending = Self::read_pending_mute(conf, t.seq_id);
                    leds[8 + col] = if pending.is_some() {
                        amber
                    } else if muted {
                        RgbColor::RED
                    } else {
                        green_dim
                    };
                }
        }

        // Mode buttons 16..19
        leds[16] = if self.fx_layer { amber } else { RgbColor::OFF };
        leds[17] = match self.bottom_mode {
            BottomMode::StepSequencer => {
                if self.mute_held {
                    amber
                } else {
                    RgbColor::OFF
                }
            }
            BottomMode::TrackControl => amber,
        };
        leds[18] = if self.prepare_mute_mode {
            amber
        } else {
            RgbColor::OFF
        };
        leds[19] = if self.trigger_latch {
            yellow
        } else {
            RgbColor::OFF
        };

        // Nav / Transport buttons 20..23 (single Red LEDs)
        let is_playing = matches!(
            conf.state_bus_read("/transport/playing"),
            Some(StateBusValue::Bool(true))
        );
        leds[20] = if self.fx_layer {
            if self.active_page > 0 {
                RgbColor::RED
            } else {
                RgbColor::OFF
            }
        } else if !is_playing {
            RgbColor::RED // Red when stopped (Stop indicator)
        } else {
            RgbColor::OFF
        };
        leds[21] = if self.fx_layer {
            if self.active_page < 3 {
                RgbColor::RED
            } else {
                RgbColor::OFF
            }
        } else if is_playing {
            RgbColor::RED // Red when playing (Play indicator)
        } else {
            RgbColor::OFF
        };
        leds[22] = RgbColor::RED;
        leds[23] = RgbColor::RED;

        // Knob LEDs 24..47 (3 rows x 8 cols)
        for row in 0..3 {
            for col in 0..8 {
                let led_idx = 24 + row * 8 + col;
                if self.fx_layer {
                    if row == 2 {
                        leds[led_idx] = if self.master_fx_row.get(col).copied().flatten().is_some() {
                            amber
                        } else {
                            RgbColor::OFF
                        };
                    } else if col < self.tracks.len() {
                        leds[led_idx] = RgbColor::RED;
                    }
                } else if col == self.active_track {
                    leds[led_idx] = RgbColor::GREEN;
                } else if col < self.tracks.len() {
                    leds[led_idx] = amber;
                }
            }
        }

        leds
    }

    fn sync_selected_track_from_bus(&mut self, conf: &mut NodeConfigurator) {
        if let Some(StateBusValue::Int(sel)) = conf.state_bus_read("/script/theotokos/selected") {
            let resolved = self
                .tracks
                .iter()
                .position(|t| t.seq_id as i64 == sel)
                .or_else(|| {
                    let idx = sel.max(0) as usize;
                    (idx < self.tracks.len()).then_some(idx)
                });
            if let Some(idx) = resolved {
                if idx != self.active_track {
                    self.active_track = idx;
                    self.leds_dirty = true;
                    self.publish_surface_state(conf);
                }
            }
        }
    }

    fn publish_surface_state(&self, conf: &mut NodeConfigurator) {
        let mode_str = match self.bottom_mode {
            BottomMode::StepSequencer => "STEP",
            BottomMode::TrackControl => "TRACK",
        };
        conf.state_bus_write(
            "/surface/105/mode",
            StateBusValue::Text(mode_str.to_string()),
        );
        conf.state_bus_write("/surface/105/fx_layer", StateBusValue::Bool(self.fx_layer));
        conf.state_bus_write(
            "/surface/105/page",
            StateBusValue::Int(self.active_page as i64),
        );
        conf.state_bus_write(
            "/surface/105/selected_track",
            StateBusValue::Int(self.active_track as i64),
        );
    }

    fn read_step_mask(conf: &NodeConfigurator, seq_id: u32) -> u64 {
        match conf.state_bus_read(&format!("/node/{seq_id}/state/steps")) {
            Some(StateBusValue::Text(s)) => {
                let mut mask = 0u64;
                for (i, ch) in s.chars().take(64).enumerate() {
                    if ch == '1' {
                        mask |= 1u64 << i;
                    }
                }
                mask
            }
            Some(StateBusValue::Int(v)) => v as u64,
            _ => 0,
        }
    }

    fn read_current_step(conf: &NodeConfigurator, seq_id: u32) -> Option<usize> {
        let playing = matches!(
            conf.state_bus_read("/transport/playing"),
            Some(StateBusValue::Bool(true))
        );
        if !playing {
            return None;
        }
        match conf.state_bus_read(&format!("/node/{seq_id}/state/current_step")) {
            Some(StateBusValue::Int(v)) if v >= 0 => Some(v as usize),
            _ => None,
        }
    }

    fn read_track_muted(conf: &NodeConfigurator, seq_id: u32) -> bool {
        match conf.state_bus_read(&format!("/node/{seq_id}/param/mute")) {
            Some(StateBusValue::Float(v)) => v >= 0.5,
            Some(StateBusValue::Bool(b)) => b,
            _ => false,
        }
    }

    fn read_pending_mute(conf: &NodeConfigurator, seq_id: u32) -> Option<bool> {
        match conf.state_bus_read(&format!("/node/{seq_id}/state/pending_mute")) {
            Some(StateBusValue::Int(0)) => Some(false),
            Some(StateBusValue::Int(1)) => Some(true),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{build_from_instrument, load_instrument_definition};
    use crate::sample_pool::SamplePool;
    use std::path::Path;

    fn setup_8track() -> (NodeConfigurator, InstrumentIds, LcxlDrumController) {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("instrument-8track.yaml");
        let def = load_instrument_definition(&manifest).expect("load instrument-8track.yaml");
        let mut conf = NodeConfigurator::new(48_000.0, 512);
        let libs = HashMap::new();
        let mut pool = SamplePool::default();
        let ids =
            build_from_instrument(&def, &mut conf, &libs, &mut pool).expect("build 8-track graph");
        let ctrl = LcxlDrumController::from_definition(&ids, def.lcxl.as_ref(), &mut conf);
        (conf, ids, ctrl)
    }

    #[test]
    fn x0x_lcxl_bundle_loads_and_applies_declarative_knob_mappings() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let bundle_path = root.join("instruments/x0x-lcxl");
        let def = load_instrument_definition(&bundle_path).expect("load instruments/x0x-lcxl");
        assert_eq!(def.name, "x0x-lcxl-drum-machine");
        assert!(def.lcxl.is_some(), "bundle must include declarative lcxl block");

        let mut conf = NodeConfigurator::new(48_000.0, 512);
        let libs = HashMap::new();
        let mut pool = SamplePool::default();
        let ids =
            build_from_instrument(&def, &mut conf, &libs, &mut pool).expect("build x0x-lcxl graph");
        let ctrl = LcxlDrumController::from_definition(&ids, def.lcxl.as_ref(), &mut conf);

        // Col 1 Row 3 (knob id = 16) in normal mode is mapped to Filter cutoff (node 40, param "cutoff")
        let r3_col0 = ctrl.resolve_knob_target(16).expect("Col 1 Row 3 target");
        assert_eq!(r3_col0.node_id, 40);
        assert_eq!(r3_col0.param_id, resolve_param_id(&conf, 40, "cutoff"));

        // Col 3 Row 1 (knob id = 2) is Clap tone (node 22, param "tone")
        let r1_col2 = ctrl.resolve_knob_target(2).expect("Col 3 Row 1 target");
        assert_eq!(r1_col2.node_id, 22);
        assert_eq!(r1_col2.param_id, ParamDescriptor::id_for_name("tone"));

        // Col 6 Row 1 (knob id = 5) is Tom tune (node 25, param "tune")
        let r1_col5 = ctrl.resolve_knob_target(5).expect("Col 6 Row 1 target");
        assert_eq!(r1_col5.node_id, 25);
        assert_eq!(r1_col5.param_id, ParamDescriptor::id_for_name("tune"));
    }

    #[test]
    fn step_sequencer_mode_toggles_steps_on_selected_track_and_updates_leds() {
        let (mut conf, ids, mut ctrl) = setup_8track();
        assert_eq!(ctrl.bottom_mode(), BottomMode::StepSequencer);
        assert_eq!(ctrl.active_track(), 0);

        let mut exec = conf.build_executor();
        let mut out_buf = [0.0f32; 1024];

        // Press bottom button 0 -> toggles step 0 on Track 1 (seq 10)
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 0 }, &mut conf);
        // Press bottom button 4 -> toggles step 4 on Track 1 (seq 10)
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 4 }, &mut conf);

        exec.process(&mut out_buf, 2);
        conf.process_main_thread();

        let mask = LcxlDrumController::read_step_mask(&conf, ids.sequencers[0]);
        assert_eq!(mask & 1, 1, "step 0 should be active");
        assert_eq!((mask >> 4) & 1, 1, "step 4 should be active");

        let leds = ctrl.compute_leds(&conf);
        let amber = RgbColor {
            r: 255,
            g: 128,
            b: 0,
        };
        assert_eq!(leds[0], amber);
        assert_eq!(leds[4], amber);
        assert_eq!(leds[1], RgbColor::OFF);
    }

    #[test]
    fn faders_and_knobs_modulate_track_levels_voice_params_and_fx_layer() {
        let (mut conf, ids, mut ctrl) = setup_8track();
        let mut exec = conf.build_executor();
        let mut out_buf = [0.0f32; 1024];

        // Move Fader 0 (id 24) to 100% (65535) -> MixNode input_gain_0 = 2.0
        ctrl.handle_event(
            SurfaceEvent::FaderMoved {
                id: 24,
                value: 65535,
            },
            &mut conf,
        );
        // Move Row 1 Knob 0 (id 0) to 100% -> Track 0 AnalogEngine tune max
        ctrl.handle_event(
            SurfaceEvent::FaderMoved {
                id: 0,
                value: 65535,
            },
            &mut conf,
        );
        exec.process(&mut out_buf, 2);
        conf.process_main_thread();

        let Some(StateBusValue::Float(gain0)) =
            conf.state_bus_read(&format!("/node/{}/param/input_gain_0", ids.mix))
        else {
            panic!("expected MixNode input_gain_0 on StateBus");
        };
        assert!(
            (gain0 - 2.0).abs() < 1e-4,
            "full fader should set gain to max 2.0, got {gain0}"
        );

        let Some(StateBusValue::Float(tune0)) =
            conf.state_bus_read(&format!("/node/{}/param/tune", ids.generators[0]))
        else {
            panic!("expected AnalogEngine tune on StateBus");
        };
        assert!(
            tune0 > 0.9,
            "full Row 1 knob should set tune near max, got {tune0}"
        );

        // Toggle Device button (id 16) -> FX layer active -> Row 3 Knob 0 (id 16) controls Reverb wet (id 200)
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 16 }, &mut conf);
        assert!(ctrl.fx_layer());
        ctrl.handle_event(
            SurfaceEvent::FaderMoved {
                id: 16,
                value: 32768,
            },
            &mut conf,
        );
        exec.process(&mut out_buf, 2);
        conf.process_main_thread();

        let Some(StateBusValue::Float(rev_wet)) = conf.state_bus_read("/node/200/param/wet") else {
            panic!("expected Reverb wet on StateBus");
        };
        assert!(
            (rev_wet - 0.5).abs() < 0.02,
            "FX layer Row 3 Knob 0 should set reverb wet to ~0.5, got {rev_wet}"
        );
    }

    #[test]
    fn track_control_mode_selects_and_mutes_tracks_and_syncs_with_theotokos() {
        let (mut conf, ids, mut ctrl) = setup_8track();
        let mut exec = conf.build_executor();
        let mut out_buf = [0.0f32; 1024];

        // Tap Mute mode button (id 17) -> enters TrackControl mode
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 17 }, &mut conf);
        ctrl.handle_event(SurfaceEvent::ButtonReleased { id: 17 }, &mut conf);
        assert_eq!(ctrl.bottom_mode(), BottomMode::TrackControl);

        // Press top-row button 2 (id 2) -> selects Track 3 (index 2) and writes /script/theotokos/selected (seq_id = 12)
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 2 }, &mut conf);
        assert_eq!(ctrl.active_track(), 2);
        assert_eq!(
            conf.state_bus_read("/script/theotokos/selected"),
            Some(StateBusValue::Int(ids.sequencers[2] as i64))
        );

        // Press bottom-row button 10 (id = 8 + 2 = 10) -> mutes Track 3 (sequencers[2])
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 10 }, &mut conf);
        exec.process(&mut out_buf, 2);
        conf.process_main_thread();

        let Some(StateBusValue::Float(muted)) =
            conf.state_bus_read(&format!("/node/{}/param/mute", ids.sequencers[2]))
        else {
            panic!("expected mute param on seq 12");
        };
        assert!((muted - 1.0).abs() < 1e-6, "track 2 should be muted");
    }

    #[test]
    fn hold_step_and_turn_knob_writes_param_lock_without_toggling_step_off() {
        let (mut conf, ids, mut ctrl) = setup_8track();
        let mut exec = conf.build_executor();
        let mut out_buf = [0.0f32; 1024];

        // Hold step button 0 on Track 1 (seq 10)
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 0 }, &mut conf);
        // While holding step 0, turn Row 1 Knob 0 (id 0 -> tune on voice 20) to 100%
        ctrl.handle_event(
            SurfaceEvent::FaderMoved {
                id: 0,
                value: 65535,
            },
            &mut conf,
        );
        // Release step button 0
        ctrl.handle_event(SurfaceEvent::ButtonReleased { id: 0 }, &mut conf);

        exec.process(&mut out_buf, 2);
        conf.process_main_thread();

        // Step 0 must remain active after release
        let mask = LcxlDrumController::read_step_mask(&conf, ids.sequencers[0]);
        assert_eq!(mask & 1, 1, "step 0 must stay active after hold-step p-lock");

        // Base tune parameter on voice 20 must remain at default 0.0 (not mutated to +24.0)
        let Some(StateBusValue::Float(base_tune)) =
            conf.state_bus_read(&format!("/node/{}/param/tune", ids.generators[0]))
        else {
            panic!("expected AnalogEngine tune on StateBus");
        };
        assert!(
            base_tune.abs() < 1e-4,
            "base tune should remain 0.0 when knob was turned during step hold, got {base_tune}"
        );

        // Sequencer 10 must publish the authored lock on step 0 (`s0:20:<tune_id>=24.000000`)
        let Some(StateBusValue::Text(locks_str)) =
            conf.state_bus_read(&format!("/node/{}/state/locks", ids.sequencers[0]))
        else {
            panic!("expected /node/10/state/locks on StateBus");
        };
        assert!(
            locks_str.starts_with("s0:20:") && locks_str.contains("=24.000000"),
            "expected step 0 lock on node 20 at +24.0 semitones, got {locks_str}"
        );
    }

    #[test]
    fn momentary_hold_to_mute_and_hardware_transport() {
        let (mut conf, ids, mut ctrl) = setup_8track();
        let mut exec = conf.build_executor();
        let mut out_buf = [0.0f32; 1024];

        assert_eq!(ctrl.bottom_mode(), BottomMode::StepSequencer);

        // 1. Hardware transport: Press Down (21) -> Play
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 21 }, &mut conf);
        exec.process(&mut out_buf, 2);
        conf.process_main_thread();
        assert_eq!(
            conf.state_bus_read("/transport/playing"),
            Some(StateBusValue::Bool(true)),
            "Down button should start transport"
        );

        // Press Down again -> Pause
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 21 }, &mut conf);
        exec.process(&mut out_buf, 2);
        conf.process_main_thread();
        assert_eq!(
            conf.state_bus_read("/transport/playing"),
            Some(StateBusValue::Bool(false)),
            "Down button again should pause transport"
        );

        // Press Up (20) -> Stop / Rewind
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 20 }, &mut conf);
        exec.process(&mut out_buf, 2);
        conf.process_main_thread();
        assert_eq!(
            conf.state_bus_read("/transport/playing"),
            Some(StateBusValue::Bool(false)),
            "Up button should stop transport"
        );

        // 2. Momentary Hold-to-Mute: Hold Mute (17) -> tap pad 0 (Kick) -> release Mute (17)
        assert_eq!(ctrl.bottom_mode(), BottomMode::StepSequencer);
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 17 }, &mut conf);
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 0 }, &mut conf);
        ctrl.handle_event(SurfaceEvent::ButtonReleased { id: 0 }, &mut conf);
        ctrl.handle_event(SurfaceEvent::ButtonReleased { id: 17 }, &mut conf);

        exec.process(&mut out_buf, 2);
        conf.process_main_thread();

        // Should return directly to StepSequencer mode without getting trapped
        assert_eq!(ctrl.bottom_mode(), BottomMode::StepSequencer);

        // Track 0 (Kick) must now be muted
        let Some(StateBusValue::Float(muted)) =
            conf.state_bus_read(&format!("/node/{}/param/mute", ids.sequencers[0]))
        else {
            panic!("expected mute param on StateBus");
        };
        assert_eq!(muted, 1.0, "Kick should be muted by momentary hold-to-mute");
    }
}
