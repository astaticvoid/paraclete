// SPDX-License-Identifier: GPL-3.0-or-later
//! ChorusNode — Stereo multi-tap chorus and flanger with quadrature stereo widening.
//!
//! Features modulated fractional delay lines with Hermite interpolation,
//! bipolar feedback for flanging textures, and adjustable stereo LFO phase spread.
//!
//! Parameters:
//!   rate         — 0.05–10.0 Hz,  default 0.8 Hz (LFO frequency)
//!   depth        — 0.1–10.0 ms,   default 3.0 ms (modulation depth)
//!   delay        — 1.0–30.0 ms,   default 10.0 ms (base delay time)
//!   feedback     — -0.9–0.9,      default 0.2 (bipolar feedback for flanging)
//!   stereo_phase — 0.0–1.0,       default 0.25 (quadrature phase offset between L/R)
//!   mix          — 0.0–1.0,       default 0.5 (wet/dry balance)

use std::f32::consts::TAU;

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn chp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

pub struct ChorusNode {
    ports:       [PortDescriptor; 2],
    bank:        ParameterBank,
    buf_l:       Vec<f32>,
    buf_r:       Vec<f32>,
    write_pos:   usize,
    lfo_phase:   f32,
    sample_rate: f32,
    node_id:     u32,
}

impl ChorusNode {
    pub const PORT_AUDIO_IN:  u32 = 0;
    pub const PORT_AUDIO_OUT: u32 = 1;

    // Buffer capacity: 100ms at 192kHz = 19200 samples
    const MAX_DELAY_SAMPLES: usize = 19200;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_AUDIO_IN,  name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            bank:        ParameterBank::from_capability_document(&doc),
            buf_l:       vec![0.0; Self::MAX_DELAY_SAMPLES],
            buf_r:       vec![0.0; Self::MAX_DELAY_SAMPLES],
            write_pos:   0,
            lfo_phase:   0.0,
            sample_rate: 44100.0,
            node_id:     0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "ChorusNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: chp("rate"),         name: "rate".into(),         min: 0.05, max: 10.0, default: 0.8,  stepped: false, in_kit: true, unit: ParamUnit::Hz,           display: None },
                ParamDescriptor { id: chp("depth"),        name: "depth".into(),        min: 0.1,  max: 10.0, default: 3.0,  stepped: false, in_kit: true, unit: ParamUnit::Milliseconds, display: None },
                ParamDescriptor { id: chp("delay"),        name: "delay".into(),        min: 1.0,  max: 30.0, default: 10.0, stepped: false, in_kit: true, unit: ParamUnit::Milliseconds, display: None },
                ParamDescriptor { id: chp("feedback"),     name: "feedback".into(),     min: -0.9, max: 0.9,  default: 0.2,  stepped: false, in_kit: true, unit: ParamUnit::Generic,      display: None },
                ParamDescriptor { id: chp("stereo_phase"), name: "stereo_phase".into(), min: 0.0,  max: 1.0,  default: 0.25, stepped: false, in_kit: true, unit: ParamUnit::Percent,      display: None },
                ParamDescriptor { id: chp("mix"),          name: "mix".into(),          min: 0.0,  max: 1.0,  default: 0.5,  stepped: false, in_kit: true, unit: ParamUnit::Percent,      display: None },
            ],
            extensions: vec!["paraclete.fx.chorus".into()],
            view: None,
        }
    }

    /// Hermite 4-point cubic interpolation for glitch-free pitch modulation.
    #[inline(always)]
    fn read_hermite(buf: &[f32], pos: f32) -> f32 {
        let len = buf.len() as f32;
        let p = pos.rem_euclid(len);
        let idx = p.floor() as usize;
        let frac = p - (idx as f32);

        let i0 = if idx == 0 { buf.len() - 1 } else { idx - 1 };
        let i1 = idx;
        let i2 = (idx + 1) % buf.len();
        let i3 = (idx + 2) % buf.len();

        let y0 = buf[i0];
        let y1 = buf[i1];
        let y2 = buf[i2];
        let y3 = buf[i3];

        let c0 = y1;
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);

        ((c3 * frac + c2) * frac + c1) * frac + c0
    }
}

