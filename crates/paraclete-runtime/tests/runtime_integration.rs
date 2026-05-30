/// Integration tests for the runtime — NodeConfigurator, NodeExecutor, ADR-005 cycle
/// rejection, and P1 event routing.
use std::sync::{Arc, Mutex, atomic::{AtomicU32, Ordering}};

use paraclete_node_api::{
    Event, HardwareDevice, HardwareEvent, HardwareOutput, StateBusValue, Node,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
    SurfaceDescriptor, TimedEvent,
};
use paraclete_runtime::{ConfigMessage, NodeConfigurator};

// ── Test node helpers ─────────────────────────────────────────────────────────

struct CountingNode {
    ports: Vec<PortDescriptor>,
    call_count: Arc<AtomicU32>,
}

impl CountingNode {
    fn new(call_count: Arc<AtomicU32>) -> Self {
        Self { ports: vec![], call_count }
    }
}

impl Node for CountingNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, _output: &mut ProcessOutput) {
        self.call_count.fetch_add(1, Ordering::Relaxed);
    }
}

struct ConstantOutputNode {
    ports: [PortDescriptor; 1],
    value: f32,
}

impl ConstantOutputNode {
    fn new(value: f32) -> Self {
        Self {
            ports: [PortDescriptor {
                id: 0,
                name: "audio_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            }],
            value,
        }
    }
}

impl Node for ConstantOutputNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        if let Some(buf) = output.audio_outputs.first_mut() {
            for ch in 0..buf.channels() {
                buf.channel_mut(ch).fill(self.value);
            }
        }
        let _ = input;
    }
}

// ── NodeConfigurator ─────────────────────────────────────────────────────────────────

#[test]
fn configurator_can_register_a_node_and_build_executor() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    let _exec = conf.build_executor();
}

/// ADR-005: connecting A → B → A must be rejected.
#[test]
fn configurator_connect_rejects_two_node_cycle() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    conf.add_node(2, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));

    assert!(conf.connect(1, 0, 2, 0).is_ok());
    assert!(conf.connect(2, 0, 1, 0).is_err());
}

#[test]
fn configurator_connect_rejects_self_loop() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    assert!(conf.connect(1, 0, 1, 0).is_err());
}

#[test]
fn configurator_connect_accepts_valid_dag() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    conf.add_node(2, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    conf.add_node(3, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));

    assert!(conf.connect(1, 0, 2, 0).is_ok());
    assert!(conf.connect(2, 0, 3, 0).is_ok());
}

// ── NodeExecutor ──────────────────────────────────────────────────────────────

#[test]
fn executor_calls_process_once_per_cycle() {
    let call_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(call_count.clone())));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 512 * 2];
    exec.process(&mut out, 2);

    assert_eq!(call_count.load(Ordering::Relaxed), 1);
}

#[test]
fn executor_calls_process_once_per_cycle_across_multiple_cycles() {
    let call_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(call_count.clone())));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 512 * 2];
    for _ in 0..4 {
        exec.process(&mut out, 2);
    }

    assert_eq!(call_count.load(Ordering::Relaxed), 4);
}

#[test]
fn executor_output_is_the_sum_of_node_audio_outputs() {
    let mut conf = NodeConfigurator::new(44100.0, 8);
    conf.add_node(1, Box::new(ConstantOutputNode::new(0.5)));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 8 * 2];
    exec.process(&mut out, 2);

    assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
}

#[test]
fn executor_output_sums_multiple_nodes() {
    let mut conf = NodeConfigurator::new(44100.0, 8);
    conf.add_node(1, Box::new(ConstantOutputNode::new(0.25)));
    conf.add_node(2, Box::new(ConstantOutputNode::new(0.25)));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 8 * 2];
    exec.process(&mut out, 2);

    assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
}

// ── Ring buffer message delivery ──────────────────────────────────────────────

#[test]
fn configurator_send_bpm_change_is_applied_by_executor() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    let mut exec = conf.build_executor();

    assert_eq!(exec.transport().bpm, 120.0);
    assert!(conf.send(ConfigMessage::SetBpm(140.0)));

    let mut out = vec![0.0f32; 512 * 2];
    exec.process(&mut out, 2);

    assert_eq!(exec.transport().bpm, 140.0);
}

#[test]
fn configurator_send_playing_change_is_applied_by_executor() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    let mut exec = conf.build_executor();

    assert!(!exec.transport().playing);
    assert!(conf.send(ConfigMessage::SetPlaying(true)));

    let mut out = vec![0.0f32; 512 * 2];
    exec.process(&mut out, 2);

    assert!(exec.transport().playing);
}

// ── P1: Event routing ─────────────────────────────────────────────────────────

struct EventCapturingNode {
    ports: Vec<PortDescriptor>,
    received: Arc<Mutex<Vec<Event>>>,
}

