// SPDX-License-Identifier: GPL-3.0-or-later
//! `InnerGraphNode` — a `Node` that owns an inner `NodeExecutor` (ADR-023).
//!
//! The inner graph runs: Sequencer → AnalogEngine::kick().
//! The executor's summed audio output is forwarded to the outer `audio_out` port.
//! Clock events arriving on the outer `clock_in` port are injected into the
//! inner Sequencer's incoming event queue each cycle.
//!
//! Inner state is not persisted at P9. Inner graph topology patching is P10+.

use paraclete_node_api::{
    CapabilityDocument, GraphNode, Node, PortDescriptor, PortDirection, PortType, ProcessInput,
    ProcessOutput, TimedEvent,
};
use paraclete_nodes::{AnalogEngine, Sequencer};
use paraclete_runtime::{NodeConfigurator, NodeExecutor};

/// Port IDs for `InnerGraphNode`'s outer interface.
pub const PORT_CLOCK_IN: u32 = 0;
pub const PORT_CV_IN_0: u32 = 1;
pub const PORT_AUDIO_OUT: u32 = 2;

/// Node IDs for nodes inside the inner graph.
pub const INNER_SEQ_ID: u32 = 1;
pub const INNER_KICK_ID: u32 = 2;

pub struct InnerGraphNode {
    executor: Option<NodeExecutor>,
    /// Stored inner edges `(src_node, src_port, dst_node, dst_port)` for runtime patching.
    inner_edges: Vec<(u32, u32, u32, u32)>,
    /// Pending inner node state blobs `(node_id, bytes)` to apply on/after `activate()`.
    pending_states: Vec<(u32, Vec<u8>)>,
    /// Interleaved stereo output buffer filled by the inner executor each cycle.
    inner_audio_buf: Vec<f32>,
    ports: [PortDescriptor; 3],
    sample_rate: f32,
    block_size: usize,
}

const IGN_MAGIC: &[u8; 4] = b"IGN1";

impl InnerGraphNode {
    /// Construct with the default inner graph (Sequencer → AnalogEngine::kick()).
    ///
    /// The executor is built at `activate()` time once `sample_rate` and
    /// `block_size` are known. Until then, `executor` is `None`.
    pub fn new() -> Self {
        InnerGraphNode {
            executor: None,
            inner_edges: vec![(
                INNER_SEQ_ID,
                Sequencer::PORT_EVENTS_OUT,
                INNER_KICK_ID,
                AnalogEngine::PORT_EVENTS_IN,
            )],
            pending_states: Vec::new(),
            inner_audio_buf: Vec::new(),
            ports: [
                PortDescriptor {
                    id: PORT_CLOCK_IN,
                    name: "clock_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Clock,
                },
                PortDescriptor {
                    id: PORT_CV_IN_0,
                    name: "cv_in_0".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Cv,
                },
                PortDescriptor {
                    id: PORT_AUDIO_OUT,
                    name: "audio_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Audio,
                },
            ],
            sample_rate: 44100.0,
            block_size: 512,
        }
    }

    /// Inject a `TimedEvent` directly into a specific inner node by its ID.
    ///
    /// Useful for testing. In production, clock events are forwarded from the
    /// outer `clock_in` port in `process()`.
    pub fn inject_inner_event(&mut self, node_id: u32, event: TimedEvent) {
        if let Some(exec) = &mut self.executor {
            exec.inject_event_for_node(node_id, event);
        }
    }

    /// Mutate the inner graph topology at runtime (`Fixes #119`).
    ///
    /// Drains live inner nodes into a temporary `NodeConfigurator`, reconnects
    /// existing edges, runs `f`, and rebuilds the inner `NodeExecutor`.
    pub fn with_configurator<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut NodeConfigurator) -> R,
    {
        let mut conf = if let Some(exec) = self.executor.take() {
            let mut c = NodeConfigurator::new(self.sample_rate, self.block_size);
            c.restore_nodes(exec.drain_nodes());
            for &(s, sp, d, dp) in &self.inner_edges {
                let _ = c.connect(s, sp, d, dp);
            }
            c
        } else {
            Self::build_default_configurator(self.sample_rate, self.block_size)
        };
        for (id, bytes) in self.pending_states.drain(..) {
            if let Some(node) = conf.node_mut(id) {
                node.deserialize(&bytes);
            }
        }
        let out = f(&mut conf);
        self.inner_edges = conf
            .all_edges()
            .map(|e| (e.src_node, e.src_port, e.dst_node, e.dst_port))
            .collect();
        self.executor = Some(conf.build_executor());
        out
    }

    fn build_default_configurator(sample_rate: f32, block_size: usize) -> NodeConfigurator {
        let mut conf = NodeConfigurator::new(sample_rate, block_size);

        // Inner Sequencer: pre-activates steps so audio fires without external pattern setup.
        let mut seq = Sequencer::new();
        // Activate a few steps so the inner pattern plays when the outer clock drives it.
        seq.set_step(0, 60, 32768, true);
        seq.set_step(2, 60, 32768, true);
        seq.set_step(4, 60, 32768, true);
        seq.set_step(6, 60, 32768, true);
        conf.add_node(INNER_SEQ_ID, Box::new(seq));
        conf.set_type_tag_for(INNER_SEQ_ID, "sequencer");

        conf.add_node(INNER_KICK_ID, Box::new(AnalogEngine::kick()));
        conf.set_type_tag_for(INNER_KICK_ID, "analog_engine:kick");

        // inner_seq.events_out → inner_kick.events_in
        conf.connect(
            INNER_SEQ_ID,
            Sequencer::PORT_EVENTS_OUT,
            INNER_KICK_ID,
            AnalogEngine::PORT_EVENTS_IN,
        )
        .expect("InnerGraphNode: inner sequencer → kick connection must succeed");

        conf
    }
}

