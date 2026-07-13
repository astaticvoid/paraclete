use paraclete_node_api::{
    CapabilityDocument, Event, ClockPriority, TransportEvent, TransportFlags, StateBusValue, Node,
    ParamDescriptor, ParamUnit, TempoSource, PortDescriptor, PortDirection, PortType,
    ProcessInput, ProcessOutput, TimedEvent, TICKS_PER_BEAT,
};

/// The internal clock master. Provides the primary clock domain in standalone mode.
///
/// Emits `TransportEvent`s on its `clock_out` port at sub-sample accuracy.
/// A bar-boundary sync pulse is emitted every bar so downstream nodes can
/// snap their internal position.
pub struct InternalClock {
    ports: [PortDescriptor; 2],
    domain_id_val: u32,
    node_id: u32,

    bpm: f64,
    bar: i32,
    beat: u32,
    tick: u32,
    time_sig_num: u8,
    time_sig_den: u8,
    playing: bool,

    /// Sub-sample accumulator. Advances by ticks_per_sample each frame.
    tick_accumulator: f64,
    sample_rate: f32,
    /// True until the first TransportEvent is emitted — triggers global_start so
    /// downstream nodes (Sequencer) know to begin playback.
    first_tick: bool,
}

impl Default for InternalClock {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalClock {
    pub const PORT_BPM_MOD:   u32 = 0;
    pub const PORT_CLOCK_OUT: u32 = 1;
    pub const PARAM_BPM: &'static str = "bpm";

    pub fn new() -> Self {
        Self::with_domain(0)
    }

    pub fn with_bpm(bpm: f64) -> Self {
        let mut clock = Self::with_domain(0);
        clock.bpm = bpm;
        clock
    }

    pub fn with_domain(domain_id: u32) -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_BPM_MOD,
                    name: "bpm_mod".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Modulation,
                },
                PortDescriptor {
                    id: Self::PORT_CLOCK_OUT,
                    name: "clock_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Clock,
                },
            ],
            domain_id_val: domain_id,
            node_id: 0,
            bpm: 120.0,
            bar: 1,
            beat: 0,
            tick: 0,
            time_sig_num: 4,
            time_sig_den: 4,
            playing: true,
            tick_accumulator: 0.0,
            sample_rate: 44100.0,
            first_tick: true,
        }
    }

    fn ticks_per_sample(&self, bpm: f64) -> f64 {
        (bpm / 60.0) * TICKS_PER_BEAT as f64 / self.sample_rate as f64
    }

    fn emit_transport(
        &mut self,
        sample_offset: u32,
        bpm: f64,
        output: &mut ProcessOutput,
    ) {
        let is_bar_start = self.beat == 0 && self.tick == 0;
        let is_first = self.first_tick;
        self.first_tick = false;

        output.events_out.push(TimedEvent::new(
            sample_offset,
            Event::Transport(TransportEvent {
                domain_id: self.domain_id_val,
                bar: self.bar,
                beat: self.beat,
                tick: self.tick,
                ticks_per_beat: TICKS_PER_BEAT,
                bpm,
                time_sig_num: self.time_sig_num,
                time_sig_den: self.time_sig_den,
                flags: TransportFlags {
                    playing: self.playing,
                    sync_pulse: is_bar_start,
                    global_start: is_first,  // signals downstream nodes to begin
                    ..TransportFlags::default()
                },
            }),
        ));
    }

    fn advance_position(&mut self) {
        self.tick += 1;
        if self.tick >= TICKS_PER_BEAT {
            self.tick = 0;
            self.beat += 1;
            if self.beat >= self.time_sig_num as u32 {
                self.beat = 0;
                self.bar += 1;
            }
        }
    }
}

impl Node for InternalClock {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate;
        self.tick_accumulator = 0.0;
    }

    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "InternalClock".into(),
            vendor: "Paraclete".into(),
            version: (0, 2, 0),
            ports: self.ports.to_vec(),
            params: vec![
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name(Self::PARAM_BPM),
                    name: "bpm".into(),
                    min: 20.0,
                    max: 300.0,
                    default: 120.0,
                    stepped: false,
                    unit: ParamUnit::Generic,
                    display: None,
                },
            ],
            extensions: vec!["paraclete.tempo_source".into()],
        }
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        for timed in input.events {
            if let Event::Transport(te) = timed.event {
                if te.flags.global_stop {
                    self.playing = false;
                } else if te.flags.global_start && !self.playing {
                    self.playing = true;
                    self.first_tick = true;
                }
            }
        }

        if !self.playing { return; }

        // BPM modulation via signal port is deferred to P9 (signal port wiring).
        // For now, use the base BPM parameter directly.
        let effective_bpm = self.bpm;
        let tps = self.ticks_per_sample(effective_bpm);

        for frame in 0..input.block_size {
            let prev_floor = self.tick_accumulator.floor();
            self.tick_accumulator += tps;

            if self.tick_accumulator.floor() > prev_floor {
                // emit_transport needs &mut self for first_tick flag — split borrows
                let sample_offset = frame as u32;
                let bpm = effective_bpm;
                self.emit_transport(sample_offset, bpm, output);
                self.advance_position();
            }
        }
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        buf.push(("/transport/bpm".to_string(),     StateBusValue::Float(self.bpm)));
        buf.push(("/transport/bar".to_string(),     StateBusValue::Int(self.bar as i64)));
        buf.push(("/transport/beat".to_string(),    StateBusValue::Int(self.beat as i64)));
        buf.push(("/transport/tick".to_string(),    StateBusValue::Int(self.tick as i64)));
        buf.push(("/transport/playing".to_string(), StateBusValue::Bool(self.playing)));
    }
}

