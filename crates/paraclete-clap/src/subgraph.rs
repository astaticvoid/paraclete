// SPDX-License-Identifier: GPL-3.0-or-later
//! SubgraphPlugin — wraps a full instrument subgraph (InternalClock + Sequencer +
//! generator) as a CLAP instrument plugin.
//!
//! The internal InternalClock drives the Sequencer autonomously. External MIDI
//! events from the DAW (passed to `process_block`) are routed directly to the
//! generator, bypassing the Sequencer. DAW transport transitions (play/stop)
//! are injected via `set_transport_override`.
//!
//! This module contains the testable safe-Rust core. CLAP FFI entry points are
//! added per machine bank binary.

use paraclete_node_api::{
    Node, NodeCommand, PortDirection, PortType, TimedEvent, TransportEvent, TransportFlags,
    TransportInfo, TICKS_PER_BEAT,
};
use paraclete_nodes::{InternalClock, Sequencer};
use paraclete_runtime::{NodeConfigurator, NodeExecutor};

use crate::bridge::ClapParamBridge;

/// Node ID reserved for the InternalClock within every machine subgraph.
const CLOCK_ID: u32 = 1;
/// Node ID reserved for the Sequencer within every machine subgraph.
/// Exposed publicly as `SUBGRAPH_SEQ_ID` via the crate root re-export.
pub(crate) const SEQ_ID: u32 = 2;

/// Wraps an InternalClock + Sequencer + generator instrument node as a CLAP
/// instrument plugin.
///
/// Exposes a safe Rust API. The CLAP FFI layer (per binary) delegates to these
/// methods after translating raw C pointers to Paraclete types.
pub struct SubgraphPlugin {
    executor: NodeExecutor,
    gen_id: u32,
    bridge: ClapParamBridge,
    sample_rate: f32,
    block_size: usize,
    prev_playing: bool,
    /// Pre-allocated interleaved stereo output buffer (block_size × 2 f32s).
    audio_out: Vec<f32>,
}

impl SubgraphPlugin {
    /// Create a SubgraphPlugin wrapping the given generator node.
    ///
    /// `gen_id` must not be 1 or 2 (reserved for InternalClock and Sequencer).
    pub fn new(generator: Box<dyn Node>, gen_id: u32, sample_rate: f32, block_size: usize) -> Self {
        let cap_doc = generator.capability_document();
        let bridge = ClapParamBridge::from_capability_document(&cap_doc);
        let executor = build_machine_subgraph(generator, gen_id, sample_rate, block_size);
        Self {
            executor,
            gen_id,
            bridge,
            sample_rate,
            block_size,
            prev_playing: false,
            audio_out: vec![0.0f32; block_size * 2],
        }
    }

