use paraclete_node_api::{
    CapabilityDocument, Event, TransportEvent, StateBusValue, Node, ParamDescriptor, ParamLockEvent,
    ParamUnit, PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
    TimedEvent, TICKS_PER_BEAT,
};

// ── Step ─────────────────────────────────────────────────────────────────────

/// One step in the Sequencer pattern.
#[derive(Clone, Debug)]
pub struct Step {
    /// Whether this step triggers.
    pub active: bool,
    /// MIDI note number 0–127.
    pub note: u8,
    /// 16-bit velocity (MIDI 2.0 resolution).
    pub velocity: u16,
    /// Note length as a fraction of ticks_per_step (1.0 = full step).
    pub length: f32,
    /// Parameter locks — per-step overrides for connected nodes.
    pub param_locks: Vec<StepParamLock>,
}

/// A per-step parameter lock.
#[derive(Clone, Debug)]
pub struct StepParamLock {
    pub node_id: u32,
    pub param_id: u32,
    pub value: f64,
}

impl Step {
    /// A silent, inactive step.
    pub fn empty() -> Self {
        Step {
            active: false,
            note: 60,
            velocity: 32768,
            length: 0.75, // release before next step
            param_locks: Vec::new(),
        }
    }
}

// ── Sequencer ────────────────────────────────────────────────────────────────

/// 16-step sequencer node.
///
/// Receives clock ticks from an `InternalClock` via its `clock_in` port.
/// On each active step, emits a `Midi2` NoteOn event and a matching NoteOff
/// after `step.length * ticks_per_step` ticks.
pub struct Sequencer {
    ports: [PortDescriptor; 3],
    node_id: u32,

    steps: Vec<Step>,
    pattern_length: usize,
    current_step: usize,
    step_tick: u32,
    /// 16th-note default: TICKS_PER_BEAT / 4 = 240
    ticks_per_step: u32,

    gate_open: bool,
    active_note: u8,
    playing: bool,

    group: u8,
    channel: u8,
}

impl Sequencer {
    pub const PORT_CLOCK_IN:   u32 = 0;
    pub const PORT_EVENTS_IN:  u32 = 1;
    pub const PORT_EVENTS_OUT: u32 = 2;

