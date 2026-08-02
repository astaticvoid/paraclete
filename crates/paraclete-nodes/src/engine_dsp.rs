//! Shared DSP primitives for AnalogEngine and FmEngine.
//! Private to paraclete-nodes — not exported.
//!
//! Was an outer `///` doc, which attached the module's own description to
//! whatever item happened to come first. MM-C7 put a `const` there and clippy
//! noticed; `//!` is what a module doc should have been all along.

use paraclete_node_api::{ParamDescriptor, ParamDisplay, ParamDisplayAdapter, ParamUnit};

// ── Sub-block rate ────────────────────────────────────────────────────────────

/// Control-rate chunk, in samples (MM §0 D2, ADR-042 amendment 4).
///
/// A machine's `process_*` reads every param once and derives its filter and
/// envelope coefficients before the sample loop, which is exactly why an LFO
/// cannot simply exist: nothing re-reads a modulated param inside a span.
/// `render_span` chunks each span into pieces of at most this many samples and
/// calls `process_*` per chunk, so a later commit's LFO has somewhere to land.
///
/// **Boundaries are measured from the span start, not from the block start.**
/// A span boundary is already a discontinuity — a note started there — so
/// aligning to it costs nothing and keeps absolute block offsets out of
/// `render_span`. The cost is that update instants are not sample-aligned
/// across spans within one block: inaudible at 64 samples (1.45 ms at
/// 44.1 kHz), and ADR-042 already accepted control-rate modulation.
///
/// Nothing modulates yet, so chunking must be **output-identical** — the
/// coefficients recomputed per chunk come from the same constant params. The
/// four ADR-035 baselines are what holds that claim.
pub(crate) const LFO_SUB_BLOCK: usize = 64;

/// `[start, end)` cut into `LFO_SUB_BLOCK` pieces, measured from `start`.
///
/// A span of 100 renders as 64 + 36 whether it begins at 0 or at 411. Empty
/// and inverted spans yield nothing, so a caller needs no `start < end` guard
/// of its own.
///
/// Borrows nothing, so a caller can drive it while holding `&mut self`.
pub(crate) fn sub_blocks(start: usize, end: usize) -> impl Iterator<Item = (usize, usize)> {
    let mut lo = start;
    std::iter::from_fn(move || {
        if lo >= end {
            return None;
        }
        let hi = (lo + LFO_SUB_BLOCK).min(end);
        let span = (lo, hi);
        lo = hi;
        Some(span)
    })
}

// ── AdState ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub(crate) enum AdPhase { Idle, Attack, Decay }

#[derive(Clone, Copy)]
pub(crate) struct AdState {
    pub phase: AdPhase,
    pub value: f32,
}

impl AdState {
    pub fn new() -> Self { AdState { phase: AdPhase::Idle, value: 0.0 } }

    pub fn trigger(&mut self) {
        self.phase = AdPhase::Attack;
        // Retrigger from current value — prevents click on rapid retriggering.
    }

    pub fn tick(&mut self, attack_inc: f32, decay_coeff: f32) -> f32 {
        match self.phase {
            AdPhase::Idle => 0.0,
            AdPhase::Attack => {
                self.value += attack_inc;
                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.phase = AdPhase::Decay;
                }
                self.value
            }
            AdPhase::Decay => {
                self.value *= decay_coeff;
                if self.value < 1.0e-5 {
                    self.value = 0.0;
                    self.phase = AdPhase::Idle;
                }
                self.value
            }
        }
    }

    pub fn is_idle(&self) -> bool { matches!(self.phase, AdPhase::Idle) }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// XOR-shift 32 LFSR white noise generator. Period = 2^32 - 1.
#[inline(always)]
pub(crate) fn xorshift(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as i32 as f32) / (i32::MAX as f32)
}

/// MIDI note + semitone offset → frequency in Hz. Note 69 = A4 = 440 Hz.
#[inline(always)]
pub(crate) fn note_to_hz(note: u8, tune_semitones: f32) -> f32 {
    440.0 * 2.0f32.powf((note as f32 - 69.0 + tune_semitones) / 12.0)
}

/// Soft-clip via tanh, bounded to -1.0..+1.0.
#[inline(always)]
pub(crate) fn soft_clip(x: f32) -> f32 { x.tanh() }

/// Single-sample Chamberlin SVF low-pass section.
#[inline(always)]
pub(crate) fn svf_lp_sample(
    input: f32, f: f32, q: f32,
    state_low: &mut f32, state_band: &mut f32,
) -> f32 {
    *state_low  += f * *state_band;
    *state_band += f * (input - *state_low - q * *state_band);
    *state_low
}

// ── LfoBlock (ADR-042 decision 1) ─────────────────────────────────────────────

// MM-C8 lands `LfoBlock` **unhosted**, by design — the spec makes it a pure,
// engine-free commit so its shapes and modes are pinned before anything
// depends on them. MM-C9 is its first consumer and these allows come off
// then; if they are still here after MM-C9, something did not get wired.

