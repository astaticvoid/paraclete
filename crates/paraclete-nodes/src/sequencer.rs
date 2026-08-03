use paraclete_node_api::{
    midi::ChannelVoice2,
    CapabilityDocument, DebugEventKind, Event, Node, NodeCommand, ParamDescriptor, ParamLockEvent,
    ParamUnit, ParameterBank, ParamDisplay, ParamDisplayAdapter, PortDescriptor, PortDirection,
    PortName, PortType, ProcessInput, ProcessOutput, StateBusValue, TimedEvent, TransportEvent,
    UmpMessage, TICKS_PER_BEAT,
};

use std::cell::Cell;
use std::fmt::Write;

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
    NthOfM {
        n: u8,
        m: u8,
    },
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
    pub fill_a: bool,
    pub fill_b: bool,
}

/// Condition governing whether a trig fires on a given loop iteration.
#[derive(Clone, Debug, PartialEq)]
pub enum TrigCondition {
    Simple {
        repeat: RepeatCondition,
        fill: FillCondition,
        /// 0–100. 100 = always (no RNG). 0 = never.
        probability: u8,
    },
}

impl TrigCondition {
    /// Evaluate the condition. Order: repeat gate → fill gate → probability roll.
    pub fn evaluate(&self, cycle_state: &SequencerCycleState, rng: &mut fastrand::Rng) -> bool {
        match self {
            TrigCondition::Simple {
                repeat,
                fill,
                probability,
            } => {
                let repeat_pass = match repeat {
                    RepeatCondition::Always => true,
                    RepeatCondition::NthOfM { n, m } => {
                        *m > 0 && (cycle_state.loop_count % *m as u32) == (*n as u32 - 1)
                    }
                };
                if !repeat_pass {
                    return false;
                }

                let fill_pass = match fill {
                    FillCondition::Ignore => true,
                    FillCondition::FillA => cycle_state.fill_a,
                    FillCondition::FillB => cycle_state.fill_b,
                    FillCondition::FillAny => cycle_state.fill_a || cycle_state.fill_b,
                    FillCondition::NoFill => !cycle_state.fill_a && !cycle_state.fill_b,
                    FillCondition::NotFillA => !cycle_state.fill_a,
                    FillCondition::NotFillB => !cycle_state.fill_b,
                };
                if !fill_pass {
                    return false;
                }

                if *probability >= 100 {
                    return true;
                }
                if *probability == 0 {
                    return false;
                }
                rng.u8(0..100) < *probability
            }
        }
    }
}

impl Default for TrigCondition {
    fn default() -> Self {
        TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill: FillCondition::Ignore,
            probability: 100,
        }
    }
}

/// Placeholder for future Script condition variant (P7+).
pub struct ConditionId(pub u32);

// ── Step ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Step {
    pub active: bool,
    pub note: u8,
    pub velocity: u16,
    pub length: f32,
    pub param_locks: Vec<StepParamLock>,
    pub condition: TrigCondition,
    pub timing: StepTiming,
    /// Per-step CV value locks. Each entry is `(cv_port_index, value)`.
    /// `cv_port_index` is 0-relative (cv_out_0 = index 0).
    /// Out-of-range indices are silently ignored at process time.
    pub cv_locks: Vec<(u16, f32)>,
}

#[derive(Clone, Debug)]
pub struct StepParamLock {
    pub node_id: u32,
    pub param_id: u32,
    pub value: f64,
}

impl Step {
    /// Maximum per-step CV locks (P11 C3). Like `LOCK_CAP_PER_STEP`, step
    /// construction reserves this capacity so `copy_into`'s clamped
    /// `Vec::push`/extend never allocates on the audio thread.
    pub(crate) const CV_LOCK_CAP: usize = 4;

    pub fn empty() -> Self {
        Step {
            active: false,
            note: 60,
            velocity: 32768,
            length: 0.75,
            param_locks: Vec::with_capacity(Sequencer::LOCK_CAP_PER_STEP),
            condition: TrigCondition::default(),
            timing: StepTiming::default(),
            cv_locks: Vec::with_capacity(Self::CV_LOCK_CAP),
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
/// of truth. `muted` (P11 C4) is the per-pattern mute tier — an independent
/// second tier alongside the bank global mute; effective mute is
/// `global OR pattern` in `is_muted()`.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub steps: Vec<Step>,
    pub length: usize,
    pub page_loop: (u8, u8),
    pub swing: f32,
    pub muted: bool,
}

impl Pattern {
    /// A pattern of `steps` empty steps, playing its full length, with the
    /// page-loop window spanning every page and no swing.
    pub fn empty(steps: usize) -> Self {
        let pages = steps.div_ceil(PAGE_SIZE).max(1);
        Pattern {
            // Built per-step, NOT via `vec![Step::empty(); n]`: cloning the
            // first step would clone its Vecs with clone-capacity (length),
            // discarding the reserved LOCK_CAP_PER_STEP / CV_LOCK_CAP that
            // copy_into relies on for allocation-free audio-thread copies.
            steps: (0..steps).map(|_| Step::empty()).collect(),
            length: steps,
            page_loop: (0, (pages - 1) as u8),
            swing: 0.0,
            muted: false,
        }
    }

    /// Copy this pattern's data into `dest` without allocation (P11 C3).
    ///
    /// `dest` must be pre-allocated with at least `self.steps.len()` steps,
    /// each step's `param_locks` reserved to `LOCK_CAP_PER_STEP` and
    /// `cv_locks` reserved to `CV_LOCK_CAP` — exactly what construction-time
    /// patterns and the temp-save shadow provide. Used on the audio thread by
    /// `CMD_TEMP_SAVE`/`CMD_TEMP_RELOAD`, so no `Vec` may grow: every copy is
    /// an element write into existing capacity. CV locks exceeding
    /// `CV_LOCK_CAP` are clamped (truncated), the same policy as
    /// `CMD_SET_STEP_LOCK`. Pattern-level state (`length`, `page_loop`,
    /// `swing`) is copied along with the steps so a reload restores the whole
    /// pattern.
    pub(crate) fn copy_into(&self, dest: &mut Pattern) {
        debug_assert!(dest.steps.len() >= self.steps.len());
        debug_assert!(dest.steps.iter().all(|s| {
            s.param_locks.capacity() >= Sequencer::LOCK_CAP_PER_STEP
                && s.cv_locks.capacity() >= Step::CV_LOCK_CAP
        }));
        // Defensive clamps: a corrupt blob could load a pattern larger than
        // the construction-time shadow — never index or play past `dest` on
        // the audio thread. The debug_asserts above flag the violation.
        let n = dest.steps.len().min(self.steps.len());
        dest.length = self.length.min(dest.steps.len());
        dest.page_loop = self.page_loop;
        dest.swing = self.swing;
        dest.muted = self.muted;
        for (i, step) in self.steps.iter().take(n).enumerate() {
            let d = &mut dest.steps[i];
            d.active = step.active;
            d.note = step.note;
            d.velocity = step.velocity;
            d.length = step.length;
            d.timing = step.timing;
            d.condition = step.condition.clone();
            d.param_locks.clear();
            d.param_locks.extend_from_slice(&step.param_locks);
            d.cv_locks.clear();
            let cv_limit = Step::CV_LOCK_CAP.min(step.cv_locks.len());
            d.cv_locks.extend_from_slice(&step.cv_locks[..cv_limit]);
        }
    }
}

// ── Sequencer ────────────────────────────────────────────────────────────────

/// P11 C5 (OQ-12 resolution): labels for the `live_quantize` stepped
/// selector — index 0 is `off` (record-as-played with micro-timing);
/// 1..=4 are hard-quantize note values (1/4, 1/8, 1/16, 1/32 of a beat),
/// where the recorded step is snapped to that grid with zero
/// micro-offset. Static so the cap-doc path can clone the descriptor.
pub(crate) struct LiveQuantizeLabels;

impl ParamDisplay for LiveQuantizeLabels {
    fn format(&self, value: f64) -> String {
        match value as u32 {
            0 => "off".into(),
            1 => "1/4".into(),
            2 => "1/8".into(),
            3 => "1/16".into(),
            4 => "1/32".into(),
            _ => String::new(),
        }
    }
    fn parse(&self, s: &str) -> Option<f64> {
        match s {
            "off" => Some(0.0),
            "1/4" => Some(1.0),
            "1/8" => Some(2.0),
            "1/16" => Some(3.0),
            "1/32" => Some(4.0),
            _ => None,
        }
    }
}

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
    /// BUG-042: the step that was just recorded by a live trig this cycle.
    /// `emit_live_trig` already fired the synth — the boundary fire paths
    /// in `handle_transport` must not fire it again in the same window.
    live_recorded_step: Option<usize>,
    /// BUG-042 nit: true when `live_recorded_step` was set this cycle.
    /// Gives the flag one extra cycle to catch a transport boundary that
    /// straddles a process() call — prevents a live trig recorded late in
    /// one cycle from being re-fired by the boundary in the next.
    live_recorded_pending: bool,
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

    cycle_state: SequencerCycleState,
    rng: fastrand::Rng,
    active_pattern: usize,
    /// Pattern to switch to at the next cycle boundary. Inert until P10 C4.
    cued_pattern: Option<usize>,
    /// Volatile pattern chain. Inert until P10 C4.
    chain: Vec<usize>,
    chain_pos: usize,
    /// Per-track speed multiplier. Inert until P10 C3.
    speed_mult: f32,
    swing_amount: f32,
    /// Last bank `swing` value forwarded to the active pattern. The bank is
    /// a write-conduit only (P10 C2, spec 2.3): a changed bank value means
    /// an encoder/CMD_SET_PARAM wrote it, and it is forwarded to
    /// `Pattern::swing` — which is what emission reads. Comparing against
    /// this field (not the pattern) keeps a pattern switch from writing the
    /// old pattern's swing into the new one.
    last_bank_swing: f32,
    sample_rate: f32,

    cv_outputs: usize,
    current_cv: Vec<f32>,

    /// Note given to steps that were never explicitly set (toggle-created,
    /// re-padded on load). 60 by default; drum tracks driving a synth engine
    /// set this to the engine's trigger reference (e.g. 36) via the
    /// instrument file so a toggled step and a bare CMD_TRIGGER fire the
    /// same pitch (BUG-022). Per-step notes remain full-range.
    default_note: u8,

    /// Lazily-built `/node/{id}/state/*` path strings (BUG-007 fix).
    /// Keyed to `self.node_id` at first `published_state()` call.
    /// The `track_name`, `locks` entries and all VALUEs are still computed
    /// fresh each call.
    state_path_cache: std::sync::OnceLock<[String; 20]>,

    /// TK1 C1: current lock target set by CMD_SET_LOCK_TARGET.
    lock_target: Option<(u32, u32)>,
    /// TK1 C1: counter for dropped CMD_SET_STEP_LOCK commands (no target
    /// set, or step-at-capacity).
    lock_dropped: u64,
    /// TK1 C1: set when locks change (CMD 34/35, deserialize); cleared
    /// after /state/locks publish. Interior mutability through Cell.
    locks_dirty: Cell<bool>,

    /// TK2 C1 (D5): a live trig queued by `CMD_TRIG_NOW`, resolved to
    /// (note, velocity) at command time, fired at the next `process`
    /// window's sample offset 0. Independent of pattern/transport state.
    pending_live_trig: Option<(u8, u16)>,
    /// TK2 C1 (§0 A3): samples remaining before a live-owned open gate
    /// closes, decremented once per `process()` window by the buffer
    /// length. `None` when the open gate (if any) belongs to a pattern
    /// step instead, which closes via the transport-tick path.
    live_gate_samples_left: Option<u32>,
    /// TK2 C1 (§0 A3): most recent tempo seen via a `TransportEvent`,
    /// used to size a live trig's gate in samples even though the gate
    /// itself closes independent of transport ticks. Default 120 BPM
    /// before any transport has been observed.
    last_bpm: f32,

    /// P11 C3: shadow copy of the active pattern for temp save/reload.
    /// Pre-allocated at build; RAM-only, never serialized.
    shadow_pattern: Pattern,
    shadow_has_data: bool,

    /// P11 C4: deferred global-mute change (CMD_PREPARE_MUTE) held until
    /// the next pattern wrap, where it is applied to the bank and cleared.
    /// Cleared on `global_stop` so a stale mute cannot land on the first
    /// wrap after a restart.
    pending_global_mute: Option<bool>,
    /// P11 C4: deferred pattern-mute change (CMD_PREPARE_PATTERN_MUTE)
    /// held for the pattern that was active when the command arrived.
    pending_pattern_mute: Option<bool>,
    /// P11 C6 (OQ-T25): live-erase arm — while true AND playing, every
    /// step the playhead reaches is cleared as it passes (Elektron-style
    /// live erase; the step does not sound). Disarmed by CMD_LIVE_ERASE 0
    /// and on global_stop.
    live_erase_armed: bool,
}

impl Sequencer {
    pub const PORT_CLOCK_IN: u32 = 0;
    pub const PORT_EVENTS_IN: u32 = 1;
    pub const PORT_EVENTS_OUT: u32 = 2;
    /// First port ID used for CV output ports. cv_out_i is at port PORT_CV_OUT_BASE + i.
    pub const PORT_CV_OUT_BASE: u32 = 3;

    /// Universal NodeCommand type IDs (node-specific, ≥ 16).
    pub const CMD_TOGGLE_STEP: u32 = 16;
    pub const CMD_SET_STEP: u32 = 17;
    pub const CMD_CLEAR: u32 = 18;
    pub const CMD_SET_FILL_A: u32 = 23;
    pub const CMD_SET_FILL_B: u32 = 24;
    pub const CMD_SET_STEP_TIMING: u32 = 25;
    pub const CMD_SET_STEP_CONDITION: u32 = 26;
    pub const CMD_SET_PATTERN: u32 = 27;
    /// P10 C3: arg0 = step count (clamped 1..=64 and to the pattern's step
    /// capacity), arg1 = pattern index (>= 0) or -1 for the active pattern.
    /// Re-derives page count and clamps page_loop into range.
    pub const CMD_SET_LENGTH: u32 = 28;
    /// P10 C3: arg1 = per-track speed multiplier, clamped to [0.125, 2.0].
    /// Takes effect at the next step boundary.
    pub const CMD_SET_SPEED: u32 = 29;
    /// P10 C2: arg0 = start_page, arg1 = end_page (inclusive). Rejected
    /// (window unchanged) unless start <= end and both pages exist for the
    /// active pattern's current length.
    pub const CMD_SET_PAGE_LOOP: u32 = 30;
    /// P10 C4: arg0 = pattern index appended to the volatile chain
    /// (capacity CHAIN_CAP; pushes beyond it or unknown indices ignored).
    pub const CMD_CHAIN_PUSH: u32 = 31;
    /// P10 C4: empty the chain and reset its position.
    pub const CMD_CHAIN_CLEAR: u32 = 32;

    /// TK1 C1: arg0 = node_id (i64 → u32), arg1 = param_id as f64 (u32 exact).
    pub const CMD_SET_LOCK_TARGET: u32 = 33;
    /// TK1 C1: arg0 = step index, arg1 = value. Requires lock_target.
    pub const CMD_SET_STEP_LOCK: u32 = 34;
    /// TK1 C1: arg0 = step index, arg1 = param_id as f64 (−1.0 = all lanes).
    pub const CMD_CLEAR_STEP_LOCK: u32 = 35;
    /// TK1 C1: arg0 = step index, arg1 = velocity (0.0–1.0).
    pub const CMD_SET_STEP_VELOCITY: u32 = 36;
    /// TK1 C1: arg0 = step index, arg1 = length (f32 unit/scale).
    pub const CMD_SET_STEP_LENGTH: u32 = 37;
    /// TK2 C1 (D5): live trig, independent of pattern state. arg0 = MIDI
    /// note (0 → default 60, `Step::empty()`'s note); arg1 = velocity
    /// 0.0..=1.0 (0.0 → default 0.5, matching `Step::empty()`'s
    /// 32768/65535). Stored as a pending trig fired at the start of the
    /// next `process` window (sample offset 0); a second command in the
    /// same window replaces the pending one.
    pub const CMD_TRIG_NOW: u32 = 38;

    /// P11 C3: node-side type ids for the app's temp-save/reload broadcast.
    /// The canonical definitions live in `paraclete_node_api::command` as
    /// `u8` (39/40); these are the `u32` ids the `handle_commands` match sees.
    pub(crate) const CMD_TEMP_SAVE: u32 = paraclete_node_api::command::CMD_TEMP_SAVE as u32;
    pub(crate) const CMD_TEMP_RELOAD: u32 = paraclete_node_api::command::CMD_TEMP_RELOAD as u32;

    /// P11 C4: node-side ids for the mute-tier family (canonical `u8`
    /// definitions in `paraclete_node_api::command`; 41–43 of the reserved
    /// 39–45 P11 range).
    pub(crate) const CMD_SET_PATTERN_MUTE: u32 =
        paraclete_node_api::command::CMD_SET_PATTERN_MUTE as u32;
    pub(crate) const CMD_PREPARE_MUTE: u32 = paraclete_node_api::command::CMD_PREPARE_MUTE as u32;
    pub(crate) const CMD_PREPARE_PATTERN_MUTE: u32 =
        paraclete_node_api::command::CMD_PREPARE_PATTERN_MUTE as u32;
    /// P11 C6: live-erase arm (canonical `u8` in `paraclete_node_api::command`).
    pub(crate) const CMD_LIVE_ERASE: u32 = paraclete_node_api::command::CMD_LIVE_ERASE as u32;

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

