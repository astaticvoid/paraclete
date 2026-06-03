// SPDX-License-Identifier: GPL-3.0-or-later
//! FilterNode — Chamberlin state-variable filter (SVF).
//!
//! Parameters:
//!   cutoff_hz   (id=0) — 20–20000 Hz, default 1000
//!   resonance   (id=1) — 0.1–4.0, default 0.7
//!   filter_type (id=2) — 0=LP, 1=HP, 2=BP, 3=Notch, default 0

use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
};

const PARAM_CUTOFF:      u32 = 0;
const PARAM_RESONANCE:   u32 = 1;
const PARAM_FILTER_TYPE: u32 = 2;

pub struct FilterNode {
    ports: [PortDescriptor; 2],
    node_id: u32,
    bank: ParameterBank,
    // SVF state (stereo)
    low_l:   f32,
    band_l:  f32,
    low_r:   f32,
    band_r:  f32,
    f_coeff: f32,
    q_coeff: f32,
    sr:      f32,
}

impl FilterNode {
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
            low_l:   0.0,
            band_l:  0.0,
            low_r:   0.0,
            band_r:  0.0,
            f_coeff: 0.0,
            q_coeff: 0.0,
            sr:      44100.0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "FilterNode",
            vendor: "Paraclete",
            version: (0, 4, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: PARAM_CUTOFF,      name: "cutoff_hz".into(),   min: 20.0,  max: 20000.0, default: 1000.0, stepped: false, unit: ParamUnit::Hz,      display: None },
                ParamDescriptor { id: PARAM_RESONANCE,   name: "resonance".into(),   min: 0.1,   max: 4.0,     default: 0.7,    stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_FILTER_TYPE, name: "filter_type".into(), min: 0.0,   max: 3.0,     default: 0.0,    stepped: true,  unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.effect"],
        }
    }

    fn update_coefficients(&mut self) {
        let cutoff = self.bank.get(PARAM_CUTOFF) as f32;
        let res    = self.bank.get(PARAM_RESONANCE) as f32;
        // Chamberlin SVF — sin() form is stable at high cutoff; linear approx diverges near Nyquist
        self.f_coeff = 2.0 * (std::f32::consts::PI * cutoff / self.sr).sin();
        self.q_coeff = 1.0 / res;
    }

    #[inline(always)]
    fn svf_sample(&self, x: f32, low: &mut f32, band: &mut f32, filter_type: u32) -> f32 {
        let f = self.f_coeff.min(1.0); // stability guard
        let q = self.q_coeff;

        *low  = *low + f * *band;
        let high  = x - *low - q * *band;
        *band = f * high + *band;
        let notch = high + *low;

        match filter_type {
            0 => *low,
            1 => high,
            2 => *band,
            3 => notch,
            _ => *low,
        }
    }
}

impl Default for FilterNode {
    fn default() -> Self { Self::new() }
}

impl Node for FilterNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn activate(&mut self, sr: f32, _block: usize) {
        self.sr   = sr;
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        self.update_coefficients();
        self.low_l  = 0.0;
        self.band_l = 0.0;
        self.low_r  = 0.0;
        self.band_r = 0.0;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let prev_cutoff = self.bank.get(PARAM_CUTOFF);
        let prev_res    = self.bank.get(PARAM_RESONANCE);
        self.bank.handle_commands(input.commands);
        if (self.bank.get(PARAM_CUTOFF) - prev_cutoff).abs() > 0.5
            || (self.bank.get(PARAM_RESONANCE) - prev_res).abs() > 1e-4
        {
            self.update_coefficients();
        }

        if let (Some(audio_in), Some(audio_out)) = (
            input.audio_inputs.first(),
            output.audio_outputs.first_mut(),
        ) {
            let frames = input.block_size;
            let filter_type = self.bank.get(PARAM_FILTER_TYPE) as u32;
            // Left channel
            if audio_in.channels() >= 1 && audio_out.channels() >= 1 {
                let src = audio_in.channel(0);
                let dst = audio_out.channel_mut(0);
                let (mut l, mut b) = (self.low_l, self.band_l);
                for f in 0..frames {
                    dst[f] = self.svf_sample(src[f], &mut l, &mut b, filter_type);
                }
                self.low_l  = l;
                self.band_l = b;
            }
            // Right channel (or duplicate left)
            if audio_out.channels() >= 2 {
                let (src, dst) = if audio_in.channels() >= 2 {
                    (audio_in.channel(1), audio_out.channel_mut(1))
                } else {
                    let src = audio_in.channel(0);
                    let dst = audio_out.channel_mut(1);
                    (src, dst)
                };
                let (mut l, mut b) = (self.low_r, self.band_r);
                for f in 0..frames {
                    dst[f] = self.svf_sample(src[f], &mut l, &mut b, filter_type);
                }
                self.low_r  = l;
                self.band_r = b;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    fn run_filter(filter: &mut FilterNode, input_val: f32, frames: usize) -> Vec<f32> {
        let mut src = AudioBuffer::new(2, frames);
        let mut dst = AudioBuffer::new(2, frames);
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
            block_size: frames,
            extended_events: &slab,
            commands: &[],
        };
        filter.process(&input, &mut ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        });
        dst.channel(0).to_vec()
    }

    #[test]
    fn filter_at_high_cutoff_passes_dc() {
        let mut f = FilterNode::new();
        f.activate(44100.0, 64);
        // High cutoff → DC passes through LP
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        f.bank.handle_commands(&[NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_CUTOFF as i64, arg1: 18000.0 }]);
        f.update_coefficients();
        let out = run_filter(&mut f, 1.0, 64);
        // After settling, output should be close to input (LP at high cutoff)
        assert!(out[63].abs() > 0.1, "expected signal to pass, got {}", out[63]);
    }

    #[test]
    fn filter_state_zeroed_between_activate_calls() {
        let mut f = FilterNode::new();
        f.activate(44100.0, 64);
        run_filter(&mut f, 1.0, 64);
        f.activate(44100.0, 64); // re-activate clears state
        assert_eq!(f.low_l, 0.0);
        assert_eq!(f.band_l, 0.0);
    }
}
