// SPDX-License-Identifier: GPL-3.0-or-later
//! ReverbNode — Freeverb algorithm (Jezar at Dreampoint, public domain / MIT).
//!
//! 8 parallel comb filters + 4 series allpass filters per channel.
//! All delay line lengths are computed at activate() from the sample rate.
//!
//! Parameters:
//!   room_size    — 0.0–1.0,   default 0.5
//!   damping      — 0.0–1.0,   default 0.5
//!   wet          — 0.0–1.0,   default 0.3
//!   dry          — 0.0–1.0,   default 0.7
//!   width        — 0.0–1.0,   default 1.0
//!   pre_delay_ms — 0.0–100.0, default 0.0

use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
};

const PARAM_ROOM_SIZE: u32 = 0;
const PARAM_DAMPING:   u32 = 1;
const PARAM_WET:       u32 = 2;
const PARAM_DRY:       u32 = 3;
const PARAM_WIDTH:     u32 = 4;
const PARAM_PRE_DELAY: u32 = 5;

// Freeverb comb delay lengths at 44100 Hz. Scaled proportionally at other sample rates.
const COMB_LENS:    [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_LENS: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

// ── Inner DSP structs ─────────────────────────────────────────────────────────

struct CombFilter {
    buf:         Vec<f32>,
    pos:         usize,
    filterstore: f32,
}

impl CombFilter {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0, filterstore: 0.0 }
    }

    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let output = self.buf[self.pos];
        self.filterstore = output * (1.0 - damp) + self.filterstore * damp;
        self.buf[self.pos] = input + self.filterstore * feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        output
    }
}

struct AllpassFilter {
    buf: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0 }
    }

    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.pos];
        let output = -input + bufout;
        self.buf[self.pos] = input + bufout * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        output
    }
}

fn make_comb_array(lens: &[usize; 8]) -> [CombFilter; 8] {
    std::array::from_fn(|i| CombFilter::new(lens[i]))
}

fn make_allpass_array(lens: &[usize; 4]) -> [AllpassFilter; 4] {
    std::array::from_fn(|i| AllpassFilter::new(lens[i]))
}

// ── ReverbNode ────────────────────────────────────────────────────────────────

pub struct ReverbNode {
    ports:         [PortDescriptor; 2],
    bank:          ParameterBank,
    comb_l:        [CombFilter; 8],
    comb_r:        [CombFilter; 8],
    allpass_l:     [AllpassFilter; 4],
    allpass_r:     [AllpassFilter; 4],
    pre_delay_l:   Vec<f32>,
    pre_delay_r:   Vec<f32>,
    pre_delay_pos: usize,
    sample_rate:   f32,
}

impl ReverbNode {
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
            bank:          ParameterBank::empty(),
            comb_l:        make_comb_array(&COMB_LENS),
            comb_r:        make_comb_array(&COMB_LENS),
            allpass_l:     make_allpass_array(&ALLPASS_LENS),
            allpass_r:     make_allpass_array(&ALLPASS_LENS),
            pre_delay_l:   vec![0.0; 1],
            pre_delay_r:   vec![0.0; 1],
            pre_delay_pos: 0,
            sample_rate:   44100.0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "ReverbNode",
            vendor: "Paraclete",
            version: (0, 5, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: PARAM_ROOM_SIZE, name: "room_size".into(),    min: 0.0, max: 1.0,   default: 0.5, stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_DAMPING,   name: "damping".into(),      min: 0.0, max: 1.0,   default: 0.5, stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_WET,       name: "wet".into(),          min: 0.0, max: 1.0,   default: 0.3, stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_DRY,       name: "dry".into(),          min: 0.0, max: 1.0,   default: 0.7, stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_WIDTH,     name: "width".into(),        min: 0.0, max: 1.0,   default: 1.0, stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_PRE_DELAY, name: "pre_delay_ms".into(), min: 0.0, max: 100.0, default: 0.0, stepped: false, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.effect"],
        }
    }
}

