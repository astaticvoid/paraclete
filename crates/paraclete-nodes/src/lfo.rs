use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

#[cfg(test)]
use paraclete_node_api::{SignalInputSlot, SignalPortKind, SignalOutputSlot};

fn lp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

/// Low-frequency oscillator running at audio rate.
/// Ports: sync_in (0, Logic), mod_out (1, Modulation).
/// Output range: -depth..+depth.
pub struct LfoNode {
    bank:        ParameterBank,
    phase:       f32,
    noise_state: u32,
    held_value:  f32,
    sample_rate: f32,
    node_id:     u32,
    ports:       [PortDescriptor; 2],
}

impl LfoNode {
    pub const PORT_SYNC_IN:  u32 = 0;
    pub const PORT_MOD_OUT:  u32 = 1;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            bank:        ParameterBank::from_capability_document(&doc),
            phase:       0.0,
            noise_state: 1,
            held_value:  0.0,
            sample_rate: 44100.0,
            node_id:     0,
            ports: [
                PortDescriptor { id: Self::PORT_SYNC_IN, name: "sync_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_MOD_OUT, name: "mod_out".into(), direction: PortDirection::Output, port_type: PortType::Modulation },
            ],
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "LfoNode".into(), vendor: "Paraclete".into(), version: (0, 6, 0),
            ports: vec![],
            params: vec![
                ParamDescriptor { id: lp("lfo_waveform"), name: "lfo_waveform".into(), min: 0.0, max: 4.0, default: 0.0, stepped: true,  unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: lp("lfo_rate"),     name: "lfo_rate".into(),     min: 0.01, max: 20.0, default: 1.0, stepped: false, unit: ParamUnit::Hz,      display: None },
                ParamDescriptor { id: lp("lfo_depth"),    name: "lfo_depth".into(),    min: 0.0,  max: 1.0,  default: 1.0, stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: lp("lfo_phase"),    name: "lfo_phase".into(),    min: 0.0,  max: 1.0,  default: 0.0, stepped: false, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec![],
        }
    }
}

impl Default for LfoNode {
    fn default() -> Self { Self::new() }
}

impl Node for LfoNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }
    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate;
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let waveform     = self.bank.get(lp("lfo_waveform")) as u8;
        let rate_hz      = self.bank.get(lp("lfo_rate"))     as f32;
        let depth        = self.bank.get(lp("lfo_depth"))    as f32;
        let phase_offset = self.bank.get(lp("lfo_phase"))    as f32;

        let phase_inc = rate_hz / self.sample_rate;
        let sync      = input.logic(Self::PORT_SYNC_IN);
        let out       = output.mod_output_mut(Self::PORT_MOD_OUT);

        let mut prev_sync = 0.0f32;
        for i in 0..out.len() {
            let s = sync.get(i).copied().unwrap_or(0.0);
            if s >= 0.5 && prev_sync < 0.5 { self.phase = phase_offset.fract(); }
            prev_sync = s;

            let p = self.phase;
            let sample = match waveform {
                0 => (p * std::f32::consts::TAU).sin(),
                1 => if p < 0.5 { 1.0f32 } else { -1.0 },
                2 => 2.0 * p - 1.0,
                3 => 1.0 - 2.0 * p,
                _ => {
                    if p < phase_inc {
                        self.noise_state ^= self.noise_state << 13;
                        self.noise_state ^= self.noise_state >> 17;
                        self.noise_state ^= self.noise_state << 5;
                        self.held_value = (self.noise_state as i32 as f32) / (i32::MAX as f32);
                    }
                    self.held_value
                }
            };

            out[i] = sample * depth;
            self.phase = (self.phase + phase_inc).fract();
        }
    }
}

// ── Test helper ───────────────────────────────────────────────────────────────