impl Default for InnerGraphNode {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for InnerGraphNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "InnerGraphNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec![],
            view: None,
        }
    }

    fn type_name(&self) -> &'static str {
        "InnerGraphNode"
    }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate = sample_rate;
        self.block_size = block_size;
        // Interleaved stereo: 2 channels × block_size frames.
        self.inner_audio_buf = vec![0.0_f32; 2 * block_size];

        let mut conf = if let Some(exec) = self.executor.take() {
            let mut c = NodeConfigurator::new(sample_rate, block_size);
            c.restore_nodes(exec.drain_nodes());
            for &(s, sp, d, dp) in &self.inner_edges {
                let _ = c.connect(s, sp, d, dp);
            }
            c
        } else {
            Self::build_default_configurator(sample_rate, block_size)
        };
        let mut exec = conf.build_executor();
        for (id, bytes) in self.pending_states.drain(..) {
            exec.deserialize_node(id, &bytes);
        }
        self.executor = Some(exec);
    }

    fn deactivate(&mut self) {
        if let Some(exec) = &self.executor {
            self.pending_states = exec.serialize_all_nodes();
        }
        self.executor = None;
        self.inner_audio_buf = Vec::new();
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let exec = match &mut self.executor {
            Some(e) => e,
            None => return,
        };

        // Forward clock events from the outer clock_in port to the inner Sequencer.
        for ev in input.events {
            exec.inject_event_for_node(INNER_SEQ_ID, *ev);
        }

        // Run all inner nodes.
        exec.process(&mut self.inner_audio_buf, 2);

        // Copy interleaved inner audio to the outer audio_out AudioBuffer.
        // The outer ProcessOutput only has one audio output (index 0).
        if let Some(audio_out) = output.audio_outputs.first_mut() {
            let frames = self.block_size;
            let channels = audio_out.channels().min(2);
            for frame in 0..frames {
                for ch in 0..channels {
                    let interleaved_idx = frame * 2 + ch;
                    if interleaved_idx < self.inner_audio_buf.len() {
                        audio_out.channel_mut(ch)[frame] += self.inner_audio_buf[interleaved_idx];
                    }
                }
            }
        }
    }

    fn serialize(&self) -> Vec<u8> {
        let states: Vec<(u32, Vec<u8>)> = if let Some(exec) = &self.executor {
            exec.serialize_all_nodes()
        } else if !self.pending_states.is_empty() {
            self.pending_states.clone()
        } else {
            return Vec::new();
        };

        let mut out = Vec::new();
        out.extend_from_slice(IGN_MAGIC);
        out.extend_from_slice(&(states.len() as u32).to_le_bytes());
        for (id, bytes) in states {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        out
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.len() < 8 || &data[0..4] != IGN_MAGIC {
            return;
        }
        let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let mut cursor = 8usize;
        let mut states = Vec::with_capacity(count.min(256));
        for _ in 0..count {
            if cursor + 8 > data.len() {
                break;
            }
            let id = u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]);
            let len = u32::from_le_bytes([
                data[cursor + 4],
                data[cursor + 5],
                data[cursor + 6],
                data[cursor + 7],
            ]) as usize;
            cursor += 8;
            if cursor + len > data.len() {
                break;
            }
            states.push((id, data[cursor..cursor + len].to_vec()));
            cursor += len;
        }

        if let Some(exec) = &mut self.executor {
            for (id, bytes) in &states {
                exec.deserialize_node(*id, bytes);
            }
        } else {
            self.pending_states = states;
        }
    }
}

