// SPDX-License-Identifier: GPL-3.0-or-later
//! SingleNodePlugin — wraps one portable Paraclete node as a CLAP instrument
//! or MIDI effect.
//!
//! For P7, the concrete use case is Sequencer as a CLAP MIDI effect.
//!
//! This module contains the testable safe-Rust core. The CLAP FFI entry points
//! (clap_plugin_entry, extern "C" callbacks) live in src/bin/sequencer.rs and
//! are added when clap-sys is introduced in Commit 6.

use paraclete_node_api::{
    EventOutputBuffer, ExtendedEventSlab, Node, NodeCommand,
    ProcessOutput, TimedEvent, TransportInfo,
};
use crate::bridge::ClapParamBridge;
use crate::process_input::make_process_input;

/// Wraps one portable Paraclete node as a CLAP instrument or MIDI effect.
///
/// Exposes a safe Rust API. The CLAP FFI layer (added in Commit 6) delegates
/// to these methods after translating raw C pointers to Paraclete types.
pub struct SingleNodePlugin {
    node:        Box<dyn Node>,
    /// CLAP ↔ Paraclete parameter translation table. Built at `activate()`.
    bridge:      ClapParamBridge,
    sample_rate: f32,
    block_size:  usize,
    /// Pre-allocated output event buffer (Sequencer MIDI output).
    events_out:  EventOutputBuffer,
}

impl SingleNodePlugin {
    pub fn new(node: Box<dyn Node>) -> Self {
        Self {
            node,
            bridge:      ClapParamBridge::empty(),
            sample_rate: 44100.0,
            block_size:  512,
            events_out:  EventOutputBuffer::new(256),
        }
    }

    /// Configure and activate the wrapped node. Must be called before
    /// `process_block()`. Re-builds the parameter bridge from the node's
    /// capability document.
    pub fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate = sample_rate;
        self.block_size  = block_size;
        self.node.activate(sample_rate, block_size);
        self.bridge = ClapParamBridge::from_capability_document(&self.node.capability_document());
    }

    /// Deactivate the wrapped node.
    pub fn deactivate(&mut self) {
        self.node.deactivate();
    }

    /// Run one audio block. Returns events emitted by the node this block
    /// (e.g. MIDI NoteOn/NoteOff from a Sequencer). Commands are processed
    /// before DSP runs, consistent with the executor's event ordering.
    pub fn process_block(
        &mut self,
        transport: &TransportInfo,
        events:    &[TimedEvent],
        commands:  &[NodeCommand],
    ) -> Vec<TimedEvent> {
        let slab = ExtendedEventSlab::empty();
        let input = make_process_input(
            transport, events, commands, &slab,
            self.sample_rate, self.block_size,
        );

        self.events_out.clear();
        // Sequencer is a MIDI effect — no audio outputs.
        let mut output = ProcessOutput::new(
            &mut [],
            &mut [],
            &mut self.events_out,
        );

        self.node.process(&input, &mut output);
        self.events_out.as_slice().to_vec()
    }

    /// Serialise node state. Maps to the CLAP state extension `save()`.
    pub fn state_save(&self) -> Vec<u8> {
        self.node.serialize()
    }

    /// Restore node state. Maps to the CLAP state extension `load()`.
    ///
    /// CLAP hosts typically call this between `deactivate()` and `activate()`
    /// during session restore. Call `activate()` after `state_load()` to
    /// re-initialise sample-rate-dependent state.
    pub fn state_load(&mut self, data: &[u8]) {
        self.node.deserialize(data);
    }

    /// Access the parameter bridge (used by the CLAP params extension).
    pub fn bridge(&self) -> &ClapParamBridge {
        &self.bridge
    }
}
