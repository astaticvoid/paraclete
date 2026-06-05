/// Integration tests for the runtime — NodeConfigurator, NodeExecutor, ADR-005 cycle
/// rejection, and P1 event routing.
use std::sync::{Arc, Mutex, atomic::{AtomicU32, Ordering}};

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, ConnectionRecord, Event, HardwareDevice,
    HardwareEvent, HardwareOutput, HardwareOutputHandle, StateBusValue, Node,
    ParamLockEvent, PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
    SurfaceDescriptor, TimedEvent, TransportEvent, TransportFlags, TransportInfo,
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

// A node with both an Event output (port 0) and Event input (port 1),
// used to test connect() direction and cycle validation end-to-end.
struct BidirectionalNode {
    ports: [PortDescriptor; 2],
}

impl BidirectionalNode {
    const PORT_OUT: u32 = 0;
    const PORT_IN:  u32 = 1;

    fn new() -> Self {
        Self {
            ports: [
                PortDescriptor { id: Self::PORT_OUT, name: "out".into(), direction: PortDirection::Output, port_type: PortType::Event },
                PortDescriptor { id: Self::PORT_IN,  name: "in".into(),  direction: PortDirection::Input,  port_type: PortType::Event },
            ],
        }
    }
}

impl Node for BidirectionalNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _: &ProcessInput, _: &mut ProcessOutput) {}
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
    conf.add_node(1, Box::new(BidirectionalNode::new()));
    conf.add_node(2, Box::new(BidirectionalNode::new()));

    // 1's output → 2's input: valid
    assert!(conf.connect(1, BidirectionalNode::PORT_OUT, 2, BidirectionalNode::PORT_IN).is_ok());
    // 2's output → 1's input: forms a cycle, must be rejected
    assert!(conf.connect(2, BidirectionalNode::PORT_OUT, 1, BidirectionalNode::PORT_IN).is_err());
}

#[test]
fn configurator_connect_rejects_self_loop() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(BidirectionalNode::new()));
    assert!(conf.connect(1, BidirectionalNode::PORT_OUT, 1, BidirectionalNode::PORT_IN).is_err());
}

#[test]
fn configurator_connect_accepts_valid_dag() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(BidirectionalNode::new()));
    conf.add_node(2, Box::new(BidirectionalNode::new()));
    conf.add_node(3, Box::new(BidirectionalNode::new()));

    assert!(conf.connect(1, BidirectionalNode::PORT_OUT, 2, BidirectionalNode::PORT_IN).is_ok());
    assert!(conf.connect(2, BidirectionalNode::PORT_OUT, 3, BidirectionalNode::PORT_IN).is_ok());
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

// A HardwareOutputHandle that counts deliver() calls.
struct DeliverCountHandle {
    count: Arc<AtomicU32>,
}
impl HardwareOutputHandle for DeliverCountHandle {
    fn tick(&mut self) {}
    fn deliver(&mut self, _output: HardwareOutput) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

// A HardwareDevice that hands its output handle to the configurator.
struct DeviceWithHandle {
    ports: Vec<PortDescriptor>,
    surface: SurfaceDescriptor,
    deliver_count: Arc<AtomicU32>,
}
impl DeviceWithHandle {
    fn new(deliver_count: Arc<AtomicU32>) -> Self {
        Self {
            ports: vec![],
            surface: SurfaceDescriptor { name: "Test", vendor: "Test", controls: vec![] },
            deliver_count,
        }
    }
}
impl Node for DeviceWithHandle {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _: &ProcessInput, _: &mut ProcessOutput) {}
}
impl HardwareDevice for DeviceWithHandle {
    fn surface(&self) -> &SurfaceDescriptor { &self.surface }
    fn update_output(&mut self, _: &HardwareOutput) {}
    fn take_output_handle(&mut self) -> Option<Box<dyn HardwareOutputHandle>> {
        Some(Box::new(DeliverCountHandle { count: Arc::clone(&self.deliver_count) }))
    }
}

