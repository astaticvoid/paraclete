// SPDX-License-Identifier: GPL-3.0-or-later
//! SampleHoldNode — Sample & Hold and Track & Hold with internal white noise generator.
//!
//! When `cv_in` is unpatched or silent, automatically samples from the internal
//! zero-allocation Xorshift32 white noise generator (standard analog S&H convention).
//!
//! Parameters:
//!   mode — 0.0 = Sample & Hold (latches on trigger rising edge)
//!          1.0 = Track & Hold (passes signal while gate is high, latches on falling edge)
//!   slew — 0.0–1.0 seconds, default 0.0 (output smoothing)

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn shp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

pub struct SampleHoldNode {
    ports:        [PortDescriptor; 4],
    bank:         ParameterBank,
    held_value:   f32,
    prev_trigger: f32,
    noise_state:  u32,
    slew_val:     f32,
    sample_rate:  f32,
    node_id:      u32,
}

impl SampleHoldNode {
    pub const PORT_CV_IN:      u32 = 0;
    pub const PORT_TRIGGER_IN: u32 = 1;
    pub const PORT_CV_OUT:     u32 = 2;
    pub const PORT_NOISE_OUT:  u32 = 3;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_CV_IN,      name: "cv_in".into(),      direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_TRIGGER_IN, name: "trigger_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_CV_OUT,     name: "cv_out".into(),     direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_NOISE_OUT,  name: "noise_out".into(),  direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            bank:         ParameterBank::from_capability_document(&doc),
            held_value:   0.0,
            prev_trigger: 0.0,
            noise_state:  0xDEAD_BEEF,
            slew_val:     0.0,
            sample_rate:  44100.0,
            node_id:      0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "SampleHoldNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "cv_in".into(),      direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 1, name: "trigger_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: 2, name: "cv_out".into(),     direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: 3, name: "noise_out".into(),  direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            params: vec![
                ParamDescriptor { id: shp("mode"), name: "mode".into(), min: 0.0, max: 1.0, default: 0.0, stepped: true,  in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: shp("slew"), name: "slew".into(), min: 0.0, max: 1.0, default: 0.0, stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None },
            ],
            extensions: vec!["paraclete.modular.sample_hold".into()],
            view: None,
        }
    }

    #[inline(always)]
    fn next_noise(&mut self) -> f32 {
        self.noise_state ^= self.noise_state << 13;
        self.noise_state ^= self.noise_state >> 17;
        self.noise_state ^= self.noise_state << 5;
        (self.noise_state as i32 as f32) / (i32::MAX as f32)
    }
}

impl Default for SampleHoldNode {
    fn default() -> Self { Self::new() }
}

impl Node for SampleHoldNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate.max(1.0);
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        self.held_value = 0.0;
        self.prev_trigger = 0.0;
        self.slew_val = 0.0;
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

        let track_mode = self.bank.get(shp("mode")) as f32 >= 0.5;
        let slew_time = self.bank.get(shp("slew")) as f32;

        let frames = input.block_size;
        let has_input_cv = input.signal_inputs.iter().any(|s| s.port_id == Self::PORT_CV_IN);
        let cv_in = input.cv_signal(Self::PORT_CV_IN);
        let trg_in = input.logic(Self::PORT_TRIGGER_IN);

        let slew_alpha = if slew_time > 0.0001 {
            1.0 - (-1.0 / (slew_time * self.sample_rate)).exp()
        } else {
            1.0
        };

        let has_cv_out = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_CV_OUT);
        let has_noise_out = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_NOISE_OUT);

        if has_cv_out || has_noise_out {
            for f in 0..frames {
                let white_noise = self.next_noise();

                if let Some(noise_out) = output.try_cv_signal_output_mut(Self::PORT_NOISE_OUT) {
                    if f < noise_out.len() {
                        noise_out[f] = white_noise;
                    }
                }

                let src_sample = if has_input_cv {
                    cv_in.get(f).copied().unwrap_or(0.0)
                } else {
                    white_noise
                };

                let trg = trg_in.get(f).copied().unwrap_or(0.0);

                if track_mode {
                    // Track & Hold
                    if trg >= 0.5 {
                        self.held_value = src_sample;
                    }
                } else {
                    // Sample & Hold (rising edge)
                    if trg >= 0.5 && self.prev_trigger < 0.5 {
                        self.held_value = src_sample;
                    }
                }
                self.prev_trigger = trg;

                if slew_alpha < 1.0 {
                    self.slew_val += (self.held_value - self.slew_val) * slew_alpha;
                } else {
                    self.slew_val = self.held_value;
                }

                if let Some(cv_out) = output.try_cv_signal_output_mut(Self::PORT_CV_OUT) {
                    if f < cv_out.len() {
                        cv_out[f] = self.slew_val;
                    }
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
    fn sample_hold_latches_on_rising_edge() {
        let mut sh = SampleHoldNode::new();
        sh.activate(44100.0, 4);

        let input_cv = [0.1f32, 0.42f32, 0.99f32, 0.0f32];
        // Trigger goes high on sample 1 (0.42)
        let trigger = [0.0f32, 1.0f32, 1.0f32, 0.0f32];

        let sig_inputs = [
            SignalInputSlot::new(SampleHoldNode::PORT_CV_IN, SignalPortKind::Cv, &input_cv),
            SignalInputSlot::new(SampleHoldNode::PORT_TRIGGER_IN, SignalPortKind::Logic, &trigger),
        ];

        let mut cv_out = [0.0f32; 4];
        let mut sig_outputs = [
            SignalOutputSlot::new(SampleHoldNode::PORT_CV_OUT, SignalPortKind::Cv, &mut cv_out),
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

        sh.process(&input, &mut output);

        // Before trigger: 0.0. After sample 1 latches 0.42, remains 0.42 through samples 2 and 3.
        assert_eq!(cv_out[0], 0.0);
        assert!((cv_out[1] - 0.42).abs() < 1e-5);
        assert!((cv_out[2] - 0.42).abs() < 1e-5);
        assert!((cv_out[3] - 0.42).abs() < 1e-5);
    }
}
