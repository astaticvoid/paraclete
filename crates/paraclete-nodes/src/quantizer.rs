// SPDX-License-Identifier: GPL-3.0-or-later
//! QuantizerNode — 1V/oct musical pitch quantizer and note trigger generator.
//!
//! Maps continuous CV into discrete musical scale degrees with root note selection
//! and transposition. Emits quantized 1V/oct CV, trigger pulses on pitch changes,
//! and MIDI pitch values.
//!
//! Parameters:
//!   scale     — 0=Chromatic, 1=Major, 2=Minor, 3=Dorian, 4=Phrygian, 5=Lydian,
//!               6=Mixolydian, 7=Minor Pentatonic, 8=Major Pentatonic, 9=Blues, 10=Whole Tone
//!   root      — 0–11 (0=C, 1=C#, 2=D, ..., 11=B)
//!   transpose — -24–24 semitones (default 0)

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn qp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

// Scale masks: bit k is 1 if semitone k relative to root is allowed.
static SCALES: [u16; 11] = [
    0b1111_1111_1111, // 0: Chromatic (all 12)
    0b1010_1011_0101, // 1: Major (0, 2, 4, 5, 7, 9, 11)
    0b0101_1010_1101, // 2: Minor (0, 2, 3, 5, 7, 8, 10)
    0b0101_0110_1101, // 3: Dorian (0, 2, 3, 5, 7, 9, 10)
    0b0101_1010_1011, // 4: Phrygian (0, 1, 3, 5, 7, 8, 10)
    0b1010_1101_0101, // 5: Lydian (0, 2, 4, 6, 7, 9, 11)
    0b0101_0101_0101, // 6: Mixolydian (0, 2, 4, 5, 7, 9, 10)
    0b0100_1010_0101, // 7: Minor Pentatonic (0, 3, 5, 7, 10)
    0b0010_1010_0101, // 8: Major Pentatonic (0, 2, 4, 7, 9)
    0b0100_1110_0101, // 9: Blues (0, 3, 5, 6, 7, 10)
    0b0101_0101_0101, // 10: Whole Tone (0, 2, 4, 6, 8, 10)
];

pub struct QuantizerNode {
    ports:         [PortDescriptor; 5],
    bank:          ParameterBank,
    last_semitone: i32,
    prev_trigger:  f32,
    pulse_remain:  u32,
    node_id:       u32,
}

impl QuantizerNode {
    pub const PORT_CV_IN:       u32 = 0;
    pub const PORT_TRIGGER_IN:  u32 = 1;
    pub const PORT_CV_OUT:      u32 = 2;
    pub const PORT_TRIGGER_OUT: u32 = 3;
    pub const PORT_PITCH_OUT:   u32 = 4;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_CV_IN,       name: "cv_in".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_TRIGGER_IN,  name: "trigger_in".into(),  direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_CV_OUT,      name: "cv_out".into(),      direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: Self::PORT_TRIGGER_OUT, name: "trigger_out".into(), direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_PITCH_OUT,   name: "pitch_out".into(),   direction: PortDirection::Output, port_type: PortType::Pitch },
            ],
            bank:          ParameterBank::from_capability_document(&doc),
            last_semitone: i32::MIN,
            prev_trigger:  0.0,
            pulse_remain:  0,
            node_id:       0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "QuantizerNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "cv_in".into(),       direction: PortDirection::Input,  port_type: PortType::Cv },
                PortDescriptor { id: 1, name: "trigger_in".into(),  direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: 2, name: "cv_out".into(),      direction: PortDirection::Output, port_type: PortType::Cv },
                PortDescriptor { id: 3, name: "trigger_out".into(), direction: PortDirection::Output, port_type: PortType::Logic },
                PortDescriptor { id: 4, name: "pitch_out".into(),   direction: PortDirection::Output, port_type: PortType::Pitch },
            ],
            params: vec![
                ParamDescriptor { id: qp("scale"),     name: "scale".into(),     min: 0.0,   max: 10.0, default: 0.0, stepped: true, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: qp("root"),      name: "root".into(),      min: 0.0,   max: 11.0, default: 0.0, stepped: true, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: qp("transpose"), name: "transpose".into(), min: -24.0, max: 24.0, default: 0.0, stepped: true, in_kit: true, unit: ParamUnit::Semitones, display: None },
            ],
            extensions: vec!["paraclete.modular.quantizer".into()],
            view: None,
        }
    }

    /// Find the closest allowed semitone degree in the active scale mask.
    fn quantize_degree(degree: i32, scale_mask: u16) -> i32 {
        let deg = degree.rem_euclid(12);
        if (scale_mask & (1 << deg)) != 0 {
            return deg;
        }
        for dist in 1..=6 {
            let up = (deg + dist) % 12;
            if (scale_mask & (1 << up)) != 0 {
                return up;
            }
            let dn = (deg - dist + 12) % 12;
            if (scale_mask & (1 << dn)) != 0 {
                return dn;
            }
        }
        deg
    }
}

