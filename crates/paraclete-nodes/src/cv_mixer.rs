// SPDX-License-Identifier: GPL-3.0-or-later
//! CvMixerNode — 4-channel CV and audio utility mixer with attenuverters and DC offset.
//!
//! Provides simultaneous summing for CV signals and audio signals.
//! Emits normal and inverted CV sum outputs.
//!
//! Parameters:
//!   level_0 .. level_3 — -2.0–2.0, default 1.0 (bipolar attenuverter per channel)
//!   offset             — -10.0–10.0, default 0.0 (DC offset added to CV sum)
//!   master             — 0.0–2.0, default 1.0 (master level)

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn mp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

pub struct CvMixerNode {
    ports:   [PortDescriptor; 8],
    bank:    ParameterBank,
    node_id: u32,
}

impl CvMixerNode {
    pub const PORT_CV_0:       u32 = 0;
    pub const PORT_CV_1:       u32 = 1;
    pub const PORT_CV_2:       u32 = 2;
    pub const PORT_CV_3:       u32 = 3;
    pub const PORT_AUDIO_IN:   u32 = 4;
    pub const PORT_CV_OUT:     u32 = 10;
    pub const PORT_CV_OUT_INV: u32 = 11;
    pub const PORT_AUDIO_OUT:  u32 = 12;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_CV_0,       name: "cv_0".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_CV_1,       name: "cv_1".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_CV_2,       name: "cv_2".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_CV_3,       name: "cv_3".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_AUDIO_IN,   name: "audio_in".into(),   direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_CV_OUT,     name: "cv_out".into(),     direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_CV_OUT_INV, name: "cv_out_inv".into(), direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_AUDIO_OUT,  name: "audio_out".into(),  direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            bank:    ParameterBank::from_capability_document(&doc),
            node_id: 0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "CvMixerNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0,  name: "cv_0".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 1,  name: "cv_1".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 2,  name: "cv_2".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 3,  name: "cv_3".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 4,  name: "audio_in".into(),   direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 10, name: "cv_out".into(),     direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: 11, name: "cv_out_inv".into(), direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: 12, name: "audio_out".into(),  direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: mp("level_0"), name: "level_0".into(), min: -2.0,  max: 2.0,  default: 1.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: mp("level_1"), name: "level_1".into(), min: -2.0,  max: 2.0,  default: 1.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: mp("level_2"), name: "level_2".into(), min: -2.0,  max: 2.0,  default: 1.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: mp("level_3"), name: "level_3".into(), min: -2.0,  max: 2.0,  default: 1.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: mp("offset"),  name: "offset".into(),  min: -10.0, max: 10.0, default: 0.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: mp("master"),  name: "master".into(),  min: 0.0,   max: 2.0,  default: 1.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.modular.cv_mixer".into()],
            view: None,
        }
    }
}

impl Default for CvMixerNode {
    fn default() -> Self { Self::new() }
}

impl Node for CvMixerNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
    }

    fn serialize(&self) -> Vec<u8> {
        self.bank.serialize()
    }

    fn deserialize(&mut self, data: &[u8]) {
        self.bank.deserialize(data);
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let levels = [
            self.bank.get(mp("level_0")) as f32,
            self.bank.get(mp("level_1")) as f32,
            self.bank.get(mp("level_2")) as f32,
            self.bank.get(mp("level_3")) as f32,
        ];
        let offset = self.bank.get(mp("offset")) as f32;
        let master = self.bank.get(mp("master")) as f32;

        let frames = input.block_size;

        let cv_in_0 = input.cv_signal(Self::PORT_CV_0);
        let cv_in_1 = input.cv_signal(Self::PORT_CV_1);
        let cv_in_2 = input.cv_signal(Self::PORT_CV_2);
        let cv_in_3 = input.cv_signal(Self::PORT_CV_3);

        let mod_in_0 = input.modulation(Self::PORT_CV_0);
        let mod_in_1 = input.modulation(Self::PORT_CV_1);
        let mod_in_2 = input.modulation(Self::PORT_CV_2);
        let mod_in_3 = input.modulation(Self::PORT_CV_3);

        let has_cv_out = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_CV_OUT);
        let has_cv_out_inv = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_CV_OUT_INV);

        if has_cv_out || has_cv_out_inv {
            for f in 0..frames {
                let v0 = cv_in_0.get(f).copied().unwrap_or(0.0) + mod_in_0.get(f).copied().unwrap_or(0.0);
                let v1 = cv_in_1.get(f).copied().unwrap_or(0.0) + mod_in_1.get(f).copied().unwrap_or(0.0);
                let v2 = cv_in_2.get(f).copied().unwrap_or(0.0) + mod_in_2.get(f).copied().unwrap_or(0.0);
                let v3 = cv_in_3.get(f).copied().unwrap_or(0.0) + mod_in_3.get(f).copied().unwrap_or(0.0);

                let sum = (v0 * levels[0] + v1 * levels[1] + v2 * levels[2] + v3 * levels[3] + offset) * master;

                if let Some(cv_out) = output.try_cv_signal_output_mut(Self::PORT_CV_OUT) {
                    if f < cv_out.len() {
                        cv_out[f] = sum;
                    }
                }
                if let Some(cv_out_inv) = output.try_cv_signal_output_mut(Self::PORT_CV_OUT_INV) {
                    if f < cv_out_inv.len() {
                        cv_out_inv[f] = -sum;
                    }
                }
            }
        }

        if !input.audio_inputs.is_empty() && !output.audio_outputs.is_empty() {
            let audio_in = &input.audio_inputs[0];
            let audio_out = &mut output.audio_outputs[0];
            let channels = audio_in.channels().min(audio_out.channels());
            for ch in 0..channels {
                let src = audio_in.channel(ch);
                let dst = audio_out.channel_mut(ch);
                for f in 0..frames.min(src.len()).min(dst.len()) {
                    dst[f] = src[f] * master;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        EventOutputBuffer, ExtendedEventSlab, SignalInputSlot, SignalOutputSlot,
        SignalPortKind, TransportInfo,
    };

    #[test]
    fn cv_mixer_sums_and_inverts() {
        let mut mixer = CvMixerNode::new();
        mixer.activate(44100.0, 4);

        let ch0 = [1.0f32; 4];
        let ch1 = [2.0f32; 4];
        let sig_inputs = [
            SignalInputSlot::new(CvMixerNode::PORT_CV_0, SignalPortKind::Cv, &ch0),
            SignalInputSlot::new(CvMixerNode::PORT_CV_1, SignalPortKind::Cv, &ch1),
        ];

        let mut cv_out = [0.0f32; 4];
        let mut cv_out_inv = [0.0f32; 4];
        let mut sig_outputs = [
            SignalOutputSlot::new(CvMixerNode::PORT_CV_OUT, SignalPortKind::Cv, &mut cv_out),
            SignalOutputSlot::new(CvMixerNode::PORT_CV_OUT_INV, SignalPortKind::Cv, &mut cv_out_inv),
        ];

        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let extended = ExtendedEventSlab::empty();

        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &sig_inputs,
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 4,
            extended_events: &extended,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut [], &mut sig_outputs, &mut events_out);

        mixer.process(&input, &mut output);

        // Sum = 1.0 * 1.0 + 2.0 * 1.0 + 0.0 = 3.0. Inverted = -3.0.
        for &v in &cv_out {
            assert!((v - 3.0).abs() < 1e-5);
        }
        for &v in &cv_out_inv {
            assert!((v - (-3.0)).abs() < 1e-5);
        }
    }
}
