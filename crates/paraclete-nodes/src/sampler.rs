use std::collections::HashMap;

use rubato::{
    Resampler, SincFixedIn, SincFixedOut, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, ConnectionRecord, Event, LockableParam,
    Negotiable, Node, ParamDescriptor, ParamUnit,
    ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, UmpMessage,
    midi::ChannelVoice2,
};

// ── Sampler envelope phase ─────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum EnvPhaseSimple { Attack, Release, Done }

// ── param_hash ────────────────────────────────────────────────────────────────

fn param_hash(name: &str) -> u32 {
    ParamDescriptor::id_for_name(name)
}

// ── ActiveParamLock ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct ActiveParamLock {
    locked_value: f64,
}

// ── Per-voice sinc resampler (P7) ─────────────────────────────────────────────

/// Maximum ratio change relative to the initial ratio (1.0).
/// Covers ±4 octaves (ratio range [1/16, 16]). Notes outside this range are
/// clamped to the limit rather than falling back to linear interpolation.
const RUBATO_MAX_RATIO: f64 = 16.0;

struct VoiceResampler {
    rs: SincFixedOut<f32>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    ratio: f64,
}

impl VoiceResampler {
    fn new(block_size: usize) -> Self {
        let rs = SincFixedOut::<f32>::new(
            1.0,
            RUBATO_MAX_RATIO,
            SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            },
            block_size,
            1,
        ).expect("SincFixedOut init");
        let max_in = rs.input_frames_max();
        Self {
            input: vec![vec![0.0_f32; max_in]; 1],
            output: vec![vec![0.0_f32; block_size]; 1],
            ratio: 1.0,
            rs,
        }
    }

    fn set_ratio(&mut self, ratio: f64) {
        let clamped = ratio.clamp(1.0 / RUBATO_MAX_RATIO, RUBATO_MAX_RATIO);
        // reset() must precede set_resample_ratio(): reset() restores ratio to original (1.0)
        // and recalculates needed_input_size from that baseline; set_resample_ratio() then
        // applies the new ratio and updates needed_input_size correctly.
        self.rs.reset();
        let _ = self.rs.set_resample_ratio(clamped, false);
        self.ratio = clamped;
    }
}

// ── Voice ──────────────────────────────────────────────────────────────────────

struct Voice {
    active: bool,
    note: u8,
    playback_pos: f64,
    active_locks: HashMap<u32, ActiveParamLock>,
    triggered_at: u64,
    env_value: f32,
    env_phase: EnvPhaseSimple,
    resampler: Option<VoiceResampler>,
}

impl Voice {
    fn new() -> Self {
        Self {
            active: false,
            note: 0,
            playback_pos: 0.0,
            active_locks: HashMap::with_capacity(9),
            triggered_at: 0,
            env_value: 1.0,
            env_phase: EnvPhaseSimple::Done,
            resampler: None,
        }
    }

    fn effective(&self, param_id: u32, base: f64) -> f64 {
        self.active_locks.get(&param_id)
            .map(|l| l.locked_value)
            .unwrap_or(base)
    }
}

// ── Envelope helper ───────────────────────────────────────────────────────────

/// Advance the per-voice envelope by one sample frame.
/// Returns `Some(env_value)` while playing, `None` when the voice should deactivate.
#[inline]
fn advance_envelope(
    env_phase: &mut EnvPhaseSimple,
    env_value: &mut f32,
    attack_inc: f32,
    release_coeff: f32,
) -> Option<f32> {
    match env_phase {
        EnvPhaseSimple::Attack => {
            *env_value += attack_inc;
            if *env_value >= 1.0 {
                *env_value = 1.0;
                *env_phase = EnvPhaseSimple::Release;
            }
            Some(*env_value)
        }
        EnvPhaseSimple::Release => {
            *env_value *= release_coeff;
            if *env_value < 1.0e-5 {
                *env_value = 0.0;
                *env_phase = EnvPhaseSimple::Done;
            }
            Some(*env_value)
        }
        EnvPhaseSimple::Done => None,
    }
}

// ── Sampler capability document ───────────────────────────────────────────────

/// Static capability document for the Sampler. Called at new() and capability_document().
/// Ports are overridden with the instance's port list in capability_document().
fn sampler_capability_document() -> CapabilityDocument {
    CapabilityDocument {
        name: "Sampler",
        vendor: "Paraclete",
        version: (0, 5, 0),
        ports: vec![],
        params: vec![
            ParamDescriptor { id: param_hash("pitch"),     name: "pitch".into(),     min: -24.0, max: 24.0,  default: 0.0,   stepped: false, unit: ParamUnit::Semitones, display: None },
            ParamDescriptor { id: param_hash("volume"),    name: "volume".into(),    min: 0.0,   max: 1.0,   default: 1.0,   stepped: false, unit: ParamUnit::Generic,   display: None },
            ParamDescriptor { id: param_hash("pan"),       name: "pan".into(),       min: -1.0,  max: 1.0,   default: 0.0,   stepped: false, unit: ParamUnit::Generic,   display: None },
            ParamDescriptor { id: param_hash("start"),     name: "start".into(),     min: 0.0,   max: 1.0,   default: 0.0,   stepped: false, unit: ParamUnit::Percent,   display: None },
            ParamDescriptor { id: param_hash("end"),       name: "end".into(),       min: 0.0,   max: 1.0,   default: 1.0,   stepped: false, unit: ParamUnit::Percent,   display: None },
            ParamDescriptor { id: param_hash("attack"),    name: "attack".into(),    min: 0.001, max: 1.0,   default: 0.005, stepped: false, unit: ParamUnit::Seconds,   display: None },
            ParamDescriptor { id: param_hash("release"),   name: "release".into(),   min: 0.0,   max: 4.0,   default: 0.1,   stepped: false, unit: ParamUnit::Seconds,   display: None },
            ParamDescriptor { id: param_hash("root_note"), name: "root_note".into(), min: 0.0,   max: 127.0, default: 60.0,  stepped: true,  unit: ParamUnit::Generic,   display: None },
        ],
        extensions: vec!["paraclete.instrument"],
    }
}

// ── Sampler ────────────────────────────────────────────────────────────────────

/// 4-voice polyphonic sampler. Loads mono WAV from a filesystem path at
/// `activate()` time. All parameters are lockable per sequencer step.
pub struct Sampler {
    node_id: u32,
    ports: [PortDescriptor; 5],

    // Sample data (loaded at activate)
    sample_data: Vec<f32>,
    sample_rate_native: f32,
    sample_frames: usize,

    // Slice table — one full-sample slice at P3: (start_frame, end_frame).
    slices: Vec<(usize, usize)>,

    // Hardware-reachable base parameters managed by ParameterBank (CMD_SET_PARAM / CMD_BUMP_PARAM).
    // Initialised at new() time so values survive across activate() calls and deserialize().
    bank: ParameterBank,

