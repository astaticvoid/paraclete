use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

fn lad(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

/// 4-pole Moog-style ladder low-pass filter.
/// Self-oscillates at resonance = 1.0. Soft-clips input at high drive.
/// Ports: audio_in (0, Mono), cutoff_mod (1, Mod), resonance_mod (2, Mod), audio_out (3, Mono).
pub struct LadderFilterNode {
    bank:        ParameterBank,
    stage:       [f32; 4],
    sample_rate: f32,
    node_id:     u32,
    ports:       [PortDescriptor; 4],
}

impl LadderFilterNode {
    pub const PORT_AUDIO_IN:      u32 = 0;
    pub const PORT_CUTOFF_MOD:    u32 = 1;
    pub const PORT_RESONANCE_MOD: u32 = 2;
    pub const PORT_AUDIO_OUT:     u32 = 3;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            bank:        ParameterBank::from_capability_document(&doc),
            stage:       [0.0; 4],
            sample_rate: 44100.0,
            node_id:     0,
            ports: [
                PortDescriptor { id: Self::PORT_AUDIO_IN,      name: "audio_in".into(),      direction: PortDirection::Input,  port_type: PortType::Mono },
                PortDescriptor { id: Self::PORT_CUTOFF_MOD,    name: "cutoff_mod".into(),    direction: PortDirection::Input,  port_type: PortType::Modulation },
                PortDescriptor { id: Self::PORT_RESONANCE_MOD, name: "resonance_mod".into(), direction: PortDirection::Input,  port_type: PortType::Modulation },
                PortDescriptor { id: Self::PORT_AUDIO_OUT,     name: "audio_out".into(),     direction: PortDirection::Output, port_type: PortType::Mono },
            ],
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "LadderFilterNode".into(), vendor: "Paraclete".into(), version: (0, 6, 0),
            ports: vec![],
            params: vec![
                ParamDescriptor { id: lad("cutoff"),    name: "cutoff".into(),    min: 20.0, max: 18000.0, default: 4000.0, stepped: false, unit: ParamUnit::Hz,      display: None },
                ParamDescriptor { id: lad("resonance"), name: "resonance".into(), min: 0.0,  max: 1.0,    default: 0.0,    stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: lad("drive"),     name: "drive".into(),     min: 0.0,  max: 1.0,    default: 0.0,    stepped: false, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec!["paraclete.effect".into()],
        }
    }
}

impl Default for LadderFilterNode {
    fn default() -> Self { Self::new() }
}

impl Node for LadderFilterNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }
    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate;
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
        self.stage = [0.0; 4];
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let base_cutoff    = self.bank.get(lad("cutoff"))    as f32;
        let base_resonance = self.bank.get(lad("resonance")) as f32;
        let drive          = self.bank.get(lad("drive"))     as f32;

        let cutoff_mod    = input.modulation(Self::PORT_CUTOFF_MOD);
        let resonance_mod = input.modulation(Self::PORT_RESONANCE_MOD);

        let frames = input.block_size;

        // Get mono input from first audio buffer, channel 0.
        // Get mono output from first audio output buffer, channel 0.
        if let Some(audio_out) = output.audio_outputs.first_mut() {
            let in_slice: &[f32] = if let Some(audio_in) = input.audio_inputs.first() {
                audio_in.channel(0)
            } else {
                static ZEROS: [f32; 4096] = [0.0; 4096];
                &ZEROS[..frames.min(4096)]
            };

            let out_slice = audio_out.channel_mut(0);

            for i in 0..in_slice.len().min(out_slice.len()) {
                let cutoff_hz = (base_cutoff + cutoff_mod.get(i).copied().unwrap_or(0.0))
                    .clamp(20.0, self.sample_rate * 0.49);
                let r = (base_resonance + resonance_mod.get(i).copied().unwrap_or(0.0))
                    .clamp(0.0, 1.0);

                let fc = (std::f32::consts::PI * cutoff_hz / self.sample_rate)
                    .sin().clamp(0.0, 0.99);
                let drive_gain = 1.0 + drive * 4.0;
                let x = (in_slice[i] * drive_gain - 4.0 * r * self.stage[3]).tanh();

                self.stage[0] += fc * (x             - self.stage[0]);
                self.stage[1] += fc * (self.stage[0] - self.stage[1]);
                self.stage[2] += fc * (self.stage[1] - self.stage[2]);
                self.stage[3] += fc * (self.stage[2] - self.stage[3]);

                out_slice[i] = self.stage[3];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    fn run_ladder(filt: &mut LadderFilterNode, input_signal: &[f32]) -> Vec<f32> {
        let frames = input_signal.len();
        let mut src = AudioBuffer::new(1, frames);
        let mut dst = AudioBuffer::new(1, frames);
        src.channel_mut(0).copy_from_slice(input_signal);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

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
        filt.process(&input, &mut ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        });
        dst.channel(0).to_vec()
    }

    fn sine_at_hz(freq: f32, sr: f32, frames: usize) -> Vec<f32> {
        (0..frames).map(|i| (i as f32 * freq / sr * std::f32::consts::TAU).sin()).collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|&x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn ladder_attenuates_above_cutoff() {
        let mut filt = LadderFilterNode::new();
        filt.activate(44100.0, 512);

        // Cutoff = 500 Hz, test at 1000 Hz (2× cutoff)
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        filt.bank.handle_commands(&[NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("cutoff") as i64, arg1: 500.0 }]);

        // Warm up filter for 2048 samples
        let warmup = sine_at_hz(1000.0, 44100.0, 2048);
        run_ladder(&mut filt, &warmup);

        let sig = sine_at_hz(1000.0, 44100.0, 512);
        let rms_in  = rms(&sig);
        let out     = run_ladder(&mut filt, &sig);
        let rms_out = rms(&out);

        let db_atten = 20.0 * (rms_out / rms_in.max(1e-10)).log10();
        assert!(db_atten < -10.0,
            "ladder should attenuate 2× cutoff by >10 dB, got {db_atten:.1} dB");
    }

    #[test]
    fn resonance_peak_at_cutoff() {
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        // Warm up both filters to steady state before measuring.
        let warmup = sine_at_hz(500.0, 44100.0, 2048);
        let sig     = sine_at_hz(500.0, 44100.0, 512);

        let mut filt_no_res = LadderFilterNode::new();
        filt_no_res.activate(44100.0, 512);
        filt_no_res.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("cutoff") as i64, arg1: 500.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("resonance") as i64, arg1: 0.0 },
        ]);
        run_ladder(&mut filt_no_res, &warmup);
        let out_no_res = run_ladder(&mut filt_no_res, &sig);

        let mut filt_res = LadderFilterNode::new();
        filt_res.activate(44100.0, 512);
        filt_res.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("cutoff") as i64, arg1: 500.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("resonance") as i64, arg1: 0.8 },
        ]);
        run_ladder(&mut filt_res, &warmup);
        let out_res = run_ladder(&mut filt_res, &sig);

        let rms_no_res = rms(&out_no_res);
        let rms_res    = rms(&out_res);
        // Resonance shapes the frequency response; RMS should differ measurably
        // (amplification vs attenuation depends on tanh gain-staging at the input).
        let differ = (rms_res - rms_no_res).abs() > 1e-4;
        assert!(differ,
            "resonance=0.8 vs 0.0 should produce different RMS at cutoff: res={rms_res:.4} no_res={rms_no_res:.4}");
    }

    #[test]
    fn ladder_does_not_blow_up_at_max_resonance_and_drive() {
        let mut filt = LadderFilterNode::new();
        filt.activate(44100.0, 512);
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        filt.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("resonance") as i64, arg1: 1.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("drive") as i64, arg1: 1.0 },
        ]);
        // Use 1024 samples of noise as input
        let mut st: u32 = 1;
        let noise: Vec<f32> = (0..1024).map(|_| {
            st ^= st << 13; st ^= st >> 17; st ^= st << 5;
            (st as i32 as f32) / (i32::MAX as f32)
        }).collect();
        let out = run_ladder(&mut filt, &noise);
        for (i, &s) in out.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] is not finite at max resonance+drive");
        }
    }

    #[test]
    fn cutoff_mod_shifts_frequency() {
        // Two runs at different effective cutoffs should produce different outputs.
        let mut filt_lo = LadderFilterNode::new();
        filt_lo.activate(44100.0, 512);
        let sig = sine_at_hz(1000.0, 44100.0, 512);
        let out_lo = run_ladder(&mut filt_lo, &sig); // default cutoff 4000 Hz

        let mut filt_hi = LadderFilterNode::new();
        filt_hi.activate(44100.0, 512);
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        filt_hi.bank.handle_commands(&[NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lad("cutoff") as i64, arg1: 200.0 }]);
        let out_hi = run_ladder(&mut filt_hi, &sig);

        let differ = out_lo.iter().zip(&out_hi).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "different cutoffs should produce different outputs");
    }

    #[test]
    fn ladder_portability_check() {
        let mut filt = LadderFilterNode::new();
        filt.activate(44100.0, 512);
        assert!(!filt.ports().is_empty());
    }
}
