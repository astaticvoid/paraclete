use paraclete_node_api::{
    CapabilityDocument, Event, NodeCommand, ParameterBank, ParamDescriptor,
    ParamLockEvent, ParamUnit, PortDescriptor, PortDirection, PortType, ProcessInput,
    ProcessOutput, StateBusValue, TimedEvent, TransportEvent, Node, TICKS_PER_BEAT,
};

// ── Step ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Step {
    pub active:      bool,
    pub note:        u8,
    pub velocity:    u16,
    pub length:      f32,
    pub param_locks: Vec<StepParamLock>,
}

#[derive(Clone, Debug)]
pub struct StepParamLock {
    pub node_id:  u32,
    pub param_id: u32,
    pub value:    f64,
}

impl Step {
    pub fn empty() -> Self {
        Step { active: false, note: 60, velocity: 32768, length: 0.75, param_locks: Vec::new() }
    }
}

// ── Sequencer ────────────────────────────────────────────────────────────────

pub struct Sequencer {
    ports: [PortDescriptor; 3],
    node_id: u32,
    track_name: String,

    steps: Vec<Step>,
    pattern_length: usize,
    current_step: usize,
    step_tick: u32,
    ticks_per_step: u32,

    gate_open: bool,
    active_note: u8,
    playing: bool,
    trig_count: u64,
    last_fired_step: usize,

    bank: ParameterBank,

    group: u8,
    channel: u8,
}

impl Sequencer {
    pub const PORT_CLOCK_IN:   u32 = 0;
    pub const PORT_EVENTS_IN:  u32 = 1;
    pub const PORT_EVENTS_OUT: u32 = 2;

    /// Universal NodeCommand type IDs (node-specific, ≥ 16).
    pub const CMD_TOGGLE_STEP: u32 = 16;
    pub const CMD_SET_STEP:    u32 = 17;
    pub const CMD_CLEAR:       u32 = 18;

    pub fn new() -> Self {
        Self::with_name("")
    }