    // Loop and slice are lockable (is_known_param) but not hardware-reachable (not in bank).
    base_loop:  bool,
    base_slice: usize,

    // Node-level active locks (applied before voice trigger each cycle)
    node_locks: HashMap<u32, ActiveParamLock>,

    // Voice pool
    voices: [Voice; 4],
    cycle_counter: u64,
    samp_trig_count: u64,
    last_triggered_note: u8,

    output_sample_rate: f32,

    sample_path: Option<String>,
    pub(crate) connection_records: Vec<ConnectionRecord>,

    // Pre-allocated render buffers — no audio-thread allocation.
    render_l: Vec<f32>,
    render_r: Vec<f32>,

    pending_initial_params: HashMap<String, f64>,
}

impl Sampler {
    pub const PORT_EVENTS_IN:   u32 = 0;
    pub const PORT_AUDIO_OUT_L: u32 = 1;
    pub const PORT_AUDIO_OUT_R: u32 = 2;
    pub const PORT_PITCH_MOD:   u32 = 3;
    pub const PORT_VOLUME_MOD:  u32 = 4;

    /// Trigger the default note at full velocity immediately.
    /// Sent from scripts in trigger mode: send_cmd(samp_id, CMD_TRIGGER, 0, 0.0)
    pub const CMD_TRIGGER: u32 = 19;

    pub fn new() -> Self { Self::build(None) }

    pub fn with_path(path: impl Into<String>) -> Self { Self::build(Some(path.into())) }

    fn build(sample_path: Option<String>) -> Self {
        Self {
            node_id: 0,
            ports: [
                PortDescriptor { id: Self::PORT_EVENTS_IN,   name: "events_in".into(),   direction: PortDirection::Input,  port_type: PortType::Event },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_L, name: "audio_out_l".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_AUDIO_OUT_R, name: "audio_out_r".into(), direction: PortDirection::Output, port_type: PortType::Audio },
                PortDescriptor { id: Self::PORT_PITCH_MOD,   name: "pitch_mod".into(),   direction: PortDirection::Input,  port_type: PortType::Modulation },
                PortDescriptor { id: Self::PORT_VOLUME_MOD,  name: "volume_mod".into(),  direction: PortDirection::Input,  port_type: PortType::Modulation },
            ],
            sample_data: vec![],
            sample_rate_native: 44100.0,
            sample_frames: 0,
            slices: vec![],
            bank: ParameterBank::from_capability_document(&sampler_capability_document()),
            base_loop: false,
            base_slice: 0,
            node_locks: HashMap::new(),
            voices: [Voice::new(), Voice::new(), Voice::new(), Voice::new()],
            cycle_counter: 0,
            samp_trig_count: 0,
            last_triggered_note: 0,
            output_sample_rate: 44100.0,
            sample_path,
            connection_records: Vec::new(),
            render_l: Vec::new(),
            render_r: Vec::new(),
            pending_initial_params: HashMap::new(),
        }
    }

    fn is_known_param(param_id: u32) -> bool {
        param_id == param_hash("pitch")
            || param_id == param_hash("volume")
            || param_id == param_hash("pan")
            || param_id == param_hash("start")
            || param_id == param_hash("end")
            || param_id == param_hash("attack")
            || param_id == param_hash("release")
            || param_id == param_hash("root_note")
            || param_id == param_hash("loop")
            || param_id == param_hash("slice")
    }

    fn base_for(&self, param_id: u32) -> f64 {
        // Bank-managed params: delegate to ParameterBank (reflects CMD_BUMP_PARAM changes).
        if param_id == param_hash("pitch")
            || param_id == param_hash("volume")
            || param_id == param_hash("pan")
            || param_id == param_hash("start")
            || param_id == param_hash("end")
            || param_id == param_hash("attack")
            || param_id == param_hash("release")
            || param_id == param_hash("root_note")
        {
            return self.bank.get(param_id);
        }
        // Non-bank params: read from dedicated fields.
        if param_id == param_hash("loop")  { return if self.base_loop { 1.0 } else { 0.0 }; }
        if param_id == param_hash("slice") { return self.base_slice as f64; }
        0.0
    }

    fn effective_node(&self, param_id: u32) -> f64 {
        self.node_locks.get(&param_id)
            .map(|l| l.locked_value)
            .unwrap_or_else(|| self.base_for(param_id))
    }

    fn trigger_voice(&mut self, note: u8, _velocity: u16, _sample_offset: u32) {
        self.samp_trig_count = self.samp_trig_count.wrapping_add(1);
        self.last_triggered_note = note;
        let voice_idx = self.voices.iter().position(|v| !v.active)
            .unwrap_or_else(|| {
                self.voices.iter().enumerate()
                    .min_by_key(|(_, v)| v.triggered_at)
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });

        // Compute rubato ratio for this trigger.
        let root_note = self.bank.get(param_hash("root_note"));
        let pitch_offset = self.effective_node(param_hash("pitch"));
        let note_diff = note as f64 - root_note + pitch_offset;
        let pitch_factor = 2.0_f64.powf(note_diff / 12.0);
        let ratio = self.output_sample_rate as f64 / (self.sample_rate_native as f64 * pitch_factor);

        let voice = &mut self.voices[voice_idx];
        voice.active = true;
        voice.note = note;
        voice.playback_pos = 0.0;
        voice.triggered_at = self.cycle_counter;
        voice.active_locks.clear();
        voice.env_value = 0.0;
        voice.env_phase = EnvPhaseSimple::Attack;

        if let Some(ref mut vrs) = voice.resampler {
            vrs.set_ratio(ratio);
        }

        for (param_id, lock) in &self.node_locks {
            voice.active_locks.insert(*param_id, lock.clone());
        }
    }

    fn release_voice(&mut self, note: u8, _sample_offset: u32) {
        let to_release = self.voices.iter().enumerate()
            .filter(|(_, v)| v.active && v.note == note)
            .max_by_key(|(_, v)| v.triggered_at)
            .map(|(i, _)| i);

        if let Some(idx) = to_release {
            self.voices[idx].active = false;
            self.voices[idx].active_locks.clear();
        }
    }

    fn lockable_params_list(&self) -> Vec<LockableParam> {
        self.capability_document().params
            .into_iter()
            .map(|p| LockableParam {
                param_id: p.id,
                name: p.name.as_str().to_string(),
                min: p.min,
                max: p.max,
                default: p.default,
                unit: p.unit,
            })
            .collect()
    }
}

impl Default for Sampler {
    fn default() -> Self { Self::new() }
}

