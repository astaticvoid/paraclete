// SPDX-License-Identifier: GPL-3.0-or-later
//! MIDI device registry — cross-platform MIDI I/O via midir.

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

#[derive(Debug)]
pub enum MidiDeviceError {
    Init(midir::InitError),
    Connect(String),
    PortNotFound(String),
}

impl std::fmt::Display for MidiDeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Init(e)         => write!(f, "MIDI init error: {e}"),
            Self::Connect(s)      => write!(f, "MIDI connect error: {s}"),
            Self::PortNotFound(s) => write!(f, "MIDI port not found: {s}"),
        }
    }
}

impl From<midir::InitError> for MidiDeviceError {
    fn from(e: midir::InitError) -> Self { Self::Init(e) }
}

/// Cross-platform MIDI device registry.
pub struct MidiDeviceRegistry {
    midi_in:  MidiInput,
    midi_out: MidiOutput,
}

impl MidiDeviceRegistry {
    pub fn new() -> Result<Self, MidiDeviceError> {
        Ok(Self {
            midi_in:  MidiInput::new("paraclete-in")?,
            midi_out: MidiOutput::new("paraclete-out")?,
        })
    }

    pub fn input_port_names(&self) -> Vec<String> {
        self.midi_in.ports().iter()
            .filter_map(|p| self.midi_in.port_name(p).ok())
            .collect()
    }

    pub fn output_port_names(&self) -> Vec<String> {
        self.midi_out.ports().iter()
            .filter_map(|p| self.midi_out.port_name(p).ok())
            .collect()
    }

    /// Open an input port whose name contains `name_substr`.
    pub fn open_input<F>(
        self,
        name_substr: &str,
        callback: F,
    ) -> Result<MidiInputConnection<()>, MidiDeviceError>
    where
        F: Fn(u64, &[u8], &mut ()) + Send + 'static,
    {
        let port = self.midi_in.ports().into_iter()
            .find(|p| self.midi_in.port_name(p)
                .map(|n| n.contains(name_substr))
                .unwrap_or(false))
            .ok_or_else(|| MidiDeviceError::PortNotFound(name_substr.to_string()))?;

        self.midi_in.connect(&port, "paraclete-in", callback, ())
            .map_err(|e| MidiDeviceError::Connect(e.to_string()))
    }

    /// Open an output port whose name contains `name_substr`.
    pub fn open_output(
        self,
        name_substr: &str,
    ) -> Result<MidiOutputConnection, MidiDeviceError> {
        let port = self.midi_out.ports().into_iter()
            .find(|p| self.midi_out.port_name(p)
                .map(|n| n.contains(name_substr))
                .unwrap_or(false))
            .ok_or_else(|| MidiDeviceError::PortNotFound(name_substr.to_string()))?;

        self.midi_out.connect(&port, "paraclete-out")
            .map_err(|e| MidiDeviceError::Connect(e.to_string()))
    }
}
