use std::collections::HashMap;
use std::borrow::Cow;

use paraclete_node_api::{
    AffordanceHint, CapabilityDocument, EnvelopeGroup, Event, Node, PageRef, ParamDescriptor,
    ParamUnit, ParameterBank, PortDescriptor, PortDirection, PortType, ProcessInput,
    ProcessOutput, Rule, StateBusValue, UmpMessage, ViewPlugin, midi::ChannelVoice2,
};

use crate::engine_dsp::{
    harmonic_sine, lfo_params, note_to_hz, soft_clip, sub_blocks, AdsrState, FmOperator,
    LfoDestLabels, LfoHost, LfoMode, LfoSettings, LfoShape, NoteStack, LFO_PAGE_ORDER,
};

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

// ── MOD page / LFO dest table (P14-C6, ADR-042) ──────────────────────────────

// Reuses the ADR-042 `LfoHost`/`LfoBlock` machinery FmEngine already ships —
// see fm_engine.rs's own dest tables/`get_param`/`update_lfo` for the pattern
// this mirrors exactly.

/// **APPEND ONLY** — `lfo_dest` persists a one-based index into this, so a
/// reorder re-points every saved patch at a different destination. Every
/// modulatable param declared so far (SYN1 minus `algo`, SYN2, AMP,
/// expression minus `env_reset`) — `algo`/`env_reset` are topology/behavior
/// selectors, not modulation targets, same reasoning as `FmEngine` excluding
/// its own `machine` param from `LFO_DESTS`. `ratio_c`/`ratio_a`/`ratio_b`
/// ARE included, deliberately: they're stepped (an LFO sweeps through
/// `RATIO_TABLE`'s indices, rounding same as a p-lock would), but per the
/// user's "keep it expressive" bias (P14 spec's framing) an LFO-driven ratio
/// sweep is a real, usable FM effect, not a knob that should be walled off
/// just because its underlying param happens to be stepped.
///
/// This is currently also `Full`'s own active table (only `Full` is
/// behaviorally reachable — `dest_names`/`dest_ids`/`dest_ranges` below
/// already dispatch per-`Machine` so P14-C7's six macros can each get their
/// own narrower table without restructuring this one).
static FM_VOICE_DEST_NAMES: &[&str] = &[
    "ratio_c", "ratio_a", "ratio_b", "harm", "detune", "feedback", "mix",
    "atk_a", "dec_a", "end_a", "lev_a", "atk_b", "dec_b", "end_b", "lev_b",
    "attack", "decay", "sustain", "release", "volume", "pan", "drive", "tone",
    "keytrack", "vel_mod", "porta",
];

/// Same table as compile-time ids — the audio thread reads these, never a
/// name hash. APPEND ONLY, same contract as the names above.
const FM_VOICE_DEST_IDS: &[u32] = &[
    ParamDescriptor::id_for_name("ratio_c"),
    ParamDescriptor::id_for_name("ratio_a"),
    ParamDescriptor::id_for_name("ratio_b"),
    ParamDescriptor::id_for_name("harm"),
    ParamDescriptor::id_for_name("detune"),
    ParamDescriptor::id_for_name("feedback"),
    ParamDescriptor::id_for_name("mix"),
    ParamDescriptor::id_for_name("atk_a"),
    ParamDescriptor::id_for_name("dec_a"),
    ParamDescriptor::id_for_name("end_a"),
    ParamDescriptor::id_for_name("lev_a"),
    ParamDescriptor::id_for_name("atk_b"),
    ParamDescriptor::id_for_name("dec_b"),
    ParamDescriptor::id_for_name("end_b"),
    ParamDescriptor::id_for_name("lev_b"),
    ParamDescriptor::id_for_name("attack"),
    ParamDescriptor::id_for_name("decay"),
    ParamDescriptor::id_for_name("sustain"),
    ParamDescriptor::id_for_name("release"),
    ParamDescriptor::id_for_name("volume"),
    ParamDescriptor::id_for_name("pan"),
    ParamDescriptor::id_for_name("drive"),
    ParamDescriptor::id_for_name("tone"),
    ParamDescriptor::id_for_name("keytrack"),
    ParamDescriptor::id_for_name("vel_mod"),
    ParamDescriptor::id_for_name("porta"),
];