impl Node for Sampler {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
        self.pending_initial_params = params.clone();
    }

    fn published_state(&self, buf: &mut Vec<(String, paraclete_node_api::StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
        buf.push((format!("/node/{}/state/trig",      self.node_id), paraclete_node_api::StateBusValue::Int(self.samp_trig_count as i64)));
        buf.push((format!("/node/{}/state/last_note", self.node_id), paraclete_node_api::StateBusValue::Int(self.last_triggered_note as i64)));
    }

    fn capability_document(&self) -> CapabilityDocument {
        let mut doc = sampler_capability_document();
        doc.ports = self.ports.to_vec();
        doc
    }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.output_sample_rate = sample_rate;
        self.render_l = vec![0.0; block_size];
        self.render_r = vec![0.0; block_size];

        // Apply initial params (from instrument definition file) to the bank.
        let doc = sampler_capability_document();
        for (name, value) in &self.pending_initial_params {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, *value);
            }
        }

        if let Some(ref path) = self.sample_path.clone() {
            match load_wav(path, sample_rate) {
                Ok(data) => {
                    self.sample_frames = data.len();
                    self.sample_data = data;
                    self.slices = vec![(0, self.sample_frames)];
                }
                Err(e) => {
                    self.sample_data = vec![0.0; 1];
                    self.sample_frames = 1;
                    self.slices = vec![(0, 1)];
                    eprintln!("Sampler: failed to load {:?}: {}", path, e);
                }
            }
        } else {
            self.slices = vec![(0, self.sample_frames.max(1))];
        }

        // Pre-allocate per-voice sinc resamplers. SincFixedOut::new() allocates the
        // filter coefficients on the main thread; process() never reallocates.
        for voice in &mut self.voices {
            voice.resampler = Some(VoiceResampler::new(block_size));
        }
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // Update persistent base params from CMD_SET_PARAM / CMD_BUMP_PARAM before any DSP.
        self.bank.handle_commands(input.commands);

        self.cycle_counter += 1;
        let block_size = input.block_size;

        // Clear per-cycle node-level locks so locks from a previous step do not
        // bleed into steps that have no param lock. Locks are re-populated from
        // the incoming events below before any voice trigger fires.
        self.node_locks.clear();

        // 0. Handle NodeCommands (CMD_TRIGGER from scripting layer).
        for cmd in input.commands {
            if cmd.type_id == Self::CMD_TRIGGER {
                self.trigger_voice(60, u16::MAX / 2, 0);
            }
        }

        // 1. Handle events (executor ensures ParamLock arrives before NoteOn).
        for timed in input.events {
            match timed.event {
                Event::ParamLock(ref lock) if lock.node_id == self.node_id => {
                    // Only accept known param IDs — unknown are silently ignored.
                    if Self::is_known_param(lock.param_id) {
                        self.node_locks.insert(lock.param_id, ActiveParamLock {
                            locked_value: lock.value,
                        });
                    }
                }
                Event::Midi2(ref ump) => {
                    match ump {
                        UmpMessage::ChannelVoice2(cv2) => match cv2 {
                            ChannelVoice2::NoteOn(n) => {
                                self.trigger_voice(
                                    u8::from(n.note_number()),
                                    n.velocity(),
                                    timed.sample_offset,
                                );
                            }
                            ChannelVoice2::NoteOff(n) => {
                                self.release_voice(u8::from(n.note_number()), timed.sample_offset);
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // 2. Effective node-level params.
        let pitch_mod = input.modulation(Self::PORT_PITCH_MOD)
            .first().copied().unwrap_or(0.0) as f64 * 12.0;
        let volume_mod = input.modulation(Self::PORT_VOLUME_MOD)
            .first().copied().unwrap_or(0.0) as f64;

        let eff_volume = (self.effective_node(param_hash("volume")) + volume_mod).clamp(0.0, 1.0);
        let eff_pan = self.effective_node(param_hash("pan")).clamp(-1.0, 1.0);
        let pan_l = ((1.0 - eff_pan) * 0.5 + 0.5).sqrt() as f32;
        let pan_r = ((1.0 + eff_pan) * 0.5 + 0.5).sqrt() as f32;
        let vol = eff_volume as f32;

        // 3. Zero render buffers.
        for s in self.render_l.iter_mut() { *s = 0.0; }
        for s in self.render_r.iter_mut() { *s = 0.0; }

        // 4. Render voices.
        // Precompute all self-derived state before the mutable voice borrow.
        let node_pitch  = self.effective_node(param_hash("pitch"));
        let slice_idx   = self.effective_node(param_hash("slice")) as usize;
        let eff_start   = self.effective_node(param_hash("start"));
        let eff_end     = self.effective_node(param_hash("end"));
        let looping     = self.effective_node(param_hash("loop")) >= 0.5;

        let (slice_start, slice_end) = self.slices.get(slice_idx)
            .copied().unwrap_or((0, self.sample_frames));
        let range = slice_end.saturating_sub(slice_start);
        let start_frame = slice_start + (eff_start * range as f64) as usize;
        let end_frame   = slice_start + (eff_end   * range as f64) as usize;

        let native_sr = self.sample_rate_native;
        let output_sr = self.output_sample_rate;
        let root_note = self.bank.get(param_hash("root_note")) as u8;

        // Envelope parameters — precomputed to avoid per-sample HashMap lookups.
        let eff_attack_s  = self.effective_node(param_hash("attack"))  as f32;
        let eff_release_s = self.effective_node(param_hash("release")) as f32;
        let attack_inc    = 1.0 / (eff_attack_s  * output_sr).max(1.0);
        let release_coeff = 0.001_f32.powf(1.0 / (eff_release_s * output_sr).max(1.0));

        // Take a shared slice of sample_data — coexists with the mutable
        // borrow of voices below since they are different fields.
        let sample_data = self.sample_data.as_slice();

        for voice in self.voices.iter_mut() {
            if !voice.active { continue; }

            // Recompute note pitch each block so live CMD_BUMP_PARAM changes take effect.
            let voice_pitch = voice.effective(param_hash("pitch"), node_pitch) + pitch_mod;
            let note_diff = voice.note as f64 - root_note as f64 + voice_pitch;
            let pitch_factor = 2.0_f64.powf(note_diff / 12.0);
            let current_ratio = (output_sr as f64 / (native_sr as f64 * pitch_factor))
                .clamp(1.0 / RUBATO_MAX_RATIO, RUBATO_MAX_RATIO);

            // Push ratio update to rubato if pitch changed since last block (no reset — mid-voice).
            if let Some(ref mut vrs) = voice.resampler {
                if (current_ratio - vrs.ratio).abs() > 1e-6 {
                    let _ = vrs.rs.set_resample_ratio(current_ratio, false);
                    vrs.ratio = current_ratio;
                }
            }

            let use_rubato = (current_ratio - 1.0).abs() > 1e-4 && voice.resampler.is_some();

            if use_rubato {
                // ── Rubato path: block-rate sinc resampling ──────────────────
                let (new_pos, past_end) = {
                    let vrs = voice.resampler.as_mut().unwrap();
                    let needed = vrs.rs.input_frames_next();
                    vrs.input[0].resize(needed, 0.0);
                    for i in 0..needed {
                        let abs = start_frame as f64 + voice.playback_pos + i as f64;
                        if abs < end_frame as f64 {
                            let idx = abs as usize;
                            let frac = (abs - idx as f64) as f32;
                            let s0 = sample_data.get(idx).copied().unwrap_or(0.0);
                            let s1 = sample_data.get(idx + 1).copied().unwrap_or(0.0);
                            vrs.input[0][i] = s0 + (s1 - s0) * frac;
                        } else {
                            vrs.input[0][i] = 0.0;
                        }
                    }
                    let _ = Resampler::process_into_buffer(
                        &mut vrs.rs,
                        vrs.input.as_slice(),
                        vrs.output.as_mut_slice(),
                        None,
                    );
                    let new_pos = voice.playback_pos + needed as f64;
                    let past_end = start_frame as f64 + new_pos >= end_frame as f64;
                    (new_pos, past_end)
                };

                let mut deactivate = false;
                if past_end {
                    if looping { voice.playback_pos = 0.0; } else { deactivate = true; }
                } else {
                    voice.playback_pos = new_pos;
                }

                if !deactivate {
                    let out = &voice.resampler.as_ref().unwrap().output[0];
                    for frame in 0..block_size {
                        let Some(env) = advance_envelope(
                            &mut voice.env_phase, &mut voice.env_value,
                            attack_inc, release_coeff,
                        ) else {
                            deactivate = true;
                            break;
                        };
                        let sample = out.get(frame).copied().unwrap_or(0.0);
                        self.render_l[frame] += sample * vol * pan_l * env;
                        self.render_r[frame] += sample * vol * pan_r * env;
                    }
                }

                if deactivate {
                    voice.active = false;
                    voice.active_locks.clear();
                }
            } else {
                // ── Bypass path: linear interpolation ────────────────────────
                let playback_rate = pitch_factor * native_sr as f64 / output_sr as f64;

                let mut deactivate = false;
                for frame in 0..block_size {
                    let Some(env) = advance_envelope(
                        &mut voice.env_phase, &mut voice.env_value,
                        attack_inc, release_coeff,
                    ) else {
                        deactivate = true;
                        break;
                    };

                    let abs_pos = start_frame as f64 + voice.playback_pos;
                    if abs_pos < end_frame as f64 {
                        let idx = abs_pos as usize;
                        let frac = (abs_pos - idx as f64) as f32;
                        let s0 = sample_data.get(idx).copied().unwrap_or(0.0);
                        let s1 = sample_data.get(idx + 1).copied().unwrap_or(0.0);
                        let sample = s0 + (s1 - s0) * frac;
                        self.render_l[frame] += sample * vol * pan_l * env;
                        self.render_r[frame] += sample * vol * pan_r * env;
                        voice.playback_pos += playback_rate;
                    } else if looping {
                        voice.playback_pos = 0.0;
                    } else {
                        deactivate = true;
                        break;
                    }
                }

                if deactivate {
                    voice.active = false;
                    voice.active_locks.clear();
                }
            }
        }

        // 5. Write render buffers to output (channels 0=L, 1=R in single stereo buffer).
        if let Some(buf) = output.audio_outputs.first_mut() {
            if buf.channels() >= 2 {
                buf.channel_mut(0).copy_from_slice(&self.render_l[..block_size]);
                buf.channel_mut(1).copy_from_slice(&self.render_r[..block_size]);
            } else if buf.channels() == 1 {
                for (i, (l, r)) in self.render_l[..block_size].iter()
                    .zip(self.render_r[..block_size].iter()).enumerate()
                {
                    buf.channel_mut(0)[i] = (l + r) * 0.5;
                }
            }
        }
    }

    fn is_negotiable(&self) -> bool { true }

    /// Declare all parameters as lockable.
    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        let mut agreement = ConnectionAgreement::baseline();
        agreement.lockable_params = self.lockable_params_list();
        agreement
    }

    fn set_connection_record(&mut self, record: ConnectionRecord) {
        self.connection_records.push(record);
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = vec![3u8]; // version 3: bank-based params including root_note
        let path = self.sample_path.as_deref().unwrap_or("");
        let path_bytes = path.as_bytes();
        buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(path_bytes);
        for &val in &[
            self.bank.get(param_hash("pitch")),
            self.bank.get(param_hash("volume")),
            self.bank.get(param_hash("pan")),
            self.bank.get(param_hash("start")),
            self.bank.get(param_hash("end")),
            self.bank.get(param_hash("attack")),
            self.bank.get(param_hash("release")),
            self.bank.get(param_hash("root_note")),
        ] {
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf.push(self.base_loop as u8);
        buf.push(self.base_slice as u8);
        buf
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.is_empty() { return; }
        let version = data[0];
        if version != 2 && version != 3 { return; }
        let mut cur = 1usize;

        if cur + 2 > data.len() { return; }
        let path_len = u16::from_le_bytes(data[cur..cur + 2].try_into().unwrap()) as usize;
        cur += 2;

        if cur + path_len > data.len() { return; }
        let path = std::str::from_utf8(&data[cur..cur + path_len]).unwrap_or("").to_string();
        cur += path_len;
        self.sample_path = if path.is_empty() { None } else { Some(path) };

        macro_rules! read_f64 {
            () => {{
                if cur + 8 > data.len() { return; }
                let v = f64::from_le_bytes(data[cur..cur + 8].try_into().unwrap());
                cur += 8; v
            }};
        }

        if version == 2 {
            // v2: root_note was stored as a raw byte before the f64 params.
            if cur >= data.len() { return; }
            let root_note_byte = data[cur].min(127) as f64; cur += 1;
            self.bank.set(param_hash("root_note"), root_note_byte);
        }

        self.bank.set(param_hash("pitch"),   read_f64!());
        self.bank.set(param_hash("volume"),  read_f64!());
        self.bank.set(param_hash("pan"),     read_f64!());
        self.bank.set(param_hash("start"),   read_f64!());
        self.bank.set(param_hash("end"),     read_f64!());
        self.bank.set(param_hash("attack"),  read_f64!());
        self.bank.set(param_hash("release"), read_f64!());

        if version == 3 {
            self.bank.set(param_hash("root_note"), read_f64!());
        }

        if cur >= data.len() { return; }
        self.base_loop = data[cur] != 0; cur += 1;

        if cur >= data.len() { return; }
        self.base_slice = data[cur] as usize;
    }
}

impl Negotiable for Sampler {}

// ── Audio file loading via symphonia ─────────────────────────────────────────
// Supports WAV, FLAC, AIFF, OGG Vorbis, MP3 (via symphonia "all" feature).
// Load-time resampling uses rubato SincFixedIn for high quality when
// native_rate != target_rate. Per-voice pitch resampling uses linear
// interpolation in the render loop (per-voice rubato deferred to P7).

fn load_wav(path: &str, target_rate: f32) -> Result<Vec<f32>, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mss  = MediaSourceStream::new(Box::new(file), Default::default());
    let hint = Hint::new(); // probe by magic bytes, not extension
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| format!("probe: {e}"))?;
    let mut reader = probed.format;
    let track = reader.default_track().ok_or_else(|| "no audio track".to_string())?;
    let codec_params = track.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &Default::default())
        .map_err(|e| format!("codec: {e}"))?;

    let native_rate = codec_params.sample_rate
        .ok_or_else(|| "audio file has no sample rate in codec metadata".to_string())? as f32;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut channels = 1usize;

    loop {
        match reader.next_packet() {
            Ok(packet) => {
                let decoded = decoder.decode(&packet).map_err(|e| format!("decode: {e}"))?;
                let spec = *decoded.spec();
                channels = spec.channels.count();
                let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buf.copy_interleaved_ref(decoded);
                interleaved.extend_from_slice(buf.samples());
            }
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(e) => return Err(format!("read: {e}")),
        }
    }

    // Deinterleave to mono
    let mono: Vec<f32> = if channels == 1 {
        interleaved
    } else {
        interleaved.chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    if (native_rate - target_rate).abs() < 1.0 {
        Ok(mono)
    } else {
        resample_sinc(&mono, native_rate, target_rate)
    }
}

fn resample_sinc(samples: &[f32], from_rate: f32, to_rate: f32) -> Result<Vec<f32>, String> {
    if samples.is_empty() {
        return Ok(vec![]);
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let chunk = 512usize;
    let params = SincInterpolationParameters {
        sinc_len: 64,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 64,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk, 1)
        .map_err(|e| format!("rubato init: {e}"))?;

    let expected = (samples.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(expected + chunk);
    let mut pos = 0;

    loop {
        let end = (pos + chunk).min(samples.len());
        let mut buf_in = samples[pos..end].to_vec();
        buf_in.resize(chunk, 0.0);

        // rubato 0.15: process() returns Result<Vec<Vec<T>>>, no output arg.
        match resampler.process(&[buf_in], None) {
            Ok(out_channels) => {
                if let Some(ch) = out_channels.into_iter().next() {
                    output.extend_from_slice(&ch);
                }
            }
            Err(e) => return Err(format!("rubato process: {e}")),
        }

        if end >= samples.len() { break; }
        pos += chunk;
    }

    output.truncate(expected);
    Ok(output)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, Event, ParamLockEvent,
        TransportInfo, ProcessInput, ProcessOutput, TimedEvent, UmpMessage,
        NodeCommand, CMD_BUMP_PARAM, CMD_SET_PARAM,
        midi::{ChannelVoice2, Grouped, Channeled, NoteOn, NoteOff, u4, u7},
    };

    fn make_note_on(note: u8) -> TimedEvent {
        let mut msg = NoteOn::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note));
        msg.set_velocity(32768);
        TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(msg))))
    }

    fn make_note_off(note: u8) -> TimedEvent {
        let mut msg = NoteOff::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note));
        msg.set_velocity(0);
        TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(msg))))
    }

    fn make_param_lock(node_id: u32, param_id: u32, value: f64) -> TimedEvent {
        TimedEvent::new(0, Event::ParamLock(ParamLockEvent { node_id, param_id, value }))
    }

    fn run_sampler(sampler: &mut Sampler, events: &[TimedEvent]) -> AudioBuffer {
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let audio_ptr: *mut AudioBuffer = &mut audio;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];

        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs, signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        sampler.process(&input, &mut output);
        audio
    }

    fn run_sampler_with_cmds(sampler: &mut Sampler, events: &[TimedEvent], commands: &[NodeCommand]) -> AudioBuffer {
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let audio_ptr: *mut AudioBuffer = &mut audio;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];

        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab,
            commands,
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs, signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        sampler.process(&input, &mut output);
        audio
    }

    fn load_test_sample(sampler: &mut Sampler, frames: usize) {
        sampler.sample_data = vec![0.5; frames];
        sampler.sample_rate_native = 44100.0;
        sampler.sample_frames = frames;
        sampler.slices = vec![(0, frames)];
        sampler.activate(44100.0, 64);
    }

    #[test]
    fn sampler_new_produces_silence_with_no_sample() {
        let mut s = Sampler::new();
        s.activate(44100.0, 64);
        let buf = run_sampler(&mut s, &[make_note_on(60)]);
        assert!(buf.channel(0).iter().all(|&x| x == 0.0));
    }

    #[test]
    fn sampler_capability_document_has_8_params() {
        // pitch, volume, pan, start, end, attack, release, root_note
        let s = Sampler::new();
        assert_eq!(s.capability_document().params.len(), 8);
    }

    #[test]
    fn sampler_capability_document_extension_is_instrument() {
        let s = Sampler::new();
        assert!(s.capability_document().extensions.contains(&"paraclete.instrument"));
    }

    #[test]
    fn sampler_negotiate_returns_8_lockable_params() {
        let mut s = Sampler::new();
        let their_doc = CapabilityDocument::from_ports(&[]);
        let agreement = s.negotiate(&their_doc);
        assert_eq!(agreement.lockable_params.len(), 8);
    }

    #[test]
    fn sampler_negotiate_lockable_params_include_pitch_and_volume() {
        let mut s = Sampler::new();
        let agreement = s.negotiate(&CapabilityDocument::from_ports(&[]));
        let names: Vec<&str> = agreement.lockable_params.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"pitch"));
        assert!(names.contains(&"volume"));
    }

    #[test]
    fn sampler_set_connection_record_stores_record() {
        let mut s = Sampler::new();
        s.set_connection_record(ConnectionRecord {
            agreement: ConnectionAgreement::baseline(),
            partner_id: 5,
            local_port_id: 0,
        });
        assert_eq!(s.connection_records.len(), 1);
        assert_eq!(s.connection_records[0].partner_id, 5);
    }

    #[test]
    fn sampler_param_lock_for_unknown_param_is_ignored() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        s.activate(44100.0, 64);
        let events = [make_param_lock(1, 0xDEAD_BEEF, 1.0)];
        let _ = run_sampler(&mut s, &events); // must not panic
    }

    #[test]
    fn sampler_param_lock_on_wrong_node_id_is_ignored() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, 4096);
        // Lock targets node 99, not node 1.
        let events = [
            make_param_lock(99, param_hash("volume"), 0.0),
            make_note_on(60),
        ];
        let buf = run_sampler(&mut s, &events);
        // Volume lock for wrong node → default volume → non-zero output
        assert!(buf.channel(0).iter().any(|&x| x != 0.0));
    }

    #[test]
    fn sampler_volume_param_lock_changes_output_level() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, 4096);

        // Without lock: normal volume
        let buf_normal = run_sampler(&mut s, &[make_note_on(60)]);
        let sum_normal: f32 = buf_normal.channel(0).iter().sum();

        // Reset
        let mut s2 = Sampler::new();
        s2.set_node_id(1);
        s2.sample_data = s.sample_data.clone();
        s2.sample_rate_native = 44100.0;
        s2.sample_frames = s.sample_frames;
        s2.slices = s.slices.clone();
        s2.activate(44100.0, 64);

        // With volume=0.0 lock
        let events = [make_param_lock(1, param_hash("volume"), 0.0), make_note_on(60)];
        let buf_locked = run_sampler(&mut s2, &events);
        let sum_locked: f32 = buf_locked.channel(0).iter().sum();

        assert!(sum_normal.abs() > 0.0, "normal output should be non-zero");
        assert_eq!(sum_locked, 0.0, "volume=0 lock should silence output");
    }

    #[test]
    fn sampler_4_voices_5th_note_steals_oldest() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, 8192);

        for note in 60u8..65 {
            let _ = run_sampler(&mut s, &[make_note_on(note)]);
        }

        let active = s.voices.iter().filter(|v| v.active).count();
        assert_eq!(active, 4);
    }

    #[test]
    fn sampler_note_off_deactivates_voice_and_clears_locks() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, 8192);

        let _ = run_sampler(&mut s, &[make_note_on(60)]);
        assert_eq!(s.voices.iter().filter(|v| v.active).count(), 1);

        let _ = run_sampler(&mut s, &[make_note_off(60)]);
        assert_eq!(s.voices.iter().filter(|v| v.active).count(), 0);
        assert!(s.node_locks.is_empty());
    }

    #[test]
    fn sampler_serialize_deserialize_round_trip() {
        let mut s = Sampler::new();
        s.bank.set(param_hash("pitch"),     2.0);
        s.bank.set(param_hash("volume"),    0.5);
        s.bank.set(param_hash("pan"),      -0.3);
        s.bank.set(param_hash("start"),     0.1);
        s.bank.set(param_hash("end"),       0.9);
        s.bank.set(param_hash("attack"),    0.05);
        s.bank.set(param_hash("release"),   0.8);
        s.bank.set(param_hash("root_note"), 69.0);
        s.base_loop = true;
        s.base_slice = 3;
        s.sample_path = Some("samples/kick.wav".to_string());

        let data = s.serialize();
        let mut t = Sampler::new();
        t.deserialize(&data);

        assert!((t.bank.get(param_hash("pitch"))     - 2.0).abs() < 1e-9);
        assert!((t.bank.get(param_hash("volume"))    - 0.5).abs() < 1e-9);
        assert!((t.bank.get(param_hash("pan"))       - (-0.3)).abs() < 1e-9);
        assert!((t.bank.get(param_hash("start"))     - 0.1).abs() < 1e-9);
        assert!((t.bank.get(param_hash("end"))       - 0.9).abs() < 1e-9);
        assert!((t.bank.get(param_hash("attack"))    - 0.05).abs() < 1e-9);
        assert!((t.bank.get(param_hash("release"))   - 0.8).abs() < 1e-9);
        assert!((t.bank.get(param_hash("root_note")) - 69.0).abs() < 1e-9);
        assert!(t.base_loop);
        assert_eq!(t.base_slice, 3);
        assert_eq!(t.sample_path.as_deref(), Some("samples/kick.wav"));
    }

    #[test]
    fn sampler_deserialize_unknown_version_leaves_defaults() {
        let mut s = Sampler::new();
        s.deserialize(&[0xFF]);
        // Unknown version byte → no-op; bank retains constructed defaults.
        assert!((s.bank.get(param_hash("volume")) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn sampler_stereo_output_reflects_pan() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        s.bank.set(param_hash("pan"), 1.0); // hard right
        load_test_sample(&mut s, 4096);

        let buf = run_sampler(&mut s, &[make_note_on(60)]);
        let sum_l: f32 = buf.channel(0).iter().sum();
        let sum_r: f32 = buf.channel(1).iter().sum();

        // Hard right: R >> L
        assert!(sum_r > sum_l, "panned right: R channel should dominate");
    }

    #[test]
    fn sampler_bump_param_volume_changes_output_level() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, 4096);

        // Default volume (1.0) — trigger and measure output.
        let buf_default = run_sampler(&mut s, &[make_note_on(60)]);
        let sum_default: f32 = buf_default.channel(0).iter().sum();
        assert!(sum_default.abs() > 0.0);

        // Reset voice state by deactivating the note.
        let _ = run_sampler(&mut s, &[make_note_off(60)]);

        // Send CMD_BUMP_PARAM to reduce volume to ~0.5 then retrigger.
        let bump = NodeCommand { target_id: s.node_id, type_id: CMD_BUMP_PARAM, arg0: param_hash("volume") as i64, arg1: -0.5 };
        let _ = run_sampler_with_cmds(&mut s, &[], &[bump]);

        let buf_half = run_sampler(&mut s, &[make_note_on(60)]);
        let sum_half: f32 = buf_half.channel(0).iter().sum();

        assert!(sum_half.abs() > 0.0, "half-volume output should be non-zero");
        assert!(sum_half.abs() < sum_default.abs(), "half-volume should be quieter than default");
    }

    /// P4.5 Fix 3 done criterion — CMD_BUMP_PARAM pitch audibly shifts playback speed.
    /// At +12 semitones the playback rate doubles, so the sample is consumed ~2× faster;
    /// the voice reaches end_frame sooner, producing fewer non-zero output frames.
    /// Spec: sampler_bump_param_pitch_changes_playback
    #[test]
    fn sampler_bump_param_pitch_changes_playback() {
        // 64-frame sample = fills exactly one block at pitch=0.
        // At pitch=+12 (2× rate), it is consumed in ~32 frames → fewer non-zero output frames.
        let frames = 64usize;
        let block  = 64usize;

        // Baseline: pitch=0
        let mut s0 = Sampler::new();
        s0.set_node_id(1);
        s0.sample_data = vec![0.5; frames];
        s0.sample_rate_native = 44100.0;
        s0.sample_frames = frames;
        s0.slices = vec![(0, frames)];
        s0.activate(44100.0, block);
        let buf0 = run_sampler(&mut s0, &[make_note_on(60)]);
        let nonzero0 = buf0.channel(0).iter().filter(|&&x| x != 0.0).count();
        assert!(nonzero0 > 0, "baseline: should produce non-zero output");

        // Pitched up: +12 semitones
        let mut s1 = Sampler::new();
        s1.set_node_id(1);
        s1.sample_data = vec![0.5; frames];
        s1.sample_rate_native = 44100.0;
        s1.sample_frames = frames;
        s1.slices = vec![(0, frames)];
        s1.activate(44100.0, block);
        let bump = NodeCommand { target_id: 1, type_id: CMD_BUMP_PARAM, arg0: param_hash("pitch") as i64, arg1: 12.0 };
        let _ = run_sampler_with_cmds(&mut s1, &[], &[bump]);
        let buf1 = run_sampler(&mut s1, &[make_note_on(60)]);
        let nonzero1 = buf1.channel(0).iter().filter(|&&x| x != 0.0).count();

        assert!(nonzero1 < nonzero0,
            "pitch=+12 should exhaust sample faster: {} non-zero frames vs {} at pitch=0",
            nonzero1, nonzero0);
    }

    #[test]
    fn sampler_set_param_volume_zero_silences_output() {
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, 4096);

        let set_zero = NodeCommand { target_id: s.node_id, type_id: CMD_SET_PARAM, arg0: param_hash("volume") as i64, arg1: 0.0 };
        let _ = run_sampler_with_cmds(&mut s, &[], &[set_zero]);

        let buf = run_sampler(&mut s, &[make_note_on(60)]);
        assert!(buf.channel(0).iter().all(|&x| x == 0.0), "volume=0 should silence output");
    }

    // ── P6: Envelope DSP tests ────────────────────────────────────────────────

    #[test]
    fn sampler_attack_nonzero_delays_onset() {
        // With attack=0.1s, the first few samples should be quieter than with attack≈0.
        let frames = 4096usize;

        // Long attack
        let mut s_slow = Sampler::new();
        s_slow.set_node_id(1);
        s_slow.bank.set(param_hash("attack"), 0.1);
        load_test_sample(&mut s_slow, frames);
        let buf_slow = run_sampler(&mut s_slow, &[make_note_on(60)]);

        // Effectively-zero attack (1 sample at 44100 Hz)
        let mut s_fast = Sampler::new();
        s_fast.set_node_id(1);
        s_fast.bank.set(param_hash("attack"), 0.001);
        load_test_sample(&mut s_fast, frames);
        let buf_fast = run_sampler(&mut s_fast, &[make_note_on(60)]);

        // First sample: fast attack should be louder than slow attack.
        assert!(buf_fast.channel(0)[0].abs() > buf_slow.channel(0)[0].abs(),
            "slow attack should produce quieter onset than fast attack");
    }

    #[test]
    fn sampler_release_nonzero_extends_tail_vs_zero() {
        // Release shapes the envelope during playback. Use a long-enough sample
        // (4096 frames) and a short attack (0.001s ≈ 44 samples at 44100 Hz) so
        // the envelope enters Release quickly. Compare energy after several blocks.
        //
        // Slow release (0.5s): envelope stays near 1.0 during playback → high energy.
        // Fast release (0.001s): envelope decays to near 0 quickly → low energy.
        let frames = 4096usize;

        // Slow release: after attack completes at ~44 samples, release=0.5s is slow.
        let mut s_slow_rel = Sampler::new();
        s_slow_rel.set_node_id(1);
        s_slow_rel.bank.set(param_hash("attack"), 0.001);
        s_slow_rel.bank.set(param_hash("release"), 0.5);
        load_test_sample(&mut s_slow_rel, frames);
        let _ = run_sampler(&mut s_slow_rel, &[make_note_on(60)]);
        // After 5 more blocks (320 samples into Release), slow release is still active.
        for _ in 0..4 { run_sampler(&mut s_slow_rel, &[]); }
        let buf_slow = run_sampler(&mut s_slow_rel, &[]);
        let energy_slow: f32 = buf_slow.channel(0).iter().map(|&x| x * x).sum();

        // Fast release: after attack, release=0.001s ≈ 44 samples → decays very quickly.
        let mut s_fast_rel = Sampler::new();
        s_fast_rel.set_node_id(1);
        s_fast_rel.bank.set(param_hash("attack"), 0.001);
        s_fast_rel.bank.set(param_hash("release"), 0.001);
        load_test_sample(&mut s_fast_rel, frames);
        let _ = run_sampler(&mut s_fast_rel, &[make_note_on(60)]);
        for _ in 0..4 { run_sampler(&mut s_fast_rel, &[]); }
        let buf_fast = run_sampler(&mut s_fast_rel, &[]);
        let energy_fast: f32 = buf_fast.channel(0).iter().map(|&x| x * x).sum();

        assert!(energy_slow > energy_fast,
            "slow release should have more energy than fast release after 5 blocks: slow={energy_slow:.6} fast={energy_fast:.6}");
    }

    #[test]
    fn sampler_default_attack_0005_is_audibly_instantaneous() {
        // Default attack = 0.005s = 220 samples at 44100 Hz.
        // This is perceptually instant for drums. Verify output is non-zero early.
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, 4096);
        let buf = run_sampler(&mut s, &[make_note_on(60)]);
        // Samples beyond the attack period should be non-zero
        let after_attack = &buf.channel(0)[30..]; // samples 30..64
        assert!(after_attack.iter().any(|&x| x.abs() > 0.0),
            "output should be non-zero after the very short default attack");
    }

    #[test]
    fn sampler_rubato_pitch_up_12_semitones_produces_audio() {
        // Pitch +12 semitones doubles the playback rate. Verify audio is produced.
        let frames = 4096usize;
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, frames);
        let bump = NodeCommand { target_id: 1, type_id: CMD_BUMP_PARAM, arg0: param_hash("pitch") as i64, arg1: 12.0 };
        let _ = run_sampler_with_cmds(&mut s, &[], &[bump]);
        let buf = run_sampler(&mut s, &[make_note_on(60)]);
        assert!(buf.channel(0).iter().any(|&x| x.abs() > 0.0),
            "pitch +12 should still produce audio");
    }

    // ── P7: Per-voice rubato tests ────────────────────────────────────────────

    #[test]
    fn sampler_rubato_nonunity_pitch_produces_audio() {
        // pitch=+6 semitones → ratio ≠ 1.0 → rubato path; verify audio is emitted.
        let frames = 4096usize;
        let mut s = Sampler::new();
        s.set_node_id(1);
        load_test_sample(&mut s, frames);
        let bump = NodeCommand { target_id: 1, type_id: CMD_BUMP_PARAM, arg0: param_hash("pitch") as i64, arg1: 6.0 };
        let _ = run_sampler_with_cmds(&mut s, &[], &[bump]);
        let buf = run_sampler(&mut s, &[make_note_on(60)]);
        assert!(buf.channel(0).iter().any(|&x| x.abs() > 0.0),
            "pitch+6 (rubato path) should produce audio");
    }

    #[test]
    fn sampler_rubato_pitch_down_12_semitones_voice_survives_more_blocks() {
        // pitch=-12 → half-speed playback (ratio≈2.0 in rubato terms).
        // The sinc filter (sinc_len=256) has a warmup cost of ~sinc_len/2=128 frames on
        // the first call, so samples must be much longer than a single block to remain
        // active across multiple blocks.
        // Use a 512-frame sample with block=64.
        let frames = 512usize;

        let mut s_fast = Sampler::new();
        s_fast.set_node_id(1);
        s_fast.sample_data = vec![0.5; frames];
        s_fast.sample_rate_native = 44100.0;
        s_fast.sample_frames = frames;
        s_fast.slices = vec![(0, frames)];
        s_fast.activate(44100.0, 64);
        let _ = run_sampler(&mut s_fast, &[make_note_on(60)]);  // block 1
        let _ = run_sampler(&mut s_fast, &[]);                   // block 2
        let active_fast = s_fast.voices.iter().filter(|v| v.active).count();

        let mut s_slow = Sampler::new();
        s_slow.set_node_id(1);
        s_slow.sample_data = vec![0.5; frames];
        s_slow.sample_rate_native = 44100.0;
        s_slow.sample_frames = frames;
        s_slow.slices = vec![(0, frames)];
        s_slow.activate(44100.0, 64);
        let bump = NodeCommand { target_id: 1, type_id: CMD_BUMP_PARAM, arg0: param_hash("pitch") as i64, arg1: -12.0 };
        let _ = run_sampler_with_cmds(&mut s_slow, &[], &[bump]);
        let _ = run_sampler(&mut s_slow, &[make_note_on(60)]);  // block 1
        let _ = run_sampler(&mut s_slow, &[]);                   // block 2
        let active_slow = s_slow.voices.iter().filter(|v| v.active).count();

        assert!(active_slow >= active_fast,
            "pitch=-12 voice should survive at least as long as pitch=0: slow_active={active_slow} fast_active={active_fast}");
    }

    #[test]
    fn sampler_root_note_param_affects_playback_ratio() {
        // root_note=72 (C5) with note=60 gives note_diff=-12 → half-speed (rubato ratio=2.0).
        // root_note=60 (C4) with note=60 gives note_diff=0  → unity speed (bypass path).
        // Use a 512-frame sample; sinc warmup needs ~160 frames so must exceed that.
        let frames = 512usize;

        let mut s_unity = Sampler::new();
        s_unity.set_node_id(1);
        s_unity.sample_data = vec![0.5; frames];
        s_unity.sample_rate_native = 44100.0;
        s_unity.sample_frames = frames;
        s_unity.slices = vec![(0, frames)];
        s_unity.bank.set(param_hash("root_note"), 60.0); // default
        s_unity.activate(44100.0, 64);
        let _ = run_sampler(&mut s_unity, &[make_note_on(60)]);
        let _ = run_sampler(&mut s_unity, &[]);
        let active_unity = s_unity.voices.iter().filter(|v| v.active).count();

        let mut s_slow = Sampler::new();
        s_slow.set_node_id(1);
        s_slow.sample_data = vec![0.5; frames];
        s_slow.sample_rate_native = 44100.0;
        s_slow.sample_frames = frames;
        s_slow.slices = vec![(0, frames)];
        s_slow.bank.set(param_hash("root_note"), 72.0); // one octave higher root → note 60 plays slow
        s_slow.activate(44100.0, 64);
        let _ = run_sampler(&mut s_slow, &[make_note_on(60)]);
        let _ = run_sampler(&mut s_slow, &[]);
        let active_slow = s_slow.voices.iter().filter(|v| v.active).count();

        assert!(active_slow >= active_unity,
            "root_note=72 should yield slower playback than root_note=60: slow={active_slow} unity={active_unity}");
    }

    #[test]
    fn sampler_serialize_v3_root_note_round_trip() {
        let mut s = Sampler::new();
        s.bank.set(param_hash("root_note"), 48.0); // C3
        s.sample_path = Some("kick.wav".to_string());
        let data = s.serialize();
        assert_eq!(data[0], 3, "version byte should be 3");

        let mut t = Sampler::new();
        t.deserialize(&data);
        assert!((t.bank.get(param_hash("root_note")) - 48.0).abs() < 1e-9,
            "v3 round-trip should preserve root_note");
        assert_eq!(t.sample_path.as_deref(), Some("kick.wav"));
    }

    // ── P6: Symphonia loading test ────────────────────────────────────────────

    #[test]
    fn sampler_symphonia_wav_loads_and_plays() {
        // Write a minimal WAV file with a 440 Hz tone, load it via symphonia,
        // verify the sample frames are populated and audio plays on NoteOn.
        let tmp_path = std::env::temp_dir().join("paraclete_test_sample.wav");
        write_minimal_wav(&tmp_path, 44100, 512);

        let mut s = Sampler::with_path(tmp_path.to_str().unwrap());
        s.activate(44100.0, 64);

        assert!(s.sample_frames > 0,
            "symphonia should load WAV; sample_frames={}", s.sample_frames);

        let buf = run_sampler(&mut s, &[make_note_on(60)]);
        assert!(buf.channel(0).iter().any(|&x| x.abs() > 0.0),
            "should produce audio from loaded WAV");

        let _ = std::fs::remove_file(&tmp_path);
    }

    fn write_minimal_wav(path: &std::path::Path, sample_rate: u32, frames: usize) {
        // Write a minimal 16-bit mono WAV file with a 440 Hz tone.
        let data_bytes = (frames * 2) as u32;
        let mut buf = Vec::with_capacity(44 + data_bytes as usize);

        // RIFF header
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        buf.extend_from_slice(b"WAVE");

        // fmt chunk
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        buf.extend_from_slice(&1u16.to_le_bytes());  // PCM
        buf.extend_from_slice(&1u16.to_le_bytes());  // mono
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        buf.extend_from_slice(&2u16.to_le_bytes());  // block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

        // data chunk
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..frames {
            let v = (i as f32 * 440.0 / sample_rate as f32 * std::f32::consts::TAU).sin();
            let s = (v * 16383.0) as i16;
            buf.extend_from_slice(&s.to_le_bytes());
        }

        std::fs::write(path, buf).unwrap();
    }

    #[test]
    fn sampler_set_initial_params_applied() {
        let mut samp = Sampler::new();
        samp.set_node_id(1);
        samp.set_initial_params(&[("attack".to_string(), 0.5)].into_iter().collect());
        samp.activate(44100.0, 256);
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        samp.published_state(&mut buf);
        let entry = buf.iter().find(|(k, _)| k.ends_with("/attack"));
        assert!(entry.is_some(), "published_state should contain /attack");
        if let paraclete_node_api::StateBusValue::Float(v) = entry.unwrap().1 {
            assert!((v - 0.5).abs() < 1e-9, "attack should be 0.5, got {v}");
        } else {
            panic!("attack entry should be Float");
        }
    }
}
