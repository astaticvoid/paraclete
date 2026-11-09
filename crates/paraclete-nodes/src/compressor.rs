// SPDX-License-Identifier: GPL-3.0-or-later
//! CompressorNode — Stereo master bus and track dynamic range compressor with sidechain ducking.
//!
//! Features soft-knee compression, ballistic peak/RMS envelope detector,
//! makeup gain, parallel dry/wet mixing, external sidechain input, and CV gain-reduction output.
//!
//! Parameters:
//!   threshold — -60.0–0.0 dB, default -12.0 dB
//!   ratio     — 1.0–20.0,     default 4.0
//!   attack    — 0.1–200.0 ms,  default 10.0 ms
//!   release   — 10.0–2000.0 ms, default 100.0 ms
//!   makeup    — 0.0–24.0 dB,   default 0.0 dB
//!   knee      — 0.0–12.0 dB,   default 3.0 dB
//!   mix       — 0.0–1.0,       default 1.0 (parallel compression)
//!   sidechain — 0.0–1.0,       default 0.0 (0.0 = internal, 1.0 = external sidechain port)

use std::collections::HashMap;
use std::sync::OnceLock;

use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn cp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

pub struct CompressorNode {
    ports:                  [PortDescriptor; 4],
    bank:                   ParameterBank,
    env_level:              f32,
    current_gr_db:          f32,
    sample_rate:            f32,
    node_id:                u32,
    gr_path_cache:          OnceLock<String>,
    pending_initial_params: HashMap<String, f64>,
}

impl CompressorNode {
    pub const PORT_AUDIO_IN:      u32 = 0;
    pub const PORT_AUDIO_OUT:     u32 = 1;
    pub const PORT_SIDECHAIN_IN:  u32 = 2;
    pub const PORT_GAIN_RED_OUT:  u32 = 3;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_AUDIO_IN,     name: "audio_in".into(),     direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT,    name: "audio_out".into(),    direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_SIDECHAIN_IN, name: "sidechain_in".into(), direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_GAIN_RED_OUT, name: "gain_red_out".into(), direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            bank:                   ParameterBank::from_capability_document(&doc),
            env_level:              0.0,
            current_gr_db:          0.0,
            sample_rate:            44100.0,
            node_id:                0,
            gr_path_cache:          OnceLock::new(),
            pending_initial_params: HashMap::new(),
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "CompressorNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),     direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(),    direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: 2, name: "sidechain_in".into(), direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 3, name: "gain_red_out".into(), direction: PortDirection::Output, port_type: PortType::Cv },
            ],
            params: vec![
                ParamDescriptor { id: cp("threshold"), name: "threshold".into(), min: -60.0, max: 0.0,    default: -12.0, stepped: false, in_kit: true, unit: ParamUnit::Decibels,     display: None },
                ParamDescriptor { id: cp("ratio"),     name: "ratio".into(),     min: 1.0,   max: 20.0,   default: 4.0,   stepped: false, in_kit: true, unit: ParamUnit::Generic,      display: None },
                ParamDescriptor { id: cp("attack"),    name: "attack".into(),    min: 0.1,   max: 200.0,  default: 10.0,  stepped: false, in_kit: true, unit: ParamUnit::Milliseconds, display: None },
                ParamDescriptor { id: cp("release"),   name: "release".into(),   min: 10.0,  max: 2000.0, default: 100.0, stepped: false, in_kit: true, unit: ParamUnit::Milliseconds, display: None },
                ParamDescriptor { id: cp("makeup"),    name: "makeup".into(),    min: 0.0,   max: 24.0,   default: 0.0,   stepped: false, in_kit: true, unit: ParamUnit::Decibels,     display: None },
                ParamDescriptor { id: cp("knee"),      name: "knee".into(),      min: 0.0,   max: 12.0,   default: 3.0,   stepped: false, in_kit: true, unit: ParamUnit::Decibels,     display: None },
                ParamDescriptor { id: cp("mix"),       name: "mix".into(),       min: 0.0,   max: 1.0,    default: 1.0,   stepped: false, in_kit: true, unit: ParamUnit::Percent,      display: None },
                ParamDescriptor { id: cp("sidechain"), name: "sidechain".into(), min: 0.0,   max: 1.0,    default: 0.0,   stepped: false, in_kit: true, unit: ParamUnit::Percent,      display: None },
            ],
            extensions: vec!["paraclete.fx.compressor".into()],
            view: None,
        }
    }
}

impl Default for CompressorNode {
    fn default() -> Self { Self::new() }
}