impl EventCapturingNode {
    fn new(received: Arc<Mutex<Vec<Event>>>, direction: PortDirection) -> Self {
        Self {
            ports: vec![PortDescriptor {
                id: 0,
                name: "events".into(),
                direction,
                port_type: PortType::Event,
            }],
            received,
        }
    }
}

impl Node for EventCapturingNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        let mut guard = self.received.lock().unwrap();
        for timed in input.events {
            guard.push(timed.event);
        }
    }
}

struct EventEmittingNode {
    ports: Vec<PortDescriptor>,
    event: Option<HardwareEvent>,
}

impl EventEmittingNode {
    fn new(event: HardwareEvent) -> Self {
        Self {
            ports: vec![PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            event: Some(event),
        }
    }
}

impl Node for EventEmittingNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        if let Some(event) = self.event {
            output.events_out.push(TimedEvent::new(0, Event::Hardware(event)));
        }
    }
}

struct TestHardwareDevice {
    ports: Vec<PortDescriptor>,
    surface: SurfaceDescriptor,
    update_count: Arc<AtomicU32>,
}

impl TestHardwareDevice {
    fn new(update_count: Arc<AtomicU32>) -> Self {
        Self {
            ports: vec![],
            surface: SurfaceDescriptor { name: "Test", vendor: "Test", controls: vec![] },
            update_count,
        }
    }
}

impl Node for TestHardwareDevice {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _: &ProcessInput, _: &mut ProcessOutput) {}
}

impl HardwareDevice for TestHardwareDevice {
    fn surface(&self) -> &SurfaceDescriptor { &self.surface }
    fn update_output(&mut self, _: &HardwareOutput) {
        self.update_count.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn event_routing_delivers_events_from_upstream_to_downstream_node() {
    let received = Arc::new(Mutex::new(vec![]));
    let hw_event = HardwareEvent::PadPressed { id: 7, velocity: 100, pressure: 0 };

    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(EventEmittingNode::new(hw_event)));
    conf.add_node(2, Box::new(EventCapturingNode::new(received.clone(), PortDirection::Input)));
    conf.connect(1, 0, 2, 0).unwrap();
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    let events = received.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], Event::Hardware(HardwareEvent::PadPressed { id: 7, .. })));
}

#[test]
fn disconnected_nodes_receive_empty_event_slice() {
    let received = Arc::new(Mutex::new(vec![]));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(EventCapturingNode::new(received.clone(), PortDirection::Input)));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn add_hardware_device_registers_device_for_output_callbacks() {
    let update_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_hardware_device(1, Box::new(TestHardwareDevice::new(update_count.clone())));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    assert_eq!(update_count.load(Ordering::Relaxed), 1);
}

// ── P2: StateBus ──────────────────────────────────────────────────────────────

/// A node that publishes a value to the StateBus on every cycle.
struct PublishingNode {
    ports: Vec<PortDescriptor>,
    key: String,
}

impl PublishingNode {
    fn new(key: &str) -> Self {
        Self { ports: vec![], key: key.to_string() }
    }
}

impl Node for PublishingNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _: &ProcessInput, _: &mut ProcessOutput) {}
    fn published_state(&self) -> Vec<(String, StateBusValue)> {
        vec![(self.key.clone(), StateBusValue::Int(42))]
    }
}

#[test]
fn state_bus_values_published_by_melos_appear_in_snapshot() {
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(PublishingNode::new("/bus/test/value")));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    // Drain the SPSC so the main-thread handle is up to date.
    conf.process_state_bus();

    let val = conf.state_bus_read("/bus/test/value");
    assert_eq!(val, Some(StateBusValue::Int(42)));
}

#[test]
fn state_bus_subscription_detects_first_change() {
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(PublishingNode::new("/bus/sub/value")));
    let mut exec = conf.build_executor();

    let mut sub = conf.state_bus_subscribe("/bus/sub/value");

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    // Drain the SPSC before polling the subscription.
    conf.process_state_bus();

    let changed = conf.state_bus_poll_subscription(&mut sub);
    assert!(changed.is_some(), "expected subscription to detect change");
}

#[test]
fn add_tempo_source_returns_domain_id_and_node_participates_in_graph() {
    let mut conf = NodeConfigurator::new(44100.0, 64);
    let node = Box::new(CountingNode::new(Arc::new(AtomicU32::new(0))));
    let (_node_id, domain_id) = conf.add_tempo_source(10, node);
    assert_eq!(domain_id, 0); // first registered domain gets id 0

    // A second tempo source gets a distinct domain id.
    let melos2 = Box::new(CountingNode::new(Arc::new(AtomicU32::new(0))));
    let (_id2, domain_id2) = conf.add_tempo_source(11, melos2);
    assert_eq!(domain_id2, 1);
}