    pub fn new() -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_CLOCK_IN,
                    name: "clock_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Clock,
                },
                PortDescriptor {
                    id: Self::PORT_EVENTS_IN,
                    name: "events_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Event,
                },
                PortDescriptor {
                    id: Self::PORT_EVENTS_OUT,
                    name: "events_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Event,
                },
            ],
            node_id: 0,
            steps: vec![Step::empty(); 16],
            pattern_length: 16,
            current_step: 0,
            step_tick: 0,
            ticks_per_step: TICKS_PER_BEAT / 4,
            gate_open: false,
            active_note: 60,
            playing: false,
            group: 0,
            channel: 0,
        }
    }

    /// Set a step's note and activate it.
    pub fn set_step(&mut self, index: usize, note: u8, velocity: u16, active: bool) {
        if index < self.steps.len() {
            self.steps[index].note = note;
            self.steps[index].velocity = velocity;
            self.steps[index].active = active;
        }
    }

    fn handle_transport(&mut self, k: &TransportEvent, sample_offset: u32, output: &mut ProcessOutput) {
        // Sync pulse — snap position to authoritative clock.
        if k.flags.sync_pulse {
            // bar is 1-based in TransportInfo but we compute from 0 for modulo
            let bars_elapsed = (k.bar - 1).max(0) as u64;
            let total_ticks = bars_elapsed * k.time_sig_num as u64 * TICKS_PER_BEAT as u64
                + k.beat as u64 * TICKS_PER_BEAT as u64
                + k.tick as u64;
            let step_index =
                (total_ticks / self.ticks_per_step as u64) % self.pattern_length as u64;
            self.current_step = step_index as usize;
            self.step_tick = (total_ticks % self.ticks_per_step as u64) as u32;
        }

        if k.flags.global_stop {
            self.playing = false;
            if self.gate_open {
                self.emit_note_off(sample_offset, output);
            }
            return;
        }

        if k.flags.global_start {
            self.playing = true;
            self.current_step = 0;
            self.step_tick = 0;
        }

        if !k.flags.playing || !self.playing {
            return;
        }

        // Check for gate-off (note length elapsed before step boundary).
        if self.gate_open {
            let gate_ticks = (self.steps[self.current_step].length
                * self.ticks_per_step as f32) as u32;
            if self.step_tick >= gate_ticks {
                self.emit_note_off(sample_offset, output);
            }
        }

        // Step boundary — advance to next step.
        if self.step_tick >= self.ticks_per_step {
            self.step_tick = 0;
            self.current_step = (self.current_step + 1) % self.pattern_length;

            let (active, locks) = {
                let step = &self.steps[self.current_step];
                (step.active, step.param_locks.clone())
            };
            if active {
                self.emit_note_on(sample_offset, output);

                // Emit parameter locks for this step.
                for lock in locks {
                    output.events_out.push(TimedEvent::new(
                        sample_offset,
                        Event::ParamLock(ParamLockEvent {
                            node_id: lock.node_id,
                            param_id: lock.param_id,
                            value: lock.value,
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
        self.active_note = step.note;
        self.gate_open = true;

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

impl Node for Sequencer {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "Sequencer",
            vendor: "Paraclete",
            version: (0, 2, 0),
            ports: self.ports.to_vec(),
            params: vec![
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("pattern_length"),
                    name: "pattern_length".into(),
                    min: 1.0,
                    max: 64.0,
                    default: 16.0,
                    stepped: true,
                    unit: ParamUnit::Generic,
                    display: None,
                },
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("ticks_per_step"),
                    name: "ticks_per_step".into(),
                    min: 60.0,
                    max: 3840.0,
                    default: 240.0,
                    stepped: true,
                    unit: ParamUnit::Generic,
                    display: None,
                },
            ],
            extensions: vec!["paraclete.sequencer"],
        }
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        for timed in input.events {
            match timed.event {
                Event::Transport(ref k) => {
                    let k = *k;
                    self.handle_transport(&k, timed.sample_offset, output);
                }
                Event::Midi2(_) => {
                    // Real-time recording deferred to P5.
                }
                _ => {}
            }
        }
    }

    fn published_state(&self) -> Vec<(String, StateBusValue)> {
        vec![
            (
                format!("/node/{}/state/current_step", self.node_id),
                StateBusValue::Int(self.current_step as i64),
            ),
            (
                format!("/node/{}/state/pattern_length", self.node_id),
                StateBusValue::Int(self.pattern_length as i64),
            ),
            (
                format!("/node/{}/state/playing", self.node_id),
                StateBusValue::Bool(self.playing),
            ),
        ]
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(1u8); // format version
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
        if data.is_empty() || data[0] != 1 {
            return; // unknown format version — ignore
        }
        let mut cur = 1usize;

        macro_rules! read_u8 {
            () => {{
                if cur >= data.len() { return; }
                let v = data[cur]; cur += 1; v
            }};
        }
        macro_rules! read_u16 {
            () => {{
                if cur + 2 > data.len() { return; }
                let v = u16::from_le_bytes(data[cur..cur+2].try_into().unwrap());
                cur += 2; v
            }};
        }
        macro_rules! read_u32 {
            () => {{
                if cur + 4 > data.len() { return; }
                let v = u32::from_le_bytes(data[cur..cur+4].try_into().unwrap());
                cur += 4; v
            }};
        }
        macro_rules! read_f32 {
            () => {{
                if cur + 4 > data.len() { return; }
                let v = f32::from_le_bytes(data[cur..cur+4].try_into().unwrap());
                cur += 4; v
            }};
        }
        macro_rules! read_f64 {
            () => {{
                if cur + 8 > data.len() { return; }
                let v = f64::from_le_bytes(data[cur..cur+8].try_into().unwrap());
                cur += 8; v
            }};
        }

        let pattern_length = read_u8!() as usize;
        let ticks_per_step = read_u32!();

        let mut steps = Vec::with_capacity(pattern_length);
        for _ in 0..pattern_length {
            let active = read_u8!() != 0;
            let note = read_u8!();
            let velocity = read_u16!();
            let length = read_f32!();
            let lock_count = read_u8!() as usize;
            let mut param_locks = Vec::with_capacity(lock_count);
            for _ in 0..lock_count {
                let node_id = read_u32!();
                let param_id = read_u32!();
                let value = read_f64!();
                param_locks.push(StepParamLock { node_id, param_id, value });
            }
            steps.push(Step { active, note, velocity, length, param_locks });
        }

        self.steps = steps;
        self.pattern_length = pattern_length;
        self.ticks_per_step = ticks_per_step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, Event, TransportEvent, TransportFlags, ExtendedEventSlab,
        TransportInfo, Node, TimedEvent, UmpMessage, TICKS_PER_BEAT,
        midi::ChannelVoice2,
    };

    fn transport_tick(tick: u32, playing: bool, global_start: bool, global_stop: bool, sync_pulse: bool) -> TimedEvent {
        TimedEvent::new(0, Event::Transport(TransportEvent {
            domain_id: 0,
            bar: 1,
            beat: 0,
            tick,
            ticks_per_beat: TICKS_PER_BEAT,
            bpm: 120.0,
            time_sig_num: 4,
            time_sig_den: 4,
            flags: TransportFlags {
                playing,
                global_start,
                global_stop,
                sync_pulse,
                ..TransportFlags::default()
            },
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
            extended_events: &slab,
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs, signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        seq.process(&input, &mut output);
        events_out.as_slice().iter().map(|e| e.event).collect()
    }

    #[test]
    fn sequencer_new_has_16_empty_steps() {
        let seq = Sequencer::new();
        let state = seq.published_state();
        let len = state.iter().find(|(k, _)| k.ends_with("/state/pattern_length"));
        assert!(matches!(len, Some((_, paraclete_node_api::StateBusValue::Int(16)))));
    }

    #[test]
    fn sequencer_does_not_emit_when_not_playing() {
        let mut seq = Sequencer::new();
        seq.set_step(0, 60, 32768, true);
        // Send a tick without global_start — playing stays false
        let events = run_seq(&mut seq, &[transport_tick(0, false, false, false, false)]);
        assert!(events.is_empty());
    }

    #[test]
    fn sequencer_advances_step_and_emits_note_on_at_boundary() {
        let mut seq = Sequencer::new();
        // Step 1 fires after the first ticks_per_step ticks from global_start.
        // (global_start resets to step 0; first boundary advances to step 1.)
        seq.set_step(1, 62, 32768, true);

        let ticks_per_step = TICKS_PER_BEAT / 4; // 240

        // global_start event
        let mut all_out = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);

        // ticks_per_step more ticks cross the step boundary → step 1 fires NoteOn
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
        // Start playing
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // Stop
        let out = run_seq(&mut seq, &[transport_tick(1, false, false, true, false)]);
        assert!(!seq.playing);
        let _ = out; // may contain a note-off if gate was open
    }

    #[test]
    fn sequencer_serialize_deserialize_round_trip() {
        let mut seq = Sequencer::new();
        seq.set_step(3, 72, 50000, true);
        seq.set_step(7, 64, 30000, false);

        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&data);

        assert_eq!(restored.steps[3].note, 72);
        assert!(restored.steps[3].active);
        assert_eq!(restored.steps[7].note, 64);
        assert!(!restored.steps[7].active);
    }

    #[test]
    fn sequencer_published_state_has_current_step_and_playing() {
        let mut seq = Sequencer::new();
        seq.set_node_id(42);
        let state = seq.published_state();
        assert!(state.iter().any(|(k, _)| k.contains("current_step")));
        assert!(state.iter().any(|(k, _)| k.contains("playing")));
        assert!(state.iter().any(|(k, _)| k.contains("pattern_length")));
    }
}

use crate::{build_note_on, build_note_off};