impl Default for ReverbNode {
    fn default() -> Self { Self::new() }
}

impl Node for ReverbNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn activate(&mut self, sr: f32, _block: usize) {
        self.sample_rate = sr;
        let scale = sr / 44100.0;

        let comb_lens_l: [usize; 8] = std::array::from_fn(|i| {
            ((COMB_LENS[i] as f32 * scale).round() as usize).max(1)
        });
        let comb_lens_r: [usize; 8] = std::array::from_fn(|i| {
            (((COMB_LENS[i] + STEREO_SPREAD) as f32 * scale).round() as usize).max(1)
        });
        let allpass_lens_l: [usize; 4] = std::array::from_fn(|i| {
            ((ALLPASS_LENS[i] as f32 * scale).round() as usize).max(1)
        });
        let allpass_lens_r: [usize; 4] = std::array::from_fn(|i| {
            (((ALLPASS_LENS[i] + STEREO_SPREAD) as f32 * scale).round() as usize).max(1)
        });

        self.comb_l    = make_comb_array(&comb_lens_l);
        self.comb_r    = make_comb_array(&comb_lens_r);
        self.allpass_l = make_allpass_array(&allpass_lens_l);
        self.allpass_r = make_allpass_array(&allpass_lens_r);

        let max_pre_delay = ((sr * 0.1) as usize).max(1);
        self.pre_delay_l   = vec![0.0; max_pre_delay];
        self.pre_delay_r   = vec![0.0; max_pre_delay];
        self.pre_delay_pos = 0;

        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let room_size    = self.bank.get(PARAM_ROOM_SIZE) as f32;
        let damping      = self.bank.get(PARAM_DAMPING)   as f32;
        let wet          = self.bank.get(PARAM_WET)        as f32;
        let dry          = self.bank.get(PARAM_DRY)        as f32;
        let width        = self.bank.get(PARAM_WIDTH)      as f32;
        let pre_delay_ms = self.bank.get(PARAM_PRE_DELAY) as f32;

        let feedback = room_size * 0.28 + 0.7;
        let damp     = damping * 0.4;
        let wet1     = wet * (width / 2.0 + 0.5);
        let wet2     = wet * ((1.0 - width) / 2.0);

        let pre_delay_samples = ((pre_delay_ms / 1000.0) * self.sample_rate) as usize;
        let pd_len = self.pre_delay_l.len();
        let pre_delay_samples = pre_delay_samples.min(pd_len - 1);

