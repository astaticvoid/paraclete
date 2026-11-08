// SPDX-License-Identifier: GPL-3.0-or-later
//! Launchpad 8×8 expressive pin-matrix controller (`LaunchpadMatrixController`, Horizon 2.1).
//!
//! Bridges Novation Launchpad X (or Launchpad terminal emulator) with `MatrixNode` (ID 50):
//! - **64 Grid Pads (`id: 0..=63`):** Row-major crosspoint cells ($row = id / 8$, $col = id \% 8$).
//!   - **Tap pad $(r, c)$:** Toggle crosspoint latch between `0.0` and `0.7` (or `-0.7` in invert mode).
//!   - **Hold + Pressure (`PadPressure`):** Continuous polyphonic aftertouch dynamically modulates
//!     crosspoint weight without changing the latched base value. Releasing the pad returns to the
//!     latched base weight.
//!   - **24-bit RGB Feedback:** Inactive pins are dark (or faint gray/cyan signal monitors). Latched pins
//!     glow Cyan (positive weight) or Amber/Magenta (inverted weight), bursting into bright White/Cyan
//!     or White/Yellow proportionally to incoming signal activity ($in\_level[row] \times |weight|$).
//! - **Right Scene Buttons (`id: 64..=71`):**
//!   - Scene 0..=6 (`64..=70`): Row mute / toggle for inputs 0..=6; doubles as input level meter (Green/Yellow).
//!   - Scene 7 (`71`): Toggle Invert Polarity mode (Bright Red/Amber when active).
//! - **Top Buttons (`id: 72..=79`):**
//!   - Top 0..=6 (`72..=78`): Column clear / mute for outputs 0..=6; glows Cyan on active output signal.
//!   - Top 7 (`79`): Clear All (resets all 64 crosspoints to `0.0`).

use std::collections::HashMap;

use paraclete_node_api::{
    LedUpdate, NodeCommand, RgbColor, StateBusValue, SurfaceEvent, SurfaceOutput, CMD_SET_PARAM,
};
use paraclete_nodes::MatrixNode;
use paraclete_runtime::NodeConfigurator;

pub const MATRIX_GRID_SIZE: usize = 8;
pub const MATRIX_CELL_COUNT: usize = MATRIX_GRID_SIZE * MATRIX_GRID_SIZE; // 64
pub const TOTAL_LP_CONTROLS: usize = 80; // 64 pads + 8 scene + 8 top

pub struct LaunchpadMatrixController {
    matrix_id: u32,
    surface_id: u32,
    cell_param_ids: [u32; MATRIX_CELL_COUNT],
    latched_weights: [f64; MATRIX_CELL_COUNT],
    active_weights: [f64; MATRIX_CELL_COUNT],
    is_pressed: [bool; MATRIX_CELL_COUNT],
    pressure_modulated: [bool; MATRIX_CELL_COUNT],
    invert_mode: bool,
    last_leds: [RgbColor; TOTAL_LP_CONTROLS],
    leds_dirty: bool,
}

impl LaunchpadMatrixController {
    /// Construct a controller explicitly bound to `matrix_id` and `surface_id`.
    pub fn new(matrix_id: u32, surface_id: u32, conf: &NodeConfigurator) -> Self {
        let mut cell_param_ids = [0u32; MATRIX_CELL_COUNT];
        let mut latched_weights = [0.0f64; MATRIX_CELL_COUNT];

        for row in 0..MATRIX_GRID_SIZE {
            for col in 0..MATRIX_GRID_SIZE {
                let idx = row * MATRIX_GRID_SIZE + col;
                let pid = MatrixNode::cell_param_id(row, col).unwrap_or(0);
                cell_param_ids[idx] = pid;

                let param_name = format!("cell_{row}_{col}");
                let path = format!("/node/{matrix_id}/param/{param_name}");
                if let Some(StateBusValue::Float(val)) = conf.state_bus_read(&path) {
                    latched_weights[idx] = val;
                }
            }
        }

        let mut ctrl = Self {
            matrix_id,
            surface_id,
            cell_param_ids,
            latched_weights,
            active_weights: latched_weights,
            is_pressed: [false; MATRIX_CELL_COUNT],
            pressure_modulated: [false; MATRIX_CELL_COUNT],
            invert_mode: false,
            last_leds: [RgbColor::OFF; TOTAL_LP_CONTROLS],
            leds_dirty: true,
        };
        ctrl.sync_from_bus(conf);
        ctrl
    }

