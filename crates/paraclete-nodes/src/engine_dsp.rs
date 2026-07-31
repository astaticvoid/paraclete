//! Shared DSP primitives for AnalogEngine and FmEngine.
//! Private to paraclete-nodes — not exported.
//!
//! Was an outer `///` doc, which attached the module's own description to
//! whatever item happened to come first. MM-C7 put a `const` there and clippy
//! noticed; `//!` is what a module doc should have been all along.

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

#[cfg(test)]
mod tests {
    use super::*;

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
