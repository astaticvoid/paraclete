// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L0 HAL — Hardware Abstraction Layer.
//!
//! The only layer that knows about the physical world. All hardware appears
//! as graph nodes above the HAL boundary.

pub mod audio;
pub mod digitakt;
pub mod emulator;
pub mod keystep;
pub mod launchpad;
pub mod midi;

pub use audio::{query_sample_rate, AudioBackend, AudioEngine, AudioError};
pub use digitakt::DigitaktMidiNode;
pub use emulator::LaunchpadEmulator;
pub use keystep::KeystepNode;
pub use launchpad::LaunchpadNode;
pub use midi::{MidiDeviceError, MidiDeviceRegistry};
