use std::collections::HashMap;
use std::borrow::Cow;

use paraclete_node_api::{
    CapabilityDocument, Event, Node, PageRef, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, Rule, StateBusValue,
    UmpMessage, ViewPlugin, midi::ChannelVoice2,
};

use crate::engine_dsp::{harmonic_sine, note_to_hz, sub_blocks, AdsrState, FmOperator};

fn fp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

// ── Algo (P14 §0 D1) ──────────────────────────────────────────────────────────

/// The eight algorithms, in the frozen order from `p14-fm-voice.md` §0 D1.
/// **Append-only** — `algo`'s stored value is this order's index, same
/// contract as `FmMachine::ALL`/`LfoShape::ALL`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Algo {
    Stack, ShortStack, YSplit, Branch,
    ParallelCarriers, FeedbackPair, AdditiveTwin, Bell,
}

impl Algo {
    pub const ALL: [Algo; 8] = [
        Algo::Stack, Algo::ShortStack, Algo::YSplit, Algo::Branch,
        Algo::ParallelCarriers, Algo::FeedbackPair, Algo::AdditiveTwin, Algo::Bell,
    ];

    pub fn value(self) -> u32 { Self::ALL.iter().position(|&a| a == self).unwrap() as u32 }

    /// Out-of-range clamps to `Stack` rather than panicking — same convention
    /// as every other stepped selector in this crate (a malformed project
    /// can carry an out-of-range `f64` bank slot).
    pub fn from_value(v: u32) -> Self { *Self::ALL.get(v as usize).unwrap_or(&Algo::Stack) }

    pub fn name(self) -> &'static str {
        match self {
            Algo::Stack => "Stack",
            Algo::ShortStack => "Short Stack",
            Algo::YSplit => "Y-Split",
            Algo::Branch => "Branch",
            Algo::ParallelCarriers => "Parallel Carriers",
            Algo::FeedbackPair => "Feedback Pair",
            Algo::AdditiveTwin => "Additive Twin",
            Algo::Bell => "Bell",
        }
    }
}

/// §0 D2's stepped ratio set. `ratio_c`/`ratio_a`/`ratio_b` store an index
/// into this, not the ratio value itself — the same "stepped selector, value
/// is the index" convention as `algo`/`machine`/`lfo_shape`.
const RATIO_TABLE: &[f32] = &[
    0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0,
    5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0,
];

/// Index of ratio 1.0 in `RATIO_TABLE` — the neutral default for all three
/// ratio params (C:A:B = 1:1:1, a simple in-tune stack; the six macro
/// machines override this per D5's recipes in P14-C7).
const RATIO_DEFAULT_INDEX: f64 = 3.0;

/// §0 D2b: B2's ratio is B1's (`ratio_b`) plus this fixed spread — additive,
/// not user-editable. Small enough to read as beating/chorus at low ratios
/// and a shimmer at high ones, never a separate harmonic partial.
const B_PAIR_SPREAD: f32 = 0.05;

fn ratio_from_index(idx: f32) -> f32 {
    let i = idx.round().clamp(0.0, (RATIO_TABLE.len() - 1) as f32) as usize;
    RATIO_TABLE[i]
}

// ── Machine (P14 §0 D5) ───────────────────────────────────────────────────────

/// Stubbed now so P14-C7 (the six macros) does not need to restructure this
/// commit — only `Full` has behavior; the rest are declared but unreachable
/// until C7 builds their `MachineVariant`s. No bank param yet (deliberately —
/// that plumbing is ADR-041's `MachineVariant`/`Rule::variants` machinery,
/// C7's job, not invented early).
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Machine { Full, Keys, Bass, Bell, Perc, Pad, Pluck }

/// P14 §2's C2 bullet: algo switch is its own audible discontinuity even
/// within one machine, so it gets the same 5 ms fade-to-silence-then-swap
/// FmEngine/AnalogEngine use for machine switching (ADR-041 decision 4).
const ALGO_SWITCH_FADE_SECS: f32 = 0.005;

#[derive(Clone, Copy, Debug)]
struct AlgoSwitchFade {
    remaining: u32,
    /// `Some` = fading out toward this algo; `None` = a cancelled switch
    /// ramping back to unity, which swaps nothing.
    target: Option<Algo>,
}

// ── FmVoice ───────────────────────────────────────────────────────────────────

/// Four-operator phase-modulation melodic voice (ADR-043, P14). Topology:
/// carrier **C**, modulator **A**, linked pair **B1**+**B2** (one `ratio_b`
/// control, B2 = B1 + `B_PAIR_SPREAD`) — eight curated algorithms (§0 D1)
/// define how they connect. Monophonic per track in v1 (ADR-043 decision 4).
///
/// Ports: events_in (0, Event), audio_out_l (1, Mono), audio_out_r (2, Mono).
///
/// P14-C2 only: SYN1 + algorithm routing, raw (unenveloped) operator output.
/// No AMP/SYN2/MOD page params exist yet (C3/C4/C6) and `NoteOff` is not yet
/// consumed (C4) — a triggered voice stays `active` until the next trigger,
/// same as any C2-stage engine before its amp envelope lands.
pub struct FmVoice {
    machine: Machine,
    algo: Algo,
    bank: ParameterBank,
    sample_rate: f32,

    op_c: FmOperator,
    op_a: FmOperator,
    op_b1: FmOperator,
    op_b2: FmOperator,