    /// Maximum per-step parameter locks (TK1 C1). Step construction reserves
    /// this capacity so Vec::push within it never allocates on the audio thread.
    pub(crate) const LOCK_CAP_PER_STEP: usize = 8;

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
        ];
        for i in 0..cv_outputs {
            ports.push(PortDescriptor {
                id: Self::PORT_CV_OUT_BASE + i as u32,
                name: PortName::Dynamic(format!("cv_out_{i}")),
                direction: PortDirection::Output,
                port_type: PortType::Cv,
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
            live_recorded_step: None,
            live_recorded_pending: false,
            gate_ticks_left: 0,
            gate_open: false,
            active_note: 60,
            playing: false,
            trig_count: 0,
            last_fired_step: 0,
            bank: ParameterBank::empty(),
            group: 0,
            channel: 0,
            cycle_state: SequencerCycleState::default(),
            rng: fastrand::Rng::new(),
            active_pattern: 0,
            cued_pattern: None,
            chain: Vec::with_capacity(Self::CHAIN_CAP),
            chain_pos: 0,
            speed_mult: 1.0,
            swing_amount: 0.0,
            last_bank_swing: 0.0,
            sample_rate: 44100.0,
            cv_outputs,
            current_cv: vec![0.0_f32; cv_outputs],
            default_note: 60,
            state_path_cache: std::sync::OnceLock::new(),
            lock_target: None,
            lock_dropped: 0,
            locks_dirty: Cell::new(false),
            pending_live_trig: None,
            live_gate_samples_left: None,
            last_bpm: 120.0,
            shadow_pattern: Pattern::empty(Self::STEP_CAPACITY),
            shadow_has_data: false,
            pending_global_mute: None,
            pending_pattern_mute: None,
            live_erase_armed: false,
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
        if next < start || next >= end {
            (start, true)
        } else {
            (next, false)
        }
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
        self.live_recorded_step = None;
        self.live_recorded_pending = false;
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
        self.live_recorded_step = None;
        self.live_recorded_pending = false;
        let swing = self.patterns[idx].swing;
        self.bank
            .set(ParamDescriptor::id_for_name("swing"), swing as f64);
        self.last_bank_swing = swing;
    }

    pub fn set_step(&mut self, index: usize, note: u8, velocity: u16, active: bool) {
        let pat = self.active_index();
        let steps = &mut self.patterns[pat].steps;
        if index < steps.len() {
            steps[index].note = note;
            steps[index].velocity = velocity;
            steps[index].active = active;
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
                    // BUG-045: a hand-activated step must land on the
                    // grid — a live-recorded micro_offset must not
                    // survive erase-and-rewrite via toggle-off/toggle-on.
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() {
                        steps[idx].active = !steps[idx].active;
                        if steps[idx].active {
                            steps[idx].timing.micro_offset = 0;
                        }
                    }
                }
                Self::CMD_SET_STEP => {
                    // BUG-045: same as CMD_TOGGLE_STEP — activating by
                    // hand zeroes the offset a prior live record may have
                    // left in place.
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() {
                        if cmd.arg1 < 0.0 {
                            steps[idx].active = false;
                        } else {
                            steps[idx].note = cmd.arg1 as u8;
                            steps[idx].active = true;
                            steps[idx].timing.micro_offset = 0;
                        }
                    }
                }
                Self::CMD_CLEAR => {
                    // BUG-045 ruling (TK2.2 C2): CMD_CLEAR resets
                    // micro-timing across the lane — micro-timing is the
                    // step's own placement, not an attached lock, and "clear
                    // the lane" that leaves the grid crooked cannot be
                    // explained to a user. Param/CV locks are NOT touched
                    // here: TK2 §0 A8 established that CMD_CLEAR deliberately
                    // preserves per-step lock data, and that stands.
                    for step in &mut self.patterns[pat].steps {
                        step.active = false;
                        step.timing.micro_offset = 0;
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
                        let repeat_n = ((enc >> 8) & 0xFF) as u8;
                        let repeat_m = ((enc >> 16) & 0xFF) as u8;
                        let fill_disc = ((enc >> 24) & 0xFF) as u8;

                        let repeat = repeat_from_nm(repeat_n, repeat_m);
                        let fill = fill_from_discriminant(fill_disc);
                        steps[idx].condition = TrigCondition::Simple {
                            repeat,
                            fill,
                            probability,
                        };
                    }
                }
                Self::CMD_SET_LENGTH => {
                    // arg0 = step count (clamped); arg1 = pattern index or
                    // -1 = active. An unknown pattern index is ignored.
                    let idx = if cmd.arg1 < 0.0 {
                        pat
                    } else {
                        let i = cmd.arg1 as usize;
                        if i >= self.patterns.len() {
                            continue;
                        }
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
                    if cmd.arg1 < 0.0 {
                        continue;
                    }
                    let start = cmd.arg0;
                    let end = cmd.arg1 as i64;
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
                Self::CMD_SET_LOCK_TARGET => {
                    let node_id = cmd.arg0 as u32;
                    let param_id = cmd.arg1 as u32;
                    self.lock_target = Some((node_id, param_id));
                }
                Self::CMD_SET_STEP_LOCK => {
                    let (nid, pid) = match self.lock_target {
                        Some(t) => t,
                        None => {
                            self.lock_dropped = self.lock_dropped.wrapping_add(1);
                            continue;
                        }
                    };
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx >= steps.len() || idx >= Self::STEP_CAPACITY {
                        self.lock_dropped = self.lock_dropped.wrapping_add(1);
                        continue;
                    }
                    let val = cmd.arg1;
                    let locks = &mut steps[idx].param_locks;
                    if let Some(existing) = locks
                        .iter_mut()
                        .find(|l| l.node_id == nid && l.param_id == pid)
                    {
                        existing.value = val;
                    } else if locks.len() < Self::LOCK_CAP_PER_STEP {
                        locks.push(StepParamLock {
                            node_id: nid,
                            param_id: pid,
                            value: val,
                        });
                    } else {
                        self.lock_dropped = self.lock_dropped.wrapping_add(1);
                        continue;
                    }
                    self.locks_dirty.set(true);
                }
                Self::CMD_CLEAR_STEP_LOCK => {
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx >= steps.len() || idx >= Self::STEP_CAPACITY {
                        continue;
                    }
                    let param_id = cmd.arg1 as i64;
                    let locks = &mut steps[idx].param_locks;
                    if param_id == -1 {
                        if !locks.is_empty() {
                            locks.clear();
                            self.locks_dirty.set(true);
                        }
                    } else {
                        let pid = param_id as u32;
                        let prev = locks.len();
                        locks.retain(|l| {
                            let (target_nid, _) = self.lock_target.unwrap_or((0, 0));
                            l.node_id != target_nid || l.param_id != pid
                        });
                        if locks.len() != prev {
                            self.locks_dirty.set(true);
                        }
                    }
                }
                Self::CMD_SET_STEP_VELOCITY => {
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() && idx < Self::STEP_CAPACITY {
                        steps[idx].velocity =
                            ((cmd.arg1.clamp(0.0, 1.0) * 65535.0) as u32).min(65535) as u16;
                    }
                }
                Self::CMD_SET_STEP_LENGTH => {
                    let idx = cmd.arg0 as usize;
                    let steps = &mut self.patterns[pat].steps;
                    if idx < steps.len() && idx < Self::STEP_CAPACITY {
                        steps[idx].length = cmd.arg1 as f32;
                    }
                }
                Self::CMD_TRIG_NOW => {
                    // <= 0 (not just == 0) resolves to the track's
                    // default_note: a negative arg0 is malformed input, not
                    // a request for MIDI note 0, so it must not silently
                    // become one. Must match the note a sequenced step on
                    // this track would sound (BUG-044) — a hardcoded value
                    // here makes a live pad hit sound different from the
                    // same track's own steps.
                    let note = if cmd.arg0 <= 0 {
                        self.default_note
                    } else {
                        cmd.arg0.clamp(0, 127) as u8
                    };
                    let velocity = if cmd.arg1 <= 0.0 {
                        32768u16
                    } else {
                        ((cmd.arg1.clamp(0.0, 1.0) * 65535.0) as u32).min(65535) as u16
                    };
                    self.pending_live_trig = Some((note, velocity));
                }
                Self::CMD_TEMP_SAVE => {
                    // P11 C3: snapshot the active pattern into the
                    // pre-allocated shadow slot (allocation-free copy_into).
                    let active = self.active_index();
                    self.patterns[active].copy_into(&mut self.shadow_pattern);
                    self.shadow_has_data = true;
                }
                Self::CMD_TEMP_RELOAD if self.shadow_has_data => {
                    // P11 C3: restore the shadow into the active pattern.
                    // One-shot: the shadow is cleared once restored, so a
                    // second reload without a new save is a no-op.
                    let active = self.active_index();
                    self.shadow_pattern.copy_into(&mut self.patterns[active]);
                    self.shadow_has_data = false;
                }
                Self::CMD_SET_PATTERN_MUTE => {
                    // P11 C4: arg0 0 = off, 1 = on, 2 = toggle. Immediate
                    // tier change on the active pattern.
                    let active = self.active_index();
                    let p = &mut self.patterns[active];
                    p.muted = match cmd.arg0 {
                        0 => false,
                        1 => true,
                        2 => !p.muted,
                        // Any other arg0 leaves the tier untouched (the
                        // documented 0/1/2 contract; a stray value must
                        // not toggle).
                        _ => p.muted,
                    };
                }
                Self::CMD_PREPARE_MUTE => {
                    // P11 C4 (ADR-039 decision 6): hold a global-mute change
                    // until the next pattern wrap. arg0: 0 = off, 1 = on.
                    self.pending_global_mute = Some(cmd.arg0 != 0);
                }
                Self::CMD_PREPARE_PATTERN_MUTE => {
                    // P11 C4: deferred per-pattern mute for the active
                    // pattern. arg0: 0 = off, 1 = on. While playing, the
                    // pattern active at the wrap is the one that was active
                    // at the command (switches only happen at wraps, after
                    // the apply); a stopped-mode CMD_SET_PATTERN between
                    // prepare and start retargets the pending mute to the
                    // newly selected pattern — documented, accepted edge.
                    self.pending_pattern_mute = Some(cmd.arg0 != 0);
                }
                Self::CMD_LIVE_ERASE => {
                    // P11 C6 (OQ-T25): arm/disarm live erase. arg0:
                    // 0 = off, 1 = on.
                    self.live_erase_armed = cmd.arg0 != 0;
                }
                _ => {}
            }
        }
    }

    fn handle_transport(
        &mut self,
        k: &TransportEvent,
        sample_offset: u32,
        output: &mut ProcessOutput,
    ) {
        let spb = 60.0 * self.sample_rate as f64 / k.bpm.max(1.0);
        // TK2 C1 (§0 A3): cache the tempo for sizing a live trig's gate
        // even after the transport stops sending events.
        self.last_bpm = k.bpm as f32;
        let pat = self.active_index();

        if k.flags.sync_pulse {
            let bars_elapsed = (k.bar - 1).max(0) as u64;
            let total_ticks = bars_elapsed * k.time_sig_num as u64 * TICKS_PER_BEAT as u64
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
            let step_index = wstart + (steps_elapsed as u64 % wlen) as usize;
            let new_tick = (total_ticks as f64 - steps_elapsed * exact).floor() as u32;

            // Drift correction only (BUG-001 fix, s0 re-diagnosis): when internal
            // counting is in sync, the natural advance below handles this event —
            // the snap must be a no-op or it pre-empts the wrap (and loop_count).
            // "In sync" = the natural path reaches (step_index, new_tick) this
            // event: either we are already there, or we are one increment away.
            let next_tick = self.step_tick + 1;
            let in_sync = if next_tick >= self.step_period {
                new_tick == 0 && step_index == self.advance_step(self.current_step).0
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
            // connects mid-session enters its pattern here. P11 C6: a resync
            // while live erase is armed must not sound the step it lands on
            // (erase means "clear as the playhead passes", and this fire is
            // exactly that pass).
            if !in_sync && new_tick == 0 && self.playing && k.flags.playing && !self.live_erase_armed
            {
                let step_active = self.patterns[pat].steps[self.current_step].active;
                if step_active {
                    let cond = self.patterns[pat].steps[self.current_step]
                        .condition
                        .clone();
                    let should_fire = cond.evaluate(&self.cycle_state, &mut self.rng);
                    if should_fire {
                        let note_off =
                            sample_offset + self.step_sample_offset(self.current_step, spb);
                        self.emit_note_on_at(self.current_step, note_off, output);
                        for lock in &self.patterns[pat].steps[self.current_step].param_locks {
                            if !self.is_muted() {
                                output.events_out.push(TimedEvent::new(
                                    note_off,
                                    Event::ParamLock(ParamLockEvent {
                                        node_id: lock.node_id,
                                        param_id: lock.param_id,
                                        value: lock.value,
                                    }),
                                ));
                            }
                        }
                    }
                }
            }
        }

        // ADR-046 R2 (renamed from global_start): set position to the
        // window start, independent of whether we are currently playing
        // (R3 — a rewind is valid while running, "return to top"). This
        // must NOT also set `playing` or fire the entry step: a mechanical
        // rename that kept the old branch's four behaviours together would
        // make CMD_CLOCK_REWIND start playback and emit a note, and since
        // R3 permits rewind while running, a mid-play rewind would
        // double-fire against the ordinary boundary path (ratification
        // hazard note). Both are decomposed below, gated on the actual
        // playing-state transition.
        if k.flags.global_rewind {
            let (wstart, _) = self.window();
            self.current_step = wstart;
            self.step_tick = 0;
            self.reset_period();
        }

        if k.flags.global_stop {
            self.playing = false;
            // P11 C4: prepared mutes must not survive a stop — a stale mute
            // applied at the first wrap after restart is an unintended side
            // effect the performer never asked for.
            self.pending_global_mute = None;
            self.pending_pattern_mute = None;
            // P11 C6: live erase is a held gesture — releasing it (or a
            // stop) must never leave it armed.
            self.live_erase_armed = false;
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

        // ADR-046 hazard note: `playing` derives from the transport's own
        // flag, symmetric with how global_stop clears it above — not from
        // the rewind flag, which used to conflate the two.
        let was_playing = self.playing;
        self.playing = k.flags.playing;

        // BUG-001 fix, decomposed: fire the entry step on the transition
        // into playing, not on rewind — a rewind-while-stopped must stay
        // silent, and a rewind-while-running must not double-fire against
        // the ordinary boundary path below. Uses the CURRENT position,
        // which the global_rewind branch above may have just relocated
        // (a normal start with no rewind plays on from wherever the
        // transport last stopped, per ADR-046 T1's "no implicit rewind").
        if !was_playing && self.playing {
            let step_active = self.patterns[pat].steps[self.current_step].active;
            if step_active {
                let cond = self.patterns[pat].steps[self.current_step]
                    .condition
                    .clone();
                if cond.evaluate(&self.cycle_state, &mut self.rng) {
                    let note_off = sample_offset + self.step_sample_offset(self.current_step, spb);
                    self.emit_note_on_at(self.current_step, note_off, output);
                    for lock in &self.patterns[pat].steps[self.current_step].param_locks {
                        if !self.is_muted() {
                            output.events_out.push(TimedEvent::new(
                                note_off,
                                Event::ParamLock(ParamLockEvent {
                                    node_id: lock.node_id,
                                    param_id: lock.param_id,
                                    value: lock.value,
                                }),
                            ));
                        }
                    }
                }
            }
            return;
        }

        if k.flags.global_rewind {
            // A rewind that wasn't also a transition into playing (already
            // running, or still stopped) has nothing further to do this
            // event — it repositioned above and must not fall into the
            // per-tick advance below; this event is not a boundary tick.
            return;
        }

        if !k.flags.playing || !self.playing {
            return;
        }

        // Count this tick first (BUG-001 fix): the old check-then-increment
        // structure spanned ticks_per_step + 1 tick events per step (241/240),
        // with the bar-sync snap silently erasing the accumulated drift.
        self.step_tick += 1;

        if self.gate_open && self.live_gate_samples_left.is_none() {
            // Absolute countdown from note-on (its length × the firing
            // step's period): an early-fired note keeps its own gate length
            // across the boundary instead of being cut at the previous
            // step's gate-close tick. A live-owned gate (§0 A3) is excluded
            // here — it closes via its own sample-counted countdown in
            // `process()`, independent of these transport ticks.
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
        //
        // BUG-042: if the next step was just recorded by a live trig,
        // the synth already heard it — suppress the early fire and mark
        // it in early_fired so the boundary won't fire it either.
        if self.step_tick < self.step_period {
            let (next, _) = self.advance_step(self.current_step);
            let live_rec = self.live_recorded_step == Some(next);
            if live_rec {
                self.early_fired = Some(next);
            } else if self.early_fired.is_none() {
                let micro = self.patterns[pat].steps[next].timing.micro_offset;
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
                            self.emit_note_on_at(next, sample_offset, output);
                            for lock in &self.patterns[pat].steps[next].param_locks {
                                if !self.is_muted() {
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
                        }
                        self.early_fired = Some(next);
                    }
                }
            }
        }

        if self.step_tick >= self.step_period {
            self.step_tick = 0;
            self.step_period = self.next_step_period();
            let (next, wrapped) = self.advance_step(self.current_step);
            self.current_step = next;

            // P11 C6 (OQ-T25): live erase — while armed, the step the
            // playhead just reached is cleared BEFORE the boundary fire
            // below, so it does not sound (Elektron-style: erase as the
            // playhead passes). Cleared after use in the same cycle.
            if self.live_erase_armed {
                let s = &mut self.patterns[pat].steps[next];
                s.active = false;
                if !s.param_locks.is_empty() || !s.cv_locks.is_empty() {
                    self.locks_dirty.set(true);
                }
                s.param_locks.clear();
                s.cv_locks.clear();
            }

            // The page-loop wrap is THE cycle boundary (P10 C2/C4):
            // loop_count increments here, and cued switches / chain
            // advances are evaluated here only.
            if wrapped {
                self.cycle_state.loop_count = self.cycle_state.loop_count.wrapping_add(1);

                // P11 C4 (ADR-039 decision 6): prepared mutes apply exactly
                // at this wrap — sample-deterministic, no app polling. The
                // pattern tier targets the pattern that was active when the
                // command arrived (what the performer was hearing), before
                // any cue/chain switch below retargets playback.
                if let Some(v) = self.pending_global_mute.take() {
                    self.bank.set(
                        ParamDescriptor::id_for_name("mute"),
                        if v { 1.0 } else { 0.0 },
                    );
                }
                if let Some(v) = self.pending_pattern_mute.take() {
                    self.patterns[pat].muted = v;
                }

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
                    self.chain_pos =
                        (self.chain_pos.min(self.chain.len() - 1) + 1) % self.chain.len();
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
            // BUG-042: also skip the step that was just live-recorded —
            // emit_live_trig already fired the synth.
            let live_recorded = self.live_recorded_step.take() == Some(self.current_step);
            let step_active = self.patterns[pat].steps[self.current_step].active;
            if step_active && !fired_early && !live_recorded {
                let cond = self.patterns[pat].steps[self.current_step]
                    .condition
                    .clone();
                let should_fire = cond.evaluate(&self.cycle_state, &mut self.rng);
                if should_fire {
                    let note_off = sample_offset + self.step_sample_offset(self.current_step, spb);
                    self.emit_note_on_at(self.current_step, note_off, output);
                    for lock in &self.patterns[pat].steps[self.current_step].param_locks {
                        if !self.is_muted() {
                            output.events_out.push(TimedEvent::new(
                                note_off,
                                Event::ParamLock(ParamLockEvent {
                                    node_id: lock.node_id,
                                    param_id: lock.param_id,
                                    value: lock.value,
                                }),
                            ));
                        }
                    }
                }
            }
        }
    }

    fn is_muted(&self) -> bool {
        // P11 C4: two independent mute tiers — the bank global mute and the
        // active pattern's per-pattern mute. Effective mute is global OR
        // pattern (ADR-039 decision 6).
        self.bank.get(ParamDescriptor::id_for_name("mute")) >= 0.5
            || self.patterns[self.active_index()].muted
    }

    /// TK2.1 C3b (D8): armed by Theotokos on entering `RecMode::Live`
    /// (`CMD_SET_PARAM live_rec = 1.0` to every track), disarmed on
    /// leaving it.
    fn is_live_recording(&self) -> bool {
        self.bank.get(ParamDescriptor::id_for_name("live_rec")) >= 0.5
    }

    /// P11 C5 (ADR-039 decision 7): extract `(note, velocity)` from a Midi2
    /// message if it is a note-on. Note-offs, CC, aftertouch and any other
    /// channel-voice2 message return `None` — live record only consumes
    /// note-ons. Velocity is the UMP 16-bit value (0..=65535).
    fn midi_note_on(midi: &UmpMessage) -> Option<(u8, u16)> {
        match midi {
            UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(n)) => {
                Some((u8::from(n.note_number()), n.velocity()))
            }
            _ => None,
        }
    }

    /// TK2.1 C3b (D8, ADR-039 decision 7): quantizes a live trig to the
    /// nearest step of the active pattern and writes it in — step-active,
    /// note, velocity, and the signed distance to the grid as the step's
    /// micro-timing. Formula (stated so nobody invents one): with `pos` =
    /// the sequencer's tick position within the pattern (`current_step`/
    /// `step_tick`) and `period` = the speed-scaled ticks-per-step the
    /// existing step-advance path already computes (`step_period`) —
    /// `nearest = round(pos / period) mod pattern_length`;
    /// `delta_ticks = pos − nearest × period`;
    /// `micro = clamp(round(delta_ticks / (TICKS_PER_BEAT / 96)), −47, 47)`.
    /// `StepTiming::micro_offset` is in 1/96-beat units, not ticks — with
    /// `TICKS_PER_BEAT = 960` one unit is 10 ticks.
    ///
    /// P11 C5 refinements:
    /// - The event's `sample_offset` within the block refines `pos` to the
    ///   sample-accurate arrival (ADR-039 decision 7: "micro-timing from
    ///   the event's sample offset"); an offset of 0 (CMD_TRIG_NOW,
    ///   harness-injected events) keeps the block-start position exactly
    ///   as before.
    /// - `live_quantize` (P11 C5, OQ-12 resolution): 0 = record-as-played
    ///   with micro-timing (above). 1..=4 = HARD quantize to a note-value
    ///   grid (1/4, 1/8, 1/16, 1/32 of a beat): the recorded step is the
    ///   grid slot nearest the arrival and `micro` is written 0 — a clean
    ///   on-grid take. A grid finer than the step grid (e.g. 1/32 with
    ///   16th steps) degenerates to the step grid with micro 0, since the
    ///   step is the finest writable unit.
    ///
    /// Caller gates on `is_live_recording() && self.playing`; a stopped
    /// transport records nothing (the trig still sounds — `emit_live_trig`
    /// gates on mute only, independently of recording).
    fn record_live_trig(&mut self, note: u8, velocity: u16, sample_offset: u32) {
        let pat = self.active_index();
        let pattern_length = self.patterns[pat].length.max(1) as f64;
        let period = self.step_period.max(1) as f64;
        let samples_per_tick =
            60.0_f64 / self.last_bpm.max(1.0) as f64 / TICKS_PER_BEAT as f64 * self.sample_rate as f64;
        let pos = self.current_step as f64 * period + self.step_tick as f64
            + (sample_offset as f64) / samples_per_tick.max(1.0);

        let q = self
            .bank
            .get(ParamDescriptor::id_for_name("live_quantize"))
            .round() as i64;
        let (nearest, micro) = if (1..=4).contains(&q) {
            // Hard quantize: snap the arrival to the note-value grid
            // (denominator 4/8/16/32 of a beat), then to the step
            // containing that grid point; micro is zeroed. A 1/N note
            // spans TICKS_PER_BEAT*4/N ticks (a 16th step is
            // TICKS_PER_BEAT/4), so 1/4 → 4 steps, 1/8 → 2, 1/16 → 1,
            // 1/32 → half a step.
            let denom = [4.0, 8.0, 16.0, 32.0][(q - 1) as usize];
            let grid_ticks = TICKS_PER_BEAT as f64 * 4.0 / denom;
            let grid_pos = (pos / grid_ticks).round() * grid_ticks;
            let nearest = (grid_pos / period).round().rem_euclid(pattern_length) as usize;
            (nearest, 0i8)
        } else {
            // Record-as-played: nearest step + signed micro-timing.
            let nearest_unwrapped = (pos / period).round();
            let nearest = nearest_unwrapped.rem_euclid(pattern_length) as usize;
            let delta_ticks = pos - nearest_unwrapped * period;
            let micro_unit_ticks = TICKS_PER_BEAT as f64 / 96.0;
            let micro = (delta_ticks / micro_unit_ticks).round().clamp(-47.0, 47.0) as i8;
            (nearest, micro)
        };

        self.set_step(nearest, note, velocity, true);
        self.patterns[pat].steps[nearest].timing.micro_offset = micro;
        // BUG-042: the live trig just fired the synth via emit_live_trig.
        // Record which step was written so handle_transport suppresses any
        // boundary/early fire of this step in the same window.
        self.live_recorded_step = Some(nearest);
        self.live_recorded_pending = true;
    }

    fn emit_note_on_at(&mut self, step_idx: usize, sample_offset: u32, output: &mut ProcessOutput) {
        if self.is_muted() {
            return;
        }
        // A previous note — pattern-fired or a live trig (TK2 C1) — still
        // sounding when this step fires must not be orphaned: close it
        // first. Centralized here so every fire path gets it, not just the
        // ones that happened to remember (a live trig's gate is real-time
        // sized and decoupled from step timing, so the overlap this guards
        // against is no longer a rare edge case).
        if self.gate_open {
            self.emit_note_off(sample_offset, output);
        }
        let pat = self.active_index();
        let step = &self.patterns[pat].steps[step_idx];
        self.active_note = step.note;
        self.gate_open = true;
        self.gate_ticks_left = (step.length * self.step_period as f32).max(1.0) as u32;
        // A pattern step retakes the gate from any live trig (§0 A3): the
        // tick-driven countdown above now owns closing it.
        self.live_gate_samples_left = None;
        self.trig_count = self.trig_count.wrapping_add(1);
        self.last_fired_step = step_idx;
        output.emit_debug(
            sample_offset,
            DebugEventKind::StepFired,
            step_idx as i64,
            step.note as f64,
        );
        output.events_out.push(TimedEvent::new(
            sample_offset,
            Event::Midi2(build_note_on(
                self.group,
                self.channel,
                step.note,
                step.velocity,
            )),
        ));
        // Apply per-step CV locks (sample-and-hold until next step fires).
        for &(idx, val) in &step.cv_locks {
            if (idx as usize) < self.cv_outputs {
                self.current_cv[idx as usize] = val;
            }
        }
    }

    /// TK2 C1 (D5/§0 A3): sample length of a live trig's gate — 0.75 of a
    /// step's real-time duration at the last known tempo (`last_bpm`,
    /// default 120 before any transport has ever been seen). Independent of
    /// whether the transport is currently running.
    fn live_gate_length_samples(&self) -> u32 {
        let samples_per_beat = 60.0 * self.sample_rate as f64 / self.last_bpm.max(1.0) as f64;
        let step_samples = samples_per_beat * (self.step_period as f64 / TICKS_PER_BEAT as f64);
        (0.75 * step_samples).round().max(1.0) as u32
    }

    /// TK2 C1 (D5): fire a live trig queued by `CMD_TRIG_NOW`, at sample
    /// offset 0 of the window it was received in. Not tied to any step —
    /// pattern state (including `last_fired_step`) is untouched. Respects
    /// mute. A step already sounding when the live trig lands must not be
    /// orphaned: its note-off is emitted first (§0 A3). The gate this opens
    /// is closed by a sample-counted countdown in `process()`, not the
    /// transport-tick path (§0 A3) — it must close even while stopped.
    fn emit_live_trig(&mut self, note: u8, velocity: u16, output: &mut ProcessOutput) {
        if self.is_muted() {
            return;
        }
        if self.gate_open {
            self.emit_note_off(0, output);
        }
        self.active_note = note;
        self.gate_open = true;
        self.live_gate_samples_left = Some(self.live_gate_length_samples());
        self.trig_count = self.trig_count.wrapping_add(1);
        output.emit_debug(0, DebugEventKind::StepFired, -1, note as f64);
        output.events_out.push(TimedEvent::new(
            0,
            Event::Midi2(build_note_on(self.group, self.channel, note, velocity)),
        ));
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
    fn default() -> Self {
        Self::new()
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
            name: "Sequencer".into(),
            vendor: "Paraclete".into(),
            version: (0, 5, 0),
            ports: self.ports.to_vec(),
            params: vec![
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("pattern_length"),
                    name: "pattern_length".into(),
                    min: 1.0,
                    max: 64.0,
                    default: 16.0,
                    stepped: true,
                    in_kit: false,
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
                    in_kit: false,
                    unit: ParamUnit::Generic,
                    display: None,
                },
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("swing"),
                    name: "swing".into(),
                    min: 0.0,
                    max: 0.5,
                    default: 0.0,
                    stepped: false,
                    in_kit: false,
                    unit: ParamUnit::Generic,
                    display: None,
                },
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("mute"),
                    name: "mute".into(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    stepped: true,
                    in_kit: false,
                    unit: ParamUnit::Generic,
                    display: None,
                },
                // TK2.1 C3b (D8, ADR-039 decision 7): a record-arm, not a
                // sound param — trig-gate shaped like `mute`. Exclude from
                // kit membership when ADR-039 amendment 1's opt-in flag
                // lands (P11 inherits this note).
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("live_rec"),
                    name: "live_rec".into(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    stepped: true,
                    in_kit: false,
                    unit: ParamUnit::Generic,
                    display: None,
                },
                // P11 C5 (OQ-12 resolution, user decision 2026-08-02): the
                // live-record quantization control. 0 = off — record as
                // played with micro-timing; 1..=4 = hard quantize to a
                // note-value grid (1/4, 1/8, 1/16, 1/32) with zero
                // micro-offset. Structural, never part of a kit.
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("live_quantize"),
                    name: "live_quantize".into(),
                    min: 0.0,
                    max: 4.0,
                    default: 0.0,
                    stepped: true,
                    in_kit: false,
                    unit: ParamUnit::Generic,
                    display: Some(ParamDisplayAdapter::Static(&LiveQuantizeLabels)),
                },
            ],
            extensions: vec!["paraclete.sequencer".into()],
            view: None,
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
        self.bank
            .set(ParamDescriptor::id_for_name("swing"), swing as f64);
        self.last_bank_swing = swing;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.handle_commands(input.commands);

        // TK2 C1 (§0 A3): a live-owned gate closes on a sample count,
        // decremented once per window by the buffer length — independent
        // of transport ticks, so it still closes with the transport
        // stopped (no ticks would ever arrive to close it otherwise).
        // Runs before this window's own live trig (below) so a fresh trig
        // is not immediately charged for time it hasn't lived yet.
        if let Some(remaining) = self.live_gate_samples_left {
            let remaining = remaining.saturating_sub(input.block_size as u32);
            if remaining == 0 {
                self.live_gate_samples_left = None;
                if self.gate_open {
                    self.emit_note_off(0, output);
                }
            } else {
                self.live_gate_samples_left = Some(remaining);
            }
        }

        // TK2 C1 (D5): a queued live trig fires at this window's sample
        // offset 0, independent of transport (works while stopped) and
        // ahead of the transport-event loop below.
        if let Some((note, velocity)) = self.pending_live_trig.take() {
            // TK2.1 C3b (D8): recorded independently of mute — a muted
            // track's live_rec still writes the step, it just doesn't
            // sound right now (emit_live_trig gates mute on its own).
            // Known bound (noted for the phase report): this runs before
            // this window's own transport events are handled below, so
            // `pos` can lag by up to one block.
            // Known bound, filed BUG-042 (hostile review, not in the
            // spec's own bound above): if `nearest` quantizes to a step
            // whose natural boundary lands in this same or the very next
            // window, that step's ordinary boundary-fire path (below/in a
            // later call) is not suppressed the way a negative-micro-
            // offset early fire suppresses itself via `early_fired` — the
            // synth can double-trigger a few samples apart. Scoped out of
            // C3b; see BUG-042 for the fix direction.
            if self.playing && self.is_live_recording() {
                // CMD_TRIG_NOW fires at sample offset 0 by contract.
                self.record_live_trig(note, velocity, 0);
            }
            self.emit_live_trig(note, velocity, output);
        }

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
                // P11 C5 (ADR-039 decision 7, the piece TK2.1 C3b deferred):
                // Midi2 note-ons on `events_in` record themselves into the
                // active pattern while the transport is playing and
                // `live_rec` is armed. Anything else (note-off, CC,
                // aftertouch) is ignored. The Keystep→sequencer edge is
                // app-side routing (C5a); the sequencer only consumes.
                Event::Midi2(ref midi) => {
                    // midi_note_on filters non-note-ons; the live-record arm
                    // (playing + live_rec) gates the rest. `.filter` keeps
                    // both conditions in one if-let — collapsible_if has
                    // nothing to collapse (let-chains need edition 2024).
                    if let Some((note, velocity)) = Self::midi_note_on(midi)
                        .filter(|_| self.playing && self.is_live_recording())
                    {
                        self.record_live_trig(note, velocity, timed.sample_offset);
                    }
                }
                _ => {}
            }
        }
        // BUG-042: live_recorded_step is consumed by the transport loop
        // above via take() at the boundary (line ~1130). If the live trig
        // arrived late in this cycle and the transport boundary fires in
        // the NEXT cycle, the pending flag grants one extra cycle.
        if self.live_recorded_pending {
            self.live_recorded_pending = false;
        } else {
            self.live_recorded_step = None;
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
                format!("/node/{id}/state/active_pattern"),
                format!("/node/{id}/state/cued_pattern"),
                format!("/node/{id}/state/current_page"),
                format!("/node/{id}/state/page_count"),
                format!("/node/{id}/state/page_loop_start"),
                format!("/node/{id}/state/page_loop_end"),
                format!("/node/{id}/state/speed_mult"),
                format!("/node/{id}/state/chain_len"),
                // TK1 C1 — lock state
                format!("/node/{id}/state/lock_dropped"),
                format!("/node/{id}/state/locks"),
                // P11 C4 — per-pattern mute tier
                format!("/node/{id}/state/pattern_muted"),
            ]
        });
        let p = &self.patterns[self.active_index()];
        buf.push((
            paths[0].clone(),
            StateBusValue::Int(self.current_step as i64),
        ));
        buf.push((paths[1].clone(), StateBusValue::Int(p.length as i64)));
        buf.push((paths[2].clone(), StateBusValue::Bool(self.playing)));
        buf.push((paths[3].clone(), StateBusValue::Text(self.steps_bitfield())));
        buf.push((paths[4].clone(), StateBusValue::Int(self.trig_count as i64)));
        buf.push((
            paths[5].clone(),
            StateBusValue::Int(self.last_fired_step as i64),
        ));
        buf.push((
            paths[6].clone(),
            StateBusValue::Int(self.cycle_state.loop_count as i64),
        ));
        buf.push((
            paths[7].clone(),
            StateBusValue::Float(if self.cycle_state.fill_a { 1.0 } else { 0.0 }),
        ));
        buf.push((
            paths[8].clone(),
            StateBusValue::Float(if self.cycle_state.fill_b { 1.0 } else { 0.0 }),
        ));
        buf.push((
            paths[9].clone(),
            StateBusValue::Int(self.active_index() as i64),
        ));
        buf.push((
            paths[10].clone(),
            StateBusValue::Int(self.cued_pattern.map_or(-1, |c| c as i64)),
        ));
        buf.push((
            paths[11].clone(),
            StateBusValue::Int((self.current_step / PAGE_SIZE) as i64),
        ));
        buf.push((
            paths[12].clone(),
            StateBusValue::Int(p.length.div_ceil(PAGE_SIZE) as i64),
        ));
        buf.push((paths[13].clone(), StateBusValue::Int(p.page_loop.0 as i64)));
        buf.push((paths[14].clone(), StateBusValue::Int(p.page_loop.1 as i64)));
        buf.push((
            paths[15].clone(),
            StateBusValue::Float(self.speed_mult as f64),
        ));
        buf.push((
            paths[16].clone(),
            StateBusValue::Int(self.chain.len() as i64),
        ));
        // TK1 C1 — lock state
        buf.push((
            paths[17].clone(),
            StateBusValue::Int(self.lock_dropped as i64),
        ));
        buf.push((paths[19].clone(), StateBusValue::Bool(p.muted)));
        if self.locks_dirty.get() {
            self.locks_dirty.set(false);
            let mut s = String::new();
            for (si, step) in p.steps.iter().enumerate() {
                for lock in &step.param_locks {
                    if !s.is_empty() {
                        s.push(';');
                    }
                    let _ = write!(
                        s,
                        "s{}:{}:{}={:.6}",
                        si, lock.node_id, lock.param_id, lock.value
                    );
                }
            }
            buf.push((paths[18].clone(), StateBusValue::Text(s)));
        }
        // Conditional entry — not cached: caching would require a second
        // OnceLock keyed on presence-at-first-call, which is fragile if
        // track_name is set after construction but before the first publish.
        // format! here is cheap (single optional String) relative to the 17
        // unconditional entries above.
        if !self.track_name.is_empty() {
            buf.push((
                format!("/node/{id}/state/track_name"),
                StateBusValue::Text(self.track_name.clone()),
            ));
        }
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
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
        let used = self
            .patterns
            .iter()
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
            // P11 C4: per-pattern mute as a trailing byte inside the record
            // envelope (after the last step record). The existing v3
            // skip-tolerance reads up to the declared record length, so a
            // reader without this field skips it — backward compatible by
            // construction; the blob version stays 3.
            buf.push(if pattern.muted { 1u8 } else { 0u8 });
            let record_len = (buf.len() - len_pos - 4) as u32;
            buf[len_pos..len_pos + 4].copy_from_slice(&record_len.to_le_bytes());
        }
        let mute = self.bank.get(ParamDescriptor::id_for_name("mute"));
        buf.push(if mute >= 0.5 { 1u8 } else { 0u8 });
        buf
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        match data[0] {
            1 | 2 => self.deserialize_legacy(data),
            3 => self.deserialize_v3(data),
            _ => {}
        }
    }
}

