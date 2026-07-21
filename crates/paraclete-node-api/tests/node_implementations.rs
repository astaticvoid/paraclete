/// Integration tests for the Node trait.
///
/// These tests double as documentation — each shows a contributor exactly what
/// they need to implement a node at a given level of engagement (per ADR-008).
use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, EventOutputBuffer, ExtendedEventSlab, Node,
    ParamDescriptor, ParamUnit, PortDescriptor, PortDirection, PortName, PortType, ProcessInput,
    ProcessOutput, TransportInfo,
};

// ── Level 1 node — minimum possible implementation ────────────────────────────

/// A Level 1 node implements only `ports()` and `process()`.
/// Everything else is provided by default. This is the VCV Rack / Bitwig Grid
/// model — an oscillator, filter, or math module needs nothing more.
struct Level1SilenceNode {
    ports: [PortDescriptor; 1],
}

impl Level1SilenceNode {
    const PORT_AUDIO_OUT: u32 = 0;

    fn new() -> Self {
        Self {
            ports: [PortDescriptor {
                id: Self::PORT_AUDIO_OUT,
                name: "audio_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            }],
        }
    }
}

impl Node for Level1SilenceNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn process(&mut self, _input: &ProcessInput, _output: &mut ProcessOutput) {
        // Output buffers are pre-zeroed by the executor. Silence needs no work.
    }
}

#[test]
fn test_level1_node_minimum_implementation() {
    let mut node = Level1SilenceNode::new();

    assert_eq!(node.ports().len(), 1);
    assert_eq!(node.ports()[0].id, Level1SilenceNode::PORT_AUDIO_OUT);
    assert_eq!(node.ports()[0].name.as_str(), "audio_out");

    assert!(!node.capabilities_changed());

    let doc = node.capability_document();
    assert_eq!(doc.ports.len(), 1);
    assert!(doc.params.is_empty());

    let agreement = node.negotiate(&doc);
    assert_eq!(agreement.sample_rate, 44100.0);

    node.activate(44100.0, 512);
    node.deactivate();

    assert!(node.serialize().is_empty());
}

// ── Level 2 node — instrument with parameters ─────────────────────────────────

struct Level2GainNode {
    ports: [PortDescriptor; 2],
    gain: f32,
}

impl Level2GainNode {
    const PORT_AUDIO_IN: u32 = 0;
    const PORT_AUDIO_OUT: u32 = 1;
    const PARAM_GAIN: &'static str = "gain";

    fn new() -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_AUDIO_IN,
                    name: "audio_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Audio,
                },
                PortDescriptor {
                    id: Self::PORT_AUDIO_OUT,
                    name: "audio_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Audio,
                },
            ],
            gain: 1.0,
        }
    }
}

impl Node for Level2GainNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn process(&mut self, _input: &ProcessInput, _output: &mut ProcessOutput) {}

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "GainNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: self.ports.to_vec(),
            params: vec![ParamDescriptor {
                id: ParamDescriptor::id_for_name(Self::PARAM_GAIN),
                name: PortName::Static(Self::PARAM_GAIN),
                min: 0.0,
                max: 2.0,
                default: 1.0,
                stepped: false,
                unit: ParamUnit::Generic,
                display: None,
            }],
            extensions: vec![],
            view: None,
        }
    }

    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {
        self.gain = 1.0;
    }

    fn serialize(&self) -> Vec<u8> {
        self.gain.to_le_bytes().to_vec()
    }

    fn deserialize(&mut self, data: &[u8]) {
        if data.len() >= 4 {
            self.gain = f32::from_le_bytes(data[..4].try_into().unwrap());
        }
    }
}

