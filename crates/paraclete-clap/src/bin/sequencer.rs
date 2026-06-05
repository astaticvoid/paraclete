// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete Sequencer — CLAP MIDI effect.
//!
//! This binary becomes `paraclete_sequencer.clap` when compiled as a cdylib.
//! The CLAP FFI glue (clap_plugin_entry, extern "C" lifecycle callbacks, params
//! extension, note ports extension, state extension) is added in Commit 6 when
//! clap-sys is introduced.
//!
//! For now this stub exists so the binary target is declared and the module
//! structure is established.

fn main() {
    // Placeholder. The .clap binary is a cdylib — this main() is never called.
    // Build as cdylib: cargo build -p paraclete-clap --lib (once crate-type includes cdylib)
}
