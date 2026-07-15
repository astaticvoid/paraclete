// SPDX-License-Identifier: GPL-3.0-or-later
//! Helpers for constructing a ProcessInput for a CLAP process() callback.
//!
//! The CLAP FFI layer (Commits 5/6) translates C pointers into Paraclete types
//! and calls these helpers. Tests call them directly with Paraclete-native data.

use paraclete_node_api::{ExtendedEventSlab, NodeCommand, ProcessInput, TimedEvent, TransportInfo};

/// Build a `ProcessInput` for a MIDI-effect node (no audio inputs).
///
/// Used by `SingleNodePlugin` for nodes like `Sequencer` that consume
/// only events and commands, not audio.
pub(crate) fn make_process_input<'a>(
    transport: &'a TransportInfo,
    events: &'a [TimedEvent],
    commands: &'a [NodeCommand],
    slab: &'a ExtendedEventSlab,
    sample_rate: f32,
    block_size: usize,
) -> ProcessInput<'a> {
    ProcessInput {
        audio_inputs: &[],
        signal_inputs: &[],
        events,
        transport,
        sample_rate,
        block_size,
        extended_events: slab,
        commands,
    }
}