/// Waveform of an `LfoBlock`. **Append-only** — the `lfo_shape` param stores
/// the numeric value, so reordering silently re-points every saved patch at a
/// different wave. Same contract as `AnalogMachine::ALL`.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LfoShape { Tri, Sine, Sqr, Saw, Exp, Ramp, Rand }

#[allow(dead_code)]
impl LfoShape {
    pub const ALL: [LfoShape; 7] = [
        LfoShape::Tri, LfoShape::Sine, LfoShape::Sqr,
        LfoShape::Saw, LfoShape::Exp, LfoShape::Ramp, LfoShape::Rand,
    ];

    /// Out-of-range clamps to the last shape rather than panicking — this
    /// reads an `f64` bank slot a malformed project could carry, the same way
    /// `AnalogMachine::from_value` does.
    pub fn from_value(v: f32) -> Self {
        if !v.is_finite() || v < 0.0 {
            return LfoShape::Tri;
        }
        *Self::ALL.get(v as usize).unwrap_or(&LfoShape::Rand)
    }
}

/// Retrigger behaviour. **Append-only**, for the same reason as `LfoShape`.
///
/// - `Free` — phase never resets; a note does not disturb it.
/// - `Trig` — phase resets to `start_phase` on every note.
/// - `Hold` — the LFO keeps running, but the *output* is sampled once at the
///   note and held until the next one (sample-and-hold).
/// - `One` — resets, runs exactly one cycle, then holds its final value.
/// - `Half` — resets, runs half a cycle, then holds.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LfoMode { Free, Trig, Hold, One, Half }

#[allow(dead_code)]
impl LfoMode {
    pub const ALL: [LfoMode; 5] = [
        LfoMode::Free, LfoMode::Trig, LfoMode::Hold, LfoMode::One, LfoMode::Half,
    ];

    pub fn from_value(v: f32) -> Self {
        if !v.is_finite() || v < 0.0 {
            return LfoMode::Free;
        }
        *Self::ALL.get(v as usize).unwrap_or(&LfoMode::Half)
    }
}

/// MM §0 D6: `lfo_fade` is a *fraction of this* — `fade = 1.0` is a four
/// second fade-in, `-0.5` a two second fade-out, `0.0` no fade.
///
/// ADR-042 gives `lfo_fade` the range −1…+1 and the meaning "fade-in (+) /
/// fade-out (−) on trig", but no time. The reference hardware measures it in
/// sequencer steps; engines here receive no tempo (ADR-042 decision 5 stages
/// sync deliberately), so a musical unit is not available and seconds are the
/// boring choice. Four seconds spans roughly a bar and a half at 140 BPM,
/// which is the range a fade is useful over.
#[allow(dead_code)]
pub(crate) const LFO_FADE_MAX_SECS: f32 = 4.0;

/// Everything about an LFO that comes from the parameter bank. `Copy`, so a
/// host reads its params once per sub-block and passes them by value — no
/// allocation, nothing borrowed.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct LfoSettings {
    pub shape: LfoShape,
    pub mode: LfoMode,
    /// Hz. ADR-042 decision 1's 0.01–64 range; free-running in v1.
    pub speed_hz: f32,
    /// 0–1, applied by the resetting modes only.
    pub start_phase: f32,
    /// −1…+1. See [`LFO_FADE_MAX_SECS`].
    pub fade: f32,
}

/// One low-frequency oscillator, in the shape of `AdState`: a plain struct
/// with `trigger()`/`tick()`, no allocation, and unit-testable with no engine
/// around it.
///
/// **Depth is not here.** The host applies
/// `effective = clamp(get_param(dest) + depth × range × lfo(t))` (ADR-042
/// amendment 1), so this returns the raw −1…+1 wave with its fade applied and
/// knows nothing about a destination.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct LfoBlock {
    /// 0–1, wrapping.
    phase: f32,
    /// Cycles travelled since the last trigger — what `One` and `Half` stop
    /// on. Counting cycles rather than watching for a phase wrap is what makes
    /// a start phase of 0.9 still run a *whole* cycle in `One`.
    travelled: f32,
    /// Seconds since the last trigger, for the fade.
    elapsed: f32,
    /// `Hold`'s captured value, and `Rand`'s current step.
    sampled: f32,
    rng: u32,
    /// False once `One`/`Half` have finished; they hold their last value.
    running: bool,
}

#[allow(dead_code)]
impl LfoBlock {
    pub fn new() -> Self {
        LfoBlock {
            phase: 0.0,
            travelled: 0.0,
            elapsed: 0.0,
            sampled: 0.0,
            // Any non-zero seed; xorshift is degenerate from 0.
            rng: 0x2545_F491,
            running: true,
        }
    }

    /// A note arrived. Every mode restarts the fade, since the fade is
    /// measured from the note; only the resetting modes move the phase.
    ///
    /// **`Hold` does not reset either, and that is the whole point of it.**
    /// Sample-and-hold freezes the *free-running* LFO at the instant of the
    /// note; resetting first would sample `start_phase` every time and emit a
    /// constant. A test caught exactly that — with sine and a start phase of
    /// 0, every note sampled 0.0 forever.
    pub fn trigger(&mut self, s: LfoSettings) {
        self.elapsed = 0.0;
        self.travelled = 0.0;
        self.running = true;
        if matches!(s.mode, LfoMode::Trig | LfoMode::One | LfoMode::Half) {
            self.phase = s.start_phase.clamp(0.0, 1.0).fract();
        }
        if s.mode == LfoMode::Hold {
            self.sampled = raw_shape(s.shape, self.phase, &mut self.rng);
        }
    }