        if let (Some(audio_in), Some(audio_out)) = (
            input.audio_inputs.first(),
            output.audio_outputs.first_mut(),
        ) {
            let frames = input.block_size;

            for i in 0..frames {
                let raw_l = if audio_in.channels() >= 1 { audio_in.channel(0)[i] } else { 0.0 };
                let raw_r = if audio_in.channels() >= 2 { audio_in.channel(1)[i] } else { raw_l };

                // Write current sample into pre-delay ring buffer
                self.pre_delay_l[self.pre_delay_pos] = raw_l;
                self.pre_delay_r[self.pre_delay_pos] = raw_r;

                // Read delayed sample
                let read_pos = (self.pre_delay_pos + pd_len - pre_delay_samples) % pd_len;
                let pd_l = self.pre_delay_l[read_pos];
                let pd_r = self.pre_delay_r[read_pos];

                self.pre_delay_pos = (self.pre_delay_pos + 1) % pd_len;

                // Mix into mono for reverb input (standard Freeverb approach)
                let input_mixed = (pd_l + pd_r) * 0.015;

                // 8 parallel comb filters
                let mut out_l_s = 0.0f32;
                let mut out_r_s = 0.0f32;
                for c in 0..8 {
                    out_l_s += self.comb_l[c].process(input_mixed, feedback, damp);
                    out_r_s += self.comb_r[c].process(input_mixed, feedback, damp);
                }

                // 4 series allpass filters
                for a in 0..4 {
                    out_l_s = self.allpass_l[a].process(out_l_s);
                    out_r_s = self.allpass_r[a].process(out_r_s);
                }

                // Mix wet/dry with stereo width
                if audio_out.channels() >= 1 { audio_out.channel_mut(0)[i] = out_l_s * wet1 + out_r_s * wet2 + raw_l * dry; }
                if audio_out.channels() >= 2 { audio_out.channel_mut(1)[i] = out_r_s * wet1 + out_l_s * wet2 + raw_r * dry; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    fn run_reverb(reverb: &mut ReverbNode, input_l: &[f32], input_r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let frames = input_l.len().min(input_r.len());
        let mut src = AudioBuffer::new(2, frames);
        let mut dst = AudioBuffer::new(2, frames);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        src.channel_mut(0)[..frames].copy_from_slice(&input_l[..frames]);
        src.channel_mut(1)[..frames].copy_from_slice(&input_r[..frames]);

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
        reverb.process(&input, &mut ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        });
        (dst.channel(0).to_vec(), dst.channel(1).to_vec())
    }

    #[test]
    fn reverb_silence_in_produces_silence_out() {
        let mut r = ReverbNode::new();
        r.activate(44100.0, 64);
        // With dry=0 and wet>0, silence in should produce silence out (reverb fills from 0)
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        r.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_DRY as i64,  arg1: 0.0 },
        ]);
        let input_l = vec![0.0f32; 512];
        let input_r = vec![0.0f32; 512];
        let (out_l, _) = run_reverb(&mut r, &input_l, &input_r);
        assert!(out_l.iter().all(|&s| s == 0.0), "silence in must produce silence out");
    }

    #[test]
    fn reverb_nonsilent_input_with_wet_produces_nonsilent_output() {
        let mut r = ReverbNode::new();
        r.activate(44100.0, 512);
        // Run enough frames for reverb to build up
        let input_l: Vec<f32> = (0..512).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let input_r = input_l.clone();
        // Run a few blocks to let the reverb tail build
        for _ in 0..4 {
            run_reverb(&mut r, &input_l, &input_r);
        }
        let (out_l, _) = run_reverb(&mut r, &input_l, &input_r);
        // With wet=0.3 (default), reverb tail should be non-zero
        let max = out_l.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max > 0.0, "reverb with non-silent input must produce non-silent output");
    }

    #[test]
    fn reverb_zero_wet_passes_dry_only() {
        let mut r = ReverbNode::new();
        r.activate(44100.0, 64);
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        r.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_WET as i64, arg1: 0.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_DRY as i64, arg1: 1.0 },
        ]);
        let input_l = vec![0.5f32; 64];
        let input_r = vec![0.5f32; 64];
        let (out_l, _) = run_reverb(&mut r, &input_l, &input_r);
        // wet=0, dry=1: output must equal input
        assert!((out_l[63] - 0.5).abs() < 1e-5, "wet=0, dry=1 must pass input through unchanged");
    }

    #[test]
    fn reverb_activate_at_48k_rescales_delay_lengths() {
        let mut r = ReverbNode::new();
        r.activate(48000.0, 64);
        // At 48kHz, first comb filter should be longer than at 44100Hz
        let expected_min_len = ((COMB_LENS[0] as f32 * 48000.0 / 44100.0).round() as usize) - 1;
        assert!(r.comb_l[0].buf.len() >= expected_min_len,
            "comb delay length must scale with sample rate");
    }

    #[test]
    fn reverb_ports_declared_correctly() {
        let r = ReverbNode::new();
        assert_eq!(r.ports().len(), 2);
        assert_eq!(r.ports()[0].direction, PortDirection::Input);
        assert_eq!(r.ports()[1].direction, PortDirection::Output);
    }
}
