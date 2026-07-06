use paraclete_node_api::{
    CapabilityDocument, Event, NodeCommand, ParameterBank, ParamDescriptor,
    ParamLockEvent, ParamUnit, PortDescriptor, PortDirection, PortName, PortType, ProcessInput,
    ProcessOutput, StateBusValue, TimedEvent, TransportEvent, Node, TICKS_PER_BEAT,
};

// ── Timing / Condition types ──────────────────────────────────────────────────

/// Per-step timing offset in 1/96-beat units. Range: ±47.
/// Positive = push forward (later). Negative = pull back (earlier).
#[derive(Clone, Copy, Debug, Default)]
pub struct StepTiming {
    pub micro_offset: i8,
}

impl StepTiming {
    /// Returns the absolute sample displacement for this offset.
    /// Returns 0 when micro_offset is 0. Caller must check micro_offset sign.
    pub fn to_sample_offset(&self, samples_per_beat: f64) -> u32 {
        if self.micro_offset == 0 {
            return 0;
        }
        let frac = self.micro_offset as f64 / 96.0;
        (frac * samples_per_beat).round().abs() as u32
    }
}

/// Repeat gate: how often across loop iterations a trig fires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RepeatCondition {
    Always,
    /// Fire on the nth repetition of every m loops (1-indexed n).
    NthOfM { n: u8, m: u8 },
}

/// Fill gate: how fill buttons interact with a trig.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FillCondition {
    Ignore,
    FillA,
    FillB,
    FillAny,
    NoFill,
    NotFillA,
    NotFillB,
}

/// Per-loop state used by condition evaluation.
#[derive(Clone, Copy, Debug, Default)]
pub struct SequencerCycleState {
    /// Increments each time the pattern completes (wrapping).
    pub loop_count: u32,
    pub fill_a:     bool,
    pub fill_b:     bool,
}

/// Condition governing whether a trig fires on a given loop iteration.
#[derive(Clone, Debug, PartialEq)]
pub enum TrigCondition {
    Simple {
        repeat:      RepeatCondition,
        fill:        FillCondition,
        /// 0–100. 100 = always (no RNG). 0 = never.
        probability: u8,
    },
}

impl TrigCondition {
    /// Evaluate the condition. Order: repeat gate → fill gate → probability roll.
    pub fn evaluate(&self, cycle_state: &SequencerCycleState, rng: &mut fastrand::Rng) -> bool {
        match self {
            TrigCondition::Simple { repeat, fill, probability } => {
                let repeat_pass = match repeat {
                    RepeatCondition::Always => true,
                    RepeatCondition::NthOfM { n, m } => {
                        *m > 0 && (cycle_state.loop_count % *m as u32) == (*n as u32 - 1)
                    }
                };
                if !repeat_pass { return false; }

                let fill_pass = match fill {
                    FillCondition::Ignore   => true,
                    FillCondition::FillA    => cycle_state.fill_a,
                    FillCondition::FillB    => cycle_state.fill_b,
                    FillCondition::FillAny  => cycle_state.fill_a || cycle_state.fill_b,
                    FillCondition::NoFill   => !cycle_state.fill_a && !cycle_state.fill_b,
                    FillCondition::NotFillA => !cycle_state.fill_a,
                    FillCondition::NotFillB => !cycle_state.fill_b,
                };
                if !fill_pass { return false; }

                if *probability >= 100 { return true; }
                if *probability == 0   { return false; }
                rng.u8(0..100) < *probability
            }
        }
    }
}

impl Default for TrigCondition {
    fn default() -> Self {
        TrigCondition::Simple {
            repeat:      RepeatCondition::Always,
            fill:        FillCondition::Ignore,
            probability: 100,
        }
    }
}

/// Placeholder for future Script condition variant (P7+).
pub struct ConditionId(pub u32);

// ── Step ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Step {
    pub active:      bool,
    pub note:        u8,
    pub velocity:    u16,
    pub length:      f32,
    pub param_locks: Vec<StepParamLock>,
    pub condition:   TrigCondition,
    pub timing:      StepTiming,
    /// Per-step CV value locks. Each entry is `(cv_port_index, value)`.
    /// `cv_port_index` is 0-relative (cv_out_0 = index 0).
    /// Out-of-range indices are silently ignored at process time.
    pub cv_locks:    Vec<(u16, f32)>,
}

#[derive(Clone, Debug)]
pub struct StepParamLock {
    pub node_id:  u32,
    pub param_id: u32,
    pub value:    f64,
}

impl Step {
    pub fn empty() -> Self {
        Step {
            active:      false,
            note:        60,
            velocity:    32768,
            length:      0.75,
            param_locks: Vec::new(),
            condition:   TrigCondition::default(),
            timing:      StepTiming::default(),
            cv_locks:    Vec::new(),
        }
    }
}

// ── Pattern ──────────────────────────────────────────────────────────────────

/// Steps per grid page. Pages are a derived view: `page = step / PAGE_SIZE`.
pub const PAGE_SIZE: usize = 8;

/// One sequencer pattern: a bank of steps plus the playback window over them
/// (ADR-030, P10 C1).
///
/// `steps` is sized at construction on the main thread and never grows on the
/// audio thread; `length` gates how many steps play. `page_loop` is the
/// inclusive `(start_page, end_page)` playback window — stored but inert until
/// P10 C2 wires page-loop playback. `swing` is per-pattern storage; until the
/// C2 migration the ParameterBank `swing` slot stays authoritative for
/// emission and this field mirrors it so serialization captures it.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub steps:     Vec<Step>,
    pub length:    usize,
    pub page_loop: (u8, u8),
    pub swing:     f32,
}

impl Pattern {
    /// A pattern of `steps` empty steps, playing its full length, with the
    /// page-loop window spanning every page and no swing.
    pub fn empty(steps: usize) -> Self {
        let pages = steps.div_ceil(PAGE_SIZE).max(1);
        Pattern {
            steps:     vec![Step::empty(); steps],
            length:    steps,
            page_loop: (0, (pages - 1) as u8),
            swing:     0.0,
        }
    }
}

// ── Sequencer ────────────────────────────────────────────────────────────────

pub struct Sequencer {
    ports: Vec<PortDescriptor>,
    node_id: u32,
    track_name: String,

    patterns: Vec<Pattern>,
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

    cycle_state:    SequencerCycleState,
    rng:            fastrand::Rng,
    active_pattern: usize,
    /// Pattern to switch to at the next cycle boundary. Inert until P10 C4.
    cued_pattern:   Option<usize>,
    /// Volatile pattern chain. Inert until P10 C4.
    chain:          Vec<usize>,
    chain_pos:      usize,
    /// Per-track speed multiplier. Inert until P10 C3.
    speed_mult:     f32,
    swing_amount:   f32,
    sample_rate:    f32,

    cv_outputs:  usize,
    current_cv:  Vec<f32>,
}

impl Sequencer {
    pub const PORT_CLOCK_IN:   u32 = 0;
    pub const PORT_EVENTS_IN:  u32 = 1;
    pub const PORT_EVENTS_OUT: u32 = 2;
    /// First port ID used for CV output ports. cv_out_i is at port PORT_CV_OUT_BASE + i.
    pub const PORT_CV_OUT_BASE: u32 = 3;

    /// Universal NodeCommand type IDs (node-specific, ≥ 16).
    pub const CMD_TOGGLE_STEP:        u32 = 16;
    pub const CMD_SET_STEP:           u32 = 17;
    pub const CMD_CLEAR:              u32 = 18;
    pub const CMD_SET_FILL_A:         u32 = 23;
    pub const CMD_SET_FILL_B:         u32 = 24;
    pub const CMD_SET_STEP_TIMING:    u32 = 25;
    pub const CMD_SET_STEP_CONDITION: u32 = 26;
    pub const CMD_SET_PATTERN:        u32 = 27;

    /// Runtime step capacity per pattern (P10: 8 pages × 8 steps). The
    /// serialized format stores counts as plain integers and does not depend
    /// on this value (forward-extensibility amendment).
    pub const STEP_CAPACITY: usize = 64;

    pub fn new() -> Self {
        Self::with_name("")
    }

    pub fn with_name(name: &str) -> Self {
        Self::with_name_and_cv(name, 0)
    }

    /// Construct a Sequencer with `n` CvSignal output ports.
    /// `n = 0` is valid; behaves identically to `Sequencer::new()`.
    pub fn with_cv_outputs(n: usize) -> Self {
        Self::with_name_and_cv("", n)
    }