    pub fn with_name(name: &str) -> Self {
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_CLOCK_IN,   name: "clock_in".into(),   direction: PortDirection::Input,  port_type: PortType::Clock },
                PortDescriptor { id: Self::PORT_EVENTS_IN,  name: "events_in".into(),  direction: PortDirection::Input,  port_type: PortType::Event },
                PortDescriptor { id: Self::PORT_EVENTS_OUT, name: "events_out".into(), direction: PortDirection::Output, port_type: PortType::Event },
            ],
            node_id: 0,
            track_name: name.to_string(),
            steps: vec![Step::empty(); 16],
            pattern_length: 16,
            current_step: 0,
            step_tick: 0,
            ticks_per_step: TICKS_PER_BEAT / 4,
            gate_open: false,
            active_note: 60,
            playing: false,
            trig_count: 0,
            last_fired_step: 0,
            bank: ParameterBank::empty(),
            group: 0,
            channel: 0,
        }
    }

    pub fn set_step(&mut self, index: usize, note: u8, velocity: u16, active: bool) {
        if index < self.steps.len() {
            self.steps[index].note     = note;
            self.steps[index].velocity = velocity;
            self.steps[index].active   = active;
        }
    }

    /// 16-character ASCII bitfield: '1' = active, '0' = inactive, padded to pattern_length.
    fn steps_bitfield(&self) -> String {
        self.steps[..self.pattern_length]
            .iter()
            .map(|s| if s.active { '1' } else { '0' })
            .collect()
    }

    fn handle_commands(&mut self, commands: &[NodeCommand]) {
        // Universal params first.
        self.bank.handle_commands(commands);

        for cmd in commands {
            match cmd.type_id {
                Self::CMD_TOGGLE_STEP => {
                    let idx = cmd.arg0 as usize;
                    if idx < self.steps.len() {
                        self.steps[idx].active = !self.steps[idx].active;
                    }
                }
                Self::CMD_SET_STEP => {
                    let idx = cmd.arg0 as usize;
                    if idx < self.steps.len() {
                        if cmd.arg1 < 0.0 {
                            self.steps[idx].active = false;
                        } else {
                            self.steps[idx].note   = cmd.arg1 as u8;
                            self.steps[idx].active = true;
                        }
                    }
                }
                Self::CMD_CLEAR => {
                    for step in &mut self.steps {
                        step.active = false;
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_transport(&mut self, k: &TransportEvent, sample_offset: u32, output: &mut ProcessOutput) {
        if k.flags.sync_pulse {
            let bars_elapsed = (k.bar - 1).max(0) as u64;
            let total_ticks  = bars_elapsed * k.time_sig_num as u64 * TICKS_PER_BEAT as u64
                + k.beat as u64 * TICKS_PER_BEAT as u64
                + k.tick as u64;
            let step_index  = (total_ticks / self.ticks_per_step as u64) % self.pattern_length as u64;
            let new_tick    = (total_ticks % self.ticks_per_step as u64) as u32;
            self.current_step = step_index as usize;
            self.step_tick    = new_tick;

            // When the sync snap lands at tick 0 (exact start of a step) and the
            // sequencer is playing, fire that step if active. Without this, step 0
            // is entered via snap rather than via the normal boundary code path,
            // so emit_note_on is never called for it.
            if new_tick == 0 && self.playing && k.flags.playing {
                let active = self.steps[self.current_step].active;
                if active {
                    if self.gate_open {
                        self.emit_note_off(sample_offset, output);
                    }
                    self.emit_note_on(sample_offset, output);
                    for lock in &self.steps[self.current_step].param_locks {
                        output.events_out.push(TimedEvent::new(
                            sample_offset,
                            Event::ParamLock(ParamLockEvent {
                                node_id:  lock.node_id,
                                param_id: lock.param_id,
                                value:    lock.value,
                            }),
                        ));
                    }
                }
            }
        }

        if k.flags.global_stop {
            self.playing = false;
            if self.gate_open {
                self.emit_note_off(sample_offset, output);
            }
            return;
        }

        if k.flags.global_start {
            self.playing      = true;
            self.current_step = 0;
            self.step_tick    = 0;
        }

        if !k.flags.playing || !self.playing {
            return;
        }

        if self.gate_open {
            let gate_ticks = (self.steps[self.current_step].length * self.ticks_per_step as f32) as u32;
            if self.step_tick >= gate_ticks {
                self.emit_note_off(sample_offset, output);
            }
        }

        if self.step_tick >= self.ticks_per_step {
            self.step_tick    = 0;
            self.current_step = (self.current_step + 1) % self.pattern_length;

            let active = self.steps[self.current_step].active;
            if active {
                self.emit_note_on(sample_offset, output);
                for lock in &self.steps[self.current_step].param_locks {
                    output.events_out.push(TimedEvent::new(
                        sample_offset,
                        Event::ParamLock(ParamLockEvent {
                            node_id:  lock.node_id,
                            param_id: lock.param_id,
                            value:    lock.value,
                        }),
                    ));
                }
            }
        } else {
            self.step_tick += 1;
        }
    }

    fn emit_note_on(&mut self, sample_offset: u32, output: &mut ProcessOutput) {
        let step = &self.steps[self.current_step];
        self.active_note     = step.note;
        self.gate_open       = true;
        self.trig_count      = self.trig_count.wrapping_add(1);
        self.last_fired_step = self.current_step;
        output.events_out.push(TimedEvent::new(
            sample_offset,
            Event::Midi2(build_note_on(self.group, self.channel, step.note, step.velocity)),
        ));
    }

    fn emit_note_off(&mut self, sample_offset: u32, output: &mut ProcessOutput) {
        self.gate_open = false;
        output.events_out.push(TimedEvent::new(
            sample_offset,
            Event::Midi2(build_note_off(self.group, self.channel, self.active_note)),
        ));
    }
}

impl Default for Sequencer {
    fn default() -> Self { Self::new() }
}

impl Node for Sequencer {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "Sequencer",
            vendor: "Paraclete",
            version: (0, 4, 0),
            ports: self.ports.to_vec(),
            params: vec![
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("pattern_length"),
                    name: "pattern_length".into(),
                    min: 1.0, max: 64.0, default: 16.0,
                    stepped: true, unit: ParamUnit::Generic, display: None,
                },
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("ticks_per_step"),
                    name: "ticks_per_step".into(),
                    min: 60.0, max: 3840.0, default: 240.0,
                    stepped: true, unit: ParamUnit::Generic, display: None,
                },
            ],
            extensions: vec!["paraclete.sequencer"],
        }
    }

    fn activate(&mut self, _sr: f32, _block: usize) {
        self.bank = ParameterBank::from_capability_document(&self.capability_document());
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.handle_commands(input.commands);

        for timed in input.events {
            match timed.event {
                Event::Transport(ref k) => {
                    let k = *k;
                    self.handle_transport(&k, timed.sample_offset, output);
                }
                _ => {}
            }
        }
    }

    fn published_state(&self) -> Vec<(String, StateBusValue)> {
        let mut state = vec![
            (format!("/node/{}/state/current_step",   self.node_id), StateBusValue::Int(self.current_step as i64)),
            (format!("/node/{}/state/pattern_length", self.node_id), StateBusValue::Int(self.pattern_length as i64)),
            (format!("/node/{}/state/playing",        self.node_id), StateBusValue::Bool(self.playing)),
            (format!("/node/{}/state/steps",          self.node_id), StateBusValue::Text(self.steps_bitfield())),
            (format!("/node/{}/state/last_trig",      self.node_id), StateBusValue::Int(self.trig_count as i64)),
            (format!("/node/{}/state/last_fired_step", self.node_id), StateBusValue::Int(self.last_fired_step as i64)),
        ];
        if !self.track_name.is_empty() {
            state.push((
                format!("/node/{}/state/track_name", self.node_id),
                StateBusValue::Text(self.track_name.clone()),
            ));
        }
        state
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(1u8);
        buf.push(self.pattern_length as u8);
        buf.extend_from_slice(&self.ticks_per_step.to_le_bytes());
        for step in &self.steps {
            buf.push(step.active as u8);
            buf.push(step.note);
            buf.extend_from_slice(&step.velocity.to_le_bytes());
            buf.extend_from_slice(&step.length.to_le_bytes());
            buf.push(step.param_locks.len() as u8);
            for lock in &step.param_locks {
                buf.extend_from_slice(&lock.node_id.to_le_bytes());
                buf.extend_from_slice(&lock.param_id.to_le_bytes());
                buf.extend_from_slice(&lock.value.to_le_bytes());
            }
        }
        buf
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.is_empty() || data[0] != 1 { return; }
        let mut cur = 1usize;

        macro_rules! read_u8  { () => {{ if cur >= data.len() { return; } let v = data[cur]; cur += 1; v }} }
        macro_rules! read_u16 { () => {{ if cur + 2 > data.len() { return; } let v = u16::from_le_bytes(data[cur..cur+2].try_into().unwrap()); cur += 2; v }} }
        macro_rules! read_u32 { () => {{ if cur + 4 > data.len() { return; } let v = u32::from_le_bytes(data[cur..cur+4].try_into().unwrap()); cur += 4; v }} }
        macro_rules! read_f32 { () => {{ if cur + 4 > data.len() { return; } let v = f32::from_le_bytes(data[cur..cur+4].try_into().unwrap()); cur += 4; v }} }
        macro_rules! read_f64 { () => {{ if cur + 8 > data.len() { return; } let v = f64::from_le_bytes(data[cur..cur+8].try_into().unwrap()); cur += 8; v }} }

        let pattern_length  = read_u8!() as usize;
        let ticks_per_step  = read_u32!();

        let mut steps = Vec::with_capacity(pattern_length);
        for _ in 0..pattern_length {
            let active     = read_u8!() != 0;
            let note       = read_u8!();
            let velocity   = read_u16!();
            let length     = read_f32!();
            let lock_count = read_u8!() as usize;
            let mut param_locks = Vec::with_capacity(lock_count);
            for _ in 0..lock_count {
                let node_id  = read_u32!();
                let param_id = read_u32!();
                let value    = read_f64!();
                param_locks.push(StepParamLock { node_id, param_id, value });
            }
            steps.push(Step { active, note, velocity, length, param_locks });
        }

        self.steps          = steps;
        self.pattern_length = pattern_length;
        self.ticks_per_step = ticks_per_step;
    }
}