    /// §0 D3/ADR-043 decision 3's two modulation-index envelopes — shape how
    /// much of A's/the B-pair's output reaches whatever it modulates over
    /// the note, not gated amplitude (that's C4's AMP-page `AdsrState`).
    /// Not reset on an algo switch (`apply_algo_switch`) — brightness "over
    /// the note" is a property of the *note*, not the topology currently
    /// shaping it.
    env_a: AdsrState,
    env_b: AdsrState,

    current_note: u8,
    active: bool,
    node_id: u32,
    last_note: u8,
    velocity_level: f32,
    node_locks: Vec<(u32, f64)>,
    locks_pending: bool,

    /// In-flight gain ramp for an algo switch (§2 C2).
    switch_fade: Option<AlgoSwitchFade>,

    render_l: Vec<f32>,
    render_r: Vec<f32>,

    pending_initial_params: HashMap<String, f64>,

    ports: [PortDescriptor; 3],
}

impl FmVoice {
    pub const PORT_EVENTS_IN: u32 = 0;
    pub const PORT_AUDIO_OUT_L: u32 = 1;
    pub const PORT_AUDIO_OUT_R: u32 = 2;

    pub fn new() -> Self {
        let doc = Self::build_doc();
        Self {
            machine: Machine::Full,
            algo: Algo::Stack,
            bank: ParameterBank::from_capability_document(&doc),
            sample_rate: 44100.0,
            op_c: FmOperator::new(),
            op_a: FmOperator::new(),
            op_b1: FmOperator::new(),
            op_b2: FmOperator::new(),
            env_a: AdsrState::new(),
            env_b: AdsrState::new(),
            current_note: 60,
            active: false,
            node_id: 0,
            last_note: 60,
            velocity_level: 1.0,
            node_locks: Vec::new(),
            locks_pending: false,
            switch_fade: None,
            render_l: Vec::new(),
            render_r: Vec::new(),
            pending_initial_params: HashMap::new(),
            ports: Self::default_ports(),
        }
    }