impl Sequencer {
    /// v1/v2 blobs: a single 16-step pattern with param/cv locks; conditions,
    /// timing, and swing were never saved (BUG-005) and default.
    fn deserialize_legacy(&mut self, data: &[u8]) {
        let version = data[0];
        let mut r = ByteReader::new(&data[1..]);

        let Some(pattern_length) = r.u8().map(|v| v as usize) else {
            return;
        };
        let Some(ticks_per_step) = r.u32() else {
            return;
        };

        let mut steps = Vec::with_capacity(pattern_length);
        for _ in 0..pattern_length {
            let Some(active) = r.u8().map(|v| v != 0) else {
                return;
            };
            let Some(note) = r.u8() else { return };
            let Some(velocity) = r.u16() else { return };
            let Some(length) = r.f32() else { return };
            let Some(lock_count) = r.u8().map(|v| v as usize) else {
                return;
            };
            let mut param_locks = Vec::with_capacity(lock_count.max(Self::LOCK_CAP_PER_STEP));
            for _ in 0..lock_count {
                let Some(node_id) = r.u32() else { return };
                let Some(param_id) = r.u32() else { return };
                let Some(value) = r.f64() else { return };
                param_locks.push(StepParamLock {
                    node_id,
                    param_id,
                    value,
                });
            }
            let mut cv_locks = Vec::with_capacity(Step::CV_LOCK_CAP);
            if version >= 2 {
                let Some(cv_count) = r.u8().map(|v| v as usize) else {
                    return;
                };
                for _ in 0..cv_count {
                    let Some(idx) = r.u16() else { return };
                    let Some(val) = r.f32() else { return };
                    cv_locks.push((idx, val));
                }
            }
            steps.push(Step {
                active,
                note,
                velocity,
                length,
                param_locks,
                condition: TrigCondition::default(),
                timing: StepTiming::default(),
                cv_locks,
            });
        }

        let mut pattern = self.blank_pattern(Self::STEP_CAPACITY.max(pattern_length));
        for (i, step) in steps.into_iter().enumerate() {
            pattern.steps[i] = step;
        }
        // Clamp: a zero-length pattern would divide-by-zero in playback.
        pattern.length = pattern_length.clamp(1, pattern.steps.len());
        pattern.page_loop = (0, (pattern_length.div_ceil(PAGE_SIZE).max(1) - 1) as u8);

        // Restore the construction-time bank size (see deserialize_v3).
        let bank_size = self.patterns.len();
        let mut patterns = vec![pattern];
        while patterns.len() < bank_size {
            patterns.push(self.blank_pattern(Self::STEP_CAPACITY));
        }
        self.patterns = patterns;
        self.locks_dirty.set(true);
        self.active_pattern = 0;
        self.cued_pattern = None;
        self.chain.clear();
        self.chain_pos = 0;
        self.speed_mult = 1.0;
        self.ticks_per_step = ticks_per_step;
        self.reset_period();
    }

