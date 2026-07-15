// SPDX-License-Identifier: GPL-3.0-or-later

pub struct TuiState {
    pub bpm: f64,
    pub playing: bool,
    /// Absolute step position within the active pattern (0-63 at P10).
    pub current_step: u8,
    pub active_track: usize,
    /// The 16-step display window's steps (see `window_base`).
    pub steps: [bool; 16],
    /// First absolute step of the 16-step window the row displays — the
    /// window containing `current_step` (P10 C5: patterns reach 64 steps,
    /// the row shows 16 at a time).
    pub window_base: usize,
    /// Active pattern length in steps (1-64).
    pub pattern_length: usize,
    /// P10 C5 pattern-engine surface (`/node/{id}/state/*`).
    pub active_pattern: usize,
    /// Cued pattern index, or -1 when none.
    pub cued_pattern: i64,
    pub current_page: usize,
    pub page_count: usize,
    pub speed_mult: f64,
    pub encoders: Vec<EncoderSlot>,
    pub dirty: bool,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            playing: false,
            current_step: 0,
            active_track: 0,
            steps: [false; 16],
            window_base: 0,
            pattern_length: 16,
            active_pattern: 0,
            cued_pattern: -1,
            current_page: 0,
            page_count: 2,
            speed_mult: 1.0,
            encoders: Vec::new(),
            dirty: true,
        }
    }
}

pub struct EncoderSlot {
    pub label: String,
    pub param_id: u32,
    pub node_id: u32,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub recently_changed: bool,
    pub changed_at_ms: u64,
}

impl Default for EncoderSlot {
    fn default() -> Self {
        Self {
            label: String::new(),
            param_id: 0,
            node_id: 0,
            value: 0.0,
            min: 0.0,
            max: 1.0,
            recently_changed: false,
            changed_at_ms: 0,
        }
    }
}
