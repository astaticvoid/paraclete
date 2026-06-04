/// Shared DSP primitives for AnalogEngine and FmEngine.
/// Private to paraclete-nodes — not exported.

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