impl Node for CompressorNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate.max(1.0);
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            let id = ParamDescriptor::id_for_name(&name);
            self.bank.set(id, value);
        }
        self.env_level = 0.0;
        self.current_gr_db = 0.0;
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
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
        let path = self.gr_path_cache.get_or_init(|| {
            format!("/node/{}/state/gain_reduction_db", self.node_id)
        });
        buf.push((
            path.clone(),
            StateBusValue::Float(self.current_gr_db as f64),
        ));
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let threshold = self.bank.get(cp("threshold")) as f32;
        let ratio     = (self.bank.get(cp("ratio")) as f32).max(1.0);
        let attack_ms = (self.bank.get(cp("attack")) as f32).max(0.1);
        let rel_ms    = (self.bank.get(cp("release")) as f32).max(1.0);
        let makeup_db = self.bank.get(cp("makeup")) as f32;
        let knee_db   = (self.bank.get(cp("knee")) as f32).max(0.0);
        let mix       = (self.bank.get(cp("mix")) as f32).clamp(0.0, 1.0);
        let sc_mix    = (self.bank.get(cp("sidechain")) as f32).clamp(0.0, 1.0);

        let sr = self.sample_rate;
        // One-pole attack and release filter coefficients
        let att_coeff = (-1000.0 / (attack_ms * sr)).exp();
        let rel_coeff = (-1000.0 / (rel_ms * sr)).exp();
        let makeup_gain = 10.0f32.powf(makeup_db / 20.0);

        let frames = input.block_size;
        let has_audio_in = !input.audio_inputs.is_empty();
        let has_audio_out = !output.audio_outputs.is_empty();
        let has_sidechain = input.audio_inputs.len() > 1;

        if has_audio_in && has_audio_out {
            let audio_in = &input.audio_inputs[0];
            let channels = audio_in.channels();

            let sidechain_buf = if has_sidechain && sc_mix > 0.0 {
                Some(&input.audio_inputs[1])
            } else {
                None
            };

            for f in 0..frames {
                // Peak detection across input channels
                let mut peak = 0.0f32;
                for ch in 0..channels {
                    peak = peak.max(audio_in.channel(ch)[f].abs());
                }

                // If external sidechain is blended, use sidechain peak
                if let Some(sc) = sidechain_buf {
                    let mut sc_peak = 0.0f32;
                    for ch in 0..sc.channels() {
                        if f < sc.channel(ch).len() {
                            sc_peak = sc_peak.max(sc.channel(ch)[f].abs());
                        }
                    }
                    peak = peak * (1.0 - sc_mix) + sc_peak * sc_mix;
                }

                // Ballistic envelope follower
                let coeff = if peak > self.env_level { att_coeff } else { rel_coeff };
                self.env_level = (1.0 - coeff) * peak + coeff * self.env_level;

                // Convert envelope to dB (safe log floor at -96 dB)
                let env_db = 20.0 * (self.env_level.max(0.0000158)).log10();

                // Compute gain reduction in dB with soft-knee
                let gr_db = if knee_db > 0.0 && env_db > (threshold - knee_db / 2.0) && env_db < (threshold + knee_db / 2.0) {
                    let d = env_db - threshold + knee_db / 2.0;
                    ((1.0 / ratio - 1.0) * d * d) / (2.0 * knee_db)
                } else if env_db > threshold {
                    (threshold + (env_db - threshold) / ratio) - env_db
                } else {
                    0.0f32
                };

                self.current_gr_db = gr_db;
                let gr_linear = 10.0f32.powf(gr_db / 20.0);
                let compressed_gain = gr_linear * makeup_gain;

                let audio_out = &mut output.audio_outputs[0];
                let out_ch = channels.min(audio_out.channels());
                for ch in 0..out_ch {
                    let dry = audio_in.channel(ch)[f];
                    let wet = dry * compressed_gain;
                    audio_out.channel_mut(ch)[f] = dry * (1.0 - mix) + wet * mix;
                }

                if let Some(gr_out) = output.try_cv_signal_output_mut(Self::PORT_GAIN_RED_OUT) {
                    if f < gr_out.len() {
                        gr_out[f] = gr_linear;
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
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, SignalOutputSlot,
        TransportInfo,
    };

    #[test]
    fn compressor_attenuates_signal_above_threshold() {
        let mut comp = CompressorNode::new();
        comp.activate(44100.0, 1000);
        // threshold = -10 dB, ratio = 10:1, attack = 0.1 ms (instant), release = 100 ms
        comp.bank.set(cp("threshold"), -10.0);
        comp.bank.set(cp("ratio"), 10.0);
        comp.bank.set(cp("attack"), 0.1);
        comp.bank.set(cp("knee"), 0.0);
        comp.bank.set(cp("makeup"), 0.0);

        // Input 0 dBFS (amplitude 1.0)
        let mut in_buf = AudioBuffer::new(1, 1000);
        in_buf.channel_mut(0).fill(1.0);
        let mut out_buf = AudioBuffer::new(1, 1000);

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
            block_size: 1000,
            extended_events: &extended,
            commands: &[],
        };

        {
            let mut sig_outs: [SignalOutputSlot; 0] = [];
            let mut output = ProcessOutput::new(&mut audio_outputs, &mut sig_outs, &mut events_out);
            comp.process(&input, &mut output);
        }

        // Output towards end of buffer should be significantly attenuated below 1.0 (around -9 dB ~ 0.35)
        let final_sample = out_buf.channel(0)[999];
        assert!(final_sample < 0.6, "expected compressed output, got {final_sample}");
        assert!(comp.current_gr_db < -5.0, "expected GR below -5dB, got {}", comp.current_gr_db);
    }
}
