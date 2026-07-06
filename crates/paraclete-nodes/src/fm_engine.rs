use std::collections::HashMap;

use paraclete_node_api::{
    CapabilityDocument, Event, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
    UmpMessage, midi::ChannelVoice2, CMD_TRIGGER,
};

use crate::engine_dsp::{AdState, note_to_hz, soft_clip};

fn fp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

// ── Machine variant ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FmMachine { Kick, Bell, Bass }

// ── FmEngine ──────────────────────────────────────────────────────────────────

/// Two-operator FM synthesizer (phase modulation). Three machine variants.
/// Topology: modulator output → scales carrier phase. Modulator has self-feedback.
/// Ports: events_in (0, Event), audio_out_l (1, Mono), audio_out_r (2, Mono).
pub struct FmEngine {
    machine:        FmMachine,
    bank:           ParameterBank,
    sample_rate:    f32,

    carrier_phase:   f32,
    modulator_phase: f32,
    prev_mod_out:    f32,

    pitch_env: AdState,
    mod_env:   AdState,
    amp_env:   AdState,

    current_hz: f32,
    active:     bool,
    node_id:    u32,
    /// Note of the last retrigger — used as the CMD_TRIGGER default note (arg0 < 0).
    last_note:  u8,
    /// Linear output-level multiplier derived from trigger velocity (0.0..=1.0).
    /// 1.0 = unity gain (full velocity, matches pre-W1 output level).
    velocity_level: f32,

    render_l: Vec<f32>,
    render_r: Vec<f32>,

    pending_initial_params: HashMap<String, f64>,

    ports: [PortDescriptor; 3],
}

impl FmEngine {
    pub const PORT_EVENTS_IN:   u32 = 0;
    pub const PORT_AUDIO_OUT_L: u32 = 1;
    pub const PORT_AUDIO_OUT_R: u32 = 2;

    pub fn new(machine: FmMachine) -> Self {
        let doc = Self::build_doc(machine);
        Self {
            machine,
            bank:           ParameterBank::from_capability_document(&doc),
            sample_rate:    44100.0,
            carrier_phase:   0.0,
            modulator_phase: 0.0,
            prev_mod_out:    0.0,
            pitch_env: AdState::new(),
            mod_env:   AdState::new(),
            amp_env:   AdState::new(),
            current_hz: 65.41, // C2
            active:     false,
            node_id:    0,
            last_note:  36, // C2 — matches current_hz's initial value
            velocity_level: 1.0,
            render_l:   Vec::new(),
            render_r:   Vec::new(),
            pending_initial_params: HashMap::new(),
            ports: [
                PortDescriptor { id: Self::PORT_EVENTS_IN,   name: "events_in".into(),   direction: PortDirection::Input,  port_type: PortType::Event },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_L, name: "audio_out_l".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_R, name: "audio_out_r".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
        }
    }

    pub fn kick() -> Self { Self::new(FmMachine::Kick) }
    pub fn bell() -> Self { Self::new(FmMachine::Bell) }
    pub fn bass() -> Self { Self::new(FmMachine::Bass) }

    fn build_doc(machine: FmMachine) -> CapabilityDocument {
        let (name, params) = match machine {
            FmMachine::Kick => ("FmKick", vec![
                ParamDescriptor { id: fp("tune"),     name: "tune".into(),     min: -24.0, max: 24.0, default: 0.0, stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: fp("punch"),    name: "punch".into(),    min: 0.0,   max: 1.0,  default: 0.7, stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("decay"),    name: "decay".into(),    min: 0.01,  max: 2.0,  default: 0.5, stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("feedback"), name: "feedback".into(), min: 0.0,   max: 1.0,  default: 0.2, stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("drive"),    name: "drive".into(),    min: 0.0,   max: 1.0,  default: 0.0, stepped: false, unit: ParamUnit::Generic,   display: None },
            ]),
            FmMachine::Bell => ("FmBell", vec![
                ParamDescriptor { id: fp("tune"),     name: "tune".into(),     min: -24.0, max: 24.0, default: 0.0,  stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: fp("ratio"),    name: "ratio".into(),    min: 0.5,   max: 8.0,  default: 3.5,  stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("index"),    name: "index".into(),    min: 0.0,   max: 8.0,  default: 2.0,  stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("decay"),    name: "decay".into(),    min: 0.05,  max: 8.0,  default: 2.0,  stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("feedback"), name: "feedback".into(), min: 0.0,   max: 0.5,  default: 0.1,  stepped: false, unit: ParamUnit::Generic,   display: None },
            ]),
            FmMachine::Bass => ("FmBass", vec![
                ParamDescriptor { id: fp("tune"),  name: "tune".into(),  min: -24.0, max: 24.0, default: 0.0,  stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: fp("ratio"), name: "ratio".into(), min: 0.5,   max: 4.0,  default: 1.0,  stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("index"), name: "index".into(), min: 0.0,   max: 8.0,  default: 2.0,  stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("attack"),name: "attack".into(),min: 0.001, max: 0.5,  default: 0.01, stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("decay"), name: "decay".into(), min: 0.05,  max: 4.0,  default: 0.5,  stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("drive"), name: "drive".into(), min: 0.0,   max: 1.0,  default: 0.0,  stepped: false, unit: ParamUnit::Generic,   display: None },
            ]),
        };
        CapabilityDocument {
            name, vendor: "Paraclete", version: (0, 6, 0),
            ports: vec![], params, extensions: vec!["paraclete.instrument"],
        }
    }

    fn retrigger(&mut self, note: u8, velocity: f32) {
        let tune = self.bank.get(fp("tune")) as f32;
        self.current_hz      = note_to_hz(note, tune);
        self.last_note        = note;
        self.velocity_level   = velocity.clamp(0.0, 1.0);
        self.carrier_phase   = 0.0;
        self.modulator_phase = 0.0;
        self.prev_mod_out    = 0.0;
        // Only Kick uses pitch_env for the pitch-drop chirp.
        // P7: re-enable pitch_env.trigger() for Bell/Bass when pitch-chirp
        // parameters are added to those machine variants.
        if self.machine == FmMachine::Kick {
            self.pitch_env.trigger();
        }
        self.mod_env.trigger();
        self.amp_env.trigger();
        self.active = true;
    }

    fn process_kick(&mut self, block_size: usize) {
        let punch    = self.bank.get(fp("punch"))    as f32;
        let decay_s  = self.bank.get(fp("decay"))    as f32;
        let feedback = self.bank.get(fp("feedback")) as f32;
        let drive    = self.bank.get(fp("drive"))    as f32;
        let sr = self.sample_rate;
        let tau = std::f32::consts::TAU;

        let pitch_attack_inc  = 1.0 / (0.002 * sr);
        let pitch_decay_coeff = 0.001f32.powf(1.0 / ((0.05 + punch * 0.1) * sr));
        let mod_attack_inc    = 1.0 / (0.001 * sr);
        let mod_decay_coeff   = 0.001f32.powf(1.0 / ((0.02 + punch * 0.05) * sr));
        let amp_attack_inc    = 1.0 / (0.001 * sr);
        let amp_decay_coeff   = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));

