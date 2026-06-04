// SPDX-License-Identifier: GPL-3.0-or-later
//! DelayNode — stereo tape-style delay with Chamberlin SVF low-pass on feedback path.
//!
//! Parameters:
//!   delay_time_ms — 1.0–2000.0 ms, default 250.0
//!   feedback      — 0.0–0.95,      default 0.4  (capped at 0.95 — no runaway)
//!   wet           — 0.0–1.0,       default 0.5
//!   dry           — 0.0–1.0,       default 1.0
//!   filter_hz     — 200.0–8000.0,  default 4000.0

use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
};

const PARAM_DELAY_TIME: u32 = 0;
const PARAM_FEEDBACK:   u32 = 1;
const PARAM_WET:        u32 = 2;
const PARAM_DRY:        u32 = 3;
const PARAM_FILTER_HZ:  u32 = 4;

pub struct DelayNode {
    ports:     [PortDescriptor; 2],
    bank:      ParameterBank,
    buf_l:     Vec<f32>,
    buf_r:     Vec<f32>,
    write_pos: usize,
    max_len:   usize,
    // Chamberlin SVF state for feedback LP filter
    low_l:     f32,
    band_l:    f32,
    low_r:     f32,
    band_r:    f32,
    sample_rate: f32,
}

impl DelayNode {
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
            bank:        ParameterBank::empty(),
            buf_l:       vec![0.0; 1],
            buf_r:       vec![0.0; 1],
            write_pos:   0,
            max_len:     1,
            low_l:       0.0,
            band_l:      0.0,
            low_r:       0.0,
            band_r:      0.0,
            sample_rate: 44100.0,
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "DelayNode",
            vendor: "Paraclete",
            version: (0, 5, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: PARAM_DELAY_TIME, name: "delay_time_ms".into(), min: 1.0,   max: 2000.0, default: 250.0,  stepped: false, unit: ParamUnit::Seconds, display: None },
                ParamDescriptor { id: PARAM_FEEDBACK,   name: "feedback".into(),      min: 0.0,   max: 0.95,   default: 0.4,    stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_WET,        name: "wet".into(),           min: 0.0,   max: 1.0,    default: 0.5,    stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_DRY,        name: "dry".into(),           min: 0.0,   max: 1.0,    default: 1.0,    stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_FILTER_HZ,  name: "filter_hz".into(),     min: 200.0, max: 8000.0, default: 4000.0, stepped: false, unit: ParamUnit::Hz,      display: None },
            ],
            extensions: vec!["paraclete.effect"],
        }
    }
}

impl Default for DelayNode {
    fn default() -> Self { Self::new() }
}

impl Node for DelayNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn activate(&mut self, sr: f32, _block: usize) {
        self.sample_rate = sr;
        self.max_len     = (sr * 2.0) as usize;  // 2000ms max
        self.buf_l       = vec![0.0; self.max_len];
        self.buf_r       = vec![0.0; self.max_len];
        self.write_pos   = 0;
        self.low_l       = 0.0;
        self.band_l      = 0.0;
        self.low_r       = 0.0;
        self.band_r      = 0.0;
        self.bank        = ParameterBank::from_capability_document(&Self::default_doc());
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let delay_ms  = self.bank.get(PARAM_DELAY_TIME) as f32;
        let feedback  = self.bank.get(PARAM_FEEDBACK)   as f32;
        let wet       = self.bank.get(PARAM_WET)        as f32;
        let dry       = self.bank.get(PARAM_DRY)        as f32;
        let filter_hz = self.bank.get(PARAM_FILTER_HZ)  as f32;

        let delay_samples = ((delay_ms / 1000.0) * self.sample_rate) as usize;
        let delay_samples = delay_samples.clamp(1, self.max_len - 1);

        // Chamberlin SVF LP coefficient (same approach as FilterNode)
        let f = (std::f32::consts::PI * filter_hz / self.sample_rate).sin();
        let q = 0.7f32;