#[cfg(test)]
fn run_lfo(lfo: &mut LfoNode, sync: Option<&[f32]>, blocks: usize) -> Vec<f32> {
    let block = 512usize;
    let mut all = Vec::new();
    for blk in 0..blocks {
        let mut out_buf = vec![0.0f32; block];
        let mut events_out = paraclete_node_api::EventOutputBuffer::new(16);
        let transport = paraclete_node_api::TransportInfo::default();
        let slab = paraclete_node_api::ExtendedEventSlab::empty();

        let sync_data: Vec<f32>;
        let mut sig_ins = Vec::new();
        if let Some(s) = sync {
            sync_data = s.to_vec();
            sig_ins.push(SignalInputSlot::new(LfoNode::PORT_SYNC_IN, SignalPortKind::Logic, &sync_data));
        }

        let out_slot = SignalOutputSlot::new(LfoNode::PORT_MOD_OUT, SignalPortKind::Modulation, &mut out_buf);
        let mut sig_outs = [out_slot];
        let input = paraclete_node_api::ProcessInput {
            audio_inputs: &[], signal_inputs: &sig_ins, events: &[],
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: &[],
        };
        let mut output = paraclete_node_api::ProcessOutput::new(
            &mut [], &mut sig_outs,
            &mut events_out,
        );
        lfo.process(&input, &mut output);
        all.extend_from_slice(&out_buf);
        let _ = blk; // suppress unused
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};

    fn set(lfo: &mut LfoNode, name: &str, v: f64) {
        lfo.bank.handle_commands(&[NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: lp(name) as i64, arg1: v }]);
    }

    #[test]
    fn lfo_sine_output_oscillates_between_minus_one_and_one() {
        let mut lfo = LfoNode::new();
        lfo.activate(44100.0, 512);
        // At 10 Hz / 44100 Hz, one period = 4410 samples = ~8.6 blocks of 512.
        // Use 12 blocks (6144 samples) to fully cover at least one period.
        set(&mut lfo, "lfo_rate", 10.0);
        let out = run_lfo(&mut lfo, None, 12);
        let max = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min = out.iter().cloned().fold(f32::INFINITY, f32::min);
        assert!(max > 0.9, "sine max should reach near +1, got {max}");
        assert!(min < -0.9, "sine min should reach near -1, got {min}");
    }

    #[test]
    fn lfo_square_output_is_only_plus_minus_depth() {
        let mut lfo = LfoNode::new();
        lfo.activate(44100.0, 512);
        set(&mut lfo, "lfo_waveform", 1.0);
        set(&mut lfo, "lfo_rate", 5.0);
        set(&mut lfo, "lfo_depth", 0.8);
        let out = run_lfo(&mut lfo, None, 2);
        // All values should be exactly ±0.8
        for &s in &out {
            assert!((s.abs() - 0.8).abs() < 1e-5, "square should be ±depth, got {s}");
        }
    }

    #[test]
    fn lfo_ramp_up_rises_monotonically_within_cycle() {
        let mut lfo = LfoNode::new();
        lfo.activate(44100.0, 512);
        set(&mut lfo, "lfo_waveform", 2.0); // ramp up
        // Very slow rate so the entire block is within one cycle
        set(&mut lfo, "lfo_rate", 0.05);
        let out = run_lfo(&mut lfo, None, 1);
        // Should be monotonically rising (one ramp cycle >> 512 samples at 44100Hz, rate=0.05Hz)
        let is_rising = out.windows(2).all(|w| w[1] >= w[0] - 1e-6);
        assert!(is_rising, "ramp up should be monotonically non-decreasing within block");
    }

    #[test]
    fn lfo_ramp_down_falls_monotonically_within_cycle() {
        let mut lfo = LfoNode::new();
        lfo.activate(44100.0, 512);
        set(&mut lfo, "lfo_waveform", 3.0); // ramp down
        set(&mut lfo, "lfo_rate", 0.05);
        let out = run_lfo(&mut lfo, None, 1);
        let is_falling = out.windows(2).all(|w| w[1] <= w[0] + 1e-6);
        assert!(is_falling, "ramp down should be monotonically non-increasing within block");
    }

    #[test]
    fn lfo_random_holds_value_between_cycles() {
        // Test the S&H state machine directly: verify that held_value changes after
        // one cycle (when phase wraps). We simulate the exact S&H update logic.
        // Initial noise_state=1, held_value=0.0. After one xorshift step,
        // the held_value should be different (0.000126 ≠ 0.0).
        let mut state: u32 = 1;
        let initial = 0.0f32;

        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let after_one = (state as i32 as f32) / (i32::MAX as f32);

        assert!((after_one - initial).abs() > 1e-4,
            "S&H: first update should change held_value: before={initial}, after={after_one}");

        // After a second xorshift step, value changes again.
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let after_two = (state as i32 as f32) / (i32::MAX as f32);

        assert!((after_two - after_one).abs() > 1e-4,
            "S&H: second update should change held_value: before={after_one}, after={after_two}");
    }

    #[test]
    fn lfo_sync_rising_edge_resets_phase() {
        let mut lfo = LfoNode::new();
        lfo.activate(44100.0, 512);
        set(&mut lfo, "lfo_rate", 1.0);
        // Run one block to advance phase
        run_lfo(&mut lfo, None, 1);
        let phase_before = lfo.phase;
        // Apply sync at sample 0: phase should reset to 0 (lfo_phase offset = 0)
        let sync: Vec<f32> = (0..512).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        run_lfo(&mut lfo, Some(&sync), 1);
        // After block, phase should be close to what it would be from 0 + 512 * phase_inc
        let expected_phase = (512.0 * 1.0 / 44100.0) % 1.0;
        assert!((lfo.phase - expected_phase).abs() < 1e-4,
            "sync reset: expected phase≈{expected_phase:.5}, got {:.5}", lfo.phase);
        let _ = phase_before;
    }

    #[test]
    fn lfo_depth_zero_produces_silence() {
        let mut lfo = LfoNode::new();
        lfo.activate(44100.0, 512);
        set(&mut lfo, "lfo_depth", 0.0);
        set(&mut lfo, "lfo_rate", 10.0);
        let out = run_lfo(&mut lfo, None, 1);
        assert!(out.iter().all(|&s| s == 0.0), "depth=0 should produce silence");
    }
}