    /// Advance by `samples` and return the faded value in −1…+1.
    ///
    /// Called once per 64-sample sub-block (MM-C7's structure), not per
    /// sample: ADR-042 accepted control-rate modulation, and re-deriving a
    /// machine's coefficients per sample would cost far more than the LFO.
    pub fn tick(&mut self, s: LfoSettings, sample_rate: f32, samples: usize) -> f32 {
        if sample_rate <= 0.0 || samples == 0 {
            return self.faded(self.value_for(s), s);
        }
        let dt = samples as f32 / sample_rate;
        self.elapsed += dt;

        if self.running {
            let advance = s.speed_hz.max(0.0) * dt;
            let before = self.phase;
            self.phase = (self.phase + advance).fract();
            self.travelled += advance;

            // The `Rand` shape redraws once per cycle, at the wrap — unless
            // `Hold` mode owns `sampled`, in which case the note is the only
            // thing allowed to move it.
            if s.shape == LfoShape::Rand
                && s.mode != LfoMode::Hold
                && (self.phase < before || advance >= 1.0)
            {
                self.sampled = xorshift(&mut self.rng);
            }

            let limit = match s.mode {
                LfoMode::One => Some(1.0),
                LfoMode::Half => Some(0.5),
                _ => None,
            };
            if let Some(limit) = limit {
                if self.travelled >= limit {
                    self.travelled = limit;
                    // Land exactly on the stopping point rather than wherever
                    // the sub-block happened to overshoot to, so `half` really
                    // holds the half-cycle value.
                    self.phase = (s.start_phase.clamp(0.0, 1.0) + limit).fract();
                    self.running = false;
                }
            }
        }
        self.faded(self.value_for(s), s)
    }

    /// The un-faded wave value for the current state.
    fn value_for(&self, s: LfoSettings) -> f32 {
        match s.mode {
            // Hold freezes the output between notes; the phase still advances
            // so the next note samples somewhere new.
            LfoMode::Hold => self.sampled,
            _ => match s.shape {
                LfoShape::Rand => self.sampled,
                other => raw_shape_no_rng(other, self.phase),
            },
        }
    }

    /// Fade envelope: `+fade` ramps 0→1 over `fade * LFO_FADE_MAX_SECS`
    /// seconds from the trigger, `-fade` ramps 1→0 over the same, `0` is off.
    fn faded(&self, v: f32, s: LfoSettings) -> f32 {
        let f = s.fade.clamp(-1.0, 1.0);
        if f == 0.0 {
            return v;
        }
        let span = f.abs() * LFO_FADE_MAX_SECS;
        if span <= 0.0 {
            return v;
        }
        let t = (self.elapsed / span).clamp(0.0, 1.0);
        let gain = if f > 0.0 { t } else { 1.0 - t };
        v * gain
    }

    /// 0–1, for `/node/{id}/state/lfo_phase`.
    pub fn phase(&self) -> f32 {
        self.phase
    }
}

/// The seven `lfo_*` params, as a hosting node declares them (ADR-042
/// decision 1). One full encoder page.
///
/// `dest_table_len` is the host's own dest-table length — see
/// [`lfo_dest_param`] for why the range is the table's and not the doc's.
pub(crate) fn lfo_params(
    dest_table_len: usize,
    dest_labels: Option<&'static (dyn ParamDisplay + 'static)>,
) -> Vec<ParamDescriptor> {
    let p = |name: &'static str, min: f64, max: f64, default: f64, stepped: bool| {
        ParamDescriptor {
            id: ParamDescriptor::id_for_name(name),
            name: name.into(),
            min,
            max,
            default,
            stepped,
            unit: ParamUnit::Generic,
            display: None,
        }
    };
    vec![
        p("lfo_shape", 0.0, (LfoShape::ALL.len() - 1) as f64, 0.0, true),
        // 0.01-64 Hz. The exponential taper ADR-042 names is a *display*
        // concern: the stored value is linear Hz, so a p-lock or a scene
        // interpolates in Hz and a surface can curve the knob however it likes
        // without changing what is stored.
        p("lfo_speed", 0.01, 64.0, 1.0, false),
        p("lfo_mode", 0.0, (LfoMode::ALL.len() - 1) as f64, 0.0, true),
        p("lfo_start_phase", 0.0, 1.0, 0.0, false),
        p("lfo_fade", -1.0, 1.0, 0.0, false),
        lfo_dest_param(dest_table_len, dest_labels),
        p("lfo_depth", -1.0, 1.0, 0.0, false),
    ]
}