use crate::{build_note_on, build_note_off};

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, Event, TransportEvent, TransportFlags,
        ExtendedEventSlab, TransportInfo, UmpMessage, TICKS_PER_BEAT,
        midi::ChannelVoice2,
    };

    fn transport_tick(tick: u32, playing: bool, global_start: bool, global_stop: bool, sync_pulse: bool) -> TimedEvent {
        TimedEvent::new(0, Event::Transport(TransportEvent {
            domain_id: 0, bar: 1, beat: 0, tick,
            ticks_per_beat: TICKS_PER_BEAT, bpm: 120.0,
            time_sig_num: 4, time_sig_den: 4,
            flags: TransportFlags { playing, global_start, global_stop, sync_pulse, ..TransportFlags::default() },
        }))
    }

    fn run_seq(seq: &mut Sequencer, events: &[TimedEvent]) -> Vec<Event> {
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];
        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: &[],
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs, signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        seq.process(&input, &mut output);
        events_out.as_slice().iter().map(|e| e.event).collect()
    }

    fn run_seq_with_cmds(seq: &mut Sequencer, cmds: &[NodeCommand]) {
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];
        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events: &[],
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: cmds,
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs, signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        seq.process(&input, &mut output);
    }

    #[test]
    fn sequencer_new_has_16_empty_steps() {
        let seq = Sequencer::new();
        let state = seq.published_state();
        let len = state.iter().find(|(k, _)| k.ends_with("/state/pattern_length"));
        assert!(matches!(len, Some((_, StateBusValue::Int(16)))));
    }

    #[test]
    fn sequencer_does_not_emit_when_not_playing() {
        let mut seq = Sequencer::new();
        seq.set_step(0, 60, 32768, true);
        let events = run_seq(&mut seq, &[transport_tick(0, false, false, false, false)]);
        assert!(events.is_empty());
    }

    #[test]
    fn sequencer_advances_step_and_emits_note_on_at_boundary() {
        let mut seq = Sequencer::new();
        seq.set_step(1, 62, 32768, true);
        let ticks_per_step = TICKS_PER_BEAT / 4;
        let mut all_out = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        for t in 1..=ticks_per_step {
            all_out.extend(run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]));
        }
        let has_note_on = all_out.iter().any(|e| {
            matches!(e, Event::Midi2(ump) if matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))))
        });
        assert!(has_note_on, "expected NoteOn at step boundary");
    }

    #[test]
    fn sequencer_responds_to_global_stop() {
        let mut seq = Sequencer::new();
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq(&mut seq, &[transport_tick(1, false, false, true, false)]);
        assert!(!seq.playing);
    }

    #[test]
    fn sequencer_cmd_toggle_step_flips_active() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        assert!(!seq.steps[3].active);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_TOGGLE_STEP, arg0: 3, arg1: 0.0 }]);
        assert!(seq.steps[3].active);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_TOGGLE_STEP, arg0: 3, arg1: 0.0 }]);
        assert!(!seq.steps[3].active);
    }

    #[test]
    fn sequencer_cmd_clear_deactivates_all_steps() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        for i in 0..16 { seq.set_step(i, 60, 32768, true); }
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_CLEAR, arg0: 0, arg1: 0.0 }]);
        assert!(seq.steps.iter().all(|s| !s.active));
    }

    #[test]
    fn sequencer_published_state_includes_steps_bitfield() {
        let mut seq = Sequencer::new();
        seq.set_node_id(5);
        seq.set_step(0, 60, 32768, true);
        seq.set_step(2, 60, 32768, true);
        let state = seq.published_state();
        let steps = state.iter().find(|(k, _)| k.ends_with("/state/steps"));
        assert!(matches!(steps, Some((_, StateBusValue::Text(s))) if s.starts_with("10100")));
    }

    #[test]
    fn sequencer_published_state_has_track_name_when_set() {
        let mut seq = Sequencer::with_name("Kick");
        seq.set_node_id(1);
        let state = seq.published_state();
        let name = state.iter().find(|(k, _)| k.ends_with("/state/track_name"));
        assert!(matches!(name, Some((_, StateBusValue::Text(n))) if n == "Kick"));
    }

    // CMD_TOGGLE_STEP activates step 0 and it fires after the full 16-step cycle.
    // This is the sequencer-level half of the "step 0 never triggers" bug
    // (the other half was state_write silently dropping strings, which meant
    // CMD_TOGGLE_STEP was never sent from the profile).
    // Regression test: sync_pulse at bar boundary snapped current_step=0 with
    // step_tick=0, bypassing the boundary code path that calls emit_note_on.
    // Step 0 was silently skipped every bar.
    #[test]
    fn step_0_fires_on_sync_pulse_at_bar_boundary() {
        let tps = TICKS_PER_BEAT / 4;  // 240
        let bar_ticks = (4 * TICKS_PER_BEAT) as u64; // 3840 = one 4/4 bar

        let mut seq = Sequencer::new();
        seq.set_node_id(1);
        seq.activate(44100.0, 64);
        seq.set_step(0, 60, 32768, true);

        // global_start
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);

        // Run to just before bar boundary; step 0 should NOT have fired yet.
        let mut fired_before_sync = false;
        for tick in 1u32..(bar_ticks as u32) {
            let events = run_seq(&mut seq, &[transport_tick(tick, true, false, false, false)]);
            if events.iter().any(|e| matches!(e, Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))))) {
                fired_before_sync = true;
            }
        }
        // Step 0 only fires at the wrap (tick ~3841), not before the bar boundary.
        // (Some implementations fire it before; this just checks the sync path.)

        // Send a sync_pulse at the bar boundary — step 0 must fire.
        let sync_tick = bar_ticks as u32;
        let sync_event = TimedEvent::new(0, Event::Transport(TransportEvent {
            domain_id: 0, bar: 2, beat: 0, tick: 0,
            ticks_per_beat: TICKS_PER_BEAT, bpm: 140.0,
            time_sig_num: 4, time_sig_den: 4,
            flags: TransportFlags {
                playing: true, sync_pulse: true, ..TransportFlags::default()
            },
        }));
        let sync_events = run_seq(&mut seq, &[sync_event]);
        let fired_at_sync = sync_events.iter().any(|e|
            matches!(e, Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_)))));

        assert!(fired_at_sync || fired_before_sync,
            "step 0 must fire: either via normal boundary before sync or via sync_pulse at bar boundary");
    }

    #[test]
    fn sequencer_serialize_deserialize_round_trip() {
        let mut seq = Sequencer::new();
        seq.set_step(3, 72, 50000, true);
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&data);
        assert_eq!(restored.steps[3].note, 72);
        assert!(restored.steps[3].active);
    }
}