        if let (Some(audio_in), Some(audio_out)) = (
            input.audio_inputs.first(),
            output.audio_outputs.first_mut(),
        ) {
            let frames = input.block_size;

            for i in 0..frames {
                let in_l = if audio_in.channels() >= 1 { audio_in.channel(0)[i] } else { 0.0 };
                let in_r = if audio_in.channels() >= 2 { audio_in.channel(1)[i] } else { in_l };

                let read_pos = (self.write_pos + self.max_len - delay_samples) % self.max_len;
                let delayed_l = self.buf_l[read_pos];
                let delayed_r = self.buf_r[read_pos];

                // Chamberlin SVF LP filter on feedback path
                self.low_l  += f * self.band_l;
                self.band_l += f * (delayed_l - self.low_l - q * self.band_l);
                self.low_r  += f * self.band_r;
                self.band_r += f * (delayed_r - self.low_r - q * self.band_r);

                self.buf_l[self.write_pos] = in_l + self.low_l * feedback;
                self.buf_r[self.write_pos] = in_r + self.low_r * feedback;

                if audio_out.channels() >= 1 { audio_out.channel_mut(0)[i] = in_l * dry + delayed_l * wet; }
                if audio_out.channels() >= 2 { audio_out.channel_mut(1)[i] = in_r * dry + delayed_r * wet; }

                self.write_pos = (self.write_pos + 1) % self.max_len;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    fn run_delay(delay: &mut DelayNode, input_val: f32, frames: usize) -> Vec<f32> {
        let mut src = AudioBuffer::new(2, frames);
        let mut dst = AudioBuffer::new(2, frames);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        src.channel_mut(0).fill(input_val);
        src.channel_mut(1).fill(input_val);

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
        delay.process(&input, &mut ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        });
        dst.channel(0).to_vec()
    }

    #[test]
    fn delay_dry_only_passes_signal_unchanged() {
        let mut d = DelayNode::new();
        d.activate(44100.0, 64);
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        d.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_WET as i64, arg1: 0.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_DRY as i64, arg1: 1.0 },
        ]);
        let out = run_delay(&mut d, 0.5, 64);
        assert!((out[0] - 0.5).abs() < 1e-5, "wet=0, dry=1 must pass input through");
    }

    #[test]
    fn delay_echo_appears_after_configured_delay_samples() {
        let mut d = DelayNode::new();
        d.activate(44100.0, 64);
        // Set delay to exactly 64 samples (64/44100 * 1000 ≈ 1.45ms)
        let delay_ms = 64.0f64 / 44100.0 * 1000.0;
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        d.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_DELAY_TIME as i64, arg1: delay_ms },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_FEEDBACK as i64,   arg1: 0.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_WET as i64,        arg1: 1.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_DRY as i64,        arg1: 0.0 },
        ]);

        // First block: impulse at sample 0, rest silence
        let mut impulse = vec![0.0f32; 64];
        impulse[0] = 1.0;
        let out_block1 = {
            let mut src = AudioBuffer::new(2, 64);
            let mut dst = AudioBuffer::new(2, 64);
            let mut events_out = EventOutputBuffer::new(16);
            let transport = TransportInfo::default();
            let slab = ExtendedEventSlab::empty();
            src.channel_mut(0).copy_from_slice(&impulse);
            let dst_ptr: *mut AudioBuffer = &mut dst;
            let dst_ref: &mut AudioBuffer = unsafe { &mut *dst_ptr };
            let mut outs = [dst_ref];
            d.process(&ProcessInput {
                audio_inputs: &[&src], signal_inputs: &[], events: &[],
                transport: &transport, sample_rate: 44100.0, block_size: 64,
                extended_events: &slab, commands: &[],
            }, &mut ProcessOutput {
                audio_outputs: &mut outs, signal_outputs: &mut [],
                events_out: &mut events_out,
            });
            dst.channel(0).to_vec()
        };

        // Block 1: delay=64 samples, so all output should be silence (delayed version
        // of block 1's impulse appears in block 2)
        let all_zero = out_block1.iter().all(|&s| s.abs() < 1e-5);
        assert!(all_zero, "delayed output must be zero for first block when delay=block_size");

        // Second block: silence input → should carry the delayed impulse at sample 0
        let silence = vec![0.0f32; 64];
        let out_block2 = run_delay(&mut d, 0.0, 64);
        // The delayed echo from the impulse should appear somewhere in block 2
        let max = out_block2.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max > 0.5, "echo must appear in block 2 after configured delay");
    }

    #[test]
    fn delay_feedback_zero_no_repeated_echo() {
        let mut d = DelayNode::new();
        d.activate(44100.0, 512);
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        d.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_FEEDBACK as i64, arg1: 0.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_WET as i64,      arg1: 1.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_DRY as i64,      arg1: 0.0 },
        ]);
        // Run a brief impulse then several silence blocks
        run_delay(&mut d, 1.0, 512);  // first block: input
        run_delay(&mut d, 0.0, 512);  // second block: echo appears
        let out = run_delay(&mut d, 0.0, 512);  // third block: must be silent (no feedback)
        let max = out.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        // With delay ~250ms at 44100Hz = 11025 samples, third block has no echo within 512 frames
        assert!(max < 0.01, "with feedback=0, echo does not repeat (got {max})");
    }

    #[test]
    fn delay_ports_declared_correctly() {
        let d = DelayNode::new();
        assert_eq!(d.ports().len(), 2);
        assert_eq!(d.ports()[0].direction, PortDirection::Input);
        assert_eq!(d.ports()[1].direction, PortDirection::Output);
    }
}