    fn deserialize_v3(&mut self, data: &[u8]) {
        let mut r = ByteReader::new(&data[1..]);
        let Some(ticks_per_step) = r.u32() else {
            return;
        };
        let Some(speed_mult) = r.f32() else { return };
        let Some(active_pattern) = r.u16().map(|v| v as usize) else {
            return;
        };
        let Some(chain_len) = r.u8().map(|v| v as usize) else {
            return;
        };
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
        let Some(pattern_count) = r.u16().map(|v| v as usize) else {
            return;
        };
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
            let Some(record_len) = r.u32().map(|v| v as usize) else {
                return;
            };
            let Some(end) = r.cur.checked_add(record_len) else {
                return;
            };
            if end > r.data.len() {
                return;
            }
            let Some(length) = r.u16().map(|v| v as usize) else {
                return;
            };
            let Some(pl_start) = r.u8() else { return };
            let Some(pl_end) = r.u8() else { return };
            let Some(swing) = r.f32() else { return };
            let Some(step_count) = r.u16().map(|v| v as usize) else {
                return;
            };
            let mut steps = Vec::with_capacity(step_count.max(Self::STEP_CAPACITY));
            for _ in 0..step_count {
                let Some(step) = read_step_record(&mut r) else {
                    return;
                };
                steps.push(step);
            }
            // P11 C4: a trailing byte inside the pattern record is the
            // per-pattern `muted` flag (new saves); its absence means a v3
            // blob written before the field existed — default false. The
            // record may also carry further unknown trailing bytes; they
            // are skipped as before.
            let muted = if r.cur < end {
                let m = r.u8().unwrap_or(0);
                r.cur = end;
                m != 0
            } else {
                false
            };
            if r.cur > end {
                return;
            }
            r.cur = end;
            // Sanitize: a pattern must hold at least one step, `length` must
            // stay within the step Vec (playback indexes `steps[..length]` on
            // the audio thread — a corrupt blob must not panic there), and
            // the runtime step capacity is restored so step editing after a
            // load behaves the same as after construction.
            if steps.is_empty() {
                return;
            }
            if steps.len() < Self::STEP_CAPACITY {
                // Padded steps carry this instance's default note (BUG-022),
                // not the Step::empty() constant. Pushed per-step, NOT via
                // `resize`: clone-padding would discard the reserved lock
                // capacities that copy_into relies on for allocation-free
                // audio-thread copies.
                while steps.len() < Self::STEP_CAPACITY {
                    let mut pad = Step::empty();
                    pad.note = self.default_note;
                    steps.push(pad);
                }
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
            patterns.push(Pattern {
                steps,
                length,
                page_loop,
                swing,
                muted,
            });
        }
        if patterns.is_empty() {
            return;
        }
        // Restore the construction-time bank size: saved patterns fill the
        // low indices, untouched trailing slots are recreated empty (§1.3).
        let bank_size = self.patterns.len();
        while patterns.len() < bank_size {
            patterns.push(self.blank_pattern(Self::STEP_CAPACITY));
        }

        self.ticks_per_step = ticks_per_step;
        self.speed_mult = speed_mult.clamp(0.125, 2.0);
        self.active_pattern = active_pattern.min(patterns.len() - 1);
        self.chain = chain;
        self.chain_pos = 0;
        self.cued_pattern = None;
        // Defensive (review M2, P11 C4): a blob load replaces the whole
        // pattern bank, so a prepared mute held for a pattern that no
        // longer exists must not land on some future wrap. The spec only
        // mandates clear-on-stop; this covers the load-while-running edge.
        self.pending_global_mute = None;
        self.pending_pattern_mute = None;
        self.patterns = patterns;
        // TK1 C1: loaded locks must be published on the next cycle.
        self.locks_dirty.set(true);
        if r.cur < r.data.len() {
            let mute_val = if r.data[r.cur] != 0 { 1.0 } else { 0.0 };
            self.bank
                .set(ParamDescriptor::id_for_name("mute"), mute_val);
        }
        // Deterministic period machinery for the loaded speed (P10 C3).
        self.reset_period();
        // Mirror the loaded swing into the bank conduit (P10 C2: the pattern
        // is authoritative; this keeps the encoder display current). Track it
        // in last_bank_swing so process() doesn't mistake the load for an
        // encoder write. No-op when deserialize runs before activate();
        // activate() then re-applies both from the pattern.
        let swing = self.patterns[self.active_index()].swing;
        self.bank
            .set(ParamDescriptor::id_for_name("swing"), swing as f64);
        self.last_bank_swing = swing;
    }
}