    /// Record the host's sample rate and block size.
    ///
    /// The underlying nodes were activated at construction time with the same
    /// values; this updates bookkeeping for subsequent `process_block()` calls.
    pub fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate = sample_rate;
        self.block_size = block_size;
        if self.audio_out.len() != block_size * 2 {
            self.audio_out.resize(block_size * 2, 0.0);
        }
    }

    pub fn deactivate(&mut self) {
        self.prev_playing = false;
    }

    /// Run one audio block.
    ///
    /// * `transport` — current host transport state.
    /// * `transport_event` — pre-computed transition event from `translate_transport`,
    ///   or `None` to compute it automatically from `prev_playing()`.
    /// * `external_events` — MIDI events from the host, delivered directly to
    ///   the generator (bypasses the internal Sequencer).
    /// * `commands` — `NodeCommand`s targeting the generator (host param
    ///   automation, translated via `bridge()`).
    ///
    /// Returns the interleaved stereo output for this block.
    pub fn process_block(
        &mut self,
        transport: &TransportInfo,
        transport_event: Option<&TransportEvent>,
        external_events: &[TimedEvent],
        commands: &[NodeCommand],
    ) -> &[f32] {
        let event = match transport_event {
            Some(ev) => {
                self.prev_playing = transport.playing;
                Some(*ev)
            }
            None => {
                let ev = transport_transition_event(transport, self.prev_playing);
                self.prev_playing = transport.playing;
                ev
            }
        };

        self.executor.set_transport_override(*transport, event);

        for &ev in external_events {
            self.executor.inject_event_for_node(self.gen_id, ev);
        }
        for &cmd in commands {
            self.executor.inject_node_command(NodeCommand {
                target_id: self.gen_id,
                ..cmd
            });
        }

        self.audio_out.fill(0.0);
        self.executor.process(&mut self.audio_out, 2);
        &self.audio_out
    }

    /// Send a command to any node in the subgraph by its ID.
    ///
    /// Used by tests and host automation to program the internal Sequencer's
    /// step pattern (`target_id = SubgraphPlugin::SEQ_ID`) or adjust generator
    /// parameters directly (`target_id = self.gen_id()`).
    pub fn send_command(&mut self, cmd: NodeCommand) {
        self.executor.inject_node_command(cmd);
    }

    /// Serialise the full subgraph state (Sequencer + generator).
    ///
    /// Format: `[seq_len: u32 LE][seq_bytes…][gen_bytes…]`.
    pub fn state_save(&self) -> Vec<u8> {
        let seq_bytes = self.executor.serialize_node(SEQ_ID);
        let gen_bytes = self.executor.serialize_node(self.gen_id);
        let mut out = Vec::with_capacity(4 + seq_bytes.len() + gen_bytes.len());
        out.extend_from_slice(&(seq_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&seq_bytes);
        out.extend_from_slice(&gen_bytes);
        out
    }

    /// Restore the full subgraph state from bytes produced by `state_save()`.
    ///
    /// Silently ignores malformed input (too short or truncated seq section).
    pub fn state_load(&mut self, data: &[u8]) {
        if data.len() < 4 {
            return;
        }
        let seq_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        if 4 + seq_len > data.len() {
            return;
        }
        self.executor
            .deserialize_node(SEQ_ID, &data[4..4 + seq_len]);
        self.executor
            .deserialize_node(self.gen_id, &data[4 + seq_len..]);
    }

    pub fn bridge(&self) -> &ClapParamBridge {
        &self.bridge
    }
    pub fn seq_id(&self) -> u32 {
        SEQ_ID
    }
    pub fn gen_id(&self) -> u32 {
        self.gen_id
    }
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
    pub fn block_size(&self) -> usize {
        self.block_size
    }
    pub fn prev_playing(&self) -> bool {
        self.prev_playing
    }
}

/// Build an executor containing InternalClock(1) → Sequencer(2) → generator(gen_id).
fn build_machine_subgraph(
    generator: Box<dyn Node>,
    gen_id: u32,
    sample_rate: f32,
    block_size: usize,
) -> NodeExecutor {
    // Discover the generator's event input port before moving it into the graph.
    let gen_event_in = generator
        .ports()
        .iter()
        .find(|p| p.direction == PortDirection::Input && p.port_type == PortType::Event)
        .map(|p| p.id)
        .expect("generator must declare an Event Input port for Sequencer→generator routing");

    let mut conf = NodeConfigurator::new(sample_rate, block_size);
    conf.add_node(CLOCK_ID, Box::new(InternalClock::new()));
    conf.add_node(SEQ_ID, Box::new(Sequencer::new()));
    conf.add_node(gen_id, generator);
    conf.connect(
        CLOCK_ID,
        InternalClock::PORT_CLOCK_OUT,
        SEQ_ID,
        Sequencer::PORT_CLOCK_IN,
    )
    .expect("InternalClock → Sequencer clock connection must succeed");
    conf.connect(SEQ_ID, Sequencer::PORT_EVENTS_OUT, gen_id, gen_event_in)
        .expect("Sequencer → generator event connection must succeed");
    conf.build_executor()
}

/// Emit a transport event only on playing-state transitions.
///
/// Returns `Some(global_start)` on false→true, `Some(global_stop)` on true→false,
/// `None` on stable state. This prevents the Sequencer from resetting its step
/// counter on every audio buffer.
fn transport_transition_event(
    transport: &TransportInfo,
    prev_playing: bool,
) -> Option<TransportEvent> {
    if !prev_playing && transport.playing {
        Some(TransportEvent {
            domain_id: 0,
            bar: 1,
            beat: 0,
            tick: 0,
            ticks_per_beat: TICKS_PER_BEAT,
            bpm: transport.bpm,
            time_sig_num: 4,
            time_sig_den: 4,
            flags: TransportFlags {
                global_start: true,
                playing: true,
                ..TransportFlags::default()
            },
        })
    } else if prev_playing && !transport.playing {
        Some(TransportEvent {
            domain_id: 0,
            bar: 1,
            beat: 0,
            tick: 0,
            ticks_per_beat: TICKS_PER_BEAT,
            bpm: transport.bpm,
            time_sig_num: 4,
            time_sig_den: 4,
            flags: TransportFlags {
                global_stop: true,
                ..TransportFlags::default()
            },
        })
    } else {
        None
    }
}