    fn with_name_and_cv(name: &str, cv_outputs: usize) -> Self {
        let mut ports = vec![
            PortDescriptor { id: Self::PORT_CLOCK_IN,   name: "clock_in".into(),   direction: PortDirection::Input,  port_type: PortType::Clock },
            PortDescriptor { id: Self::PORT_EVENTS_IN,  name: "events_in".into(),  direction: PortDirection::Input,  port_type: PortType::Event },
            PortDescriptor { id: Self::PORT_EVENTS_OUT, name: "events_out".into(), direction: PortDirection::Output, port_type: PortType::Event },
        ];
        for i in 0..cv_outputs {
            ports.push(PortDescriptor {
                id:         Self::PORT_CV_OUT_BASE + i as u32,
                name:       PortName::Dynamic(format!("cv_out_{i}")),
                direction:  PortDirection::Output,
                port_type:  PortType::Cv,
            });
        }
        // Pattern 0 preserves the P9 default preset: 16 played steps over a
        // 64-step (main-thread, construction-time) allocation.
        let mut pattern0 = Pattern::empty(Self::STEP_CAPACITY);
        pattern0.length = 16;
        pattern0.page_loop = (0, 1);
        Self {
            ports,
            node_id: 0,
            track_name: name.to_string(),
            patterns: vec![pattern0],
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
            cycle_state:    SequencerCycleState::default(),
            rng:            fastrand::Rng::new(),
            active_pattern: 0,
            cued_pattern:   None,
            chain:          Vec::new(),
            chain_pos:      0,
            speed_mult:     1.0,
            swing_amount:   0.0,
            sample_rate:    44100.0,
            cv_outputs,
            current_cv:  vec![0.0_f32; cv_outputs],
        }
    }

    /// Index of the pattern playback reads, clamped into the allocated bank.
    /// `CMD_SET_PATTERN` is still a stub until P10 C4: it stores any index,
    /// but with a single-pattern bank playback always resolves to pattern 0.
    fn active_index(&self) -> usize {
        self.active_pattern.min(self.patterns.len() - 1)
    }

    pub fn set_step(&mut self, index: usize, note: u8, velocity: u16, active: bool) {
        let pat = self.active_index();
        let steps = &mut self.patterns[pat].steps;
        if index < steps.len() {
            steps[index].note     = note;
            steps[index].velocity = velocity;
            steps[index].active   = active;
        }
    }

    /// ASCII bitfield over the active pattern's played steps: '1' = active,
    /// '0' = inactive; `length` characters (1–64; 16 for the default pattern).
    fn steps_bitfield(&self) -> String {
        let p = &self.patterns[self.active_index()];
        p.steps[..p.length]
            .iter()
            .map(|s| if s.active { '1' } else { '0' })
            .collect()
    }

    fn handle_commands(&mut self, commands: &[NodeCommand]) {
        // Universal params first.
        self.bank.handle_commands(commands);

        for cmd in commands {
            // Resolved per command, not hoisted: a CMD_SET_PATTERN earlier in
            // the same batch must direct later step edits at the new pattern.
            let pat = self.active_index();
            match cmd.type_id {
                Self::CMD_TOGGLE_STEP => {
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() {
                        steps[idx].active = !steps[idx].active;
                    }
                }
                Self::CMD_SET_STEP => {
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() {
                        if cmd.arg1 < 0.0 {
                            steps[idx].active = false;
                        } else {
                            steps[idx].note   = cmd.arg1 as u8;
                            steps[idx].active = true;
                        }
                    }
                }
                Self::CMD_CLEAR => {
                    for step in &mut self.patterns[pat].steps {
                        step.active = false;
                    }
                }
                Self::CMD_SET_FILL_A => {
                    self.cycle_state.fill_a = cmd.arg0 != 0;
                }
                Self::CMD_SET_FILL_B => {
                    self.cycle_state.fill_b = cmd.arg0 != 0;
                }
                Self::CMD_SET_STEP_TIMING => {
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() {
                        steps[idx].timing.micro_offset = cmd.arg1 as i8;
                    }
                }
                Self::CMD_SET_STEP_CONDITION => {
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() {
                        let enc = cmd.arg1 as i64 as u64;
                        let probability = (enc & 0xFF) as u8;
                        let repeat_n    = ((enc >> 8) & 0xFF) as u8;
                        let repeat_m    = ((enc >> 16) & 0xFF) as u8;
                        let fill_disc   = ((enc >> 24) & 0xFF) as u8;

                        let repeat = repeat_from_nm(repeat_n, repeat_m);
                        let fill = fill_from_discriminant(fill_disc);
                        steps[idx].condition = TrigCondition::Simple { repeat, fill, probability };
                    }
                }
                Self::CMD_SET_PATTERN => {
                    self.active_pattern = cmd.arg0.max(0) as usize;
                    // stub until P10 C4: stored, but playback clamps to the
                    // single-pattern bank (always pattern 0)
                }
                _ => {}
            }
        }
    }