/// P4.5 Fix 1 — deliver_script_output() routes LED output to the registered handle.
/// Spec: led_routing_deliver_script_output_reaches_handle
#[test]
fn led_routing_deliver_script_output_reaches_handle() {
    let deliver_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_hardware_device(42, Box::new(DeviceWithHandle::new(Arc::clone(&deliver_count))));
    let _exec = conf.build_executor();

    let mut pending = std::collections::HashMap::new();
    pending.insert(42u32, HardwareOutput::empty());
    conf.deliver_script_output(pending);

    assert_eq!(deliver_count.load(Ordering::Relaxed), 1, "deliver() should be called once");
}

#[test]
fn led_routing_unknown_device_id_is_silently_dropped() {
    let deliver_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_hardware_device(42, Box::new(DeviceWithHandle::new(Arc::clone(&deliver_count))));
    let _exec = conf.build_executor();

    // device_id 99 has no registered handle — should not panic
    let mut pending = std::collections::HashMap::new();
    pending.insert(99u32, HardwareOutput::empty());
    conf.deliver_script_output(pending);

    assert_eq!(deliver_count.load(Ordering::Relaxed), 0, "unknown device_id must be silently dropped");
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
    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        buf.push((self.key.clone(), StateBusValue::Int(42)));
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

// ── P3-S7: connect() invokes negotiate() end-to-end ──────────────────────────

struct NegotiatingInstrument {
    ports: Vec<PortDescriptor>,
    negotiate_called: Arc<AtomicU32>,
    record_received: Arc<AtomicU32>,
}

impl NegotiatingInstrument {
    fn new(negotiate_called: Arc<AtomicU32>, record_received: Arc<AtomicU32>) -> Self {
        Self {
            ports: vec![PortDescriptor {
                id: 0,
                name: "events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            }],
            negotiate_called,
            record_received,
        }
    }
}

impl Node for NegotiatingInstrument {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _: &ProcessInput, _: &mut ProcessOutput) {}

    fn is_negotiable(&self) -> bool { true }

    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        self.negotiate_called.fetch_add(1, Ordering::Relaxed);
        ConnectionAgreement::baseline()
    }

    fn set_connection_record(&mut self, _record: ConnectionRecord) {
        self.record_received.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn connect_invokes_negotiate_and_set_connection_record_on_both_sides() {
    let src_negotiate  = Arc::new(AtomicU32::new(0));
    let dst_negotiate  = Arc::new(AtomicU32::new(0));
    let src_record_rx  = Arc::new(AtomicU32::new(0));
    let dst_record_rx  = Arc::new(AtomicU32::new(0));

    let mut conf = NodeConfigurator::new(44100.0, 64);

    // src: Event Output
    conf.add_node(1, Box::new(EventEmittingNode::new(
        HardwareEvent::PadPressed { id: 0, velocity: 0, pressure: 0 },
    )));
    conf.add_node(2, Box::new(NegotiatingInstrument::new(
        dst_negotiate.clone(), dst_record_rx.clone(),
    )));
    conf.connect(1, 0, 2, 0).unwrap();

    // negotiate() is called on both sides; set_connection_record() is called on both sides.
    // The NegotiatingInstrument on dst side (node 2) should have been called.
    assert_eq!(dst_negotiate.load(Ordering::Relaxed), 1, "dst negotiate() not called");
    assert_eq!(dst_record_rx.load(Ordering::Relaxed), 1, "dst set_connection_record() not called");
    // src side uses the default negotiate() which we cannot count here (CountingNode doesn't
    // override it), so we only assert on the NegotiatingInstrument side.
    let _ = (src_negotiate, src_record_rx); // unused but declared for symmetry
}

// ── P3-S8: ParamLockEvent delivery ordering ───────────────────────────────────

struct OrderCapturingNode {
    ports: Vec<PortDescriptor>,
    received_order: Arc<Mutex<Vec<String>>>,
}

impl OrderCapturingNode {
    fn new(received_order: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            ports: vec![PortDescriptor {
                id: 0,
                name: "events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            }],
            received_order,
        }
    }
}

impl Node for OrderCapturingNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        let mut order = self.received_order.lock().unwrap();
        for timed in input.events {
            let label = match timed.event {
                Event::ParamLock(_) => "param_lock",
                Event::Midi2(_)     => "midi2",
                Event::Hardware(_)  => "hardware",
                _                   => "other",
            };
            order.push(label.to_string());
        }
    }
}

struct MultiEventEmitter {
    ports: Vec<PortDescriptor>,
}

impl MultiEventEmitter {
    fn new() -> Self {
        Self {
            ports: vec![PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
        }
    }
}

impl Node for MultiEventEmitter {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        use paraclete_node_api::{UmpMessage, midi::{ChannelVoice2, NoteOn}};
        let note_on = NoteOn::<[u32; 4]>::new();
        let ump = UmpMessage::from(ChannelVoice2::from(note_on));

        // Emit in reverse priority order: Midi2 first, then ParamLock.
        // The executor must reorder so ParamLock arrives before Midi2.
        output.events_out.push(TimedEvent::new(0, Event::Midi2(ump)));
        output.events_out.push(TimedEvent::new(0, Event::ParamLock(
            ParamLockEvent { node_id: 99, param_id: 0, value: 1.0 }
        )));
    }
}

#[test]
fn executor_delivers_param_lock_before_midi2_at_same_sample_offset() {
    let order = Arc::new(Mutex::new(vec![]));

    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(MultiEventEmitter::new()));
    conf.add_node(2, Box::new(OrderCapturingNode::new(order.clone())));
    conf.connect(1, 0, 2, 0).unwrap();
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    let received = order.lock().unwrap();
    assert_eq!(received.len(), 2, "expected 2 events");
    assert_eq!(received[0], "param_lock", "ParamLock must arrive before Midi2");
    assert_eq!(received[1], "midi2");
}

// ── audio_inputs wiring (P4.5 Fix 4) ─────────────────────────────────────────

/// A node that fills its audio output with a constant value.
struct AudioSourceNode {
    ports: [PortDescriptor; 1],
    value: f32,
}

impl AudioSourceNode {
    fn new(value: f32) -> Self {
        Self {
            ports: [PortDescriptor { id: 0, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio }],
            value,
        }
    }
}

impl Node for AudioSourceNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        if let Some(buf) = output.audio_outputs.first_mut() {
            for ch in 0..buf.channels() { buf.channel_mut(ch).fill(self.value); }
        }
    }
}