/// `lfo_dest`: `0` = off, `1..=N` = one-based index into the host's
/// **append-only dest table**.
///
/// ADR-042's body and its amendment 2 disagree here, and MM §1 freezes this
/// surface, so the reading is recorded rather than left implicit. The body
/// says "index into the node's declared params"; amendment 2 replaces the
/// storage with the target's name-hash id, *and* introduces the append-only
/// per-engine dest table.
///
/// Storing the **table index** is what landed (user decision, 2026-07-31).
/// Amendment 2's objection is that *declaration order* is unstable — but the
/// table it mandates is append-only and separate from declaration order, so an
/// index into it is exactly as stable as an id: appends never move existing
/// entries. Once the table exists, putting the id in the bank as well buys no
/// stability and costs the encoder, because a name-hash needs `0..u32::MAX`
/// and `ViewMetaParam::options` (MM-C5) is a *value-indexed* label array —
/// it cannot describe a param whose values are hashes. A dense `0..=N` maps
/// onto it exactly.
///
/// The stability this depends on is the table being append-only; every host
/// carries a test pinning its table's head so a reorder fails loudly (MM §0
/// D3) — two union tables and six per-machine tables across the two engine
/// families, plus the Sampler's and Filter's.
pub(crate) fn lfo_dest_param(
    dest_table_len: usize,
    labels: Option<&'static (dyn ParamDisplay + 'static)>,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamDescriptor::id_for_name("lfo_dest"),
        name: "lfo_dest".into(),
        min: 0.0,
        max: dest_table_len as f64,
        default: 0.0,
        stepped: true,
        unit: ParamUnit::Generic,
        display: labels.map(ParamDisplayAdapter::Static),
    }
}

/// MOD page order for the seven `lfo_*` params — slot `i` gets
/// `LFO_PAGE_ORDER[i]`. Shared so both engines lay the page out identically
/// and a performer's muscle memory carries across tracks.
pub(crate) const LFO_PAGE_ORDER: [u32; 7] = [
    ParamDescriptor::id_for_name("lfo_dest"),
    ParamDescriptor::id_for_name("lfo_depth"),
    ParamDescriptor::id_for_name("lfo_shape"),
    ParamDescriptor::id_for_name("lfo_speed"),
    ParamDescriptor::id_for_name("lfo_mode"),
    ParamDescriptor::id_for_name("lfo_start_phase"),
    ParamDescriptor::id_for_name("lfo_fade"),
];

/// Value labels for a host's `lfo_dest` encoder (MM §0 D4).
///
/// ADR-042 amendment 5 ruled out a *dynamic* descriptor display —
/// `ParamDisplayAdapter::Dynamic` panics on clone and the cap-doc path clones.
/// A **static** one has neither problem, and the labels here are known at
/// compile time: they are the names of the params in the host's dest table.
/// So the cap-doc carries them, which is what lets a surface label the encoder
/// without knowing anything about LFOs (D4's "built from the dest table plus
/// the cap-doc's param names", with the engine doing the joining once).
///
/// Index 0 is `off`; `1..=N` are the dest table's entries in order.
pub(crate) struct LfoDestLabels(pub &'static [&'static str]);

impl ParamDisplay for LfoDestLabels {
    fn format(&self, value: f64) -> String {
        if !value.is_finite() || value < 1.0 {
            return "off".to_string();
        }
        match self.0.get(value as usize - 1) {
            Some(name) => (*name).to_string(),
            // #179: past this machine's table. Empty, not "off" — the
            // descriptor's `max` is the *union* width (so the bank never
            // truncates a value belonging to a machine with a longer list),
            // while the labels are the active machine's, so indices between
            // the two are real gaps. `ParamDescriptor::value_labels` turns an
            // empty label into `None`, which is how a client is told there is
            // no choice here. Naming them all "off" would draw five identical
            // decoy entries on a HiHat.
            //
            // Also reachable through a p-lock, which bypasses the bank's
            // clamp; `lfo_dest_id` independently reads an unknown index as
            // off, so nothing is modulated either way.
            None => String::new(),
        }
    }

    fn parse(&self, s: &str) -> Option<f64> {
        if s.eq_ignore_ascii_case("off") {
            return Some(0.0);
        }
        self.0
            .iter()
            .position(|n| n.eq_ignore_ascii_case(s))
            .map(|i| (i + 1) as f64)
    }
}

/// An `LfoBlock` plus the per-sub-block resolution of where its output goes.
///
/// Shared by both machine engines, and by `Sampler`/`FilterNode` at MM-C10, so
/// the application rule lives in exactly one place.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LfoHost {
    block: LfoBlock,
    /// Param the LFO is modulating this sub-block; `None` = nothing.
    ///
    /// **`Option`, not a `0` sentinel.** `FilterNode` numbers its params
    /// `0, 1, 2` rather than name-hashing them, so `PARAM_CUTOFF == 0` — a
    /// zero sentinel silently read the filter's main destination as "off",
    /// and the LFO did nothing at all. Caught by MM-C10's coefficient-cache
    /// test; a node that name-hashes its ids would never have shown it.
    dest: Option<u32>,
    /// Additive offset for `dest`, already `depth x range x wave`.
    offset: f32,
    /// `dest`'s declared range, for the clamp.
    dest_range: (f32, f32),
}

