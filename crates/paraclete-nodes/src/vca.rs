// SPDX-License-Identifier: GPL-3.0-or-later
//! VcaNode — Voltage-Controlled Amplifier and Attenuverter.
//!
//! Provides simultaneous audio and CV attenuation/modulation.
//! Supports linear and exponential response curves.
//!
//! Parameters:
//!   level     — 0.0–2.0, default 1.0 (manual base level)
//!   cv_amount — -2.0–2.0, default 1.0 (bipolar CV attenuverter)
//!   bias      — -1.0–1.0, default 0.0 (DC control bias)
//!   response  — 0.0–1.0, default 0.0 (0.0 = Linear, 1.0 = Exponential)

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn vp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

pub struct VcaNode {
    ports:   [PortDescriptor; 5],
    bank:    ParameterBank,
    node_id: u32,
}

impl VcaNode {
    pub const PORT_AUDIO_IN:  u32 = 0;
    pub const PORT_AUDIO_OUT: u32 = 1;
    pub const PORT_CV_IN:     u32 = 2;
    pub const PORT_MOD_IN:    u32 = 3;
    pub const PORT_CV_OUT:    u32 = 4;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_AUDIO_IN,  name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_CV_IN,     name: "cv_in".into(),     direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_MOD_IN,    name: "mod_in".into(),    direction: PortDirection::Input,  port_type: PortType::Modulation },
                PortDescriptor { id: Self::PORT_CV_OUT,    name: "cv_out".into(),    direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            bank:    ParameterBank::from_capability_document(&doc),
            node_id: 0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "VcaNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: 2, name: "cv_in".into(),     direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 3, name: "mod_in".into(),    direction: PortDirection::Input,  port_type: PortType::Modulation },
                PortDescriptor { id: 4, name: "cv_out".into(),    direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            params: vec![
                ParamDescriptor { id: vp("level"),     name: "level".into(),     min: 0.0,  max: 2.0, default: 1.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: vp("cv_amount"), name: "cv_amount".into(), min: -2.0, max: 2.0, default: 1.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: vp("bias"),      name: "bias".into(),      min: -1.0, max: 1.0, default: 0.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: vp("response"),  name: "response".into(),  min: 0.0,  max: 1.0, default: 0.0, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.modular.vca".into()],
            view: None,
        }
    }
}

impl Default for VcaNode {
    fn default() -> Self { Self::new() }
}

impl Node for VcaNode {
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

        let level     = self.bank.get(vp("level")) as f32;
        let cv_amount = self.bank.get(vp("cv_amount")) as f32;
        let bias      = self.bank.get(vp("bias")) as f32;
        let response  = self.bank.get(vp("response")) as f32;

        let frames = input.block_size;
        let mod_in = input.modulation(Self::PORT_MOD_IN);
        let cv_ctrl = input.cv_signal(Self::PORT_MOD_IN);

        let has_audio_in = !input.audio_inputs.is_empty();
        let has_audio_out = !output.audio_outputs.is_empty();
        let has_cv_out = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_CV_OUT);

        if has_audio_in && has_audio_out {
            let audio_in = &input.audio_inputs[0];
            let audio_out = &mut output.audio_outputs[0];
            let channels = audio_in.channels().min(audio_out.channels());

            for f in 0..frames {
                let m = mod_in.get(f).copied().unwrap_or(0.0) + cv_ctrl.get(f).copied().unwrap_or(0.0);
                let c = level + cv_amount * m + bias;

                let gain = if response > 0.0 {
                    let lin = c.max(0.0);
                    let exp = if c <= 0.0 { 0.0 } else { ((c * 3.0).exp2() - 1.0) / 7.0 };
                    lin * (1.0 - response) + exp * response
                } else {
                    c.max(0.0)
                };

                for ch in 0..channels {
                    audio_out.channel_mut(ch)[f] = audio_in.channel(ch)[f] * gain;
                }
            }
        }

        if has_cv_out {
            let cv_in = input.cv_signal(Self::PORT_CV_IN);
            if let Some(cv_out) = output.try_cv_signal_output_mut(Self::PORT_CV_OUT) {
                for f in 0..frames.min(cv_out.len()) {
                    let m = mod_in.get(f).copied().unwrap_or(0.0) + cv_ctrl.get(f).copied().unwrap_or(0.0);
                    let c = level + cv_amount * m + bias;
                    let sig = cv_in.get(f).copied().unwrap_or(0.0);
                    cv_out[f] = sig * c;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, SignalInputSlot, SignalOutputSlot,
        SignalPortKind, TransportInfo,
    };

    #[test]
    fn vca_audio_linear_attenuation() {
        let mut vca = VcaNode::new();
        vca.activate(44100.0, 16);

        let mut in_buf = AudioBuffer::new(1, 16);
        in_buf.channel_mut(0).fill(1.0);
        let mut out_buf = AudioBuffer::new(1, 16);

        let audio_inputs = [&in_buf];
        let mut audio_outputs = [&mut out_buf];
        let mut signal_outs: [SignalOutputSlot; 0] = [];
        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let extended = ExtendedEventSlab::empty();

        let input = ProcessInput {
            audio_inputs: &audio_inputs,
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 16,
            extended_events: &extended,
            commands: &[],
        };
        {
            let mut output = ProcessOutput::new(&mut audio_outputs, &mut signal_outs, &mut events_out);
            vca.process(&input, &mut output);
        }

        for &val in out_buf.channel(0) {
            assert!((val - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn vca_cv_bipolar_multiplication() {
        let mut vca = VcaNode::new();
        vca.activate(44100.0, 8);

        let cv_in = [0.5f32; 8];
        let mod_in = [-0.5f32; 8];
        let sig_inputs = [
            SignalInputSlot::new(VcaNode::PORT_CV_IN, SignalPortKind::Cv, &cv_in),
            SignalInputSlot::new(VcaNode::PORT_MOD_IN, SignalPortKind::Modulation, &mod_in),
        ];

        let mut cv_out = [0.0f32; 8];
        let mut sig_outputs = [
            SignalOutputSlot::new(VcaNode::PORT_CV_OUT, SignalPortKind::Cv, &mut cv_out),
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
            block_size: 8,
            extended_events: &extended,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut [], &mut sig_outputs, &mut events_out);

        vca.process(&input, &mut output);

        // level=1.0, cv_amount=1.0, bias=0.0 -> gain = 1.0 + 1.0 * (-0.5) = 0.5.
        // cv_out = cv_in (0.5) * gain (0.5) = 0.25.
        for &val in &cv_out {
            assert!((val - 0.25).abs() < 1e-5);
        }
    }
}