        for i in 0..block_size {
            let pitch_val  = self.pitch_env.tick(pitch_attack_inc, pitch_decay_coeff);
            let carrier_hz = self.current_hz * 2.0f32.powf(pitch_val * punch * 24.0 / 12.0);

            let mod_env_val = self.mod_env.tick(mod_attack_inc, mod_decay_coeff);
            let mod_hz = carrier_hz;
            self.modulator_phase = (self.modulator_phase + mod_hz / sr).fract();
            let fb_term = feedback * self.prev_mod_out;
            let m = (self.modulator_phase * tau + fb_term * tau).sin();
            self.prev_mod_out = m;
            let mod_out = m * mod_env_val;

            let index = 2.0 + punch * 4.0;
            self.carrier_phase = (self.carrier_phase + carrier_hz / sr + index * mod_out / tau).fract();
            let carrier_out = (self.carrier_phase * tau).sin();
            let amp = self.amp_env.tick(amp_attack_inc, amp_decay_coeff);
            let out = soft_clip(carrier_out * amp * (1.0 + drive * 9.0));
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }

    fn process_bell(&mut self, block_size: usize) {
        let ratio    = self.bank.get(fp("ratio"))    as f32;
        let index    = self.bank.get(fp("index"))    as f32;
        let decay_s  = self.bank.get(fp("decay"))    as f32;
        let feedback = self.bank.get(fp("feedback")) as f32;
        let sr = self.sample_rate;
        let tau = std::f32::consts::TAU;

        let decay_coeff = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let attack_inc  = 1.0 / (0.001 * sr);

        for i in 0..block_size {
            let mod_env_val = self.mod_env.tick(attack_inc, decay_coeff);
            let amp_val     = self.amp_env.tick(attack_inc, decay_coeff);

            let mod_hz = self.current_hz * ratio;
            self.modulator_phase = (self.modulator_phase + mod_hz / sr).fract();
            let fb_term = feedback * self.prev_mod_out;
            let mod_out = (self.modulator_phase * tau + fb_term * tau).sin();
            self.prev_mod_out = mod_out;

            self.carrier_phase = (self.carrier_phase + self.current_hz / sr
                + index * mod_out * mod_env_val / tau).fract();
            let out = (self.carrier_phase * tau).sin() * amp_val;
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }

    fn process_bass(&mut self, block_size: usize) {
        let ratio    = self.bank.get(fp("ratio"))    as f32;
        let index    = self.bank.get(fp("index"))    as f32;
        let attack_s = self.bank.get(fp("attack"))   as f32;
        let decay_s  = self.bank.get(fp("decay"))    as f32;
        let drive    = self.bank.get(fp("drive"))    as f32;
        let sr = self.sample_rate;
        let tau = std::f32::consts::TAU;

        let attack_inc      = 1.0 / (attack_s * sr).max(1.0);
        let decay_coeff     = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let mod_decay_coeff = 0.001f32.powf(1.0 / ((decay_s * 0.3) * sr).max(1.0));

        for i in 0..block_size {
            let mod_env_val = self.mod_env.tick(attack_inc, mod_decay_coeff);
            let amp_val     = self.amp_env.tick(attack_inc, decay_coeff);

            let mod_hz = self.current_hz * ratio;
            self.modulator_phase = (self.modulator_phase + mod_hz / sr).fract();
            let mod_out = (self.modulator_phase * tau).sin() * mod_env_val;

            self.carrier_phase = (self.carrier_phase + self.current_hz / sr
                + index * mod_out / tau).fract();
            let out = soft_clip((self.carrier_phase * tau).sin() * amp_val * (1.0 + drive * 9.0));
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }
}

impl Node for FmEngine {
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
        self.sample_rate    = sample_rate;
        let doc = Self::build_doc(self.machine);
        self.bank           = ParameterBank::from_capability_document(&doc);
        // BUG-008 fix: consume the pending map so a re-activate (dynamic
        // topology rebuild, P9 C4) cannot overwrite deserialized state.
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, value);
            }
        }
        self.render_l       = vec![0.0; block_size];
        self.render_r       = vec![0.0; block_size];
        self.carrier_phase   = 0.0;
        self.modulator_phase = 0.0;
        self.prev_mod_out    = 0.0;
        self.pitch_env  = AdState::new();
        self.mod_env    = AdState::new();
        self.amp_env    = AdState::new();
        self.current_hz = 65.41;
        self.active     = false;
        self.last_note  = 36;
        self.velocity_level = 1.0;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let block_size = input.block_size;
        for s in &mut self.render_l { *s = 0.0; }
        for s in &mut self.render_r { *s = 0.0; }

        // Handle NodeCommands: CMD_TRIGGER live-triggers a voice (same retrigger
        // path as NoteOn). arg0 = note (< 0 → last-triggered note); arg1 = velocity
        // 0.0..=1.0 (<= 0.0 → default 0.79).
        for cmd in input.commands {
            if cmd.type_id == CMD_TRIGGER {
                let note: u8 = if cmd.arg0 < 0 {
                    self.last_note
                } else {
                    cmd.arg0.clamp(0, 127) as u8
                };
                let velocity: f32 = if cmd.arg1 <= 0.0 {
                    0.79
                } else {
                    cmd.arg1.clamp(0.0, 1.0) as f32
                };
                self.retrigger(note, velocity);
            }
        }

        for timed in input.events {
            match timed.event {
                Event::ParamLock(ref pl) if pl.node_id == self.node_id => {
                    self.bank.handle_commands(&[paraclete_node_api::NodeCommand {
                        target_id: self.node_id,
                        type_id:   paraclete_node_api::CMD_SET_PARAM,
                        arg0:      pl.param_id as i64,
                        arg1:      pl.value as f64,
                    }]);
                }
                Event::Midi2(ref ump) => {
                    if let UmpMessage::ChannelVoice2(cv2) = ump {
                        if let ChannelVoice2::NoteOn(n) = cv2 {
                            let velocity = n.velocity() as f32 / 65535.0;
                            self.retrigger(u8::from(n.note_number()), velocity);
                        }
                    }
                }
                _ => {}
            }
        }

        if self.active {
            match self.machine {
                FmMachine::Kick => self.process_kick(block_size),
                FmMachine::Bell => self.process_bell(block_size),
                FmMachine::Bass => self.process_bass(block_size),
            }
        }

        if let Some(buf) = output.audio_outputs.first_mut() {
            if buf.channels() >= 2 {
                for (dst, &src) in buf.channel_mut(0).iter_mut().zip(self.render_l[..block_size].iter()) {
                    *dst = src * self.velocity_level;
                }
                for (dst, &src) in buf.channel_mut(1).iter_mut().zip(self.render_r[..block_size].iter()) {
                    *dst = src * self.velocity_level;
                }
            } else if buf.channels() == 1 {
                for (i, (&l, &r)) in self.render_l[..block_size].iter()
                    .zip(self.render_r[..block_size].iter()).enumerate()
                {
                    buf.channel_mut(0)[i] = (l + r) * 0.5 * self.velocity_level;
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
        NodeCommand, CMD_SET_PARAM, CMD_TRIGGER, ParamLockEvent, TimedEvent, TransportInfo,
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

    fn run_fm(eng: &mut FmEngine, events: &[TimedEvent]) -> Vec<f32> {
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

    fn run_fm_cmds(eng: &mut FmEngine, events: &[TimedEvent], cmds: &[NodeCommand]) -> Vec<f32> {
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

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|&x| x*x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn fm_kick_produces_audio_on_note_on() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        let out = run_fm(&mut eng, &[make_note_on(36)]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "FM kick should produce audio");
    }

    #[test]
    fn fm_kick_feedback_zero_vs_nonzero_timbre_differs() {
        let mut eng_nofb = FmEngine::kick();
        eng_nofb.activate(44100.0, 512);
        let cmds_nofb = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("feedback") as i64, arg1: 0.0 }];
        run_fm_cmds(&mut eng_nofb, &[], &cmds_nofb);
        let out_nofb = run_fm(&mut eng_nofb, &[make_note_on(36)]);

        let mut eng_fb = FmEngine::kick();
        eng_fb.activate(44100.0, 512);
        let cmds_fb = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("feedback") as i64, arg1: 0.8 }];
        run_fm_cmds(&mut eng_fb, &[], &cmds_fb);
        let out_fb = run_fm(&mut eng_fb, &[make_note_on(36)]);

        let differ = out_nofb.iter().zip(&out_fb).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "feedback=0 vs feedback=0.8 should produce different timbres");
    }

    #[test]
    fn fm_bell_decays_longer_than_kick_at_same_decay_param() {
        // Bell default decay = 2.0s; kick default decay = 0.5s.
        // After 8 blocks (≈93ms), bell should still have more energy.
        let mut bell = FmEngine::bell();
        bell.activate(44100.0, 512);
        let _ = run_fm(&mut bell, &[make_note_on(60)]);
        for _ in 0..8 { run_fm(&mut bell, &[]); }
        let bell_energy: f32 = run_fm(&mut bell, &[]).iter().map(|&x| x*x).sum();

        let mut kick = FmEngine::kick();
        kick.activate(44100.0, 512);
        let _ = run_fm(&mut kick, &[make_note_on(36)]);
        for _ in 0..8 { run_fm(&mut kick, &[]); }
        let kick_energy: f32 = run_fm(&mut kick, &[]).iter().map(|&x| x*x).sum();

        assert!(bell_energy > kick_energy,
            "bell should have more energy after 8 blocks: bell={bell_energy:.6} kick={kick_energy:.6}");
    }

    #[test]
    fn fm_bell_noninteger_ratio_differs_from_integer_ratio() {
        let mut eng_int = FmEngine::bell();
        eng_int.activate(44100.0, 512);
        let cmd_int = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("ratio") as i64, arg1: 2.0 }];
        run_fm_cmds(&mut eng_int, &[], &cmd_int);
        let out_int = run_fm(&mut eng_int, &[make_note_on(60)]);

        let mut eng_frac = FmEngine::bell();
        eng_frac.activate(44100.0, 512);
        let cmd_frac = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("ratio") as i64, arg1: 3.5 }];
        run_fm_cmds(&mut eng_frac, &[], &cmd_frac);
        let out_frac = run_fm(&mut eng_frac, &[make_note_on(60)]);

        let differ = out_int.iter().zip(&out_frac).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "ratio=2 vs ratio=3.5 should produce different spectra");
    }

    #[test]
    fn fm_bass_attack_param_changes_onset_slope() {
        // Short attack → louder at sample 5; long attack → quieter.
        let mut eng_fast = FmEngine::bass();
        eng_fast.activate(44100.0, 512);
        let cmds_fast = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("attack") as i64, arg1: 0.001 }];
        run_fm_cmds(&mut eng_fast, &[], &cmds_fast);
        let out_fast = run_fm(&mut eng_fast, &[make_note_on(36)]);

        let mut eng_slow = FmEngine::bass();
        eng_slow.activate(44100.0, 512);
        let cmds_slow = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("attack") as i64, arg1: 0.3 }];
        run_fm_cmds(&mut eng_slow, &[], &cmds_slow);
        let out_slow = run_fm(&mut eng_slow, &[make_note_on(36)]);

        // Fast attack → louder early onset
        assert!(out_fast[5].abs() > out_slow[5].abs(),
            "fast attack should be louder at onset: fast={:.4} slow={:.4}",
            out_fast[5].abs(), out_slow[5].abs());
    }

    #[test]
    fn fm_bass_drive_increases_rms_output() {
        let mut eng_no_drive = FmEngine::bass();
        eng_no_drive.activate(44100.0, 512);
        let out_no_drive = run_fm(&mut eng_no_drive, &[make_note_on(36)]);

        let mut eng_drive = FmEngine::bass();
        eng_drive.activate(44100.0, 512);
        let cmds = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("drive") as i64, arg1: 1.0 }];
        run_fm_cmds(&mut eng_drive, &[], &cmds);
        let out_drive = run_fm(&mut eng_drive, &[make_note_on(36)]);

        assert!(rms(&out_drive) > rms(&out_no_drive),
            "drive=1.0 should increase RMS: {:.4} vs {:.4}", rms(&out_drive), rms(&out_no_drive));
    }

    #[test]
    fn fm_param_lock_ratio_overrides_base_ratio() {
        let node_id = 55u32;
        let mut eng = FmEngine::bell();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        let out_base = run_fm(&mut eng, &[make_note_on(60)]);

        let lock_event = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: fp("ratio"), value: 7.0,
        }));
        let out_locked = run_fm(&mut eng, &[lock_event, make_note_on(60)]);

        let differ = out_base.iter().zip(&out_locked).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "param lock ratio=7.0 should change timbre vs base ratio=3.5");
    }

    #[test]
    fn fm_portability_check() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        assert!(!eng.ports().is_empty());
    }

    #[test]
    fn fm_engine_set_initial_params_applied() {
        let mut eng = FmEngine::bass();
        eng.set_node_id(1);
        eng.set_initial_params(&[("index".to_string(), 4.0)].into_iter().collect());
        eng.activate(44100.0, 256);
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        eng.published_state(&mut buf);
        let entry = buf.iter().find(|(k, _)| k.ends_with("/index"));
        assert!(entry.is_some(), "published_state should contain /index");
        if let paraclete_node_api::StateBusValue::Float(v) = entry.unwrap().1 {
            assert!((v - 4.0).abs() < 1e-9, "index should be 4.0, got {v}");
        } else {
            panic!("index entry should be Float");
        }
    }

    // ── W1 Commit 0: CMD_TRIGGER + velocity plumbing ─────────────────────────

    #[test]
    fn cmd_trigger_produces_audio() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        let cmd = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 1.0 };
        let out = run_fm_cmds(&mut eng, &[], &[cmd]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "CMD_TRIGGER should produce audio");
    }

    #[test]
    fn cmd_trigger_negative_note_uses_default() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        let cmd = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: -1, arg1: 1.0 };
        let out = run_fm_cmds(&mut eng, &[], &[cmd]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5),
            "CMD_TRIGGER with arg0<0 should use the default/last note and produce audio");
    }

    #[test]
    fn velocity_scales_output_level() {
        let mut eng_hi = FmEngine::kick();
        eng_hi.activate(44100.0, 512);
        let cmd_hi = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 1.0 };
        let out_hi = run_fm_cmds(&mut eng_hi, &[], &[cmd_hi]);
        let peak_hi = out_hi.iter().fold(0.0f32, |m, &x| m.max(x.abs()));

        let mut eng_lo = FmEngine::kick();
        eng_lo.activate(44100.0, 512);
        let cmd_lo = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 0.25 };
        let out_lo = run_fm_cmds(&mut eng_lo, &[], &[cmd_lo]);
        let peak_lo = out_lo.iter().fold(0.0f32, |m, &x| m.max(x.abs()));

        assert!(peak_hi > peak_lo,
            "higher velocity should produce a louder peak: hi={peak_hi:.4} lo={peak_lo:.4}");
        let ratio = peak_hi / peak_lo.max(1e-9);
        assert!(ratio > 2.0,
            "velocity ratio (1.0 vs 0.25) should roughly scale peak amplitude, got ratio={ratio:.2}");
    }
}
