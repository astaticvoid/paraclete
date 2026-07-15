// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete terminal UI — transport bar, encoder row, step row.

mod app;
mod layout;
mod state;

pub use app::TuiApp;
pub use state::{EncoderSlot, TuiState};

pub struct TuiConfig {
    pub clock_id: u32,
    pub seq_ids: Vec<u32>,
    pub encoder_count: u8,
    pub fps: u8,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            clock_id: 0,
            seq_ids: vec![],
            encoder_count: 8,
            fps: 30,
        }
    }
}

#[derive(Debug)]
pub enum TuiError {
    Io(std::io::Error),
    Draw(String),
}

impl std::fmt::Display for TuiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TuiError::Io(e) => write!(f, "TUI I/O error: {}", e),
            TuiError::Draw(s) => write!(f, "TUI draw error: {}", s),
        }
    }
}

impl std::error::Error for TuiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TuiError::Io(e) => Some(e),
            TuiError::Draw(_) => None,
        }
    }
}

impl From<std::io::Error> for TuiError {
    fn from(e: std::io::Error) -> Self {
        TuiError::Io(e)
    }
}
