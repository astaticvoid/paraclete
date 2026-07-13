use paraclete_node_api::{
    CapabilityDocument, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
};

#[cfg(test)]
use paraclete_node_api::{SignalInputSlot, SignalPortKind, SignalOutputSlot};

fn ep(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

#[derive(Clone, Copy, PartialEq)]
enum EnvPhase { Idle, Attack, Decay, Sustain, Release }

/// Triggered envelope generator. Outputs modulation 0.0–1.0.
/// Modes: 0=ADSR, 1=AD, 2=Looping AD.
/// Ports: gate_in (0, Logic), mod_out (1, Modulation).
pub struct EnvelopeNode {
    bank:       ParameterBank,
    phase:      EnvPhase,
    value:      f32,
    prev_gate:  f32,
    sample_rate: f32,
    node_id:    u32,
    ports:      [PortDescriptor; 2],
}

impl EnvelopeNode {
    pub const PORT_GATE_IN: u32 = 0;
    pub const PORT_MOD_OUT: u32 = 1;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            bank:        ParameterBank::from_capability_document(&doc),
            phase:       EnvPhase::Idle,
            value:       0.0,
            prev_gate:   0.0,
            sample_rate: 44100.0,
            node_id:     0,
            ports: [
                PortDescriptor { id: Self::PORT_GATE_IN, name: "gate_in".into(), direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_MOD_OUT, name: "mod_out".into(), direction: PortDirection::Output, port_type: PortType::Modulation },
            ],
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "EnvelopeNode".into(), vendor: "Paraclete".into(), version: (0, 6, 0),
            ports: vec![],
            params: vec![
                ParamDescriptor { id: ep("env_mode"), name: "env_mode".into(), min: 0.0, max: 2.0, default: 0.0, stepped: true,  unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: ep("attack"),   name: "attack".into(),   min: 0.001, max: 4.0, default: 0.01,  stepped: false, unit: ParamUnit::Seconds, display: None },
                ParamDescriptor { id: ep("decay"),    name: "decay".into(),    min: 0.001, max: 8.0, default: 0.3,   stepped: false, unit: ParamUnit::Seconds, display: None },
                ParamDescriptor { id: ep("sustain"),  name: "sustain".into(),  min: 0.0,   max: 1.0, default: 0.7,   stepped: false, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: ep("release"),  name: "release".into(),  min: 0.001, max: 8.0, default: 0.5,   stepped: false, unit: ParamUnit::Seconds, display: None },
            ],
            extensions: vec![],
        }
    }
}

impl Default for EnvelopeNode {
    fn default() -> Self { Self::new() }
}

impl Node for EnvelopeNode {
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

        let mode      = self.bank.get(ep("env_mode")) as u8;
        let attack_s  = self.bank.get(ep("attack"))   as f32;
        let decay_s   = self.bank.get(ep("decay"))    as f32;
        let sustain   = self.bank.get(ep("sustain"))  as f32;
        let release_s = self.bank.get(ep("release"))  as f32;

        let sr = self.sample_rate;
        let attack_inc    = 1.0 / (attack_s * sr).max(1.0);
        let decay_coeff   = 0.001_f32.powf(1.0 / (decay_s  * sr).max(1.0));
        let sustain_lvl   = sustain;
        let release_coeff = 0.001_f32.powf(1.0 / (release_s * sr).max(1.0));

        let gate = input.logic(Self::PORT_GATE_IN);
        let out  = output.mod_output_mut(Self::PORT_MOD_OUT);

