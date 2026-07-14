// SPDX-License-Identifier: GPL-3.0-or-later
//! SplitNode — routes one audio stream to two independent outputs with per-output gain.
//! Enables send/return topologies. No state; activate() builds ParameterBank only.
//!
//! Parameters:
//!   gain_0 — 0.0–2.0, default 1.0
//!   gain_1 — 0.0–2.0, default 1.0

use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
};

const PARAM_GAIN_0: u32 = 0;
const PARAM_GAIN_1: u32 = 1;

pub struct SplitNode {
    ports: [PortDescriptor; 3],
    bank:  ParameterBank,
}

impl SplitNode {
    pub const PORT_AUDIO_IN:   u32 = 0;
    pub const PORT_AUDIO_OUT_0: u32 = 1;
    pub const PORT_AUDIO_OUT_1: u32 = 2;

    pub fn new() -> Self {
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_AUDIO_IN,    name: "audio_in".into(),    direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_0, name: "audio_out_0".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_1, name: "audio_out_1".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            bank: ParameterBank::empty(),
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "SplitNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 5, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),    direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out_0".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: 2, name: "audio_out_1".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: PARAM_GAIN_0, name: "gain_0".into(), min: 0.0, max: 2.0, default: 1.0, stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_GAIN_1, name: "gain_1".into(), min: 0.0, max: 2.0, default: 1.0, stepped: false, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.split".into()],
        }
    }
}

impl Default for SplitNode {
    fn default() -> Self { Self::new() }
}

impl Node for SplitNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn activate(&mut self, _sr: f32, _block: usize) {
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let gain_0 = self.bank.get(PARAM_GAIN_0) as f32;
        let gain_1 = self.bank.get(PARAM_GAIN_1) as f32;

        // SplitNode has one input but two outputs. The executor provides one audio_out
        // buffer per slot; to emit two outputs we write to the single audio_out using
        // gain_0 (the "first" output path, summed by the executor as-is). gain_1 output
        // is written to audio_outputs[1] if present.
        //
        // NOTE: The executor currently gives each node exactly one audio_out buffer.
        // Multi-output audio is a P6+ feature. For P5, we write gain_0 to audio_outputs[0]
        // and note the gain_1 path as a stub (gain_1 output effectively mirrors gain_0).
        if let Some(audio_in) = input.audio_inputs.first() {
            let frames = input.block_size;

            // Output 0 (primary) — gain_0 applied
            if let Some(audio_out_0) = output.audio_outputs.first_mut() {
                for ch in 0..2usize.min(audio_in.channels()).min(audio_out_0.channels()) {
                    let src = audio_in.channel(ch);
                    let dst = audio_out_0.channel_mut(ch);
                    for f in 0..frames {
                        dst[f] = src[f] * gain_0;
                    }
                }
            }

            // Output 1 (secondary) — gain_1 applied, if a second output buffer exists
            if let Some(audio_out_1) = output.audio_outputs.get_mut(1) {
                for ch in 0..2usize.min(audio_in.channels()).min(audio_out_1.channels()) {
                    let src = audio_in.channel(ch);
                    let dst = audio_out_1.channel_mut(ch);
                    for f in 0..frames {
                        dst[f] = src[f] * gain_1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    fn run_split_single_output(split: &mut SplitNode, input_val: f32) -> f32 {
        let frames = 64usize;
        let mut src = AudioBuffer::new(2, frames);
        let mut dst = AudioBuffer::new(2, frames);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        src.channel_mut(0).fill(input_val);

        let dst_ptr: *mut AudioBuffer = &mut dst;
        let dst_ref: &mut AudioBuffer = unsafe { &mut *dst_ptr };
        let mut outs = [dst_ref];

        split.process(&ProcessInput {
            audio_inputs: &[&src], signal_inputs: &[], events: &[],
            transport: &transport, sample_rate: 44100.0, block_size: frames,
            extended_events: &slab, commands: &[],
        },     &mut ProcessOutput::new(
            &mut outs, &mut [],
            &mut events_out,
        ));
        dst.channel(0)[0]
    }

    #[test]
    fn split_unity_gain_passes_signal_unchanged() {
        let mut s = SplitNode::new();
        s.activate(44100.0, 64);
        let out = run_split_single_output(&mut s, 0.75);
        assert!((out - 0.75).abs() < 1e-5, "unity gain must not alter signal");
    }

    #[test]
    fn split_gain_0_scales_output() {
        let mut s = SplitNode::new();
        s.activate(44100.0, 64);
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        s.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_GAIN_0 as i64, arg1: 0.5 },
        ]);
        let out = run_split_single_output(&mut s, 1.0);
        assert!((out - 0.5).abs() < 1e-5, "gain_0=0.5 must halve signal");
    }

    #[test]
    fn split_zero_gain_silences_output() {
        let mut s = SplitNode::new();
        s.activate(44100.0, 64);
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        s.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_GAIN_0 as i64, arg1: 0.0 },
        ]);
        let out = run_split_single_output(&mut s, 1.0);
        assert!(out.abs() < 1e-5, "gain_0=0 must silence output");
    }

    #[test]
    fn split_ports_declared_correctly() {
        let s = SplitNode::new();
        assert_eq!(s.ports().len(), 3);
        assert_eq!(s.ports()[0].direction, PortDirection::Input);
        assert_eq!(s.ports()[1].direction, PortDirection::Output);
        assert_eq!(s.ports()[2].direction, PortDirection::Output);
    }

    #[test]
    fn split_capability_document_has_two_gain_params() {
        let s = SplitNode::new();
        let doc = s.capability_document();
        assert_eq!(doc.params.len(), 2);
        assert_eq!(doc.params[0].id, PARAM_GAIN_0);
        assert_eq!(doc.params[1].id, PARAM_GAIN_1);
        assert_eq!(doc.params[0].default, 1.0);
        assert_eq!(doc.params[1].default, 1.0);
    }
}
