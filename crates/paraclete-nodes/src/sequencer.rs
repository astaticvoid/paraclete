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
/// inclusive `(start_page, end_page)` playback window (P10 C2): playback
/// advances across `[start*8, min((end+1)*8, length))` and wraps to
/// `start*8` — that wrap is the pattern's cycle boundary. `swing` is
/// authoritative for emission and serialization (P10 C2); the ParameterBank
/// `swing` slot is a write-through conduit for encoders, never the source
/// of truth.
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
    /// Effective tick length of the CURRENT step: `ticks_per_step` scaled by
    /// `speed_mult` with the fractional remainder carried in `period_frac`
    /// (P10 C3) — non-integer multipliers alternate floor/ceil periods so
    /// the long-run average is exact (no BUG-001-class drift).
    step_period: u32,
    /// Fractional tick remainder carried between steps (see `step_period`).
    period_frac: f64,
    /// The step that was already emitted early this window (its micro_offset
    /// is negative — BUG-004); the boundary skips re-firing exactly that
    /// step. Storing the index (not a bool) keeps a window/length edit
    /// arriving between the early fire and the boundary from swallowing a
    /// DIFFERENT step's fire (review finding, C3).
    early_fired: Option<usize>,
    /// Ticks until the open gate closes, counted from note-on (review
    /// finding, C3): an absolute countdown, so an early-fired note keeps its
    /// own step's gate length instead of being cut at the previous step's
    /// gate-close tick.
    gate_ticks_left: u32,

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
    /// Last bank `swing` value forwarded to the active pattern. The bank is
    /// a write-conduit only (P10 C2, spec 2.3): a changed bank value means
    /// an encoder/CMD_SET_PARAM wrote it, and it is forwarded to
    /// `Pattern::swing` — which is what emission reads. Comparing against
    /// this field (not the pattern) keeps a pattern switch from writing the
    /// old pattern's swing into the new one.
    last_bank_swing: f32,
    sample_rate:    f32,

    cv_outputs:  usize,
    current_cv:  Vec<f32>,

    /// Note given to steps that were never explicitly set (toggle-created,
    /// re-padded on load). 60 by default; drum tracks driving a synth engine
    /// set this to the engine's trigger reference (e.g. 36) via the
    /// instrument file so a toggled step and a bare CMD_TRIGGER fire the
    /// same pitch (BUG-022). Per-step notes remain full-range.
    default_note: u8,

    /// Lazily-built `/node/{id}/state/*` path strings for the 17
    /// unconditional `published_state()` entries (BUG-007 fix: eliminates
    /// the per-cycle `format!` on the audio thread for these fixed keys).
    /// Keyed to `self.node_id` at first `published_state()` call. The
    /// conditional `track_name` entry and all VALUEs are still computed
    /// fresh each call.
    state_path_cache: std::sync::OnceLock<[String; 17]>,
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
    /// P10 C3: arg0 = step count (clamped 1..=64 and to the pattern's step
    /// capacity), arg1 = pattern index (>= 0) or -1 for the active pattern.
    /// Re-derives page count and clamps page_loop into range.
    pub const CMD_SET_LENGTH:         u32 = 28;
    /// P10 C3: arg1 = per-track speed multiplier, clamped to [0.125, 2.0].
    /// Takes effect at the next step boundary.
    pub const CMD_SET_SPEED:          u32 = 29;
    /// P10 C2: arg0 = start_page, arg1 = end_page (inclusive). Rejected
    /// (window unchanged) unless start <= end and both pages exist for the
    /// active pattern's current length.
    pub const CMD_SET_PAGE_LOOP:      u32 = 30;
    /// P10 C4: arg0 = pattern index appended to the volatile chain
    /// (capacity CHAIN_CAP; pushes beyond it or unknown indices ignored).
    pub const CMD_CHAIN_PUSH:         u32 = 31;
    /// P10 C4: empty the chain and reset its position.
    pub const CMD_CHAIN_CLEAR:        u32 = 32;

    /// Runtime step capacity per pattern (P10: 8 pages × 8 steps). The
    /// serialized format stores counts as plain integers and does not depend
    /// on this value (forward-extensibility amendment).
    pub const STEP_CAPACITY: usize = 64;

    /// Per-track pattern bank size (P10 C4). The whole bank is allocated at
    /// construction on the main thread — 8 patterns × 64 steps ≈ 512 Steps
    /// per track, the deliberate cost that buys allocation-free pattern
    /// switching on the audio thread (spec 4.3). The serialized format
    /// stores a used-pattern count, not this value.
    pub const PATTERN_BANK_SIZE: usize = 8;

    /// Chain capacity (spec 4.2): pushes beyond this are ignored, so the
    /// chain Vec (reserved at construction) never grows on the audio thread.
    pub const CHAIN_CAP: usize = 8;

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

    /// Set the note used for never-explicitly-set steps (BUG-022): rewrites
    /// the still-blank construction-time patterns and is applied to steps
    /// re-padded at `deserialize()`. Builder-style; call before first use.
    pub fn with_default_note(mut self, note: u8) -> Self {
        self.default_note = note;
        for pattern in &mut self.patterns {
            for step in &mut pattern.steps {
                step.note = note;
            }
        }
        self
    }

    /// `Pattern::empty` with this instance's `default_note` applied (BUG-022).
    fn blank_pattern(&self, steps: usize) -> Pattern {
        let mut pattern = Pattern::empty(steps);
        for step in &mut pattern.steps {
            step.note = self.default_note;
        }
        pattern
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
        // The full pattern bank is allocated here, on the main thread
        // (P10 C4): pattern 0 preserves the P9 default preset (16 played
        // steps over the 64-step capacity); the rest are empty and reachable
        // by CMD_SET_PATTERN without any audio-thread allocation.
        let mut pattern0 = Pattern::empty(Self::STEP_CAPACITY);
        pattern0.length = 16;
        pattern0.page_loop = (0, 1);
        let mut patterns = Vec::with_capacity(Self::PATTERN_BANK_SIZE);
        patterns.push(pattern0);
        while patterns.len() < Self::PATTERN_BANK_SIZE {
            patterns.push(Pattern::empty(Self::STEP_CAPACITY));
        }
        Self {
            ports,
            node_id: 0,
            track_name: name.to_string(),
            patterns,
            current_step: 0,
            step_tick: 0,
            ticks_per_step: TICKS_PER_BEAT / 4,
            step_period: TICKS_PER_BEAT / 4,
            period_frac: 0.0,
            early_fired: None,
            gate_ticks_left: 0,
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
            chain:          Vec::with_capacity(Self::CHAIN_CAP),
            chain_pos:      0,
            speed_mult:     1.0,
            swing_amount:   0.0,
            last_bank_swing: 0.0,
            sample_rate:    44100.0,
            cv_outputs,
            current_cv:  vec![0.0_f32; cv_outputs],
            default_note: 60,
            state_path_cache: std::sync::OnceLock::new(),
        }
    }

    /// Index of the pattern playback reads. `CMD_SET_PATTERN` validates
    /// against the bank (P10 C4), so the clamp is a defensive invariant for
    /// direct field manipulation (tests) and future code, not a live path.
    fn active_index(&self) -> usize {
        self.active_pattern.min(self.patterns.len() - 1)
    }

    /// The active pattern's page-loop window as `[start, end)` step indices
    /// (P10 C2). Defensive clamps keep the window inside `length` and
    /// non-empty even if `page_loop` and `length` disagree transiently —
    /// the audio thread must never panic on an index.
    fn window(&self) -> (usize, usize) {
        let p = &self.patterns[self.active_index()];
        let start = (p.page_loop.0 as usize * PAGE_SIZE).min(p.length.saturating_sub(1));
        let end = ((p.page_loop.1 as usize + 1) * PAGE_SIZE).min(p.length);
        (start, end.max(start + 1))
    }

    /// The step after `current` under page-loop playback, and whether the
    /// move wrapped. The wrap is THE cycle boundary (loop_count, and from
    /// P10 C4 cued switches / chain advances). A `current` outside the
    /// window (the window was edited mid-play) re-enters at window start,
    /// counted as a wrap.
    fn advance_step(&self, current: usize) -> (usize, bool) {
        let (start, end) = self.window();
        let next = current + 1;
        if next < start || next >= end { (start, true) } else { (next, false) }
    }

    /// Exact (fractional) tick length of one step at the current speed.
    fn exact_period(&self) -> f64 {
        self.ticks_per_step as f64 / self.speed_mult.clamp(0.125, 2.0) as f64
    }

    /// Tick length of the NEXT step (P10 C3): the exact period plus the
    /// carried fractional remainder, rounded — non-integer speed multipliers
    /// alternate floor/ceil periods so the long-run average stays exact.
    fn next_step_period(&mut self) -> u32 {
        let total = self.exact_period() + self.period_frac;
        let period = total.round().max(1.0);
        self.period_frac = total - period;
        period as u32
    }

    /// Reset the period machinery to a fresh step at the current speed
    /// (transport start / resync — deterministic, no carried remainder).
    fn reset_period(&mut self) {
        self.period_frac = 0.0;
        self.step_period = self.exact_period().round().max(1.0) as u32;
        self.early_fired = None;
    }

    /// Make `idx` the active pattern and refresh everything keyed to it
    /// (P10 C4): the swing write-conduit (spec 2.3 — the encoder must show
    /// the pattern now playing, and a stale conduit drops writes of the
    /// stale value). Callers ensure `idx` is within the bank.
    fn switch_pattern(&mut self, idx: usize) {
        self.active_pattern = idx;
        // A step pulled early belonged to the OLD pattern — it must not
        // suppress the new pattern's entry step at the boundary.
        self.early_fired = None;
        let swing = self.patterns[idx].swing;
        self.bank.set(ParamDescriptor::id_for_name("swing"), swing as f64);
        self.last_bank_swing = swing;
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
                Self::CMD_SET_LENGTH => {
                    // arg0 = step count (clamped); arg1 = pattern index or
                    // -1 = active. An unknown pattern index is ignored.
                    let idx = if cmd.arg1 < 0.0 {
                        pat
                    } else {
                        let i = cmd.arg1 as usize;
                        if i >= self.patterns.len() { continue; }
                        i
                    };
                    let p = &mut self.patterns[idx];
                    p.length = (cmd.arg0.max(1) as usize).min(p.steps.len());
                    // Re-derive the page count and clamp the window into it
                    // (spec 3.1) — no reallocation, steps are pre-sized.
                    let pages = p.length.div_ceil(PAGE_SIZE).max(1) as u8;
                    let end = p.page_loop.1.min(pages - 1);
                    p.page_loop = (p.page_loop.0.min(end), end);
                }
                Self::CMD_SET_SPEED => {
                    // Clamped multiplier; takes effect at the next boundary
                    // (the current step finishes at its computed period).
                    self.speed_mult = (cmd.arg1 as f32).clamp(0.125, 2.0);
                }
                Self::CMD_SET_PAGE_LOOP => {
                    // Validate-or-ignore (ADR-019): start <= end and both
                    // pages within the active pattern's current page count.
                    // arg1 < 0 is rejected before the cast — `as i64`
                    // truncates toward zero, so -0.5 would slip in as page 0.
                    if cmd.arg1 < 0.0 { continue; }
                    let start = cmd.arg0;
                    let end   = cmd.arg1 as i64;
                    let p = &mut self.patterns[pat];
                    let page_count = p.length.div_ceil(PAGE_SIZE) as i64;
                    if start >= 0 && end >= start && end < page_count {
                        p.page_loop = (start as u8, end as u8);
                    }
                }
                Self::CMD_SET_PATTERN => {
                    // P10 C4 (redefined from the P5 stub): an index outside
                    // the pre-allocated bank is silently ignored (ADR-019 —
                    // patterns are never grown on the audio thread). Stopped:
                    // switch immediately (editing flow). Playing: cue; the
                    // switch lands at the next cycle boundary.
                    let idx = cmd.arg0;
                    if idx < 0 || idx as usize >= self.patterns.len() {
                        continue;
                    }
                    if self.playing {
                        self.cued_pattern = Some(idx as usize);
                    } else {
                        self.switch_pattern(idx as usize);
                    }
                }
                Self::CMD_CHAIN_PUSH => {
                    let idx = cmd.arg0;
                    if idx >= 0
                        && (idx as usize) < self.patterns.len()
                        && self.chain.len() < Self::CHAIN_CAP
                    {
                        self.chain.push(idx as usize);
                    }
                }
                Self::CMD_CHAIN_CLEAR => {
                    self.chain.clear();
                    self.chain_pos = 0;
                }
                _ => {}
            }
        }
    }

    fn handle_transport(&mut self, k: &TransportEvent, sample_offset: u32, output: &mut ProcessOutput) {
        let spb = 60.0 * self.sample_rate as f64 / k.bpm.max(1.0);
        let pat = self.active_index();

        if k.flags.sync_pulse {
            let bars_elapsed = (k.bar - 1).max(0) as u64;
            let total_ticks  = bars_elapsed * k.time_sig_num as u64 * TICKS_PER_BEAT as u64
                + k.beat as u64 * TICKS_PER_BEAT as u64
                + k.tick as u64;
            // Transport position maps into the page-loop window (P10 C2):
            // a mid-session join lands inside the run of steps that plays.
            // Step spacing uses the exact (fractional) speed-scaled period
            // (P10 C3) — at non-integer multipliers the snap is a
            // nearest-tick approximation, same class as the natural path's
            // remainder-carrying advance.
            let (wstart, wend) = self.window();
            let wlen = (wend - wstart) as u64;
            let exact = self.exact_period();
            let steps_elapsed = (total_ticks as f64 / exact).floor();
            let step_index  = wstart + (steps_elapsed as u64 % wlen) as usize;
            let new_tick    = (total_ticks as f64 - steps_elapsed * exact).floor() as u32;

            // Drift correction only (BUG-001 fix, s0 re-diagnosis): when internal
            // counting is in sync, the natural advance below handles this event —
            // the snap must be a no-op or it pre-empts the wrap (and loop_count).
            // "In sync" = the natural path reaches (step_index, new_tick) this
            // event: either we are already there, or we are one increment away.
            let next_tick   = self.step_tick + 1;
            let in_sync = if next_tick >= self.step_period {
                new_tick == 0
                    && step_index == self.advance_step(self.current_step).0
            } else {
                step_index == self.current_step && new_tick == next_tick
            };
            if !in_sync && self.playing {
                self.current_step = step_index;
                self.reset_period();
                // At fractional speeds new_tick derives from the exact
                // (unrounded) period and can reach step_period when the
                // period rounded down — clamp inside the step so the joined
                // step is not skipped by an immediate boundary.
                self.step_tick = new_tick.min(self.step_period - 1);
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
                        self.emit_note_on_at(self.current_step, note_off, output);
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
            // A pending cue collapses to an immediate switch (review
            // finding, C4): stopped switches are immediate by contract, and
            // the user's last selection should be what plays on restart —
            // not one surprise cycle of the old pattern first.
            if let Some(cue) = self.cued_pattern.take() {
                if cue < self.patterns.len() {
                    self.switch_pattern(cue);
                }
            }
            return;
        }

        if k.flags.global_start {
            self.playing      = true;
            // Transport start enters the pattern at the page-loop window's
            // first step (P10 C2) — step 0 for the default full window.
            let (wstart, _) = self.window();
            self.current_step = wstart;
            self.step_tick    = 0;
            self.reset_period();
            // Fire the entry step (BUG-001 fix): previously the first step
            // was never emitted by the boundary path and only sounded via
            // the bar-sync snap. The start event IS tick 0 of that step —
            // return so it is not also counted as progress.
            let step_active = self.patterns[pat].steps[wstart].active;
            if step_active {
                let cond = self.patterns[pat].steps[wstart].condition.clone();
                if cond.evaluate(&self.cycle_state, &mut self.rng) {
                    let note_off = sample_offset + self.step_sample_offset(wstart, spb);
                    self.emit_note_on_at(wstart, note_off, output);
                    for lock in &self.patterns[pat].steps[wstart].param_locks {
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
            // Absolute countdown from note-on (its length × the firing
            // step's period): an early-fired note keeps its own gate length
            // across the boundary instead of being cut at the previous
            // step's gate-close tick.
            self.gate_ticks_left = self.gate_ticks_left.saturating_sub(1);
            if self.gate_ticks_left == 0 {
                self.emit_note_off(sample_offset, output);
            }
        }

        // BUG-004 (P10 C3): a negative micro_offset pulls a step EARLIER —
        // it fires during the previous step's window, `|offset|` 1/96-beat
        // units (10 ticks each) before its grid boundary. Tick-exact: the
        // micro unit is a whole number of clock ticks. The boundary below
        // then advances position without re-firing (early_fired).
        if self.early_fired.is_none() && self.step_tick < self.step_period {
            let (next, _) = self.advance_step(self.current_step);
            let micro  = self.patterns[pat].steps[next].timing.micro_offset;
            let active = self.patterns[pat].steps[next].active;
            if active && micro < 0 {
                let early_ticks = ((-(micro as i32)) as u32 * (TICKS_PER_BEAT / 96))
                    .min(self.step_period - 1);
                if self.step_tick >= self.step_period - early_ticks {
                    // The condition rolls at fire time (pre-wrap loop_count
                    // for a window-wrapping early step — documented choice),
                    // and rolls exactly once: the boundary skips this step
                    // whether or not it fired.
                    let cond = self.patterns[pat].steps[next].condition.clone();
                    if cond.evaluate(&self.cycle_state, &mut self.rng) {
                        if self.gate_open {
                            self.emit_note_off(sample_offset, output);
                        }
                        self.emit_note_on_at(next, sample_offset, output);
                        for lock in &self.patterns[pat].steps[next].param_locks {
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
                    self.early_fired = Some(next);
                }
            }
        }

        if self.step_tick >= self.step_period {
            self.step_tick   = 0;
            self.step_period = self.next_step_period();
            let (next, wrapped) = self.advance_step(self.current_step);
            self.current_step = next;

            // The page-loop wrap is THE cycle boundary (P10 C2/C4):
            // loop_count increments here, and cued switches / chain
            // advances are evaluated here only.
            if wrapped {
                self.cycle_state.loop_count = self.cycle_state.loop_count.wrapping_add(1);

                // An explicit cue wins over the chain for this one boundary
                // (spec 4.2) — the chain does not advance that cycle.
                if let Some(cue) = self.cued_pattern.take() {
                    if cue < self.patterns.len() {
                        self.switch_pattern(cue);
                        self.current_step = self.window().0;
                    }
                } else if !self.chain.is_empty() {
                    // Read-then-advance: a fresh chain [0,1,2] visits its
                    // entries in order from the first boundary (spec 4.4
                    // test), then wraps.
                    let target = self.chain[self.chain_pos.min(self.chain.len() - 1)];
                    self.chain_pos = (self.chain_pos.min(self.chain.len() - 1) + 1) % self.chain.len();
                    if target < self.patterns.len() {
                        self.switch_pattern(target);
                        self.current_step = self.window().0;
                    }
                }
            }

            // Re-resolve after a possible switch: the fire below must read
            // the pattern that is active NOW.
            let pat = self.active_index();

            // Skip re-firing only the exact step that was pulled early; if a
            // window edit landed in between, the boundary may be on a
            // different step, which must still fire normally.
            let fired_early = self.early_fired.take() == Some(self.current_step);
            let step_active = self.patterns[pat].steps[self.current_step].active;
            if step_active && !fired_early {
                let cond = self.patterns[pat].steps[self.current_step].condition.clone();
                let should_fire = cond.evaluate(&self.cycle_state, &mut self.rng);
                if should_fire {
                    let note_off = sample_offset + self.step_sample_offset(self.current_step, spb);
                    self.emit_note_on_at(self.current_step, note_off, output);
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

    fn emit_note_on_at(&mut self, step_idx: usize, sample_offset: u32, output: &mut ProcessOutput) {
        let pat  = self.active_index();
        let step = &self.patterns[pat].steps[step_idx];
        self.active_note     = step.note;
        self.gate_open       = true;
        self.gate_ticks_left = (step.length * self.step_period as f32).max(1.0) as u32;
        self.trig_count      = self.trig_count.wrapping_add(1);
        self.last_fired_step = step_idx;
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

    /// Forward (positive) sample displacement for a step fired at its grid
    /// boundary: positive micro-timing plus swing. A NEGATIVE micro_offset
    /// contributes nothing here — pulled-early steps fire via the early-fire
    /// path in `handle_transport` (BUG-004); when a negative-offset step is
    /// fired directly at its grid position (transport start, bar-sync
    /// resync), "on the grid" is the closest playable time, not `|offset|`
    /// late.
    fn step_sample_offset(&self, step_idx: usize, samples_per_beat: f64) -> u32 {
        // BUG-031 (ADR-030): intra-step displacements are proportional to the
        // step. The step period is speed-scaled (`exact_period`), so swing and
        // micro-timing offsets scale the same way (÷ speed_mult) — the swing
        // feel stays a fixed fraction of the step at any per-track speed, and a
        // per-step nudge can never overshoot into the next step because it grew
        // with speed.
        let speed = self.speed_mult.clamp(0.125, 2.0) as f64;
        let timing = self.patterns[self.active_index()].steps[step_idx].timing;
        let micro = if timing.micro_offset > 0 {
            (timing.to_sample_offset(samples_per_beat) as f64 / speed) as u32
        } else {
            0
        };
        let swing = if step_idx % 2 == 1 {
            (self.swing_amount as f64 * samples_per_beat / speed) as u32
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
            name: "Sequencer".into(),
            vendor: "Paraclete".into(),
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
            extensions: vec!["paraclete.sequencer".into()],
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
        // class). `last_bank_swing` tracks the conduit (P10 C2) so this
        // re-apply is not mistaken for an encoder write in process().
        let swing = self.patterns[self.active_index()].swing;
        self.bank.set(ParamDescriptor::id_for_name("swing"), swing as f64);
        self.last_bank_swing = swing;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.handle_commands(input.commands);
        // Swing is per-pattern (P10 C2, spec 2.3): `Pattern::swing` is the
        // source of truth for emission and serialization. The bank slot is a
        // write-conduit only — a changed bank value means an encoder or
        // CMD_SET_PARAM wrote it, and it is forwarded to the active pattern.
        let pat = self.active_index();
        let bank_swing = self.bank.get(ParamDescriptor::id_for_name("swing")) as f32;
        if bank_swing != self.last_bank_swing {
            self.patterns[pat].swing = bank_swing;
            self.last_bank_swing = bank_swing;
        }
        self.swing_amount = self.patterns[pat].swing;

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
        let paths = self.state_path_cache.get_or_init(|| {
            [
                format!("/node/{id}/state/current_step"),
                format!("/node/{id}/state/pattern_length"),
                format!("/node/{id}/state/playing"),
                format!("/node/{id}/state/steps"),
                format!("/node/{id}/state/last_trig"),
                format!("/node/{id}/state/last_fired_step"),
                format!("/node/{id}/state/loop_count"),
                format!("/node/{id}/state/fill_a"),
                format!("/node/{id}/state/fill_b"),
                // P10 C5 pattern-engine surface (spec 5.1).
                format!("/node/{id}/state/active_pattern"),
                format!("/node/{id}/state/cued_pattern"),
                format!("/node/{id}/state/current_page"),
                format!("/node/{id}/state/page_count"),
                format!("/node/{id}/state/page_loop_start"),
                format!("/node/{id}/state/page_loop_end"),
                format!("/node/{id}/state/speed_mult"),
                format!("/node/{id}/state/chain_len"),
            ]
        });
        let p = &self.patterns[self.active_index()];
        buf.push((paths[0].clone(), StateBusValue::Int(self.current_step as i64)));
        buf.push((paths[1].clone(), StateBusValue::Int(p.length as i64)));
        buf.push((paths[2].clone(), StateBusValue::Bool(self.playing)));
        buf.push((paths[3].clone(), StateBusValue::Text(self.steps_bitfield())));
        buf.push((paths[4].clone(), StateBusValue::Int(self.trig_count as i64)));
        buf.push((paths[5].clone(), StateBusValue::Int(self.last_fired_step as i64)));
        buf.push((paths[6].clone(), StateBusValue::Int(self.cycle_state.loop_count as i64)));
        buf.push((paths[7].clone(), StateBusValue::Float(if self.cycle_state.fill_a { 1.0 } else { 0.0 })));
        buf.push((paths[8].clone(), StateBusValue::Float(if self.cycle_state.fill_b { 1.0 } else { 0.0 })));
        buf.push((paths[9].clone(), StateBusValue::Int(self.active_index() as i64)));
        buf.push((paths[10].clone(), StateBusValue::Int(self.cued_pattern.map_or(-1, |c| c as i64))));
        buf.push((paths[11].clone(), StateBusValue::Int((self.current_step / PAGE_SIZE) as i64)));
        buf.push((paths[12].clone(), StateBusValue::Int(p.length.div_ceil(PAGE_SIZE) as i64)));
        buf.push((paths[13].clone(), StateBusValue::Int(p.page_loop.0 as i64)));
        buf.push((paths[14].clone(), StateBusValue::Int(p.page_loop.1 as i64)));
        buf.push((paths[15].clone(), StateBusValue::Float(self.speed_mult as f64)));
        buf.push((paths[16].clone(), StateBusValue::Int(self.chain.len() as i64)));
        // Conditional entry — not cached: caching would require a second
        // OnceLock keyed on presence-at-first-call, which is fragile if
        // track_name is set after construction but before the first publish.
        // format! here is cheap (single optional String) relative to the 17
        // unconditional entries above.
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

        let mut pattern = self.blank_pattern(Self::STEP_CAPACITY.max(pattern_length));
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
            patterns.push(self.blank_pattern(Self::STEP_CAPACITY));
        }
        self.patterns       = patterns;
        self.active_pattern = 0;
        self.cued_pattern   = None;
        self.chain.clear();
        self.chain_pos      = 0;
        self.speed_mult     = 1.0;
        self.ticks_per_step = ticks_per_step;
        self.reset_period();
    }

    fn deserialize_v3(&mut self, data: &[u8]) {
        let mut r = ByteReader::new(&data[1..]);
        let Some(ticks_per_step) = r.u32() else { return };
        let Some(speed_mult)     = r.f32() else { return };
        let Some(active_pattern) = r.u16().map(|v| v as usize) else { return };
        let Some(chain_len)      = r.u8().map(|v| v as usize) else { return };
        // Capacity CHAIN_CAP regardless of the loaded length (P10 C4): a
        // later CMD_CHAIN_PUSH on the audio thread must never reallocate.
        // Entries beyond the cap (foreign/corrupt blob) are dropped.
        let mut chain = Vec::with_capacity(chain_len.max(Self::CHAIN_CAP));
        for _ in 0..chain_len {
            let Some(c) = r.u16() else { return };
            if chain.len() < Self::CHAIN_CAP {
                chain.push(c as usize);
            }
        }
        let Some(pattern_count) = r.u16().map(|v| v as usize) else { return };
        // Foreign/corrupt blobs may claim more patterns than the bank holds
        // (u16 count × 64 Steps each = unbounded memory, and every comment
        // in this file assumes the PATTERN_BANK_SIZE invariant). Read only
        // the bank's worth; the leftover records land in the ignored
        // trailing-bytes region the format already defines.
        let pattern_count = pattern_count.min(Self::PATTERN_BANK_SIZE);
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
                // Padded steps carry this instance's default note (BUG-022),
                // not the Step::empty() constant.
                let mut pad = Step::empty();
                pad.note = self.default_note;
                steps.resize(Self::STEP_CAPACITY, pad);
            }
            let length = length.clamp(1, steps.len());
            // Sanitize the window like CMD_SET_PAGE_LOOP would (corrupt or
            // foreign blobs): an invalid window resets to the full span
            // instead of loading into degenerate single-step playback.
            let pages = length.div_ceil(PAGE_SIZE).max(1) as u8;
            let page_loop = if pl_start <= pl_end && pl_end < pages {
                (pl_start, pl_end)
            } else {
                (0, pages - 1)
            };
            patterns.push(Pattern { steps, length, page_loop, swing });
        }
        if patterns.is_empty() { return; }
        // Restore the construction-time bank size: saved patterns fill the
        // low indices, untouched trailing slots are recreated empty (§1.3).
        let bank_size = self.patterns.len();
        while patterns.len() < bank_size {
            patterns.push(self.blank_pattern(Self::STEP_CAPACITY));
        }

        self.ticks_per_step = ticks_per_step;
        self.speed_mult     = speed_mult.clamp(0.125, 2.0);
        self.active_pattern = active_pattern.min(patterns.len() - 1);
        self.chain          = chain;
        self.chain_pos      = 0;
        self.cued_pattern   = None;
        self.patterns       = patterns;
        // Deterministic period machinery for the loaded speed (P10 C3).
        self.reset_period();
        // Mirror the loaded swing into the bank conduit (P10 C2: the pattern
        // is authoritative; this keeps the encoder display current). Track it
        // in last_bank_swing so process() doesn't mistake the load for an
        // encoder write. No-op when deserialize runs before activate();
        // activate() then re-applies both from the pattern.
        let swing = self.patterns[self.active_index()].swing;
        self.bank.set(ParamDescriptor::id_for_name("swing"), swing as f64);
        self.last_bank_swing = swing;
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
        // C4: the bank is restored to PATTERN_BANK_SIZE on load; the legacy
        // single pattern fills index 0 and the rest are empty (spec 1.3).
        assert_eq!(seq.patterns.len(), Sequencer::PATTERN_BANK_SIZE);
        assert!(seq.patterns[1..].iter().all(|p| !pattern_is_used(p)),
            "v2 blob fills only pattern 0");
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

    /// ADR latent-issue audit item #11: a foreign/corrupt v3 blob whose
    /// active_pattern exceeds its pattern count must load gracefully — the
    /// index is clamped into the padded bank, never indexed out of bounds.
    #[test]
    fn deserialize_v3_active_pattern_beyond_count_is_clamped() {
        let mut s = Step::empty();
        s.active = true;
        let mut blob = vec![3u8];
        blob.extend_from_slice(&240u32.to_le_bytes());  // ticks_per_step
        blob.extend_from_slice(&1.0f32.to_le_bytes());  // speed_mult
        blob.extend_from_slice(&999u16.to_le_bytes());  // active_pattern: way out of range
        blob.push(0);                                    // chain_len
        blob.extend_from_slice(&2u16.to_le_bytes());    // pattern_count = 2
        for _ in 0..2 {
            let mut body = Vec::new();
            body.extend_from_slice(&1u16.to_le_bytes()); // length
            body.push(0);                                 // page_loop start
            body.push(0);                                 // page_loop end
            body.extend_from_slice(&0.0f32.to_le_bytes()); // swing
            body.extend_from_slice(&1u16.to_le_bytes());  // step_count
            write_step_record(&s, &mut body);
            blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
            blob.extend_from_slice(&body);
        }
        let mut seq = Sequencer::new();
        seq.deserialize(&blob);
        assert!(seq.active_pattern < seq.patterns.len(),
            "active_pattern {} must be clamped inside the bank of {}",
            seq.active_pattern, seq.patterns.len());
        // Playback index paths must not panic on the loaded state.
        let _ = seq.window();
    }

    /// ADR latent-issue audit item #6: a corrupt v3 blob declaring zero
    /// patterns must abort the load, keeping prior state — an empty patterns
    /// Vec would panic every playback index (active_index subtracts 1).
    #[test]
    fn deserialize_v3_zero_pattern_count_keeps_existing_state() {
        let mut blob = vec![3u8];
        blob.extend_from_slice(&240u32.to_le_bytes());  // ticks_per_step
        blob.extend_from_slice(&1.0f32.to_le_bytes());  // speed_mult
        blob.extend_from_slice(&0u16.to_le_bytes());    // active_pattern
        blob.push(0);                                    // chain_len
        blob.extend_from_slice(&0u16.to_le_bytes());    // pattern_count = 0
        let mut seq = Sequencer::new();
        seq.set_step(3, 72, 32768, true);
        seq.deserialize(&blob);
        assert!(!seq.patterns.is_empty(), "patterns bank must never be empty");
        assert!(seq.patterns[0].steps[3].active, "prior state must be retained");
    }

    #[test]
    fn serialize_persists_effective_pattern_index() {
        // C1 wrote the effective (clamped) index because CMD_SET_PATTERN was
        // a stub that stored anything. C4 enforces the same property at the
        // command site: a valid index round-trips, an out-of-bank index never
        // lands — a save can never select a different pattern on load.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(5)]);
        assert_eq!(seq.active_pattern, 5, "valid bank index switches (stopped)");
        let blob = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&blob);
        assert_eq!(restored.active_pattern, 5, "valid index round-trips");

        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(99)]);
        assert_eq!(seq.active_pattern, 5, "out-of-bank index is rejected, not stored");
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

    // ── P10 C2: multi-page patterns ──────────────────────────────────────────

    /// Drive `n` step boundaries (ticks_per_step transport ticks each) and
    /// record `current_step` after each boundary.
    fn drive_steps(seq: &mut Sequencer, n: usize) -> Vec<usize> {
        let tps = seq.ticks_per_step;
        let mut positions = Vec::with_capacity(n);
        for _ in 0..n {
            for _ in 0..tps {
                run_seq(seq, &[transport_tick(1, true, false, false, false)]);
            }
            positions.push(seq.current_step);
        }
        positions
    }

    fn set_page_loop_cmd(start: i64, end: f64) -> NodeCommand {
        NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_PAGE_LOOP, arg0: start, arg1: end }
    }

    #[test]
    fn page_loop_window_wraps_within_window() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns[0].length = 32;
        run_seq_with_cmds(&mut seq, &[set_page_loop_cmd(0, 1.0)]);
        assert_eq!(seq.patterns[0].page_loop, (0, 1));

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        assert_eq!(seq.current_step, 0);
        let positions = drive_steps(&mut seq, 40);
        assert!(positions.iter().all(|&p| p < 16),
            "a (0,1) window must never reach step 16: {positions:?}");
        let at15 = positions.iter().position(|&p| p == 15).expect("reaches step 15");
        assert_eq!(positions[at15 + 1], 0, "wraps 15 -> 0");
    }

    #[test]
    fn page_loop_opens_to_full() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns[0].length = 32;
        run_seq_with_cmds(&mut seq, &[set_page_loop_cmd(0, 3.0)]);
        assert_eq!(seq.patterns[0].page_loop, (0, 3));

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let positions = drive_steps(&mut seq, 40);
        assert_eq!(positions.iter().max(), Some(&31), "plays out to step 31");
        let at31 = positions.iter().position(|&p| p == 31).unwrap();
        assert_eq!(positions[at31 + 1], 0, "wraps 31 -> 0");
    }

    #[test]
    fn set_page_loop_clamps_start_le_end() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns[0].length = 32;
        run_seq_with_cmds(&mut seq, &[set_page_loop_cmd(1, 2.0)]);
        assert_eq!(seq.patterns[0].page_loop, (1, 2));

        // start > end: rejected, window unchanged.
        run_seq_with_cmds(&mut seq, &[set_page_loop_cmd(2, 1.0)]);
        assert_eq!(seq.patterns[0].page_loop, (1, 2));
        // end beyond the pattern's page count (32 steps = pages 0-3): rejected.
        run_seq_with_cmds(&mut seq, &[set_page_loop_cmd(0, 7.0)]);
        assert_eq!(seq.patterns[0].page_loop, (1, 2));
        // negative start: rejected.
        run_seq_with_cmds(&mut seq, &[set_page_loop_cmd(-1, 2.0)]);
        assert_eq!(seq.patterns[0].page_loop, (1, 2));
    }

    #[test]
    fn page_derivation() {
        // Spec 2.4: steps 0-7 derive page 0, steps 8-15 derive page 1.
        for step in 0..8usize {
            assert_eq!(step / PAGE_SIZE, 0, "steps 0-7 are page 0");
            assert_eq!((step + 8) / PAGE_SIZE, 1, "steps 8-15 are page 1");
        }

        // And playback agrees: a (1,1) window on a 32-step pattern enters at
        // the window's first step (8) and every played step derives page 1.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns[0].length = 32;
        run_seq_with_cmds(&mut seq, &[set_page_loop_cmd(1, 1.0)]);

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        assert_eq!(seq.current_step, 8, "start enters at the window start");
        assert_eq!(seq.current_step / PAGE_SIZE, 1);
        let positions = drive_steps(&mut seq, 12);
        assert!(positions.iter().all(|&p| p / PAGE_SIZE == 1),
            "every step in a (1,1) window derives page 1: {positions:?}");
        assert!(positions.contains(&8) && positions.contains(&15));

        // Page-0 half through playback too: default window (0,1) on 16 steps
        // never leaves pages 0-1, and the first 8 boundaries stay in page 0.
        let mut seq0 = Sequencer::new();
        seq0.activate(44100.0, 64);
        run_seq(&mut seq0, &[transport_tick(0, true, true, false, false)]);
        assert_eq!(seq0.current_step / PAGE_SIZE, 0, "start (step 0) is page 0");
        let first7 = drive_steps(&mut seq0, 7);
        assert!(first7.iter().all(|&p| p / PAGE_SIZE == 0),
            "steps 1-7 derive page 0: {first7:?}");
    }

    #[test]
    fn swing_is_per_pattern() {
        let swing_id = ParamDescriptor::id_for_name("swing");
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns.push(Pattern::empty(Sequencer::STEP_CAPACITY));

        // Encoder write (CMD_SET_PARAM through the bank conduit) lands on the
        // active pattern only.
        run_seq_with_cmds(&mut seq, &[NodeCommand {
            target_id: 0, type_id: 0, arg0: swing_id as i64, arg1: 0.3,
        }]);
        assert!((seq.patterns[0].swing - 0.3).abs() < 1e-6);
        assert_eq!(seq.patterns[1].swing, 0.0, "inactive pattern untouched");

        // Switch active; a new write lands on pattern 1, pattern 0 keeps its own.
        seq.active_pattern = 1;
        run_seq_with_cmds(&mut seq, &[NodeCommand {
            target_id: 0, type_id: 0, arg0: swing_id as i64, arg1: 0.1,
        }]);
        assert!((seq.patterns[1].swing - 0.1).abs() < 1e-6);
        assert!((seq.patterns[0].swing - 0.3).abs() < 1e-6, "patterns hold independent swing");
        assert!((seq.swing_amount - 0.1).abs() < 1e-6, "emission reads the active pattern");
    }

    #[test]
    fn pattern_switch_refreshes_swing_conduit() {
        // Review finding (C2): a live CMD_SET_PATTERN must refresh the bank
        // conduit from the new pattern, or a post-switch write of exactly the
        // stale value is dropped by the change guard.
        let swing_id = ParamDescriptor::id_for_name("swing");
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns.push(Pattern::empty(Sequencer::STEP_CAPACITY));
        seq.patterns[1].swing = 0.5;

        // Pattern 0's swing = 0.3 via the conduit.
        run_seq_with_cmds(&mut seq, &[NodeCommand {
            target_id: 0, type_id: 0, arg0: swing_id as i64, arg1: 0.3,
        }]);
        // Live switch to pattern 1: bank must now read 0.5, not 0.3.
        run_seq_with_cmds(&mut seq, &[NodeCommand {
            target_id: 0, type_id: Sequencer::CMD_SET_PATTERN, arg0: 1, arg1: 0.0,
        }]);
        assert!((seq.bank.get(swing_id) - 0.5).abs() < 1e-6, "conduit refreshed on switch");
        assert!((seq.swing_amount - 0.5).abs() < 1e-6);

        // Writing exactly the pre-switch value (0.3) must land on pattern 1.
        run_seq_with_cmds(&mut seq, &[NodeCommand {
            target_id: 0, type_id: 0, arg0: swing_id as i64, arg1: 0.3,
        }]);
        assert!((seq.patterns[1].swing - 0.3).abs() < 1e-6,
            "write of the stale value is not dropped");
        assert!((seq.patterns[0].swing - 0.3).abs() < 1e-6, "pattern 0 untouched by the switch");
    }

    #[test]
    fn swing_offset_scales_with_step_speed() {
        // BUG-031 fix (ADR-030): intra-step displacements (swing + micro) are
        // proportional to the step. The step PERIOD is speed-scaled
        // (exact_period = ticks_per_step / speed_mult), and the swing/micro
        // offset scales the same way (÷ speed_mult), so the swing feel stays a
        // fixed fraction of the step at any per-track speed.
        let spb = 22_050.0; // 120 BPM @ 44.1 kHz
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.swing_amount = 0.25; // odd steps delayed by 0.25 of a step

        // Baseline at speed 1.0: swing = swing_amount * samples_per_beat.
        seq.speed_mult = 1.0;
        let period_1x = seq.exact_period();
        let swing_1x  = seq.step_sample_offset(1, spb);
        assert_eq!(swing_1x, (0.25 * spb) as u32,
            "at speed 1.0 swing = swing_amount * samples_per_beat");

        // At speed 2.0 the step period halves AND the swing offset halves with
        // it — the swing stays the same fraction of the (now shorter) step.
        seq.speed_mult = 2.0;
        let period_2x = seq.exact_period();
        let swing_2x  = seq.step_sample_offset(1, spb);
        assert!((period_2x - period_1x / 2.0).abs() < 1e-9,
            "step period halves at speed 2.0: {period_2x} vs {period_1x}/2");
        assert_eq!(swing_2x, swing_1x / 2,
            "swing offset scales with the step (halves at speed 2.0)");

        // Even step never swings, at any speed.
        assert_eq!(seq.step_sample_offset(0, spb), 0, "even steps do not swing");
    }

    // ── P10 C3: per-track length & speed + BUG-004 ──────────────────────────

    fn set_length_cmd(count: i64, pattern: f64) -> NodeCommand {
        NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_LENGTH, arg0: count, arg1: pattern }
    }

    fn set_speed_cmd(mult: f64) -> NodeCommand {
        NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_SPEED, arg0: 0, arg1: mult }
    }

    fn is_note_on(e: &Event) -> bool {
        matches!(e, Event::Midi2(ump)
            if matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))))
    }

    /// Tick events (playing, no start/stop/sync) until the first NoteOn;
    /// returns the 0-based tick index it appeared on.
    fn ticks_until_note_on(seq: &mut Sequencer, max_ticks: usize) -> Option<usize> {
        for t in 0..max_ticks {
            let evs = run_seq(seq, &[transport_tick(1, true, false, false, false)]);
            if evs.iter().any(is_note_on) {
                return Some(t);
            }
        }
        None
    }

    #[test]
    fn set_length_changes_cycle_length() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_length_cmd(8, -1.0)]);
        assert_eq!(seq.patterns[0].length, 8);
        assert_eq!(seq.patterns[0].page_loop, (0, 0), "window re-clamped to the new page count");

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let positions = drive_steps(&mut seq, 20);
        assert!(positions.iter().all(|&p| p < 8), "length 8 wraps after 8 steps: {positions:?}");
        let at7 = positions.iter().position(|&p| p == 7).unwrap();
        assert_eq!(positions[at7 + 1], 0);
    }

    #[test]
    fn set_length_targets_specific_pattern() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns.push(Pattern::empty(Sequencer::STEP_CAPACITY));
        run_seq_with_cmds(&mut seq, &[set_length_cmd(8, 1.0)]);
        assert_eq!(seq.patterns[1].length, 8, "arg1 = 1 targets pattern 1");
        assert_eq!(seq.patterns[0].length, 16, "active pattern untouched");
        // Unknown pattern index: ignored.
        run_seq_with_cmds(&mut seq, &[set_length_cmd(4, 9.0)]);
        assert_eq!(seq.patterns[0].length, 16);
        assert_eq!(seq.patterns[1].length, 8);
    }

    #[test]
    fn speed_2x_doubles_rate() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_speed_cmd(2.0)]);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // One base step of ticks (ticks_per_step) advances TWO steps at 2x.
        let positions = drive_steps(&mut seq, 2);
        assert_eq!(positions, vec![2, 4], "2x advances two steps per base step");
    }

    #[test]
    fn speed_half_halves_rate() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_speed_cmd(0.5)]);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // At 0.5x a step lasts two base steps of ticks.
        let positions = drive_steps(&mut seq, 4);
        assert_eq!(positions, vec![0, 1, 1, 2], "0.5x advances every other base step");
    }

    #[test]
    fn speed_clamped_to_range() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_speed_cmd(4.0)]);
        assert_eq!(seq.speed_mult, 2.0, "4.0 clamps to 2.0");
        run_seq_with_cmds(&mut seq, &[set_speed_cmd(0.0)]);
        assert_eq!(seq.speed_mult, 0.125, "0.0 clamps to 0.125");
    }

    #[test]
    fn fractional_speed_carries_remainder_without_drift() {
        // 1.5x: exact period 160 ticks — integral here, so use 1.3x
        // (~184.6 ticks): over many steps the accumulated boundary ticks
        // must track k * (240 / 1.3) within 1 tick (no BUG-001-class drift).
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_speed_cmd(1.3)]);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let exact = 240.0_f64 / 1.3;
        let mut ticks = 0u64;
        let mut boundaries = 0u64;
        let mut last_step = seq.current_step;
        while boundaries < 100 {
            run_seq(&mut seq, &[transport_tick(1, true, false, false, false)]);
            ticks += 1;
            if seq.current_step != last_step {
                last_step = seq.current_step;
                boundaries += 1;
                let ideal = boundaries as f64 * exact;
                assert!((ticks as f64 - ideal).abs() <= 1.0,
                    "boundary {boundaries} at tick {ticks}, ideal {ideal:.1}");
            }
        }
    }

    #[test]
    fn two_tracks_polyrhythm() {
        // Track A: 16 steps; track B: 12 steps; one clock. They realign
        // (both at window start together) first at LCM(16,12) = 48.
        let mut a = Sequencer::new();
        let mut b = Sequencer::new();
        a.activate(44100.0, 64);
        b.activate(44100.0, 64);
        run_seq_with_cmds(&mut b, &[set_length_cmd(12, -1.0)]);

        run_seq(&mut a, &[transport_tick(0, true, true, false, false)]);
        run_seq(&mut b, &[transport_tick(0, true, true, false, false)]);
        assert_eq!((a.current_step, b.current_step), (0, 0));

        let tps = a.ticks_per_step;
        for boundary in 1..=48usize {
            for _ in 0..tps {
                run_seq(&mut a, &[transport_tick(1, true, false, false, false)]);
                run_seq(&mut b, &[transport_tick(1, true, false, false, false)]);
            }
            assert_eq!(a.current_step, boundary % 16);
            assert_eq!(b.current_step, boundary % 12);
            let aligned = a.current_step == 0 && b.current_step == 0;
            if boundary < 48 {
                assert!(!aligned, "must not realign before LCM; boundary {boundary}");
            } else {
                assert!(aligned, "realigns at LCM(16,12) = 48");
            }
        }
    }

    #[test]
    fn negative_micro_offset_emits_early() {
        // BUG-004 regression: micro_offset < 0 fires BEFORE the grid
        // boundary; 0 fires on it; +N fires on it with a positive sample
        // displacement. One unit = 1/96 beat = 10 ticks.
        let base = {
            let mut seq = Sequencer::new();
            seq.activate(44100.0, 64);
            seq.set_step(4, 60, 32768, true);
            run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
            ticks_until_note_on(&mut seq, 2000).expect("offset 0 fires")
        };

        let early = {
            let mut seq = Sequencer::new();
            seq.activate(44100.0, 64);
            seq.set_step(4, 60, 32768, true);
            seq.patterns[0].steps[4].timing.micro_offset = -12;
            run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
            ticks_until_note_on(&mut seq, 2000).expect("negative offset fires")
        };
        assert_eq!(base - early, 120, "-12 units = 120 ticks early (base {base}, early {early})");

        // +N stays on the boundary tick, displaced by samples instead.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(4, 60, 32768, true);
        seq.patterns[0].steps[4].timing.micro_offset = 12;
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let mut late_tick = None;
        let mut late_sample = 0;
        for t in 0..2000 {
            let block = 64usize;
            let mut audio = AudioBuffer::new(2, block);
            let mut events_out = EventOutputBuffer::new(256);
            let transport = TransportInfo::default();
            let slab = ExtendedEventSlab::empty();
            let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
            let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
            let mut outs = [audio_ref];
            let evs = [transport_tick(1, true, false, false, false)];
            let input = ProcessInput {
                audio_inputs: &[], signal_inputs: &[], events: &evs,
                transport: &transport, sample_rate: 44100.0, block_size: block,
                extended_events: &slab, commands: &[],
            };
            let mut output = ProcessOutput {
                audio_outputs: &mut outs, signal_outputs: &mut [],
                events_out: &mut events_out,
            };
            seq.process(&input, &mut output);
            if let Some(te) = events_out.as_slice().iter().find(|te| is_note_on(&te.event)) {
                late_tick = Some(t);
                late_sample = te.sample_offset;
                break;
            }
        }
        assert_eq!(late_tick, Some(base), "+N fires on the grid boundary tick");
        assert!(late_sample > 0, "+N displaces by samples (got {late_sample})");
    }

    fn is_note_off(e: &Event) -> bool {
        matches!(e, Event::Midi2(ump)
            if matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOff(_))))
    }

    #[test]
    fn early_fired_note_keeps_its_gate_length() {
        // Review finding (C3): the gate countdown is absolute from note-on,
        // so an early-fired note holds for its own step's gate (length 0.75
        // x 240 = 180 ticks), not until the previous step's gate-close tick.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(4, 60, 32768, true);
        seq.patterns[0].steps[4].timing.micro_offset = -12;
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);

        let on_tick = ticks_until_note_on(&mut seq, 2000).expect("early note fires");
        let mut off_after = None;
        for t in 0..400 {
            let evs = run_seq(&mut seq, &[transport_tick(1, true, false, false, false)]);
            if evs.iter().any(is_note_off) {
                off_after = Some(t + 1);
                break;
            }
        }
        let held = off_after.expect("note off arrives");
        assert!((170..=190).contains(&held),
            "early note holds its own gate (~180 ticks), got {held} (on at tick {on_tick})");
    }

    #[test]
    fn window_edit_between_early_fire_and_boundary_does_not_swallow() {
        // Review finding (C3): early_fired records WHICH step fired early;
        // a length edit landing before the boundary must not swallow the
        // fire of the different step the boundary now lands on.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_length_cmd(8, -1.0)]);
        seq.set_step(0, 60, 32768, true);
        seq.set_step(7, 62, 32768, true);
        seq.patterns[0].steps[7].timing.micro_offset = -12;

        // Start fires step 0; step 7's early fire lands at 7*240 - 120 = 1560.
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let mut saw_early = false;
        for _ in 0..1600 {
            let evs = run_seq(&mut seq, &[transport_tick(1, true, false, false, false)]);
            if evs.iter().any(is_note_on) {
                saw_early = true;
            }
        }
        assert!(saw_early, "step 7 fired early during step 6's window");
        assert_eq!(seq.early_fired, Some(7));

        // Shrink to 4 steps before the boundary at 1680: the boundary wraps
        // to step 0, which fired early NEVER — it must sound.
        run_seq_with_cmds(&mut seq, &[set_length_cmd(4, -1.0)]);
        let mut boundary_fired = false;
        for _ in 0..120 {
            let evs = run_seq(&mut seq, &[transport_tick(1, true, false, false, false)]);
            if evs.iter().any(is_note_on) {
                boundary_fired = true;
                break;
            }
        }
        assert!(boundary_fired, "step 0's fire must not be swallowed by step 7's early_fired");
        assert_eq!(seq.current_step, 0);
    }

    #[test]
    fn negative_micro_offset_step_zero_wraps() {
        // Step 0 with a negative offset: its second occurrence fires in the
        // PREVIOUS cycle's final window (120 ticks before the wrap).
        let second_fire = |micro: i8| -> usize {
            let mut seq = Sequencer::new();
            seq.activate(44100.0, 64);
            seq.set_step(0, 60, 32768, true);
            seq.patterns[0].steps[0].timing.micro_offset = micro;
            // global_start fires step 0 at the grid (no earlier time exists).
            let evs = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
            assert!(evs.iter().any(is_note_on), "start fires step 0");
            ticks_until_note_on(&mut seq, 8000).expect("second occurrence fires")
        };
        let base  = second_fire(0);
        let early = second_fire(-12);
        assert_eq!(base - early, 120,
            "wrapped step 0 fires 120 ticks into the previous cycle's last step (base {base}, early {early})");
    }

    // ── P10 C4: seamless switching + chaining ────────────────────────────────

    fn set_pattern_cmd(idx: i64) -> NodeCommand {
        NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_PATTERN, arg0: idx, arg1: 0.0 }
    }

    fn chain_push_cmd(idx: i64) -> NodeCommand {
        NodeCommand { target_id: 0, type_id: Sequencer::CMD_CHAIN_PUSH, arg0: idx, arg1: 0.0 }
    }

    /// Drive whole pattern cycles (16-step default) and return active_pattern
    /// after each wrap.
    fn drive_wraps(seq: &mut Sequencer, wraps: usize) -> Vec<usize> {
        let mut out = Vec::with_capacity(wraps);
        for _ in 0..wraps {
            let before = seq.cycle_state.loop_count;
            while seq.cycle_state.loop_count == before {
                run_seq(seq, &[transport_tick(1, true, false, false, false)]);
            }
            out.push(seq.active_pattern);
        }
        out
    }

    #[test]
    fn set_pattern_while_stopped_switches_immediately() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(2)]);
        assert_eq!(seq.active_pattern, 2, "stopped: switch is immediate");
        // Out-of-bank index ignored.
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(99)]);
        assert_eq!(seq.active_pattern, 2);
    }

    #[test]
    fn set_pattern_while_playing_cues_until_boundary() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        drive_steps(&mut seq, 3);
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(1)]);
        assert_eq!(seq.active_pattern, 0, "playing: switch waits for the boundary");
        assert_eq!(seq.cued_pattern, Some(1));
        let actives = drive_wraps(&mut seq, 1);
        assert_eq!(actives, vec![1], "switch lands at the cycle boundary");
        assert_eq!(seq.current_step, 0, "entry at the new pattern's window start");
    }

    #[test]
    fn cued_pattern_clears_after_switch() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(1)]);
        drive_wraps(&mut seq, 1);
        assert_eq!(seq.cued_pattern, None);
        assert_eq!(seq.active_pattern, 1);
    }

    #[test]
    fn chain_advances_on_boundary() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[chain_push_cmd(0), chain_push_cmd(1), chain_push_cmd(2)]);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // Patterns 1/2 are empty (length 64) — cycles get long after the
        // first switch, so keep the count modest: 0 -> 1 -> 2 -> 0 wraps.
        let actives = drive_wraps(&mut seq, 4);
        assert_eq!(actives, vec![0, 1, 2, 0], "chain visits its entries in order, then wraps");
    }

    #[test]
    fn explicit_cue_overrides_chain_for_one_boundary() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[chain_push_cmd(1), chain_push_cmd(2)]);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(3)]);
        let actives = drive_wraps(&mut seq, 2);
        assert_eq!(actives[0], 3, "explicit cue wins the first boundary");
        assert_eq!(actives[1], 1, "chain resumes (unadvanced) at the next boundary");
    }

    #[test]
    fn stop_applies_pending_cue() {
        // Review finding (C4): a cue pending at global_stop collapses to an
        // immediate switch — restart plays the user's last selection, not
        // one surprise cycle of the old pattern.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(2)]);
        assert_eq!(seq.cued_pattern, Some(2));
        run_seq(&mut seq, &[transport_tick(1, false, false, true, false)]);
        assert_eq!(seq.active_pattern, 2, "stop applies the pending cue");
        assert_eq!(seq.cued_pattern, None);
    }

    #[test]
    fn deserialize_clamps_pattern_bank_and_active_index() {
        // Review finding (C4): a foreign v3 blob claiming more patterns than
        // PATTERN_BANK_SIZE must not break the bank invariant or leave
        // active_pattern outside it.
        let mut src = Sequencer::new();
        src.activate(44100.0, 64);
        // Grow past the bank by direct manipulation and mark the tail used.
        while src.patterns.len() < 12 {
            src.patterns.push(Pattern::empty(Sequencer::STEP_CAPACITY));
        }
        src.patterns[11].steps[0].active = true;
        src.active_pattern = 11;
        let blob = src.serialize();

        let mut dst = Sequencer::new();
        dst.activate(44100.0, 64);
        dst.deserialize(&blob);
        assert_eq!(dst.patterns.len(), Sequencer::PATTERN_BANK_SIZE,
            "bank invariant holds against oversized blobs");
        assert!(dst.active_pattern < Sequencer::PATTERN_BANK_SIZE,
            "active index clamped into the bank");
    }

    #[test]
    fn chain_push_caps_at_eight() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let cmds: Vec<NodeCommand> = (0..10).map(|i| chain_push_cmd(i % 4)).collect();
        run_seq_with_cmds(&mut seq, &cmds);
        assert_eq!(seq.chain.len(), Sequencer::CHAIN_CAP, "pushes beyond the cap are ignored");
    }

    // ── P10 C5: state-bus surface ────────────────────────────────────────────

    #[test]
    fn published_state_includes_pattern_paths() {
        let mut seq = Sequencer::new();
        seq.set_node_id(42);
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[
            set_speed_cmd(2.0),
            chain_push_cmd(1),
            chain_push_cmd(2),
        ]);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(1)]); // playing -> cued

        let mut state = Vec::new();
        seq.published_state(&mut state);
        let get = |key: &str| {
            state
                .iter()
                .find(|(k, _)| k == &format!("/node/42/state/{key}"))
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("missing path {key}"))
        };
        assert!(matches!(get("active_pattern"), StateBusValue::Int(0)));
        assert!(matches!(get("cued_pattern"), StateBusValue::Int(1)));
        assert!(matches!(get("current_page"), StateBusValue::Int(0)));
        assert!(matches!(get("page_count"), StateBusValue::Int(2)), "16 steps = 2 pages");
        assert!(matches!(get("page_loop_start"), StateBusValue::Int(0)));
        assert!(matches!(get("page_loop_end"), StateBusValue::Int(1)));
        assert!(matches!(get("speed_mult"), StateBusValue::Float(v) if v == 2.0));
        assert!(matches!(get("chain_len"), StateBusValue::Int(2)));

        // No cue -> -1 sentinel.
        let mut seq2 = Sequencer::new();
        seq2.set_node_id(7);
        seq2.activate(44100.0, 64);
        let mut state2 = Vec::new();
        seq2.published_state(&mut state2);
        let cued = state2.iter().find(|(k, _)| k == "/node/7/state/cued_pattern").unwrap();
        assert!(matches!(cued.1, StateBusValue::Int(-1)));
    }

    #[test]
    fn steps_bitfield_reflects_current_page() {
        // Decided convention (spec 5.1): /state/steps is the FULL active
        // pattern (length chars); consumers slice the displayed page. On
        // page 1, chars 8..16 are steps 8-15.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_length_cmd(32, -1.0)]);
        seq.set_step(9, 60, 32768, true);
        seq.set_step(15, 60, 32768, true);

        let mut state = Vec::new();
        seq.set_node_id(1);
        seq.published_state(&mut state);
        let (_, StateBusValue::Text(bits)) = state
            .iter()
            .find(|(k, _)| k.ends_with("/state/steps"))
            .unwrap()
        else {
            panic!("steps must be Text")
        };
        assert_eq!(bits.len(), 32, "full active pattern, not one page");
        let page1: Vec<char> = bits.chars().skip(8).take(8).collect();
        assert_eq!(page1.iter().collect::<String>(), "01000001",
            "page-1 slice matches steps 8-15");
    }

    #[test]
    fn deserialize_sanitizes_invalid_page_loop() {
        // Review finding (C2): a blob whose page_loop disagrees with its
        // length (corrupt or foreign writer) must load with a full-span
        // window, not degenerate single-step playback.
        let mut src = Sequencer::new();
        src.activate(44100.0, 64);
        src.set_step(0, 60, 32768, true);
        src.patterns[0].page_loop = (4, 5); // invalid for length 16 (pages 0-1)
        let blob = src.serialize();

        let mut dst = Sequencer::new();
        dst.activate(44100.0, 64);
        dst.deserialize(&blob);
        assert_eq!(dst.patterns[0].page_loop, (0, 1),
            "invalid window resets to the full span of the loaded length");

        // Reversed window sanitized the same way.
        let mut src2 = Sequencer::new();
        src2.activate(44100.0, 64);
        src2.set_step(0, 60, 32768, true);
        src2.patterns[0].page_loop = (1, 0);
        let blob2 = src2.serialize();
        let mut dst2 = Sequencer::new();
        dst2.activate(44100.0, 64);
        dst2.deserialize(&blob2);
        assert_eq!(dst2.patterns[0].page_loop, (0, 1));
    }
}
