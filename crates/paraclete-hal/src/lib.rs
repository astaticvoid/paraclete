// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L0 HAL — Hardware Abstraction Layer.
//!
//! The only layer that knows about the physical world. All hardware appears
//! as graph nodes above the HAL boundary.

pub mod audio;
pub mod emulator;
pub mod midi;
pub mod launchpad;
pub mod digitakt;
pub mod keystep;

pub use audio::{AudioBackend, AudioError};
pub use emulator::LaunchpadEmulator;
pub use launchpad::LaunchpadNode;
pub use digitakt::DigitaktMidiNode;
pub use keystep::KeystepNode;
pub use midi::{MidiDeviceError, MidiDeviceRegistry};