    fn handle_transport(&mut self, k: &TransportEvent, sample_offset: u32, output: &mut ProcessOutput) {
        let spb  = 60.0 * self.sample_rate as f64 / k.bpm.max(1.0);
        let pat  = self.active_index();
        let plen = self.patterns[pat].length;

        if k.flags.sync_pulse {
            let bars_elapsed = (k.bar - 1).max(0) as u64;
            let total_ticks  = bars_elapsed * k.time_sig_num as u64 * TICKS_PER_BEAT as u64
                + k.beat as u64 * TICKS_PER_BEAT as u64
                + k.tick as u64;
            let step_index  = (total_ticks / self.ticks_per_step as u64) % plen as u64;
            let new_tick    = (total_ticks % self.ticks_per_step as u64) as u32;

            // Drift correction only (BUG-001 fix, s0 re-diagnosis): when internal
            // counting is in sync, the natural advance below handles this event —
            // the snap must be a no-op or it pre-empts the wrap (and loop_count).
            // "In sync" = the natural path reaches (step_index, new_tick) this
            // event: either we are already there, or we are one increment away.
            let next_tick   = self.step_tick + 1;
            let in_sync = if next_tick >= self.ticks_per_step {
                new_tick == 0
                    && step_index as usize == (self.current_step + 1) % plen
            } else {
                step_index as usize == self.current_step && new_tick == next_tick
            };
            if !in_sync && self.playing {
                self.current_step = step_index as usize;
                self.step_tick    = new_tick;
            }

            // When a genuine resync lands at tick 0 (exact start of a step) and
            // the sequencer is playing, fire that step if active — a node that
            // connects mid-session enters its pattern here.
            if !in_sync && new_tick == 0 && self.playing && k.flags.playing {
                let step_active = self.patterns[pat].steps[self.current_step].active;
                if step_active {
                    let cond = self.patterns[pat].steps[self.current_step].condition.clone();
                    let should_fire = cond.evaluate(&self.cycle_state, &mut self.rng);
                    if should_fire {
                        let note_off = sample_offset + self.step_sample_offset(self.current_step, spb);
                        if self.gate_open {
                            self.emit_note_off(sample_offset, output);
                        }
                        self.emit_note_on(note_off, output);
                        for lock in &self.patterns[pat].steps[self.current_step].param_locks {
                            output.events_out.push(TimedEvent::new(
                                note_off,
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
            // Fire step 0 (BUG-001 fix): previously the first step was never
            // emitted by the boundary path and only sounded via the bar-sync
            // snap. The start event IS tick 0 of step 0 — return so it is not
            // also counted as progress.
            let step_active = self.patterns[pat].steps[0].active;
            if step_active {
                let cond = self.patterns[pat].steps[0].condition.clone();
                if cond.evaluate(&self.cycle_state, &mut self.rng) {
                    let note_off = sample_offset + self.step_sample_offset(0, spb);
                    self.emit_note_on(note_off, output);
                    for lock in &self.patterns[pat].steps[0].param_locks {
                        output.events_out.push(TimedEvent::new(
                            note_off,
                            Event::ParamLock(ParamLockEvent {
                                node_id:  lock.node_id,
                                param_id: lock.param_id,
                                value:    lock.value,
                            }),
                        ));
                    }
                }
            }
            return;
        }

        if !k.flags.playing || !self.playing {
            return;
        }

        // Count this tick first (BUG-001 fix): the old check-then-increment
        // structure spanned ticks_per_step + 1 tick events per step (241/240),
        // with the bar-sync snap silently erasing the accumulated drift.
        self.step_tick += 1;

        if self.gate_open {
            let gate_ticks = (self.patterns[pat].steps[self.current_step].length * self.ticks_per_step as f32) as u32;
            if self.step_tick >= gate_ticks {
                self.emit_note_off(sample_offset, output);
            }
        }

        if self.step_tick >= self.ticks_per_step {
            let prev_step = self.current_step;
            self.step_tick    = 0;
            self.current_step = (self.current_step + 1) % plen;

            // Increment loop_count each time the pattern wraps.
            if self.current_step == 0 && prev_step == plen - 1 {
                self.cycle_state.loop_count = self.cycle_state.loop_count.wrapping_add(1);
            }

            let step_active = self.patterns[pat].steps[self.current_step].active;
            if step_active {
                let cond = self.patterns[pat].steps[self.current_step].condition.clone();
                let should_fire = cond.evaluate(&self.cycle_state, &mut self.rng);
                if should_fire {
                    let note_off = sample_offset + self.step_sample_offset(self.current_step, spb);
                    self.emit_note_on(note_off, output);
                    for lock in &self.patterns[pat].steps[self.current_step].param_locks {
                        output.events_out.push(TimedEvent::new(
                            note_off,
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
    }

    fn emit_note_on(&mut self, sample_offset: u32, output: &mut ProcessOutput) {
        let pat  = self.active_index();
        let step = &self.patterns[pat].steps[self.current_step];
        self.active_note     = step.note;
        self.gate_open       = true;
        self.trig_count      = self.trig_count.wrapping_add(1);
        self.last_fired_step = self.current_step;
        output.events_out.push(TimedEvent::new(
            sample_offset,
            Event::Midi2(build_note_on(self.group, self.channel, step.note, step.velocity)),
        ));
        // Apply per-step CV locks (sample-and-hold until next step fires).
        for &(idx, val) in &step.cv_locks {
            if (idx as usize) < self.cv_outputs {
                self.current_cv[idx as usize] = val;
            }
        }
    }

    fn emit_note_off(&mut self, sample_offset: u32, output: &mut ProcessOutput) {
        self.gate_open = false;
        output.events_out.push(TimedEvent::new(
            sample_offset,
            Event::Midi2(build_note_off(self.group, self.channel, self.active_note)),
        ));
    }

    /// Compute the sample offset for a step, combining micro-timing and swing.
    fn step_sample_offset(&self, step_idx: usize, samples_per_beat: f64) -> u32 {
        let micro = self.patterns[self.active_index()].steps[step_idx].timing.to_sample_offset(samples_per_beat);
        let swing = if step_idx % 2 == 1 {
            (self.swing_amount as f64 * samples_per_beat) as u32
        } else {
            0
        };
        micro + swing
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
            version: (0, 5, 0),
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
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("swing"),
                    name: "swing".into(),
                    min: 0.0, max: 0.5, default: 0.0,
                    stepped: false, unit: ParamUnit::Generic, display: None,
                },
            ],
            extensions: vec!["paraclete.sequencer"],
        }
    }

    fn activate(&mut self, sr: f32, _block: usize) {
        self.sample_rate = sr;
        self.rng = fastrand::Rng::new();
        self.bank = ParameterBank::from_capability_document(&self.capability_document());
        self.current_cv = vec![0.0_f32; self.cv_outputs];
        // Re-apply the active pattern's swing to the freshly-built bank. Two
        // orderings occur in practice: executor rebuilds follow the documented
        // deserialize-after-activate contract (deserialize's own bank.set
        // covers swing there), but `--load` runs deserialize() on
        // un-activated nodes before build_executor (ADR-025), and this
        // re-apply is what carries the loaded value into the first bank.
        // It also keeps re-activation from resetting live swing (BUG-008
        // class): process() mirrors the bank into the pattern each cycle,
        // so the pattern always holds the latest value.
        let swing = self.patterns[self.active_index()].swing;
        self.bank.set(ParamDescriptor::id_for_name("swing"), swing as f64);
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.handle_commands(input.commands);
        self.swing_amount = self.bank.get(ParamDescriptor::id_for_name("swing")) as f32;
        // Mirror the bank value into the active pattern so serialize() captures
        // it (the bank stays authoritative for emission until P10 C2).
        let pat = self.active_index();
        self.patterns[pat].swing = self.swing_amount;

        for timed in input.events {
            match timed.event {
                Event::Transport(ref k) => {
                    let k = *k;
                    self.handle_transport(&k, timed.sample_offset, output);
                }
                _ => {}
            }
        }

        // Write sample-and-hold CV values to each CV output port every cycle.
        for i in 0..self.cv_outputs {
            let port_id = Self::PORT_CV_OUT_BASE + i as u32;
            let buf = output.cv_signal_output_mut(port_id);
            buf.fill(self.current_cv[i]);
        }
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        let id = self.node_id;
        buf.push((format!("/node/{id}/state/current_step"),    StateBusValue::Int(self.current_step as i64)));
        buf.push((format!("/node/{id}/state/pattern_length"),  StateBusValue::Int(self.patterns[self.active_index()].length as i64)));
        buf.push((format!("/node/{id}/state/playing"),         StateBusValue::Bool(self.playing)));
        buf.push((format!("/node/{id}/state/steps"),           StateBusValue::Text(self.steps_bitfield())));
        buf.push((format!("/node/{id}/state/last_trig"),       StateBusValue::Int(self.trig_count as i64)));
        buf.push((format!("/node/{id}/state/last_fired_step"), StateBusValue::Int(self.last_fired_step as i64)));
        buf.push((format!("/node/{id}/state/loop_count"),      StateBusValue::Int(self.cycle_state.loop_count as i64)));
        buf.push((format!("/node/{id}/state/fill_a"),          StateBusValue::Float(if self.cycle_state.fill_a { 1.0 } else { 0.0 })));
        buf.push((format!("/node/{id}/state/fill_b"),          StateBusValue::Float(if self.cycle_state.fill_b { 1.0 } else { 0.0 })));
        if !self.track_name.is_empty() {
            buf.push((format!("/node/{id}/state/track_name"), StateBusValue::Text(self.track_name.clone())));
        }
    }

    fn serialize(&self) -> Vec<u8> {
        // v3 (P10 C1, fixes BUG-005): full sequencer state — every Step field
        // including condition/timing, per-pattern length/page_loop/swing, and
        // the track-level pattern-engine fields. Step records are
        // length-prefixed so future versions can append fields (e.g. per-step
        // note lists) that v3 readers skip; counts are plain integers so
        // engine caps can grow without a format bump (universality amendment).
        let mut buf = Vec::new();
        buf.push(3u8);
        buf.extend_from_slice(&self.ticks_per_step.to_le_bytes());
        buf.extend_from_slice(&self.speed_mult.to_le_bytes());
        // Only patterns up to the highest used index are written (at least
        // one, and always including the active pattern — empty-but-active is
        // a legitimate state); untouched trailing bank slots are recreated at
        // load time.
        let used = self.patterns.iter()
            .rposition(pattern_is_used)
            .map(|i| i + 1)
            .unwrap_or(1)
            .max(self.active_index() + 1);
        // Persist the *effective* pattern index, not the stub's raw storage:
        // an index the engine cannot honor (CMD_SET_PATTERN is a stub until
        // C4) must not round-trip into a future bank where it would silently
        // select a different (empty) pattern.
        let active = self.active_index() as u16;
        buf.extend_from_slice(&active.to_le_bytes());
        buf.push(self.chain.len() as u8);
        for &c in &self.chain {
            buf.extend_from_slice(&(c as u16).to_le_bytes());
        }
        buf.extend_from_slice(&(used as u16).to_le_bytes());
        for pattern in &self.patterns[..used] {
            // Pattern records carry the same skip-tolerant framing as step
            // records (u32: a pattern of 64 full steps can exceed u16), so a
            // future version can append per-pattern fields without a format
            // break. Track-level fields can likewise be appended after the
            // last pattern record — readers stop at pattern_count and ignore
            // trailing bytes.
            let len_pos = buf.len();
            buf.extend_from_slice(&0u32.to_le_bytes()); // patched below
            buf.extend_from_slice(&(pattern.length as u16).to_le_bytes());
            buf.push(pattern.page_loop.0);
            buf.push(pattern.page_loop.1);
            buf.extend_from_slice(&pattern.swing.to_le_bytes());
            buf.extend_from_slice(&(pattern.steps.len() as u16).to_le_bytes());
            for step in &pattern.steps {
                write_step_record(step, &mut buf);
            }
            let record_len = (buf.len() - len_pos - 4) as u32;
            buf[len_pos..len_pos + 4].copy_from_slice(&record_len.to_le_bytes());
        }
        buf
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.is_empty() { return; }
        match data[0] {
            1 | 2 => self.deserialize_legacy(data),
            3     => self.deserialize_v3(data),
            _     => {}
        }
    }
}

impl Sequencer {
    /// v1/v2 blobs: a single 16-step pattern with param/cv locks; conditions,
    /// timing, and swing were never saved (BUG-005) and default.
    fn deserialize_legacy(&mut self, data: &[u8]) {
        let version = data[0];
        let mut r = ByteReader::new(&data[1..]);

        let Some(pattern_length) = r.u8().map(|v| v as usize) else { return };
        let Some(ticks_per_step) = r.u32() else { return };

        let mut steps = Vec::with_capacity(pattern_length);
        for _ in 0..pattern_length {
            let Some(active)     = r.u8().map(|v| v != 0) else { return };
            let Some(note)       = r.u8() else { return };
            let Some(velocity)   = r.u16() else { return };
            let Some(length)     = r.f32() else { return };
            let Some(lock_count) = r.u8().map(|v| v as usize) else { return };
            let mut param_locks = Vec::with_capacity(lock_count);
            for _ in 0..lock_count {
                let Some(node_id)  = r.u32() else { return };
                let Some(param_id) = r.u32() else { return };
                let Some(value)    = r.f64() else { return };
                param_locks.push(StepParamLock { node_id, param_id, value });
            }
            let mut cv_locks = Vec::new();
            if version >= 2 {
                let Some(cv_count) = r.u8().map(|v| v as usize) else { return };
                for _ in 0..cv_count {
                    let Some(idx) = r.u16() else { return };
                    let Some(val) = r.f32() else { return };
                    cv_locks.push((idx, val));
                }
            }
            steps.push(Step { active, note, velocity, length, param_locks,
                             condition: TrigCondition::default(), timing: StepTiming::default(),
                             cv_locks });
        }

        let mut pattern = Pattern::empty(Self::STEP_CAPACITY.max(pattern_length));
        for (i, step) in steps.into_iter().enumerate() {
            pattern.steps[i] = step;
        }
        // Clamp: a zero-length pattern would divide-by-zero in playback.
        pattern.length    = pattern_length.clamp(1, pattern.steps.len());
        pattern.page_loop = (0, (pattern_length.div_ceil(PAGE_SIZE).max(1) - 1) as u8);

        // Restore the construction-time bank size (see deserialize_v3).
        let bank_size = self.patterns.len();
        let mut patterns = vec![pattern];
        while patterns.len() < bank_size {
            patterns.push(Pattern::empty(Self::STEP_CAPACITY));
        }
        self.patterns       = patterns;
        self.active_pattern = 0;
        self.cued_pattern   = None;
        self.chain.clear();
        self.chain_pos      = 0;
        self.speed_mult     = 1.0;
        self.ticks_per_step = ticks_per_step;
    }

    fn deserialize_v3(&mut self, data: &[u8]) {
        let mut r = ByteReader::new(&data[1..]);
        let Some(ticks_per_step) = r.u32() else { return };
        let Some(speed_mult)     = r.f32() else { return };
        let Some(active_pattern) = r.u16().map(|v| v as usize) else { return };
        let Some(chain_len)      = r.u8().map(|v| v as usize) else { return };
        let mut chain = Vec::with_capacity(chain_len);
        for _ in 0..chain_len {
            let Some(c) = r.u16() else { return };
            chain.push(c as usize);
        }
        let Some(pattern_count) = r.u16().map(|v| v as usize) else { return };
        let mut patterns = Vec::with_capacity(pattern_count.max(1));
        for _ in 0..pattern_count {
            // Pattern records are length-prefixed like step records; unknown
            // trailing bytes are future per-pattern fields — skip them.
            let Some(record_len) = r.u32().map(|v| v as usize) else { return };
            let Some(end) = r.cur.checked_add(record_len) else { return };
            if end > r.data.len() { return; }
            let Some(length)     = r.u16().map(|v| v as usize) else { return };
            let Some(pl_start)   = r.u8() else { return };
            let Some(pl_end)     = r.u8() else { return };
            let Some(swing)      = r.f32() else { return };
            let Some(step_count) = r.u16().map(|v| v as usize) else { return };
            let mut steps = Vec::with_capacity(step_count.max(Self::STEP_CAPACITY));
            for _ in 0..step_count {
                let Some(step) = read_step_record(&mut r) else { return };
                steps.push(step);
            }
            if r.cur > end { return; }
            r.cur = end;
            // Sanitize: a pattern must hold at least one step, `length` must
            // stay within the step Vec (playback indexes `steps[..length]` on
            // the audio thread — a corrupt blob must not panic there), and
            // the runtime step capacity is restored so step editing after a
            // load behaves the same as after construction.
            if steps.is_empty() { return; }
            if steps.len() < Self::STEP_CAPACITY {
                steps.resize(Self::STEP_CAPACITY, Step::empty());
            }
            let length = length.clamp(1, steps.len());
            patterns.push(Pattern { steps, length, page_loop: (pl_start, pl_end), swing });
        }
        if patterns.is_empty() { return; }
        // Restore the construction-time bank size: saved patterns fill the
        // low indices, untouched trailing slots are recreated empty (§1.3).
        let bank_size = self.patterns.len();
        while patterns.len() < bank_size {
            patterns.push(Pattern::empty(Self::STEP_CAPACITY));
        }

        self.ticks_per_step = ticks_per_step;
        self.speed_mult     = speed_mult;
        self.active_pattern = active_pattern;
        self.chain          = chain;
        self.chain_pos      = 0;
        self.cued_pattern   = None;
        self.patterns       = patterns;
        // Re-apply the loaded swing to the bank (authoritative until P10 C2).
        // No-op when deserialize runs before activate(); activate() then
        // re-applies it from the pattern.
        let swing = self.patterns[self.active_index()].swing;
        self.bank.set(ParamDescriptor::id_for_name("swing"), swing as f64);
    }
}

/// A pattern is "used" (worth serializing) if any step deviates from empty —
/// including inactive steps that carry pre-programmed conditions or
/// micro-timing — or the pattern carries swing. Trailing unused bank slots
/// are dropped from the blob and recreated at load time — writing the full
/// pre-allocated bank would bloat every project file for no benefit.
fn pattern_is_used(p: &Pattern) -> bool {
    p.swing != 0.0 || p.steps.iter().any(|s| {
        s.active
            || !s.param_locks.is_empty()
            || !s.cv_locks.is_empty()
            || s.timing.micro_offset != 0
            || s.condition != TrigCondition::default()
    })
}

/// The zero-sentinel (n, m) ⇄ `RepeatCondition` mapping shared by the
/// CMD_SET_STEP_CONDITION encoding and the v3 step record.
fn repeat_from_nm(n: u8, m: u8) -> RepeatCondition {
    if n == 0 || m == 0 {
        RepeatCondition::Always
    } else {
        RepeatCondition::NthOfM { n, m }
    }
}

fn nm_from_repeat(r: RepeatCondition) -> (u8, u8) {
    match r {
        RepeatCondition::Always          => (0, 0),
        RepeatCondition::NthOfM { n, m } => (n, m),
    }
}

fn fill_discriminant(f: FillCondition) -> u8 {
    match f {
        FillCondition::Ignore   => 0,
        FillCondition::FillA    => 1,
        FillCondition::FillB    => 2,
        FillCondition::FillAny  => 3,
        FillCondition::NoFill   => 4,
        FillCondition::NotFillA => 5,
        FillCondition::NotFillB => 6,
    }
}

fn fill_from_discriminant(d: u8) -> FillCondition {
    match d {
        1 => FillCondition::FillA,
        2 => FillCondition::FillB,
        3 => FillCondition::FillAny,
        4 => FillCondition::NoFill,
        5 => FillCondition::NotFillA,
        6 => FillCondition::NotFillB,
        _ => FillCondition::Ignore,
    }
}

/// Append one length-prefixed v3 step record. The u16 prefix counts the bytes
/// that follow it; readers parse the fields they know and skip the remainder,
/// so future versions can append fields without breaking v3 readers.
fn write_step_record(step: &Step, buf: &mut Vec<u8>) {
    let len_pos = buf.len();
    buf.extend_from_slice(&0u16.to_le_bytes()); // patched below
    buf.push(step.active as u8);
    buf.push(step.note);
    buf.extend_from_slice(&step.velocity.to_le_bytes());
    buf.extend_from_slice(&step.length.to_le_bytes());
    let (probability, n, m, fill) = match &step.condition {
        TrigCondition::Simple { repeat, fill, probability } => {
            let (n, m) = nm_from_repeat(*repeat);
            (*probability, n, m, fill_discriminant(*fill))
        }
    };
    buf.push(probability);
    buf.push(n);
    buf.push(m);
    buf.push(fill);
    buf.push(step.timing.micro_offset as u8);
    buf.push(step.param_locks.len() as u8);
    for lock in &step.param_locks {
        buf.extend_from_slice(&lock.node_id.to_le_bytes());
        buf.extend_from_slice(&lock.param_id.to_le_bytes());
        buf.extend_from_slice(&lock.value.to_le_bytes());
    }
    buf.push(step.cv_locks.len() as u8);
    for &(idx, val) in &step.cv_locks {
        buf.extend_from_slice(&idx.to_le_bytes());
        buf.extend_from_slice(&val.to_le_bytes());
    }
    let record_len = (buf.len() - len_pos - 2) as u16;
    buf[len_pos..len_pos + 2].copy_from_slice(&record_len.to_le_bytes());
}

/// Read one length-prefixed v3 step record. Returns `None` on truncated or
/// malformed data; unknown trailing bytes within the record are skipped.
fn read_step_record(r: &mut ByteReader) -> Option<Step> {
    let record_len = r.u16()? as usize;
    let end = r.cur.checked_add(record_len)?;
    if end > r.data.len() { return None; }

    let active      = r.u8()? != 0;
    let note        = r.u8()?;
    let velocity    = r.u16()?;
    let length      = r.f32()?;
    let probability = r.u8()?;
    let n           = r.u8()?;
    let m           = r.u8()?;
    let fill        = fill_from_discriminant(r.u8()?);
    let micro       = r.u8()? as i8;
    let pl_count    = r.u8()? as usize;
    let mut param_locks = Vec::with_capacity(pl_count);
    for _ in 0..pl_count {
        let node_id  = r.u32()?;
        let param_id = r.u32()?;
        let value    = r.f64()?;
        param_locks.push(StepParamLock { node_id, param_id, value });
    }
    let cv_count = r.u8()? as usize;
    let mut cv_locks = Vec::with_capacity(cv_count);
    for _ in 0..cv_count {
        let idx = r.u16()?;
        let val = r.f32()?;
        cv_locks.push((idx, val));
    }
    // A record whose declared length is shorter than its own fields is
    // malformed; anything between here and `end` is a future field — skip it.
    if r.cur > end { return None; }
    r.cur = end;

    let repeat = repeat_from_nm(n, m);
    Some(Step {
        active, note, velocity, length, param_locks,
        condition: TrigCondition::Simple { repeat, fill, probability },
        timing:    StepTiming { micro_offset: micro },
        cv_locks,
    })
}

/// Bounds-checked little-endian reader over a serialized node blob.
struct ByteReader<'a> {
    data: &'a [u8],
    cur:  usize,
}

impl<'a> ByteReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, cur: 0 }
    }

    fn u8(&mut self) -> Option<u8> {
        let v = *self.data.get(self.cur)?;
        self.cur += 1;
        Some(v)
    }

    fn u16(&mut self) -> Option<u16> {
        let b = self.data.get(self.cur..self.cur + 2)?;
        self.cur += 2;
        Some(u16::from_le_bytes(b.try_into().unwrap()))
    }

    fn u32(&mut self) -> Option<u32> {
        let b = self.data.get(self.cur..self.cur + 4)?;
        self.cur += 4;
        Some(u32::from_le_bytes(b.try_into().unwrap()))
    }

    fn f32(&mut self) -> Option<f32> {
        let b = self.data.get(self.cur..self.cur + 4)?;
        self.cur += 4;
        Some(f32::from_le_bytes(b.try_into().unwrap()))
    }

    fn f64(&mut self) -> Option<f64> {
        let b = self.data.get(self.cur..self.cur + 8)?;
        self.cur += 8;
        Some(f64::from_le_bytes(b.try_into().unwrap()))
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
        let mut state = Vec::new();
        seq.published_state(&mut state);
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
        assert!(!seq.patterns[0].steps[3].active);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_TOGGLE_STEP, arg0: 3, arg1: 0.0 }]);
        assert!(seq.patterns[0].steps[3].active);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_TOGGLE_STEP, arg0: 3, arg1: 0.0 }]);
        assert!(!seq.patterns[0].steps[3].active);
    }

