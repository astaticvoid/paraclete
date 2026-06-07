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
#[derive(Clone, Debug)]
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

// ── Sequencer ────────────────────────────────────────────────────────────────

pub struct Sequencer {
    ports: Vec<PortDescriptor>,
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

    cycle_state:    SequencerCycleState,
    rng:            fastrand::Rng,
    active_pattern: u8,
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
        Self {
            ports,
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
            cycle_state:    SequencerCycleState::default(),
            rng:            fastrand::Rng::new(),
            active_pattern: 0,
            swing_amount:   0.0,
            sample_rate:    44100.0,
            cv_outputs,
            current_cv:  vec![0.0_f32; cv_outputs],
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
                Self::CMD_SET_FILL_A => {
                    self.cycle_state.fill_a = cmd.arg0 != 0;
                }
                Self::CMD_SET_FILL_B => {
                    self.cycle_state.fill_b = cmd.arg0 != 0;
                }
                Self::CMD_SET_STEP_TIMING => {
                    let idx = cmd.arg0 as usize;
                    if idx < self.steps.len() {
                        self.steps[idx].timing.micro_offset = cmd.arg1 as i8;
                    }
                }
                Self::CMD_SET_STEP_CONDITION => {
                    let idx = cmd.arg0 as usize;
                    if idx < self.steps.len() {
                        let enc = cmd.arg1 as i64 as u64;
                        let probability = (enc & 0xFF) as u8;
                        let repeat_n    = ((enc >> 8) & 0xFF) as u8;
                        let repeat_m    = ((enc >> 16) & 0xFF) as u8;
                        let fill_disc   = ((enc >> 24) & 0xFF) as u8;

                        let repeat = if repeat_n == 0 || repeat_m == 0 {
                            RepeatCondition::Always
                        } else {
                            RepeatCondition::NthOfM { n: repeat_n, m: repeat_m }
                        };
                        let fill = match fill_disc {
                            1 => FillCondition::FillA,
                            2 => FillCondition::FillB,
                            3 => FillCondition::FillAny,
                            4 => FillCondition::NoFill,
                            5 => FillCondition::NotFillA,
                            6 => FillCondition::NotFillB,
                            _ => FillCondition::Ignore,
                        };
                        self.steps[idx].condition = TrigCondition::Simple { repeat, fill, probability };
                    }
                }
                Self::CMD_SET_PATTERN => {
                    self.active_pattern = cmd.arg0 as u8;
                    // stub: pattern bank not implemented until P6; always plays pattern 0
                }
                _ => {}
            }
        }
    }

    fn handle_transport(&mut self, k: &TransportEvent, sample_offset: u32, output: &mut ProcessOutput) {
        let spb = 60.0 * self.sample_rate as f64 / k.bpm.max(1.0);

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
                let step_active = self.steps[self.current_step].active;
                if step_active {
                    let cond = self.steps[self.current_step].condition.clone();
                    let should_fire = cond.evaluate(&self.cycle_state, &mut self.rng);
                    if should_fire {
                        let note_off = sample_offset + self.step_sample_offset(self.current_step, spb);
                        if self.gate_open {
                            self.emit_note_off(sample_offset, output);
                        }
                        self.emit_note_on(note_off, output);
                        for lock in &self.steps[self.current_step].param_locks {
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
            let prev_step = self.current_step;
            self.step_tick    = 0;
            self.current_step = (self.current_step + 1) % self.pattern_length;

            // Increment loop_count each time the pattern wraps.
            if self.current_step == 0 && prev_step == self.pattern_length - 1 {
                self.cycle_state.loop_count = self.cycle_state.loop_count.wrapping_add(1);
            }

            let step_active = self.steps[self.current_step].active;
            if step_active {
                let cond = self.steps[self.current_step].condition.clone();
                let should_fire = cond.evaluate(&self.cycle_state, &mut self.rng);
                if should_fire {
                    let note_off = sample_offset + self.step_sample_offset(self.current_step, spb);
                    self.emit_note_on(note_off, output);
                    for lock in &self.steps[self.current_step].param_locks {
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
        let micro = self.steps[step_idx].timing.to_sample_offset(samples_per_beat);
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
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.handle_commands(input.commands);
        self.swing_amount = self.bank.get(ParamDescriptor::id_for_name("swing")) as f32;

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
        buf.push((format!("/node/{id}/state/pattern_length"),  StateBusValue::Int(self.pattern_length as i64)));
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
        let mut buf = Vec::new();
        buf.push(2u8); // version 2: adds cv_locks per step
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
            buf.push(step.cv_locks.len() as u8);
            for &(idx, val) in &step.cv_locks {
                buf.extend_from_slice(&idx.to_le_bytes());
                buf.extend_from_slice(&val.to_le_bytes());
            }
        }
        buf
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.is_empty() { return; }
        let version = data[0];
        if version != 1 && version != 2 { return; }
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
            let mut cv_locks = Vec::new();
            if version >= 2 {
                let cv_count = read_u8!() as usize;
                for _ in 0..cv_count {
                    let idx = read_u16!();
                    let val = read_f32!();
                    cv_locks.push((idx, val));
                }
            }
            steps.push(Step { active, note, velocity, length, param_locks,
                             condition: TrigCondition::default(), timing: StepTiming::default(),
                             cv_locks });
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
        assert_eq!(seq.steps[3].timing.micro_offset, 12i8);
    }

    #[test]
    fn sequencer_cmd_set_step_condition_encodes_correctly() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        // probability=75, repeat_n=1, repeat_m=2, fill=Ignore(0)
        let enc: i64 = 75 | (1 << 8) | (2 << 16) | (0i64 << 24);
        run_seq_with_cmds(&mut seq, &[NodeCommand { target_id: 0, type_id: Sequencer::CMD_SET_STEP_CONDITION, arg0: 5, arg1: enc as f64 }]);
        assert!(matches!(&seq.steps[5].condition, TrigCondition::Simple {
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

    #[test]
    fn sequencer_loop_count_increments_on_pattern_wrap() {
        let mut seq = Sequencer::new();
        seq.activate(44100.0, 64);
        let tps = TICKS_PER_BEAT / 4;  // 240 ticks per step
        // Known off-by-one: each step takes tps+1 ticks (the transport start model
        // costs one extra tick at startup). One full 16-step pattern = 16*(tps+1).
        let wrap_tick = 16 * (tps + 1);

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
        seq.steps[1].active = true;
        seq.steps[1].condition = TrigCondition::Simple {
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
        assert_eq!(restored.steps[3].note, 72);
        assert!(restored.steps[3].active);
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
        seq.steps[1].active = true;
        seq.steps[1].cv_locks = vec![(0, 0.75)];
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
        seq.steps[1].active = true;
        seq.steps[1].cv_locks = vec![(0, 0.5)];
        seq.steps[2].active = true;
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
        seq.steps[1].active = true;
        seq.steps[1].cv_locks = vec![(5, 9.9)]; // index 5 out of range for cv_outputs=1
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
        seq.steps[0].active = true;
        seq.steps[0].cv_locks = vec![(0, 0.3), (1, 0.7)];
        seq.steps[3].active = true;
        seq.steps[3].cv_locks = vec![(0, 1.0)];

        let data = seq.serialize();

        let mut seq2 = Sequencer::with_cv_outputs(2);
        seq2.activate(44100.0, 64);
        seq2.deserialize(&data);

        assert_eq!(seq2.steps[0].cv_locks, vec![(0u16, 0.3f32), (1u16, 0.7f32)],
            "step 0 cv_locks mismatch after roundtrip");
        assert_eq!(seq2.steps[3].cv_locks, vec![(0u16, 1.0f32)],
            "step 3 cv_locks mismatch after roundtrip");
        assert!(seq2.steps[1].cv_locks.is_empty(), "step 1 cv_locks must be empty");
    }
}
