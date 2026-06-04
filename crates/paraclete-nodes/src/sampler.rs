use std::collections::HashMap;

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, ConnectionRecord, Event, LockableParam,
    Negotiable, Node, ParamDescriptor, ParamUnit,
    ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, UmpMessage,
    midi::ChannelVoice2,
};

// ── param_hash ────────────────────────────────────────────────────────────────

fn param_hash(name: &str) -> u32 {
    ParamDescriptor::id_for_name(name)
}

// ── ActiveParamLock ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct ActiveParamLock {
    locked_value: f64,
}

// ── Voice ──────────────────────────────────────────────────────────────────────

struct Voice {
    active: bool,
    note: u8,
    playback_pos: f64,
    active_locks: HashMap<u32, ActiveParamLock>,
    triggered_at: u64,
}

impl Voice {
    fn new() -> Self {
        Self {
            active: false,
            note: 0,
            playback_pos: 0.0,
            active_locks: HashMap::with_capacity(9), // pitch/vol/pan/start/end/attack/release/loop/slice
            triggered_at: 0,
        }
    }

    fn effective(&self, param_id: u32, base: f64) -> f64 {
        self.active_locks.get(&param_id)
            .map(|l| l.locked_value)
            .unwrap_or(base)
    }
}

// ── Sampler capability document ───────────────────────────────────────────────