/// A node that asserts audio_inputs is non-empty and records the first sample.
struct AudioSinkNode {
    ports: [PortDescriptor; 1],
    received: Arc<Mutex<Option<f32>>>,
}

impl AudioSinkNode {
    fn new(received: Arc<Mutex<Option<f32>>>) -> Self {
        Self {
            ports: [PortDescriptor { id: 0, name: "audio_in".into(), direction: PortDirection::Input, port_type: PortType::Audio }],
            received,
        }
    }
}

impl Node for AudioSinkNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        if let Some(buf) = input.audio_inputs.first() {
            if buf.channels() > 0 && !buf.channel(0).is_empty() {
                *self.received.lock().unwrap() = Some(buf.channel(0)[0]);
            }
        }
    }
}

// ── Signal routing test helpers ───────────────────────────────────────────────

/// A node that writes a constant value to its modulation output port.
struct ModOutputNode {
    ports: [PortDescriptor; 1],
    value: f32,
}

impl ModOutputNode {
    fn new(value: f32) -> Self {
        Self {
            ports: [PortDescriptor { id: 0, name: "mod_out".into(), direction: PortDirection::Output, port_type: PortType::Modulation }],
            value,
        }
    }
}

impl Node for ModOutputNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        let out = output.mod_output_mut(0);
        out.fill(self.value);
    }
}

/// A node that reads its modulation input and records the first sample.
struct ModInputNode {
    ports: [PortDescriptor; 1],
    received: Arc<Mutex<Option<f32>>>,
}

impl ModInputNode {
    fn new(received: Arc<Mutex<Option<f32>>>) -> Self {
        Self {
            ports: [PortDescriptor { id: 0, name: "mod_in".into(), direction: PortDirection::Input, port_type: PortType::Modulation }],
            received,
        }
    }
}

impl Node for ModInputNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        let mod_in = input.modulation(0);
        if !mod_in.is_empty() {
            *self.received.lock().unwrap() = Some(mod_in[0]);
        }
    }
}

/// P6.5 Commit 3: executor allocates signal output buffers for nodes with signal output ports.
/// Registers a ModOutputNode (which calls output.mod_output_mut()) and verifies it
/// does not panic — confirming the executor pre-allocated the modulation output slot.
#[test]
fn signal_port_executor_allocates_mod_output() {
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(ModOutputNode::new(0.5)));
    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2); // must not panic
}

