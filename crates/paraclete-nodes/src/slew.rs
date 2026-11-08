// SPDX-License-Identifier: GPL-3.0-or-later
//! SlewNode — Independent rise/fall slew rate limiter and portamento filter.
//!
//! Smooths CV transitions for pitch glide or control smoothing.
//! Supports linear and exponential slew curves, and optional gated legato mode.
//!
//! Parameters:
//!   rise   — 0.0001–10.0 seconds, default 0.01 (10ms)
//!   fall   — 0.0001–10.0 seconds, default 0.01 (10ms)
//!   shape  — 0.0–1.0, default 0.0 (0.0 = Linear, 1.0 = Exponential RC)
//!   legato — 0.0–1.0, default 0.0 (stepped, 0 = always active, 1 = slew only while gate is high)

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn sp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

pub struct SlewNode {
    ports:       [PortDescriptor; 3],
    bank:        ParameterBank,
    current_val: f32,
    sample_rate: f32,
    node_id:     u32,
}

impl SlewNode {
    pub const PORT_CV_IN:   u32 = 0;
    pub const PORT_GATE_IN: u32 = 1;
    pub const PORT_CV_OUT:  u32 = 2;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_CV_IN,   name: "cv_in".into(),   direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_GATE_IN, name: "gate_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_CV_OUT,  name: "cv_out".into(),  direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            bank:        ParameterBank::from_capability_document(&doc),
            current_val: 0.0,
            sample_rate: 44100.0,
            node_id:     0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "SlewNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "cv_in".into(),   direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 1, name: "gate_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: 2, name: "cv_out".into(),  direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            params: vec![
                ParamDescriptor { id: sp("rise"),   name: "rise".into(),   min: 0.0001, max: 10.0, default: 0.01, stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None },
                ParamDescriptor { id: sp("fall"),   name: "fall".into(),   min: 0.0001, max: 10.0, default: 0.01, stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None },
                ParamDescriptor { id: sp("shape"),  name: "shape".into(),  min: 0.0,    max: 1.0,  default: 0.0,  stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: sp("legato"), name: "legato".into(), min: 0.0,    max: 1.0,  default: 0.0,  stepped: true,  in_kit: true, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.modular.slew".into()],
            view: None,
        }
    }
}

impl Default for SlewNode {
    fn default() -> Self { Self::new() }
}

impl Node for SlewNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate.max(1.0);
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        self.current_val = 0.0;
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

        let rise = (self.bank.get(sp("rise")) as f32).max(0.0001);
        let fall = (self.bank.get(sp("fall")) as f32).max(0.0001);
        let shape = (self.bank.get(sp("shape")) as f32).clamp(0.0, 1.0);
        let legato = self.bank.get(sp("legato")) as f32 >= 0.5;

        let frames = input.block_size;
        let cv_in = input.cv_signal(Self::PORT_CV_IN);
        let mod_in = input.modulation(Self::PORT_CV_IN);
        let gate_in = input.logic(Self::PORT_GATE_IN);

        if let Some(cv_out) = output.try_cv_signal_output_mut(Self::PORT_CV_OUT) {
            let sr = self.sample_rate;
            let linear_rise_step = 1.0 / (rise * sr);
            let linear_fall_step = 1.0 / (fall * sr);
            let exp_rise_alpha = 1.0 - (-1.0 / (rise * sr)).exp();
            let exp_fall_alpha = 1.0 - (-1.0 / (fall * sr)).exp();

            for f in 0..frames.min(cv_out.len()) {
                let target = cv_in.get(f).copied().unwrap_or(0.0) + mod_in.get(f).copied().unwrap_or(0.0);
                let gate = gate_in.get(f).copied().unwrap_or(1.0);

                if legato && gate < 0.5 {
                    self.current_val = target;
                } else if target > self.current_val {
                    let delta = target - self.current_val;
                    let lin_step = delta.min(linear_rise_step);
                    let exp_step = delta * exp_rise_alpha;
                    let step = lin_step * (1.0 - shape) + exp_step * shape;
                    self.current_val += step;
                } else if target < self.current_val {
                    let delta = self.current_val - target;
                    let lin_step = delta.min(linear_fall_step);
                    let exp_step = delta * exp_fall_alpha;
                    let step = lin_step * (1.0 - shape) + exp_step * shape;
                    self.current_val -= step;
                }

                cv_out[f] = self.current_val;
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
    fn slew_linear_rise_limits_rate() {
        let mut slew = SlewNode::new();
        // 100 Hz sample rate: 10 samples = 0.1s.
        slew.activate(100.0, 10);
        // rise = 0.1s -> linear step = 1.0 / (0.1 * 100) = 0.1 per sample.
        slew.bank.set(sp("rise"), 0.1);

        let target = [1.0f32; 10];
        let sig_inputs = [
            SignalInputSlot::new(SlewNode::PORT_CV_IN, SignalPortKind::Cv, &target),
        ];

        let mut cv_out = [0.0f32; 10];
        let mut sig_outputs = [
            SignalOutputSlot::new(SlewNode::PORT_CV_OUT, SignalPortKind::Cv, &mut cv_out),
        ];

        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let extended = ExtendedEventSlab::empty();

        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &sig_inputs,
            events: &[],
            transport: &transport,
            sample_rate: 100.0,
            block_size: 10,
            extended_events: &extended,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut [], &mut sig_outputs, &mut events_out);

        slew.process(&input, &mut output);

        // After 1 sample: 0.1; after 5 samples: 0.5; after 10 samples: 1.0.
        assert!((cv_out[0] - 0.1).abs() < 1e-4);
        assert!((cv_out[4] - 0.5).abs() < 1e-4);
        assert!((cv_out[9] - 1.0).abs() < 1e-4);
        assert!((cv_out[4] - 0.5).abs() < 1e-4);
        assert!((cv_out[9] - 1.0).abs() < 1e-4);
    }
}