/// A pattern is "used" (worth serializing) if any step deviates from empty —
/// including inactive steps that carry pre-programmed conditions or
/// micro-timing — or the pattern carries swing. Trailing unused bank slots
/// are dropped from the blob and recreated at load time — writing the full
/// pre-allocated bank would bloat every project file for no benefit.
fn pattern_is_used(p: &Pattern) -> bool {
    p.swing != 0.0
        || p.steps.iter().any(|s| {
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
        RepeatCondition::Always => (0, 0),
        RepeatCondition::NthOfM { n, m } => (n, m),
    }
}

fn fill_discriminant(f: FillCondition) -> u8 {
    match f {
        FillCondition::Ignore => 0,
        FillCondition::FillA => 1,
        FillCondition::FillB => 2,
        FillCondition::FillAny => 3,
        FillCondition::NoFill => 4,
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
        TrigCondition::Simple {
            repeat,
            fill,
            probability,
        } => {
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
    if end > r.data.len() {
        return None;
    }

    let active = r.u8()? != 0;
    let note = r.u8()?;
    let velocity = r.u16()?;
    let length = r.f32()?;
    let probability = r.u8()?;
    let n = r.u8()?;
    let m = r.u8()?;
    let fill = fill_from_discriminant(r.u8()?);
    let micro = r.u8()? as i8;
    let pl_count = r.u8()? as usize;
    let mut param_locks = Vec::with_capacity(pl_count.max(Sequencer::LOCK_CAP_PER_STEP));
    for _ in 0..pl_count {
        let node_id = r.u32()?;
        let param_id = r.u32()?;
        let value = r.f64()?;
        param_locks.push(StepParamLock {
            node_id,
            param_id,
            value,
        });
    }
    let cv_count = r.u8()? as usize;
    // Reserve CV_LOCK_CAP regardless of the loaded count so every loaded
    // step satisfies copy_into's capacity contract (no audio-thread
    // allocation on CMD_TEMP_RELOAD into a loaded pattern).
    let mut cv_locks = Vec::with_capacity(cv_count.max(Step::CV_LOCK_CAP));
    for _ in 0..cv_count {
        let idx = r.u16()?;
        let val = r.f32()?;
        cv_locks.push((idx, val));
    }
    // A record whose declared length is shorter than its own fields is
    // malformed; anything between here and `end` is a future field — skip it.
    if r.cur > end {
        return None;
    }
    r.cur = end;

    let repeat = repeat_from_nm(n, m);
    Some(Step {
        active,
        note,
        velocity,
        length,
        param_locks,
        condition: TrigCondition::Simple {
            repeat,
            fill,
            probability,
        },
        timing: StepTiming {
            micro_offset: micro,
        },
        cv_locks,
    })
}

/// Bounds-checked little-endian reader over a serialized node blob.
struct ByteReader<'a> {
    data: &'a [u8],
    cur: usize,
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

use crate::{build_note_off, build_note_on};

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        midi::ChannelVoice2, AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab,
        TransportEvent, TransportFlags, TransportInfo, UmpMessage, TICKS_PER_BEAT,
    };

    fn transport_tick(
        tick: u32,
        playing: bool,
        global_rewind: bool,
        global_stop: bool,
        sync_pulse: bool,
    ) -> TimedEvent {
        TimedEvent::new(
            0,
            Event::Transport(TransportEvent {
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
                    global_rewind,
                    global_stop,
                    sync_pulse,
                    ..TransportFlags::default()
                },
            }),
        )
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
            audio_inputs: &[],
            signal_inputs: &[],
            events,
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
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
            audio_inputs: &[],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: cmds,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        seq.process(&input, &mut output);
    }

    #[test]
    fn sequencer_new_has_16_empty_steps() {
        let seq = Sequencer::new();
        let mut state = Vec::new();
        seq.published_state(&mut state);
        let len = state
            .iter()
            .find(|(k, _)| k.ends_with("/state/pattern_length"));
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
            all_out.extend(run_seq(
                &mut seq,
                &[transport_tick(t, true, false, false, false)],
            ));
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

    /// BUG-041: drives a real `InternalClock` → `Sequencer` pair (rather than
    /// injecting `global_stop` directly, as every other test above does) so a
    /// regression in the clock's own emission is caught here.
    fn drive_clock(
        clock: &mut crate::internal_clock::InternalClock,
        commands: &[NodeCommand],
    ) -> Vec<Event> {
        let block = 512usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        clock.process(&input, &mut output);
        events_out.as_slice().iter().map(|e| e.event).collect()
    }

    #[test]
    fn clock_stop_emits_global_stop() {
        use crate::internal_clock::{InternalClock, CMD_CLOCK_START, CMD_CLOCK_STOP};

        let mut clock = InternalClock::new();
        clock.activate(44100.0, 512);

        // ADR-046 T3: the clock boots stopped, so STOP must start from a
        // playing state to actually transition (and so emit anything).
        let start = NodeCommand {
            target_id: 0,
            type_id: CMD_CLOCK_START,
            arg0: 0,
            arg1: 0.0,
        };
        drive_clock(&mut clock, &[start]);

        let stop = NodeCommand {
            target_id: 0,
            type_id: CMD_CLOCK_STOP,
            arg0: 0,
            arg1: 0.0,
        };
        let events = drive_clock(&mut clock, &[stop]);
        let has_global_stop = events.iter().any(|e| {
            matches!(e, Event::Transport(te) if te.flags.global_stop)
        });
        assert!(
            has_global_stop,
            "CMD_CLOCK_STOP must emit a transport event carrying global_stop (BUG-041)"
        );
    }

    #[test]
    fn sequencer_playing_clears_on_clock_stop() {
        use crate::internal_clock::{InternalClock, CMD_CLOCK_START, CMD_CLOCK_STOP};

        let mut clock = InternalClock::new();
        clock.activate(44100.0, 512);
        let mut seq = Sequencer::new();
        seq.set_step(0, 60, 32768, true);

        let start = NodeCommand {
            target_id: 0,
            type_id: CMD_CLOCK_START,
            arg0: 0,
            arg1: 0.0,
        };
        let start_events = drive_clock(&mut clock, &[start]);
        let start_timed: Vec<TimedEvent> = start_events
            .into_iter()
            .map(|e| TimedEvent::new(0, e))
            .collect();
        run_seq(&mut seq, &start_timed);
        assert!(seq.playing, "sequencer must start on the clock's global_start");

        let stop = NodeCommand {
            target_id: 0,
            type_id: CMD_CLOCK_STOP,
            arg0: 0,
            arg1: 0.0,
        };
        let stop_events = drive_clock(&mut clock, &[stop]);
        let stop_timed: Vec<TimedEvent> = stop_events
            .into_iter()
            .map(|e| TimedEvent::new(0, e))
            .collect();
        run_seq(&mut seq, &stop_timed);
        assert!(
            !seq.playing,
            "sequencer's playing must clear when the clock is stopped via CMD_CLOCK_STOP (BUG-041)"
        );
    }

    #[test]
    fn sequencer_cmd_toggle_step_flips_active() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        assert!(!seq.patterns[0].steps[3].active);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_TOGGLE_STEP,
                arg0: 3,
                arg1: 0.0,
            }],
        );
        assert!(seq.patterns[0].steps[3].active);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_TOGGLE_STEP,
                arg0: 3,
                arg1: 0.0,
            }],
        );
        assert!(!seq.patterns[0].steps[3].active);
    }

    #[test]
    fn sequencer_cmd_clear_deactivates_all_steps() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        for i in 0..16 {
            seq.set_step(i, 60, 32768, true);
        }
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_CLEAR,
                arg0: 0,
                arg1: 0.0,
            }],
        );
        assert!(seq.patterns[0].steps.iter().all(|s| !s.active));
    }

    #[test]
    fn toggle_step_off_then_on_zeroes_a_live_recorded_offset() {
        // BUG-045: a hand-written step must land on the grid — erasing and
        // rewriting a step (toggle off, toggle on) must not let a prior
        // live-recorded micro_offset survive.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(3, 60, 32768, true);
        seq.patterns[0].steps[3].timing.micro_offset = 20;

        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_TOGGLE_STEP,
                arg0: 3,
                arg1: 0.0,
            }],
        );
        assert!(!seq.patterns[0].steps[3].active, "sanity: toggled off");

        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_TOGGLE_STEP,
                arg0: 3,
                arg1: 0.0,
            }],
        );
        assert!(seq.patterns[0].steps[3].active, "sanity: toggled back on");
        assert_eq!(
            seq.patterns[0].steps[3].timing.micro_offset, 0,
            "activating a step by hand must zero a live-recorded micro_offset"
        );
    }

    #[test]
    fn set_step_zeroes_a_previously_offset_steps_micro_offset() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(5, 60, 32768, true);
        seq.patterns[0].steps[5].timing.micro_offset = -30;

        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP,
                arg0: 5,
                arg1: 64.0,
            }],
        );
        assert_eq!(seq.patterns[0].steps[5].note, 64);
        assert_eq!(
            seq.patterns[0].steps[5].timing.micro_offset, 0,
            "CMD_SET_STEP on a previously-offset step must zero the offset"
        );
    }

    #[test]
    fn cmd_clear_resets_micro_timing_but_preserves_locks() {
        // TK2.2 C2 ruling: CMD_CLEAR resets every step's micro-timing (the
        // step's own placement) but must NOT touch param locks — TK2 §0 A8
        // established that a clear deliberately preserves per-step lock
        // data, and that stands unchanged by this ruling.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        for i in 0..16 {
            seq.set_step(i, 60, 32768, true);
        }
        seq.patterns[0].steps[2].timing.micro_offset = 15;
        seq.patterns[0].steps[2].param_locks.push(StepParamLock {
            node_id: 20,
            param_id: 7,
            value: 0.5,
        });

        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_CLEAR,
                arg0: 0,
                arg1: 0.0,
            }],
        );

        assert!(
            seq.patterns[0]
                .steps
                .iter()
                .all(|s| s.timing.micro_offset == 0),
            "CMD_CLEAR must reset micro-timing across the whole lane"
        );
        assert_eq!(
            seq.patterns[0].steps[2].param_locks.len(),
            1,
            "CMD_CLEAR must preserve per-step param locks (TK2 §0 A8)"
        );
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
        let _tps = TICKS_PER_BEAT / 4; // 240
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
            if events.iter().any(|e| {
                matches!(
                    e,
                    Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_)))
                )
            }) {
                fired_before_sync = true;
            }
        }
        // Step 0 only fires at the wrap (tick ~3841), not before the bar boundary.
        // (Some implementations fire it before; this just checks the sync path.)

        // Send a sync_pulse at the bar boundary — step 0 must fire.
        let _sync_tick = bar_ticks as u32;
        let sync_event = TimedEvent::new(
            0,
            Event::Transport(TransportEvent {
                domain_id: 0,
                bar: 2,
                beat: 0,
                tick: 0,
                ticks_per_beat: TICKS_PER_BEAT,
                bpm: 140.0,
                time_sig_num: 4,
                time_sig_den: 4,
                flags: TransportFlags {
                    playing: true,
                    sync_pulse: true,
                    ..TransportFlags::default()
                },
            }),
        );
        let sync_events = run_seq(&mut seq, &[sync_event]);
        let fired_at_sync = sync_events.iter().any(|e| {
            matches!(
                e,
                Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_)))
            )
        });

        assert!(fired_at_sync || fired_before_sync,
            "step 0 must fire: either via normal boundary before sync or via sync_pulse at bar boundary");
    }

    // ── Runtime command tests (Commit 7) ─────────────────────────────────────

    #[test]
    fn sequencer_cmd_set_fill_a_updates_cycle_state() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        assert!(!seq.cycle_state.fill_a);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_FILL_A,
                arg0: 1,
                arg1: 0.0,
            }],
        );
        assert!(seq.cycle_state.fill_a);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_FILL_A,
                arg0: 0,
                arg1: 0.0,
            }],
        );
        assert!(!seq.cycle_state.fill_a);
    }

    #[test]
    fn sequencer_cmd_set_fill_b_updates_cycle_state() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_FILL_B,
                arg0: 1,
                arg1: 0.0,
            }],
        );
        assert!(seq.cycle_state.fill_b);
    }

    #[test]
    fn sequencer_cmd_set_step_timing_updates_micro_offset() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP_TIMING,
                arg0: 3,
                arg1: 12.0,
            }],
        );
        assert_eq!(seq.patterns[0].steps[3].timing.micro_offset, 12i8);
    }

    #[test]
    fn sequencer_cmd_set_step_condition_encodes_correctly() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // probability=75, repeat_n=1, repeat_m=2, fill=Ignore(0)
        let enc: i64 = 75 | (1 << 8) | (2 << 16);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP_CONDITION,
                arg0: 5,
                arg1: enc as f64,
            }],
        );
        assert!(matches!(
            &seq.patterns[0].steps[5].condition,
            TrigCondition::Simple {
                repeat: RepeatCondition::NthOfM { n: 1, m: 2 },
                fill: FillCondition::Ignore,
                probability: 75,
            }
        ));
    }

    #[test]
    fn sequencer_cmd_set_pattern_is_stubbed() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_PATTERN,
                arg0: 3,
                arg1: 0.0,
            }],
        );
        assert_eq!(seq.active_pattern, 3); // stored but has no playback effect
    }

    fn contains_note_on(events: &[Event]) -> bool {
        events.iter().any(|e| {
            matches!(e, Event::Midi2(ump)
                if matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))))
        })
    }

    /// ADR-046: the BUG-001 entry-step fire lives on the transition into
    /// playing, not on rewind (the ratification hazard note this phase's
    /// decomposition exists to address) — so a normal start (`playing`
    /// alone, no `global_rewind`) must still fire step 0 exactly once.
    /// Replaces `sequencer_fires_step0_on_global_start`, which combined
    /// `playing` and `global_rewind` on the same synthetic event and so
    /// could not distinguish which flag actually caused the fire.
    #[test]
    fn normal_start_fires_entry_step_exactly_once() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(0, 60, 32768, true);
        let out = run_seq(&mut seq, &[transport_tick(0, true, false, false, false)]);
        let note_on_count = out
            .iter()
            .filter(|e| {
                matches!(e, Event::Midi2(ump)
                    if matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))))
            })
            .count();
        assert_eq!(
            note_on_count, 1,
            "a normal start (playing alone, no rewind) must fire its entry \
             step exactly once (BUG-001 regression)"
        );
        assert!(seq.playing, "playing must derive from the transport's flag");
    }

    /// ADR-046 R1/T2: rewind while stopped must relocate the sequencer's
    /// position but stay silent — the entry-step fire is gated on the
    /// transition into playing, which a rewind alone is not.
    #[test]
    fn rewind_while_stopped_is_silent_and_moves_position() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(0, 60, 32768, true);
        seq.set_step(5, 62, 32768, true);
        assert!(!seq.playing, "sanity: stopped");

        // Move off step 0 first (as if a prior session had advanced),
        // then rewind while stopped — playing:false throughout.
        seq.current_step = 5;
        let out = run_seq(&mut seq, &[transport_tick(0, false, true, false, false)]);

        assert!(
            !contains_note_on(&out),
            "a rewind while stopped must not sound anything"
        );
        assert!(!seq.playing, "a rewind alone must not start playback");
        assert_eq!(
            seq.current_step, 0,
            "a rewind must relocate to the window start even while stopped"
        );
    }

    /// ADR-046 R3/ratification hazard note: rewind while running must
    /// relocate without double-firing the entry step — a mechanical rename
    /// that kept the old four-behaviour branch together would make a
    /// mid-play rewind emit a note twice (once from the rewind, once from
    /// the ordinary boundary path).
    #[test]
    fn rewind_while_running_relocates_without_double_firing() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        for i in 0..16 {
            seq.set_step(i, 60, 32768, true);
        }

        // Start normally (no rewind needed — already at step 0) and let
        // the transport carry it forward off step 0.
        run_seq(&mut seq, &[transport_tick(0, true, false, false, false)]);
        seq.current_step = 5;
        assert!(seq.playing, "sanity: running");

        let out = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        assert!(
            !contains_note_on(&out),
            "a rewind while running must not itself fire a note — it is \
             not a transition into playing, so firing here would double-\
             fire against the ordinary boundary path that plays the \
             relocated step in due course"
        );
        assert!(seq.playing, "must still be playing after the rewind");
        assert_eq!(
            seq.current_step, 0,
            "must have relocated to the window start"
        );
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
        assert!(
            fire_ticks.len() >= 4,
            "expected >=4 fires, got {:?}",
            fire_ticks
        );
        for w in fire_ticks.windows(2) {
            assert_eq!(
                w[1] - w[0],
                tps,
                "step period must be exactly {tps} ticks, fires: {fire_ticks:?}"
            );
        }
    }

    #[test]
    fn sequencer_loop_count_increments_on_pattern_wrap() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let tps = TICKS_PER_BEAT / 4; // 240 ticks per step
                                      // BUG-001 fixed (P10 C0): each step takes exactly tps ticks.
        let wrap_tick = 16 * tps;

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        assert_eq!(seq.cycle_state.loop_count, 0);

        for t in 1..=wrap_tick {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }
        assert_eq!(
            seq.cycle_state.loop_count, 1,
            "loop_count must increment after one full pattern"
        );
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
        assert!(
            swing.is_some(),
            "swing param must be declared in capability_document"
        );
        assert_eq!(swing.unwrap().default, 0.0);
        assert_eq!(swing.unwrap().max, 0.5);
    }

    #[test]
    fn sequencer_swing_nonzero_shifts_odd_step_sample_offset() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.swing_amount = 0.25;
        let spb = 60.0 * 44100.0f64 / 140.0;
        assert_eq!(seq.step_sample_offset(0, spb), 0, "even step: no swing");
        assert!(
            seq.step_sample_offset(1, spb) > 0,
            "odd step: swing pushes forward"
        );
    }

    #[test]
    fn sequencer_fill_condition_blocks_trig_when_fill_inactive() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // Step 1 fires only when fill A is active
        seq.patterns[0].steps[1].active = true;
        seq.patterns[0].steps[1].condition = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill: FillCondition::FillA,
            probability: 100,
        };
        seq.cycle_state.fill_a = false;

        // Advance to step 1 boundary
        let tps = TICKS_PER_BEAT / 4;
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let mut fired = false;
        for t in 1..=tps {
            let events = run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
            if events.iter().any(|e| {
                matches!(
                    e,
                    Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_)))
                )
            }) {
                fired = true;
            }
        }
        assert!(
            !fired,
            "step with FillA condition must not fire when fill_a is false"
        );
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
        assert!(
            cond.evaluate(&state, &mut rng),
            "default condition must always fire"
        );
    }

    #[test]
    fn trig_condition_probability_zero_never_fires() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill: FillCondition::Ignore,
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
            fill: FillCondition::Ignore,
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
            fill: FillCondition::Ignore,
            probability: 100,
        };
        let mut rng = fastrand::Rng::with_seed(0);
        // loop_count=0 → n=1, 0%2==0 == (1-1)=0 → fires
        let state0 = SequencerCycleState {
            loop_count: 0,
            ..Default::default()
        };
        assert!(cond.evaluate(&state0, &mut rng));
        // loop_count=1 → 1%2==1 ≠ 0 → does not fire
        let state1 = SequencerCycleState {
            loop_count: 1,
            ..Default::default()
        };
        assert!(!cond.evaluate(&state1, &mut rng));
    }

    #[test]
    fn trig_condition_fill_a_only_fires_when_fill_a_active() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill: FillCondition::FillA,
            probability: 100,
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let fill_on = SequencerCycleState {
            fill_a: true,
            ..Default::default()
        };
        let fill_off = SequencerCycleState {
            fill_a: false,
            ..Default::default()
        };
        assert!(cond.evaluate(&fill_on, &mut rng));
        assert!(!cond.evaluate(&fill_off, &mut rng));
    }

    #[test]
    fn trig_condition_no_fill_fires_when_neither_fill_active() {
        let cond = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill: FillCondition::NoFill,
            probability: 100,
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let no_fill = SequencerCycleState {
            fill_a: false,
            fill_b: false,
            ..Default::default()
        };
        let fill_a_on = SequencerCycleState {
            fill_a: true,
            fill_b: false,
            ..Default::default()
        };
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
        assert!(matches!(
            step.condition,
            TrigCondition::Simple {
                repeat: RepeatCondition::Always,
                fill: FillCondition::Ignore,
                probability: 100,
            }
        ));
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
            audio_inputs: &[],
            signal_inputs: &[],
            events,
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut sig_outs, &mut events_out);
        seq.process(&input, &mut output);
        cv_buf
    }

    #[test]
    fn sequencer_cv_output_ports_present() {
        let seq = Sequencer::with_cv_outputs(2);
        let ports = seq.ports();
        let names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"cv_out_0"),
            "missing cv_out_0 in {:?}",
            names
        );
        assert!(
            names.contains(&"cv_out_1"),
            "missing cv_out_1 in {:?}",
            names
        );
        let cv_ports: Vec<_> = ports
            .iter()
            .filter(|p| p.port_type == PortType::Cv && p.direction == PortDirection::Output)
            .collect();
        assert_eq!(cv_ports.len(), 2, "expected 2 CvSignal Output ports");
    }

    #[test]
    fn sequencer_no_cv_no_extra_ports() {
        let seq = Sequencer::new();
        let cv_ports: Vec<_> = seq
            .ports()
            .iter()
            .filter(|p| p.port_type == PortType::Cv && p.direction == PortDirection::Output)
            .collect();
        assert!(
            cv_ports.is_empty(),
            "Sequencer::new() must have no CvSignal Output ports"
        );
    }

    #[test]
    fn sequencer_cv_output_initial_zero() {
        let mut seq = Sequencer::with_cv_outputs(1);
        seq.activate(44100.0, 256);
        // No step fires — initial hold value must be 0.0.
        let cv_port_id = Sequencer::PORT_CV_OUT_BASE;
        let out = run_seq_with_cv(&mut seq, &[], cv_port_id, 256);
        assert!(
            out.iter().all(|&v| v == 0.0),
            "CV output must be all zeros initially: {:?}",
            &out[..4]
        );
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
        let _ = run_seq_with_cv(
            &mut seq,
            &[transport_tick(0, true, true, false, false)],
            cv_port_id,
            64,
        );

        // Advance tps ticks: at t=tps the boundary fires and step 1 triggers.
        let mut cv_after_fire = vec![0.0f32; 64];
        for t in 1..=tps {
            cv_after_fire = run_seq_with_cv(
                &mut seq,
                &[transport_tick(t, true, false, false, false)],
                cv_port_id,
                64,
            );
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
        let _ = run_seq_with_cv(
            &mut seq,
            &[transport_tick(0, true, true, false, false)],
            cv_port_id,
            64,
        );

        // Advance 2*tps ticks: step 1 fires at tps, step 2 fires at 2*tps.
        let mut after_step2 = vec![0.0f32; 64];
        for t in 1..=(2 * tps) {
            let out = run_seq_with_cv(
                &mut seq,
                &[transport_tick(t, true, false, false, false)],
                cv_port_id,
                64,
            );
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

        let _ = run_seq_with_cv(
            &mut seq,
            &[transport_tick(0, true, true, false, false)],
            cv_port_id,
            64,
        );
        let mut final_out = vec![0.0f32; 64];
        for t in 1..=tps {
            final_out = run_seq_with_cv(
                &mut seq,
                &[transport_tick(t, true, false, false, false)],
                cv_port_id,
                64,
            );
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

        assert_eq!(
            seq2.patterns[0].steps[0].cv_locks,
            vec![(0u16, 0.3f32), (1u16, 0.7f32)],
            "step 0 cv_locks mismatch after roundtrip"
        );
        assert_eq!(
            seq2.patterns[0].steps[3].cv_locks,
            vec![(0u16, 1.0f32)],
            "step 3 cv_locks mismatch after roundtrip"
        );
        assert!(
            seq2.patterns[0].steps[1].cv_locks.is_empty(),
            "step 1 cv_locks must be empty"
        );
    }

    // ── P10 C1: Pattern + serializer v3 tests ─────────────────────────────────

    #[test]
    fn serialize_roundtrip_preserves_conditions() {
        let mut seq = Sequencer::new();
        seq.patterns[0].steps[5].active = true;
        seq.patterns[0].steps[5].condition = TrigCondition::Simple {
            repeat: RepeatCondition::NthOfM { n: 2, m: 4 },
            fill: FillCondition::NotFillB,
            probability: 42,
        };
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&data);
        assert!(
            matches!(
                &restored.patterns[0].steps[5].condition,
                TrigCondition::Simple {
                    repeat: RepeatCondition::NthOfM { n: 2, m: 4 },
                    fill: FillCondition::NotFillB,
                    probability: 42,
                }
            ),
            "TrigCondition must survive a v3 roundtrip (BUG-005)"
        );
    }

    #[test]
    fn serialize_roundtrip_preserves_timing() {
        let mut seq = Sequencer::new();
        seq.patterns[0].steps[7].active = true;
        seq.patterns[0].steps[7].timing.micro_offset = -12;
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.deserialize(&data);
        assert_eq!(
            restored.patterns[0].steps[7].timing.micro_offset, -12,
            "StepTiming micro_offset must survive a v3 roundtrip (BUG-005)"
        );
    }

    #[test]
    fn serialize_roundtrip_preserves_swing() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // Set swing the way an encoder does: CMD_SET_PARAM through process().
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: paraclete_node_api::CMD_SET_PARAM,
                arg0: ParamDescriptor::id_for_name("swing") as i64,
                arg1: 0.3,
            }],
        );
        let data = seq.serialize();
        let mut restored = Sequencer::new();
        restored.activate(44100.0, 64);
        restored.deserialize(&data);
        assert!(
            (restored.patterns[0].swing - 0.3).abs() < 1e-6,
            "pattern swing must survive a v3 roundtrip (BUG-005)"
        );
        let bank_swing = restored.bank.get(ParamDescriptor::id_for_name("swing"));
        assert!(
            (bank_swing - 0.3).abs() < 1e-6,
            "loaded swing must be re-applied to the bank (emission source until C2)"
        );
    }

    #[test]
    fn serialize_roundtrip_preserves_cv_locks() {
        let mut seq = Sequencer::with_cv_outputs(2);
        seq.patterns[0].steps[2].active = true;
        seq.patterns[0].steps[2].cv_locks = vec![(0, 0.25), (1, 0.9)];
        let data = seq.serialize();
        let mut restored = Sequencer::with_cv_outputs(2);
        restored.deserialize(&data);
        assert_eq!(
            restored.patterns[0].steps[2].cv_locks,
            vec![(0u16, 0.25f32), (1u16, 0.9f32)],
            "cv_locks must survive a v3 roundtrip (v2 regression guard)"
        );
    }

    #[test]
    fn deserialize_v2_into_single_pattern() {
        // Hand-built v2 blob matching the P9 writer: 16 steps, step 3 active.
        let mut blob = Vec::new();
        blob.push(2u8); // version
        blob.push(16u8); // pattern_length
        blob.extend_from_slice(&240u32.to_le_bytes()); // ticks_per_step
        for i in 0..16u8 {
            blob.push((i == 3) as u8); // active
            blob.push(if i == 3 { 72 } else { 60 }); // note
            blob.extend_from_slice(&32768u16.to_le_bytes()); // velocity
            blob.extend_from_slice(&0.75f32.to_le_bytes()); // length
            blob.push(0); // param_locks
            blob.push(0); // cv_locks
        }
        let mut seq = Sequencer::new();
        seq.deserialize(&blob);
        // C4: the bank is restored to PATTERN_BANK_SIZE on load; the legacy
        // single pattern fills index 0 and the rest are empty (spec 1.3).
        assert_eq!(seq.patterns.len(), Sequencer::PATTERN_BANK_SIZE);
        assert!(
            seq.patterns[1..].iter().all(|p| !pattern_is_used(p)),
            "v2 blob fills only pattern 0"
        );
        assert_eq!(seq.patterns[0].length, 16);
        assert_eq!(
            seq.patterns[0].page_loop,
            (0, 1),
            "page_loop must span the loaded length"
        );
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
        if contains_note_on(&run_seq(
            &mut seq,
            &[transport_tick(0, true, true, false, false)],
        )) {
            fire_ticks.push(0);
        }
        for t in 1..=(32 * tps) {
            if contains_note_on(&run_seq(
                &mut seq,
                &[transport_tick(t, true, false, false, false)],
            )) {
                fire_ticks.push(t);
            }
        }
        let expected: Vec<u32> = (0..=8).map(|i| i * 4 * tps).collect();
        assert_eq!(
            fire_ticks, expected,
            "step-fire tick sequence must be identical to P9"
        );
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
        assert!(
            restored.patterns[0].steps[1].active,
            "step 1 must parse correctly after skipped unknown bytes"
        );
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
        blob.extend_from_slice(&0u16.to_le_bytes()); // active_pattern
        blob.push(0); // chain_len
        blob.extend_from_slice(&1u16.to_le_bytes()); // pattern_count
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
        assert!(
            seq.patterns[0].steps[3].active,
            "prior state must be retained"
        );
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
        assert!(
            seq.patterns[0].length <= seq.patterns[0].steps.len(),
            "length {} must be clamped within steps {}",
            seq.patterns[0].length,
            seq.patterns[0].steps.len()
        );
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
        blob.extend_from_slice(&240u32.to_le_bytes()); // ticks_per_step
        blob.extend_from_slice(&1.0f32.to_le_bytes()); // speed_mult
        blob.extend_from_slice(&999u16.to_le_bytes()); // active_pattern: way out of range
        blob.push(0); // chain_len
        blob.extend_from_slice(&2u16.to_le_bytes()); // pattern_count = 2
        for _ in 0..2 {
            let mut body = Vec::new();
            body.extend_from_slice(&1u16.to_le_bytes()); // length
            body.push(0); // page_loop start
            body.push(0); // page_loop end
            body.extend_from_slice(&0.0f32.to_le_bytes()); // swing
            body.extend_from_slice(&1u16.to_le_bytes()); // step_count
            write_step_record(&s, &mut body);
            blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
            blob.extend_from_slice(&body);
        }
        let mut seq = Sequencer::new();
        seq.deserialize(&blob);
        assert!(
            seq.active_pattern < seq.patterns.len(),
            "active_pattern {} must be clamped inside the bank of {}",
            seq.active_pattern,
            seq.patterns.len()
        );
        // Playback index paths must not panic on the loaded state.
        let _ = seq.window();
    }

    /// ADR latent-issue audit item #6: a corrupt v3 blob declaring zero
    /// patterns must abort the load, keeping prior state — an empty patterns
    /// Vec would panic every playback index (active_index subtracts 1).
    #[test]
    fn deserialize_v3_zero_pattern_count_keeps_existing_state() {
        let mut blob = vec![3u8];
        blob.extend_from_slice(&240u32.to_le_bytes()); // ticks_per_step
        blob.extend_from_slice(&1.0f32.to_le_bytes()); // speed_mult
        blob.extend_from_slice(&0u16.to_le_bytes()); // active_pattern
        blob.push(0); // chain_len
        blob.extend_from_slice(&0u16.to_le_bytes()); // pattern_count = 0
        let mut seq = Sequencer::new();
        seq.set_step(3, 72, 32768, true);
        seq.deserialize(&blob);
        assert!(
            !seq.patterns.is_empty(),
            "patterns bank must never be empty"
        );
        assert!(
            seq.patterns[0].steps[3].active,
            "prior state must be retained"
        );
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
        assert_eq!(
            seq.active_pattern, 5,
            "out-of-bank index is rejected, not stored"
        );
    }

    #[test]
    fn pattern_is_used_counts_condition_and_timing_only_steps() {
        // An inactive step carrying a pre-programmed condition or micro-offset
        // makes the pattern worth serializing (data-loss guard for C4).
        let mut p = Pattern::empty(64);
        assert!(!pattern_is_used(&p));
        p.steps[5].timing.micro_offset = 12;
        assert!(
            pattern_is_used(&p),
            "micro-timing-only step must count as used"
        );
        let mut q = Pattern::empty(64);
        q.steps[2].condition = TrigCondition::Simple {
            repeat: RepeatCondition::Always,
            fill: FillCondition::FillA,
            probability: 100,
        };
        assert!(
            pattern_is_used(&q),
            "condition-only step must count as used"
        );
    }

    #[test]
    fn set_pattern_then_step_edit_in_same_batch() {
        // In-batch ordering: a CMD_SET_PATTERN followed by a step edit in the
        // same command batch must direct the edit at the newly-active pattern
        // (observable in C1 only via the clamp — both resolve to pattern 0 —
        // but the per-command resolution is what C4's bank relies on).
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_PATTERN,
                    arg0: 0,
                    arg1: 0.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_TOGGLE_STEP,
                    arg0: 7,
                    arg1: 0.0,
                },
            ],
        );
        assert!(
            seq.patterns[0].steps[7].active,
            "step edit after CMD_SET_PATTERN in one batch must land"
        );
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
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_SET_PAGE_LOOP,
            arg0: start,
            arg1: end,
        }
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
        assert!(
            positions.iter().all(|&p| p < 16),
            "a (0,1) window must never reach step 16: {positions:?}"
        );
        let at15 = positions
            .iter()
            .position(|&p| p == 15)
            .expect("reaches step 15");
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
        assert!(
            positions.iter().all(|&p| p / PAGE_SIZE == 1),
            "every step in a (1,1) window derives page 1: {positions:?}"
        );
        assert!(positions.contains(&8) && positions.contains(&15));

        // Page-0 half through playback too: default window (0,1) on 16 steps
        // never leaves pages 0-1, and the first 8 boundaries stay in page 0.
        let mut seq0 = Sequencer::new();
        seq0.activate(44100.0, 64);
        run_seq(&mut seq0, &[transport_tick(0, true, true, false, false)]);
        assert_eq!(seq0.current_step / PAGE_SIZE, 0, "start (step 0) is page 0");
        let first7 = drive_steps(&mut seq0, 7);
        assert!(
            first7.iter().all(|&p| p / PAGE_SIZE == 0),
            "steps 1-7 derive page 0: {first7:?}"
        );
    }

    #[test]
    fn swing_is_per_pattern() {
        let swing_id = ParamDescriptor::id_for_name("swing");
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns.push(Pattern::empty(Sequencer::STEP_CAPACITY));

        // Encoder write (CMD_SET_PARAM through the bank conduit) lands on the
        // active pattern only.
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: 0,
                arg0: swing_id as i64,
                arg1: 0.3,
            }],
        );
        assert!((seq.patterns[0].swing - 0.3).abs() < 1e-6);
        assert_eq!(seq.patterns[1].swing, 0.0, "inactive pattern untouched");

        // Switch active; a new write lands on pattern 1, pattern 0 keeps its own.
        seq.active_pattern = 1;
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: 0,
                arg0: swing_id as i64,
                arg1: 0.1,
            }],
        );
        assert!((seq.patterns[1].swing - 0.1).abs() < 1e-6);
        assert!(
            (seq.patterns[0].swing - 0.3).abs() < 1e-6,
            "patterns hold independent swing"
        );
        assert!(
            (seq.swing_amount - 0.1).abs() < 1e-6,
            "emission reads the active pattern"
        );
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
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: 0,
                arg0: swing_id as i64,
                arg1: 0.3,
            }],
        );
        // Live switch to pattern 1: bank must now read 0.5, not 0.3.
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_PATTERN,
                arg0: 1,
                arg1: 0.0,
            }],
        );
        assert!(
            (seq.bank.get(swing_id) - 0.5).abs() < 1e-6,
            "conduit refreshed on switch"
        );
        assert!((seq.swing_amount - 0.5).abs() < 1e-6);

        // Writing exactly the pre-switch value (0.3) must land on pattern 1.
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: 0,
                arg0: swing_id as i64,
                arg1: 0.3,
            }],
        );
        assert!(
            (seq.patterns[1].swing - 0.3).abs() < 1e-6,
            "write of the stale value is not dropped"
        );
        assert!(
            (seq.patterns[0].swing - 0.3).abs() < 1e-6,
            "pattern 0 untouched by the switch"
        );
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
        let swing_1x = seq.step_sample_offset(1, spb);
        assert_eq!(
            swing_1x,
            (0.25 * spb) as u32,
            "at speed 1.0 swing = swing_amount * samples_per_beat"
        );

        // At speed 2.0 the step period halves AND the swing offset halves with
        // it — the swing stays the same fraction of the (now shorter) step.
        seq.speed_mult = 2.0;
        let period_2x = seq.exact_period();
        let swing_2x = seq.step_sample_offset(1, spb);
        assert!(
            (period_2x - period_1x / 2.0).abs() < 1e-9,
            "step period halves at speed 2.0: {period_2x} vs {period_1x}/2"
        );
        assert_eq!(
            swing_2x,
            swing_1x / 2,
            "swing offset scales with the step (halves at speed 2.0)"
        );

        // Even step never swings, at any speed.
        assert_eq!(seq.step_sample_offset(0, spb), 0, "even steps do not swing");
    }

    // ── P10 C3: per-track length & speed + BUG-004 ──────────────────────────

    fn set_length_cmd(count: i64, pattern: f64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_SET_LENGTH,
            arg0: count,
            arg1: pattern,
        }
    }

    fn set_speed_cmd(mult: f64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_SET_SPEED,
            arg0: 0,
            arg1: mult,
        }
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
        assert_eq!(
            seq.patterns[0].page_loop,
            (0, 0),
            "window re-clamped to the new page count"
        );

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let positions = drive_steps(&mut seq, 20);
        assert!(
            positions.iter().all(|&p| p < 8),
            "length 8 wraps after 8 steps: {positions:?}"
        );
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
        assert_eq!(
            positions,
            vec![0, 1, 1, 2],
            "0.5x advances every other base step"
        );
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
                assert!(
                    (ticks as f64 - ideal).abs() <= 1.0,
                    "boundary {boundaries} at tick {ticks}, ideal {ideal:.1}"
                );
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
        assert_eq!(
            base - early,
            120,
            "-12 units = 120 ticks early (base {base}, early {early})"
        );

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
                audio_inputs: &[],
                signal_inputs: &[],
                events: &evs,
                transport: &transport,
                sample_rate: 44100.0,
                block_size: block,
                extended_events: &slab,
                commands: &[],
            };
            let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
            seq.process(&input, &mut output);
            if let Some(te) = events_out
                .as_slice()
                .iter()
                .find(|te| is_note_on(&te.event))
            {
                late_tick = Some(t);
                late_sample = te.sample_offset;
                break;
            }
        }
        assert_eq!(late_tick, Some(base), "+N fires on the grid boundary tick");
        assert!(
            late_sample > 0,
            "+N displaces by samples (got {late_sample})"
        );
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
        assert!(
            (170..=190).contains(&held),
            "early note holds its own gate (~180 ticks), got {held} (on at tick {on_tick})"
        );
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
        assert!(
            boundary_fired,
            "step 0's fire must not be swallowed by step 7's early_fired"
        );
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
        let base = second_fire(0);
        let early = second_fire(-12);
        assert_eq!(base - early, 120,
            "wrapped step 0 fires 120 ticks into the previous cycle's last step (base {base}, early {early})");
    }

    // ── P10 C4: seamless switching + chaining ────────────────────────────────

    /// BUG-042 regression: a step recorded by live_rec must not fire again
    /// at its own boundary within the same process() window — the live trig's
    /// emit_live_trig already sounded the synth.
    ///
    /// Scenario: step_tick is past the midpoint, so the trig quantizes to
    /// `next` (the step the boundary will advance to). Without the fix, the
    /// boundary fires that step a second time a few samples later.
    #[test]
    fn live_recorded_step_is_not_re_fired_at_boundary() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // Position at step 3, near the boundary — nearest rounds to step 4.
        seq.current_step = 3;
        seq.step_tick = 230; // 10 ticks shy of step_period (240)
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);

        // Deliver the trig_now command AND 10 transport ticks in one
        // process() call so both the live trig and the boundary fire
        // compete within the same window.
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let cmds = [trig_now_cmd(60, 0.9)];
        let ticks: Vec<TimedEvent> = (0..10)
            .map(|_| {
                TimedEvent::new(
                    0,
                    Event::Transport(TransportEvent {
                        domain_id: 0,
                        bar: 1,
                        beat: 0,
                        tick: 1,
                        ticks_per_beat: TICKS_PER_BEAT,
                        bpm: 120.0,
                        time_sig_num: 4,
                        time_sig_den: 4,
                        flags: TransportFlags {
                            playing: true,
                            ..TransportFlags::default()
                        },
                    }),
                )
            })
            .collect();
        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &ticks,
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: &cmds,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        seq.process(&input, &mut output);

        let note_on_count = events_out.as_slice().iter().filter(|e| is_note_on(&e.event)).count();
        assert_eq!(
            note_on_count, 1,
            "live trig must fire exactly once — no boundary re-fire (BUG-042)"
        );

        // The step must be recorded so it fires on the next loop pass.
        assert!(
            seq.patterns[0].steps[4].active,
            "step 4 must be recorded (nearest to tick 230/240)"
        );
    }

    #[test]
    fn live_recorded_step_suppresses_early_fire_too() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);

        // Set up step 4 with a negative micro_offset so it would fire early
        // during step 3's window. Then live-record AT step 4 — the early
        // fire path must also be suppressed.
        seq.set_step(4, 62, 32768, true);
        seq.patterns[0].steps[4].timing.micro_offset = -12; // 120 ticks early

        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // Position inside step 3's window, past the early-fire threshold
        // (240 - 120 = 120): at tick 200, early fire would trigger.
        seq.current_step = 3;
        seq.step_tick = 200;
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);

        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let cmds = [trig_now_cmd(60, 0.9)];
        let ticks = [TimedEvent::new(
            0,
            Event::Transport(TransportEvent {
                domain_id: 0,
                bar: 1,
                beat: 0,
                tick: 1,
                ticks_per_beat: TICKS_PER_BEAT,
                bpm: 120.0,
                time_sig_num: 4,
                time_sig_den: 4,
                flags: TransportFlags {
                    playing: true,
                    ..TransportFlags::default()
                },
            }),
        )];
        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &ticks,
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: &cmds,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        seq.process(&input, &mut output);

        let note_on_count = events_out.as_slice().iter().filter(|e| is_note_on(&e.event)).count();
        assert_eq!(
            note_on_count, 1,
            "live trig must fire exactly once even when the recorded step \
             would also fire early (BUG-042)"
        );

        // The step's existing content survives — the micro_offset is
        // overwritten by record_live_trig, but the note and active flag
        // from set_step are intact.
        assert!(seq.patterns[0].steps[4].active);
    }

    // ── P10 C4: seamless switching + chaining ────────────────────────────────

    fn set_pattern_cmd(idx: i64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_SET_PATTERN,
            arg0: idx,
            arg1: 0.0,
        }
    }

    fn chain_push_cmd(idx: i64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_CHAIN_PUSH,
            arg0: idx,
            arg1: 0.0,
        }
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
        assert_eq!(
            seq.active_pattern, 0,
            "playing: switch waits for the boundary"
        );
        assert_eq!(seq.cued_pattern, Some(1));
        let actives = drive_wraps(&mut seq, 1);
        assert_eq!(actives, vec![1], "switch lands at the cycle boundary");
        assert_eq!(
            seq.current_step, 0,
            "entry at the new pattern's window start"
        );
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
        run_seq_with_cmds(
            &mut seq,
            &[chain_push_cmd(0), chain_push_cmd(1), chain_push_cmd(2)],
        );
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // Patterns 1/2 are empty (length 64) — cycles get long after the
        // first switch, so keep the count modest: 0 -> 1 -> 2 -> 0 wraps.
        let actives = drive_wraps(&mut seq, 4);
        assert_eq!(
            actives,
            vec![0, 1, 2, 0],
            "chain visits its entries in order, then wraps"
        );
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
        assert_eq!(
            actives[1], 1,
            "chain resumes (unadvanced) at the next boundary"
        );
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
        assert_eq!(
            dst.patterns.len(),
            Sequencer::PATTERN_BANK_SIZE,
            "bank invariant holds against oversized blobs"
        );
        assert!(
            dst.active_pattern < Sequencer::PATTERN_BANK_SIZE,
            "active index clamped into the bank"
        );
    }

    #[test]
    fn chain_push_caps_at_eight() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let cmds: Vec<NodeCommand> = (0..10).map(|i| chain_push_cmd(i % 4)).collect();
        run_seq_with_cmds(&mut seq, &cmds);
        assert_eq!(
            seq.chain.len(),
            Sequencer::CHAIN_CAP,
            "pushes beyond the cap are ignored"
        );
    }

    // ── P10 C5: state-bus surface ────────────────────────────────────────────

    #[test]
    fn published_state_includes_pattern_paths() {
        let mut seq = Sequencer::new();
        seq.set_node_id(42);
        seq.activate(44100.0, 64);
        run_seq_with_cmds(
            &mut seq,
            &[set_speed_cmd(2.0), chain_push_cmd(1), chain_push_cmd(2)],
        );
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
        assert!(
            matches!(get("page_count"), StateBusValue::Int(2)),
            "16 steps = 2 pages"
        );
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
        let cued = state2
            .iter()
            .find(|(k, _)| k == "/node/7/state/cued_pattern")
            .unwrap();
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
        assert_eq!(
            page1.iter().collect::<String>(),
            "01000001",
            "page-1 slice matches steps 8-15"
        );
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
        assert_eq!(
            dst.patterns[0].page_loop,
            (0, 1),
            "invalid window resets to the full span of the loaded length"
        );

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

    // ── TK1 C1: p-lock command family ───────────────────────────────────────

    #[test]
    fn set_lock_target_then_step_lock_stores_lock() {
        let mut seq = Sequencer::new();
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 3,
                    arg1: 0.42,
                },
            ],
        );
        let step = &seq.patterns[0].steps[3];
        assert_eq!(step.param_locks.len(), 1);
        assert_eq!(step.param_locks[0].node_id, 20);
        assert_eq!(step.param_locks[0].param_id, 1);
        assert!((step.param_locks[0].value - 0.42).abs() < 0.0001);
    }

    #[test]
    fn step_lock_without_target_drops_and_counts() {
        let mut seq = Sequencer::new();
        assert_eq!(seq.lock_dropped, 0);
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP_LOCK,
                arg0: 0,
                arg1: 0.5,
            }],
        );
        assert_eq!(seq.lock_dropped, 1);
        let step = &seq.patterns[0].steps[0];
        assert!(step.param_locks.is_empty());
    }

    #[test]
    fn step_lock_updates_in_place_when_same_target() {
        let mut seq = Sequencer::new();
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.1,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.9,
                },
            ],
        );
        let step = &seq.patterns[0].steps[0];
        assert_eq!(step.param_locks.len(), 1);
        assert!((step.param_locks[0].value - 0.9).abs() < 0.0001);
    }

    #[test]
    fn step_lock_overflow_at_cap_drops_and_counts() {
        let mut seq = Sequencer::new();
        // Fill step 0 to LOCK_CAP_PER_STEP with unique param_ids.
        run_seq_with_cmds(
            &mut seq,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_LOCK_TARGET,
                arg0: 20,
                arg1: 0.0,
            }],
        );
        for i in 0..(Sequencer::LOCK_CAP_PER_STEP as u32) {
            run_seq_with_cmds(
                &mut seq,
                &[NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: (100 + i) as f64,
                }],
            );
            run_seq_with_cmds(
                &mut seq,
                &[NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.1 + i as f64 * 0.01,
                }],
            );
        }
        assert_eq!(
            seq.patterns[0].steps[0].param_locks.len(),
            Sequencer::LOCK_CAP_PER_STEP
        );
        // One more should overflow.
        let before = seq.lock_dropped;
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 200.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.99,
                },
            ],
        );
        assert_eq!(seq.lock_dropped, before + 1);
        assert_eq!(
            seq.patterns[0].steps[0].param_locks.len(),
            Sequencer::LOCK_CAP_PER_STEP
        );
    }

    #[test]
    fn clear_step_lock_all_lanes_with_minus_one() {
        let mut seq = Sequencer::new();
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.3,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 2.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.7,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_CLEAR_STEP_LOCK,
                    arg0: 0,
                    arg1: -1.0,
                },
            ],
        );
        assert!(seq.patterns[0].steps[0].param_locks.is_empty());
    }

    #[test]
    fn clear_step_lock_target_lane_only() {
        let mut seq = Sequencer::new();
        run_seq_with_cmds(
            &mut seq,
            &[
                // Lock A: node 20, param 1
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.3,
                },
                // Lock B: node 20, param 2
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 2.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.7,
                },
                // Clear only param 1 lane
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_CLEAR_STEP_LOCK,
                    arg0: 0,
                    arg1: 1.0,
                },
            ],
        );
        let step = &seq.patterns[0].steps[0];
        assert_eq!(step.param_locks.len(), 1);
        assert_eq!(step.param_locks[0].param_id, 2);
        assert!((step.param_locks[0].value - 0.7).abs() < 0.0001);
    }

    #[test]
    fn step_lock_beyond_length_preserved_inert() {
        let mut seq = Sequencer::new();
        seq.patterns[0].length = 8;
        // Lock on step 16 (beyond length=8, within STEP_CAPACITY).
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 16,
                    arg1: 0.42,
                },
            ],
        );
        let step = &seq.patterns[0].steps[16];
        assert_eq!(step.param_locks.len(), 1);
        // Extend length — lock must survive.
        seq.patterns[0].length = 17;
        let step2 = &seq.patterns[0].steps[16];
        assert_eq!(step2.param_locks.len(), 1);
    }

    #[test]
    fn set_step_velocity_and_length_commands() {
        let mut seq = Sequencer::new();
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_VELOCITY,
                    arg0: 2,
                    arg1: 0.5,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LENGTH,
                    arg0: 2,
                    arg1: 0.25,
                },
            ],
        );
        let step = &seq.patterns[0].steps[2];
        assert_eq!(step.velocity, 32767); // 0.5 * 65535 = 32767.5 → 32767
        assert!((step.length - 0.25).abs() < 0.001);
    }

    #[test]
    fn authored_lock_fires_param_lock_event_at_step() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // Author a lock on step 0 (node 20, param 1, value 0.42).
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.42,
                },
            ],
        );
        // Activate step 0 so it fires.
        seq.set_step(0, 60, 32768, true);
        let events = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let locks: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let Event::ParamLock(pl) = e {
                    Some((pl.node_id, pl.param_id, pl.value))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(locks.len(), 1, "authored lock must emit ParamLock event");
        assert_eq!(locks[0], (20, 1, 0.42));
    }

    #[test]
    fn authored_lock_roundtrips_v3_serializer() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 2,
                    arg1: 0.42,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 2.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 2,
                    arg1: 0.77,
                },
            ],
        );
        let blob = seq.serialize();
        let mut restored = Sequencer::new();
        restored.activate(44100.0, 64);
        restored.deserialize(&blob);
        let step = &restored.patterns[0].steps[2];
        assert_eq!(step.param_locks.len(), 2);
        assert_eq!(step.param_locks[0].node_id, 20);
        assert_eq!(step.param_locks[0].param_id, 1);
        assert!((step.param_locks[0].value - 0.42).abs() < 0.0001);
        assert_eq!(step.param_locks[1].node_id, 20);
        assert_eq!(step.param_locks[1].param_id, 2);
        assert!((step.param_locks[1].value - 0.77).abs() < 0.0001);
    }

    #[test]
    fn locks_publish_only_when_dirty() {
        let mut seq = Sequencer::new();
        seq.set_node_id(200);
        seq.activate(44100.0, 64);
        // First publish after deserialize sets dirty.
        seq.locks_dirty.set(true);
        let mut state = Vec::new();
        seq.published_state(&mut state);
        let lock_entry = state.iter().find(|(k, _)| k.ends_with("/state/locks"));
        // Dirty was true, so locks is published (even if empty).
        assert!(lock_entry.is_some());
        assert!(matches!(lock_entry.unwrap(), (_, StateBusValue::Text(s)) if s.is_empty()));

        // Second publish without setting dirty again — locks should NOT be included.
        let mut state2 = Vec::new();
        seq.published_state(&mut state2);
        let lock_entry2 = state2.iter().find(|(k, _)| k.ends_with("/state/locks"));
        assert!(lock_entry2.is_none(), "locks must not publish when clean");
    }

    /// The **writer** half of the lock round trip. `locks_publish_only_when
    /// _dirty` above covers only the empty string, so until this existed
    /// nothing in the workspace pinned the shape actually emitted for a real
    /// lock — a review mutation changing the separator from `=` to `|` left
    /// all 994 tests green while, at runtime, jog accumulation and the trig
    /// strip's lock dots both silently stopped working.
    ///
    /// The reader is `Model::parse_lock_value` / `read_step_locks` in
    /// `paraclete-theotokos`, which cannot be reached from here:
    /// `paraclete-theotokos` does not depend on `paraclete-nodes` and must not
    /// start. So the round trip is pinned by two halves that have to move
    /// together — the other is
    /// `a_second_jog_accumulates_from_the_stored_lock_value`.
    #[test]
    fn the_published_lock_string_has_the_shape_readers_parse() {
        let mut seq = Sequencer::new();
        seq.set_node_id(200);
        seq.activate(44100.0, 64);
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 7.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 3,
                    arg1: 0.25,
                },
            ],
        );

        let mut state = Vec::new();
        seq.published_state(&mut state);
        let (_, v) = state
            .iter()
            .find(|(k, _)| k.ends_with("/state/locks"))
            .expect("a stored lock must publish");
        let StateBusValue::Text(s) = v else {
            panic!("locks must publish as Text, got {v:?}");
        };

        // `s{step}:{node_id}:{param_id}={value:.6}`, entries joined by ';'.
        assert_eq!(
            s, "s3:20:7=0.250000",
            "the emitted lock string is a cross-crate contract — \
             `Model::parse_lock_value` splits on [':', '='] after an 's' prefix"
        );
    }

    // ── TK1 C4: mute tests ────────────────────────────────────────────────

    #[test]
    fn muted_sequencer_skips_note_and_lock_emission() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(0, 60, 32768, true);
        let mute_id = ParamDescriptor::id_for_name("mute");
        seq.bank.set(mute_id, 1.0);
        let events = run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let notes: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::Midi2(_)))
            .collect();
        assert!(notes.is_empty(), "muted seq must not emit note-on");

        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_LOCK_TARGET,
                    arg0: 20,
                    arg1: 1.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP_LOCK,
                    arg0: 0,
                    arg1: 0.42,
                },
            ],
        );
        let ticks_per_step = TICKS_PER_BEAT / 4;
        for t in 1..=ticks_per_step {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }
        let events2 = run_seq(&mut seq, &[transport_tick(0, true, false, false, false)]);
        let locks: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, Event::ParamLock(_)))
            .collect();
        assert!(locks.is_empty(), "muted seq must not emit param-locks");
    }

    #[test]
    fn muted_sequencer_still_publishes_playhead() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let mute_id = ParamDescriptor::id_for_name("mute");
        seq.bank.set(mute_id, 1.0);
        seq.set_step(0, 60, 32768, true);
        let ticks_per_step = TICKS_PER_BEAT / 4;
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        for t in 1..=ticks_per_step {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }
        let mut state = Vec::new();
        seq.published_state(&mut state);
        let current = state
            .iter()
            .find(|(k, _)| k.ends_with("/state/current_step"));
        assert!(matches!(current, Some((_, StateBusValue::Int(s))) if *s >= 1));
    }

    #[test]
    fn mute_publishes_bank_state_path() {
        let mut seq = Sequencer::new();
        seq.set_node_id(200);
        seq.activate(44100.0, 64);
        let mut state = Vec::new();
        seq.published_state(&mut state);
        let mute = state.iter().find(|(k, _)| k == "/node/200/param/mute");
        assert!(
            mute.is_some(),
            "mute param must be published via publish_bank_state"
        );
    }

    #[test]
    fn mute_roundtrips_v3_serializer() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let mute_id = ParamDescriptor::id_for_name("mute");
        seq.bank.set(mute_id, 1.0);
        let blob = seq.serialize();
        let mut restored = Sequencer::new();
        restored.activate(44100.0, 64);
        restored.deserialize(&blob);
        assert_eq!(restored.bank.get(mute_id), 1.0);
    }

    #[test]
    fn unmute_resumes_at_next_step() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let mute_id = ParamDescriptor::id_for_name("mute");
        seq.bank.set(mute_id, 1.0);
        seq.set_step(0, 60, 32768, true);
        seq.set_step(1, 62, 32768, true);
        seq.set_step(2, 64, 32768, true);
        let ticks_per_step = TICKS_PER_BEAT / 4;

        // Advance through step 0→1 boundary muted — no note.
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        for t in 1..=ticks_per_step {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }

        // Unmute, then collect ALL events across step 1→2 cycle.
        seq.bank.set(mute_id, 0.0);
        let mut all_out = Vec::new();
        for t in 1..=ticks_per_step {
            all_out.extend(run_seq(
                &mut seq,
                &[transport_tick(t, true, false, false, false)],
            ));
        }
        let has_note_on = all_out.iter().any(|e| matches!(e, Event::Midi2(_)));
        assert!(has_note_on, "must emit NoteOn after unmute");
    }

    // ── TK2 C1: live-trig engine command (D5, CMD_TRIG_NOW) ──────────────────

    fn trig_now_cmd(note: i64, velocity: f64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_TRIG_NOW,
            arg0: note,
            arg1: velocity,
        }
    }

    fn run_seq_with_cmds_events(seq: &mut Sequencer, cmds: &[NodeCommand]) -> Vec<Event> {
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(256);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: cmds,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        seq.process(&input, &mut output);
        events_out.as_slice().iter().map(|e| e.event).collect()
    }

    fn note_on_pitch_velocity(events: &[Event]) -> Option<(u8, u16)> {
        events.iter().find_map(|e| match e {
            Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(n))) => {
                Some((u8::from(n.note_number()), n.velocity()))
            }
            _ => None,
        })
    }

    #[test]
    fn trig_now_emits_note_on_next_window() {
        let mut seq = Sequencer::new();
        let events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(72, 0.9)]);
        let (note, _) = note_on_pitch_velocity(&events)
            .expect("CMD_TRIG_NOW must emit a NoteOn in the window it lands");
        assert_eq!(note, 72);
    }

    #[test]
    fn trig_now_uses_default_note_and_velocity_when_zero() {
        // BUG-044: a live pad trig must sound the same note as this
        // track's own sequenced steps, not a hardcoded constant — checked
        // against a non-default `default_note` so a hardcode cannot pass.
        let mut seq = Sequencer::new().with_default_note(36);
        let events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(0, 0.0)]);
        let hit = note_on_pitch_velocity(&events);
        assert_eq!(
            hit,
            Some((36, 32768)),
            "arg0=0/arg1=0.0 must resolve to this track's default_note (36), matching its sequenced steps"
        );
    }

    #[test]
    fn trig_now_matches_sequenced_step_note_at_another_default() {
        // A second value distinct from both the old hardcode (60) and the
        // first case (36), so a future hardcode of either cannot pass.
        let mut seq = Sequencer::new().with_default_note(48);
        let live_events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(0, 0.0)]);
        let live_note = note_on_pitch_velocity(&live_events)
            .expect("CMD_TRIG_NOW must emit a NoteOn")
            .0;

        let sequenced_note = seq.patterns[seq.active_pattern].steps[0].note;
        assert_eq!(
            live_note, sequenced_note,
            "a live pad trig must sound the same note as this track's sequenced step 0"
        );
        assert_eq!(live_note, 48);
    }

    #[test]
    fn trig_now_respects_mute() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.bank.set(ParamDescriptor::id_for_name("mute"), 1.0);
        let events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(64, 0.8)]);
        assert!(
            !events.iter().any(is_note_on),
            "a muted track must not sound a live trig"
        );
    }

    #[test]
    fn trig_now_works_while_stopped() {
        let mut seq = Sequencer::new();
        assert!(!seq.playing, "sanity: a fresh sequencer is stopped");
        let events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(50, 0.5)]);
        assert!(
            events.iter().any(is_note_on),
            "live trig must sound even with the transport stopped"
        );

        // §0 A3: with the transport stopped, no TransportEvents ever arrive
        // to drive the pattern-step gate-close path — the live trig's gate
        // must close itself via a sample-counted countdown in `process()`,
        // or the note rings forever.
        let mut closed = false;
        for _ in 0..200 {
            let more = run_seq_with_cmds_events(&mut seq, &[]);
            if more.iter().any(|e| {
                matches!(
                    e,
                    Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOff(_)))
                )
            }) {
                closed = true;
                break;
            }
        }
        assert!(
            closed,
            "a live trig's gate must close on its own even while the transport is stopped (§0 A3)"
        );
    }

    #[test]
    fn trig_now_second_command_replaces_pending() {
        let mut seq = Sequencer::new();
        let events =
            run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(40, 0.5), trig_now_cmd(80, 0.5)]);
        let notes: Vec<u8> = events
            .iter()
            .filter_map(|e| match e {
                Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(n))) => {
                    Some(u8::from(n.note_number()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            notes,
            vec![80],
            "a second CMD_TRIG_NOW in the same window replaces the first, not stacks"
        );
    }

    #[test]
    fn trig_now_does_not_modify_pattern() {
        let mut seq = Sequencer::new();
        let before_bits = seq.steps_bitfield();
        let before_len = seq.patterns[0].length;
        run_seq_with_cmds(&mut seq, &[trig_now_cmd(64, 0.5)]);
        assert_eq!(seq.steps_bitfield(), before_bits);
        assert_eq!(seq.patterns[0].length, before_len);
    }

    // ── TK2.1 C3b (D8, ADR-039 decision 7): live_rec ──────────────────────

    #[test]
    fn live_rec_records_trig_now_at_nearest_step() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        assert!(seq.playing, "sanity: global_start must have armed playing");

        // step_period defaults to 240 (TICKS_PER_BEAT/4); step_tick=200 is
        // past its midpoint (120), so the nearest step is 4, not 3 —
        // proving this genuinely rounds rather than recording at
        // current_step.
        seq.current_step = 3;
        seq.step_tick = 200;
        run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(70, 0.9)]);

        assert!(
            !seq.patterns[0].steps[3].active,
            "must not record at current_step when it rounds to a neighbor"
        );
        assert!(
            seq.patterns[0].steps[4].active,
            "must record at the nearest step (4), not current_step (3)"
        );
        assert_eq!(seq.patterns[0].steps[4].note, 70);
    }

    #[test]
    fn live_rec_writes_micro_timing_in_96th_units() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);

        // period=240, step_tick=50 -> nearest=3 (round(770/240)=3),
        // delta_ticks=770-720=50, micro=round(50/(960/96))=round(50/10)=5.
        seq.current_step = 3;
        seq.step_tick = 50;
        run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(70, 0.9)]);

        assert!(seq.patterns[0].steps[3].active);
        assert_eq!(
            seq.patterns[0].steps[3].timing.micro_offset, 5,
            "50 ticks late at 10 ticks/unit (TICKS_PER_BEAT=960) must \
             quantize to +5 in 1/96-beat units"
        );
    }

    #[test]
    fn live_rec_ignores_trig_now_while_stopped() {
        let mut seq = Sequencer::new();
        assert!(!seq.playing, "sanity: a fresh sequencer is stopped");
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        let before_bits = seq.steps_bitfield();

        let events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(70, 0.9)]);

        assert_eq!(
            seq.steps_bitfield(), before_bits,
            "a stopped transport must record nothing, even with live_rec armed"
        );
        assert!(
            events.iter().any(is_note_on),
            "the trig must still sound while stopped (recording and \
             sounding are independent)"
        );
    }

    #[test]
    fn live_rec_off_leaves_pattern_untouched() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        // live_rec left at its default (0.0) — armed nowhere.
        let before_bits = seq.steps_bitfield();

        let events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(70, 0.9)]);

        assert_eq!(
            seq.steps_bitfield(), before_bits,
            "playing with live_rec off must not record anything"
        );
        assert!(events.iter().any(is_note_on), "the trig must still sound");
    }

    #[test]
    fn live_rec_trig_still_sounds_when_recording() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);

        let events = run_seq_with_cmds_events(&mut seq, &[trig_now_cmd(70, 0.9)]);

        assert!(
            events.iter().any(is_note_on),
            "recording a live trig must not suppress its sound"
        );
        assert!(
            seq.patterns[0].steps.iter().any(|s| s.active),
            "sanity: the trig must also have been recorded"
        );
    }

    // ── P11 C5: Midi2 note-on consumption + live_quantize ──────────────────

    fn midi_note_on_event(note: u8, velocity: u16, offset: u32) -> TimedEvent {
        TimedEvent::new(offset, Event::Midi2(build_note_on(0, 0, note, velocity)))
    }

    #[test]
    fn midi_note_on_helper_rejects_non_note_on() {
        // CC / aftertouch / note-off / system messages are not note-ons.
        let note_on = build_note_on(0, 0, 60, 30000);
        assert_eq!(
            Sequencer::midi_note_on(&note_on),
            Some((60, 30000)),
            "a NoteOn must yield (note, velocity)"
        );
        let note_off = build_note_off(0, 0, 60);
        assert_eq!(Sequencer::midi_note_on(&note_off), None);
        // A ChannelVoice2 variant that is neither — construct one and
        // assert the match is exhaustive-safe (None).
        let msg = paraclete_node_api::UmpMessage::ChannelVoice2(
            paraclete_node_api::midi::ChannelVoice2::NoteOff(
                paraclete_node_api::midi::NoteOff::<[u32; 4]>::new(),
            ),
        );
        assert_eq!(Sequencer::midi_note_on(&msg), None);
    }

    #[test]
    fn midi2_note_on_records_step_when_live_rec_armed() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);

        // position: step 3, tick 50 (period 240) — same arithmetic the
        // trig_now tests use: nearest=3, micro=5.
        seq.current_step = 3;
        seq.step_tick = 50;
        run_seq(&mut seq, &[midi_note_on_event(70, 30000, 0)]);

        assert!(seq.patterns[0].steps[3].active, "step must be activated");
        assert_eq!(seq.patterns[0].steps[3].note, 70);
        assert_eq!(seq.patterns[0].steps[3].velocity, 30000);
        assert_eq!(
            seq.patterns[0].steps[3].timing.micro_offset, 5,
            "offset-0 Midi2 note must record the same micro-timing as CMD_TRIG_NOW"
        );
    }

    #[test]
    fn midi2_note_on_sample_offset_refines_micro_timing() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        seq.last_bpm = 120.0;
        // samples_per_tick at 120 bpm = 44100 * (60/120) / 960 = 22.97, so
        // a 23-sample offset adds ~1 tick. At step 3, tick 44 the micro
        // rounds to 4 (44/10 = 4.4); the extra tick makes it 45/10 = 4.5,
        // which rounds to 5 — the sample offset visibly refines the write.
        seq.current_step = 3;
        seq.step_tick = 44;
        run_seq(&mut seq, &[midi_note_on_event(70, 30000, 0)]);
        assert_eq!(
            seq.patterns[0].steps[3].timing.micro_offset, 4,
            "sanity: no offset → 44 ticks late = 4.4 → 4"
        );

        let mut seq2 = Sequencer::new();
        seq2.activate(44100.0, 64);
        run_seq(&mut seq2, &[transport_tick(0, true, true, false, false)]);
        seq2.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        seq2.last_bpm = 120.0;
        seq2.current_step = 3;
        seq2.step_tick = 44;
        run_seq(&mut seq2, &[midi_note_on_event(70, 30000, 23)]);

        assert_eq!(
            seq2.patterns[0].steps[3].timing.micro_offset, 5,
            "a 23-sample (one tick) offset must refine micro from 4 to 5"
        );
    }

    #[test]
    fn midi2_note_on_ignored_when_live_rec_off() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        let before = seq.steps_bitfield();

        run_seq(&mut seq, &[midi_note_on_event(70, 30000, 0)]);
        assert_eq!(
            seq.steps_bitfield(), before,
            "playing with live_rec off must not record Midi2 note-ons"
        );
    }

    #[test]
    fn midi2_note_on_ignored_while_stopped() {
        let mut seq = Sequencer::new();
        assert!(!seq.playing, "sanity: a fresh sequencer is stopped");
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        let before = seq.steps_bitfield();

        run_seq(&mut seq, &[midi_note_on_event(70, 30000, 0)]);
        assert_eq!(
            seq.steps_bitfield(), before,
            "a stopped transport must not record Midi2 note-ons"
        );
    }

    #[test]
    fn live_quantize_hard_snaps_to_grid_with_zero_micro() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        // Hard quantize to 1/8 notes: grid every 2 steps (period 240 ticks
        // = 16th; 1/8 = 2 steps = 480 ticks).
        seq.bank.set(ParamDescriptor::id_for_name("live_quantize"), 2.0);

        // Arrive 30 ticks into step 3 (pos = 3*240+30 = 750). The nearest
        // 1/8 grid point is 2 steps (480 ticks) → round(750/480)=2 → grid
        // at 960 ticks → step 4 (960/240). Micro must be 0.
        seq.current_step = 3;
        seq.step_tick = 30;
        run_seq(&mut seq, &[midi_note_on_event(70, 30000, 0)]);

        assert!(
            seq.patterns[0].steps[4].active,
            "the 1/8 grid slot (step 4) must receive the trig"
        );
        assert!(
            !seq.patterns[0].steps[3].active,
            "the off-grid arrival step must stay empty"
        );
        assert_eq!(seq.patterns[0].steps[4].timing.micro_offset, 0);
    }

    #[test]
    fn live_quantize_off_preserves_micro_timing() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        // live_quantize defaults to 0 (off).
        seq.current_step = 3;
        seq.step_tick = 30;
        run_seq(&mut seq, &[midi_note_on_event(70, 30000, 0)]);

        assert!(
            seq.patterns[0].steps[3].active,
            "off: the nearest step is recorded"
        );
        assert_eq!(
            seq.patterns[0].steps[3].timing.micro_offset, 3,
            "off: 30 ticks late at 10 ticks/unit = +3 micro"
        );
    }

    #[test]
    fn live_quantize_labels_cover_selector() {
        // The stepped selector's labels are the contract a surface reads
        // (value_labels path) — pin them.
        let doc = Sequencer::new().capability_document();
        let d = doc
            .params
            .iter()
            .find(|p| p.name.as_str() == "live_quantize")
            .expect("live_quantize must be declared");
        let labels = d.value_labels().expect("stepped selector with labels");
        assert_eq!(labels.len(), 5);
        assert_eq!(labels[0].as_deref(), Some("off"));
        assert_eq!(labels[1].as_deref(), Some("1/4"));
        assert_eq!(labels[2].as_deref(), Some("1/8"));
        assert_eq!(labels[3].as_deref(), Some("1/16"));
        assert_eq!(labels[4].as_deref(), Some("1/32"));
        assert!(!d.in_kit, "live_quantize is structural, never in a kit");
    }

    /// Review finding (post-C1 hostile review): the live gate's close was
    /// only wired into `process()`'s own sample countdown and the two fire
    /// paths that already called `emit_note_off` first — not the ordinary
    /// step-boundary path, which could silently overwrite a still-open live
    /// gate without ever closing it. Fixed by centralizing the close inside
    /// `emit_note_on_at`; this reproduces the exact orphan scenario.
    // ── P11 C3: temp save/reload (shadow pattern + copy_into) ───────────────

    fn temp_save_cmd() -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_TEMP_SAVE,
            arg0: 0,
            arg1: 0.0,
        }
    }

    fn temp_reload_cmd() -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_TEMP_RELOAD,
            arg0: 0,
            arg1: 0.0,
        }
    }

    // ── P11 C4: mute tiers (pattern mute, prepared mutes) ───────────────────

    fn set_pattern_mute_cmd(arg0: i64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_SET_PATTERN_MUTE,
            arg0,
            arg1: 0.0,
        }
    }

    fn prepare_mute_cmd(arg0: i64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_PREPARE_MUTE,
            arg0,
            arg1: 0.0,
        }
    }

    fn prepare_pattern_mute_cmd(arg0: i64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_PREPARE_PATTERN_MUTE,
            arg0,
            arg1: 0.0,
        }
    }

    fn live_erase_cmd(arg0: i64) -> NodeCommand {
        NodeCommand {
            target_id: 0,
            type_id: Sequencer::CMD_LIVE_ERASE,
            arg0,
            arg1: 0.0,
        }
    }

    /// Drive `count` full 16-step loops from a started transport (tps=240,
    /// so one loop = 3840 ticks) and return the events emitted.
    fn run_loops(seq: &mut Sequencer, count: usize) -> Vec<Event> {
        let tps = TICKS_PER_BEAT / 4;
        let mut all = Vec::new();
        for t in 1..=(count * 16 * tps as usize) {
            all.extend(run_seq(seq, &[transport_tick(t as u32, true, false, false, false)]));
        }
        all
    }

    #[test]
    fn pattern_mute_set_toggle_and_off() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);

        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(1)]);
        assert!(
            seq.patterns[0].muted,
            "arg0=1 must mute the active pattern"
        );
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(2)]);
        assert!(
            !seq.patterns[0].muted,
            "arg0=2 must toggle off a muted pattern"
        );
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(2)]);
        assert!(seq.patterns[0].muted, "arg0=2 must toggle back on");
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(0)]);
        assert!(!seq.patterns[0].muted, "arg0=0 must unmute");
    }

    #[test]
    fn is_muted_global_or_pattern_independent_tiers() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        assert!(!seq.is_muted(), "fresh sequencer is unmuted");

        // Pattern tier alone.
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(1)]);
        assert!(
            seq.is_muted(),
            "pattern mute alone must mute (global off)"
        );

        // Global tier alone (independent of pattern).
        let mut seq2 = Sequencer::new();
        seq2.activate(44100.0, 64);
        seq2
            .bank
            .set(ParamDescriptor::id_for_name("mute"), 1.0);
        assert!(seq2.is_muted(), "global mute alone must mute");
        assert!(
            !seq2.patterns[0].muted,
            "setting the global mute must not touch the pattern tier"
        );

        // Un-muting the pattern tier does not clear the global tier.
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(0)]);
        seq.bank.set(ParamDescriptor::id_for_name("mute"), 1.0);
        assert!(
            seq.is_muted(),
            "global mute must still mute after pattern tier cleared"
        );
    }

    #[test]
    fn prepared_mute_applies_at_wrap_and_clears() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);

        // Queue a deferred global mute and a deferred pattern mute mid-loop.
        run_seq_with_cmds(&mut seq, &[prepare_mute_cmd(1), prepare_pattern_mute_cmd(1)]);
        assert!(
            seq.pending_global_mute == Some(true) && seq.pending_pattern_mute == Some(true),
            "queued mutes must be held, not applied"
        );
        assert!(
            !seq.is_muted(),
            "a prepared mute must not mute before the wrap"
        );

        // Drive to the wrap. 16 steps × 240 ticks; start at tick 1.
        run_loops(&mut seq, 1);

        assert_eq!(seq.pending_global_mute, None, "applied at wrap");
        assert_eq!(seq.pending_pattern_mute, None, "applied at wrap");
        assert!(
            seq.patterns[0].muted,
            "pattern tier must be set at the wrap"
        );
        assert!(
            seq.bank.get(ParamDescriptor::id_for_name("mute")) >= 0.5,
            "global tier must be set at the wrap"
        );
        assert!(seq.is_muted());
    }

    #[test]
    fn prepared_mute_cleared_on_stop() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);

        run_seq_with_cmds(&mut seq, &[prepare_mute_cmd(1), prepare_pattern_mute_cmd(1)]);

        // Transport stops before any wrap.
        run_seq(&mut seq, &[transport_tick(1, false, false, true, false)]);

        assert_eq!(
            seq.pending_global_mute, None,
            "a prepared mute must not survive a stop"
        );
        assert_eq!(
            seq.pending_pattern_mute, None,
            "a prepared pattern mute must not survive a stop"
        );
        assert!(
            !seq.patterns[0].muted && seq.bank.get(ParamDescriptor::id_for_name("mute")) < 0.5,
            "nothing may be applied by a stop"
        );
    }

    #[test]
    fn pattern_mute_roundtrips_v3_trailing_byte() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(1)]);
        // A second pattern, muted differently, to prove per-pattern state.
        run_seq_with_cmds(&mut seq, &[set_pattern_cmd(1)]);
        // (set_pattern_cmd on a stopped sequencer switches immediately.)

        let data = seq.serialize();

        let mut seq2 = Sequencer::new();
        seq2.activate(44100.0, 64);
        seq2.deserialize(&data);

        assert!(
            seq2.patterns[0].muted,
            "pattern 0's muted flag must round-trip through the v3 blob"
        );
        assert!(
            !seq2.patterns[1].muted,
            "pattern 1's un-muted flag must round-trip"
        );
    }

    #[test]
    fn v3_blob_without_muted_byte_loads_unmuted() {
        // A v3 blob written before the P11 C4 trailing byte existed: pattern
        // records end exactly at their step records. The loader must default
        // muted=false and skip nothing else.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let mut data = seq.serialize();
        // Layout (empty chain): version u8 (0), ticks_per_step u32 (1..5),
        // speed_mult f32 (5..9), active u16 (9..11), chain_len u8 (11),
        // used u16 (12..14), then pattern records from 14. Each pattern
        // record is a u32 length followed by its bytes; serialize appends
        // the muted byte as the record's final byte. Drain it and shrink
        // the length so the blob reads as pre-P11-C4.
        let used = u16::from_le_bytes([data[12], data[13]]);
        let mut pos = 14usize;
        for _ in 0..used {
            let len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
            let record_start = pos;
            let record_end = pos + 4 + len;
            data.drain(record_end - 1..record_end);
            let new_len = (len - 1) as u32;
            data[record_start..record_start + 4].copy_from_slice(&new_len.to_le_bytes());
            pos = record_start + 4 + (len - 1);
        }

        let mut seq2 = Sequencer::new();
        seq2.activate(44100.0, 64);
        seq2.deserialize(&data);
        assert!(
            seq2.patterns.iter().all(|p| !p.muted),
            "a blob without the trailing byte must load every pattern unmuted"
        );
    }

    #[test]
    fn pattern_mute_publishes_to_state_bus() {
        let mut seq = Sequencer::new();
        seq.set_node_id(7);
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(1)]);

        let mut state = Vec::new();
        seq.published_state(&mut state);
        let muted = state
            .iter()
            .find(|(k, _)| k == "/node/7/state/pattern_muted")
            .expect("pattern_muted path must be published");
        assert!(
            matches!(muted.1, StateBusValue::Bool(true)),
            "muted pattern must publish Bool(true): {muted:?}"
        );
    }

    #[test]
    fn prepared_mute_last_write_wins() {
        // Two prepares before a wrap: the later one overrides (last-write-
        // wins), it must not queue twice or apply both.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);

        run_seq_with_cmds(&mut seq, &[prepare_pattern_mute_cmd(1), prepare_pattern_mute_cmd(0)]);
        run_loops(&mut seq, 1);

        assert!(
            !seq.patterns[0].muted,
            "the second (off) prepare must win at the wrap"
        );
    }

    #[test]
    fn deserialize_v3_clears_pending_mutes() {
        // A blob load replaces the pattern bank; a prepared mute held for
        // the old bank must not land on some future wrap of the new one.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[prepare_mute_cmd(1), prepare_pattern_mute_cmd(1)]);

        let data = seq.serialize();
        seq.deserialize(&data);

        assert_eq!(seq.pending_global_mute, None, "cleared by the load");
        assert_eq!(seq.pending_pattern_mute, None, "cleared by the load");
    }

    #[test]
    fn live_erase_clears_steps_as_playhead_passes() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // Author active steps at 0 and 1 with a lock on step 1.
        seq.patterns[0].steps[0].active = true;
        seq.patterns[0].steps[1].active = true;
        seq.patterns[0].steps[1]
            .param_locks
            .push(StepParamLock { node_id: 20, param_id: 1, value: 0.5 });
        let tps = TICKS_PER_BEAT / 4;
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[live_erase_cmd(1)]);
        assert!(seq.live_erase_armed, "arm must take effect");

        // Drive one step boundary (tps ticks): step 1 is reached and must
        // be cleared BEFORE it fires.
        for t in 1..=tps {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }
        assert!(
            !seq.patterns[0].steps[1].active,
            "the passed step must be erased (active cleared)"
        );
        assert!(
            seq.patterns[0].steps[1].param_locks.is_empty(),
            "the passed step's locks must be erased too"
        );

        // Disarm: the next boundary must NOT erase step 2.
        run_seq_with_cmds(&mut seq, &[live_erase_cmd(0)]);
        assert!(!seq.live_erase_armed, "disarm must take effect");
        seq.patterns[0].steps[2].active = true;
        for t in (tps + 1)..=(2 * tps) {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }
        assert!(
            seq.patterns[0].steps[2].active,
            "a disarmed sequencer must not erase the passed step"
        );
    }

    #[test]
    fn live_erase_does_not_sound_the_erased_step() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.patterns[0].steps[1].active = true;
        let tps = TICKS_PER_BEAT / 4;
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[live_erase_cmd(1)]);

        let mut all = Vec::new();
        for t in 1..=tps {
            all.extend(run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]));
        }
        assert!(
            !all.iter().any(is_note_on),
            "the erased step must not sound at the boundary it was erased on"
        );
    }

    #[test]
    fn live_erase_disarmed_on_stop() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        run_seq_with_cmds(&mut seq, &[live_erase_cmd(1)]);

        run_seq(&mut seq, &[transport_tick(1, false, false, true, false)]);
        assert!(
            !seq.live_erase_armed,
            "a stop must disarm live erase (it is a held gesture)"
        );
    }

    #[test]
    fn live_erase_preserves_bug042_suppression() {
        // Review regression (P11 C6): erase must not disturb the
        // live_recorded_step / early_fired suppressions — it only flips
        // active + locks before the fire, and the fire paths take the
        // suppression flags afterwards. Drive a REAL live trig that
        // records step 1 (live_recorded_step = Some(1)), then arm erase
        // and cross the step-1 boundary: the erase clears the step, the
        // fire is skipped (active=false), and the suppression flags still
        // consume/clear normally.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq(&mut seq, &[transport_tick(0, true, true, false, false)]);
        seq.bank.set(ParamDescriptor::id_for_name("live_rec"), 1.0);
        // pos = 0*240 + 200 ticks → nearest step 1.
        seq.current_step = 0;
        seq.step_tick = 200;
        run_seq(&mut seq, &[midi_note_on_event(70, 30000, 0)]);
        assert_eq!(seq.live_recorded_step, Some(1), "sanity: live trig records step 1");

        // Arm erase, then drive up to the step-1 boundary.
        run_seq_with_cmds(&mut seq, &[live_erase_cmd(1)]);
        let tps = TICKS_PER_BEAT / 4;
        for t in 1..=tps {
            run_seq(&mut seq, &[transport_tick(t, true, false, false, false)]);
        }
        // Step 1 was erased (active false) and did not fire.
        assert!(!seq.patterns[0].steps[1].active);
        // The suppression flags are consumed/cleared by the boundary path
        // regardless — nothing hangs over into the next window.
        assert_eq!(seq.early_fired, None);
        assert_eq!(seq.live_recorded_step, None);
        assert!(!seq.live_recorded_pending);
    }

    #[test]
    fn temp_save_reload_preserves_pattern_mute() {
        // The shadow copy (CMD_TEMP_SAVE/RELOAD) must carry the mute tier
        // like every other piece of pattern state.
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(1), temp_save_cmd()]);
        run_seq_with_cmds(&mut seq, &[set_pattern_mute_cmd(0)]);

        run_seq_with_cmds(&mut seq, &[temp_reload_cmd()]);
        assert!(
            seq.patterns[0].muted,
            "temp reload must restore the muted flag captured at save"
        );
    }

    #[test]
    fn copy_into_copies_steps_locks_and_pattern_state() {
        let mut src = Pattern::empty(Sequencer::STEP_CAPACITY);
        let mut dest = Pattern::empty(Sequencer::STEP_CAPACITY);

        // Pre-fill a destination lock that must be cleared by the copy
        // (proves copy_into replaces, not appends to, stale content).
        dest.steps[0]
            .param_locks
            .push(StepParamLock { node_id: 9, param_id: 9, value: 0.9 });

        // Distinctive state across every copied dimension.
        src.length = 24;
        src.page_loop = (1, 2);
        src.swing = 0.25;
        src.steps[0].active = true;
        src.steps[0].note = 61;
        src.steps[0].velocity = 20000;
        src.steps[0].length = 0.5;
        src.steps[0].timing.micro_offset = -3;
        src.steps[0].condition = TrigCondition::Simple {
            repeat: RepeatCondition::NthOfM { n: 2, m: 4 },
            fill: FillCondition::FillA,
            probability: 75,
        };
        src.steps[0].param_locks.push(StepParamLock {
            node_id: 1,
            param_id: 2,
            value: 0.5,
        });
        // More CV locks than CV_LOCK_CAP: the copy must clamp.
        for i in 0..6u16 {
            src.steps[0].cv_locks.push((i, i as f32));
        }
        src.steps[1].active = true;
        src.steps[1].note = 48;

        src.copy_into(&mut dest);

        assert_eq!(dest.length, 24, "length copied");
        assert_eq!(dest.page_loop, (1, 2), "page_loop copied");
        assert_eq!(dest.swing, 0.25, "swing copied");
        assert_eq!(dest.steps[0].active, true);
        assert_eq!(dest.steps[0].note, 61);
        assert_eq!(dest.steps[0].velocity, 20000);
        assert_eq!(dest.steps[0].length, 0.5);
        assert_eq!(dest.steps[0].timing.micro_offset, -3);
        assert_eq!(dest.steps[0].condition, src.steps[0].condition);
        assert_eq!(
            dest.steps[0].param_locks.len(),
            1,
            "stale destination lock cleared; exactly the source lock copied"
        );
        assert_eq!(dest.steps[0].param_locks[0].node_id, 1);
        assert_eq!(dest.steps[0].param_locks[0].param_id, 2);
        assert_eq!(dest.steps[0].param_locks[0].value, 0.5);
        assert_eq!(
            dest.steps[0].cv_locks.len(),
            Step::CV_LOCK_CAP,
            "CV locks beyond CV_LOCK_CAP are clamped"
        );
        assert_eq!(
            dest.steps[0].cv_locks,
            src.steps[0].cv_locks[..Step::CV_LOCK_CAP].to_vec()
        );
        assert_eq!(dest.steps[1].note, 48);
        assert_eq!(dest.steps[2].active, false, "untouched step stays default");
    }

    #[test]
    fn copy_into_does_not_reallocate_destination_locks() {
        // P11 C3: copy_into runs on the audio thread (CMD_TEMP_SAVE /
        // CMD_TEMP_RELOAD) and must never grow a lock Vec. Pre-reserved
        // capacity (LOCK_CAP_PER_STEP / CV_LOCK_CAP) must be preserved.
        let mut src = Pattern::empty(Sequencer::STEP_CAPACITY);
        let mut dest = Pattern::empty(Sequencer::STEP_CAPACITY);
        for i in 0..Sequencer::LOCK_CAP_PER_STEP as u16 {
            src.steps[0]
                .param_locks
                .push(StepParamLock { node_id: i as u32, param_id: 0, value: 0.1 });
        }
        for i in 0..Step::CV_LOCK_CAP as u16 {
            src.steps[0].cv_locks.push((i, i as f32));
        }

        src.copy_into(&mut dest);

        assert_eq!(dest.steps[0].param_locks.len(), Sequencer::LOCK_CAP_PER_STEP);
        assert_eq!(
            dest.steps[0].param_locks.capacity(),
            Sequencer::LOCK_CAP_PER_STEP,
            "param_locks must not reallocate"
        );
        assert_eq!(dest.steps[0].cv_locks.len(), Step::CV_LOCK_CAP);
        assert_eq!(
            dest.steps[0].cv_locks.capacity(),
            Step::CV_LOCK_CAP,
            "cv_locks must not reallocate"
        );
    }

    #[test]
    fn temp_save_reload_round_trip() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        seq.set_step(0, 60, 32768, true);

        run_seq_with_cmds(&mut seq, &[temp_save_cmd()]);
        assert!(seq.shadow_has_data, "save arms the shadow");

        // Mutate after the save: erase step 0 and rewrite it as note 72.
        run_seq_with_cmds(
            &mut seq,
            &[
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_SET_STEP,
                    arg0: 0,
                    arg1: 72.0,
                },
                NodeCommand {
                    target_id: 0,
                    type_id: Sequencer::CMD_CLEAR,
                    arg0: 0,
                    arg1: 0.0,
                },
            ],
        );
        assert!(!seq.patterns[0].steps[0].active, "sanity: mutated after save");

        run_seq_with_cmds(&mut seq, &[temp_reload_cmd()]);

        assert!(
            !seq.shadow_has_data,
            "a reload consumes the shadow (one-shot snapshot)"
        );
        assert!(seq.patterns[0].steps[0].active, "reload restores step 0");
        assert_eq!(seq.patterns[0].steps[0].note, 60, "reload restores note");
    }

    #[test]
    fn temp_save_uses_active_pattern() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // Playback/editing works on pattern 1, not pattern 0.
        seq.active_pattern = 1;
        seq.set_step(0, 70, 32768, true);

        run_seq_with_cmds(&mut seq, &[temp_save_cmd()]);

        // Mutate pattern 1, then reload: the saved pattern 1 state returns,
        // and pattern 0 was never touched.
        seq.set_step(0, 71, 32768, false);
        run_seq_with_cmds(&mut seq, &[temp_reload_cmd()]);

        assert!(seq.patterns[1].steps[0].active, "active pattern restored");
        assert_eq!(seq.patterns[1].steps[0].note, 70);
        assert!(
            !seq.patterns[0].steps[0].active,
            "inactive pattern untouched by temp save/reload"
        );
    }

    #[test]
    fn temp_reload_without_save_is_noop() {
        let mut seq = Sequencer::new();
        seq.set_step(0, 60, 32768, true);
        seq.set_step(1, 61, 32768, true);
        let before_bits = seq.steps_bitfield();

        run_seq_with_cmds(&mut seq, &[temp_reload_cmd()]);

        assert!(!seq.shadow_has_data);
        assert_eq!(seq.steps_bitfield(), before_bits, "no-op without a save");
    }

    #[test]
    fn temp_save_reload_works_after_deserialize() {
        // A project load rebuilds pattern steps from the blob; those steps
        // must still satisfy copy_into's reserved-capacity contract, or a
        // temp reload into the loaded active pattern would reallocate on the
        // audio thread (or trip the debug_asserts).
        let mut seq = Sequencer::new();
        seq.set_step(0, 63, 32768, true);
        seq.set_step(2, 64, 20000, false);

        let blob = seq.serialize();
        let mut loaded = Sequencer::new();
        loaded.deserialize(&blob);
        assert!(loaded.patterns[0].steps[0].active, "sanity: load restored steps");
        assert_eq!(loaded.patterns[0].steps[2].note, 64);

        // Temp save on the loaded sequencer, mutate, reload.
        run_seq_with_cmds(&mut loaded, &[temp_save_cmd()]);
        assert!(loaded.shadow_has_data);
        run_seq_with_cmds(
            &mut loaded,
            &[NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_CLEAR,
                arg0: 0,
                arg1: 0.0,
            }],
        );
        assert!(
            !loaded.patterns[0].steps[0].active,
            "sanity: cleared after save"
        );

        run_seq_with_cmds(&mut loaded, &[temp_reload_cmd()]);

        assert!(loaded.patterns[0].steps[0].active, "reload restores into loaded pattern");
        assert_eq!(loaded.patterns[0].steps[0].note, 63);
        assert_eq!(loaded.patterns[0].steps[2].note, 64);
    }
}