/// Per-dest (id, min, max) — the LFO scales/clamps against each param's own
/// declared range (BUG-069), not the bank's union (moot right now since
/// `Full` IS the union, but keeping the mechanism matches `FmEngine`'s so
/// C7's narrower macro tables need no new plumbing). Every tuple here is a
/// direct copy of that param's `ParamDescriptor` range declared above.
const FM_VOICE_DEST_RANGES: &[(u32, f32, f32)] = &[
    (ParamDescriptor::id_for_name("ratio_c"), 0.0, (RATIO_TABLE.len() - 1) as f32),
    (ParamDescriptor::id_for_name("ratio_a"), 0.0, (RATIO_TABLE.len() - 1) as f32),
    (ParamDescriptor::id_for_name("ratio_b"), 0.0, (RATIO_TABLE.len() - 1) as f32),
    (ParamDescriptor::id_for_name("harm"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("detune"), -50.0, 50.0),
    (ParamDescriptor::id_for_name("feedback"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("mix"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("atk_a"), 0.001, 0.5),
    (ParamDescriptor::id_for_name("dec_a"), 0.05, 4.0),
    (ParamDescriptor::id_for_name("end_a"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("lev_a"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("atk_b"), 0.001, 0.5),
    (ParamDescriptor::id_for_name("dec_b"), 0.05, 4.0),
    (ParamDescriptor::id_for_name("end_b"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("lev_b"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("attack"), 0.001, 4.0),
    (ParamDescriptor::id_for_name("decay"), 0.01, 8.0),
    (ParamDescriptor::id_for_name("sustain"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("release"), 0.01, 8.0),
    (ParamDescriptor::id_for_name("volume"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("pan"), -1.0, 1.0),
    (ParamDescriptor::id_for_name("drive"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("tone"), -1.0, 1.0),
    (ParamDescriptor::id_for_name("keytrack"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("vel_mod"), 0.0, 1.0),
    (ParamDescriptor::id_for_name("porta"), 0.0, 2.0),
];

static FM_VOICE_DEST_LABELS: LfoDestLabels = LfoDestLabels(FM_VOICE_DEST_NAMES);

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

// ── Portamento (P14-C5) ───────────────────────────────────────────────────────

/// In-flight glide from the note being left to the note-stack-pop's
/// revealed note. Only ever started by `legato_retarget` (a `porta > 0`
/// stack-pop) — a fresh `retrigger()` never glides, matching how a real
/// mono synth's portamento only bridges held-note transitions, not a new
/// keypress. Advanced at sub-block granularity (once per `process_algo`
/// call, by that call's own span length) — the same control-rate cadence
/// ADR-042 already accepted for LFOs, plenty smooth for a pitch glide.
#[derive(Clone, Copy)]
struct Glide {
    from_hz: f32,
    elapsed_secs: f32,
    total_secs: f32,
}

// ── AMP page DSP (P14 §0 D4) ─────────────────────────────────────────────────

/// Pre-gain multiplier at `drive=1.0`, before `soft_clip`. `drive<=0.0` is
/// an **exact** bypass (not just an approximate one via `soft_clip(x*1.0)`,
/// which still audibly rounds typical FM-voice amplitudes) — chosen so
/// "drive=0 is a no-op" is a literal regression guarantee, not a tolerance.
const DRIVE_GAIN_K: f32 = 3.0;

/// One-pole cutoff for the `tone` tilt filter — small and gentle, a subtle
/// tilt rather than a real filter sweep (this is not a filter-design phase;
/// C4's job is AMP-page plumbing, not a proper tone-stack).
const TONE_LP_COEFF: f32 = 0.05;

/// `drive<=0.0` → `x` unchanged. Otherwise `soft_clip` (`tanh`) on a
/// drive-scaled pre-gain — same shape as `DistortionNode`'s own
/// `tanh(x*exp(drive*4))`, but a plain linear pre-gain and a branch bypass
/// rather than `exp(drive*4)` at `drive=0`: `DistortionNode`'s own "drive=0
/// passthrough" test only holds to within its own stated tolerance for a
/// *small* input (0.01) — `tanh` is not truly identity there for
/// voice-level amplitude, and this AMP stage's own exit bar (this commit's
/// spec) requires an exact match against pre-C4 output at `drive=0`.
#[inline(always)]
fn apply_drive(x: f32, drive: f32) -> f32 {
    if drive <= 0.0 {
        x
    } else {
        soft_clip(x * (1.0 + drive * DRIVE_GAIN_K))
    }
}

/// Equal-power pan law — identical formula to `Sampler`'s `pan_l`/`pan_r`
/// (`sampler.rs`), so the crate has one pan convention, not two.
#[inline(always)]
fn equal_power_pan(pan: f32) -> (f32, f32) {
    let l = ((1.0 - pan) * 0.5 + 0.5).sqrt();
    let r = ((1.0 + pan) * 0.5 + 0.5).sqrt();
    (l, r)
}

// ── FmVoice ───────────────────────────────────────────────────────────────────

/// Four-operator phase-modulation melodic voice (ADR-043, P14). Topology:
/// carrier **C**, modulator **A**, linked pair **B1**+**B2** (one `ratio_b`
/// control, B2 = B1 + `B_PAIR_SPREAD`) — eight curated algorithms (§0 D1)
/// define how they connect. Monophonic per track in v1 (ADR-043 decision 4).
///
/// Ports: events_in (0, Event), audio_out_l (1, Mono), audio_out_r (2, Mono).
///
/// P14-C0-C4: SYN1 algorithm routing, SYN2 modulation envelopes, AMP page
/// (attack/decay/sustain/release/volume/pan/drive/tone) with `NoteOff`
/// consumed through a mono note stack. No expression page or portamento
/// yet (C5), no MOD page (C6), no machine variants beyond `Full` (C7).
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

    /// P14-C4: the actual gate on whether the voice produces audible
    /// output. Replaces the old `active: bool` flag, which flipped false
    /// the instant a key lifted — wrong once `NoteOff` triggers a real
    /// Release tail that must keep rendering after the note stack empties.
    /// See `is_sounding()`.
    env_amp: AdsrState,

    /// P14-C0's foundation, first wired here: mono note stack (last-note
    /// priority). A `NoteOff` pops it; if a note remains, the voice
    /// legato-retargets to it instead of going silent.
    note_stack: NoteStack,

    /// One-pole state for the `tone` tilt filter (P14-C4) — persists across
    /// notes (not reset on retrigger), since a tone-shaping filter is a
    /// downstream audio stage, not part of the oscillator core.
    tone_lp_state: f32,

    /// P14-C5 portamento: `Some` while a note-stack-pop's glide is still in
    /// flight. Always cleared by a hard `retrigger()` — a fresh keypress
    /// never glides.
    glide: Option<Glide>,

    /// P14-C6, ADR-042: one shared LFO. Retriggered on a hard `retrigger()`
    /// (a fresh keypress); **not** on `legato_retarget` — a note-stack pop's
    /// whole point is continuity, and resetting the LFO's phase there would
    /// be exactly the kind of modulation jump that path exists to avoid,
    /// same reasoning as `legato_retarget` never resetting operator phase.
    lfo: LfoHost,

    /// Cached `/node/{id}/state/lfo_phase` path (BUG-007: no `format!` on
    /// the audio thread after the first cycle) — same technique
    /// `publish_bank_state`'s own `path_cache` uses, since this field isn't
    /// covered by that helper (it's an extra, hand-published state entry,
    /// not a bank param). Found in P14-C6 review: the first draft called
    /// `format!` unconditionally in `published_state()`, run every cycle.
    lfo_phase_path: std::sync::OnceLock<String>,

    current_note: u8,
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

    /// The union/`Full` dest table's width — see the module-level dest-table
    /// comment. `lfo_dest`'s declared range is this, never a narrower
    /// per-machine table's (ADR-041 union-bank invariant: the bank never
    /// narrows for a macro).
    const LFO_DESTS: &'static [u32] = FM_VOICE_DEST_IDS;

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
            env_amp: AdsrState::new(),
            note_stack: NoteStack::new(),
            tone_lp_state: 0.0,
            glide: None,
            lfo: LfoHost::new(),
            lfo_phase_path: std::sync::OnceLock::new(),
            current_note: 60,
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

    /// §0 D4's AMP page, exactly 8 params. `attack`/`decay`/`release` drive
    /// the new `AdsrState` gating the voice's own audibility (`env_amp` —
    /// see `is_sounding()`); `sustain` is a true held level (unlike SYN2's
    /// `end_a`/`end_b`, which are modulation levels, this is the actual
    /// output gain a held note settles at). `drive`/`tone` are a post-op
    /// waveshaper (`apply_drive`/`apply_tone`); `volume`/`pan` are the
    /// final stage, `pan` using the same equal-power law `Sampler` already
    /// uses (`sampler.rs`'s `pan_l`/`pan_r`), for one pan convention
    /// crate-wide rather than inventing a second.
    ///
    /// Defaults: fast attack / moderate decay+release / high sustain — a
    /// held note should sound characterful immediately, not require a
    /// performer to dial in an envelope before anything is audible.
    /// `drive`/`tone` default to 0.0, their exact-bypass value (see the two
    /// functions) — a fresh voice's AMP stage should not color the tone
    /// until asked to.
    fn amp_params() -> Vec<ParamDescriptor> {
        vec![
            ParamDescriptor {
                id: fp("attack"), name: "attack".into(),
                min: 0.001, max: 4.0, default: 0.005,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
            },
            ParamDescriptor {
                id: fp("decay"), name: "decay".into(),
                min: 0.01, max: 8.0, default: 0.3,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
            },
            ParamDescriptor {
                id: fp("sustain"), name: "sustain".into(),
                min: 0.0, max: 1.0, default: 0.7,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("release"), name: "release".into(),
                min: 0.01, max: 8.0, default: 0.2,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
            },
            ParamDescriptor {
                id: fp("volume"), name: "volume".into(),
                min: 0.0, max: 1.0, default: 1.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("pan"), name: "pan".into(),
                min: -1.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("drive"), name: "drive".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id: fp("tone"), name: "tone".into(),
                min: -1.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
        ]
    }

    /// ADR-043 decision 4's expression params (4) — placed on **AMP sub-page
    /// 2** (slots 8-11), not a new `page_groups` entry: ADR-038's own
    /// post-ratification amendment revises "FUNC+PG overflow chords" to
    /// "re-pressing the active PG key to cycle sub-pages" — overflow *is*
    /// the sub-page mechanism, the same one SYN1/SYN2 already use on SRC
    /// (ADR-043 amendment 1). A brand-new "EXP" page_groups entry would be
    /// the pre-amendment reading of ADR-038, not the current one.
    ///
    /// `keytrack` and `vel_mod` both default to 0.0 (no effect) — a fresh
    /// voice's brightness should come from `harm`/SYN2 alone until a
    /// performer asks for note- or velocity-dependent variation.
    /// `env_reset` defaults to 0.0 (legato) — this is the exact behavior
    /// P14-C4 hard-coded for every note-stack-pop, so a project with
    /// `env_reset` untouched sees no change from before this commit.
    /// `porta` defaults to 0.0 (instant retune) for the same regression
    /// reason — glide is opt-in.
    fn expression_params() -> Vec<ParamDescriptor> {
        vec![
            // §0/ADR-043 decision 4: "ratio-independent brightness
            // tracking" — the ratio tables already track pitch
            // *proportionally* (every operator's Hz is a multiple of the
            // note's own Hz via `note_to_hz`), so this has to be a
            // *separate* mechanism: it shifts `harm` by how far the
            // current note sits from middle C (note 60), scaled by this
            // depth control. See `process_algo`'s `keytrack_offset`.
            ParamDescriptor {
                id: fp("keytrack"), name: "keytrack".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            // "velocity -> modulation level": scales gain_a/gain_b
            // (SYN2's own depth-scaled envelope outputs) by how far below
            // max velocity the triggering NoteOn was — velocity=1.0 always
            // gives full gain regardless of vel_mod (see process_algo's
            // vel_scale), so this only ever *reduces* brightness at softer
            // playing, never boosts beyond the SYN2 envelopes' own ceiling.
            ParamDescriptor {
                id: fp("vel_mod"), name: "vel_mod".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            // Stepped boolean, same convention as Sampler's "loop"
            // (sampler.rs) — 0.0 = legato (a note-stack pop continues
            // env_a/env_b/env_amp from their current level, P14-C4's
            // behavior), 1.0 = retrig (a pop restarts all three envelopes
            // from zero). Either way, operator phase is NEVER reset on
            // this path — that's `legato_retarget`'s permanent contract
            // (the click env_reset=retrig is not meant to reintroduce),
            // only the envelopes' continuity is what this switches.
            ParamDescriptor {
                id: fp("env_reset"), name: "env_reset".into(),
                min: 0.0, max: 1.0, default: 0.0,
                stepped: true, in_kit: true, unit: ParamUnit::Generic, display: None,
            },
            // Glide time in seconds for a note-stack-pop retarget only —
            // a fresh `retrigger()` (a genuine NoteOn) never glides, the
            // way a real mono synth's portamento doesn't affect a brand
            // new keypress, only a legato transition between held notes.
            ParamDescriptor {
                id: fp("porta"), name: "porta".into(),
                min: 0.0, max: 2.0, default: 0.0,
                stepped: false, in_kit: true, unit: ParamUnit::Seconds, display: None,
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
            // `Self::LFO_DESTS.len()` (26) is the union/`Full` dest-table
            // width, same role `FmEngine::LFO_DESTS.len()` plays for its own
            // `lfo_dest` range — see the dest-table comment above.
            params: Self::params().into_iter().chain(Self::syn2_params()).chain(Self::amp_params())
                .chain(Self::expression_params())
                .chain(lfo_params(Self::LFO_DESTS.len(), Some(&FM_VOICE_DEST_LABELS)))
                .collect(),
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

    /// Parameter read honoring per-cycle `ParamLock` overrides (ADR-019)
    /// **and** the LFO (ADR-042 amendment 1) — see `FmEngine::get_param`.
    /// Every dest-table param this engine reads should go through this, not
    /// `raw_param`, so the LFO's offset actually reaches whichever param
    /// `lfo_dest` currently names — `apply` is a no-op for every other
    /// param, so calling this unconditionally costs nothing when the LFO
    /// targets something else (or is off).
    fn get_param(&self, param_id: u32) -> f32 {
        self.lfo.apply(param_id, self.raw_param(param_id))
    }

    fn lfo_settings(&self) -> LfoSettings {
        LfoSettings {
            shape: LfoShape::from_value(self.raw_param(fp("lfo_shape"))),
            mode: LfoMode::from_value(self.raw_param(fp("lfo_mode"))),
            speed_hz: self.raw_param(fp("lfo_speed")),
            start_phase: self.raw_param(fp("lfo_start_phase")),
            fade: self.raw_param(fp("lfo_fade")),
        }
    }

    /// The destination names offered on `machine`, in index order (P14-C7's
    /// six macros each narrow this — every arm returns the same `Full`/union
    /// table for now, since `Full` is the only behaviorally reachable
    /// `Machine` value until C7 builds `MachineVariant`s for the rest).
    /// Only consumed by this file's own tests right now (a `cargo build`
    /// doesn't see `#[cfg(test)]`) — C7's `dest_param_labels`-equivalent is
    /// its real production consumer, matching `FmEngine::dest_names`.
    #[allow(dead_code)]
    fn dest_names(machine: Machine) -> &'static [&'static str] {
        match machine {
            Machine::Full | Machine::Keys | Machine::Bass | Machine::Bell
            | Machine::Perc | Machine::Pad | Machine::Pluck => FM_VOICE_DEST_NAMES,
        }
    }

    /// Same tables as [`Self::dest_names`], as compile-time ids — the audio
    /// thread reads these, never a name hash.
    fn dest_ids(machine: Machine) -> &'static [u32] {
        match machine {
            Machine::Full | Machine::Keys | Machine::Bass | Machine::Bell
            | Machine::Perc | Machine::Pad | Machine::Pluck => FM_VOICE_DEST_IDS,
        }
    }

    /// The active machine's declared (id, min, max) per destination
    /// (BUG-069) — the LFO scales/clamps against these, not the bank's union.
    fn dest_ranges(machine: Machine) -> &'static [(u32, f32, f32)] {
        match machine {
            Machine::Full | Machine::Keys | Machine::Bass | Machine::Bell
            | Machine::Perc | Machine::Pad | Machine::Pluck => FM_VOICE_DEST_RANGES,
        }
    }

    /// Only consumed by this file's own tests right now, same reason as
    /// `dest_names` above — `build_doc` references `FM_VOICE_DEST_LABELS`
    /// directly since it has no `machine` to dispatch on (it's a bare `fn`,
    /// constructing the capability document before any instance exists).
    #[allow(dead_code)]
    fn dest_labels(machine: Machine) -> &'static LfoDestLabels {
        match machine {
            Machine::Full | Machine::Keys | Machine::Bass | Machine::Bell
            | Machine::Perc | Machine::Pad | Machine::Pluck => &FM_VOICE_DEST_LABELS,
        }
    }

    /// Resolve `lfo_dest` against the active machine's table (#179 pattern).
    /// One-based; an index past the machine's list (or `0`) reads as off.
    fn lfo_dest_id(&self) -> Option<u32> {
        let v = self.raw_param(fp("lfo_dest"));
        // Non-integral values read as off, not truncated — a p-lock bypasses
        // the bank's clamp, so this path can carry a malformed value.
        if !v.is_finite() || v < 1.0 || v.fract() != 0.0 {
            return None;
        }
        Self::dest_ids(self.machine).get(v as usize - 1).copied()
    }

    fn update_lfo(&mut self, samples: usize) {
        let dest = self.lfo_dest_id();
        let range = dest
            .and_then(|d| {
                Self::dest_ranges(self.machine)
                    .iter()
                    .find(|(id, _, _)| *id == d)
                    .map(|&(_, lo, hi)| (lo, hi))
            })
            .unwrap_or((0.0, 1.0));
        let depth = self.raw_param(fp("lfo_depth"));
        let settings = self.lfo_settings();
        let sr = self.sample_rate;
        self.lfo.update(settings, dest, range, depth, sr, samples);
    }

    /// The real gate on whether the voice should render at all (P14-C4).
    /// True while a note is held (`note_stack` non-empty) **or** while
    /// `env_amp` is still working through its Release tail after the last
    /// `NoteOff` — a note that's releasing must keep rendering even though
    /// nothing is currently held. Replaces the old `active: bool` flag,
    /// which had no way to express "no key is down, but still sounding."
    fn is_sounding(&self) -> bool {
        !self.note_stack.is_empty() || !self.env_amp.is_idle()
    }

    /// `tone=0.0` → `x` unchanged (exact bypass, same contract as
    /// `apply_drive`). Otherwise blends toward a one-pole low-pass (darker,
    /// `tone<0`) or its complement (`x - lowpass`, brighter, `tone>0`). The
    /// filter state itself always advances (even at `tone=0`) so engaging
    /// the knob mid-note doesn't start from a stale, uninitialized filter.
    fn apply_tone(&mut self, x: f32, tone: f32) -> f32 {
        self.tone_lp_state += TONE_LP_COEFF * (x - self.tone_lp_state);
        if tone == 0.0 {
            x
        } else if tone > 0.0 {
            let hp = x - self.tone_lp_state;
            x + tone * (hp - x)
        } else {
            x + (-tone) * (self.tone_lp_state - x)
        }
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
        if !self.is_sounding() {
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
    /// declick technique. **Deliberately does not** end the voice the way
    /// `apply_machine_switch` does (there is no `self.active` any more,
    /// P14-C4 replaced it with `is_sounding()`): a machine swap on a drum
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

    /// A `NoteOn` — the very first note of a phrase or an overlapping press
    /// on an already-sounding mono voice. Always a hard reset: a fresh
    /// keypress is a fresh attack, click accepted, and it never glides
    /// (P14-C5: portamento only bridges a note-stack-pop transition, the
    /// way a real mono synth's `porta` doesn't affect a brand new keypress).
    fn retrigger(&mut self, note: u8, velocity: f32) {
        self.consume_pending_locks();
        self.note_stack.push(note);
        self.trigger_note(note, Some(velocity), true, true);
        // A fresh keypress restarts the LFO per its own trig mode (Trig/One/
        // Half reset phase, Free doesn't) — `legato_retarget` deliberately
        // does NOT do this (see the `lfo` field's doc comment).
        self.lfo.trigger(self.lfo_settings());
    }

    /// A `NoteOff` that reveals a still-held note underneath (mono
    /// note-stack pop, e.g. releasing the top of a held chord). **Never**
    /// resets operator phase — that's the click this path exists to avoid,
    /// and it's permanent, not something `env_reset` reintroduces (see
    /// `trigger_note`). `env_reset` (P14-C5) only decides whether
    /// `env_a`/`env_b`/`env_amp` restart from zero (retrig, `env_reset=1`)
    /// or continue from their current level (legato, `env_reset=0`, the
    /// default and P14-C4's original hard-coded behavior).
    ///
    /// Also starts a glide (P14-C5) if `porta > 0`: captures the outgoing
    /// note's frequency as the glide's start point before retargeting.
    ///
    /// **Known limitation**: `NoteStack` (P14-C0) stores note numbers only,
    /// not velocity — the revealed note resumes at whatever
    /// `velocity_level` is already set (the releasing note's), not its own
    /// original velocity. Widening `NoteStack`'s frozen C0 contract to fix
    /// this is out of scope here (OQ-P14-2 already flags the note stack as
    /// provisional pending P13's paraphonic allocator work).
    fn legato_retarget(&mut self, note: u8) {
        // `get_param`, not `raw_param`: P14-C6 makes `porta` a dest-table
        // target — an occasional, note-triggered effect (glide sometimes
        // faster/slower depending where the LFO's cycle happens to be at
        // the instant of release) rather than a continuously-modulated one,
        // but there's nothing unsound about it being read through the same
        // chokepoint every other dest-table param uses.
        let porta = self.get_param(fp("porta"));
        if porta > 0.0 {
            // NOT `note_to_hz(self.current_note, ...)` — if a glide from an
            // *earlier* stack-pop is still in flight (e.g. a 3-note chord
            // released top-down faster than `porta`), the voice is actually
            // sounding somewhere between that glide's start and target, not
            // at the target's nominal frequency yet. Restarting from the
            // nominal frequency would itself be an audible pitch-snap, on
            // the one path that exists specifically to avoid one. Found in
            // C5 review, reachable through Pad's own D5 recipe ("slow
            // attack, porta on" — pads are canonically played as held
            // chords with staggered releases).
            let from_hz = self.current_sounding_hz();
            self.glide = Some(Glide { from_hz, elapsed_secs: 0.0, total_secs: porta });
        } else {
            self.glide = None;
        }
        let env_reset = self.raw_param(fp("env_reset")) >= 0.5;
        self.trigger_note(note, None, false, env_reset);
    }

    /// The frequency the voice is *actually* sounding right now: the live
    /// interpolated point of an in-flight glide, or the plain target
    /// frequency if no glide is running. `self.glide`'s `elapsed_secs` is
    /// already current at the point this is called — `process()`'s event
    /// loop always renders up through an event's own sample offset (which
    /// advances `elapsed_secs` via `process_algo`) before dispatching that
    /// event, so no additional bookkeeping is needed here, only a read.
    fn current_sounding_hz(&self) -> f32 {
        // `get_param`, matching `process_algo`'s own `target_base_hz` read —
        // NOT `raw_param`. P14-C6 review: an earlier draft read `detune` raw
        // here on the (wrong) theory that it mirrors `FmEngine::retrigger`'s
        // `#175` precedent. That precedent doesn't actually apply:
        // `FmEngine` has no legato/note-stack path at all, only a hard
        // `retrigger()` with no prior continuity to preserve. This
        // function's whole job is the opposite — reporting the frequency an
        // *already-sounding* voice is actually producing right now, so a new
        // glide starts from where the pitch really is. If `detune` is
        // LFO-modulated and this read used the unmodulated value instead,
        // the captured start point would differ from what `process_algo`
        // had just rendered by up to the LFO's full swing — an audible snap
        // at the very first glide, not just the overlapping-glide case
        // P14-C5 already fixed.
        let detune_cents = self.get_param(fp("detune"));
        let target_hz = note_to_hz(self.current_note, detune_cents / 100.0);
        match self.glide {
            Some(g) => {
                let t = (g.elapsed_secs / g.total_secs).min(1.0);
                g.from_hz + (target_hz - g.from_hz) * t
            }
            None => target_hz,
        }
    }

    /// Shared core of every note-target change.
    ///
    /// `reset_operators=true` resets the four operators' running phase and
    /// the `tone` filter state — appropriate only for a genuine `NoteOn`
    /// (`retrigger`), where the click is acceptable because the note itself
    /// is new. `legato_retarget` always passes `false`: not resetting
    /// operator phase is the whole reason that path exists, and it does not
    /// vary with `env_reset` (P14-C5) — a stack-pop is never allowed to
    /// reintroduce the click, retrig or not.
    ///
    /// `reset_envelopes` independently decides whether `env_a`/`env_b`/
    /// `env_amp` restart from zero. `retrigger` always passes `true`
    /// (a fresh keypress always restarts everything). `legato_retarget`
    /// passes `env_reset`'s value (P14-C5) — `env_reset=0` (legato, the
    /// default) preserves P14-C4's original behavior exactly.
    fn trigger_note(&mut self, note: u8, velocity: Option<f32>, reset_operators: bool, reset_envelopes: bool) {
        self.current_note = note;
        self.last_note = note;
        if let Some(v) = velocity {
            self.velocity_level = v.clamp(0.0, 1.0);
        }
        if reset_operators {
            self.op_c.reset();
            self.op_a.reset();
            self.op_b1.reset();
            self.op_b2.reset();
            // The oscillator core resets specifically to avoid carrying a
            // discontinuity into the new note; the `tone` one-pole filter
            // sitting downstream of it is the same kind of state and gets
            // the same treatment — otherwise a loud prior tail's filter
            // history blends into the fresh attack under a nonzero `tone`.
            self.tone_lp_state = 0.0;
            // A hard reset is always a fresh attack, never a glide target.
            self.glide = None;
        }
        self.env_a.trigger(reset_envelopes);
        self.env_b.trigger(reset_envelopes);
        self.env_amp.trigger(reset_envelopes);
    }

    /// Render `[start, end)` with the current voice state. Chunked into
    /// `LFO_SUB_BLOCK` pieces relative to `start` (MM-C7 pattern) — P14-C6's
    /// MOD page lands on exactly this loop, same cadence `FmEngine` updates
    /// its own LFO at.
    fn render_span(&mut self, start: usize, end: usize) {
        if start >= end || !self.is_sounding() {
            return;
        }
        for (lo, hi) in sub_blocks(start, end) {
            self.update_lfo(hi - lo);
            self.process_algo(lo, hi);
        }
        let v = self.velocity_level;
        if v != 1.0 {
            for s in &mut self.render_l[start..end] { *s *= v; }
            for s in &mut self.render_r[start..end] { *s *= v; }
        }
    }

    fn process_algo(&mut self, start: usize, end: usize) {
        // §0/ADR-043 decision 4's `keytrack`: "ratio-independent brightness
        // tracking" — the ratio tables already scale every operator's Hz
        // *proportionally* to the note, so this has to shift something
        // else. Shifts `harm` by how far the current note sits from middle
        // C (60), scaled by `keytrack`'s own depth: `keytrack=0` is exactly
        // today's flat `harm` (no note dependence); `keytrack=1` spans
        // +/-0.5 harm across +/-4 octaves from middle C — higher notes get
        // brighter, lower notes darker, a common "brighter on top" EP/bell
        // character. Clamped back into `harm`'s own 0..1 contract.
        const KEYTRACK_CENTER_NOTE: f32 = 60.0;
        const KEYTRACK_RANGE_SEMITONES: f32 = 48.0;
        const KEYTRACK_MAX_HARM_SHIFT: f32 = 0.5;
        // P14-C6: every dest-table param below reads through `get_param`
        // (raw value + the LFO's offset, a no-op unless `lfo_dest` names
        // that exact param) — `keytrack`/`env_reset` excluded, matching
        // `algo`'s exclusion (topology/behavior selectors, not modulation
        // targets, same reasoning `FmEngine` excludes its own `machine`).
        let keytrack = self.get_param(fp("keytrack"));
        let keytrack_offset = keytrack
            * ((self.current_note as f32 - KEYTRACK_CENTER_NOTE) / KEYTRACK_RANGE_SEMITONES)
            * KEYTRACK_MAX_HARM_SHIFT;
        let harm = (self.get_param(fp("harm")) + keytrack_offset).clamp(0.0, 1.0);
        let feedback = self.get_param(fp("feedback"));
        let mix = self.get_param(fp("mix"));
        let detune_cents = self.get_param(fp("detune"));
        let ratio_c = ratio_from_index(self.get_param(fp("ratio_c")));
        let ratio_a = ratio_from_index(self.get_param(fp("ratio_a")));
        let ratio_b1 = ratio_from_index(self.get_param(fp("ratio_b")));
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
        let atk_a_inc = 1.0 / (self.get_param(fp("atk_a")) * sr).max(1.0);
        let dec_a_coeff = 0.001f32.powf(1.0 / (self.get_param(fp("dec_a")) * sr).max(1.0));
        let end_a = self.get_param(fp("end_a"));
        let lev_a = self.get_param(fp("lev_a"));
        let atk_b_inc = 1.0 / (self.get_param(fp("atk_b")) * sr).max(1.0);
        let dec_b_coeff = 0.001f32.powf(1.0 / (self.get_param(fp("dec_b")) * sr).max(1.0));
        let end_b = self.get_param(fp("end_b"));
        let lev_b = self.get_param(fp("lev_b"));

        // §0/ADR-043 decision 4's `vel_mod`: "velocity -> modulation level".
        // `vel_mod=0` is exactly today's flat gain (no velocity dependence,
        // the default). At full velocity (1.0), `vel_scale` is always 1.0
        // regardless of `vel_mod` — this only ever *reduces* brightness at
        // softer playing, it never boosts past what lev_a/lev_b/the SYN2
        // envelopes already allow.
        let vel_mod = self.get_param(fp("vel_mod"));
        let vel_scale = 1.0 - vel_mod * (1.0 - self.velocity_level);

        // §0 D4's AMP page (P14-C4). Same sub-block-cached-coefficient
        // convention as SYN2 above; `env_amp` is the real gate on the
        // voice's audibility now (see `is_sounding()`), ticked per-sample
        // alongside `env_a`/`env_b`.
        let attack_amp = self.get_param(fp("attack"));
        let decay_amp = self.get_param(fp("decay"));
        let sustain_amp = self.get_param(fp("sustain"));
        let release_amp = self.get_param(fp("release"));
        let volume = self.get_param(fp("volume"));
        let pan = self.get_param(fp("pan"));
        let drive = self.get_param(fp("drive"));
        let tone = self.get_param(fp("tone"));
        let atk_amp_inc = 1.0 / (attack_amp * sr).max(1.0);
        let dec_amp_coeff = 0.001f32.powf(1.0 / (decay_amp * sr).max(1.0));
        let rel_amp_coeff = 0.001f32.powf(1.0 / (release_amp * sr).max(1.0));
        let (pan_l, pan_r) = equal_power_pan(pan);

        // P14-C5 portamento: while a glide is in flight, `base_hz` is the
        // linear interpolation from where the outgoing note left off toward
        // the current (target) note's own frequency, advanced by this call's
        // own span length — `render_span` may call `process_algo` several
        // times per `process()` block (one per `LFO_SUB_BLOCK` chunk), so
        // accumulating `elapsed_secs` here by `(end-start)/sr` each call adds
        // up to the real wall-clock time across the whole block. `porta=0`
        // never creates a `Glide` (see `legato_retarget`), so this reduces
        // to the pre-C5 instant-retune exactly when portamento is off.
        let target_base_hz = note_to_hz(self.current_note, detune_cents / 100.0);
        let base_hz = match self.glide {
            Some(mut g) => {
                g.elapsed_secs += (end - start) as f32 / sr;
                let t = (g.elapsed_secs / g.total_secs).min(1.0);
                let interpolated = g.from_hz + (target_base_hz - g.from_hz) * t;
                self.glide = if t >= 1.0 { None } else { Some(g) };
                interpolated
            }
            None => target_base_hz,
        };
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
            let gain_a = self.env_a.tick(atk_a_inc, dec_a_coeff, end_a, 0.0) * lev_a * vel_scale;
            let gain_b = self.env_b.tick(atk_b_inc, dec_b_coeff, end_b, 0.0) * lev_b * vel_scale;

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

            // P14-C4's AMP stage: post-op waveshaper, then the envelope
            // gate, then the final pan split. `env_amp` is ticked here even
            // though its output also decides `is_sounding()` at the block
            // level — a still-releasing voice keeps ticking its own gate
            // shut across however many blocks Release takes.
            let driven = apply_drive(out, drive);
            let toned = self.apply_tone(driven, tone);
            let amp_gain = self.env_amp.tick(atk_amp_inc, dec_amp_coeff, sustain_amp, rel_amp_coeff) * volume;
            let voiced = toned * amp_gain;
            self.render_l[i] = voiced * pan_l;
            self.render_r[i] = voiced * pan_r;
        }
    }
}

impl Default for FmVoice {
    fn default() -> Self { Self::new() }
}

impl ViewPlugin for FmVoice {
    fn to_rule(&self, _node_id: u64, _sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule {
        const SRC: &str = "SRC";
        const AMP: &str = "AMP";
        let src = |name: &str, slot: u8| (fp(name), PageRef { page: Cow::Borrowed(SRC), slot });
        let amp = |name: &str, slot: u8| (fp(name), PageRef { page: Cow::Borrowed(AMP), slot });
        Rule {
            name: Cow::Borrowed("FM Voice"),
            page_groups: Cow::Owned(vec![Cow::Borrowed("SRC"), Cow::Borrowed("AMP"), Cow::Borrowed("MOD")]),
            // SYN2 (P14-C3) is SRC **sub-page 2** (slots 8-15); the P14-C5
            // expression params are AMP **sub-page 2** (slots 8-11) — same
            // sub-page mechanism, not a separate page_groups entry.
            // ADR-043's post-ratification amendment 1 is explicit about
            // SYN1/SYN2; ADR-038's own post-ratification amendment revises
            // "FUNC+PG overflow chords" to "re-pressing the active PG key
            // to cycle sub-pages" — the expression page's "overflow" *is*
            // this same sub-page cycling, not a new page key.
            param_pages: {
                let mut pages = vec![
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
                    amp("attack", 0),
                    amp("decay", 1),
                    amp("sustain", 2),
                    amp("release", 3),
                    amp("volume", 4),
                    amp("pan", 5),
                    amp("drive", 6),
                    amp("tone", 7),
                    amp("keytrack", 8),
                    amp("vel_mod", 9),
                    amp("env_reset", 10),
                    amp("porta", 11),
                ];
                // P14-C6: MOD page, laid out identically to `FmEngine`'s own
                // (`LFO_PAGE_ORDER`) so a performer's muscle memory carries
                // across tracks.
                for (i, id) in LFO_PAGE_ORDER.iter().enumerate() {
                    pages.push((*id, PageRef { page: Cow::Borrowed("MOD"), slot: i as u8 }));
                }
                Cow::Owned(pages)
            },
            macros: Cow::Borrowed(&[]),
            // Anchored on each group's `dec_*`/`decay` param, matching
            // FmEngine's/AnalogEngine's own EnvelopeCurve convention
            // (`decay_id` is the anchor there too) — the renderer locates
            // the curve overlay from this one param id, the full shape
            // comes from `envelopes[group_idx].param_ids`.
            affordances: Cow::Owned(vec![
                (fp("dec_a"), AffordanceHint::EnvelopeCurve { group_idx: 0 }),
                (fp("dec_b"), AffordanceHint::EnvelopeCurve { group_idx: 1 }),
                (fp("decay"), AffordanceHint::EnvelopeCurve { group_idx: 2 }),
            ]),
            envelopes: Cow::Owned(vec![
                // §0 D6: SYN2's mod-index groups use "ADSR" proper, same as
                // AMP — there's no release stage (the last slot is unused,
                // zero-padded, matching FmEngine's own convention for a
                // group narrower than the full four), but `end_a`/`end_b`
                // occupy the "sustain" slot exactly as intended (§0 D3:
                // "AdsrState used with sustain = end_lev").
                EnvelopeGroup {
                    env_type: Cow::Borrowed("ADSR"),
                    label: Cow::Borrowed("Mod Envelope A"),
                    param_ids: [fp("atk_a"), fp("dec_a"), fp("end_a"), 0],
                },
                EnvelopeGroup {
                    env_type: Cow::Borrowed("ADSR"),
                    label: Cow::Borrowed("Mod Envelope B"),
                    param_ids: [fp("atk_b"), fp("dec_b"), fp("end_b"), 0],
                },
                EnvelopeGroup {
                    env_type: Cow::Borrowed("ADSR"),
                    label: Cow::Borrowed("Amp Envelope"),
                    param_ids: [fp("attack"), fp("decay"), fp("sustain"), fp("release")],
                },
            ]),
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
        // ADR-042 decision 7. Path built once and cached (BUG-007) — the
        // residual `String::clone` per cycle is the same accepted cost
        // `publish_bank_state`'s own `path_cache` pays, not a fresh `format!`.
        let path = self
            .lfo_phase_path
            .get_or_init(|| format!("/node/{}/state/lfo_phase", self.node_id));
        buf.push((path.clone(), StateBusValue::Float(self.lfo.phase() as f64)));
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
        self.env_amp = AdsrState::new();
        self.note_stack = NoteStack::new();
        self.tone_lp_state = 0.0;
        self.glide = None;
        self.lfo = LfoHost::new();
        self.current_note = 60;
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
                Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOff(n))) => {
                    let off = timed.sample_offset as usize;
                    if off > cursor {
                        self.render_span(cursor, off);
                        cursor = off;
                    }
                    let note = u8::from(n.note_number());
                    match self.note_stack.pop_matching(note) {
                        Some(new_top) => self.legato_retarget(new_top),
                        None => self.env_amp.note_off(),
                    }
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
        UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOff, NoteOn, u4, u7},
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

    fn make_note_off(note: u8) -> TimedEvent {
        let mut msg = NoteOff::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note & 0x7F));
        msg.set_velocity(0);
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
    /// voice is not sounding, where `poll_algo_param`'s `!self.is_sounding()`
    /// branch swaps instantly with no fade at all — none of them exercise
    /// this.
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
    ///
    /// P14-C4 note: the AMP envelope now shapes every sample (its own
    /// ramp/sustain, not a constant 1.0), so this test pins `attack` to its
    /// minimum and `sustain` to 1.0 and only compares samples past the
    /// attack ramp (64, one sub-block — comfortably past the ~44-sample
    /// ramp at these settings) — `volume`/`pan`/`drive`/`tone` stay at their
    /// exact-no-op defaults (1.0/0.0/0.0/0.0), so the reference carrier
    /// still needs no scaling of its own in that region.
    #[test]
    fn lev_zero_degenerates_to_pure_carrier_tone_on_every_algorithm() {
        for algo in Algo::ALL {
            let mut eng = new_engine();
            set_algo(&mut eng, algo);
            eng.bank.set(fp("harm"), 0.0);
            eng.bank.set(fp("attack"), 0.001); // AMP page min — reaches ceiling fast
            eng.bank.set(fp("sustain"), 1.0); // no headroom loss once settled
            run(&mut eng, &[]); // resolve the initial (instant, inactive) switch
            eng.bank.set(fp("lev_a"), 0.0);
            eng.bank.set(fp("lev_b"), 0.0);
            let out = run(&mut eng, &[make_note_on(60)]);

            let mut op_c_ref = FmOperator::new();
            let base_hz = note_to_hz(60, 0.0);
            let c_freq = base_hz * RATIO_TABLE[RATIO_DEFAULT_INDEX as usize]; // ratio_c default = 1.0
            let velocity_scale = 32768.0_f32 / 65535.0; // matches make_note_on's fixed velocity
            // Skip the AMP attack ramp in the *assertion* — `op_c_ref` has
            // no envelope of its own, so it can only stand in for samples
            // where `env_amp` has already settled at a constant 1.0.
            // `tick()` still runs for every sample (not just the asserted
            // ones), so `op_c_ref`'s phase stays aligned with `eng`'s op_c.
            for (i, &got) in out.iter().enumerate() {
                let want = op_c_ref.tick(c_freq, 0.0, 0.0, 0.0, 44100.0) * velocity_scale;
                if i < 64 {
                    continue;
                }
                assert!((got - want).abs() < 1e-4, "{algo:?} sample {i}: got {got}, expected {want}");
            }
        }
    }

    // ── P14-C4: AMP page, NoteOff, mono-stack wiring ────────────────────

    /// A held note's AMP envelope must reach Sustain and stay there
    /// indefinitely — it must never decay or move on its own without a
    /// `NoteOff`. This is C0's `AdsrState` foundation exercised end-to-end
    /// through a real note for the first time.
    #[test]
    fn held_note_amp_sustain_never_decays_on_its_own() {
        let mut eng = new_engine();
        eng.bank.set(fp("attack"), 0.001);
        eng.bank.set(fp("decay"), 0.05);
        eng.bank.set(fp("sustain"), 0.6);
        run(&mut eng, &[make_note_on(60)]);
        for _ in 0..20 {
            run(&mut eng, &[]); // no NoteOff — the note stays held
        }
        assert!(
            matches!(eng.env_amp.phase, AdsrPhase::Sustain),
            "expected AMP envelope to reach and hold Sustain"
        );
        assert!(
            (eng.env_amp.value - 0.6).abs() < 1e-2,
            "expected AMP envelope to hold at sustain=0.6, got {}",
            eng.env_amp.value
        );
    }

    /// Release must not begin until `NoteOff` actually arrives — not
    /// before, and not automatically after some fixed time.
    #[test]
    fn release_starts_exactly_at_note_off_not_before() {
        let mut eng = new_engine();
        eng.bank.set(fp("attack"), 0.001);
        eng.bank.set(fp("decay"), 0.05);
        eng.bank.set(fp("sustain"), 0.6);
        eng.bank.set(fp("release"), 0.05);
        run(&mut eng, &[make_note_on(60)]);
        for _ in 0..20 {
            run(&mut eng, &[]);
        }
        assert!(
            matches!(eng.env_amp.phase, AdsrPhase::Sustain),
            "must still be sustaining before NoteOff"
        );
        let level_before_note_off = eng.env_amp.value;

        run(&mut eng, &[make_note_off(60)]);
        assert!(
            matches!(eng.env_amp.phase, AdsrPhase::Release),
            "expected Release to begin exactly at NoteOff"
        );
        assert!(
            eng.env_amp.value < level_before_note_off,
            "expected the envelope to have started decaying by the end of the NoteOff block"
        );

        // AdsrState's Release->Idle threshold (1e-5) needs ~1.6x the
        // release time constant, not 1x (0.001^k < 1e-5/start needs k>1.6)
        // — release=0.05s * 1.6 ≈ 88ms ≈ 4 blocks; 20 is a comfortable margin.
        for _ in 0..20 {
            run(&mut eng, &[]);
        }
        assert!(matches!(eng.env_amp.phase, AdsrPhase::Idle), "expected the voice to go idle once Release completes");
        assert!(!eng.is_sounding(), "expected is_sounding() to go false once Release completes and no note is held");
    }

    /// `NoteOff` emptying the note stack must let Release play out, and the
    /// voice must actually go idle (not stay stuck silently "sounding")
    /// once it completes — the render-gating redesign's actual contract.
    #[test]
    fn note_off_emptying_the_stack_gates_render_span_correctly() {
        let mut eng = new_engine();
        eng.bank.set(fp("attack"), 0.001);
        eng.bank.set(fp("release"), 0.05); // short, so this test doesn't need many blocks
        run(&mut eng, &[make_note_on(60)]);
        assert!(eng.is_sounding(), "must be sounding right after NoteOn");

        run(&mut eng, &[make_note_off(60)]);
        assert!(eng.is_sounding(), "must still be sounding during the Release tail");

        for _ in 0..20 {
            run(&mut eng, &[]);
        }
        assert!(!eng.is_sounding(), "must stop sounding once Release fully completes");
        let out = run(&mut eng, &[]);
        assert!(out.iter().all(|&s| s == 0.0), "must render pure silence once idle, not stale non-zero samples");
    }

    /// The actual mechanism `legato_retarget` (a `NoteOff` revealing a
    /// still-held note on the mono note stack) must not use — level
    /// continuity is the whole point of this path existing (unlike
    /// `trigger_note(.., hard_reset: true)`, which deliberately snaps to
    /// zero). Tests the private method directly rather than inferring it
    /// from a full block of audio, since a full block's *end* state
    /// converges to the same Sustain level regardless of whether the
    /// transient snapped or not — only the instant of the call itself
    /// distinguishes them.
    #[test]
    fn legato_retarget_preserves_amp_envelope_level_no_snap() {
        let mut eng = new_engine();
        eng.env_amp.trigger(true);
        for _ in 0..5 {
            eng.env_amp.tick(0.01, 0.999, 0.5, 0.999); // partway into Attack, not settled
        }
        let level_before = eng.env_amp.value;
        assert!(
            level_before > 0.0 && level_before < 1.0,
            "expected a partial, non-settled level to test continuity against, got {level_before}"
        );

        eng.legato_retarget(72);

        assert_eq!(eng.env_amp.value, level_before, "legato retarget must not touch the envelope's level");
        assert!(
            matches!(eng.env_amp.phase, AdsrPhase::Attack),
            "legato retarget re-enters Attack per AdsrState::trigger(false)'s contract"
        );
        assert_eq!(eng.current_note, 72, "must have retargeted to the new note");
    }

    /// `legato_retarget`'s entire reason to exist is *not* doing what
    /// `retrigger` does to the oscillator core — assert that directly,
    /// rather than trusting the `if reset_operators` guard by inspection. A
    /// mutation that made the operator-reset block in `trigger_note`
    /// unconditional would pass every other test in this file (they all
    /// converge to the same steady state by the end of a full block,
    /// hard-reset or not) but must be caught here. This is permanent and
    /// does not vary with `env_reset` (P14-C5) — a stack-pop never clicks,
    /// retrig or legato.
    ///
    /// `env_a`/`env_b` DO re-enter `Attack` on a legato retarget (P14-C5
    /// resolved C4's "open question": all three envelopes — `env_a`,
    /// `env_b`, `env_amp` — share one `env_reset` policy) — the same way
    /// `env_amp` already does in `legato_retarget_preserves_amp_envelope_
    /// level_no_snap` below. What must hold under legato (`env_reset=0`,
    /// the default) is level continuity, no snap — not that the phase is
    /// untouched.
    #[test]
    fn legato_retarget_does_not_touch_operator_phase() {
        let mut eng = new_engine();
        run(&mut eng, &[make_note_on(60)]);
        run(&mut eng, &[]); // advance operators off their post-trigger reset values

        let (c, a, b1, b2) = (eng.op_c.phase, eng.op_a.phase, eng.op_b1.phase, eng.op_b2.phase);
        assert!(c != 0.0 || a != 0.0 || b1 != 0.0 || b2 != 0.0, "operators must have advanced by now");

        eng.legato_retarget(64);

        assert_eq!(eng.op_c.phase, c, "legato retarget must not reset op_c's phase");
        assert_eq!(eng.op_a.phase, a, "legato retarget must not reset op_a's phase");
        assert_eq!(eng.op_b1.phase, b1, "legato retarget must not reset op_b1's phase");
        assert_eq!(eng.op_b2.phase, b2, "legato retarget must not reset op_b2's phase");
    }

    /// P14-C5's `env_reset=0` (legato, the default): `env_a`/`env_b` must
    /// preserve their level across a stack-pop, same continuity contract as
    /// `env_amp`'s own legato test below — no snap, even though the phase
    /// itself moves back to `Attack` (that's `AdsrState::trigger(false)`'s
    /// documented contract, not a bug).
    #[test]
    fn legato_retarget_preserves_mod_envelope_levels_no_snap() {
        let mut eng = new_engine();
        eng.env_a.trigger(true);
        eng.env_b.trigger(true);
        for _ in 0..5 {
            eng.env_a.tick(0.01, 0.999, 0.5, 0.999);
            eng.env_b.tick(0.01, 0.999, 0.4, 0.999);
        }
        let (level_a, level_b) = (eng.env_a.value, eng.env_b.value);
        assert!(level_a > 0.0 && level_a < 1.0, "expected a partial level to test continuity against");
        assert!(level_b > 0.0 && level_b < 1.0, "expected a partial level to test continuity against");

        eng.legato_retarget(64); // env_reset defaults to 0.0 (legato)

        assert_eq!(eng.env_a.value, level_a, "legato retarget must not snap env_a's level");
        assert_eq!(eng.env_b.value, level_b, "legato retarget must not snap env_b's level");
        assert!(matches!(eng.env_a.phase, AdsrPhase::Attack));
        assert!(matches!(eng.env_b.phase, AdsrPhase::Attack));
    }

    /// P14-C5's `env_reset=1` (retrig): a stack-pop now snaps all three
    /// envelopes to zero, unlike the legato default — while still never
    /// touching operator phase (traced in the test above's companion
    /// assertions, reconfirmed here for the retrig path specifically).
    #[test]
    fn env_reset_retrig_snaps_envelopes_to_zero_but_not_operator_phase() {
        let mut eng = new_engine();
        eng.bank.set(fp("env_reset"), 1.0);
        run(&mut eng, &[make_note_on(60)]);
        run(&mut eng, &[]); // let everything settle away from zero

        let (c, a, b1, b2) = (eng.op_c.phase, eng.op_a.phase, eng.op_b1.phase, eng.op_b2.phase);
        assert!(eng.env_amp.value > 0.0, "expected env_amp to have risen off zero by now");

        eng.legato_retarget(64);

        assert_eq!(eng.env_a.value, 0.0, "env_reset=1 must snap env_a to zero");
        assert_eq!(eng.env_b.value, 0.0, "env_reset=1 must snap env_b to zero");
        assert_eq!(eng.env_amp.value, 0.0, "env_reset=1 must snap env_amp to zero");
        assert_eq!(eng.op_c.phase, c, "operator phase must still be untouched under env_reset=1");
        assert_eq!(eng.op_a.phase, a, "operator phase must still be untouched under env_reset=1");
        assert_eq!(eng.op_b1.phase, b1, "operator phase must still be untouched under env_reset=1");
        assert_eq!(eng.op_b2.phase, b2, "operator phase must still be untouched under env_reset=1");
    }

    /// The other half of the same guard: a genuine `NoteOn` (`retrigger`)
    /// DOES reset every operator's phase to zero — confirming the two paths
    /// actually diverge, not just that `legato_retarget` is inert. Calls
    /// `retrigger` directly (like the `legato_retarget_preserves_amp_
    /// envelope_level_no_snap` test above calls `legato_retarget` directly)
    /// rather than through a full `run()` block: a block rendered after the
    /// reset immediately advances the phase away from zero again, so
    /// observing the reset itself requires checking right after the call,
    /// not after a full block of subsequent rendering.
    #[test]
    fn retrigger_resets_every_operator_phase_to_zero() {
        let mut eng = new_engine();
        run(&mut eng, &[make_note_on(60)]);
        run(&mut eng, &[]); // advance operators off zero
        assert!(eng.op_c.phase != 0.0, "expected the phase to have advanced before the second trigger");

        eng.retrigger(64, 1.0); // a second NoteOn while the first is still notionally held

        assert_eq!(eng.op_c.phase, 0.0, "a fresh NoteOn must hard-reset op_c's phase");
        assert_eq!(eng.op_a.phase, 0.0, "a fresh NoteOn must hard-reset op_a's phase");
        assert_eq!(eng.op_b1.phase, 0.0, "a fresh NoteOn must hard-reset op_b1's phase");
        assert_eq!(eng.op_b2.phase, 0.0, "a fresh NoteOn must hard-reset op_b2's phase");
    }

    /// End-to-end sanity for the actual `NoteOff` dispatch path (not just
    /// the `legato_retarget` mechanism in isolation): holding two notes and
    /// releasing the top must retarget to the bottom note, not go silent.
    #[test]
    fn releasing_the_top_of_a_held_pair_retargets_to_the_one_underneath() {
        let mut eng = new_engine();
        eng.bank.set(fp("release"), 0.05); // fast, so the final idle check needs few blocks
        run(&mut eng, &[make_note_on(60)]);
        run(&mut eng, &[make_note_on(64)]); // overlapping press, hard-reset per this commit's design
        assert_eq!(eng.current_note, 64);

        run(&mut eng, &[make_note_off(64)]);
        assert_eq!(eng.current_note, 60, "releasing the top must reveal the note underneath");
        assert!(eng.is_sounding(), "must still be sounding — the revealed note is still held");

        let out = run(&mut eng, &[]);
        assert!(out.iter().any(|&s| s.abs() > 1e-3), "must still be audible under the revealed note");

        run(&mut eng, &[make_note_off(60)]);
        for _ in 0..20 {
            run(&mut eng, &[]);
        }
        assert!(!eng.is_sounding(), "must go silent once the last held note is released and Release completes");
    }

    // ── P14-C5: expression page + portamento ────────────────────────────

    /// `porta=0` (the default) is the hard regression requirement this
    /// commit's spec names: a stack-pop must never create a glide, and the
    /// sounding frequency must retune to the new note exactly, not
    /// partway. Measured via a single-sample `process_algo` call (calling
    /// the private method directly, same module) rather than a full
    /// 512-sample block: at note 60's frequency (~261.6 Hz), a block spans
    /// several full phase wraps, which a two-point before/after phase
    /// comparison can't safely unwrap — one sample's increment is well
    /// under a full cycle, so no unwrapping ambiguity exists.
    #[test]
    fn porta_zero_creates_no_glide_and_retunes_instantly() {
        let mut eng = new_engine();
        run(&mut eng, &[make_note_on(60)]);
        run(&mut eng, &[make_note_on(64)]); // overlapping press, hard reset

        run(&mut eng, &[make_note_off(64)]); // reveals 60; porta=0 by default
        assert_eq!(eng.current_note, 60);
        assert!(eng.glide.is_none(), "porta=0 must never create a glide");

        let phase_before = eng.op_c.phase;
        eng.process_algo(0, 1);
        let advanced = eng.op_c.phase - phase_before;
        let expected_per_sample = note_to_hz(60, 0.0) / 44100.0;
        assert!(
            (advanced - expected_per_sample).abs() < 1e-6,
            "expected an instant retune to note 60's frequency, got increment {advanced}, \
             expected {expected_per_sample}"
        );
    }

    /// A second stack-pop arriving before the first glide finishes (a
    /// 3-note chord released top-down faster than `porta` — Pad's own D5
    /// recipe is exactly this: "slow attack, porta on", played as held
    /// chords with staggered releases) must restart from the *live*
    /// interpolated pitch, not from the first glide's nominal target — found
    /// in C5 review: `legato_retarget` originally read
    /// `note_to_hz(self.current_note, ...)` unconditionally, which is only
    /// correct when no glide is already in flight; recomputing the nominal
    /// target while a glide is live would itself be the audible pitch-snap
    /// this whole mechanism exists to avoid.
    #[test]
    fn overlapping_glide_restarts_from_the_live_interpolated_pitch_not_the_nominal_target() {
        let mut eng = new_engine();
        eng.bank.set(fp("porta"), 0.5); // long enough to reliably catch mid-flight
        run(&mut eng, &[make_note_on(67)]);
        run(&mut eng, &[make_note_on(64)]); // overlapping presses build the stack: 67, 64
        run(&mut eng, &[make_note_on(60)]); // ... then 60 on top

        run(&mut eng, &[make_note_off(60)]); // reveals 64, starts a glide 60 -> 64
        for _ in 0..8 {
            run(&mut eng, &[]); // let it run partway, well short of porta=0.5s
        }
        let g = eng.glide.expect("glide toward 64 should still be in flight");
        let t = (g.elapsed_secs / g.total_secs).min(1.0);
        assert!(t > 0.05 && t < 0.95, "expected to catch the glide mid-flight, t={t}");
        let live_hz = eng.current_sounding_hz();
        assert!(
            (live_hz - note_to_hz(60, 0.0)).abs() > 0.5 && (live_hz - note_to_hz(64, 0.0)).abs() > 0.5,
            "expected to be genuinely between 60 and 64's frequencies, got {live_hz}"
        );

        run(&mut eng, &[make_note_off(64)]); // reveals 67 — must restart from `live_hz`, not note_to_hz(64)

        let new_glide = eng.glide.expect("second stack-pop should start a new glide toward 67");
        assert!(
            (new_glide.from_hz - live_hz).abs() < 1.0,
            "the new glide must start from the live sounding pitch ({live_hz}), not a nominal one, got {}",
            new_glide.from_hz
        );
    }

    /// Nonzero `porta`: the sounding frequency must glide from the
    /// outgoing note toward the new one, completing only once `porta`
    /// seconds have actually elapsed — not immediately, and not much
    /// later either.
    #[test]
    fn nonzero_porta_glides_to_target_within_porta_seconds() {
        let mut eng = new_engine();
        eng.bank.set(fp("porta"), 0.1); // 100ms glide
        run(&mut eng, &[make_note_on(60)]);
        run(&mut eng, &[make_note_on(72)]); // fresh NoteOn — hard reset, never glides

        run(&mut eng, &[make_note_off(72)]); // reveals 60, starts a glide toward it
        assert_eq!(eng.current_note, 60);
        let glide = eng.glide.expect("porta>0 stack-pop must start a glide");
        assert!(
            (glide.from_hz - note_to_hz(72, 0.0)).abs() < 1.0,
            "glide must start from the outgoing note's (72's) frequency, got {}",
            glide.from_hz
        );

        // ~11.6ms/block at 44100Hz/512 samples — 0.1s is ~8.6 blocks away;
        // one more block is nowhere near enough to have finished gliding.
        run(&mut eng, &[]);
        assert!(eng.glide.is_some(), "glide must still be in flight well before porta has elapsed");

        // 15 blocks ≈ 174ms, comfortably past 0.1s + a block's margin.
        for _ in 0..15 {
            run(&mut eng, &[]);
        }
        assert!(eng.glide.is_none(), "glide must have completed within porta seconds (+ margin)");
    }

    /// `keytrack`/`vel_mod` at both range extremes must not destabilize
    /// output — same non-finite/non-panic bar every other continuous P14
    /// param has been held to (`harm`/`ratio_*` in C2/C3). A high note
    /// (96) stresses `keytrack`'s note-distance shift the hardest.
    #[test]
    fn keytrack_and_vel_mod_extremes_are_reachable() {
        for &(param, val) in &[("keytrack", 0.0), ("keytrack", 1.0), ("vel_mod", 0.0), ("vel_mod", 1.0)] {
            let mut eng = new_engine();
            eng.bank.set(fp(param), val);
            let out = run(&mut eng, &[make_note_on(96)]);
            assert!(out.iter().all(|s| s.is_finite()), "{param}={val} produced a non-finite sample");
        }
    }

    /// `keytrack=1`'s note-distance shift is only designed around a ±4-octave
    /// range (§0's `keytrack` formula) — the true MIDI extremes (0 and 127)
    /// sit well outside that, which is exactly where an unclamped shift
    /// would push `harm` out of `harmonic_sine`'s caller-guaranteed
    /// `0.0..=1.0` domain (a P14-C1 finding). Confirms the final clamp holds
    /// at the actual extremes, not just the design-assumption range.
    #[test]
    fn keytrack_extreme_shift_stays_within_harm_domain_at_true_midi_extremes() {
        for note in [0u8, 127u8] {
            let mut eng = new_engine();
            eng.bank.set(fp("keytrack"), 1.0);
            eng.bank.set(fp("harm"), 1.0); // pre-shift ceiling — the case most likely to overflow
            let out = run(&mut eng, &[make_note_on(note)]);
            assert!(out.iter().all(|s| s.is_finite()), "note {note} produced a non-finite sample");
        }
    }

    #[test]
    fn apply_drive_zero_is_exact_identity() {
        for x in [-1.0f32, -0.3, 0.0, 0.3, 1.0] {
            assert_eq!(apply_drive(x, 0.0), x);
            assert_eq!(apply_drive(x, -1.0), x, "any non-positive drive must bypass, not just exactly 0.0");
        }
    }

    #[test]
    fn apply_drive_nonzero_actually_changes_the_signal() {
        let x = 0.8f32;
        assert_ne!(apply_drive(x, 0.5), x);
    }

    #[test]
    fn apply_tone_zero_is_exact_identity() {
        let mut eng = new_engine();
        for x in [-1.0f32, -0.3, 0.0, 0.3, 1.0] {
            assert_eq!(eng.apply_tone(x, 0.0), x);
        }
    }

    #[test]
    fn apply_tone_nonzero_actually_changes_the_signal() {
        let mut eng = new_engine();
        // Prime the filter state with some non-constant signal first, so
        // the tilt has something to act on.
        for i in 0..10 {
            eng.apply_tone((i as f32 * 0.37).sin(), 0.0);
        }
        let bright = eng.apply_tone(0.8, 0.6);
        let dark = eng.apply_tone(0.8, -0.6);
        assert_ne!(bright, 0.8);
        assert_ne!(dark, 0.8);
        assert_ne!(bright, dark, "tone>0 and tone<0 must tilt in different directions");
    }

    // ── MOD page / LFO dest table (P14-C6) ──────────────────────────────

    #[test]
    fn dest_table_pins_its_head_append_only() {
        assert_eq!(
            &FM_VOICE_DEST_NAMES[..7],
            &["ratio_c", "ratio_a", "ratio_b", "harm", "detune", "feedback", "mix"],
            "dest table head is append-only — a reorder re-points every saved patch"
        );
    }

    /// `algo`/`env_reset` are topology/behavior selectors, not modulation
    /// targets (same reasoning `FmEngine` excludes its own `machine`); the
    /// seven `lfo_*` params are the LFO's own controls, modulating one of
    /// them would be circular.
    #[test]
    fn dest_table_excludes_algo_env_reset_and_lfo_params() {
        for excluded in [
            "algo", "env_reset", "lfo_dest", "lfo_depth", "lfo_shape",
            "lfo_speed", "lfo_mode", "lfo_start_phase", "lfo_fade",
        ] {
            assert!(!FM_VOICE_DEST_NAMES.contains(&excluded), "{excluded} must not be a modulation dest");
        }
    }

    #[test]
    fn dest_names_ids_ranges_and_labels_stay_in_sync_for_full() {
        assert_eq!(FmVoice::dest_names(Machine::Full), FM_VOICE_DEST_NAMES);
        assert_eq!(FmVoice::dest_ids(Machine::Full), FM_VOICE_DEST_IDS);
        assert_eq!(FmVoice::dest_labels(Machine::Full).0, FM_VOICE_DEST_NAMES);
        assert_eq!(FM_VOICE_DEST_NAMES.len(), FM_VOICE_DEST_IDS.len());
        assert_eq!(FM_VOICE_DEST_NAMES.len(), FM_VOICE_DEST_RANGES.len());

        // The three checks above only confirm the *lengths* line up and that
        // the accessor functions return the exact arrays they're hard-coded
        // to return — neither can catch one of the three parallel tables
        // being silently reordered relative to the other two (e.g. two
        // adjacent `FM_VOICE_DEST_RANGES` entries swapped), which would
        // apply the wrong destination's scale/clamp range while still
        // looking internally consistent by length alone. Per-position
        // correspondence, matching `FmEngine`'s own
        // `the_dest_ids_and_names_correspond` test, is what actually pins
        // the three tables together.
        for (i, name) in FM_VOICE_DEST_NAMES.iter().enumerate() {
            assert_eq!(fp(name), FM_VOICE_DEST_IDS[i], "name/id mismatch at position {i} ({name})");
            assert_eq!(
                FM_VOICE_DEST_RANGES[i].0, FM_VOICE_DEST_IDS[i],
                "range/id mismatch at position {i} ({name})"
            );
        }
    }

    /// Every declared destination must be reachable — modulating it must
    /// not panic or produce non-finite output. `Full` is the only machine
    /// with real behavior yet (P14-C7 adds the six macros' own tables).
    #[test]
    fn every_dest_is_reachable_without_panicking_or_non_finite_output() {
        for (idx, name) in FM_VOICE_DEST_NAMES.iter().enumerate() {
            let mut eng = new_engine();
            eng.bank.set(fp("lfo_dest"), (idx + 1) as f64); // one-based
            eng.bank.set(fp("lfo_depth"), 1.0);
            eng.bank.set(fp("lfo_speed"), 5.0);
            let out = run(&mut eng, &[make_note_on(60)]);
            assert!(out.iter().all(|s| s.is_finite()), "dest {idx} ({name}) produced a non-finite sample");
        }
    }

    /// Depth/dest sanity (ADR-042 amendment 2 vs body, resolved the same way
    /// MM-C9 resolved it for `FmEngine`: one-based index into the active
    /// machine's table). Targeting `harm` must measurably shift `harm`'s
    /// `get_param` reading away from its raw value, and must NOT leak into
    /// any other param's reading.
    #[test]
    fn lfo_depth_and_dest_only_affect_the_named_param() {
        let mut eng = new_engine();
        let harm_index = FM_VOICE_DEST_NAMES.iter().position(|&n| n == "harm").unwrap() + 1;
        eng.bank.set(fp("lfo_dest"), harm_index as f64);
        eng.bank.set(fp("lfo_depth"), 1.0);
        eng.bank.set(fp("lfo_speed"), 5.0);
        run(&mut eng, &[make_note_on(60)]);

        let harm_raw = eng.raw_param(fp("harm"));
        let harm_mod = eng.get_param(fp("harm"));
        assert!(
            (harm_mod - harm_raw).abs() > 1e-3,
            "expected the LFO to measurably shift harm, got raw={harm_raw} mod={harm_mod}"
        );

        for other in ["feedback", "mix", "attack", "volume"] {
            let raw = eng.raw_param(fp(other));
            let modulated = eng.get_param(fp(other));
            assert_eq!(modulated, raw, "{other} must be unaffected when lfo_dest targets harm");
        }
    }

    /// `current_sounding_hz` must reflect `detune`'s LFO-modulated value,
    /// not the raw one — the P14-C6 review finding: an earlier draft read
    /// `detune` via `raw_param` here (on the mistaken theory that it
    /// mirrored `FmEngine::retrigger`'s unrelated `#175` precedent), which
    /// would make a portamento glide's captured start point differ from
    /// what `process_algo` had just actually rendered by up to the LFO's
    /// full swing — an audible snap at glide onset whenever `detune` is an
    /// LFO destination (Pad's own D5 recipe turns `porta` on, and `detune`
    /// is one of the 26 declared destinations).
    #[test]
    fn current_sounding_hz_reflects_lfo_modulated_detune_not_raw() {
        let mut eng = new_engine();
        let detune_index = FM_VOICE_DEST_NAMES.iter().position(|&n| n == "detune").unwrap() + 1;
        eng.bank.set(fp("lfo_dest"), detune_index as f64);
        eng.bank.set(fp("lfo_depth"), 1.0);
        eng.bank.set(fp("lfo_speed"), 5.0);
        run(&mut eng, &[make_note_on(60)]);
        run(&mut eng, &[]); // advance the LFO off its trigger-time reset point

        let raw_detune = eng.raw_param(fp("detune"));
        let modulated_detune = eng.get_param(fp("detune"));
        assert!(
            (modulated_detune - raw_detune).abs() > 1e-3,
            "expected the LFO to actually be modulating detune by now, raw={raw_detune} mod={modulated_detune}"
        );

        let expected_hz = note_to_hz(eng.current_note, modulated_detune / 100.0);
        assert!(
            (eng.current_sounding_hz() - expected_hz).abs() < 1e-3,
            "current_sounding_hz must reflect the LFO-modulated detune ({expected_hz} Hz), \
             not the raw value, got {}",
            eng.current_sounding_hz()
        );
    }

    /// `lfo_dest=0` (off) must leave every param exactly at its raw value —
    /// the default, and the regression `apply`'s own short-circuit exists
    /// to guarantee.
    #[test]
    fn lfo_dest_off_is_a_no_op_on_every_param() {
        let mut eng = new_engine();
        eng.bank.set(fp("lfo_depth"), 1.0);
        eng.bank.set(fp("lfo_speed"), 5.0);
        run(&mut eng, &[make_note_on(60)]);
        for name in FM_VOICE_DEST_NAMES {
            let raw = eng.raw_param(fp(name));
            let modulated = eng.get_param(fp(name));
            assert_eq!(modulated, raw, "{name} must be untouched when lfo_dest is off");
        }
    }
}