#[test]
fn test_level2_node_declares_parameters_in_capability_document() {
    let node = Level2GainNode::new();
    let doc = node.capability_document();

    assert_eq!(doc.name, "GainNode");
    assert_eq!(doc.params.len(), 1);

    let param = &doc.params[0];
    assert_eq!(param.name.as_str(), "gain");
    assert_eq!(param.min, 0.0);
    assert_eq!(param.max, 2.0);
    assert_eq!(param.default, 1.0);
    assert_eq!(param.id, ParamDescriptor::id_for_name("gain"));
}

#[test]
fn test_level2_node_serialize_deserialize_round_trips() {
    let mut node = Level2GainNode::new();
    node.gain = 0.75;

    let data = node.serialize();
    assert_eq!(data.len(), 4);

    let mut restored = Level2GainNode::new();
    restored.deserialize(&data);
    assert_eq!(restored.gain, 0.75);
}

// ── Level 3 node — smart node with capability negotiation ────────────────────

struct Level3NegotiatingNode {
    ports: [PortDescriptor; 1],
    agreed_sample_rate: Option<f32>,
}

impl Level3NegotiatingNode {
    fn new() -> Self {
        Self {
            ports: [PortDescriptor {
                id: 0,
                name: "event_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            agreed_sample_rate: None,
        }
    }
}

impl Node for Level3NegotiatingNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn process(&mut self, _input: &ProcessInput, _output: &mut ProcessOutput) {}

    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        let agreement = ConnectionAgreement::baseline(44100.0, 512);
        self.agreed_sample_rate = Some(agreement.sample_rate);
        agreement
    }
}

#[test]
fn test_level3_node_negotiate_is_called_at_connection_time() {
    let mut node = Level3NegotiatingNode::new();
    assert!(node.agreed_sample_rate.is_none());

    let their_doc = CapabilityDocument::from_ports(&[]);
    let agreement = node.negotiate(&their_doc);

    assert!(node.agreed_sample_rate.is_some());
    assert_eq!(agreement.sample_rate, 44100.0);
}

// ── process() receives correct context ───────────────────────────────────────

struct ContextInspectingNode {
    ports: Vec<PortDescriptor>,
    last_sample_rate: f32,
    last_block_size: usize,
    process_call_count: u32,
}

impl ContextInspectingNode {
    fn new() -> Self {
        Self {
            ports: vec![],
            last_sample_rate: 0.0,
            last_block_size: 0,
            process_call_count: 0,
        }
    }
}

impl Node for ContextInspectingNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        self.last_sample_rate = input.sample_rate;
        self.last_block_size = input.block_size;
        self.process_call_count += 1;
    }
}

#[test]
fn test_node_process_receives_sample_rate_and_block_size() {
    let mut node = ContextInspectingNode::new();
    let transport = TransportInfo::default();
    let slab = ExtendedEventSlab::empty();
    let mut events_out = EventOutputBuffer::new(16);

    let input = ProcessInput {
        audio_inputs: &[],
        signal_inputs: &[],
        events: &[],
        transport: &transport,
        sample_rate: 48000.0,
        block_size: 256,
        extended_events: &slab,
        commands: &[],
    };

    let mut output = ProcessOutput::new(&mut [], &mut [], &mut events_out);

    node.process(&input, &mut output);

    assert_eq!(node.last_sample_rate, 48000.0);
    assert_eq!(node.last_block_size, 256);
    assert_eq!(node.process_call_count, 1);
}

#[test]
fn silence_buffer_at_max_block_size_is_all_zeros() {
    use paraclete_node_api::{ExtendedEventSlab, ProcessInput, TransportInfo};
    let transport = TransportInfo::default();
    let slab = ExtendedEventSlab::empty();
    let input = ProcessInput {
        audio_inputs: &[],
        signal_inputs: &[],
        events: &[],
        transport: &transport,
        sample_rate: 44100.0,
        block_size: 65536,
        extended_events: &slab,
        commands: &[],
    };
    let silence = input.logic(999);
    assert_eq!(silence.len(), 65536);
    assert!(silence.iter().all(|&s| s == 0.0));
}