    /// §0 D1's eight SYN1 params, exactly — no coarse `tune` (pitch comes
    /// from `note_to_hz(note)` × each operator's own ratio; `detune` is the
    /// only pitch fine-tune, matching ADR-043 decision 2's named set).
    fn params() -> Vec<ParamDescriptor> {
        vec![
            ParamDescriptor {
                id: fp("algo"), name: "algo".into(),
                min: 0.0, max: (Algo::ALL.len() - 1) as f64, default: 0.0,
                stepped: true, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("ratio_c"), name: "ratio_c".into(),
                min: 0.0, max: (RATIO_TABLE.len() - 1) as f64, default: RATIO_DEFAULT_INDEX,
                stepped: true, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("ratio_a"), name: "ratio_a".into(),
                min: 0.0, max: (RATIO_TABLE.len() - 1) as f64, default: RATIO_DEFAULT_INDEX,
                stepped: true, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("ratio_b"), name: "ratio_b".into(),
                min: 0.0, max: (RATIO_TABLE.len() - 1) as f64, default: RATIO_DEFAULT_INDEX,
                stepped: true, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("harm"), name: "harm".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            // +/-50 cents: enough for a chorus-y fine detune without
            // reaching into semitone (coarse-tune) territory — there is no
            // coarse tune param on SYN1 by design (see the doc comment above).
            ParamDescriptor {
                id: fp("detune"), name: "detune".into(),
                min: -50.0, max: 50.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Cents, display: None,
            },
            ParamDescriptor {
                id: fp("feedback"), name: "feedback".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            // 0.0 = X tap only (the algorithm's fuller/deeper tone) — see §0
            // D1's table; each algorithm's Y tap is the simpler alternative.
            ParamDescriptor {
                id: fp("mix"), name: "mix".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
        ]
    }

    /// §0 D3/ADR-043 decision 3's SYN2: two modulation-index envelope
    /// groups, A and B — "end level" not "sustain" (`AdsrState`'s `sustain`
    /// arg *is* `end_a`/`end_b`; there is no release stage here, C4's AMP
    /// page owns `NoteOff`). Ranges/units mirror `FmEngine`'s existing
    /// attack/decay convention (`ParamUnit::Seconds`, same order of
    /// magnitude) rather than inventing a new one.
    ///
    /// `end_a`/`end_b` default to 0.0 — a classic FM shape, bright at the
    /// attack and decaying to a plainer sustained tone, not a knob a
    /// performer has to reach for to hear the envelope do anything.
    /// `lev_a`/`lev_b` default to 1.0 (full depth) rather than 0.0 — a
    /// fresh voice should sound characterful out of the box, not silent-of-
    /// modulation until a performer discovers the depth knobs.
    fn syn2_params() -> Vec<ParamDescriptor> {
        vec![
            ParamDescriptor {
                id: fp("atk_a"), name: "atk_a".into(),
                min: 0.001, max: 0.5, default: 0.01,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
            },
            ParamDescriptor {
                id: fp("dec_a"), name: "dec_a".into(),
                min: 0.05, max: 4.0, default: 0.3,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
            },
            ParamDescriptor {
                id: fp("end_a"), name: "end_a".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("lev_a"), name: "lev_a".into(),
                min: 0.0, max: 1.0, default: 1.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("atk_b"), name: "atk_b".into(),
                min: 0.001, max: 0.5, default: 0.01,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
            },
            ParamDescriptor {
                id: fp("dec_b"), name: "dec_b".into(),
                min: 0.05, max: 4.0, default: 0.3,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
            },
            ParamDescriptor {
                id: fp("end_b"), name: "end_b".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("lev_b"), name: "lev_b".into(),
                min: 0.0, max: 1.0, default: 1.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
        ]
    }

    fn default_ports() -> [PortDescriptor; 3] {
        [
            PortDescriptor { id: Self::PORT_EVENTS_IN, name: "events_in".into(), direction: PortDirection::Input, port_type: PortType::Event },
            PortDescriptor { id: Self::PORT_AUDIO_OUT_L, name: "audio_out_l".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            PortDescriptor { id: Self::PORT_AUDIO_OUT_R, name: "audio_out_r".into(), direction: PortDirection::Output, port_type: PortType::Audio },
        ]
    }

    fn build_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "FmVoice".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: Self::default_ports().to_vec(),
            params: Self::params().into_iter().chain(Self::syn2_params()).collect(),
            extensions: vec!["paraclete.instrument".into()],
            view: None,
        }
    }

    /// See `FmEngine::push_lock`/`consume_pending_locks` (#169) — same
    /// per-note ParamLock contract, unaffected by this engine having no AMP
    /// page yet.
    fn push_lock(&mut self, param_id: u32, value: f64) {
        if !self.locks_pending {
            self.node_locks.clear();
            self.locks_pending = true;
        }
        self.node_locks.push((param_id, value));
    }

    fn consume_pending_locks(&mut self) {
        if self.locks_pending {
            self.locks_pending = false;
        } else {
            self.node_locks.clear();
        }
    }

    fn raw_param(&self, param_id: u32) -> f32 {
        for &(id, val) in &self.node_locks {
            if id == param_id { return val as f32; }
        }
        self.bank.get(param_id) as f32
    }

    /// Has `algo` moved? Called at the block boundary — see
    /// `FmEngine::poll_machine_param` for the identical fade-then-swap shape,
    /// adapted here to `algo` instead of `machine`.
    fn poll_algo_param(&mut self) {
        let target = Algo::from_value(self.raw_param(fp("algo")) as u32);
        let total = self.fade_len();

        if target == self.algo {
            if let Some(f) = self.switch_fade {
                if f.target.is_some() {
                    self.switch_fade = Some(AlgoSwitchFade {
                        remaining: total.saturating_sub(f.remaining).max(1),
                        target: None,
                    });
                }
            }
            return;
        }

        if self.switch_fade.map(|f| f.target) == Some(Some(target)) {
            return;
        }
        if !self.active {
            self.apply_algo_switch(target);
            self.switch_fade = None;
            return;
        }

        let remaining = match self.switch_fade {
            Some(f) if f.target.is_some() => f.remaining.min(total),
            Some(f) => total.saturating_sub(f.remaining).max(1),
            None => total,
        };
        self.switch_fade = Some(AlgoSwitchFade { remaining, target: Some(target) });
    }

    fn fade_len(&self) -> u32 {
        (ALGO_SWITCH_FADE_SECS * self.sample_rate).max(1.0) as u32
    }

    fn apply_switch_fade(&mut self, block_size: usize) {
        let Some(fade) = self.switch_fade else {
            return;
        };
        let total = self.fade_len() as f32;
        let fading_out = fade.target.is_some();
        let mut left = fade.remaining;

        for i in 0..block_size.min(self.render_l.len()) {
            if left == 0 {
                if fading_out {
                    self.render_l[i] = 0.0;
                    self.render_r[i] = 0.0;
                }
                continue;
            }
            let g = if fading_out {
                left as f32 / total
            } else {
                1.0 - (left as f32 / total)
            };
            self.render_l[i] *= g;
            self.render_r[i] *= g;
            left -= 1;
        }

        if left == 0 {
            if let Some(target) = fade.target {
                self.apply_algo_switch(target);
            }
            self.switch_fade = None;
        } else {
            self.switch_fade = Some(AlgoSwitchFade { remaining: left, target: fade.target });
        }
    }

    /// A new topology means the old phase/feedback state means nothing under
    /// it — reset every operator, matching `FmEngine::apply_machine_switch`'s
    /// declick technique. **Deliberately does not** clear `self.active` the
    /// way `apply_machine_switch` does, though: a machine swap on a drum
    /// engine ends the hit it was mid-playing, but an algo swap here is a
    /// timbral change under a still-held melodic note — the voice should
    /// keep sounding under the new algorithm, not require a fresh trigger.
    fn apply_algo_switch(&mut self, target: Algo) {
        self.algo = target;
        self.op_c.reset();
        self.op_a.reset();
        self.op_b1.reset();
        self.op_b2.reset();
    }

    fn retrigger(&mut self, note: u8, velocity: f32) {
        self.consume_pending_locks();
        self.current_note = note;
        self.last_note = note;
        self.velocity_level = velocity.clamp(0.0, 1.0);
        self.op_c.reset();
        self.op_a.reset();
        self.op_b1.reset();
        self.op_b2.reset();
        // `true` (retrig): C4 will decide whether a legato note-stack pop
        // should pass `false` instead — this commit only ever retriggers.
        self.env_a.trigger(true);
        self.env_b.trigger(true);
        self.active = true;
    }

    /// Render `[start, end)` with the current voice state. Chunked into
    /// `LFO_SUB_BLOCK` pieces relative to `start` (MM-C7 pattern) even though
    /// nothing modulates yet in C2 — C6's MOD page lands on this same loop.
    fn render_span(&mut self, start: usize, end: usize) {
        if start >= end || !self.active {
            return;
        }
        for (lo, hi) in sub_blocks(start, end) {
            self.process_algo(lo, hi);
        }
        let v = self.velocity_level;
        if v != 1.0 {
            for s in &mut self.render_l[start..end] { *s *= v; }
            for s in &mut self.render_r[start..end] { *s *= v; }
        }
    }

    fn process_algo(&mut self, start: usize, end: usize) {
        let harm = self.raw_param(fp("harm"));
        let feedback = self.raw_param(fp("feedback"));
        let mix = self.raw_param(fp("mix"));
        let detune_cents = self.raw_param(fp("detune"));
        let ratio_c = ratio_from_index(self.raw_param(fp("ratio_c")));
        let ratio_a = ratio_from_index(self.raw_param(fp("ratio_a")));
        let ratio_b1 = ratio_from_index(self.raw_param(fp("ratio_b")));
        let ratio_b2 = ratio_b1 + B_PAIR_SPREAD;

        // §0 D3/ADR-043 decision 3's two mod-index envelopes. Coefficients
        // derived the same way `FmEngine`'s `AdState`-driven envelopes are
        // (attack: linear ramp reaching 1.0 in `atk_*` seconds; decay: an
        // exponential coefficient reaching ~0.1% of the gap to `end_*` in
        // `dec_*` seconds) — see e.g. `fm_engine.rs`'s `pitch_decay_coeff`.
        // `release_coeff` is an unused placeholder: `note_off()` is never
        // called on these envelopes until C4 wires `NoteOff`, so `Release`
        // is unreachable here.
        let sr = self.sample_rate;
        let atk_a_inc = 1.0 / (self.raw_param(fp("atk_a")) * sr).max(1.0);
        let dec_a_coeff = 0.001f32.powf(1.0 / (self.raw_param(fp("dec_a")) * sr).max(1.0));
        let end_a = self.raw_param(fp("end_a"));
        let lev_a = self.raw_param(fp("lev_a"));
        let atk_b_inc = 1.0 / (self.raw_param(fp("atk_b")) * sr).max(1.0);
        let dec_b_coeff = 0.001f32.powf(1.0 / (self.raw_param(fp("dec_b")) * sr).max(1.0));
        let end_b = self.raw_param(fp("end_b"));
        let lev_b = self.raw_param(fp("lev_b"));

        let base_hz = note_to_hz(self.current_note, detune_cents / 100.0);
        let c_freq = base_hz * ratio_c;
        let a_freq = base_hz * ratio_a;
        let b1_freq = base_hz * ratio_b1;
        let b2_freq = base_hz * ratio_b2;
        let tau = std::f32::consts::TAU;

        let algo = self.algo;
        for i in start..end {
            // Ticked per-sample (not per-sub-block) for correct time
            // resolution, matching `AdState`'s usage elsewhere in this
            // crate — only the coefficients above are sub-block-cached.
            //
            // `gain_a`/`gain_b`: §0's "modulation-depth multipliers feeding
            // the A/B operator's effective mod_in gain" — applied ONLY at
            // the point A's (resp. the B-pair's) output crosses into
            // *another* operator as `mod_in`. An X/Y tap that listens to A
            // or the B-pair directly (not as modulation) hears its raw,
            // unscaled output — the envelope shapes how much of it *drives
            // something else*, not its own character as a monitored tap.
            // Intra-group hops (B2 feeding B1) and the self/cross feedback
            // terms already routed through `feedback` are unaffected by
            // either gain; those are the `feedback` knob's job, not SYN2's.
            let gain_a = self.env_a.tick(atk_a_inc, dec_a_coeff, end_a, 0.0) * lev_a;
            let gain_b = self.env_b.tick(atk_b_inc, dec_b_coeff, end_b, 0.0) * lev_b;

            let (x, y) = match algo {
                // §0 D1 #1 — Stack: B2->B1->A->C serial. Feedback: B2.
                // X = C, Y = A (pre-carrier, raw/unscaled).
                Algo::Stack => {
                    let b2 = self.op_b2.tick(b2_freq, 0.0, feedback, 0.0, sr);
                    let b1 = self.op_b1.tick(b1_freq, b2, 0.0, 0.0, sr);
                    let a = self.op_a.tick(a_freq, b1 * gain_b, 0.0, harm, sr);
                    let c = self.op_c.tick(c_freq, a * gain_a, 0.0, harm, sr);
                    (c, a)
                }
                // §0 D1 #2 — Short Stack: B1->A->C serial; B2 sums with B1
                // into A as a fixed-offset unison pair. Feedback: A.
                // X = C, Y = B1+B2 raw pair (pre-A, raw/unscaled).
                Algo::ShortStack => {
                    let b1 = self.op_b1.tick(b1_freq, 0.0, 0.0, 0.0, sr);
                    let b2 = self.op_b2.tick(b2_freq, 0.0, 0.0, 0.0, sr);
                    let pair = (b1 + b2) * 0.5;
                    let a = self.op_a.tick(a_freq, pair * gain_b, feedback, harm, sr);
                    let c = self.op_c.tick(c_freq, a * gain_a, 0.0, harm, sr);
                    (c, pair)
                }
                // §0 D1 #3 — Y-Split: B1->A and B2->A in parallel, A->C.
                // Feedback: A. X = C, Y = A alone (raw/unscaled).
                Algo::YSplit => {
                    let b1 = self.op_b1.tick(b1_freq, 0.0, 0.0, 0.0, sr);
                    let b2 = self.op_b2.tick(b2_freq, 0.0, 0.0, 0.0, sr);
                    let combined = (b1 + b2) * 0.5;
                    let a = self.op_a.tick(a_freq, combined * gain_b, feedback, harm, sr);
                    let c = self.op_c.tick(c_freq, a * gain_a, 0.0, harm, sr);
                    (c, a)
                }
                // §0 D1 #4 — Branch: B1->C direct, B2->A->C deep, sum into
                // C. Feedback: B2. X = full sum, Y = A->C branch alone (B1
                // muted). C's phase clock is only ever advanced once (via
                // the X-tap `tick()` call, using the full mod sum) — Y reads
                // the operator's already-advanced `phase` directly through
                // the free `harmonic_sine` function rather than calling
                // `tick()` a second time, which would double-advance it.
                // Both B1's direct hop into C and B2's hop into A cross the
                // B-group boundary, so both get `gain_b`; A's hop into C
                // gets `gain_a`.
                Algo::Branch => {
                    let b1 = self.op_b1.tick(b1_freq, 0.0, 0.0, 0.0, sr);
                    let b2 = self.op_b2.tick(b2_freq, 0.0, feedback, 0.0, sr);
                    let a = self.op_a.tick(a_freq, b2 * gain_b, 0.0, harm, sr);
                    let a_mod = a * gain_a;
                    let full_mod = b1 * gain_b + a_mod;
                    let c_full = self.op_c.tick(c_freq, full_mod, 0.0, harm, sr);
                    let y = harmonic_sine(self.op_c.phase + a_mod / tau, harm);
                    (c_full, y)
                }
                // §0 D1 #5 — Parallel Carriers: B1->C, B2->A; C and A both
                // output directly and sum. Feedback: B1<->B2 mutual (cross
                // only — each reads the OTHER's previous sample, snapshotted
                // before either ticks so the order of the two calls below
                // cannot matter; unaffected by `gain_b`, which governs
                // group-B *output* crossing out, not the pair's own mutual
                // coupling). X = C branch alone, Y = A branch alone
                // (raw/unscaled — A never modulates anything else here, so
                // `gain_a` has no crossing point to apply at in this algo).
                Algo::ParallelCarriers => {
                    let b1_prev = self.op_b1.prev_out;
                    let b2_prev = self.op_b2.prev_out;
                    let b1 = self.op_b1.tick(b1_freq, feedback * b2_prev, 0.0, 0.0, sr);
                    let b2 = self.op_b2.tick(b2_freq, feedback * b1_prev, 0.0, 0.0, sr);
                    let c = self.op_c.tick(c_freq, b1 * gain_b, 0.0, harm, sr);
                    let a = self.op_a.tick(a_freq, b2 * gain_b, 0.0, harm, sr);
                    (c, a)
                }
                // §0 D1 #6 — Feedback Pair: B1<->B2 mutual feedback pair ->
                // A -> C serial. Feedback: "self + cross" — each of B1/B2
                // gets its OWN self-feedback (via `tick`'s `feedback` arg)
                // *and* the other's previous sample (via `mod_in`), which is
                // what distinguishes this from Parallel Carriers' cross-only
                // pair; neither is scaled by `gain_b` (feedback's own knob).
                // The pair's combined hop into A gets `gain_b`; A's hop into
                // C gets `gain_a`. X = C, Y = A (raw/unscaled).
                Algo::FeedbackPair => {
                    let b1_prev = self.op_b1.prev_out;
                    let b2_prev = self.op_b2.prev_out;
                    let b1 = self.op_b1.tick(b1_freq, feedback * b2_prev, feedback, 0.0, sr);
                    let b2 = self.op_b2.tick(b2_freq, feedback * b1_prev, feedback, 0.0, sr);
                    let pair = (b1 + b2) * 0.5;
                    let a = self.op_a.tick(a_freq, pair * gain_b, 0.0, harm, sr);
                    let c = self.op_c.tick(c_freq, a * gain_a, 0.0, harm, sr);
                    (c, a)
                }
                // §0 D1 #7 — Additive Twin: C (unmodulated pure tone) and
                // A<-B1<-B2 (a full 3-op stack, A itself the second output
                // tap) sum, independently leveled via `mix`. Feedback: B2.
                // B2's hop into B1 is intra-group (unscaled); B1's hop into
                // A crosses the boundary, so it gets `gain_b`. `gain_a` has
                // no crossing point here — A is itself the Y tap, it never
                // modulates C (`c` is fed 0.0, matching D1's "unmodulated
                // pure tone"). X = C alone, Y = stack (A, raw/unscaled).
                Algo::AdditiveTwin => {
                    let b2 = self.op_b2.tick(b2_freq, 0.0, feedback, 0.0, sr);
                    let b1 = self.op_b1.tick(b1_freq, b2, 0.0, 0.0, sr);
                    let a = self.op_a.tick(a_freq, b1 * gain_b, 0.0, harm, sr);
                    let c = self.op_c.tick(c_freq, 0.0, 0.0, harm, sr);
                    (c, a)
                }
                // §0 D1 #8 — Bell: A->C (feedback-heavy) and B1+B2 (fixed
                // unison pair) -> C directly, both into carrier. Feedback: A
                // (deep, self-only — unaffected by `gain_a`). Both A's and
                // the B-pair's hops into C cross their respective group
                // boundaries, so `gain_a`/`gain_b` apply to each. Same
                // single-phase-advance-then-peek technique as Branch above
                // for the muted-A Y tap.
                Algo::Bell => {
                    let a = self.op_a.tick(a_freq, 0.0, feedback, harm, sr);
                    let a_mod = a * gain_a;
                    let b1 = self.op_b1.tick(b1_freq, 0.0, 0.0, 0.0, sr);
                    let b2 = self.op_b2.tick(b2_freq, 0.0, 0.0, 0.0, sr);
                    let b_pair_mod = (b1 + b2) * 0.5 * gain_b;
                    let full_mod = a_mod + b_pair_mod;
                    let c_full = self.op_c.tick(c_freq, full_mod, 0.0, harm, sr);
                    let y = harmonic_sine(self.op_c.phase + b_pair_mod / tau, harm);
                    (c_full, y)
                }
            };
            let out = x * (1.0 - mix) + y * mix;
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
    }
}

impl Default for FmVoice {
    fn default() -> Self { Self::new() }
}

impl ViewPlugin for FmVoice {
    fn to_rule(&self, _node_id: u64, _sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule {
        const SRC: &str = "SRC";
        let src = |name: &str, slot: u8| (fp(name), PageRef { page: Cow::Borrowed(SRC), slot });
        Rule {
            name: Cow::Borrowed("FM Voice"),
            // AMP/MOD are declared now, empty, so the page order is stable
            // across commits — P14-C3/C4/C6 fill them in, they don't append
            // a new group after the fact.
            page_groups: Cow::Owned(vec![Cow::Borrowed("SRC"), Cow::Borrowed("AMP"), Cow::Borrowed("MOD")]),
            // SYN2 (P14-C3) is SRC **sub-page 2** (slots 8-15), not a
            // separate page_groups entry — ADR-043's post-ratification
            // amendment 1 is explicit: "SYN1/SYN2 are SRC sub-pages 1/2."
            param_pages: Cow::Owned(vec![
                src("algo", 0),
                src("ratio_c", 1),
                src("ratio_a", 2),
                src("ratio_b", 3),
                src("harm", 4),
                src("detune", 5),
                src("feedback", 6),
                src("mix", 7),
                src("atk_a", 8),
                src("dec_a", 9),
                src("end_a", 10),
                src("lev_a", 11),
                src("atk_b", 12),
                src("dec_b", 13),
                src("end_b", 14),
                src("lev_b", 15),
            ]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        }
    }
}

impl Node for FmVoice {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn capability_document(&self) -> CapabilityDocument {
        let mut doc = Self::build_doc();
        doc.view = Some(self.to_rule(0, &[]));
        doc
    }

    fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
        self.pending_initial_params = params.clone();
    }

    fn serialize(&self) -> Vec<u8> {
        self.bank.serialize()
    }

    fn deserialize(&mut self, data: &[u8]) {
        self.bank.deserialize(data);
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate = sample_rate;
        let doc = Self::build_doc();
        self.bank = ParameterBank::from_capability_document(&doc);
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, value);
            }
        }
        self.render_l = vec![0.0; block_size];
        self.render_r = vec![0.0; block_size];
        self.machine = Machine::Full;
        self.algo = Algo::Stack;
        self.op_c.reset();
        self.op_a.reset();
        self.op_b1.reset();
        self.op_b2.reset();
        self.env_a = AdsrState::new();
        self.env_b = AdsrState::new();
        self.current_note = 60;
        self.active = false;
        self.last_note = 60;
        self.velocity_level = 1.0;
        self.switch_fade = None;
        self.node_locks.clear();
        self.locks_pending = false;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let _ = output;
        self.bank.handle_commands(input.commands);
        self.poll_algo_param();

        let block_size = input.block_size;
        for s in &mut self.render_l { *s = 0.0; }
        for s in &mut self.render_r { *s = 0.0; }

        self.locks_pending = false;

        let mut cursor = 0usize;
        for timed in input.events {
            match timed.event {
                Event::ParamLock(ref pl) if pl.node_id == self.node_id => {
                    self.push_lock(pl.param_id, pl.value);
                }
                Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(n))) => {
                    let off = timed.sample_offset as usize;
                    if off > cursor {
                        self.render_span(cursor, off);
                        cursor = off;
                    }
                    let velocity = n.velocity() as f32 / 65535.0;
                    self.retrigger(u8::from(n.note_number()), velocity);
                }
                _ => {}
            }
        }
        self.render_span(cursor, block_size);
        self.apply_switch_fade(block_size);

        if let Some(buf) = output.audio_outputs.first_mut() {
            if buf.channels() >= 2 {
                for (dst, &src) in buf.channel_mut(0).iter_mut().zip(self.render_l[..block_size].iter()) {
                    *dst = src;
                }
                for (dst, &src) in buf.channel_mut(1).iter_mut().zip(self.render_r[..block_size].iter()) {
                    *dst = src;
                }
            } else if buf.channels() == 1 {
                for (i, (&l, &r)) in self.render_l[..block_size].iter()
                    .zip(self.render_r[..block_size].iter()).enumerate()
                {
                    buf.channel_mut(0)[i] = (l + r) * 0.5;
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab,
        TimedEvent, TransportInfo,
        UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7},
    };
    use crate::engine_dsp::AdsrPhase;

    fn make_note_on(note: u8) -> TimedEvent {
        let mut msg = NoteOn::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note & 0x7F));
        msg.set_velocity(32768);
        TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(msg))))
    }

    fn run(eng: &mut FmVoice, events: &[TimedEvent]) -> Vec<f32> {
        let block = 512usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let ap: *mut AudioBuffer = &mut audio;
        let ar: &mut AudioBuffer = unsafe { &mut *ap };
        let mut outs = [ar];
        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        eng.process(&input, &mut output);
        audio.channel(0).to_vec()
    }

    fn set_algo(eng: &mut FmVoice, algo: Algo) {
        eng.bank.set(fp("algo"), algo.value() as f64);
    }

    fn new_engine() -> FmVoice {
        let mut eng = FmVoice::new();
        eng.activate(44100.0, 512);
        eng
    }

    /// Every algorithm must produce non-silent, finite output for a plain
    /// triggered note at defaults — the baseline structural sanity check
    /// P14-C2's spec asks for.
    #[test]
    fn every_algorithm_is_non_silent_and_finite() {
        for algo in Algo::ALL {
            let mut eng = new_engine();
            set_algo(&mut eng, algo);
            // Let the algo-switch fade (if any) resolve, then trigger.
            run(&mut eng, &[]);
            let out = run(&mut eng, &[make_note_on(60)]);
            assert!(out.iter().all(|s| s.is_finite()), "{:?} produced a non-finite sample", algo);
            let rms = (out.iter().map(|&x| x * x).sum::<f32>() / out.len() as f32).sqrt();
            assert!(rms > 1e-4, "{:?} was effectively silent (rms {rms})", algo);
        }
    }

    /// `ratio_c`/`ratio_a`/`ratio_b` must reach both ends of `RATIO_TABLE`
    /// without panicking or producing non-finite output — the table's
    /// extremes (0.25 and 16.0) are the widest range a performer can dial.
    #[test]
    fn ratio_table_extremes_are_reachable_on_every_operator() {
        let last_index = (RATIO_TABLE.len() - 1) as f64;
        for &(param, idx) in &[("ratio_c", 0.0), ("ratio_c", last_index),
                                ("ratio_a", 0.0), ("ratio_a", last_index),
                                ("ratio_b", 0.0), ("ratio_b", last_index)] {
            let mut eng = new_engine();
            eng.bank.set(fp(param), idx);
            let out = run(&mut eng, &[make_note_on(60)]);
            assert!(out.iter().all(|s| s.is_finite()), "{param}={idx} produced a non-finite sample");
        }
    }

    /// `harm` at both ends (0.0 pure sine, 1.0 the full 5-partial stack)
    /// must not destabilize any algorithm.
    #[test]
    fn harm_extremes_are_reachable_on_every_algorithm() {
        for algo in Algo::ALL {
            for harm in [0.0, 1.0] {
                let mut eng = new_engine();
                set_algo(&mut eng, algo);
                run(&mut eng, &[]);
                eng.bank.set(fp("harm"), harm);
                let out = run(&mut eng, &[make_note_on(60)]);
                assert!(
                    out.iter().all(|s| s.is_finite()),
                    "{algo:?} at harm={harm} produced a non-finite sample"
                );
            }
        }
    }

    /// `ratio_from_index` must clamp rather than panic on an out-of-range
    /// index — a malformed project could carry one (same convention as
    /// `Algo::from_value`).
    #[test]
    fn ratio_from_index_clamps_out_of_range() {
        assert_eq!(ratio_from_index(-5.0), RATIO_TABLE[0]);
        assert_eq!(ratio_from_index(1000.0), RATIO_TABLE[RATIO_TABLE.len() - 1]);
    }

    #[test]
    fn algo_from_value_clamps_out_of_range() {
        assert_eq!(Algo::from_value(999), Algo::Stack);
    }

    /// A mid-note algo switch must fade to silence (not click) before the
    /// swap, and the swap itself must land on the far side of that fade —
    /// the actual deliverable this commit's "Anti-click on algo switch"
    /// bullet promises. Every other test here changes `algo` while the
    /// voice is inactive, where `poll_algo_param`'s `!self.active` branch
    /// swaps instantly with no fade at all — none of them exercise this.
    #[test]
    fn mid_note_algo_switch_fades_out_then_swaps_without_clicking() {
        let mut eng = new_engine();
        set_algo(&mut eng, Algo::Stack);
        run(&mut eng, &[]); // resolve the initial (instant, inactive) switch
        let out = run(&mut eng, &[make_note_on(60)]);
        assert!(out.iter().any(|&s| s.abs() > 1e-3), "must be sounding before the switch");

        set_algo(&mut eng, Algo::Bell);
        assert_eq!(eng.algo, Algo::Stack, "must not swap immediately while active");
        let fade_len = eng.fade_len() as usize;
        let out = run(&mut eng, &[]);

        // The fade window must actually decay toward silence, not click
        // straight to zero and not stay at full volume throughout.
        let fade_region = &out[..fade_len];
        let early_peak = fade_region[..20].iter().cloned().fold(0.0f32, |m, s| m.max(s.abs()));
        let late_peak = fade_region[fade_len - 20..].iter().cloned().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(early_peak > late_peak, "fade must decay: early {early_peak}, late {late_peak}");
        assert!(late_peak < 0.05, "fade must reach near-silence by its end, got {late_peak}");

        // The fade window (220 samples) is shorter than the block (512), so
        // the remainder of this same block must stay silent, not resume the
        // old algo's output while waiting for the next block's swap.
        assert!(
            out[fade_len..].iter().all(|&s| s.abs() < 1e-6),
            "must stay silent after the fade window until the swap, not resume the old algo"
        );
        assert_eq!(eng.algo, Algo::Bell, "swap must complete by the end of the block containing the fade");

        // And the voice must actually resume sounding on the very next
        // block, not stay stuck silent under the new algorithm.
        let out = run(&mut eng, &[]);
        assert!(out.iter().any(|&s| s.abs() > 1e-3), "must resume sounding under the new algo after the swap");
    }

    /// Re-setting `algo` to the value it's already at, mid-note, must not
    /// start a fade (or silence the voice) at all.
    #[test]
    fn setting_algo_to_its_current_value_mid_note_is_a_no_op() {
        let mut eng = new_engine();
        set_algo(&mut eng, Algo::Stack);
        run(&mut eng, &[]);
        run(&mut eng, &[make_note_on(60)]);

        set_algo(&mut eng, Algo::Stack); // same value while active
        let out = run(&mut eng, &[]);
        assert!(eng.switch_fade.is_none(), "same-value switch must not start a fade");
        assert!(out.iter().any(|&s| s.abs() > 1e-3), "must keep sounding, no silence inserted");
    }

    /// Group A's mod envelope must actually reach `end_a` and hold there —
    /// the P14-C3 spec's explicit test requirement. Reads the private
    /// `env_a` field directly (same file, same crate) rather than inferring
    /// convergence from audio output, since that's the actual state this
    /// commit's SYN2 wiring is responsible for.
    #[test]
    fn syn2_group_a_envelope_reaches_end_level_and_holds() {
        let mut eng = new_engine();
        eng.bank.set(fp("atk_a"), 0.001);
        eng.bank.set(fp("dec_a"), 0.05); // the declared minimum
        eng.bank.set(fp("end_a"), 0.4);
        run(&mut eng, &[make_note_on(60)]);
        // 20 blocks * 512 samples ≈ 232 ms — comfortably past a 50 ms decay
        // reaching AdsrState's 1e-4 Decay->Sustain convergence threshold.
        for _ in 0..20 {
            run(&mut eng, &[]);
        }
        assert!(
            matches!(eng.env_a.phase, AdsrPhase::Sustain),
            "expected group A envelope to reach Sustain"
        );
        assert!(
            (eng.env_a.value - 0.4).abs() < 1e-2,
            "expected envelope to hold at end_a=0.4, got {}",
            eng.env_a.value
        );
    }

    /// Mirror of the group A test above — without it, a copy-paste defect
    /// (e.g. `dec_b` silently reading `dec_a`'s param, or `env_b` never
    /// reaching Sustain) would slip past every other test in this file: the
    /// `lev=0` test below only exercises `env_b` through `gain_b` being
    /// multiplied by `lev_b=0`, which hides `env_b`'s own value entirely.
    #[test]
    fn syn2_group_b_envelope_reaches_end_level_and_holds() {
        let mut eng = new_engine();
        eng.bank.set(fp("atk_b"), 0.001);
        eng.bank.set(fp("dec_b"), 0.05); // the declared minimum
        eng.bank.set(fp("end_b"), 0.6);
        run(&mut eng, &[make_note_on(60)]);
        for _ in 0..20 {
            run(&mut eng, &[]);
        }
        assert!(
            matches!(eng.env_b.phase, AdsrPhase::Sustain),
            "expected group B envelope to reach Sustain"
        );
        assert!(
            (eng.env_b.value - 0.6).abs() < 1e-2,
            "expected envelope to hold at end_b=0.6, got {}",
            eng.env_b.value
        );
    }

    /// `lev_a = lev_b = 0.0` must degenerate to a bare, unmodulated carrier
    /// tone at `ratio_c`'s frequency — on **every** algorithm, not just one.
    /// The gain-mapping rule (§0 D3) was applied by hand, separately, across
    /// all 8 `Algo::` match arms — a single-algorithm test provides zero
    /// regression protection for the other 7 (a missing or misplaced
    /// `* gain_a`/`* gain_b` in, say, `Bell`'s arm would not be caught by a
    /// `Stack`-only check). `op_c`'s own `feedback` argument is always 0.0
    /// in every algorithm (D1's Feedback column never names C), so this
    /// same reference carrier is valid for all 8.
    #[test]
    fn lev_zero_degenerates_to_pure_carrier_tone_on_every_algorithm() {
        for algo in Algo::ALL {
            let mut eng = new_engine();
            set_algo(&mut eng, algo);
            eng.bank.set(fp("harm"), 0.0);
            run(&mut eng, &[]); // resolve the initial (instant, inactive) switch
            eng.bank.set(fp("lev_a"), 0.0);
            eng.bank.set(fp("lev_b"), 0.0);
            let out = run(&mut eng, &[make_note_on(60)]);

            let mut op_c_ref = FmOperator::new();
            let base_hz = note_to_hz(60, 0.0);
            let c_freq = base_hz * RATIO_TABLE[RATIO_DEFAULT_INDEX as usize]; // ratio_c default = 1.0
            let velocity_scale = 32768.0_f32 / 65535.0; // matches make_note_on's fixed velocity
            for (i, &got) in out.iter().enumerate() {
                let want = op_c_ref.tick(c_freq, 0.0, 0.0, 0.0, 44100.0) * velocity_scale;
                assert!((got - want).abs() < 1e-4, "{algo:?} sample {i}: got {got}, expected {want}");
            }
        }
    }
}
