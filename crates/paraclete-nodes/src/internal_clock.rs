use paraclete_node_api::{
    CapabilityDocument, Event, ClockPriority, TransportEvent, TransportFlags, StateBusValue, Node,
    ParamDescriptor, ParamUnit, TempoSource, PortDescriptor, PortDirection, PortType,
    ProcessInput, ProcessOutput, NodeCommand, CMD_SET_PARAM, CMD_BUMP_PARAM,
    TimedEvent, TICKS_PER_BEAT,
};

pub const CMD_CLOCK_START: u32 = 16;
pub const CMD_CLOCK_STOP:  u32 = 17;
/// ADR-046 T1: set position to the window start, independent of `playing`
/// (R3: valid while running too). Replaces the implicit rewind that used
/// to ride along with `CMD_CLOCK_START`.
pub const CMD_CLOCK_REWIND: u32 = 18;

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
            // ADR-046 T3: boots stopped — closes BUG-039 at the source.
            // Every surface used to auto-start; Theotokos's own
            // startup CMD_CLOCK_STOP (ADR-044 D7) was a surface-side
            // workaround for this and is retired in the same phase
            // (TK2.2 C5) that lands this, not left as a second mechanism
            // for the same invariant.
            playing: false,
            tick_accumulator: 0.0,
            sample_rate: 44100.0,
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
                    // ADR-046 T1/T2: a natural per-sample tick is never a
                    // rewind — only the dedicated `emit_rewind_event` carries
                    // that flag, on an explicit CMD_CLOCK_REWIND.
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

    /// Returns `(saw_stop_command, saw_rewind_command)`. The stop half:
    /// whether a `CMD_CLOCK_STOP` transitioned `self.playing` from true to
    /// false — the caller combines this with the *final* `self.playing`
    /// (after commands AND incoming events) to decide whether to emit
    /// `global_stop` (BUG-041), so a STOP reversed later in the same batch
    /// — by a `CMD_CLOCK_START` here or an incoming `global_rewind` event —
    /// does not emit a stale, spurious stop (hostile review finding).
    /// Scoped to the command path only: a bare incoming `global_stop` event
    /// (unreachable in today's single-domain graph) is unchanged, matching
    /// BUG-041's fix direction.
    ///
    /// ADR-046 T1: `CMD_CLOCK_START` no longer implies a rewind — it only
    /// ever sets `playing = true`. `CMD_CLOCK_REWIND` (R3: valid regardless
    /// of `playing`) is tracked separately and always signalled via
    /// `emit_rewind_event`, never folded into the natural tick stream.
    fn handle_commands(&mut self, commands: &[NodeCommand]) -> (bool, bool) {
        let bpm_id = ParamDescriptor::id_for_name(Self::PARAM_BPM);
        let mut saw_stop_command = false;
        let mut saw_rewind_command = false;
        for cmd in commands {
            match cmd.type_id {
                CMD_CLOCK_START => {
                    self.playing = true;
                }
                CMD_CLOCK_STOP => {
                    if self.playing {
                        saw_stop_command = true;
                    }
                    self.playing = false;
                }
                CMD_CLOCK_REWIND => {
                    saw_rewind_command = true;
                }
                CMD_SET_PARAM => {
                    if cmd.arg0 as u32 == bpm_id {
                        self.bpm = cmd.arg1.clamp(20.0, 300.0);
                    }
                }
                CMD_BUMP_PARAM => {
                    if cmd.arg0 as u32 == bpm_id {
                        self.bpm = (self.bpm + cmd.arg1).clamp(20.0, 300.0);
                    }
                }
                _ => {}
            }
        }
        (saw_stop_command, saw_rewind_command)
    }

    /// Mirror of `emit_transport`'s emission (BUG-041): the one event
    /// downstream nodes (Sequencer) need to clear their own `playing` flag.
    fn emit_stop_event(&mut self, output: &mut ProcessOutput) {
        output.events_out.push(TimedEvent::new(
            0,
            Event::Transport(TransportEvent {
                domain_id: self.domain_id_val,
                bar: self.bar,
                beat: self.beat,
                tick: self.tick,
                ticks_per_beat: TICKS_PER_BEAT,
                bpm: self.bpm,
                time_sig_num: self.time_sig_num,
                time_sig_den: self.time_sig_den,
                flags: TransportFlags {
                    playing: false,
                    global_stop: true,
                    ..TransportFlags::default()
                },
            }),
        ));
    }

    /// ADR-046 T1/R3: one immediate event carrying `global_rewind`,
    /// regardless of `playing` — a rewind must relocate downstream nodes
    /// even while stopped (silently: no note, since the entry-step fire is
    /// gated on the transition into playing, not on rewind, at the
    /// sequencer). `playing` on the event mirrors current state so a
    /// rewind-while-running is distinguishable from a rewind-while-stopped
    /// by any listener.
    fn emit_rewind_event(&mut self, output: &mut ProcessOutput) {
        output.events_out.push(TimedEvent::new(
            0,
            Event::Transport(TransportEvent {
                domain_id: self.domain_id_val,
                bar: self.bar,
                beat: self.beat,
                tick: self.tick,
                ticks_per_beat: TICKS_PER_BEAT,
                bpm: self.bpm,
                time_sig_num: self.time_sig_num,
                time_sig_den: self.time_sig_den,
                flags: TransportFlags {
                    playing: self.playing,
                    global_rewind: true,
                    ..TransportFlags::default()
                },
            }),
        ));
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
    view: None,
        }
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let (saw_stop_command, saw_rewind_command) = self.handle_commands(input.commands);

        // Cross-domain sync (unexercised in today's single-domain graph,
        // out of ADR-046's scope — mechanically renamed only): an incoming
        // global_rewind from another clock domain still also starts this
        // one if it wasn't already playing, preserving prior behaviour.
        for timed in input.events {
            if let Event::Transport(te) = timed.event {
                if te.flags.global_stop {
                    self.playing = false;
                } else if te.flags.global_rewind && !self.playing {
                    self.playing = true;
                }
            }
        }

        // BUG-041: gate on the FINAL playing state, not a mid-batch flag —
        // a STOP reversed later in the same batch (a CMD_CLOCK_START here,
        // or an incoming global_rewind event) must not emit a spurious
        // global_stop the caller never asked for (hostile review finding).
        if saw_stop_command && !self.playing {
            self.emit_stop_event(output);
        }

        // ADR-046 T1/R3: a rewind is signalled regardless of `playing` —
        // unlike the tick loop below, this must not be gated on
        // `!self.playing { return; }`, or a rewind-while-stopped would
        // never reach downstream nodes.
        if saw_rewind_command {
            self.emit_rewind_event(output);
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

    fn run_internal_clock_with_events(
        node: &mut InternalClock,
        block_size: usize,
        in_events: &[paraclete_node_api::TimedEvent],
    ) -> Vec<Event> {
        run_internal_clock_with_commands(node, block_size, in_events, &[])
    }

    fn run_internal_clock_with_commands(
        node: &mut InternalClock,
        block_size: usize,
        in_events: &[paraclete_node_api::TimedEvent],
        commands: &[NodeCommand],
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
            commands,
        };
        let mut output = ProcessOutput::new(
            &mut outs,
            &mut [],
            &mut events_out,
        );
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
        // ADR-046 T3: boots stopped — must be started explicitly.
        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        let events = run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
        // At 120 BPM, 44100 Hz: ticks/sample = 120/60 * 960 / 44100 ≈ 0.0435
        // In 512 samples: ~22 ticks → should have ~22 TransportEvents
        assert!(!events.is_empty(), "expected transport events but got none");
        assert!(events.iter().all(|e| matches!(e, Event::Transport(_))));
    }

    #[test]
    fn internal_clock_emits_sync_pulse_at_bar_start() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        // ADR-046 T3: boots stopped — must be started explicitly. Run
        // enough cycles to hit a bar start (bar=1, beat=0, tick=0 is the
        // initial start) — the very first tick emitted should have
        // sync_pulse = true (it starts at bar 1, beat 0, tick 0).
        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        let events = run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
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

        // ADR-046 T3: boots stopped — must be started explicitly before
        // this test's own premise ("clock must emit ticks before stop")
        // holds.
        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        let first = run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
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
    fn internal_clock_resumes_after_global_stop_then_global_rewind() {
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

        // ADR-046 (out-of-scope cross-domain path, mechanically renamed
        // only, see `process()`'s comment): an incoming global_rewind with
        // `playing: true` still also starts this clock if it wasn't
        // already playing, preserving pre-ADR-046 behaviour for this path.
        let rewind_event = TimedEvent::new(0, Event::Transport(TransportEvent {
            domain_id: 0, bar: 1, beat: 0, tick: 0,
            ticks_per_beat: TICKS_PER_BEAT, bpm: 120.0,
            time_sig_num: 4, time_sig_den: 4,
            flags: TransportFlags { global_rewind: true, playing: true, ..TransportFlags::default() },
        }));
        let resumed = run_internal_clock_with_events(&mut node, 512, &[rewind_event]);
        assert!(
            !resumed.is_empty(),
            "clock must emit ticks after an incoming global_rewind following a stop"
        );
    }

    #[test]
    fn clock_stop_reversed_in_same_batch_does_not_emit_global_stop() {
        // Hostile review finding on BUG-041: a STOP immediately reversed by
        // a START in the SAME command batch (or alongside an incoming
        // global_start event) must not emit a spurious global_stop — the
        // net effect across the whole process() call is "never stopped".
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let stop = NodeCommand { target_id: 0, type_id: CMD_CLOCK_STOP, arg0: 0, arg1: 0.0 };
        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        let events = run_internal_clock_with_commands(&mut node, 512, &[], &[stop, start]);

        assert!(node.playing, "net effect of STOP then START must be playing");
        let has_global_stop = events.iter().any(|e| {
            matches!(e, Event::Transport(te) if te.flags.global_stop)
        });
        assert!(
            !has_global_stop,
            "a STOP reversed within the same batch must not emit global_stop"
        );
    }

    /// ADR-046 T1: `CMD_CLOCK_START` no longer implies a rewind — replaces
    /// `clock_start_via_command_plays_and_resets_first_tick`, which
    /// asserted the retired compound behaviour (the first tick after START
    /// used to carry `global_start`). This is a statement about the new
    /// semantics, not a green-at-any-cost patch.
    #[test]
    fn clock_start_via_command_plays_without_rewinding() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        // ADR-046 T3: boots stopped, so establish a playing state first —
        // STOP against an already-stopped clock is a no-op transition
        // (see `clock_stop_is_idempotent`), not what this test is about.
        let initial_start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[initial_start]);
        assert!(node.playing, "sanity: playing before the stop under test");

        let stop = NodeCommand { target_id: 0, type_id: CMD_CLOCK_STOP, arg0: 0, arg1: 0.0 };
        let stopped = run_internal_clock_with_commands(&mut node, 512, &[], &[stop]);
        assert_eq!(stopped.len(), 1,
            "CMD_CLOCK_STOP must emit exactly one transport event on the transition to stopped (BUG-041)");
        match &stopped[0] {
            Event::Transport(te) => assert!(te.flags.global_stop,
                "the emitted event must carry global_stop"),
            other => panic!("expected Transport event, got {:?}", other),
        }
        assert!(!node.playing, "playing must be false after CMD_CLOCK_STOP");

        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        let resumed = run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
        assert!(!resumed.is_empty(), "must emit ticks after CMD_CLOCK_START");
        assert!(node.playing, "playing must be true after CMD_CLOCK_START");

        assert!(
            resumed
                .iter()
                .all(|e| matches!(e, Event::Transport(te) if !te.flags.global_rewind)),
            "CMD_CLOCK_START must never emit a global_rewind — no implicit rewind (ADR-046 T1)"
        );
        let first = &resumed[0];
        match first {
            Event::Transport(te) => assert!(te.flags.playing),
            _ => panic!("expected Transport event, got {:?}", first),
        }
    }

    /// ADR-046 T1: replaces `clock_start_is_idempotent_does_not_reset_first_tick`
    /// — with rewind fully decoupled from starting, "idempotent" now means
    /// a redundant START neither rewinds nor otherwise disturbs playback.
    #[test]
    fn clock_start_is_idempotent_and_never_rewinds() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        let first = run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
        assert!(node.playing);
        assert!(
            first
                .iter()
                .all(|e| matches!(e, Event::Transport(te) if !te.flags.global_rewind)),
            "an initial START must not emit global_rewind"
        );

        let redundant = run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
        assert!(!redundant.is_empty(), "still emits ticks");
        assert!(
            redundant
                .iter()
                .all(|e| matches!(e, Event::Transport(te) if !te.flags.global_rewind)),
            "a redundant START must not emit global_rewind either"
        );
    }

    #[test]
    fn clock_stop_is_idempotent() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let stop = NodeCommand { target_id: 0, type_id: CMD_CLOCK_STOP, arg0: 0, arg1: 0.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[stop]);
        assert!(!node.playing);

        let still_silent = run_internal_clock_with_commands(&mut node, 512, &[], &[stop]);
        assert!(still_silent.is_empty());
        assert!(!node.playing);
    }

    #[test]
    fn clock_stop_happens_after_start() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        let resumed = run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
        assert!(!resumed.is_empty());
        assert!(node.playing);

        let stop = NodeCommand { target_id: 0, type_id: CMD_CLOCK_STOP, arg0: 0, arg1: 0.0 };
        let stopped = run_internal_clock_with_commands(&mut node, 512, &[], &[stop]);
        assert_eq!(stopped.len(), 1,
            "must emit exactly the global_stop transition event, then go silent (BUG-041)");
        assert!(!node.playing);
    }

    #[test]
    fn clock_bpm_set_param_applies_and_clamps() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let bpm_id = ParamDescriptor::id_for_name(InternalClock::PARAM_BPM);
        let set_200 = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: bpm_id as i64, arg1: 200.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[set_200]);
        assert!((node.bpm - 200.0).abs() < 0.001);

        let set_over = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: bpm_id as i64, arg1: 999.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[set_over]);
        assert!((node.bpm - 300.0).abs() < 0.001, "must clamp to max 300.0");

        let set_under = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: bpm_id as i64, arg1: -10.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[set_under]);
        assert!((node.bpm - 20.0).abs() < 0.001, "must clamp to min 20.0");
    }

    #[test]
    fn clock_bpm_bump_param_applies() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let bpm_id = ParamDescriptor::id_for_name(InternalClock::PARAM_BPM);
        let init = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: bpm_id as i64, arg1: 140.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[init]);
        assert!((node.bpm - 140.0).abs() < 0.001);

        let up = NodeCommand { target_id: 0, type_id: CMD_BUMP_PARAM, arg0: bpm_id as i64, arg1: 5.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[up]);
        assert!((node.bpm - 145.0).abs() < 0.001);

        let down = NodeCommand { target_id: 0, type_id: CMD_BUMP_PARAM, arg0: bpm_id as i64, arg1: -3.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[down]);
        assert!((node.bpm - 142.0).abs() < 0.001);

        let big_down = NodeCommand { target_id: 0, type_id: CMD_BUMP_PARAM, arg0: bpm_id as i64, arg1: -999.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[big_down]);
        assert!((node.bpm - 20.0).abs() < 0.001, "must clamp at floor 20.0");

        let init_high = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: bpm_id as i64, arg1: 295.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[init_high]);
        assert!((node.bpm - 295.0).abs() < 0.001);

        let big_up = NodeCommand { target_id: 0, type_id: CMD_BUMP_PARAM, arg0: bpm_id as i64, arg1: 999.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[big_up]);
        assert!((node.bpm - 300.0).abs() < 0.001, "must clamp at ceiling 300.0");
    }

    /// ADR-046 T3 (closes BUG-039): the clock boots stopped at the source
    /// — no surface-side workaround (ADR-044 D7's startup CMD_CLOCK_STOP,
    /// retired in the same phase) is needed to keep the instrument silent.
    #[test]
    fn internal_clock_boots_stopped() {
        let node = InternalClock::new();
        assert!(
            !node.playing,
            "a fresh InternalClock must boot stopped (BUG-039)"
        );
    }

    /// ADR-046 T1/R3: a rewind while stopped must relocate (signal
    /// downstream) but emit nothing audible — the entry-step fire lives at
    /// the sequencer, gated on the transition into playing, not on
    /// rewind. Here at the clock, "emits nothing audible" means the
    /// emitted event carries `playing: false`, matching the stopped state.
    #[test]
    fn rewind_while_stopped_emits_one_event_with_playing_false() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);
        assert!(!node.playing, "sanity: boots stopped");

        let rewind = NodeCommand { target_id: 0, type_id: CMD_CLOCK_REWIND, arg0: 0, arg1: 0.0 };
        let events = run_internal_clock_with_commands(&mut node, 512, &[], &[rewind]);

        assert_eq!(
            events.len(),
            1,
            "a rewind while stopped must emit exactly one event, not start the tick stream"
        );
        match &events[0] {
            Event::Transport(te) => {
                assert!(te.flags.global_rewind, "the event must carry global_rewind");
                assert!(
                    !te.flags.playing,
                    "must reflect the stopped state, not start it"
                );
            }
            other => panic!("expected Transport event, got {:?}", other),
        }
        assert!(
            !node.playing,
            "CMD_CLOCK_REWIND alone must not start playback"
        );
    }

    /// ADR-046 R3: rewind is valid while running — a musical "return to
    /// top" that keeps playing, without stopping the tick stream.
    #[test]
    fn rewind_while_running_keeps_playing_and_relocates() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);

        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
        assert!(node.playing, "sanity: running");

        let rewind = NodeCommand { target_id: 0, type_id: CMD_CLOCK_REWIND, arg0: 0, arg1: 0.0 };
        let events = run_internal_clock_with_commands(&mut node, 512, &[], &[rewind]);

        assert!(
            node.playing,
            "a rewind while running must not stop the clock"
        );
        let rewind_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::Transport(te) if te.flags.global_rewind))
            .collect();
        assert_eq!(
            rewind_events.len(),
            1,
            "exactly one rewind event, not a rewind per subsequent tick"
        );
        match rewind_events[0] {
            Event::Transport(te) => assert!(te.flags.playing, "must reflect the running state"),
            _ => unreachable!(),
        }
    }

    /// ADR-046 T4/R4: clock-level `playing`, `bar`, `beat`, `tick` are
    /// published so any surface can render the transport from state alone.
    #[test]
    fn published_state_includes_playing_and_position() {
        let mut node = InternalClock::new();
        node.activate(44100.0, 512);
        let mut state = Vec::new();
        node.published_state(&mut state);

        let playing = state.iter().find(|(k, _)| k == "/transport/playing");
        assert_eq!(
            playing.map(|(_, v)| v.clone()),
            Some(paraclete_node_api::StateBusValue::Bool(false)),
            "a fresh clock must publish playing=false (T3)"
        );
        for path in ["/transport/bar", "/transport/beat", "/transport/tick"] {
            assert!(
                state.iter().any(|(k, _)| k == path),
                "{path} must be published so a surface can render position from state alone"
            );
        }

        let start = NodeCommand { target_id: 0, type_id: CMD_CLOCK_START, arg0: 0, arg1: 0.0 };
        run_internal_clock_with_commands(&mut node, 512, &[], &[start]);
        let mut state_after = Vec::new();
        node.published_state(&mut state_after);
        assert_eq!(
            state_after
                .iter()
                .find(|(k, _)| k == "/transport/playing")
                .map(|(_, v)| v.clone()),
            Some(paraclete_node_api::StateBusValue::Bool(true)),
            "published playing must track the actual state after a start"
        );
    }
}