    /// Automatically discover a `MatrixNode` in the configurator DAG.
    pub fn from_graph(conf: &NodeConfigurator, surface_id: u32) -> Option<Self> {
        let matrix_id = conf
            .known_node_ids()
            .find(|&id| conf.type_tag_for(id) == Some("matrix"))?;
        Some(Self::new(matrix_id, surface_id, conf))
    }

    pub fn matrix_id(&self) -> u32 {
        self.matrix_id
    }

    pub fn surface_id(&self) -> u32 {
        self.surface_id
    }

    pub fn latched_weight(&self, row: usize, col: usize) -> f64 {
        if row < MATRIX_GRID_SIZE && col < MATRIX_GRID_SIZE {
            self.latched_weights[row * MATRIX_GRID_SIZE + col]
        } else {
            0.0
        }
    }

    pub fn active_weight(&self, row: usize, col: usize) -> f64 {
        if row < MATRIX_GRID_SIZE && col < MATRIX_GRID_SIZE {
            self.active_weights[row * MATRIX_GRID_SIZE + col]
        } else {
            0.0
        }
    }

    pub fn invert_mode(&self) -> bool {
        self.invert_mode
    }

    pub fn set_invert_mode(&mut self, invert: bool) {
        self.invert_mode = invert;
        self.leds_dirty = true;
    }

