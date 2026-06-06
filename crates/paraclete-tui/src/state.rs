// SPDX-License-Identifier: GPL-3.0-or-later

pub struct TuiState {
    pub bpm:          f64,
    pub playing:      bool,
    pub current_step: u8,
    pub active_track: usize,
    pub steps:        [bool; 16],
    pub encoders:     Vec<EncoderSlot>,
    pub dirty:        bool,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            bpm:          120.0,
            playing:      false,
            current_step: 0,
            active_track: 0,
            steps:        [false; 16],
            encoders:     Vec::new(),
            dirty:        true,
        }
    }
}

pub struct EncoderSlot {
    pub label:            String,
    pub param_id:         u32,
    pub node_id:          u32,
    pub value:            f64,
    pub min:              f64,
    pub max:              f64,
    pub recently_changed: bool,
    pub changed_at_ms:    u64,
}

impl Default for EncoderSlot {
    fn default() -> Self {
        Self {
            label:            String::new(),
            param_id:         0,
            node_id:          0,
            value:            0.0,
            min:              0.0,
            max:              1.0,
            recently_changed: false,
            changed_at_ms:    0,
        }
    }
}
