// SPDX-License-Identifier: GPL-3.0-or-later
//! DistortionNode — soft-clipping saturation.
//!
//! Parameters:
//!   drive    (id=0) — 0.0–1.0, default 0.0 (maps to 0–4× pre-gain)
//!   output_level (id=1) — −24.0..+6.0 dB, default 0.0
//!   blend    (id=2) — 0.0–1.0, default 1.0 (wet/dry)
//!
//! DSP: tanh(input × exp(drive × 4)) × db_to_linear(output_level)
//! lerped with dry signal by `blend`.

use std::collections::HashMap;

use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

// Sequential IDs rather than id_for_name() hashes. These must match the order
// in capability_document() — the profile scripts reference them by these numeric values.
const PARAM_DRIVE:        u32 = 0;
const PARAM_OUTPUT_LEVEL: u32 = 1;
const PARAM_BLEND:        u32 = 2;

pub struct DistortionNode {
    ports: [PortDescriptor; 2],
    node_id: u32,
    bank: ParameterBank,
    pending_initial_params: HashMap<String, f64>,
}

impl DistortionNode {
    pub const PORT_AUDIO_IN:  u32 = 0;
    pub const PORT_AUDIO_OUT: u32 = 1;

    pub fn new() -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_AUDIO_IN,
                    name: "audio_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Audio,
                },
                PortDescriptor {
                    id: Self::PORT_AUDIO_OUT,
                    name: "audio_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Audio,
                },
            ],
            node_id: 0,
            bank: ParameterBank::empty(),
            pending_initial_params: HashMap::new(),
        }
    }

    pub fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "DistortionNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 4, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: PARAM_DRIVE,        name: "drive".into(),        min: 0.0, max: 1.0, default: 0.0,  stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_OUTPUT_LEVEL, name: "output_level".into(), min: -24.0, max: 6.0, default: 0.0, stepped: false, unit: ParamUnit::Decibels, display: None },
                ParamDescriptor { id: PARAM_BLEND,        name: "blend".into(),        min: 0.0, max: 1.0, default: 1.0,  stepped: false, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.effect".into()],
        }
    }
}

impl Default for DistortionNode {
    fn default() -> Self { Self::new() }
}

#[inline(always)]
fn db_to_linear(db: f64) -> f32 {
    (10.0f64.powf(db / 20.0)) as f32
}

impl Node for DistortionNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
        self.pending_initial_params = params.clone();
    }

    fn activate(&mut self, _sr: f32, _block: usize) {
        let doc = Self::default_doc();
        self.bank = ParameterBank::from_capability_document(&doc);
        // BUG-008 fix: consume the pending map so a re-activate (dynamic
        // topology rebuild, P9 C4) cannot overwrite deserialized state.
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, value);
            }
        }
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let drive  = self.bank.get(PARAM_DRIVE) as f32;
        let out_db = self.bank.get(PARAM_OUTPUT_LEVEL);
        let blend  = self.bank.get(PARAM_BLEND) as f32;
        let gain   = (drive * 4.0).exp();
        let level  = db_to_linear(out_db);

        if let (Some(audio_in), Some(audio_out)) = (
            input.audio_inputs.first(),
            output.audio_outputs.first_mut(),
        ) {
            let frames = input.block_size;
            for ch in 0..2usize.min(audio_in.channels()).min(audio_out.channels()) {
                let src = audio_in.channel(ch);
                let dst = audio_out.channel_mut(ch);
                for f in 0..frames {
                    let dry = src[f];
                    let wet = (dry * gain).tanh() * level;
                    dst[f] = dry + blend * (wet - dry);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    fn run_distortion(dist: &mut DistortionNode, input_val: f32) -> f32 {
        let block = 64usize;
        let mut src = AudioBuffer::new(2, block);
        let mut dst = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        src.channel_mut(0).fill(input_val);

        let dst_ptr: *mut AudioBuffer = &mut dst;
        let dst_ref: &mut AudioBuffer = unsafe { &mut *dst_ptr };
        let mut outs = [dst_ref];

        let input = ProcessInput {
            audio_inputs: &[&src],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        dist.process(&input, &mut output);
        dst.channel(0)[0]
    }

    #[test]
    fn distortion_at_zero_drive_passes_signal_through() {
        let mut dist = DistortionNode::new();
        dist.activate(44100.0, 64);
        // At drive=0, blend=1: out = tanh(input * 1.0) * 1.0
        // For small input (0.01), tanh(0.01) ≈ 0.01 — less than 0.01% error.
        let out = run_distortion(&mut dist, 0.01);
        assert!((out - 0.01).abs() < 0.001, "expected ~0.01, got {out}");
    }

    #[test]
    fn distortion_at_max_drive_clips_near_unity() {
        let mut dist = DistortionNode::new();
        dist.activate(44100.0, 64);
        // Set drive to 1.0
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        let cmd = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_DRIVE as i64, arg1: 1.0 };
        dist.bank.handle_commands(&[cmd]);
        let out = run_distortion(&mut dist, 10.0);
        assert!(out.abs() <= 1.1, "expected clipped output, got {out}");
    }

    #[test]
    fn distortion_node_published_state_nonzero() {
        let mut dist = DistortionNode::new();
        dist.set_node_id(3);
        dist.activate(44100.0, 64);
        let mut buf: Vec<(String, StateBusValue)> = Vec::new();
        dist.published_state(&mut buf);
        let entry = buf.iter().find(|(k, _)| k == "/node/3/param/drive");
        assert!(entry.is_some(), "expected /node/3/param/drive in published_state");
        assert_eq!(entry.unwrap().1, StateBusValue::Float(0.0));
    }
}