    /// Handle a hardware event from the Launchpad or terminal emulator.
    pub fn handle_event(&mut self, ev: SurfaceEvent, conf: &mut NodeConfigurator) {
        match ev {
            SurfaceEvent::PadPressed {
                id,
                velocity: _,
                pressure,
            } => {
                if (id as usize) < MATRIX_CELL_COUNT {
                    let idx = id as usize;
                    self.is_pressed[idx] = true;
                    self.pressure_modulated[idx] = false;

                    // If initial press already carries strong polyphonic pressure:
                    if pressure > 2000 {
                        self.pressure_modulated[idx] = true;
                        let norm = (pressure as f64 / 65535.0).clamp(0.0, 1.0);
                        let weight = if self.invert_mode { -norm } else { norm };
                        self.active_weights[idx] = weight;
                        let _ = conf.send_command(NodeCommand {
                            target_id: self.matrix_id,
                            type_id: CMD_SET_PARAM,
                            arg0: self.cell_param_ids[idx] as i64,
                            arg1: weight,
                        });
                    }
                    self.leds_dirty = true;
                }
            }
            SurfaceEvent::PadPressure { id, pressure } => {
                if (id as usize) < MATRIX_CELL_COUNT {
                    let idx = id as usize;
                    if self.is_pressed[idx] {
                        let norm = (pressure as f64 / 65535.0).clamp(0.0, 1.0);
                        if norm > 0.02 {
                            self.pressure_modulated[idx] = true;
                            let weight = if self.invert_mode { -norm } else { norm };
                            self.active_weights[idx] = weight;
                            let _ = conf.send_command(NodeCommand {
                                target_id: self.matrix_id,
                                type_id: CMD_SET_PARAM,
                                arg0: self.cell_param_ids[idx] as i64,
                                arg1: weight,
                            });
                            self.leds_dirty = true;
                        }
                    }
                }
            }
            SurfaceEvent::PadReleased { id } => {
                if (id as usize) < MATRIX_CELL_COUNT {
                    let idx = id as usize;
                    self.is_pressed[idx] = false;

                    if self.pressure_modulated[idx] {
                        // Aftertouch modulation released -> revert to latched base weight
                        let base = self.latched_weights[idx];
                        self.active_weights[idx] = base;
                        let _ = conf.send_command(NodeCommand {
                            target_id: self.matrix_id,
                            type_id: CMD_SET_PARAM,
                            arg0: self.cell_param_ids[idx] as i64,
                            arg1: base,
                        });
                    } else {
                        // Clean tap -> toggle latch state
                        let current = self.latched_weights[idx];
                        let new_weight = if current.abs() > 0.05 {
                            0.0
                        } else if self.invert_mode {
                            -0.7
                        } else {
                            0.7
                        };
                        self.latched_weights[idx] = new_weight;
                        self.active_weights[idx] = new_weight;
                        let _ = conf.send_command(NodeCommand {
                            target_id: self.matrix_id,
                            type_id: CMD_SET_PARAM,
                            arg0: self.cell_param_ids[idx] as i64,
                            arg1: new_weight,
                        });
                    }
                    self.pressure_modulated[idx] = false;
                    self.leds_dirty = true;
                }
            }
            SurfaceEvent::ButtonPressed { id } => {
                match id {
                    // Scene 7 (id 71): Invert mode toggle
                    71 => {
                        self.invert_mode = !self.invert_mode;
                        self.leds_dirty = true;
                    }
                    // Scene 0..=6 (id 64..=70): Row toggle/mute
                    64..=70 => {
                        let row = (id - 64) as usize;
                        let row_active = (0..MATRIX_GRID_SIZE)
                            .any(|c| self.latched_weights[row * MATRIX_GRID_SIZE + c].abs() > 0.05);

                        for col in 0..MATRIX_GRID_SIZE {
                            let idx = row * MATRIX_GRID_SIZE + col;
                            let new_weight = if row_active {
                                0.0
                            } else if row == col {
                                0.7 // Default diagonal un-mute
                            } else {
                                self.latched_weights[idx]
                            };
                            self.latched_weights[idx] = new_weight;
                            self.active_weights[idx] = new_weight;
                            let _ = conf.send_command(NodeCommand {
                                target_id: self.matrix_id,
                                type_id: CMD_SET_PARAM,
                                arg0: self.cell_param_ids[idx] as i64,
                                arg1: new_weight,
                            });
                        }
                        self.leds_dirty = true;
                    }
                    // Top 7 (id 79): Clear all crosspoints
                    79 => {
                        for idx in 0..MATRIX_CELL_COUNT {
                            if self.latched_weights[idx].abs() > 0.001
                                || self.active_weights[idx].abs() > 0.001
                            {
                                self.latched_weights[idx] = 0.0;
                                self.active_weights[idx] = 0.0;
                                let _ = conf.send_command(NodeCommand {
                                    target_id: self.matrix_id,
                                    type_id: CMD_SET_PARAM,
                                    arg0: self.cell_param_ids[idx] as i64,
                                    arg1: 0.0,
                                });
                            }
                        }
                        self.leds_dirty = true;
                    }
                    // Top 0..=6 (id 72..=78): Clear column
                    72..=78 => {
                        let col = (id - 72) as usize;
                        for row in 0..MATRIX_GRID_SIZE {
                            let idx = row * MATRIX_GRID_SIZE + col;
                            if self.latched_weights[idx].abs() > 0.001
                                || self.active_weights[idx].abs() > 0.001
                            {
                                self.latched_weights[idx] = 0.0;
                                self.active_weights[idx] = 0.0;
                                let _ = conf.send_command(NodeCommand {
                                    target_id: self.matrix_id,
                                    type_id: CMD_SET_PARAM,
                                    arg0: self.cell_param_ids[idx] as i64,
                                    arg1: 0.0,
                                });
                            }
                        }
                        self.leds_dirty = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Read incoming signal levels packed into `/node/{matrix_id}/state/in_levels` on the StateBus.
    fn read_in_levels(conf: &NodeConfigurator, matrix_id: u32) -> [f64; MATRIX_GRID_SIZE] {
        let mut levels = [0.0f64; MATRIX_GRID_SIZE];
        let path = format!("/node/{matrix_id}/state/in_levels");
        if let Some(StateBusValue::Int(packed)) = conf.state_bus_read(&path) {
            for (row, slot) in levels.iter_mut().enumerate() {
                let byte = ((packed >> (row * 8)) & 0xFF) as u8;
                *slot = byte as f64 / 255.0;
            }
        }
        levels
    }

    /// Sync active weights from StateBus if modified externally (e.g. project load or patch).
    pub fn sync_from_bus(&mut self, conf: &NodeConfigurator) {
        let path = format!("/node/{}/state/active_cells", self.matrix_id);
        if let Some(StateBusValue::Int(mask)) = conf.state_bus_read(&path) {
            for i in 0..MATRIX_CELL_COUNT {
                if !self.is_pressed[i] {
                    let has_cell = (mask >> i) & 1 != 0;
                    if !has_cell && self.latched_weights[i].abs() > 0.05 {
                        self.latched_weights[i] = 0.0;
                        self.active_weights[i] = 0.0;
                        self.leds_dirty = true;
                    }
                }
            }
        }
    }

    /// Compute desired RGB colors for the 64 pads and 16 buttons.
    pub fn compute_leds(&self, conf: &NodeConfigurator) -> [RgbColor; TOTAL_LP_CONTROLS] {
        let mut leds = [RgbColor::OFF; TOTAL_LP_CONTROLS];
        let in_levels = Self::read_in_levels(conf, self.matrix_id);

        let mut col_activity = [0.0f64; MATRIX_GRID_SIZE];

        // ── 64 Crosspoint Pads ──────────────────────────────────────────────
        for (row, &in_sig) in in_levels.iter().enumerate().take(MATRIX_GRID_SIZE) {
            for (col, act) in col_activity.iter_mut().enumerate().take(MATRIX_GRID_SIZE) {
                let idx = row * MATRIX_GRID_SIZE + col;
                let w = self.active_weights[idx];
                let cell_sig = in_sig * w.abs();
                *act += cell_sig;

                leds[idx] = if w == 0.0 {
                    if in_sig > 0.05 {
                        // Inactive cell with active row signal: dim indicator
                        RgbColor {
                            r: 0,
                            g: (in_sig * 18.0) as u8,
                            b: (in_sig * 28.0) as u8,
                        }
                    } else {
                        RgbColor::OFF
                    }
                } else if w > 0.0 {
                    // Normal routing (positive weight): Cyan/Green base, bright white/cyan burst on signal
                    let base_g = (80.0 + 80.0 * w).min(200.0);
                    let base_b = (120.0 + 130.0 * w).min(255.0);
                    let burst = (cell_sig * 220.0).min(255.0);
                    RgbColor {
                        r: burst as u8,
                        g: (base_g + burst * 0.4).min(255.0) as u8,
                        b: (base_b + burst * 0.2).min(255.0) as u8,
                    }
                } else {
                    // Inverted routing (negative weight): Amber/Pink base, bright yellow/white burst on signal
                    let abs_w = -w;
                    let base_r = (140.0 + 110.0 * abs_w).min(255.0);
                    let base_g = (40.0 + 50.0 * abs_w).min(120.0);
                    let base_b = (80.0 * abs_w).min(100.0);
                    let burst = (cell_sig * 220.0).min(255.0);
                    RgbColor {
                        r: (base_r + burst * 0.2).min(255.0) as u8,
                        g: (base_g + burst * 0.8).min(255.0) as u8,
                        b: (base_b + burst).min(255.0) as u8,
                    }
                };
            }
        }

        // ── Right Scene Buttons 64..=71 ──────────────────────────────────────
        for row in 0..7 {
            let in_sig = in_levels[row];
            leds[64 + row] = if in_sig > 0.05 {
                // Input activity meter: Green into Yellow
                let g = (in_sig * 255.0).clamp(40.0, 255.0) as u8;
                let r = if in_sig > 0.7 {
                    ((in_sig - 0.7) / 0.3 * 220.0) as u8
                } else {
                    0
                };
                RgbColor { r, g, b: 0 }
            } else {
                RgbColor { r: 0, g: 30, b: 0 }
            };
        }
        // Scene 7 (id 71): Invert Polarity mode status
        leds[71] = if self.invert_mode {
            RgbColor { r: 255, g: 40, b: 0 } // Bright Red/Amber
        } else {
            RgbColor { r: 0, g: 20, b: 60 } // Dim Blue
        };

        // ── Top Buttons 72..=79 ──────────────────────────────────────────────
        for col in 0..7 {
            let act = col_activity[col].min(1.0);
            leds[72 + col] = if act > 0.05 {
                RgbColor {
                    r: 0,
                    g: (act * 200.0).clamp(30.0, 240.0) as u8,
                    b: (act * 255.0).clamp(50.0, 255.0) as u8,
                }
            } else {
                RgbColor { r: 0, g: 15, b: 25 }
            };
        }
        // Top 7 (id 79): Clear All button (dim Amber)
        leds[79] = RgbColor {
            r: 120,
            g: 40,
            b: 0,
        };

        leds
    }

    /// Compute LED deltas and deliver incremental updates to the Launchpad surface.
    pub fn tick_leds(&mut self, conf: &mut NodeConfigurator) {
        self.sync_from_bus(conf);
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
                self.surface_id,
                SurfaceOutput {
                    led_updates: updates,
                    display_updates: Vec::new(),
                },
            );
            conf.deliver_script_output(map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_runtime::NodeConfigurator;

    fn make_test_conf() -> (NodeConfigurator, u32) {
        let mut conf = NodeConfigurator::new(44100.0, 128);
        let mat = Box::new(MatrixNode::new());
        let matrix_id = 50;
        conf.add_node(matrix_id, mat);
        (conf, matrix_id)
    }

    #[test]
    fn tap_pad_toggles_crosspoint_latch() {
        let (mut conf, matrix_id) = make_test_conf();
        let mut ctrl = LaunchpadMatrixController::new(matrix_id, 102, &conf);

        assert_eq!(ctrl.latched_weight(2, 3), 0.0);

        // Press pad (row 2, col 3 -> id = 19)
        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 19,
                velocity: 32000,
                pressure: 0,
            },
            &mut conf,
        );
        // Release pad without aftertouch
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 19 }, &mut conf);

        assert!((ctrl.latched_weight(2, 3) - 0.7).abs() < 1e-4);
        assert!((ctrl.active_weight(2, 3) - 0.7).abs() < 1e-4);

        // Tap again -> toggles off to 0.0
        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 19,
                velocity: 32000,
                pressure: 0,
            },
            &mut conf,
        );
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 19 }, &mut conf);

        assert_eq!(ctrl.latched_weight(2, 3), 0.0);
        assert_eq!(ctrl.active_weight(2, 3), 0.0);
    }

    #[test]
    fn aftertouch_pressure_modulates_weight_and_restores_latch_on_release() {
        let (mut conf, matrix_id) = make_test_conf();
        let mut ctrl = LaunchpadMatrixController::new(matrix_id, 102, &conf);

        // First tap to latch at 0.7
        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 0,
                velocity: 30000,
                pressure: 0,
            },
            &mut conf,
        );
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 0 }, &mut conf);
        assert!((ctrl.latched_weight(0, 0) - 0.7).abs() < 1e-4);

        // Now press pad 0 and push aftertouch up to full scale (65535)
        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 0,
                velocity: 30000,
                pressure: 1000,
            },
            &mut conf,
        );
        ctrl.handle_event(
            SurfaceEvent::PadPressure {
                id: 0,
                pressure: 65535,
            },
            &mut conf,
        );

        assert!((ctrl.active_weight(0, 0) - 1.0).abs() < 1e-4);
        assert!((ctrl.latched_weight(0, 0) - 0.7).abs() < 1e-4); // latch unaffected!

        // Release -> restores latch
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 0 }, &mut conf);
        assert!((ctrl.active_weight(0, 0) - 0.7).abs() < 1e-4);
        assert!((ctrl.latched_weight(0, 0) - 0.7).abs() < 1e-4);
    }

    #[test]
    fn invert_mode_latches_negative_weights() {
        let (mut conf, matrix_id) = make_test_conf();
        let mut ctrl = LaunchpadMatrixController::new(matrix_id, 102, &conf);

        // Press Scene 7 (id 71) to toggle invert mode
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 71 }, &mut conf);
        assert!(ctrl.invert_mode());

        // Tap pad (row 1, col 1 -> id 9)
        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 9,
                velocity: 30000,
                pressure: 0,
            },
            &mut conf,
        );
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 9 }, &mut conf);

        assert!((ctrl.latched_weight(1, 1) - (-0.7)).abs() < 1e-4);
    }

    #[test]
    fn clear_all_top_button_resets_all_crosspoints() {
        let (mut conf, matrix_id) = make_test_conf();
        let mut ctrl = LaunchpadMatrixController::new(matrix_id, 102, &conf);

        // Latch two pads
        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 1,
                velocity: 30000,
                pressure: 0,
            },
            &mut conf,
        );
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 1 }, &mut conf);

        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 10,
                velocity: 30000,
                pressure: 0,
            },
            &mut conf,
        );
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 10 }, &mut conf);

        assert!(ctrl.latched_weight(0, 1) > 0.0);
        assert!(ctrl.latched_weight(1, 2) > 0.0);

        // Top button 7 (id 79) = Clear All
        ctrl.handle_event(SurfaceEvent::ButtonPressed { id: 79 }, &mut conf);

        assert_eq!(ctrl.latched_weight(0, 1), 0.0);
        assert_eq!(ctrl.latched_weight(1, 2), 0.0);
    }

    #[test]
    fn compute_leds_reflects_active_pins_and_signal_activity() {
        let (mut conf, matrix_id) = make_test_conf();
        let mut ctrl = LaunchpadMatrixController::new(matrix_id, 102, &conf);

        // Latch pad (row 0, col 0)
        ctrl.handle_event(
            SurfaceEvent::PadPressed {
                id: 0,
                velocity: 30000,
                pressure: 0,
            },
            &mut conf,
        );
        ctrl.handle_event(SurfaceEvent::PadReleased { id: 0 }, &mut conf);

        let leds_silent = ctrl.compute_leds(&conf);
        assert_ne!(leds_silent[0], RgbColor::OFF); // Latched pad is lit Cyan

        // Simulate incoming signal on Row 0 by writing in_levels to StateBus
        let packed_sig = 255i64; // row 0 at full scale (byte 0 = 255)
        conf.state_bus_write(
            &format!("/node/{matrix_id}/state/in_levels"),
            StateBusValue::Int(packed_sig),
        );

        let leds_active = ctrl.compute_leds(&conf);
        assert!(leds_active[0].r > 150); // Burst to white/cyan
        assert_ne!(leds_active[64], RgbColor::OFF); // Scene 0 meter lit
    }
}
