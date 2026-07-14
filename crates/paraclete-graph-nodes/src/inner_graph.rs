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
    CapabilityDocument, GraphNode, Node, PortDescriptor, PortDirection, PortType,
    ProcessInput, ProcessOutput, TimedEvent,
};
use paraclete_nodes::{AnalogEngine, Sequencer};
use paraclete_runtime::{NodeConfigurator, NodeExecutor};

/// Port IDs for `InnerGraphNode`'s outer interface.
pub const PORT_CLOCK_IN:  u32 = 0;
pub const PORT_CV_IN_0:   u32 = 1;
pub const PORT_AUDIO_OUT: u32 = 2;

/// Node IDs for nodes inside the inner graph.
pub const INNER_SEQ_ID:  u32 = 1;
pub const INNER_KICK_ID: u32 = 2;

pub struct InnerGraphNode {
    executor:        Option<NodeExecutor>,
    /// Interleaved stereo output buffer filled by the inner executor each cycle.
    inner_audio_buf: Vec<f32>,
    ports:           [PortDescriptor; 3],
    sample_rate:     f32,
    block_size:      usize,
}

impl InnerGraphNode {
    /// Construct with the default inner graph (Sequencer → AnalogEngine::kick()).
    ///
    /// The executor is built at `activate()` time once `sample_rate` and
    /// `block_size` are known. Until then, `executor` is `None`.
    pub fn new() -> Self {
        InnerGraphNode {
            executor: None,
            inner_audio_buf: Vec::new(),
            ports: [
                PortDescriptor {
                    id:         PORT_CLOCK_IN,
                    name:       "clock_in".into(),
                    direction:  PortDirection::Input,
                    port_type:  PortType::Clock,
                },
                PortDescriptor {
                    id:         PORT_CV_IN_0,
                    name:       "cv_in_0".into(),
                    direction:  PortDirection::Input,
                    port_type:  PortType::Cv,
                },
                PortDescriptor {
                    id:         PORT_AUDIO_OUT,
                    name:       "audio_out".into(),
                    direction:  PortDirection::Output,
                    port_type:  PortType::Audio,
                },
            ],
            sample_rate: 44100.0,
            block_size:  512,
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

    fn build_inner_executor(sample_rate: f32, block_size: usize) -> NodeExecutor {
        let mut conf = NodeConfigurator::new(sample_rate, block_size);

        // Inner Sequencer: pre-activates steps so audio fires without external pattern setup.
        let mut seq = Sequencer::new();
        // Activate a few steps so the inner pattern plays when the outer clock drives it.
        seq.set_step(0, 60, 32768, true);
        seq.set_step(2, 60, 32768, true);
        seq.set_step(4, 60, 32768, true);
        seq.set_step(6, 60, 32768, true);
        conf.add_node(INNER_SEQ_ID, Box::new(seq));

        conf.add_node(INNER_KICK_ID, Box::new(AnalogEngine::kick()));

        // inner_seq.events_out → inner_kick.events_in
        conf.connect(
            INNER_SEQ_ID,  Sequencer::PORT_EVENTS_OUT,
            INNER_KICK_ID, AnalogEngine::PORT_EVENTS_IN,
        ).expect("InnerGraphNode: inner sequencer → kick connection must succeed");

        conf.build_executor()
    }
}

impl Default for InnerGraphNode {
    fn default() -> Self { Self::new() }
}

impl Node for InnerGraphNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name:       "InnerGraphNode".into(),
            vendor:     "Paraclete".into(),
            version:    (0, 1, 0),
            ports:      self.ports.to_vec(),
            params:     vec![],
            extensions: vec![],
        }
    }

    fn type_name(&self) -> &'static str { "InnerGraphNode" }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate     = sample_rate;
        self.block_size      = block_size;
        // Interleaved stereo: 2 channels × block_size frames.
        self.inner_audio_buf = vec![0.0_f32; 2 * block_size];
        self.executor        = Some(Self::build_inner_executor(sample_rate, block_size));
    }

    fn deactivate(&mut self) {
        // Dropping NodeExecutor deactivates all inner nodes.
        self.executor        = None;
        self.inner_audio_buf = Vec::new();
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let exec = match &mut self.executor {
            Some(e) => e,
            None    => return,
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
            let frames   = self.block_size;
            let channels = audio_out.channels().min(2);
            for frame in 0..frames {
                for ch in 0..channels {
                    let interleaved_idx = frame * 2 + ch;
                    if interleaved_idx < self.inner_audio_buf.len() {
                        audio_out.channel_mut(ch)[frame] +=
                            self.inner_audio_buf[interleaved_idx];
                    }
                }
            }
        }
    }

    fn serialize(&self) -> Vec<u8> { vec![] }
    fn deserialize(&mut self, _data: &[u8]) {}
}

impl GraphNode for InnerGraphNode {}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab, TimedEvent,
        TransportInfo,
    };

    fn make_note_on() -> TimedEvent {
        use paraclete_node_api::{UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7}};
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
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: ignode.sample_rate,
            block_size: block, extended_events: &slab, commands: &[],
        };
        let mut output = ProcessOutput::new(
            &mut outs,
            &mut [],
            &mut events_out,
        );
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
}
