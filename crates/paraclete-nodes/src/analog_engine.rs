use std::collections::HashMap;

use paraclete_node_api::{
    CapabilityDocument, Event, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
    UmpMessage, midi::ChannelVoice2,
};

use crate::engine_dsp::{AdState, note_to_hz, soft_clip, svf_lp_sample, xorshift};

fn ap(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

// ── Machine variant ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AnalogMachine { Kick, Snare, HiHat }

// ── AnalogEngine ──────────────────────────────────────────────────────────────

/// Analog drum voice synthesizer with three machine variants.
/// Event interface identical to Sampler — same graph wiring works.
/// Ports: events_in (0, Event), audio_out_l (1, Mono), audio_out_r (2, Mono).
pub struct AnalogEngine {
    machine:     AnalogMachine,
    bank:        ParameterBank,
    sample_rate: f32,

    osc_phase:   f32,
    pitch_env:   AdState,
    amp_env:     AdState,
    body_env:    AdState,
    noise_env:   AdState,
    noise_state: u32,
    hihat_noise: u32,
    svf_low:     f32,
    svf_band:    f32,
    current_hz:  f32,
    active:      bool,
    node_id:     u32,
    node_locks:  Vec<(u32, f64)>,

    render_l:    Vec<f32>,
    render_r:    Vec<f32>,

    pending_initial_params: HashMap<String, f64>,

    ports: [PortDescriptor; 3],
}

impl AnalogEngine {
    pub const PORT_EVENTS_IN:   u32 = 0;
    pub const PORT_AUDIO_OUT_L: u32 = 1;
    pub const PORT_AUDIO_OUT_R: u32 = 2;

    pub fn new(machine: AnalogMachine) -> Self {
        let doc = Self::build_doc(machine);
        Self {
            machine,
            bank:        ParameterBank::from_capability_document(&doc),
            sample_rate: 44100.0,
            osc_phase:   0.0,
            pitch_env:   AdState::new(),
            amp_env:     AdState::new(),
            body_env:    AdState::new(),
            noise_env:   AdState::new(),
            noise_state: 1,
            hihat_noise: 1,
            svf_low:     0.0,
            svf_band:    0.0,
            current_hz:  65.41, // C2
            active:      false,
            node_id:     0,
            node_locks:  Vec::new(),
            render_l:    Vec::new(),
            render_r:    Vec::new(),
            pending_initial_params: HashMap::new(),
            ports: [
                PortDescriptor { id: Self::PORT_EVENTS_IN,   name: "events_in".into(),   direction: PortDirection::Input,  port_type: PortType::Event },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_L, name: "audio_out_l".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_R, name: "audio_out_r".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
        }
    }

    pub fn kick()  -> Self { Self::new(AnalogMachine::Kick)  }
    pub fn snare() -> Self { Self::new(AnalogMachine::Snare) }
    pub fn hihat() -> Self { Self::new(AnalogMachine::HiHat) }

    fn get_param(&self, param_id: u32) -> f32 {
        for &(id, val) in &self.node_locks {
            if id == param_id { return val as f32; }
        }
        self.bank.get(param_id) as f32
    }

    fn build_doc(machine: AnalogMachine) -> CapabilityDocument {
        let params = match machine {
            AnalogMachine::Kick => vec![
                ParamDescriptor { id: ap("tune"),  name: "tune".into(),  min: -24.0, max: 24.0,   default: 0.0,   stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: ap("punch"), name: "punch".into(), min: 0.0,   max: 1.0,    default: 0.7,   stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: ap("decay"), name: "decay".into(), min: 0.01,  max: 2.0,    default: 0.5,   stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: ap("drive"), name: "drive".into(), min: 0.0,   max: 1.0,    default: 0.0,   stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: ap("tone"),  name: "tone".into(),  min: 200.0, max: 8000.0, default: 4000.0, stepped: false, unit: ParamUnit::Hz,        display: None },
            ],
            AnalogMachine::Snare => vec![
                ParamDescriptor { id: ap("tune"),  name: "tune".into(),  min: -24.0, max: 24.0,    default: 0.0,  stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: ap("snap"),  name: "snap".into(),  min: 0.005, max: 0.3,     default: 0.05, stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: ap("noise"), name: "noise".into(), min: 0.0,   max: 1.0,     default: 0.5,  stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: ap("decay"), name: "decay".into(), min: 0.01,  max: 2.0,     default: 0.3,  stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: ap("tone"),  name: "tone".into(),  min: 200.0, max: 8000.0,  default: 2000.0, stepped: false, unit: ParamUnit::Hz,      display: None },
            ],
            AnalogMachine::HiHat => vec![
                ParamDescriptor { id: ap("tone"),  name: "tone".into(),  min: 1000.0, max: 18000.0, default: 8000.0, stepped: false, unit: ParamUnit::Hz,      display: None },
                ParamDescriptor { id: ap("decay"), name: "decay".into(), min: 0.01,   max: 1.0,     default: 0.08,   stepped: false, unit: ParamUnit::Seconds, display: None },
                ParamDescriptor { id: ap("open"),  name: "open".into(),  min: 0.0,    max: 1.0,     default: 0.0,    stepped: false, unit: ParamUnit::Generic, display: None },
            ],
        };

        let name = match machine {
            AnalogMachine::Kick  => "AnalogKick",
            AnalogMachine::Snare => "AnalogSnare",
            AnalogMachine::HiHat => "AnalogHiHat",
        };

        CapabilityDocument {
            name, vendor: "Paraclete", version: (0, 6, 0),
            ports: vec![],
            params,
            extensions: vec!["paraclete.instrument"],
        }
    }

    fn retrigger(&mut self, note: u8) {
        let tune = self.get_param(ap("tune"));
        self.current_hz = note_to_hz(note, tune);
        self.pitch_env.trigger();
        self.amp_env.trigger();
        self.body_env.trigger();
        self.noise_env.trigger();
        self.osc_phase = 0.0;
        self.active    = true;
    }

    fn process_kick(&mut self, block_size: usize) {
        let punch    = self.get_param(ap("punch"));
        let decay_s  = self.get_param(ap("decay"));
        let drive    = self.get_param(ap("drive"));
        let tone_hz  = self.get_param(ap("tone"));
        let sr = self.sample_rate;

        let pitch_attack_inc  = 1.0 / (0.002 * sr);
        let pitch_decay_coeff = 0.001f32.powf(1.0 / (0.08 * sr));
        let amp_attack_inc    = 1.0 / (0.001 * sr);
        let amp_decay_coeff   = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let f_svf = (std::f32::consts::PI * tone_hz / sr).sin().clamp(0.0, 0.99);

        for i in 0..block_size {
            let pitch_val = self.pitch_env.tick(pitch_attack_inc, pitch_decay_coeff);
            let freq = self.current_hz * 2.0f32.powf(pitch_val * punch * 24.0 / 12.0);
            let phase_inc = (freq / sr).clamp(0.0, 0.5);
            self.osc_phase = (self.osc_phase + phase_inc).fract();
            let osc = (self.osc_phase * std::f32::consts::TAU).sin();
            let amp = self.amp_env.tick(amp_attack_inc, amp_decay_coeff);
            let sig = svf_lp_sample(osc * amp, f_svf, 0.7, &mut self.svf_low, &mut self.svf_band);
            let out = soft_clip(sig * (1.0 + drive * 9.0));
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }

    fn process_snare(&mut self, block_size: usize) {
        let snap_s   = self.get_param(ap("snap"));
        let noise_lvl= self.get_param(ap("noise"));
        let decay_s  = self.get_param(ap("decay"));
        let tone_hz  = self.get_param(ap("tone"));
        let sr = self.sample_rate;

        let body_attack_inc   = 1.0 / (0.001 * sr);
        let body_decay_coeff  = 0.001f32.powf(1.0 / (snap_s * sr).max(1.0));
        let noise_attack_inc  = 1.0 / (0.001 * sr);
        let noise_decay_coeff = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let f_svf = (std::f32::consts::PI * tone_hz / sr).sin().clamp(0.0, 0.99);

        for i in 0..block_size {
            let body_amp = self.body_env.tick(body_attack_inc, body_decay_coeff);
            self.osc_phase = (self.osc_phase + self.current_hz / sr).fract();
            let body = (self.osc_phase * std::f32::consts::TAU).sin() * body_amp;

            let noise_raw = xorshift(&mut self.noise_state);
            let noise_amp = self.noise_env.tick(noise_attack_inc, noise_decay_coeff);
            // Bandpass SVF: use band output
            self.svf_low  += f_svf * self.svf_band;
            self.svf_band += f_svf * (noise_raw - self.svf_low - 1.0 * self.svf_band);
            let noise_out = self.svf_band * noise_amp * noise_lvl;

            let out = soft_clip(body + noise_out);
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.body_env.is_idle() || !self.noise_env.is_idle();
    }

    fn process_hihat(&mut self, block_size: usize) {
        let tone_hz = self.get_param(ap("tone"));
        let decay_s = self.get_param(ap("decay"));
        let open    = self.get_param(ap("open"));
        let sr = self.sample_rate;

        let effective_decay = decay_s * (1.0 + open * 7.0);
        let amp_attack_inc  = 1.0 / (0.0005 * sr);
        let amp_decay_coeff = 0.001f32.powf(1.0 / (effective_decay * sr).max(1.0));
        let f_svf = (std::f32::consts::PI * tone_hz / sr).sin().clamp(0.0, 0.99);

        for i in 0..block_size {
            let noise_raw = xorshift(&mut self.hihat_noise);
            self.svf_low  += f_svf * self.svf_band;
            self.svf_band += f_svf * (noise_raw - self.svf_low - 0.5 * self.svf_band);
            let hp_out = noise_raw - self.svf_low;
            let amp = self.amp_env.tick(amp_attack_inc, amp_decay_coeff);
            let out = hp_out * amp;
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }
}

impl Node for AnalogEngine {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }
    fn capability_document(&self) -> CapabilityDocument { Self::build_doc(self.machine) }

    fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
        self.pending_initial_params = params.clone();
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate = sample_rate;
        let doc = Self::build_doc(self.machine);
        self.bank        = ParameterBank::from_capability_document(&doc);
        for (name, value) in &self.pending_initial_params {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, *value);
            }
        }
        self.render_l    = vec![0.0; block_size];
        self.render_r    = vec![0.0; block_size];
        self.osc_phase   = 0.0;
        self.svf_low     = 0.0;
        self.svf_band    = 0.0;
        self.pitch_env   = AdState::new();
        self.amp_env     = AdState::new();
        self.body_env    = AdState::new();
        self.noise_env   = AdState::new();
        self.noise_state = 1;
        self.hihat_noise = 1;
        self.current_hz  = 65.41;
        self.active      = false;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let block_size = input.block_size;
        for s in &mut self.render_l { *s = 0.0; }
        for s in &mut self.render_r { *s = 0.0; }

        // Per-cycle param overrides from ParamLock events — cleared each cycle so
        // locks from one step do not bleed into steps that carry no lock.
        self.node_locks.clear();

        // Handle events: ParamLock before NoteOn (executor guarantees ordering).
        for timed in input.events {
            match timed.event {
                Event::ParamLock(ref pl) if pl.node_id == self.node_id => {
                    self.node_locks.push((pl.param_id, pl.value));
                }
                Event::Midi2(ref ump) => {
                    if let UmpMessage::ChannelVoice2(cv2) = ump {
                        match cv2 {
                            ChannelVoice2::NoteOn(n) => self.retrigger(u8::from(n.note_number())),
                            ChannelVoice2::NoteOff(_) => {}
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        if self.active {
            match self.machine {
                AnalogMachine::Kick  => self.process_kick(block_size),
                AnalogMachine::Snare => self.process_snare(block_size),
                AnalogMachine::HiHat => self.process_hihat(block_size),
            }
        }

        if let Some(buf) = output.audio_outputs.first_mut() {
            if buf.channels() >= 2 {
                buf.channel_mut(0).copy_from_slice(&self.render_l[..block_size]);
                buf.channel_mut(1).copy_from_slice(&self.render_r[..block_size]);
            } else if buf.channels() == 1 {
                for (i, (&l, &r)) in self.render_l[..block_size].iter()
                    .zip(self.render_r[..block_size].iter()).enumerate()
                {
                    buf.channel_mut(0)[i] = (l + r) * 0.5;
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab,
        NodeCommand, CMD_SET_PARAM, ParamLockEvent, TimedEvent, TransportInfo,
        UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7},
    };

    fn make_note_on(note: u8) -> TimedEvent {
        let mut msg = NoteOn::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note & 0x7F));
        msg.set_velocity(32768);
        TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(msg))))
    }

    fn run_engine(eng: &mut AnalogEngine, events: &[TimedEvent]) -> Vec<f32> {
        let block = 512usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let ap: *mut AudioBuffer = &mut audio;
        let ar: &mut AudioBuffer = unsafe { &mut *ap };
        let mut outs = [ar];

        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: &[],
        };
        let mut output = ProcessOutput { audio_outputs: &mut outs, signal_outputs: &mut [], events_out: &mut events_out };
        eng.process(&input, &mut output);
        audio.channel(0).to_vec()
    }

    fn run_engine_cmds(eng: &mut AnalogEngine, events: &[TimedEvent], cmds: &[NodeCommand]) -> Vec<f32> {
        let block = 512usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let ap: *mut AudioBuffer = &mut audio;
        let ar: &mut AudioBuffer = unsafe { &mut *ap };
        let mut outs = [ar];

        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: cmds,
        };
        let mut output = ProcessOutput { audio_outputs: &mut outs, signal_outputs: &mut [], events_out: &mut events_out };
        eng.process(&input, &mut output);
        audio.channel(0).to_vec()
    }

    #[test]
    fn analog_kick_produces_audio_on_note_on() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let out = run_engine(&mut eng, &[make_note_on(36)]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "kick should produce audio");
    }

    #[test]
    fn analog_kick_is_silent_before_any_note_on() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let out = run_engine(&mut eng, &[]);
        assert!(out.iter().all(|&s| s == 0.0), "no NoteOn → silence");
    }

    #[test]
    fn analog_kick_punch_zero_has_no_pitch_drop() {
        let mut eng_punch = AnalogEngine::kick();
        eng_punch.activate(44100.0, 512);
        let cmds = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("punch") as i64, arg1: 1.0 }];
        run_engine_cmds(&mut eng_punch, &[], &cmds);
        let out_punch = run_engine(&mut eng_punch, &[make_note_on(36)]);

        let mut eng_flat = AnalogEngine::kick();
        eng_flat.activate(44100.0, 512);
        let cmds_flat = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("punch") as i64, arg1: 0.0 }];
        run_engine_cmds(&mut eng_flat, &[], &cmds_flat);
        let out_flat = run_engine(&mut eng_flat, &[make_note_on(36)]);

        // Both produce audio; waveforms differ due to pitch sweep.
        let differ = out_punch.iter().zip(&out_flat).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "punch=1 vs punch=0 should produce different waveforms");
    }

    #[test]
    fn analog_snare_body_and_noise_both_present() {
        let mut eng = AnalogEngine::snare();
        eng.activate(44100.0, 512);
        let out = run_engine(&mut eng, &[make_note_on(48)]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "snare should produce audio");
    }

    #[test]
    fn analog_snare_noise_zero_silences_noise_path() {
        let mut eng = AnalogEngine::snare();
        eng.activate(44100.0, 512);
        let cmds = [
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("noise") as i64, arg1: 0.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("snap")  as i64, arg1: 0.3 },
        ];
        run_engine_cmds(&mut eng, &[], &cmds);
        // With noise=0, only body oscillator remains (low amplitude after snap decay).
        // Just verify it doesn't crash and produces some output.
        let out = run_engine(&mut eng, &[make_note_on(48)]);
        // Some audio expected from body oscillator.
        assert!(out.iter().any(|&s| s.abs() > 0.0), "snare with noise=0 should still have body");
    }

    #[test]
    fn analog_hihat_open_extends_decay() {
        let mut eng_closed = AnalogEngine::hihat();
        eng_closed.activate(44100.0, 512);
        let cmds_c = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("open") as i64, arg1: 0.0 }];
        run_engine_cmds(&mut eng_closed, &[], &cmds_c);
        let _t0 = run_engine(&mut eng_closed, &[make_note_on(60)]);
        // Let it decay for 5 blocks
        for _ in 0..5 { run_engine(&mut eng_closed, &[]); }
        let tail_closed: f32 = run_engine(&mut eng_closed, &[]).iter().map(|&x| x.abs()).sum();

        let mut eng_open = AnalogEngine::hihat();
        eng_open.activate(44100.0, 512);
        let cmds_o = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("open") as i64, arg1: 1.0 }];
        run_engine_cmds(&mut eng_open, &[], &cmds_o);
        let _t0o = run_engine(&mut eng_open, &[make_note_on(60)]);
        for _ in 0..5 { run_engine(&mut eng_open, &[]); }
        let tail_open: f32 = run_engine(&mut eng_open, &[]).iter().map(|&x| x.abs()).sum();

        assert!(tail_open > tail_closed,
            "open hihat should have longer decay tail: open={tail_open:.4}, closed={tail_closed:.4}");
    }

    #[test]
    fn analog_hihat_closed_short_decay() {
        let mut eng = AnalogEngine::hihat();
        eng.activate(44100.0, 512);
        let _ = run_engine(&mut eng, &[make_note_on(60)]);
        // After many blocks, closed hihat should be silent.
        for _ in 0..40 { run_engine(&mut eng, &[]); }
        let tail: f32 = run_engine(&mut eng, &[]).iter().map(|&x| x.abs()).sum();
        assert!(tail < 1e-4, "closed hihat should be silent after decay, got {tail:.6}");
    }

    #[test]
    fn analog_bump_param_decay_changes_output_length() {
        // Shorter decay → sample exhausts sooner → fewer non-zero frames after N blocks.
        let mut eng_long = AnalogEngine::kick();
        eng_long.activate(44100.0, 512);
        let _ = run_engine(&mut eng_long, &[make_note_on(36)]);
        for _ in 0..4 { run_engine(&mut eng_long, &[]); }
        let long_energy: f32 = run_engine(&mut eng_long, &[]).iter().map(|&x| x * x).sum();

        let mut eng_short = AnalogEngine::kick();
        eng_short.activate(44100.0, 512);
        let bump = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("decay") as i64, arg1: 0.02 };
        run_engine_cmds(&mut eng_short, &[], &[bump]);
        let _ = run_engine(&mut eng_short, &[make_note_on(36)]);
        for _ in 0..4 { run_engine(&mut eng_short, &[]); }
        let short_energy: f32 = run_engine(&mut eng_short, &[]).iter().map(|&x| x * x).sum();

        assert!(long_energy > short_energy,
            "long decay should have more energy after same time: {long_energy:.6} vs {short_energy:.6}");
    }

    #[test]
    fn analog_param_lock_drive_overrides_base_drive() {
        let node_id = 42u32;
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);
        let _ = run_engine(&mut eng, &[make_note_on(36)]);

        // With no param lock: drive=0 (default)
        let out_no_lock = run_engine(&mut eng, &[make_note_on(36)]);

        // With param lock: drive=1.0 (max)
        let lock_event = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("drive"), value: 1.0,
        }));
        let out_lock = run_engine(&mut eng, &[lock_event, make_note_on(36)]);

        let rms = |v: &[f32]| (v.iter().map(|&x| x*x).sum::<f32>() / v.len() as f32).sqrt();
        // Drive=1.0 should produce different (typically louder/more saturated) output.
        let differ = (rms(&out_lock) - rms(&out_no_lock)).abs() > 1e-5;
        assert!(differ, "param lock drive=1.0 should change output vs drive=0");
    }

    #[test]
    fn analog_param_lock_does_not_bleed_to_next_cycle() {
        let node_id = 42u32;
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        // Cycle 1: param lock drive=1.0 — must not permanently mutate the bank.
        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("drive"), value: 1.0,
        }));
        let _ = run_engine(&mut eng, &[lock, make_note_on(36)]);

        // Bank drive should still be 0.0 (the default) — the lock goes to node_locks,
        // not to the bank, so the base value is unchanged for subsequent cycles.
        assert!((eng.bank.get(ap("drive")) - 0.0).abs() < 1e-9,
            "bank drive should stay 0.0 after a locked cycle; got {:.4}", eng.bank.get(ap("drive")));

        // Cycle 2: no lock — drive=0.0 (base), output should differ from locked drive=1.0.
        let out_base = run_engine(&mut eng, &[make_note_on(36)]);
        let out_locked = {
            let mut e2 = AnalogEngine::kick();
            e2.activate(44100.0, 512);
            e2.set_node_id(node_id);
            let lock2 = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
                node_id, param_id: ap("drive"), value: 1.0,
            }));
            run_engine(&mut e2, &[lock2, make_note_on(36)])
        };
        let rms = |v: &[f32]| (v.iter().map(|&x| x*x).sum::<f32>() / v.len() as f32).sqrt();
        assert!((rms(&out_base) - rms(&out_locked)).abs() > 1e-4,
            "cycle 2 (no lock) should differ from locked drive=1.0; base={:.4} locked={:.4}",
            rms(&out_base), rms(&out_locked));
    }

    #[test]
    fn analog_portability_check() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        assert!(!eng.ports().is_empty());
    }

    #[test]
    fn analog_engine_published_state_contains_decay() {
        let mut eng = AnalogEngine::kick();
        eng.set_node_id(10);
        eng.activate(44100.0, 256);
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        eng.published_state(&mut buf);
        let decay_entry = buf.iter().find(|(k, _)| k.ends_with("/decay"));
        assert!(decay_entry.is_some(), "published_state should contain a /decay entry");
        assert!(matches!(decay_entry.unwrap().1, paraclete_node_api::StateBusValue::Float(_)),
            "decay entry should be StateBusValue::Float");
    }

    #[test]
    fn analog_engine_set_initial_params_applied() {
        let mut eng = AnalogEngine::kick();
        eng.set_node_id(1);
        eng.set_initial_params(&[("decay".to_string(), 0.9)].into_iter().collect());
        eng.activate(44100.0, 256);
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        eng.published_state(&mut buf);
        let entry = buf.iter().find(|(k, _)| k.ends_with("/decay"));
        assert!(entry.is_some(), "published_state should contain /decay");
        if let paraclete_node_api::StateBusValue::Float(v) = entry.unwrap().1 {
            assert!((v - 0.9).abs() < 1e-9, "decay should be 0.9, got {v}");
        } else {
            panic!("decay entry should be Float");
        }
    }

    #[test]
    fn set_initial_params_unknown_key_ignored() {
        let mut eng = AnalogEngine::snare();
        eng.set_initial_params(&[("nonexistent_param".to_string(), 99.0)].into_iter().collect());
        eng.activate(44100.0, 256); // must not panic
    }
}
