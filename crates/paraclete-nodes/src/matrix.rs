// SPDX-License-Identifier: GPL-3.0-or-later
//! `MatrixNode` — 8×8 click-free smoothed pin-matrix router (Horizon 2.1).
//!
//! Routes 8 signal inputs (`in_0..in_7`, `PortType::Cv`) to 8 signal outputs
//! (`out_0..out_7`, `PortType::Cv`) plus stereo audio output (`audio_out_l`,
//! `audio_out_r` mirroring `out_0` / `out_1`).
//!
//! Each of the 64 crosspoints (`cell_{row}_{col}`, `row` = input `0..8`,
//! `col` = output `0..8`) has a bipolar weight in `-1.0..=1.0` (default `0.0`),
//! smoothed per-sample on the audio thread with a 1-pole filter ($\tau = 5\text{ ms}$)
//! so live pad toggles, polyphonic aftertouch pressure, and step parameter
//! locks never click.

use std::collections::HashMap;
use std::sync::OnceLock;

use paraclete_node_api::{
    publish_bank_state, CapabilityDocument, Event, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortName, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

const MATRIX_SIZE: usize = 8;
const CELL_COUNT: usize = MATRIX_SIZE * MATRIX_SIZE;
const SMOOTH_TIME_SECS: f32 = 0.005;

static CELL_NAMES: [&str; CELL_COUNT] = [
    "cell_0_0", "cell_0_1", "cell_0_2", "cell_0_3", "cell_0_4", "cell_0_5", "cell_0_6", "cell_0_7",
    "cell_1_0", "cell_1_1", "cell_1_2", "cell_1_3", "cell_1_4", "cell_1_5", "cell_1_6", "cell_1_7",
    "cell_2_0", "cell_2_1", "cell_2_2", "cell_2_3", "cell_2_4", "cell_2_5", "cell_2_6", "cell_2_7",
    "cell_3_0", "cell_3_1", "cell_3_2", "cell_3_3", "cell_3_4", "cell_3_5", "cell_3_6", "cell_3_7",
    "cell_4_0", "cell_4_1", "cell_4_2", "cell_4_3", "cell_4_4", "cell_4_5", "cell_4_6", "cell_4_7",
    "cell_5_0", "cell_5_1", "cell_5_2", "cell_5_3", "cell_5_4", "cell_5_5", "cell_5_6", "cell_5_7",
    "cell_6_0", "cell_6_1", "cell_6_2", "cell_6_3", "cell_6_4", "cell_6_5", "cell_6_6", "cell_6_7",
    "cell_7_0", "cell_7_1", "cell_7_2", "cell_7_3", "cell_7_4", "cell_7_5", "cell_7_6", "cell_7_7",
];

pub struct MatrixNode {
    node_id: u32,
    ports: [PortDescriptor; 18],
    bank: ParameterBank,
    cell_ids: [u32; CELL_COUNT],
    smoothed: [f32; CELL_COUNT],
    node_locks: Vec<(u32, f64)>,
    sample_rate: f32,
    smooth_coeff: f32,
    active_mask_path: OnceLock<String>,
    in_activity_path: OnceLock<String>,
    in_peaks: [f32; MATRIX_SIZE],
    pending_initial_params: HashMap<String, f64>,
}

impl MatrixNode {
    pub const PORT_IN_BASE: u32 = 0;      // 0..=7: Cv inputs
    pub const PORT_OUT_BASE: u32 = 8;     // 8..=15: Cv outputs
    pub const PORT_AUDIO_OUT_L: u32 = 16; // stereo audio out L (mirrors out_0)
    pub const PORT_AUDIO_OUT_R: u32 = 17; // stereo audio out R (mirrors out_1)

    pub fn new() -> Self {
        let doc = Self::build_doc();
        let mut cell_ids = [0u32; CELL_COUNT];
        for (i, name) in CELL_NAMES.iter().enumerate() {
            cell_ids[i] = ParamDescriptor::id_for_name(name);
        }
        Self {
            node_id: 0,
            ports: Self::default_ports(),
            bank: ParameterBank::from_capability_document(&doc),
            cell_ids,
            smoothed: [0.0; CELL_COUNT],
            node_locks: Vec::with_capacity(CELL_COUNT),
            sample_rate: 44100.0,
            smooth_coeff: Self::compute_coeff(44100.0),
            active_mask_path: OnceLock::new(),
            in_activity_path: OnceLock::new(),
            in_peaks: [0.0; MATRIX_SIZE],
            pending_initial_params: HashMap::new(),
        }
    }

    /// Return the canonical parameter ID for crosspoint `(row, col)` (`0..8`, `0..8`).
    pub fn cell_param_id(row: usize, col: usize) -> Option<u32> {
        if row < MATRIX_SIZE && col < MATRIX_SIZE {
            Some(ParamDescriptor::id_for_name(CELL_NAMES[row * MATRIX_SIZE + col]))
        } else {
            None
        }
    }

    fn compute_coeff(sample_rate: f32) -> f32 {
        let sr = sample_rate.max(1.0);
        1.0 - (-1.0 / (SMOOTH_TIME_SECS * sr)).exp()
    }

    fn default_ports() -> [PortDescriptor; 18] {
        [
            PortDescriptor { id: 0,  name: PortName::Static("in_0"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 1,  name: PortName::Static("in_1"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 2,  name: PortName::Static("in_2"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 3,  name: PortName::Static("in_3"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 4,  name: PortName::Static("in_4"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 5,  name: PortName::Static("in_5"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 6,  name: PortName::Static("in_6"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 7,  name: PortName::Static("in_7"),  direction: PortDirection::Input,  port_type: PortType::Cv },
            PortDescriptor { id: 8,  name: PortName::Static("out_0"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 9,  name: PortName::Static("out_1"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 10, name: PortName::Static("out_2"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 11, name: PortName::Static("out_3"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 12, name: PortName::Static("out_4"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 13, name: PortName::Static("out_5"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 14, name: PortName::Static("out_6"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 15, name: PortName::Static("out_7"), direction: PortDirection::Output, port_type: PortType::Cv },
            PortDescriptor { id: 16, name: PortName::Static("audio_out_l"), direction: PortDirection::Output, port_type: PortType::Audio },
            PortDescriptor { id: 17, name: PortName::Static("audio_out_r"), direction: PortDirection::Output, port_type: PortType::Audio },
        ]
    }

    fn build_doc() -> CapabilityDocument {
        let params = CELL_NAMES
            .iter()
            .map(|&name| ParamDescriptor {
                id: ParamDescriptor::id_for_name(name),
                name: name.into(),
                min: -1.0,
                max: 1.0,
                default: 0.0,
                stepped: false,
                in_kit: true,
                unit: ParamUnit::Generic,
                display: None,
            })
            .collect();

        CapabilityDocument {
            name: "Matrix8x8".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: Self::default_ports().to_vec(),
            params,
            extensions: vec![],
            view: None,
        }
    }

    #[inline]
    fn target_weight(&self, idx: usize) -> f32 {
        let pid = self.cell_ids[idx];
        for &(lock_id, val) in &self.node_locks {
            if lock_id == pid {
                return (val as f32).clamp(-1.0, 1.0);
            }
        }
        (self.bank.get(pid) as f32).clamp(-1.0, 1.0)
    }
}

impl Default for MatrixNode {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for MatrixNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        Self::build_doc()
    }

    fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
        self.pending_initial_params = params.clone();
    }

    fn serialize(&self) -> Vec<u8> {
        self.bank.serialize()
    }

    fn deserialize(&mut self, data: &[u8]) {
        self.bank.deserialize(data);
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        publish_bank_state(self.node_id, &self.bank, buf);
        let mut mask: i64 = 0;
        for i in 0..CELL_COUNT {
            if self.target_weight(i).abs() > 0.01 {
                mask |= 1i64 << i;
            }
        }
        let path = self
            .active_mask_path
            .get_or_init(|| format!("/node/{}/state/active_cells", self.node_id));
        buf.push((path.clone(), StateBusValue::Int(mask)));

        let mut packed_levels: i64 = 0;
        for row in 0..MATRIX_SIZE {
            let peak = self.in_peaks[row].clamp(0.0, 1.0);
            let byte = (peak * 255.0).round() as u8;
            packed_levels |= (byte as i64) << (row * 8);
        }
        let in_path = self
            .in_activity_path
            .get_or_init(|| format!("/node/{}/state/in_levels", self.node_id));
        buf.push((in_path.clone(), StateBusValue::Int(packed_levels)));
    }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate;
        self.smooth_coeff = Self::compute_coeff(sample_rate);
        let doc = Self::build_doc();
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            if let Some(p) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(p.id, value);
            }
        }
        for i in 0..CELL_COUNT {
            self.smoothed[i] = self.bank.get(self.cell_ids[i]) as f32;
        }
        self.in_peaks = [0.0; MATRIX_SIZE];
        self.node_locks.clear();
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        for timed in input.events {
            if let Event::ParamLock(ref pl) = timed.event {
                if pl.node_id == self.node_id {
                    if let Some(slot) = self.node_locks.iter_mut().find(|(id, _)| *id == pl.param_id) {
                        slot.1 = pl.value;
                    } else if self.node_locks.len() < self.node_locks.capacity() {
                        self.node_locks.push((pl.param_id, pl.value));
                    }
                }
            }
        }

        let block_size = input.block_size;
        let coeff = self.smooth_coeff;

        let mut targets = [0.0f32; CELL_COUNT];
        for (i, target) in targets.iter_mut().enumerate() {
            *target = self.target_weight(i);
        }

        // Gather input slices (`in_0..in_7`). Also allow stereo `audio_inputs[0]`
        // to feed `in_0` (L) and `in_1` (R) if connected via audio edges.
        let mut cv_ins: [&[f32]; MATRIX_SIZE] = [&[]; MATRIX_SIZE];
        let mut has_cv_in = [false; MATRIX_SIZE];
        for row in 0..MATRIX_SIZE {
            let port_id = Self::PORT_IN_BASE + row as u32;
            has_cv_in[row] = input
                .signal_inputs
                .iter()
                .any(|s| s.port_id == port_id && s.kind == paraclete_node_api::SignalPortKind::Cv);
            cv_ins[row] = input.cv_signal(port_id);
        }
        let audio_in_l = input
            .audio_inputs
            .first()
            .filter(|b| b.channels() >= 1)
            .map(|b| b.channel(0));
        let audio_in_r = input
            .audio_inputs
            .first()
            .filter(|b| b.channels() >= 2)
            .map(|b| b.channel(1));

        let mut out_0_scratch = [0.0f32; 2048];
        let mut out_1_scratch = [0.0f32; 2048];
        let frames = block_size.min(2048);

        // Track peak signal level per input row for LED visualization and metering
        for row in 0..MATRIX_SIZE {
            let mut peak = 0.0f32;
            if has_cv_in[row] {
                for &s in cv_ins[row].iter().take(frames) {
                    let a = s.abs();
                    if a > peak {
                        peak = a;
                    }
                }
            } else if row == 0 {
                if let Some(s) = audio_in_l {
                    for &val in s.iter().take(frames) {
                        let a = val.abs();
                        if a > peak {
                            peak = a;
                        }
                    }
                }
            } else if row == 1 {
                if let Some(s) = audio_in_r {
                    for &val in s.iter().take(frames) {
                        let a = val.abs();
                        if a > peak {
                            peak = a;
                        }
                    }
                }
            }
            // Smoothly decay previous peak while capturing new maxima
            self.in_peaks[row] = (self.in_peaks[row] * 0.95).max(peak);
        }

        // Advance smoothed weights sample-by-sample and compute all 8 output columns.
        let initial_smoothed = self.smoothed;
        for col in 0..MATRIX_SIZE {
            let mut s_col = [0.0f32; MATRIX_SIZE];
            for row in 0..MATRIX_SIZE {
                s_col[row] = initial_smoothed[row * MATRIX_SIZE + col];
            }

            if let Some(out_slice) = output.try_cv_signal_output_mut(Self::PORT_OUT_BASE + col as u32) {
                for frame in 0..block_size.min(out_slice.len()) {
                    let mut sum = 0.0f32;
                    for row in 0..MATRIX_SIZE {
                        let idx = row * MATRIX_SIZE + col;
                        s_col[row] += coeff * (targets[idx] - s_col[row]);
                        let w = s_col[row];
                        if w.abs() > 1e-7 {
                            let x = if has_cv_in[row] {
                                cv_ins[row].get(frame).copied().unwrap_or(0.0)
                            } else if row == 0 {
                                audio_in_l.and_then(|s| s.get(frame).copied()).unwrap_or(0.0)
                            } else if row == 1 {
                                audio_in_r.and_then(|s| s.get(frame).copied()).unwrap_or(0.0)
                            } else {
                                0.0
                            };
                            sum += x * w;
                        }
                    }
                    out_slice[frame] = sum;
                    if col == 0 && frame < frames {
                        out_0_scratch[frame] = sum;
                    } else if col == 1 && frame < frames {
                        out_1_scratch[frame] = sum;
                    }
                }
            } else {
                for frame in 0..frames {
                    let mut sum = 0.0f32;
                    for row in 0..MATRIX_SIZE {
                        let idx = row * MATRIX_SIZE + col;
                        s_col[row] += coeff * (targets[idx] - s_col[row]);
                        let w = s_col[row];
                        if w.abs() > 1e-7 && (col == 0 || col == 1) {
                            let x = if has_cv_in[row] {
                                cv_ins[row].get(frame).copied().unwrap_or(0.0)
                            } else if row == 0 {
                                audio_in_l.and_then(|s| s.get(frame).copied()).unwrap_or(0.0)
                            } else if row == 1 {
                                audio_in_r.and_then(|s| s.get(frame).copied()).unwrap_or(0.0)
                            } else {
                                0.0
                            };
                            sum += x * w;
                        }
                    }
                    if col == 0 {
                        out_0_scratch[frame] = sum;
                    } else if col == 1 {
                        out_1_scratch[frame] = sum;
                    }
                }
            }

            for (row, &val) in s_col.iter().enumerate() {
                self.smoothed[row * MATRIX_SIZE + col] = val;
            }
        }

        if let Some(audio_out) = output.audio_outputs.first_mut() {
            if audio_out.channels() >= 1 {
                let n = frames.min(audio_out.channel(0).len());
                audio_out.channel_mut(0)[..n].copy_from_slice(&out_0_scratch[..n]);
            }
            if audio_out.channels() >= 2 {
                let n = frames.min(audio_out.channel(1).len());
                audio_out.channel_mut(1)[..n].copy_from_slice(&out_1_scratch[..n]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, NodeCommand, SignalInputSlot,
        SignalOutputSlot, SignalPortKind, TransportInfo, CMD_SET_PARAM,
    };

    #[test]
    fn matrix_node_routes_and_smooths_crosspoints_without_clicks() {
        let mut mat = MatrixNode::new();
        mat.activate(48_000.0, 64);

        // Set cell_2_5 (input 2 -> output 5) to 1.0 via CMD_SET_PARAM
        let pid = MatrixNode::cell_param_id(2, 5).expect("cell_2_5 id");
        let cmds = [NodeCommand {
            target_id: 0,
            type_id: CMD_SET_PARAM,
            arg0: pid as i64,
            arg1: 1.0,
        }];

        let in2 = [1.0f32; 64];
        let mut out5 = [0.0f32; 64];
        let sig_in = [SignalInputSlot::new(2, SignalPortKind::Cv, &in2)];
        let mut sig_out = [SignalOutputSlot::new(8 + 5, SignalPortKind::Cv, &mut out5)];

        let mut audio = AudioBuffer::new(2, 64);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let mut outs = [&mut audio];

        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &sig_in,
            events: &[],
            transport: &transport,
            sample_rate: 48_000.0,
            block_size: 64,
            extended_events: &slab,
            commands: &cmds,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut sig_out, &mut events_out);
        mat.process(&input, &mut output);

        // Smoothed output must ramp monotonically upward from >0 toward 1.0 without a step jump
        assert!(out5[0] > 0.0 && out5[0] < 0.05, "first sample must start ramping smoothly, got {}", out5[0]);
        assert!(out5[63] > out5[0], "output must ramp upward across the block");

        // Serialization round-trip preserves cell_2_5
        let blob = mat.serialize();
        let mut mat2 = MatrixNode::new();
        mat2.activate(48_000.0, 64);
        mat2.deserialize(&blob);
        let mut state = Vec::new();
        mat2.published_state(&mut state);
        let mask = state
            .iter()
            .find(|(k, _)| k.ends_with("/state/active_cells"))
            .and_then(|(_, v)| match v {
                StateBusValue::Int(m) => Some(*m),
                _ => None,
            })
            .expect("active_cells published");
        let bit = 2 * 8 + 5;
        assert_eq!((mask >> bit) & 1, 1, "active_cells bitmask must reflect cell_2_5");
    }
}