/// Static capability document for the Sampler. Called at new() and capability_document().
/// Ports are overridden with the instance's port list in capability_document().
fn sampler_capability_document() -> CapabilityDocument {
    CapabilityDocument {
        name: "Sampler",
        vendor: "Paraclete",
        version: (0, 4, 0),
        ports: vec![],
        params: vec![
            ParamDescriptor { id: param_hash("pitch"),   name: "pitch".into(),   min: -24.0, max: 24.0, default: 0.0,   stepped: false, unit: ParamUnit::Semitones, display: None },
            ParamDescriptor { id: param_hash("volume"),  name: "volume".into(),  min: 0.0,   max: 1.0,  default: 1.0,   stepped: false, unit: ParamUnit::Generic,   display: None },
            ParamDescriptor { id: param_hash("pan"),     name: "pan".into(),     min: -1.0,  max: 1.0,  default: 0.0,   stepped: false, unit: ParamUnit::Generic,   display: None },
            ParamDescriptor { id: param_hash("start"),   name: "start".into(),   min: 0.0,   max: 1.0,  default: 0.0,   stepped: false, unit: ParamUnit::Percent,   display: None },
            ParamDescriptor { id: param_hash("end"),     name: "end".into(),     min: 0.0,   max: 1.0,  default: 1.0,   stepped: false, unit: ParamUnit::Percent,   display: None },
            ParamDescriptor { id: param_hash("attack"),  name: "attack".into(),  min: 0.0,   max: 1.0,  default: 0.005, stepped: false, unit: ParamUnit::Seconds,   display: None },
            ParamDescriptor { id: param_hash("release"), name: "release".into(), min: 0.0,   max: 4.0,  default: 0.1,   stepped: false, unit: ParamUnit::Seconds,   display: None },
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
    root_note: u8,

    sample_path: Option<String>,
    pub(crate) connection_records: Vec<ConnectionRecord>,

    // Pre-allocated render buffers — no audio-thread allocation.
    render_l: Vec<f32>,
    render_r: Vec<f32>,
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
            root_note: 60,
            sample_path,
            connection_records: Vec::new(),
            render_l: Vec::new(),
            render_r: Vec::new(),
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

        let voice = &mut self.voices[voice_idx];
        voice.active = true;
        voice.note = note;
        voice.playback_pos = 0.0;
        voice.triggered_at = self.cycle_counter;
        voice.active_locks.clear();

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

    fn published_state(&self) -> Vec<(String, paraclete_node_api::StateBusValue)> {
        vec![
            (format!("/node/{}/state/trig",      self.node_id), paraclete_node_api::StateBusValue::Int(self.samp_trig_count as i64)),
            (format!("/node/{}/state/last_note", self.node_id), paraclete_node_api::StateBusValue::Int(self.last_triggered_note as i64)),
        ]
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
        let root_note = self.root_note;

        // Take a shared slice of sample_data — coexists with the mutable
        // borrow of voices below since they are different fields.
        let sample_data = self.sample_data.as_slice();

        for voice in self.voices.iter_mut() {
            if !voice.active { continue; }

            let voice_pitch = voice.effective(param_hash("pitch"), node_pitch) + pitch_mod;
            let note_diff = voice.note as f64 - root_note as f64 + voice_pitch;
            let playback_rate = 2.0_f64.powf(note_diff / 12.0)
                * native_sr as f64 / output_sr as f64;

            let mut deactivate = false;
            for frame in 0..block_size {
                let abs_pos = start_frame as f64 + voice.playback_pos;
                if abs_pos < end_frame as f64 {
                    let idx = abs_pos as usize;
                    let frac = (abs_pos - idx as f64) as f32;
                    let s0 = sample_data.get(idx).copied().unwrap_or(0.0);
                    let s1 = sample_data.get(idx + 1).copied().unwrap_or(0.0);
                    let sample = s0 + (s1 - s0) * frac;
                    self.render_l[frame] += sample * vol * pan_l;
                    self.render_r[frame] += sample * vol * pan_r;
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
        let mut buf = vec![2u8]; // version 2: bank-based params
        let path = self.sample_path.as_deref().unwrap_or("");
        let path_bytes = path.as_bytes();
        buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(path_bytes);
        buf.push(self.root_note);
        for &val in &[
            self.bank.get(param_hash("pitch")),
            self.bank.get(param_hash("volume")),
            self.bank.get(param_hash("pan")),
            self.bank.get(param_hash("start")),
            self.bank.get(param_hash("end")),
            self.bank.get(param_hash("attack")),
            self.bank.get(param_hash("release")),
        ] {
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf.push(self.base_loop as u8);
        buf.push(self.base_slice as u8);
        buf
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.is_empty() || data[0] != 2 { return; }
        let mut cur = 1usize;

        if cur + 2 > data.len() { return; }
        let path_len = u16::from_le_bytes(data[cur..cur + 2].try_into().unwrap()) as usize;
        cur += 2;

        if cur + path_len > data.len() { return; }
        let path = std::str::from_utf8(&data[cur..cur + path_len]).unwrap_or("").to_string();
        cur += path_len;
        self.sample_path = if path.is_empty() { None } else { Some(path) };

        if cur >= data.len() { return; }
        self.root_note = data[cur]; cur += 1;

        macro_rules! read_f64 {
            () => {{
                if cur + 8 > data.len() { return; }
                let v = f64::from_le_bytes(data[cur..cur + 8].try_into().unwrap());
                cur += 8; v
            }};
        }

        self.bank.set(param_hash("pitch"),   read_f64!());
        self.bank.set(param_hash("volume"),  read_f64!());
        self.bank.set(param_hash("pan"),     read_f64!());
        self.bank.set(param_hash("start"),   read_f64!());
        self.bank.set(param_hash("end"),     read_f64!());
        self.bank.set(param_hash("attack"),  read_f64!());
        self.bank.set(param_hash("release"), read_f64!());

        if cur >= data.len() { return; }
        self.base_loop = data[cur] != 0; cur += 1;

        if cur >= data.len() { return; }
        self.base_slice = data[cur] as usize;
    }
}

impl Negotiable for Sampler {}

// ── WAV loading ────────────────────────────────────────────────────────────────

fn load_wav(path: &str, target_rate: f32) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("hound: {}", e))?;

    let spec = reader.spec();
    let native_rate = spec.sample_rate as f32;

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect()
        }
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample as f32;
            let max = 2.0_f32.powf(bits - 1.0);
            reader.samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / max)
                .collect()
        }
    };

    let mono: Vec<f32> = if spec.channels == 2 {
        samples.chunks(2)
            .map(|c| (c[0] + c.get(1).copied().unwrap_or(0.0)) * 0.5)
            .collect()
    } else {
        samples
    };

    if (native_rate - target_rate).abs() < 1.0 {
        Ok(mono)
    } else {
        // Simple linear interpolation — TODO P6: replace with rubato (MIT).
        let ratio = native_rate / target_rate;
        let output_len = (mono.len() as f32 / ratio) as usize;
        let mut resampled = Vec::with_capacity(output_len);
        for i in 0..output_len {
            let pos = i as f32 * ratio;
            let idx = pos as usize;
            let frac = pos - idx as f32;
            let s0 = mono.get(idx).copied().unwrap_or(0.0);
            let s1 = mono.get(idx + 1).copied().unwrap_or(0.0);
            resampled.push(s0 + (s1 - s0) * frac);
        }
        Ok(resampled)
    }
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
    fn sampler_capability_document_has_7_params() {
        // pitch, volume, pan, start, end, attack, release
        let s = Sampler::new();
        assert_eq!(s.capability_document().params.len(), 7);
    }

    #[test]
    fn sampler_capability_document_extension_is_instrument() {
        let s = Sampler::new();
        assert!(s.capability_document().extensions.contains(&"paraclete.instrument"));
    }

    #[test]
    fn sampler_negotiate_returns_7_lockable_params() {
        let mut s = Sampler::new();
        let their_doc = CapabilityDocument::from_ports(&[]);
        let agreement = s.negotiate(&their_doc);
        assert_eq!(agreement.lockable_params.len(), 7);
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
        s.bank.set(param_hash("pitch"),  2.0);
        s.bank.set(param_hash("volume"), 0.5);
        s.bank.set(param_hash("pan"),   -0.3);
        s.bank.set(param_hash("start"),  0.1);
        s.bank.set(param_hash("end"),    0.9);
        s.bank.set(param_hash("attack"), 0.05);
        s.bank.set(param_hash("release"), 0.8);
        s.base_loop = true;
        s.base_slice = 3;
        s.root_note = 69;
        s.sample_path = Some("samples/kick.wav".to_string());

        let data = s.serialize();
        let mut t = Sampler::new();
        t.deserialize(&data);

        assert!((t.bank.get(param_hash("pitch"))   - 2.0).abs() < 1e-9);
        assert!((t.bank.get(param_hash("volume"))  - 0.5).abs() < 1e-9);
        assert!((t.bank.get(param_hash("pan"))     - (-0.3)).abs() < 1e-9);
        assert!((t.bank.get(param_hash("start"))   - 0.1).abs() < 1e-9);
        assert!((t.bank.get(param_hash("end"))     - 0.9).abs() < 1e-9);
        assert!((t.bank.get(param_hash("attack"))  - 0.05).abs() < 1e-9);
        assert!((t.bank.get(param_hash("release")) - 0.8).abs() < 1e-9);
        assert!(t.base_loop);
        assert_eq!(t.base_slice, 3);
        assert_eq!(t.root_note, 69);
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
}
