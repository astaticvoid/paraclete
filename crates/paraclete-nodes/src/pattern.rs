// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical 8-track test/demo pattern.
//!
//! Used by paraclete-app for the startup preset and by the integration test
//! to verify all 8 sequencers fire at the expected step positions.

/// One track: its name and the active step indices (0-based, 0–15).
pub struct TrackPreset {
    pub name:  &'static str,
    pub steps: &'static [usize],
    /// MIDI note number for each active step.
    pub note:  u8,
}

/// Industrial/techno 16-step pattern across 8 tracks.
///
/// ```text
/// Step:  0 1 2 3 4 5 6 7 8 9 A B C D E F
/// Kick:  X . . . X . . . X . . . X . . .   4-on-the-floor
/// Snare: . . . . X . . . . . . . X . . .   backbeat 2/4
/// HatCH: X . X . X . X . X . X . X . X .   eighth notes
/// HatOH: . . X . . . X . . . X . . . X .   off-beat eighths
/// PercA: . . X . . . . . . . X . . . . .   sparse cross-beat
/// PercB: . . . . . . X . . . . . . . X .   sparse cross-beat
/// FX:    . . . X . . . . . . . X . . . .   off-beat accents
/// Bass:  X . . . X . . . X . . . X . . .   matches kick
/// ```
pub const TRACKS: &[TrackPreset] = &[
    TrackPreset { name: "Kick",   note: 60, steps: &[0, 4, 8, 12] },
    TrackPreset { name: "Snare",  note: 60, steps: &[4, 12] },
    TrackPreset { name: "Hat CH", note: 60, steps: &[0, 2, 4, 6, 8, 10, 12, 14] },
    TrackPreset { name: "Hat OH", note: 60, steps: &[2, 6, 10, 14] },
    TrackPreset { name: "Perc A", note: 60, steps: &[2, 10] },
    TrackPreset { name: "Perc B", note: 60, steps: &[6, 14] },
    TrackPreset { name: "FX",     note: 60, steps: &[3, 11] },
    TrackPreset { name: "Bass",   note: 60, steps: &[0, 4, 8, 12] },
];

/// Build a `Sequencer` from a `TrackPreset`.
pub fn apply_preset(seq: &mut crate::Sequencer, preset: &TrackPreset) {
    for i in 0..16 {
        let active = preset.steps.contains(&i);
        seq.set_step(i, preset.note, 40_000, active);
    }
}