        for i in 0..out.len() {
            let gate_high = gate.get(i).copied().unwrap_or(0.0) >= 0.5;
            let gate_rose = gate_high && self.prev_gate < 0.5;
            let gate_fell = !gate_high && self.prev_gate >= 0.5;
            self.prev_gate = gate.get(i).copied().unwrap_or(0.0);

            if gate_rose { self.phase = EnvPhase::Attack; }
            if gate_fell && mode == 0 { self.phase = EnvPhase::Release; }

            match self.phase {
                EnvPhase::Idle => {
                    // Looping AD starts autonomously — no gate required after initial cycle.
                    if mode == 2 { self.phase = EnvPhase::Attack; }
                }
                EnvPhase::Attack => {
                    self.value += attack_inc;
                    if self.value >= 1.0 {
                        self.value = 1.0;
                        self.phase = EnvPhase::Decay;
                    }
                }
                EnvPhase::Decay => {
                    self.value *= decay_coeff;
                    match mode {
                        0 => {
                            if self.value <= sustain_lvl {
                                self.value = sustain_lvl;
                                self.phase = EnvPhase::Sustain;
                            }
                        }
                        1 => {
                            if self.value < 1.0e-5 {
                                self.value = 0.0;
                                self.phase = EnvPhase::Idle;
                            }
                        }
                        _ => {
                            if self.value < 1.0e-5 {
                                self.value = 0.0;
                                self.phase = EnvPhase::Attack;
                            }
                        }
                    }
                }
                EnvPhase::Sustain => { self.value = sustain_lvl; }
                EnvPhase::Release => {
                    self.value *= release_coeff;
                    if self.value < 1.0e-5 {
                        self.value = 0.0;
                        self.phase = EnvPhase::Idle;
                    }
                }
            }

            out[i] = self.value;
        }
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) fn run_envelope_blocks(env: &mut EnvelopeNode, gate_blocks: &[Vec<f32>]) -> Vec<f32> {
    let block = gate_blocks.first().map(|b| b.len()).unwrap_or(512);
    let mut all_out = Vec::new();
    for gate_block in gate_blocks {
        let mut out_buf = vec![0.0f32; block];
        let mut events_out = paraclete_node_api::EventOutputBuffer::new(16);
        let transport = paraclete_node_api::TransportInfo::default();
        let slab = paraclete_node_api::ExtendedEventSlab::empty();
        let gate_slot = SignalInputSlot::new(EnvelopeNode::PORT_GATE_IN, SignalPortKind::Logic, gate_block);
        let sig_ins = [gate_slot];
        let out_slot = SignalOutputSlot::new(EnvelopeNode::PORT_MOD_OUT, SignalPortKind::Modulation, &mut out_buf);
        let mut sig_outs = [out_slot];
        let input = paraclete_node_api::ProcessInput {
            audio_inputs: &[], signal_inputs: &sig_ins, events: &[],
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: &[],
        };
        let mut output = paraclete_node_api::ProcessOutput {
            audio_outputs: &mut [], signal_outputs: &mut sig_outs,
            events_out: &mut events_out,
        };
        env.process(&input, &mut output);
        all_out.extend_from_slice(&out_buf);
    }
    all_out
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};

    fn set_param(env: &mut EnvelopeNode, name: &str, v: f64) {
        env.bank.handle_commands(&[NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ep(name) as i64, arg1: v }]);
    }

    fn gate_high(n: usize) -> Vec<f32> { vec![1.0; n] }
    fn gate_low(n: usize)  -> Vec<f32> { vec![0.0; n] }

    fn run_env(env: &mut EnvelopeNode, gate: &[Vec<f32>]) -> Vec<f32> {
        run_envelope_blocks(env, gate)
    }

    #[test]
    fn envelope_adsr_attack_reaches_one() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "attack", 0.01);
        set_param(&mut env, "decay", 1.0);
        set_param(&mut env, "sustain", 0.7);
        // Gate high for enough time to complete attack at 44100 Hz × 0.01s = 441 samples
        let out = run_env(&mut env, &[gate_high(512)]);
        let max = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(max >= 0.999, "attack should reach 1.0, got max={max}");
    }

    #[test]
    fn envelope_adsr_decay_falls_to_sustain() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "attack", 0.001);
        set_param(&mut env, "decay", 0.1);
        set_param(&mut env, "sustain", 0.5);
        // Run 8 blocks at 512 samples = 4096 samples ≈ 93ms; decay at 0.1s should be done.
        let blocks: Vec<Vec<f32>> = (0..8).map(|_| gate_high(512)).collect();
        let out = run_env(&mut env, &blocks);
        let last = out.last().copied().unwrap_or(0.0);
        assert!((last - 0.5).abs() < 0.02, "should settle at sustain=0.5, got {last}");
    }

    #[test]
    fn envelope_adsr_gate_low_triggers_release() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "attack", 0.001);
        set_param(&mut env, "decay", 0.01);
        set_param(&mut env, "sustain", 0.8);
        set_param(&mut env, "release", 0.01);
        // Sustain phase
        let blocks_on: Vec<Vec<f32>> = (0..4).map(|_| gate_high(512)).collect();
        run_env(&mut env, &blocks_on);
        // Release: gate low
        let out = run_env(&mut env, &[gate_low(512)]);
        // After release starts, value should be falling
        let first = out[0];
        let last  = out.last().copied().unwrap_or(0.0);
        assert!(last < first || last < 0.01, "release should decay: first={first}, last={last}");
    }

    #[test]
    fn envelope_adsr_release_reaches_zero() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "attack", 0.001);
        set_param(&mut env, "decay", 0.01);
        set_param(&mut env, "sustain", 1.0);
        set_param(&mut env, "release", 0.01);
        let blocks_on: Vec<Vec<f32>> = (0..4).map(|_| gate_high(512)).collect();
        run_env(&mut env, &blocks_on);
        // Long release window: 20 blocks = 10240 samples ≈ 232ms
        let blocks_off: Vec<Vec<f32>> = (0..20).map(|_| gate_low(512)).collect();
        let out = run_env(&mut env, &blocks_off);
        let last = out.last().copied().unwrap_or(0.0);
        assert!(last < 1e-3, "release should reach near-zero, got {last}");
    }

    #[test]
    fn envelope_ad_ignores_gate_low() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "env_mode", 1.0); // AD
        set_param(&mut env, "attack", 0.001);
        set_param(&mut env, "decay", 0.2);
        // Trigger with gate high then go low — AD should continue decaying
        let out = run_env(&mut env, &[gate_high(512)]);
        let peak = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let out2 = run_env(&mut env, &[gate_low(512)]);
        // Should continue decaying, not stuck at zero
        let sum = out2.iter().sum::<f32>();
        assert!(peak >= 0.9, "should reach peak");
        assert!(sum > 0.0, "AD mode: gate low should not stop decay");
    }

    #[test]
    fn envelope_ad_completes_without_gate() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "env_mode", 1.0); // AD
        set_param(&mut env, "attack", 0.001);
        set_param(&mut env, "decay", 0.005);
        // One gate pulse (512 samples with gate high), then many silent blocks
        let trigger = vec![gate_high(1), gate_low(511)].concat();
        let blocks: Vec<Vec<f32>> = (0..5).map(|i| if i == 0 { trigger.clone() } else { gate_low(512) }).collect();
        let out = run_env(&mut env, &blocks);
        let last = out.last().copied().unwrap_or(0.0);
        assert!(last < 1e-3, "AD envelope should complete and reach zero");
    }

    #[test]
    fn envelope_looping_ad_restarts_after_decay() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "env_mode", 2.0); // Looping AD
        set_param(&mut env, "attack", 0.001);
        set_param(&mut env, "decay", 0.001);
        // Looping: should oscillate. Run many blocks; count peaks.
        let trigger = vec![gate_high(1), gate_low(511)].concat();
        let mut blocks = vec![trigger];
        blocks.extend((0..15).map(|_| gate_low(512)));
        let out = run_env(&mut env, &blocks);
        // Count peaks (value near 1.0)
        let peaks = out.windows(2).filter(|w| w[0] > 0.9 && w[1] <= w[0]).count();
        assert!(peaks >= 2, "looping AD should produce multiple peaks, got {peaks}");
    }

    #[test]
    fn envelope_looping_ad_no_gate_cycles_continuously() {
        // Looping AD must cycle autonomously — no gate signal after the initial trigger.
        // With the fix, mode=2 transitions from Idle to Attack without a gate pulse.
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "env_mode", 2.0); // Looping AD
        set_param(&mut env, "attack", 0.005);
        set_param(&mut env, "decay", 0.005);
        // One cycle ≈ (0.005 + 0.005) × 44100 ≈ 441 samples. 8 blocks = 4096 samples ≈ 9 cycles.
        // Gate is all-zeros throughout — autonomous loop only.
        let blocks: Vec<Vec<f32>> = (0..8).map(|_| gate_low(512)).collect();
        let out = run_env(&mut env, &blocks);
        let peaks = out.windows(2).filter(|w| w[0] > 0.9 && w[1] <= w[0]).count();
        assert!(peaks >= 2,
            "looping AD should cycle continuously with no gate; got {peaks} peaks");
    }

    #[test]
    fn envelope_retrigger_from_nonzero_no_click() {
        let mut env = EnvelopeNode::new();
        env.activate(44100.0, 512);
        set_param(&mut env, "attack", 0.01);
        set_param(&mut env, "decay", 0.2);
        set_param(&mut env, "sustain", 0.0);
        // First trigger
        run_env(&mut env, &[gate_high(512)]);
        // Mid-decay (value is somewhere between 0 and 1): retrigger
        let out_before = env.value;
        let out = run_env(&mut env, &[gate_high(512)]);
        // Value should NOT drop discontinuously on retrigger
        let first = out[0];
        assert!((first - out_before).abs() < 0.1,
            "retrigger should not cause discontinuity: before={out_before}, first={first}");
    }
}
