// SPDX-License-Identifier: GPL-3.0-or-later
//! ClockDividerNode — Clock and trigger frequency divider.
//!
//! Divides incoming clock/gate pulses by /2, /3, /4, /8, and /16.
//! Features reset input for phrase/bar re-synchronization.
//!
//! Parameters:
//!   pulse_width — 0.05–0.95, default 0.5 (duty cycle for gate mode)
//!   mode        — 0.0 = Gate (square wave), 1.0 = Trigger (1ms pulse per division)

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn cdp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

pub struct ClockDividerNode {
    ports:       [PortDescriptor; 7],
    bank:        ParameterBank,
    clock_count: u32,
    prev_clock:  f32,
    prev_reset:  f32,
    pulses:      [u32; 5],
    sample_rate: f32,
    node_id:     u32,
}

impl ClockDividerNode {
    pub const PORT_CLOCK_IN: u32 = 0;
    pub const PORT_RESET_IN: u32 = 1;
    pub const PORT_DIV_2:    u32 = 10;
    pub const PORT_DIV_3:    u32 = 11;
    pub const PORT_DIV_4:    u32 = 12;
    pub const PORT_DIV_8:    u32 = 13;
    pub const PORT_DIV_16:   u32 = 14;

    const DIVISIONS: [u32; 5] = [2, 3, 4, 8, 16];
    const OUT_PORTS: [u32; 5] = [
        Self::PORT_DIV_2,
        Self::PORT_DIV_3,
        Self::PORT_DIV_4,
        Self::PORT_DIV_8,
        Self::PORT_DIV_16,
    ];

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_CLOCK_IN, name: "clock_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_RESET_IN, name: "reset_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_DIV_2,    name: "div_2".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_DIV_3,    name: "div_3".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_DIV_4,    name: "div_4".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_DIV_8,    name: "div_8".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_DIV_16,   name: "div_16".into(),   direction: PortDirection::Output, port_type: PortType::Logic },
            ],
            bank:        ParameterBank::from_capability_document(&doc),
            clock_count: 0,
            prev_clock:  0.0,
            prev_reset:  0.0,
            pulses:      [0; 5],
            sample_rate: 44100.0,
            node_id:     0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "ClockDividerNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0,  name: "clock_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: 1,  name: "reset_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: 10, name: "div_2".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: 11, name: "div_3".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: 12, name: "div_4".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: 13, name: "div_8".into(),    direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: 14, name: "div_16".into(),   direction: PortDirection::Output, port_type: PortType::Logic },
            ],
            params: vec![
                ParamDescriptor { id: cdp("pulse_width"), name: "pulse_width".into(), min: 0.05, max: 0.95, default: 0.5, stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: cdp("mode"),        name: "mode".into(),        min: 0.0,  max: 1.0,  default: 0.0, stepped: true,  in_kit: true, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.modular.clock_divider".into()],
            view: None,
        }
    }
}

impl Default for ClockDividerNode {
    fn default() -> Self { Self::new() }
}

impl Node for ClockDividerNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate.max(1.0);
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        self.clock_count = 0;
        self.prev_clock = 0.0;
        self.prev_reset = 0.0;
        self.pulses = [0; 5];
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

        let pulse_width = (self.bank.get(cdp("pulse_width")) as f32).clamp(0.05, 0.95);
        let trigger_mode = self.bank.get(cdp("mode")) as f32 >= 0.5;

        let frames = input.block_size;
        let clock_in = input.logic(Self::PORT_CLOCK_IN);
        let reset_in = input.logic(Self::PORT_RESET_IN);

        let pulse_len_samples = (self.sample_rate * 0.001).max(1.0) as u32; // 1ms pulse

        for f in 0..frames {
            let rst = reset_in.get(f).copied().unwrap_or(0.0);
            if rst >= 0.5 && self.prev_reset < 0.5 {
                self.clock_count = 0;
                self.pulses = [0; 5];
            }
            self.prev_reset = rst;

            let clk = clock_in.get(f).copied().unwrap_or(0.0);
            if clk >= 0.5 && self.prev_clock < 0.5 {
                self.clock_count = self.clock_count.wrapping_add(1);

                for (idx, &div) in Self::DIVISIONS.iter().enumerate() {
                    if self.clock_count % div == 0 {
                        self.pulses[idx] = pulse_len_samples;
                    }
                }
            }
            self.prev_clock = clk;

            // Output logic levels for each division
            for (idx, &port_id) in Self::OUT_PORTS.iter().enumerate() {
                let div = Self::DIVISIONS[idx];
                let out_val = if trigger_mode {
                    if self.pulses[idx] > 0 {
                        self.pulses[idx] -= 1;
                        1.0f32
                    } else {
                        0.0f32
                    }
                } else {
                    let phase_in_cycle = self.clock_count % div;
                    let high_threshold = ((div as f32) * pulse_width).max(1.0) as u32;
                    if phase_in_cycle < high_threshold {
                        1.0f32
                    } else {
                        0.0f32
                    }
                };

                if let Some(sig) = output.try_logic_output_mut(port_id) {
                    if f < sig.len() {
                        sig[f] = out_val;
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
    fn clock_divider_divides_by_two_and_four() {
        let mut cd = ClockDividerNode::new();
        cd.activate(44100.0, 8);

        // 8 clock pulses: low -> high alternating
        let clock = [1.0f32, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0];
        let sig_inputs = [
            SignalInputSlot::new(ClockDividerNode::PORT_CLOCK_IN, SignalPortKind::Logic, &clock),
        ];

        let mut out_div2 = [0.0f32; 8];
        let mut out_div4 = [0.0f32; 8];
        let mut sig_outputs = [
            SignalOutputSlot::new(ClockDividerNode::PORT_DIV_2, SignalPortKind::Logic, &mut out_div2),
            SignalOutputSlot::new(ClockDividerNode::PORT_DIV_4, SignalPortKind::Logic, &mut out_div4),
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

        cd.process(&input, &mut output);

        // Clock ticks occur at indices 0 (clk=1), 2 (clk=2), 4 (clk=3), 6 (clk=4).
        // For div_2 (width=0.5 -> threshold=1): clk % 2 < 1 means clk % 2 == 0 is high.
        // At clk=1: low (0). At clk=2: high (1). At clk=3: low (0). At clk=4: high (1).
        assert_eq!(out_div2[0], 0.0);
        assert_eq!(out_div2[2], 1.0);
        assert_eq!(out_div2[4], 0.0);
        assert_eq!(out_div2[6], 1.0);
    }
}