impl TempoSource for InternalClock {
    fn domain_id(&self) -> u32 {
        self.domain_id_val
    }

    fn priority(&self) -> ClockPriority {
        ClockPriority::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, Event, ExtendedEventSlab, TransportInfo, Node,
        ProcessInput, ProcessOutput,
    };

    fn run_internal_clock(node: &mut InternalClock, block_size: usize) -> Vec<Event> {
        run_internal_clock_with_events(node, block_size, &[])
    }

    fn run_internal_clock_with_events(
        node: &mut InternalClock,
        block_size: usize,
        in_events: &[paraclete_node_api::TimedEvent],
    ) -> Vec<Event> {
        let mut audio = AudioBuffer::new(2, block_size);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];

        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: in_events,
            transport: &transport,
            sample_rate: 44100.0,
            block_size,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        node.process(&input, &mut output);
        events_out.as_slice().iter().map(|e| e.event).collect()
    }

    #[test]
    fn internal_clock_new_has_bpm_120() {
        let node = InternalClock::new();
        assert_eq!(node.bpm, 120.0);
    }

    #[test]
    fn internal_clock_emits_transport_events_each_cycle() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);
        let events = run_internal_clock(&mut node, 512);
        // At 120 BPM, 44100 Hz: ticks/sample = 120/60 * 960 / 44100 ≈ 0.0435
        // In 512 samples: ~22 ticks → should have ~22 TransportEvents
        assert!(!events.is_empty(), "expected transport events but got none");
        assert!(events.iter().all(|e| matches!(e, Event::Transport(_))));
    }

    #[test]
    fn internal_clock_emits_sync_pulse_at_bar_start() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        // Run enough cycles to hit a bar start (bar=1, beat=0, tick=0 is the initial start)
        // The very first tick emitted should have sync_pulse = true (it starts at bar 1, beat 0, tick 0)
        let events = run_internal_clock(&mut node, 512);
        let has_sync = events.iter().any(|e| {
            if let Event::Transport(k) = e { k.flags.sync_pulse } else { false }
        });
        assert!(has_sync, "expected sync_pulse on bar start");
    }

    #[test]
    fn internal_clock_domain_id_and_priority() {
        let node = InternalClock::with_domain(7);
        assert_eq!(node.domain_id(), 7);
        assert_eq!(node.priority(), ClockPriority::Internal);
    }

    #[test]
    fn node_type_name_default_is_nonempty() {
        let node = InternalClock::new();
        let name = node.type_name();
        assert!(!name.is_empty(), "type_name() must return a non-empty string");
    }

    #[test]
    fn internal_clock_published_state_includes_bpm() {
        let node = InternalClock::new();
        let mut state = Vec::new();
        node.published_state(&mut state);
        let bpm_entry = state.iter().find(|(k, _)| k == "/transport/bpm");
        assert!(bpm_entry.is_some());
        if let Some((_, v)) = bpm_entry {
            assert!(matches!(v, paraclete_node_api::StateBusValue::Float(_)));
        }
    }

    #[test]
    fn internal_clock_stops_on_global_stop_event() {
        use paraclete_node_api::{TimedEvent, TransportEvent, TransportFlags, TICKS_PER_BEAT};

        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let first = run_internal_clock(&mut node, 512);
        assert!(!first.is_empty(), "clock must emit ticks before stop");

        let stop_event = TimedEvent::new(0, Event::Transport(TransportEvent {
            domain_id: 0, bar: 1, beat: 0, tick: 0,
            ticks_per_beat: TICKS_PER_BEAT, bpm: 120.0,
            time_sig_num: 4, time_sig_den: 4,
            flags: TransportFlags { global_stop: true, ..TransportFlags::default() },
        }));
        let second = run_internal_clock_with_events(&mut node, 512, &[stop_event]);
        assert!(
            second.is_empty(),
            "clock must not emit ticks after GlobalStop (got {} events)",
            second.len()
        );
    }

    #[test]
    fn internal_clock_resumes_after_global_stop_then_global_start() {
        use paraclete_node_api::{TimedEvent, TransportEvent, TransportFlags, TICKS_PER_BEAT};

        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let stop_event = TimedEvent::new(0, Event::Transport(TransportEvent {
            domain_id: 0, bar: 1, beat: 0, tick: 0,
            ticks_per_beat: TICKS_PER_BEAT, bpm: 120.0,
            time_sig_num: 4, time_sig_den: 4,
            flags: TransportFlags { global_stop: true, ..TransportFlags::default() },
        }));
        let silent = run_internal_clock_with_events(&mut node, 512, &[stop_event]);
        assert!(silent.is_empty(), "must be silent after stop");

        let start_event = TimedEvent::new(0, Event::Transport(TransportEvent {
            domain_id: 0, bar: 1, beat: 0, tick: 0,
            ticks_per_beat: TICKS_PER_BEAT, bpm: 120.0,
            time_sig_num: 4, time_sig_den: 4,
            flags: TransportFlags { global_start: true, playing: true, ..TransportFlags::default() },
        }));
        let resumed = run_internal_clock_with_events(&mut node, 512, &[start_event]);
        assert!(
            !resumed.is_empty(),
            "clock must emit ticks after GlobalStart following a stop"
        );
    }
}