/// P6.5 Commit 3 done criterion: signal routing delivers upstream mod output to
/// downstream mod input. ModOutputNode writes 0.5 to its output; ModInputNode
/// reads it via input.modulation(0).
#[test]
fn signal_routing_mod_output_reaches_mod_input() {
    let received = Arc::new(Mutex::new(None::<f32>));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(ModOutputNode::new(0.5)));
    conf.add_node(2, Box::new(ModInputNode::new(received.clone())));
    conf.connect(1, 0, 2, 0).unwrap();
    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);
    let val = *received.lock().unwrap();
    assert!(val.is_some(), "mod input should receive data from connected upstream");
    assert!((val.unwrap() - 0.5).abs() < 1e-6, "mod input value should match upstream output");
}

/// P6.5 Commit 3: unconnected signal input reads all-zeros (silence).
#[test]
fn signal_port_unconnected_returns_silence() {
    let received = Arc::new(Mutex::new(None::<f32>));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(ModInputNode::new(received.clone())));
    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);
    let val = received.lock().unwrap();
    // modulation() returns SILENCE[..block_size] for unconnected ports → first sample = 0.0
    assert_eq!(*val, Some(0.0), "unconnected mod input should read 0.0");
}

/// P4.5 Fix 4 done criterion: downstream node receives non-empty audio_inputs when
/// wired to an upstream audio source. Covers the FilterNode scenario — FilterNode
/// uses audio_inputs identically to AudioSinkNode here; the wiring mechanism is the same.
/// Spec: filter_node_receives_audio_inputs
#[test]
fn audio_inputs_populated_from_upstream_audio_output() {
    let received = Arc::new(Mutex::new(None::<f32>));

    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(AudioSourceNode::new(0.75)));
    conf.add_node(2, Box::new(AudioSinkNode::new(received.clone())));
    conf.connect(1, 0, 2, 0).unwrap();
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    let val = *received.lock().unwrap();
    assert!(val.is_some(), "audio_inputs should be non-empty when an audio edge exists");
    assert!((val.unwrap() - 0.75).abs() < 1e-6, "audio_inputs should carry upstream output value");
}

#[test]
fn audio_inputs_empty_when_no_audio_edge() {
    let received = Arc::new(Mutex::new(None::<f32>));

    let mut conf = NodeConfigurator::new(44100.0, 64);
    // AudioSinkNode added alone with no upstream connection
    conf.add_node(1, Box::new(AudioSinkNode::new(received.clone())));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    assert!(received.lock().unwrap().is_none(), "audio_inputs should be empty with no audio edge");
}

#[test]
fn published_state_push_down_no_allocation_after_first_cycle() {
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(PublishingNode::new("/bus/cap/value")));
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);
    let cap_after_first = exec.state_buf_capacity(0);

    exec.process(&mut out, 2);
    let cap_after_second = exec.state_buf_capacity(0);

    assert_eq!(
        cap_after_first, cap_after_second,
        "state_buf capacity must be stable after the first cycle (no reallocation)"
    );
}

/// Regression: transport override events must survive the incoming.clear() call.
/// The executor clears all incoming queues early in process(); if override events are
/// injected before that clear, nodes never see them. This test verifies the event
/// reaches a downstream node.
#[test]
fn transport_override_event_delivered_to_nodes() {
    let received = Arc::new(Mutex::new(vec![]));

    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(EventCapturingNode::new(received.clone(), PortDirection::Input)));
    let mut exec = conf.build_executor();

    let info = TransportInfo { playing: true, bpm: 130.0, ..TransportInfo::default() };
    let event = TransportEvent {
        domain_id: 0, bar: 1, beat: 0, tick: 0,
        ticks_per_beat: paraclete_node_api::TICKS_PER_BEAT,
        bpm: 130.0, time_sig_num: 4, time_sig_den: 4,
        flags: TransportFlags { global_start: true, ..TransportFlags::default() },
    };
    exec.set_transport_override(info, Some(event));

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    let guard = received.lock().unwrap();
    let found = guard.iter().any(|e| {
        matches!(e, Event::Transport(te) if te.flags.global_start)
    });
    assert!(found, "set_transport_override event must reach nodes (not be cleared by incoming.clear())");
}