impl LfoHost {
    pub fn new() -> Self {
        LfoHost {
            block: LfoBlock::new(),
            dest: None,
            offset: 0.0,
            dest_range: (0.0, 1.0),
        }
    }

    /// A note arrived.
    pub fn trigger(&mut self, s: LfoSettings) {
        self.block.trigger(s);
    }

    /// Advance the LFO and latch what it does to which param for this
    /// sub-block. Call once per sub-block, before the machine renders.
    ///
    /// A `None` destination latches a zero offset rather than skipping the
    /// tick — the phase must keep running so `/state/lfo_phase` stays live and
    /// switching a destination on does not jump.
    pub fn update(
        &mut self,
        s: LfoSettings,
        dest: Option<u32>,
        dest_range: (f32, f32),
        depth: f32,
        sample_rate: f32,
        samples: usize,
    ) {
        let wave = self.block.tick(s, sample_rate, samples);
        self.dest = dest;
        self.dest_range = dest_range;
        let span = dest_range.1 - dest_range.0;
        // The `dest == 0` arm is belt to `apply`'s brace, not load-bearing:
        // `apply` short-circuits on `dest != 0`, so the offset is never read
        // when the LFO is off. Kept so `offset` is meaningful on its own (in a
        // debugger, or to a future reader) rather than holding a stale value —
        // a mutant removing it is genuinely equivalent, which is why no test
        // kills it.
        self.offset = if dest.is_none() { 0.0 } else { depth * span * wave };
    }

    /// `base` with this sub-block's modulation applied, if `param_id` is the
    /// destination.
    ///
    /// **Only the modulated param is clamped.** Every other read returns
    /// `base` untouched — the bank already clamps on write, and clamping
    /// unconditionally here would be a behaviour change for every param the
    /// LFO is not touching.
    #[inline]
    pub fn apply(&self, param_id: u32, base: f32) -> f32 {
        if self.dest == Some(param_id) {
            (base + self.offset).clamp(self.dest_range.0, self.dest_range.1)
        } else {
            base
        }
    }

    pub fn phase(&self) -> f32 {
        self.block.phase()
    }
}

/// The wave at `phase`, drawing a fresh random value when asked for `Rand`.
/// Only `trigger` needs the drawing form; `tick` manages `Rand` itself.
#[allow(dead_code)]
fn raw_shape(shape: LfoShape, phase: f32, rng: &mut u32) -> f32 {
    match shape {
        LfoShape::Rand => xorshift(rng),
        other => raw_shape_no_rng(other, phase),
    }
}