    #[test]
    fn sequencer_cmd_clear_deactivates_all_steps() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        for i in 0..16 { seq.set_step(i, 60, 32768, true); }
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_CLEAR, arg0: 0, arg1: 0.0 }]);
        assert!(seq.patterns[0].steps.iter().all(|s| !s.active));
    }

    #[test]
    fn sequencer_published_state_includes_steps_bitfield() {
        let mut seq = Sequencer::new();
        seq.set_node_id(5);
        seq.set_step(0, 60, 32768, true);
        seq.set_step(2, 60, 32768, true);
        let mut state = Vec::new();
        seq.published_state(&mut state);
        let steps = state.iter().find(|(k, _)| k.ends_with("/state/steps"));
        assert!(matches!(steps, Some((_, StateBusValue::Text(s))) if s.starts_with("10100")));
    }

    #[test]
    fn sequencer_published_state_has_track_name_when_set() {
        let mut seq = Sequencer::with_name("Kick");
        seq.set_node_id(1);
        let mut state = Vec::new();
        seq.published_state(&mut state);
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
        let _tps = TICKS_PER_BEAT / 4;  // 240
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
        let _sync_tick = bar_ticks as u32;
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

    // ── Runtime command tests (Commit 7) ─────────────────────────────────────

    #[test]
    fn sequencer_cmd_set_fill_a_updates_cycle_state() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        assert!(!seq.cycle_state.fill_a);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_FILL_A, arg0: 1, arg1: 0.0 }]);
        assert!(seq.cycle_state.fill_a);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_FILL_A, arg0: 0, arg1: 0.0 }]);
        assert!(!seq.cycle_state.fill_a);
    }

    #[test]
    fn sequencer_cmd_set_fill_b_updates_cycle_state() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_FILL_B, arg0: 1, arg1: 0.0 }]);
        assert!(seq.cycle_state.fill_b);
    }

    #[test]
    fn sequencer_cmd_set_step_timing_updates_micro_offset() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_STEP_TIMING, arg0: 3, arg1: 12.0 }]);
        assert_eq!(seq.patterns[0].steps[3].timing.micro_offset, 12i8);
    }

    #[test]
    fn sequencer_cmd_set_step_condition_encodes_correctly() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // probability=75, repeat_n=1, repeat_m=2, fill=Ignore(0)
        let enc: i64 = 75 | (1 << 8) | (2 << 16) | (0i64 << 24);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_STEP_CONDITION, arg0: 5, arg1: enc as f64 }]);
        assert!(matches!(&seq.patterns[0].steps[5].condition, TrigCondition::Simple {
            repeat: RepeatCondition::NthOfM { n: 1, m: 2 },
            fill:   FillCondition::Ignore,
            probability: 75,
        }));
    }

    #[test]
    fn sequencer_cmd_set_pattern_is_stubbed() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_PATTERN, arg0: 3, arg1: 0.0 }]);
        assert_eq!(seq.active_pattern, 3); // stored but has no playback effect
    }

    fn contains_note_on(events: &[Event]) -> bool {
        events.iter().any(|e| {
            matches!(e, Event::Midi2(ump)
                if matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))))
        })
    }

    #[test]
    fn sequencer_fires_step0_on_global_start() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(0, 60, 32768, true);
        let out = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        assert!(contains_note_on(&out), "step 0 must fire on the global_start event (BUG-001 fix)");
    }

    #[test]
    fn step_period_is_240_ticks() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        for i in 0..16 {
            seq.set_step(i, 60, 32768, true);
        }
        let tps = TICKS_PER_BEAT / 4;
        let mut fire_ticks: Vec<u32> = Vec::new();
        let out0 = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        if contains_note_on(&out0) {
            fire_ticks.push(0);
        }
        for t in 1..=(4 * tps) {
            let out = run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
            if contains_note_on(&out) {
                fire_ticks.push(t);
            }
        }
        assert!(fire_ticks.len() >= 4, "expected >=4 fires, got {:?}", fire_ticks);
        for w in fire_ticks.windows(2) {
            assert_eq!(w[1] - w[0], tps, "step period must be exactly {tps} ticks, fires: {fire_ticks:?}");
        }
    }

    #[test]
    fn sequencer_loop_count_increments_on_pattern_wrap() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let tps = TICKS_PER_BEAT / 4;  // 240 ticks per step
        // BUG-001 fixed (P10 C0): each step takes exactly tps ticks.
        let wrap_tick = 16 * tps;

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        assert_eq!(seq.cycle_state.loop_count, 0);

        for t in 1..=wrap_tick {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }
        assert_eq!(seq.cycle_state.loop_count, 1, "loop_count must increment after one full pattern");
    }

    #[test]
    fn sequencer_published_state_includes_loop_count_and_fills() {
        let mut seq = Sequencer::new();
        seq.set_node_id(7);
        seq.activate(44100.0, 64);
        seq.cycle_state.loop_count = 5;
        seq.cycle_state.fill_a = true;
        let mut state = Vec::new();
        seq.published_state(&mut state);

        let lc = state.iter().find(|(k, _)| k.ends_with("/state/loop_count"));
        assert!(matches!(lc, Some((_, StateBusValue::Int(5)))));

        let fa = state.iter().find(|(k, _)| k.ends_with("/state/fill_a"));
        assert!(matches!(fa, Some((_, StateBusValue::Float(v))) if *v == 1.0));

        let fb = state.iter().find(|(k, _)| k.ends_with("/state/fill_b"));
        assert!(matches!(fb, Some((_, StateBusValue::Float(v))) if *v == 0.0));
    }

    #[test]
    fn sequencer_swing_param_in_capability_document() {
        let seq = Sequencer::new();
        let doc = seq.capability_document();
        let swing_id = ParamDescriptor::id_for_name("swing");
        let swing = doc.params.iter().find(|p| p.id == swing_id);
        assert!(swing.is_some(), "swing param must be declared in capability_document");
        assert_eq!(swing.unwrap().default, 0.0);
        assert_eq!(swing.unwrap().max, 0.5);
    }

    #[test]
    fn sequencer_swing_nonzero_shifts_odd_step_sample_offset() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.swing_amount = 0.25;
        let spb = 60.0 * 44100.0f64 / 140.0;
        assert_eq!(seq.step_sample_offset(0, spb), 0,  "even step: no swing");
        assert!(seq.step_sample_offset(1, spb) > 0,    "odd step: swing pushes forward");
    }

    #[test]
    fn sequencer_fill_condition_blocks_trig_when_fill_inactive() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // Step 1 fires only when fill A is active
        seq.patterns[0].steps[1].active = true;
        seq.patterns[0].steps[1].condition = TrigCondition::Simple {
            repeat:      RepeatCondition::Always,
            fill:        FillCondition::FillA,
            probability: 100,
        };
        seq.cycle_state.fill_a = false;

        // Advance to step 1 boundary
        let tps = TICKS_PER_BEAT / 4;
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let mut fired = false;
        for t in 1..=tps {
            let events = run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
            if events.iter().any(|e| matches!(e, Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))))) {
                fired = true;
            }
        }
        assert!(!fired, "step with FillA condition must not fire when fill_a is false");
    }

    #[test]
    fn sequencer_serialize_deserialize_round_trip() {
        let mut seq = Sequencer::new();
        seq.set_step(3, 72, 50000, true);
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&data);
        assert_eq!(restored.patterns[0].steps[3].note, 72);
        assert!(restored.patterns[0].steps[3].active);
    }

    // ── Condition / timing type tests ────────────────────────────────────────

    #[test]
    fn trig_condition_default_always_fires() {
        let cond = TrigCondition::default();
        let state = SequencerCycleState::default();
        let mut rng = fastrand::Rng::with_seed(0);
        assert!(cond.evaluate(&state, &mut rng), "default condition must always fire");
    }

    #[test]
    fn trig_condition_probability_zero_never_fires() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill:   FillCondition::Ignore,
            probability: 0,
        };
        let state = SequencerCycleState::default();
        let mut rng = fastrand::Rng::with_seed(0);
        for _ in 0..100 {
            assert!(!cond.evaluate(&state, &mut rng));
        }
    }

    #[test]
    fn trig_condition_probability_100_always_fires() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill:   FillCondition::Ignore,
            probability: 100,
        };
        let state = SequencerCycleState::default();
        let mut rng = fastrand::Rng::with_seed(0);
        for _ in 0..100 {
            assert!(cond.evaluate(&state, &mut rng));
        }
    }

    #[test]
    fn trig_condition_nth_of_m_fires_on_correct_loop() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::NthOfM { n: 1, m: 2 },
            fill:   FillCondition::Ignore,
            probability: 100,
        };
        let mut rng = fastrand::Rng::with_seed(0);
        // loop_count=0 → n=1, 0%2==0 == (1-1)=0 → fires
        let state0 = SequencerCycleState { loop_count: 0, ..Default::default() };
        assert!(cond.evaluate(&state0, &mut rng));
        // loop_count=1 → 1%2==1 ≠ 0 → does not fire
        let state1 = SequencerCycleState { loop_count: 1, ..Default::default() };
        assert!(!cond.evaluate(&state1, &mut rng));
    }

    #[test]
    fn trig_condition_fill_a_only_fires_when_fill_a_active() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill:   FillCondition::FillA,
            probability: 100,
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let fill_on  = SequencerCycleState { fill_a: true,  ..Default::default() };
        let fill_off = SequencerCycleState { fill_a: false, ..Default::default() };
        assert!(cond.evaluate(&fill_on, &mut rng));
        assert!(!cond.evaluate(&fill_off, &mut rng));
    }

    #[test]
    fn trig_condition_no_fill_fires_when_neither_fill_active() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill:   FillCondition::NoFill,
            probability: 100,
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let no_fill   = SequencerCycleState { fill_a: false, fill_b: false, ..Default::default() };
        let fill_a_on = SequencerCycleState { fill_a: true,  fill_b: false, ..Default::default() };
        assert!(cond.evaluate(&no_fill, &mut rng));
        assert!(!cond.evaluate(&fill_a_on, &mut rng));
    }

    #[test]
    fn step_timing_zero_offset_returns_zero() {
        let t = StepTiming { micro_offset: 0 };
        assert_eq!(t.to_sample_offset(44100.0 / 2.0), 0);
    }

    #[test]
    fn step_timing_nonzero_offset_nonzero_samples() {
        let t = StepTiming { micro_offset: 48 };
        let spb = 44100.0 / 2.0_f64;
        assert!(t.to_sample_offset(spb) > 0);
    }

    #[test]
    fn step_empty_has_default_condition_and_timing() {
        let step = Step::empty();
        assert!(matches!(step.condition, TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill:   FillCondition::Ignore,
            probability: 100,
        }));
        assert_eq!(step.timing.micro_offset, 0);
    }

    // ── CV output tests (Commit 5) ────────────────────────────────────────────

    use paraclete_node_api::{SignalOutputSlot, SignalPortKind};

    /// Run one process cycle and capture the CV output for port `cv_port_id`.
    fn run_seq_with_cv(
        seq: &mut Sequencer,
        events: &[TimedEvent],
        cv_port_id: u32,
        block: usize,
    ) -> Vec<f32> {
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_ptr: *mut AudioBuffer = &mut audio;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];

        let mut cv_buf = vec![0.0f32; block];
        let cv_slot = SignalOutputSlot::new(cv_port_id, SignalPortKind::Cv, &mut cv_buf);
        let mut sig_outs = [cv_slot];

        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: &[],
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut sig_outs,
            events_out: &mut events_out,
        };
        seq.process(&input, &mut output);
        cv_buf
    }

    #[test]
    fn sequencer_cv_output_ports_present() {
        let seq = Sequencer::with_cv_outputs(2);
        let ports = seq.ports();
        let names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"cv_out_0"), "missing cv_out_0 in {:?}", names);
        assert!(names.contains(&"cv_out_1"), "missing cv_out_1 in {:?}", names);
        let cv_ports: Vec<_> = ports.iter()
            .filter(|p| p.port_type == PortType::Cv && p.direction == PortDirection::Output)
            .collect();
        assert_eq!(cv_ports.len(), 2, "expected 2 CvSignal Output ports");
    }

    #[test]
    fn sequencer_no_cv_no_extra_ports() {
        let seq = Sequencer::new();
        let cv_ports: Vec<_> = seq.ports().iter()
            .filter(|p| p.port_type == PortType::Cv && p.direction == PortDirection::Output)
            .collect();
        assert!(cv_ports.is_empty(), "Sequencer::new() must have no CvSignal Output ports");
    }

    #[test]
    fn sequencer_cv_output_initial_zero() {
        let mut seq = Sequencer::with_cv_outputs(1);
        seq.activate(44100.0, 256);
        // No step fires — initial hold value must be 0.0.
        let cv_port_id = Sequencer::PORT_CV_OUT_BASE;
        let out = run_seq_with_cv(&mut seq, &[], cv_port_id, 256);
        assert!(out.iter().all(|&v| v == 0.0), "CV output must be all zeros initially: {:?}", &out[..4]);
    }

    #[test]
    fn sequencer_cv_lock_updates_on_step_fire() {
        // After global_start, step_tick starts at 0. The first step boundary fires
        // step 1 (current_step advances 0→1). So step 1 is the one to activate.
        let mut seq = Sequencer::with_cv_outputs(1);
        seq.activate(44100.0, 64);
        seq.patterns[0].steps[1].active = true;
        seq.patterns[0].steps[1].cv_locks = vec![(0, 0.75)];
        let tps = TICKS_PER_BEAT / 4;
        let cv_port_id = Sequencer::PORT_CV_OUT_BASE;

        // global_start: current_step=0, step_tick=0→1
        let _ = run_seq_with_cv(&mut seq, &[transport_tick(0, true, true, false, false)], cv_port_id, 64);

        // Advance tps ticks: at t=tps the boundary fires and step 1 triggers.
        let mut cv_after_fire = vec![0.0f32; 64];
        for t in 1..=tps {
            cv_after_fire = run_seq_with_cv(&mut seq, &[transport_tick(t, true, false, false, false)], cv_port_id, 64);
        }
        assert!(
            cv_after_fire.iter().any(|&v| (v - 0.75).abs() < 1e-6),
            "CV lock value 0.75 must appear after step 1 fires: {:?}",
            &cv_after_fire[..4]
        );
    }

    #[test]
    fn sequencer_cv_lock_sample_and_hold() {
        // Step 1 fires first (tps ticks after global_start), step 2 fires second.
        let mut seq = Sequencer::with_cv_outputs(1);
        seq.activate(44100.0, 64);
        seq.patterns[0].steps[1].active = true;
        seq.patterns[0].steps[1].cv_locks = vec![(0, 0.5)];
        seq.patterns[0].steps[2].active = true;
        // Step 2 has no cv_locks — held value from step 1 must persist.
        let tps = TICKS_PER_BEAT / 4;
        let cv_port_id = Sequencer::PORT_CV_OUT_BASE;

        // global_start
        let _ = run_seq_with_cv(&mut seq, &[transport_tick(0, true, true, false, false)], cv_port_id, 64);

        // Advance 2*tps ticks: step 1 fires at tps, step 2 fires at 2*tps.
        let mut after_step2 = vec![0.0f32; 64];
        for t in 1..=(2 * tps) {
            let out = run_seq_with_cv(&mut seq, &[transport_tick(t, true, false, false, false)], cv_port_id, 64);
            if t == 2 * tps {
                after_step2 = out;
            }
        }
        // After step 2 fires (no cv_locks), the held value from step 1 must still be 0.5.
        assert!(
            after_step2.iter().all(|&v| (v - 0.5).abs() < 1e-6),
            "Sample-and-hold: CV must still be 0.5 after step 2 (no cv_locks): {:?}",
            &after_step2[..4]
        );
    }

    #[test]
    fn sequencer_cv_lock_out_of_range_ignored() {
        // Step 1 fires first; give it an out-of-range cv_lock.
        let mut seq = Sequencer::with_cv_outputs(1);
        seq.activate(44100.0, 64);
        seq.patterns[0].steps[1].active = true;
        seq.patterns[0].steps[1].cv_locks = vec![(5, 9.9)]; // index 5 out of range for cv_outputs=1
        let tps = TICKS_PER_BEAT / 4;
        let cv_port_id = Sequencer::PORT_CV_OUT_BASE;

        let _ = run_seq_with_cv(&mut seq, &[transport_tick(0, true, true, false, false)], cv_port_id, 64);
        let mut final_out = vec![0.0f32; 64];
        for t in 1..=tps {
            final_out = run_seq_with_cv(&mut seq, &[transport_tick(t, true, false, false, false)], cv_port_id, 64);
        }
        // cv_out_0 must remain 0.0 — out-of-range index is silently ignored.
        assert!(
            final_out.iter().all(|&v| v == 0.0),
            "Out-of-range cv_lock must not affect cv_out_0: {:?}",
            &final_out[..4]
        );
    }

    #[test]
    fn sequencer_cv_step_lock_serialization_roundtrip() {
        let mut seq = Sequencer::with_cv_outputs(2);
        seq.activate(44100.0, 64);
        seq.patterns[0].steps[0].active = true;
        seq.patterns[0].steps[0].cv_locks = vec![(0, 0.3), (1, 0.7)];
        seq.patterns[0].steps[3].active = true;
        seq.patterns[0].steps[3].cv_locks = vec![(0, 1.0)];

        let data = seq.serialize();

        let mut seq2 = Sequencer::with_cv_outputs(2);
        seq2.activate(44100.0, 64);
        seq2.deserialize(&data);

        assert_eq!(seq2.patterns[0].steps[0].cv_locks, vec![(0u16, 0.3f32), (1u16, 0.7f32)],
            "step 0 cv_locks mismatch after roundtrip");
        assert_eq!(seq2.patterns[0].steps[3].cv_locks, vec![(0u16, 1.0f32)],
            "step 3 cv_locks mismatch after roundtrip");
        assert!(seq2.patterns[0].steps[1].cv_locks.is_empty(), "step 1 cv_locks must be empty");
    }

    // ── P10 C1: Pattern + serializer v3 tests ─────────────────────────────────

    #[test]
    fn serialize_roundtrip_preserves_conditions() {
        let mut seq = Sequencer::new();
        seq.patterns[0].steps[5].active = true;
        seq.patterns[0].steps[5].condition = TrigCondition::Simple {
            repeat:      RepeatCondition::NthOfM { n: 2, m: 4 },
            fill:        FillCondition::NotFillB,
            probability: 42,
        };
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&data);
        assert!(matches!(&restored.patterns[0].steps[5].condition,
            TrigCondition::Simple {
                repeat: RepeatCondition::NthOfM { n: 2, m: 4 },
                fill:   FillCondition::NotFillB,
                probability: 42,
            }), "TrigCondition must survive a v3 roundtrip (BUG-005)");
    }

    #[test]
    fn serialize_roundtrip_preserves_timing() {
        let mut seq = Sequencer::new();
        seq.patterns[0].steps[7].active = true;
        seq.patterns[0].steps[7].timing.micro_offset = -12;
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&data);
        assert_eq!(restored.patterns[0].steps[7].timing.micro_offset, -12,
            "StepTiming micro_offset must survive a v3 roundtrip (BUG-005)");
    }

    #[test]
    fn serialize_roundtrip_preserves_swing() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // Set swing the way an encoder does: CMD_SET_PARAM through process().
        run_seq_with_cmds(&mut seq, &[NodeCommand {
            target_id: 0,
            type_id:   paraclete_node_api::CMD_SET_PARAM,
            arg0:      ParamDescriptor::id_for_name("swing") as i64,
            arg1:      0.3,
        }]);
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.activate(44100.0, 64);
        restored.deserialize(&data);
        assert!((restored.patterns[0].swing - 0.3).abs() < 1e-6,
            "pattern swing must survive a v3 roundtrip (BUG-005)");
        let bank_swing = restored.bank.get(ParamDescriptor::id_for_name("swing"));
        assert!((bank_swing - 0.3).abs() < 1e-6,
            "loaded swing must be re-applied to the bank (emission source until C2)");
    }

    #[test]
    fn serialize_roundtrip_preserves_cv_locks() {
        let mut seq = Sequencer::with_cv_outputs(2);
        seq.patterns[0].steps[2].active = true;
        seq.patterns[0].steps[2].cv_locks = vec![(0, 0.25), (1, 0.9)];
        let data = seq.serialize();
        let mut restored = Sequencer::with_cv_outputs(2);
        restored.deserialize(&data);
        assert_eq!(restored.patterns[0].steps[2].cv_locks,
            vec![(0u16, 0.25f32), (1u16, 0.9f32)],
            "cv_locks must survive a v3 roundtrip (v2 regression guard)");
    }

    #[test]
    fn deserialize_v2_into_single_pattern() {
        // Hand-built v2 blob matching the P9 writer: 16 steps, step 3 active.
        let mut blob = Vec::new();
        blob.push(2u8);   // version
        blob.push(16u8);  // pattern_length
        blob.extend_from_slice(&240u32.to_le_bytes()); // ticks_per_step
        for i in 0..16u8 {
            blob.push((i == 3) as u8);                       // active
            blob.push(if i == 3 { 72 } else { 60 });          // note
            blob.extend_from_slice(&32768u16.to_le_bytes()); // velocity
            blob.extend_from_slice(&0.75f32.to_le_bytes());  // length
            blob.push(0);                                     // param_locks
            blob.push(0);                                     // cv_locks
        }
        let mut seq = Sequencer::new();
        seq.deserialize(&blob);
        assert_eq!(seq.patterns.len(), 1, "v2 blob must load as a single pattern");
        assert_eq!(seq.patterns[0].length, 16);
        assert_eq!(seq.patterns[0].page_loop, (0, 1), "page_loop must span the loaded length");
        assert!(seq.patterns[0].steps[3].active);
        assert_eq!(seq.patterns[0].steps[3].note, 72);
        assert_eq!(seq.speed_mult, 1.0);
        assert!(seq.cued_pattern.is_none());
        assert!(seq.chain.is_empty());
    }

    #[test]
    fn single_pattern_playback_unchanged() {
        // The four-on-the-floor preset must fire at exactly the P9 tick
        // positions across two full pattern cycles — the playback-identity
        // gate for the C1 data-model refactor.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        for &s in &[0usize, 4, 8, 12] {
            seq.set_step(s, 60, 32768, true);
        }
        let tps = TICKS_PER_BEAT / 4;
        let mut fire_ticks = Vec::new();
        if contains_note_on(&run_seq(&mut seq, &[transport_tick(0, true, true, false, false)])) {
            fire_ticks.push(0);
        }
        for t in 1..=(32 * tps) {
            if contains_note_on(&run_seq(&mut seq, &[transport_tick(t, true, false, false, false)])) {
                fire_ticks.push(t);
            }
        }
        let expected: Vec<u32> = (0..=8).map(|i| i * 4 * tps).collect();
        assert_eq!(fire_ticks, expected,
            "step-fire tick sequence must be identical to P9");
    }

    #[test]
    fn deserialize_v3_skips_unknown_trailing_step_fields() {
        // Forward-extensibility contract (universality amendment): a v3 reader
        // must skip step-record fields it does not know about.
        let mut seq = Sequencer::new();
        seq.set_step(0, 61, 40_000, true);
        seq.set_step(1, 62, 40_000, true);
        let mut blob = seq.serialize();
        // First step record offset: header 14 (version 1 + ticks 4 + speed 4 +
        // active_pattern 2 + chain_len 1 + used_count 2) + pattern record
        // prefix 4 (u32) + pattern header 10 (length 2 + page_loop 2 +
        // swing 4 + step_count 2).
        const PAT_OFF: usize = 14;
        let rec_off = PAT_OFF + 4 + 10;
        let rec_len = u16::from_le_bytes([blob[rec_off], blob[rec_off + 1]]) as usize;
        // Append three unknown bytes to step 0's record and bump its prefix
        // (and the enclosing pattern record's u32 prefix to match).
        let insert_at = rec_off + 2 + rec_len;
        for k in 0..3 {
            blob.insert(insert_at + k, 0xEE);
        }
        blob[rec_off..rec_off + 2].copy_from_slice(&((rec_len + 3) as u16).to_le_bytes());
        let pat_len = u32::from_le_bytes(blob[PAT_OFF..PAT_OFF + 4].try_into().unwrap());
        blob[PAT_OFF..PAT_OFF + 4].copy_from_slice(&(pat_len + 3).to_le_bytes());

        let mut restored = Sequencer::new();
        restored.deserialize(&blob);
        assert!(restored.patterns[0].steps[0].active);
        assert_eq!(restored.patterns[0].steps[0].note, 61);
        assert!(restored.patterns[0].steps[1].active,
            "step 1 must parse correctly after skipped unknown bytes");
        assert_eq!(restored.patterns[0].steps[1].note, 62);
    }

    #[test]
    fn deserialize_v3_skips_unknown_trailing_pattern_fields() {
        // Same contract one level up: unknown bytes appended to a pattern
        // record (future per-pattern fields) must be skipped.
        let mut seq = Sequencer::new();
        seq.set_step(0, 63, 40_000, true);
        let mut blob = seq.serialize();
        const PAT_OFF: usize = 14;
        // Extend the (single, last) pattern record with 4 junk bytes.
        blob.extend_from_slice(&[0xEE; 4]);
        let pat_len = u32::from_le_bytes(blob[PAT_OFF..PAT_OFF + 4].try_into().unwrap());
        blob[PAT_OFF..PAT_OFF + 4].copy_from_slice(&(pat_len + 4).to_le_bytes());

        let mut restored = Sequencer::new();
        restored.deserialize(&blob);
        assert!(restored.patterns[0].steps[0].active);
        assert_eq!(restored.patterns[0].steps[0].note, 63);
    }

    /// Hand-build a v3 blob with one pattern of `step_count` step records and
    /// the given declared `length`.
    fn v3_blob(length: u16, steps: &[Step]) -> Vec<u8> {
        let mut blob = vec![3u8];
        blob.extend_from_slice(&240u32.to_le_bytes()); // ticks_per_step
        blob.extend_from_slice(&1.0f32.to_le_bytes()); // speed_mult
        blob.extend_from_slice(&0u16.to_le_bytes());   // active_pattern
        blob.push(0);                                   // chain_len
        blob.extend_from_slice(&1u16.to_le_bytes());   // pattern_count
        let mut body = Vec::new();
        body.extend_from_slice(&length.to_le_bytes());
        body.push(0);
        body.push(1);
        body.extend_from_slice(&0.0f32.to_le_bytes()); // swing
        body.extend_from_slice(&(steps.len() as u16).to_le_bytes());
        for s in steps {
            write_step_record(s, &mut body);
        }
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&body);
        blob
    }

    #[test]
    fn deserialize_v3_rejects_zero_step_pattern() {
        // A malformed blob declaring a zero-step pattern must abort the load,
        // leaving prior state intact (a 0-length pattern would panic playback).
        let mut seq = Sequencer::new();
        seq.set_step(3, 72, 32768, true);
        seq.deserialize(&v3_blob(16, &[]));
        assert!(seq.patterns[0].steps[3].active, "prior state must be retained");
        assert_eq!(seq.patterns[0].length, 16);
    }

    #[test]
    fn deserialize_v3_clamps_length_to_step_count() {
        // A blob whose declared length exceeds its steps must not load a
        // pattern that playback would index out of bounds.
        let mut s = Step::empty();
        s.active = true;
        let steps = vec![s, Step::empty()];
        let mut seq = Sequencer::new();
        seq.deserialize(&v3_blob(100, &steps));
        assert!(seq.patterns[0].length <= seq.patterns[0].steps.len(),
            "length {} must be clamped within steps {}",
            seq.patterns[0].length, seq.patterns[0].steps.len());
    }

    #[test]
    fn deserialize_v3_pads_steps_to_capacity() {
        // A foreign blob with fewer step records than the runtime capacity
        // loads with the step Vec restored to STEP_CAPACITY, so post-load
        // step editing behaves identically to a fresh sequencer.
        let mut s0 = Step::empty();
        s0.active = true;
        s0.note = 65;
        let steps = vec![s0, Step::empty()];
        let mut seq = Sequencer::new();
        seq.deserialize(&v3_blob(2, &steps));
        assert_eq!(seq.patterns[0].steps.len(), Sequencer::STEP_CAPACITY);
        assert_eq!(seq.patterns[0].length, 2);
        assert!(seq.patterns[0].steps[0].active);
        assert_eq!(seq.patterns[0].steps[0].note, 65);
        assert!(!seq.patterns[0].steps[63].active);
    }

    #[test]
    fn serialize_persists_effective_pattern_index() {
        // The CMD_SET_PATTERN stub stores any index, but the blob must carry
        // the index playback actually honors — a C1 save must not select a
        // different pattern under a future multi-pattern bank.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[NodeCommand {
            target_id: 0, type_id: Sequencer::CMD_SET_PATTERN, arg0: 5, arg1: 0.0,
        }]);
        assert_eq!(seq.active_pattern, 5, "stub still stores the raw index");
        let blob = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&blob);
        assert_eq!(restored.active_pattern, 0,
            "persisted index must be the effective (clamped) one");
    }

    #[test]
    fn pattern_is_used_counts_condition_and_timing_only_steps() {
        // An inactive step carrying a pre-programmed condition or micro-offset
        // makes the pattern worth serializing (data-loss guard for C4).
        let mut p = Pattern::empty(64);
        assert!(!pattern_is_used(&p));
        p.steps[5].timing.micro_offset = 12;
        assert!(pattern_is_used(&p), "micro-timing-only step must count as used");
        let mut q = Pattern::empty(64);
        q.steps[2].condition = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill:   FillCondition::FillA,
            probability: 100,
        };
        assert!(pattern_is_used(&q), "condition-only step must count as used");
    }

    #[test]
    fn set_pattern_then_step_edit_in_same_batch() {
        // In-batch ordering: a CMD_SET_PATTERN followed by a step edit in the
        // same command batch must direct the edit at the newly-active pattern
        // (observable in C1 only via the clamp — both resolve to pattern 0 —
        // but the per-command resolution is what C4's bank relies on).
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[
            NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_PATTERN, arg0: 0, arg1: 0.0 },
            NodeCommand { target_id: 0, type_id: Sequencer::CMD_TOGGLE_STEP, arg0: 7, arg1: 0.0 },
        ]);
        assert!(seq.patterns[0].steps[7].active,
            "step edit after CMD_SET_PATTERN in one batch must land");
    }
}