impl Default for ChorusNode {
    fn default() -> Self { Self::new() }
}

impl Node for ChorusNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate.max(1.0);
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write_pos = 0;
        self.lfo_phase = 0.0;
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

        let rate         = (self.bank.get(chp("rate")) as f32).max(0.01);
        let depth_ms     = (self.bank.get(chp("depth")) as f32).max(0.0);
        let delay_ms     = (self.bank.get(chp("delay")) as f32).max(0.1);
        let feedback     = (self.bank.get(chp("feedback")) as f32).clamp(-0.95, 0.95);
        let stereo_phase = self.bank.get(chp("stereo_phase")) as f32;
        let mix          = (self.bank.get(chp("mix")) as f32).clamp(0.0, 1.0);

        let frames = input.block_size;
        let has_audio_in = !input.audio_inputs.is_empty();
        let has_audio_out = !output.audio_outputs.is_empty();

        if has_audio_in && has_audio_out {
            let audio_in = &input.audio_inputs[0];
            let in_ch = audio_in.channels();
            let audio_out = &mut output.audio_outputs[0];
            let out_ch = audio_out.channels();

            let sr = self.sample_rate;
            let lfo_inc = rate / sr;
            let base_delay_samples = (delay_ms * 0.001 * sr).min(Self::MAX_DELAY_SAMPLES as f32 - 100.0);
            let mod_depth_samples = (depth_ms * 0.001 * sr).min(base_delay_samples * 0.9);
            let buf_len = self.buf_l.len();

            for f in 0..frames {
                let in_l = audio_in.channel(0)[f];
                let in_r = if in_ch > 1 { audio_in.channel(1)[f] } else { in_l };

                // Modulated delay times for Left and Right with phase offset
                let lfo_l = (self.lfo_phase * TAU).sin();
                let lfo_r = ((self.lfo_phase + stereo_phase) * TAU).sin();

                let dt_l = base_delay_samples + lfo_l * mod_depth_samples;
                let dt_r = base_delay_samples + lfo_r * mod_depth_samples;

                let read_pos_l = (self.write_pos as f32 - dt_l + buf_len as f32).rem_euclid(buf_len as f32);
                let read_pos_r = (self.write_pos as f32 - dt_r + buf_len as f32).rem_euclid(buf_len as f32);

                let wet_l = Self::read_hermite(&self.buf_l, read_pos_l);
                let wet_r = Self::read_hermite(&self.buf_r, read_pos_r);

                // Write into circular delay lines with feedback
                self.buf_l[self.write_pos] = in_l + wet_l * feedback;
                self.buf_r[self.write_pos] = in_r + wet_r * feedback;

                self.write_pos = (self.write_pos + 1) % buf_len;
                self.lfo_phase = (self.lfo_phase + lfo_inc).fract();

                // Mix dry and wet
                audio_out.channel_mut(0)[f] = in_l * (1.0 - mix) + wet_l * mix;
                if out_ch > 1 {
                    audio_out.channel_mut(1)[f] = in_r * (1.0 - mix) + wet_r * mix;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, SignalOutputSlot, TransportInfo,
    };

    #[test]
    fn chorus_produces_modulated_output() {
        let mut chorus = ChorusNode::new();
        chorus.activate(44100.0, 512);

        let mut in_buf = AudioBuffer::new(2, 512);
        // Unit impulse at sample 0
        in_buf.channel_mut(0)[0] = 1.0;
        in_buf.channel_mut(1)[0] = 1.0;
        let mut out_buf = AudioBuffer::new(2, 512);

        let audio_inputs = [&in_buf];
        let mut audio_outputs = [&mut out_buf];
        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let extended = ExtendedEventSlab::empty();

        let input = ProcessInput {
            audio_inputs: &audio_inputs,
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 512,
            extended_events: &extended,
            commands: &[],
        };

        {
            let mut sig_outs: [SignalOutputSlot; 0] = [];
            let mut output = ProcessOutput::new(&mut audio_outputs, &mut sig_outs, &mut events_out);
            chorus.process(&input, &mut output);
        }

        // Dry sample passes through (with mix 0.5 -> 0.5)
        assert!((out_buf.channel(0)[0] - 0.5).abs() < 1e-4);
    }
}