/// The analytic shapes, −1…+1 over one cycle.
///
/// `Tri` and `Sine` start at 0 rising; `Saw` falls from +1 and `Ramp` rises
/// from −1, which is the usual distinction between the two names and the one
/// a performer will assume.
#[allow(dead_code)]
fn raw_shape_no_rng(shape: LfoShape, phase: f32) -> f32 {
    let p = phase.rem_euclid(1.0);
    match shape {
        LfoShape::Tri => {
            if p < 0.25 {
                4.0 * p
            } else if p < 0.75 {
                2.0 - 4.0 * p
            } else {
                4.0 * p - 4.0
            }
        }
        LfoShape::Sine => (p * std::f32::consts::TAU).sin(),
        LfoShape::Sqr => if p < 0.5 { 1.0 } else { -1.0 },
        LfoShape::Saw => 1.0 - 2.0 * p,
        LfoShape::Ramp => 2.0 * p - 1.0,
        // Decays from +1 toward -1; the 5.0 gives ~99% of the fall inside one
        // cycle without the tail sitting audibly short of the floor.
        LfoShape::Exp => 2.0 * (-5.0 * p).exp() - 1.0,
        // Handled by the caller, which owns the RNG.
        LfoShape::Rand => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── LfoBlock (MM-C8) ─────────────────────────────────────────────────

    const SR: f32 = 44100.0;

    fn settings(shape: LfoShape, mode: LfoMode) -> LfoSettings {
        LfoSettings { shape, mode, speed_hz: 1.0, start_phase: 0.0, fade: 0.0 }
    }

    /// Advance by whole sub-blocks, the way a host does.
    fn run(lfo: &mut LfoBlock, s: LfoSettings, blocks: usize) -> Vec<f32> {
        (0..blocks).map(|_| lfo.tick(s, SR, LFO_SUB_BLOCK)).collect()
    }

    /// Sample one cycle of a shape at 16 points by driving the phase directly,
    /// so the assertion is about the wave and not about timing.
    fn cycle(shape: LfoShape) -> Vec<f32> {
        (0..16)
            .map(|i| raw_shape_no_rng(shape, i as f32 / 16.0))
            .collect()
    }

    #[test]
    fn every_shape_stays_in_range_over_a_cycle() {
        for shape in LfoShape::ALL {
            if shape == LfoShape::Rand {
                continue; // drawn, not analytic — covered below
            }
            for v in cycle(shape) {
                assert!(
                    (-1.0..=1.0).contains(&v),
                    "{shape:?} leaves -1..1 with {v}"
                );
            }
        }
    }

    /// Every shape must actually reach **both** rails over its cycle. An LFO
    /// that only ever goes positive modulates in one direction, which is
    /// badly wrong and completely audible — and "starts at 0 rising" plus
    /// "stays in −1..1" both pass on it. A mutant making sine span half a
    /// cycle (`* PI` instead of `* TAU`) survived the whole suite until this
    /// existed.
    #[test]
    fn every_shape_is_bipolar_over_a_cycle() {
        for shape in LfoShape::ALL {
            if shape == LfoShape::Rand {
                continue;
            }
            // Sampled finely, because `Saw` and `Ramp` only approach their
            // far rail as the phase nears the wrap — at 16 points a correct
            // saw bottoms out at -0.875 and this reads as a failure.
            let c: Vec<f32> = (0..256)
                .map(|i| raw_shape_no_rng(shape, i as f32 / 256.0))
                .collect();
            let max = c.iter().cloned().fold(f32::MIN, f32::max);
            let min = c.iter().cloned().fold(f32::MAX, f32::min);
            assert!(max > 0.9, "{shape:?} never reaches the top rail (max {max})");
            assert!(min < -0.9, "{shape:?} never reaches the bottom rail (min {min})");
        }
    }

    /// Sine specifically, at its four quarter points — the shape whose period
    /// is easiest to get wrong by a factor of two.
    #[test]
    fn sine_spans_exactly_one_period() {
        let at = |p: f32| raw_shape_no_rng(LfoShape::Sine, p);
        assert!(at(0.0).abs() < 1e-6);
        assert!((at(0.25) - 1.0).abs() < 1e-5, "quarter must be +1, got {}", at(0.25));
        assert!(at(0.5).abs() < 1e-5, "half must return to 0, got {}", at(0.5));
        assert!((at(0.75) + 1.0).abs() < 1e-5, "three-quarter must be -1, got {}", at(0.75));
    }

    #[test]
    fn tri_and_sine_start_at_zero_rising() {
        for shape in [LfoShape::Tri, LfoShape::Sine] {
            let c = cycle(shape);
            assert!(c[0].abs() < 1e-6, "{shape:?} must start at 0, got {}", c[0]);
            assert!(c[1] > 0.0, "{shape:?} must rise first");
        }
        // Tri's corners are where a triangle's should be.
        assert!((raw_shape_no_rng(LfoShape::Tri, 0.25) - 1.0).abs() < 1e-6);
        assert!((raw_shape_no_rng(LfoShape::Tri, 0.75) + 1.0).abs() < 1e-6);
    }

    /// The one pair a performer will assume from the names: saw falls, ramp
    /// rises. Getting these the wrong way round is silent and infuriating.
    #[test]
    fn saw_falls_and_ramp_rises() {
        assert!((raw_shape_no_rng(LfoShape::Saw, 0.0) - 1.0).abs() < 1e-6);
        assert!(raw_shape_no_rng(LfoShape::Saw, 0.99) < -0.9);
        assert!((raw_shape_no_rng(LfoShape::Ramp, 0.0) + 1.0).abs() < 1e-6);
        assert!(raw_shape_no_rng(LfoShape::Ramp, 0.99) > 0.9);
    }

    #[test]
    fn sqr_is_two_valued_and_exp_decays_monotonically() {
        for p in [0.0, 0.25, 0.49] {
            assert_eq!(raw_shape_no_rng(LfoShape::Sqr, p), 1.0);
        }
        for p in [0.5, 0.75, 0.99] {
            assert_eq!(raw_shape_no_rng(LfoShape::Sqr, p), -1.0);
        }
        let e = cycle(LfoShape::Exp);
        assert!((e[0] - 1.0).abs() < 1e-6, "exp starts at +1");
        for w in e.windows(2) {
            assert!(w[1] < w[0], "exp must fall monotonically");
        }
    }

    /// `trig` resets phase to `lfo_start_phase`; `free` does not.
    #[test]
    fn trig_resets_the_phase_and_free_does_not() {
        let mut s = settings(LfoShape::Sine, LfoMode::Trig);
        s.start_phase = 0.25;
        let mut lfo = LfoBlock::new();
        run(&mut lfo, s, 100);
        let moved = lfo.phase();
        assert!((moved - 0.25).abs() > 1e-3, "phase must have advanced");
        lfo.trigger(s);
        assert!((lfo.phase() - 0.25).abs() < 1e-6, "trig resets to start_phase");

        let free = LfoSettings { mode: LfoMode::Free, ..s };
        let mut lfo = LfoBlock::new();
        run(&mut lfo, free, 100);
        let before = lfo.phase();
        lfo.trigger(free);
        assert_eq!(lfo.phase(), before, "free must ignore the note");
    }

    /// `hold` samples once per trigger and then holds, even though the phase
    /// underneath keeps moving — otherwise the next note samples the same spot.
    #[test]
    fn hold_samples_once_per_trigger() {
        let s = settings(LfoShape::Sine, LfoMode::Hold);
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        let out = run(&mut lfo, s, 40);
        assert!(
            out.windows(2).all(|w| w[0] == w[1]),
            "output must be frozen between notes: {out:?}"
        );
        let phase_moved = lfo.phase() != 0.0;
        assert!(phase_moved, "the phase must keep running underneath");

        lfo.trigger(s);
        let after = lfo.tick(s, SR, LFO_SUB_BLOCK);
        assert_ne!(after, out[0], "a new note must sample somewhere new");
    }

    /// `one` stops after a cycle; `half` stops at 0.5. Both then hold.
    #[test]
    fn one_stops_after_a_cycle_and_half_at_the_midpoint() {
        // 1 Hz, 64-sample blocks at 44.1 kHz -> ~690 blocks per cycle.
        let per_cycle = (SR / LFO_SUB_BLOCK as f32).ceil() as usize;

        let s = settings(LfoShape::Ramp, LfoMode::One);
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        run(&mut lfo, s, per_cycle * 3);
        let held = lfo.tick(s, SR, LFO_SUB_BLOCK);
        assert!((lfo.phase() - 0.0).abs() < 1e-4, "one lands back at start_phase");
        assert_eq!(held, lfo.tick(s, SR, LFO_SUB_BLOCK), "and then holds");

        let s = settings(LfoShape::Ramp, LfoMode::Half);
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        run(&mut lfo, s, per_cycle * 3);
        assert!(
            (lfo.phase() - 0.5).abs() < 1e-4,
            "half must stop exactly at the midpoint, got {}",
            lfo.phase()
        );
        let held = lfo.tick(s, SR, LFO_SUB_BLOCK);
        assert_eq!(held, lfo.tick(s, SR, LFO_SUB_BLOCK));
    }

    /// A sub-block can overshoot the stopping point; landing where the
    /// overshoot happened to fall would make `half` hold an arbitrary value.
    #[test]
    fn a_stopping_mode_lands_on_its_limit_not_where_the_sub_block_fell() {
        // Fast enough that one sub-block is a large fraction of a cycle.
        let mut s = settings(LfoShape::Ramp, LfoMode::Half);
        s.speed_hz = 64.0;
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        run(&mut lfo, s, 20);
        assert!(
            (lfo.phase() - 0.5).abs() < 1e-6,
            "expected exactly 0.5, got {}",
            lfo.phase()
        );
    }

    /// Fade in both directions, and off.
    #[test]
    fn fade_in_rises_from_zero_and_fade_out_falls_to_zero() {
        // Square wave at +1 for the first half cycle makes the envelope the
        // only thing moving.
        let mut s = settings(LfoShape::Sqr, LfoMode::Trig);
        s.speed_hz = 0.0; // hold phase at 0 -> raw value pinned at +1
        s.fade = 1.0;

        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        let first = lfo.tick(s, SR, LFO_SUB_BLOCK);
        assert!(first < 0.01, "fade-in starts near zero, got {first}");
        let blocks = (LFO_FADE_MAX_SECS * SR / LFO_SUB_BLOCK as f32) as usize;
        let out = run(&mut lfo, s, blocks);
        assert!(
            out.windows(2).all(|w| w[1] >= w[0]),
            "fade-in must be monotonic"
        );
        assert!((out[out.len() - 1] - 1.0).abs() < 1e-3, "and reach unity");

        s.fade = -1.0;
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        let first = lfo.tick(s, SR, LFO_SUB_BLOCK);
        assert!(first > 0.99, "fade-out starts at unity, got {first}");
        let out = run(&mut lfo, s, blocks);
        assert!(out.windows(2).all(|w| w[1] <= w[0]), "fade-out is monotonic");
        assert!(out[out.len() - 1].abs() < 1e-3, "and reaches zero");

        s.fade = 0.0;
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        assert_eq!(lfo.tick(s, SR, LFO_SUB_BLOCK), 1.0, "no fade means no scaling");
    }

    /// Half the fade knob is half the time, which is what makes the range
    /// usable rather than a switch.
    #[test]
    fn the_fade_knob_scales_the_fade_time() {
        let mut s = settings(LfoShape::Sqr, LfoMode::Trig);
        s.speed_hz = 0.0;
        s.fade = 0.5;
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        let blocks = (0.5 * LFO_FADE_MAX_SECS * SR / LFO_SUB_BLOCK as f32) as usize;
        let out = run(&mut lfo, s, blocks);
        assert!(
            (out[out.len() - 1] - 1.0).abs() < 1e-2,
            "fade 0.5 must complete in half of LFO_FADE_MAX_SECS, got {}",
            out[out.len() - 1]
        );
    }

    /// Sample-and-hold redraws once per cycle, not per tick.
    #[test]
    fn rand_holds_its_value_for_a_whole_cycle() {
        let s = settings(LfoShape::Rand, LfoMode::Trig);
        let mut lfo = LfoBlock::new();
        lfo.trigger(s);
        let per_cycle = (SR / LFO_SUB_BLOCK as f32).ceil() as usize;
        let out = run(&mut lfo, s, per_cycle - 2);
        assert!(
            out.windows(2).all(|w| w[0] == w[1]),
            "rand must hold within a cycle"
        );
        let next = run(&mut lfo, s, 4);
        assert!(
            next.iter().any(|v| *v != out[0]),
            "and redraw at the wrap"
        );
        assert!(next.iter().all(|v| (-1.0..=1.0).contains(v)));
    }

    /// Both selectors read an `f64` bank slot, so a malformed project must
    /// clamp rather than panic — the same contract as `AnalogMachine`.
    #[test]
    fn shape_and_mode_selectors_clamp_instead_of_panicking() {
        assert_eq!(LfoShape::from_value(0.0), LfoShape::Tri);
        assert_eq!(LfoShape::from_value(6.0), LfoShape::Rand);
        assert_eq!(LfoShape::from_value(99.0), LfoShape::Rand);
        assert_eq!(LfoShape::from_value(-1.0), LfoShape::Tri);
        assert_eq!(LfoShape::from_value(f32::NAN), LfoShape::Tri);
        assert_eq!(LfoMode::from_value(0.0), LfoMode::Free);
        assert_eq!(LfoMode::from_value(4.0), LfoMode::Half);
        assert_eq!(LfoMode::from_value(99.0), LfoMode::Half);
        assert_eq!(LfoMode::from_value(f32::NEG_INFINITY), LfoMode::Free);
    }

    /// Declaration order is the stored value, so it is append-only. A reorder
    /// silently re-points every saved patch at a different wave.
    #[test]
    fn shape_and_mode_declaration_order_is_pinned() {
        assert_eq!(
            LfoShape::ALL,
            [
                LfoShape::Tri, LfoShape::Sine, LfoShape::Sqr, LfoShape::Saw,
                LfoShape::Exp, LfoShape::Ramp, LfoShape::Rand
            ]
        );
        assert_eq!(
            LfoMode::ALL,
            [LfoMode::Free, LfoMode::Trig, LfoMode::Hold, LfoMode::One, LfoMode::Half]
        );
    }

    /// A zero or absent sample rate must not divide by zero or advance.
    #[test]
    fn a_degenerate_tick_is_inert() {
        let s = settings(LfoShape::Sine, LfoMode::Free);
        let mut lfo = LfoBlock::new();
        let v = lfo.tick(s, 0.0, LFO_SUB_BLOCK);
        assert!(v.is_finite());
        assert_eq!(lfo.phase(), 0.0);
        let v = lfo.tick(s, SR, 0);
        assert!(v.is_finite());
        assert_eq!(lfo.phase(), 0.0);
    }

    fn chunks(start: usize, end: usize) -> Vec<(usize, usize)> {
        sub_blocks(start, end).collect()
    }

    /// MM-C7's named cases. The 64/65 pair is the one that matters: an
    /// off-by-one in the boundary shows up here and nowhere else, since a
    /// 512-sample block divides evenly and the baselines would stay clean.
    #[test]
    fn a_span_shorter_than_one_sub_block_is_one_chunk() {
        assert_eq!(chunks(0, 1), vec![(0, 1)]);
        assert_eq!(chunks(0, 63), vec![(0, 63)]);
    }

    #[test]
    fn a_span_of_exactly_one_sub_block_is_one_chunk() {
        assert_eq!(chunks(0, LFO_SUB_BLOCK), vec![(0, 64)]);
    }

    #[test]
    fn one_sample_more_than_a_sub_block_is_two_chunks() {
        assert_eq!(chunks(0, LFO_SUB_BLOCK + 1), vec![(0, 64), (64, 65)]);
    }

    /// MM §0 D2: boundaries are measured from the span start, not from the
    /// block start. A span of 100 is 64 + 36 wherever it begins.
    #[test]
    fn boundaries_are_relative_to_the_span_not_the_block() {
        assert_eq!(chunks(411, 511), vec![(411, 475), (475, 511)]);
        let from_zero: Vec<usize> = chunks(0, 100).iter().map(|(a, b)| b - a).collect();
        let from_411: Vec<usize> = chunks(411, 511).iter().map(|(a, b)| b - a).collect();
        assert_eq!(from_zero, from_411, "same span length, same cut");
    }

    /// The caller needs no `start < end` guard, and an inverted span must not
    /// wrap into an enormous loop.
    #[test]
    fn empty_and_inverted_spans_yield_nothing() {
        assert!(chunks(0, 0).is_empty());
        assert!(chunks(64, 64).is_empty());
        assert!(chunks(100, 0).is_empty());
    }

    /// Whatever the cut, the chunks must tile the span exactly — no gap, no
    /// overlap, no sample rendered twice.
    #[test]
    fn chunks_tile_the_span_exactly() {
        for (start, end) in [(0, 512), (0, 100), (7, 7 + 129), (411, 511), (0, 64)] {
            let cs = chunks(start, end);
            assert_eq!(cs.first().unwrap().0, start);
            assert_eq!(cs.last().unwrap().1, end);
            for w in cs.windows(2) {
                assert_eq!(w[0].1, w[1].0, "gap or overlap in {start}..{end}");
            }
            assert!(cs.iter().all(|(a, b)| b > a && b - a <= LFO_SUB_BLOCK));
        }
    }
}