impl GraphNode for InnerGraphNode {}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab, TimedEvent, TransportInfo,
    };

    fn make_note_on() -> TimedEvent {
        use paraclete_node_api::{
            midi::{u4, u7, ChannelVoice2, Channeled, Grouped, NoteOn},
            UmpMessage,
        };
        let mut msg = NoteOn::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(60));
        msg.set_velocity(32768u16);
        TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(msg))))
    }

    fn run_ignode(ignode: &mut InnerGraphNode, events: &[TimedEvent]) -> Vec<f32> {
        let block = ignode.block_size.max(256);
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        let audio_ptr: *mut AudioBuffer = &mut audio;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events,
            transport: &transport,
            sample_rate: ignode.sample_rate,
            block_size: block,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        ignode.process(&input, &mut output);
        // Collect all samples from both channels.
        let ch0 = audio.channel(0).to_vec();
        let ch1 = audio.channel(1).to_vec();
        [ch0, ch1].concat()
    }

    #[test]
    fn inner_graph_node_ports_correct() {
        let ignode = InnerGraphNode::new();
        let ports = ignode.ports();
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0].name.as_str(), "clock_in");
        assert_eq!(ports[0].port_type, PortType::Clock);
        assert_eq!(ports[0].direction, PortDirection::Input);
        assert_eq!(ports[1].name.as_str(), "cv_in_0");
        assert_eq!(ports[1].port_type, PortType::Cv);
        assert_eq!(ports[1].direction, PortDirection::Input);
        assert_eq!(ports[2].name.as_str(), "audio_out");
        assert_eq!(ports[2].port_type, PortType::Audio);
        assert_eq!(ports[2].direction, PortDirection::Output);
    }

    #[test]
    fn inner_graph_node_is_graph_node() {
        fn assert_graph_node<T: GraphNode>(_: &T) {}
        assert_graph_node(&InnerGraphNode::new());
    }

    #[test]
    fn inner_graph_node_activate_no_panic() {
        let mut ignode = InnerGraphNode::new();
        ignode.activate(44100.0, 256);
    }

    #[test]
    fn inner_graph_node_deactivate_no_panic() {
        let mut ignode = InnerGraphNode::new();
        ignode.activate(44100.0, 256);
        ignode.deactivate();
    }

    #[test]
    fn inner_graph_node_serialize_returns_empty() {
        let ignode = InnerGraphNode::new();
        assert!(ignode.serialize().is_empty());
    }

    #[test]
    fn graph_node_marker_trait_implemented() {
        // Compile-time assertion: InnerGraphNode implements both Node and GraphNode.
        fn accepts_node<T: Node>(_: &T) {}
        fn accepts_graph_node<T: GraphNode>(_: &T) {}
        let ignode = InnerGraphNode::new();
        accepts_node(&ignode);
        accepts_graph_node(&ignode);
    }

    #[test]
    fn inner_graph_node_produces_audio_on_note_on() {
        let mut ignode = InnerGraphNode::new();
        ignode.activate(44100.0, 256);

        // Inject a NoteOn directly into the inner kick engine via the inner executor handle.
        ignode.inject_inner_event(INNER_KICK_ID, make_note_on());

        // Run 4 process cycles; the inner kick should produce non-zero audio.
        let mut all_samples: Vec<f32> = Vec::new();
        for _ in 0..4 {
            let samples = run_ignode(&mut ignode, &[]);
            all_samples.extend_from_slice(&samples);
        }
        assert!(
            all_samples.iter().any(|&s| s != 0.0),
            "InnerGraphNode must produce non-zero audio after NoteOn to inner kick"
        );
    }

    #[test]
    fn registry_contains_inner_graph() {
        // This test is in paraclete-app — verified via a separate integration test.
        // Compile-time: InnerGraphNode::new() returns a valid node.
        let _ = InnerGraphNode::new();
    }

    #[test]
    fn inner_graph_node_serialize_deserialize_roundtrips_inner_node_state() {
        use paraclete_node_api::{NodeCommand, ParamDescriptor, CMD_SET_PARAM};

        let mut src = InnerGraphNode::new();
        src.activate(44100.0, 256);

        // Mutate inner kick's tune to +12.0 via with_configurator + process
        let tune_id = ParamDescriptor::id_for_name("tune");
        src.with_configurator(|conf| {
            let _ = conf.send_command(NodeCommand {
                target_id: INNER_KICK_ID,
                type_id: CMD_SET_PARAM,
                arg0: tune_id as i64,
                arg1: 12.0,
            });
        });
        let _ = run_ignode(&mut src, &[]);

        let blob = src.serialize();
        assert!(
            blob.starts_with(b"IGN1") && blob.len() > 16,
            "activated InnerGraphNode::serialize() must return non-empty IGN1 state blob"
        );

        let mut dst = InnerGraphNode::new();
        dst.activate(44100.0, 256);
        dst.deserialize(&blob);

        assert_eq!(
            dst.serialize(),
            blob,
            "InnerGraphNode serialize/deserialize must round-trip identically"
        );
    }

    #[test]
    fn inner_graph_node_with_configurator_patches_live_inner_topology() {
        use paraclete_nodes::FilterNode;

        let mut ignode = InnerGraphNode::new();
        ignode.activate(44100.0, 256);

        // Patch a FilterNode (id 40) downstream of INNER_KICK_ID (2) at runtime
        ignode.with_configurator(|conf| {
            conf.add_node(40, Box::new(FilterNode::new()));
            conf.connect(INNER_KICK_ID, 1, 40, 0)
                .expect("connect inner kick -> inner filter");
        });

        ignode.inject_inner_event(INNER_KICK_ID, make_note_on());
        let samples = run_ignode(&mut ignode, &[]);
        assert!(
            samples.iter().any(|&s| s.abs() > 1e-6),
            "patched InnerGraphNode with inner filter must render audio"
        );
    }
}