impl Default for QuantizerNode {
    fn default() -> Self { Self::new() }
}

impl Node for QuantizerNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        self.last_semitone = i32::MIN;
        self.prev_trigger = 0.0;
        self.pulse_remain = 0;
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

        let scale_idx = (self.bank.get(qp("scale")).round() as usize).min(SCALES.len() - 1);
        let scale_mask = SCALES[scale_idx];
        let root = (self.bank.get(qp("root")).round() as i32).clamp(0, 11);
        let transpose = self.bank.get(qp("transpose")).round() as i32;

        let frames = input.block_size;
        let has_clock = input.signal_inputs.iter().any(|s| s.port_id == Self::PORT_TRIGGER_IN);
        let cv_in = input.cv_signal(Self::PORT_CV_IN);
        let trg_in = input.logic(Self::PORT_TRIGGER_IN);

        let has_cv_out = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_CV_OUT);
        let has_trg_out = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_TRIGGER_OUT);
        let has_pitch_out = output.signal_outputs.iter().any(|s| s.port_id == Self::PORT_PITCH_OUT);

        if has_cv_out || has_trg_out || has_pitch_out {
            for f in 0..frames {
                let trg = trg_in.get(f).copied().unwrap_or(0.0);
                let should_quantize = if has_clock {
                    trg >= 0.5 && self.prev_trigger < 0.5
                } else {
                    true
                };
                self.prev_trigger = trg;

                if should_quantize {
                    let raw_cv = cv_in.get(f).copied().unwrap_or(0.0);
                    let raw_semitones = (raw_cv * 12.0).round() as i32;
                    let octave = if raw_semitones >= 0 {
                        raw_semitones / 12
                    } else {
                        (raw_semitones - 11) / 12
                    };
                    let chrom_deg = ((raw_semitones % 12) + 12) % 12;
                    let rel_deg = (chrom_deg - root + 12) % 12;
                    let quant_deg = Self::quantize_degree(rel_deg, scale_mask);
                    let quantized_semi = octave * 12 + quant_deg + root + transpose;

                    if self.last_semitone != i32::MIN && quantized_semi != self.last_semitone {
                        // 44 samples pulse (~1ms at 44.1kHz)
                        self.pulse_remain = 44;
                    }
                    self.last_semitone = quantized_semi;
                }

                let active_semi = if self.last_semitone == i32::MIN { 0 } else { self.last_semitone };
                let out_cv = active_semi as f32 / 12.0;

                let trg_pulse = if self.pulse_remain > 0 {
                    self.pulse_remain -= 1;
                    1.0f32
                } else {
                    0.0f32
                };

                if let Some(cv_out) = output.try_cv_signal_output_mut(Self::PORT_CV_OUT) {
                    if f < cv_out.len() {
                        cv_out[f] = out_cv;
                    }
                }
                if let Some(trg_out) = output.try_logic_output_mut(Self::PORT_TRIGGER_OUT) {
                    if f < trg_out.len() {
                        trg_out[f] = trg_pulse;
                    }
                }
                if let Some(pitch_out) = output.try_pitch_output_mut(Self::PORT_PITCH_OUT) {
                    if f < pitch_out.len() {
                        pitch_out[f] = (active_semi + 60).clamp(0, 127) as f32;
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
    fn quantizer_major_scale_quantization() {
        let mut q = QuantizerNode::new();
        q.activate(44100.0, 4);
        // Scale 1 = Major (C, D, E, F, G, A, B).
        q.bank.set(qp("scale"), 1.0);

        // Input voltages:
        // 0.0 -> C (0) -> 0.0
        // 1/12 (0.0833, C#) -> rounds/quantizes to D (2/12 = 0.1667)
        // 2/12 (0.1667, D) -> D (2/12 = 0.1667)
        // 3/12 (0.25, D#) -> E (4/12 = 0.3333)
        let inputs = [0.0f32, 1.0 / 12.0, 2.0 / 12.0, 3.0 / 12.0];
        let sig_inputs = [
            SignalInputSlot::new(QuantizerNode::PORT_CV_IN, SignalPortKind::Cv, &inputs),
        ];

        let mut cv_out = [0.0f32; 4];
        let mut sig_outputs = [
            SignalOutputSlot::new(QuantizerNode::PORT_CV_OUT, SignalPortKind::Cv, &mut cv_out),
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

        q.process(&input, &mut output);

        assert!((cv_out[0] - 0.0).abs() < 1e-4);
        assert!((cv_out[1] - 2.0 / 12.0).abs() < 1e-4);
        assert!((cv_out[2] - 2.0 / 12.0).abs() < 1e-4);
        assert!((cv_out[3] - 4.0 / 12.0).abs() < 1e-4);
    }
}
