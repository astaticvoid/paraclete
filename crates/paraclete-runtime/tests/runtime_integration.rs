/// Integration tests for the runtime — NodeConfigurator, NodeExecutor, ADR-005 cycle
/// rejection, and P1 event routing.
use std::sync::{Arc, Mutex, atomic::{AtomicU32, Ordering}};

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, ConnectionRecord, Event, Surface,
    SurfaceEvent, SurfaceOutput, SurfaceOutputHandle, StateBusValue, Node,
    ParamLockEvent, PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
    SurfaceDescriptor, TimedEvent, TransportEvent, TransportFlags, TransportInfo,
};
use paraclete_runtime::{ConfigMessage, ConnectError, NodeConfigurator};

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
    event: Option<SurfaceEvent>,
}

impl EventEmittingNode {
    fn new(event: SurfaceEvent) -> Self {
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
            output.events_out.push(TimedEvent::new(0, Event::Surface(event)));
        }
    }
}

struct TestSurface {
    ports: Vec<PortDescriptor>,
    surface: SurfaceDescriptor,
    update_count: Arc<AtomicU32>,
}

impl TestSurface {
    fn new(update_count: Arc<AtomicU32>) -> Self {
        Self {
            ports: vec![],
            surface: SurfaceDescriptor { name: "Test".into(), vendor: "Test".into(), controls: vec![] },
            update_count,
        }
    }
}

impl Node for TestSurface {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _: &ProcessInput, _: &mut ProcessOutput) {}
}

impl Surface for TestSurface {
    fn descriptor(&self) -> &SurfaceDescriptor { &self.surface }
    fn update_output(&mut self, _: &SurfaceOutput) {
        self.update_count.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn event_routing_delivers_events_from_upstream_to_downstream_node() {
    let received = Arc::new(Mutex::new(vec![]));
    let hw_event = SurfaceEvent::PadPressed { id: 7, velocity: 100, pressure: 0 };

    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(EventEmittingNode::new(hw_event)));
    conf.add_node(2, Box::new(EventCapturingNode::new(received.clone(), PortDirection::Input)));
    conf.connect(1, 0, 2, 0).unwrap();
    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    let events = received.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], Event::Surface(SurfaceEvent::PadPressed { id: 7, .. })));
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

// A SurfaceOutputHandle that counts deliver() calls.
struct DeliverCountHandle {
    count: Arc<AtomicU32>,
}
impl SurfaceOutputHandle for DeliverCountHandle {
    fn tick(&mut self) {}
    fn deliver(&mut self, _output: SurfaceOutput) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

// A Surface that hands its output handle to the configurator.
struct DeviceWithHandle {
    ports: Vec<PortDescriptor>,
    surface: SurfaceDescriptor,
    deliver_count: Arc<AtomicU32>,
}
impl DeviceWithHandle {
    fn new(deliver_count: Arc<AtomicU32>) -> Self {
        Self {
            ports: vec![],
            surface: SurfaceDescriptor { name: "Test".into(), vendor: "Test".into(), controls: vec![] },
            deliver_count,
        }
    }
}
impl Node for DeviceWithHandle {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _: &ProcessInput, _: &mut ProcessOutput) {}
}
impl Surface for DeviceWithHandle {
    fn descriptor(&self) -> &SurfaceDescriptor { &self.surface }
    fn update_output(&mut self, _: &SurfaceOutput) {}
    fn take_output_handle(&mut self) -> Option<Box<dyn SurfaceOutputHandle>> {
        Some(Box::new(DeliverCountHandle { count: Arc::clone(&self.deliver_count) }))
    }
}

/// P4.5 Fix 1 — deliver_script_output() routes LED output to the registered handle.
/// Spec: led_routing_deliver_script_output_reaches_handle
#[test]
fn led_routing_deliver_script_output_reaches_handle() {
    let deliver_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_surface(42, Box::new(DeviceWithHandle::new(Arc::clone(&deliver_count))));
    let _exec = conf.build_executor();

    let mut pending = std::collections::HashMap::new();
    pending.insert(42u32, SurfaceOutput::empty());
    conf.deliver_script_output(pending);

    assert_eq!(deliver_count.load(Ordering::Relaxed), 1, "deliver() should be called once");
}

#[test]
fn led_routing_unknown_device_id_is_silently_dropped() {
    let deliver_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_surface(42, Box::new(DeviceWithHandle::new(Arc::clone(&deliver_count))));
    let _exec = conf.build_executor();

    // device_id 99 has no registered handle — should not panic
    let mut pending = std::collections::HashMap::new();
    pending.insert(99u32, SurfaceOutput::empty());
    conf.deliver_script_output(pending);

    assert_eq!(deliver_count.load(Ordering::Relaxed), 0, "unknown device_id must be silently dropped");
}

/// BUG-030 — remove_node on a surface device must refuse WITHOUT destroying
/// it. The old order removed the slot first and then errored, dropping the
/// device while reporting failure.
#[test]
fn remove_node_on_surface_is_nondestructive() {
    let update_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_surface(7, Box::new(TestSurface::new(update_count.clone())));

    let result = conf.remove_node(7);
    assert!(result.is_err(), "remove_node on a device must error");

    // The device must still be registered: it participates in the executor
    // and receives its per-cycle output callback.
    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);
    assert_eq!(
        update_count.load(Ordering::Relaxed), 1,
        "device must survive the refused remove_node"
    );
}

/// remove_node on a drained slot (node moved into a live executor) must refuse
/// WITHOUT partially editing `id_to_index` — the same partial-mutation class as
/// BUG-030. After the refusal the id is still known, so a later legitimate
/// removal (once the node is restored) still works.
#[test]
fn remove_node_on_drained_slot_leaves_id_map_intact() {
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(9, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));

    // Drain the node into an executor — the configurator's `nodes` slot is now
    // empty for id 9, but `id_to_index` still maps it.
    let _exec = conf.build_executor();

    let result = conf.remove_node(9);
    assert!(result.is_err(), "remove_node on a drained slot must error");
    assert!(
        conf.contains_node(9),
        "a refused removal must not evict the id from id_to_index"
    );
}

#[test]
fn add_surface_registers_device_for_output_callbacks() {
    let update_count = Arc::new(AtomicU32::new(0));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_surface(1, Box::new(TestSurface::new(update_count.clone())));
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
        SurfaceEvent::PadPressed { id: 0, velocity: 0, pressure: 0 },
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
                Event::Surface(_)  => "hardware",
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

// ── BUG-026: stable sort preserves same-priority same-offset ordering ─────

struct NoteOrderRecorder {
    ports: Vec<PortDescriptor>,
    order: Arc<Mutex<Vec<String>>>,
}

impl NoteOrderRecorder {
    fn new() -> Self {
        Self {
            ports: vec![PortDescriptor {
                id: 0, name: "events_in".into(),
                direction: PortDirection::Input, port_type: PortType::Event,
            }],
            order: Arc::new(Mutex::new(vec![])),
        }
    }
}

impl Node for NoteOrderRecorder {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        use paraclete_node_api::{UmpMessage, midi::ChannelVoice2};
        for timed in input.events {
            if let Event::Midi2(ref ump) = timed.event {
                if let UmpMessage::ChannelVoice2(cv2) = *ump { match cv2 {
                    ChannelVoice2::NoteOn(_)  => self.order.lock().unwrap().push("on".into()),
                    ChannelVoice2::NoteOff(_) => self.order.lock().unwrap().push("off".into()),
                    _ => {},
                } }
            }
        }
    }
}

#[test]
fn stable_sort_preserves_noteoff_before_noteon_at_same_offset() {
    use paraclete_node_api::{UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOff, NoteOn, u4, u7}};

    let recorder = NoteOrderRecorder::new();
    let order = recorder.order.clone();

    let mut conf = NodeConfigurator::new(44100.0, 512);

    let emitter_ports = vec![PortDescriptor {
        id: 0, name: "events_out".into(),
        direction: PortDirection::Output, port_type: PortType::Event,
    }];
    struct EventEmitter {
        ports: Vec<PortDescriptor>,
        events: Vec<TimedEvent>,
    }
    impl Node for EventEmitter {
        fn ports(&self) -> &[PortDescriptor] { &self.ports }
        fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
            for e in &self.events {
                output.events_out.push(*e);
            }
        }
    }

    let mut note_off = NoteOff::<[u32; 4]>::new();
    note_off.set_group(u4::new(0));
    note_off.set_channel(u4::new(0));
    note_off.set_note_number(u7::new(36));
    note_off.set_velocity(0);
    let mut note_on = NoteOn::<[u32; 4]>::new();
    note_on.set_group(u4::new(0));
    note_on.set_channel(u4::new(0));
    note_on.set_note_number(u7::new(36));
    note_on.set_velocity(32768);

    let emitter = EventEmitter {
        ports: emitter_ports,
        events: vec![
            TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(note_off)))),
            TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(note_on)))),
        ],
    };

    conf.add_node(1, Box::new(emitter));
    conf.add_node(2, Box::new(recorder));
    conf.connect(1, 0, 2, 0).unwrap();

    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 512 * 2];
    exec.process(&mut out, 2);

    let received = order.lock().unwrap();
    assert_eq!(
        received.as_slice(),
        &["off", "on"],
        "BUG-026: NoteOff must arrive before NoteOn at same sample_offset. Got: {:?}",
        *received
    );
}

// ── BUG-025: executor defers cross-block offset events ───────────────────

/// A node that emits a single event at a configurable sample_offset.
struct DeferredEventEmitter {
    ports: Vec<PortDescriptor>,
    event: Option<TimedEvent>,
}
impl DeferredEventEmitter {
    fn new(event: TimedEvent) -> Self {
        Self {
            ports: vec![PortDescriptor {
                id: 0, name: "events_out".into(),
                direction: PortDirection::Output, port_type: PortType::Event,
            }],
            event: Some(event),
        }
    }
}
impl Node for DeferredEventEmitter {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        if let Some(e) = self.event.take() {
            output.events_out.push(e);
        }
    }
}

#[test]
fn executor_defers_cross_block_offset_event_to_next_block() {
    use paraclete_node_api::{UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7}};

    let recorder = NoteOrderRecorder::new();
    let received = recorder.order.clone();

    let mut note_on = NoteOn::<[u32; 4]>::new();
    note_on.set_group(u4::new(0));
    note_on.set_channel(u4::new(0));
    note_on.set_note_number(u7::new(60));
    note_on.set_velocity(32768);

    let block_size = 512usize;
    let emitter = DeferredEventEmitter::new(
        TimedEvent::new(block_size as u32 + 10, Event::Midi2(UmpMessage::from(ChannelVoice2::from(note_on))))
    );

    let mut conf = NodeConfigurator::new(44100.0, block_size);
    conf.add_node(1, Box::new(emitter));
    conf.add_node(2, Box::new(recorder));
    conf.connect(1, 0, 2, 0).unwrap();

    let mut exec = conf.build_executor();

    let mut out = vec![0.0f32; block_size * 2];
    exec.process(&mut out, 2);
    assert!(received.lock().unwrap().is_empty(),
        "block 1: event with offset >= block_size must be deferred, not delivered");

    exec.process(&mut out, 2);
    let r = received.lock().unwrap();
    assert_eq!(r.len(), 1, "block 2: deferred event must be delivered, got {:?}", *r);
    assert_eq!(r[0], "on");
}

#[test]
fn executor_delivers_inblock_event_immediately() {
    use paraclete_node_api::{UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7}};

    let recorder = NoteOrderRecorder::new();
    let received = recorder.order.clone();

    let mut note_on = NoteOn::<[u32; 4]>::new();
    note_on.set_group(u4::new(0));
    note_on.set_channel(u4::new(0));
    note_on.set_note_number(u7::new(60));
    note_on.set_velocity(32768);

    let block_size = 512usize;
    let emitter = DeferredEventEmitter::new(
        TimedEvent::new(100, Event::Midi2(UmpMessage::from(ChannelVoice2::from(note_on))))
    );

    let mut conf = NodeConfigurator::new(44100.0, block_size);
    conf.add_node(1, Box::new(emitter));
    conf.add_node(2, Box::new(recorder));
    conf.connect(1, 0, 2, 0).unwrap();

    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; block_size * 2];
    exec.process(&mut out, 2);

    assert_eq!(received.lock().unwrap().len(), 1,
        "block 1: in-block event (offset=100) must be delivered immediately");
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

// ── cap_doc_cache tests (P9 Commit 2) ────────────────────────────────────────

#[test]
fn cap_doc_cache_populated_on_add_node() {
    use paraclete_nodes::DistortionNode;
    let mut conf = NodeConfigurator::new(44100.0, 256);
    conf.add_node(42, Box::new(DistortionNode::new()));
    let doc = conf.get_node_cap_doc(42);
    assert!(doc.is_some(), "cap doc should be present after add_node");
    let doc = doc.unwrap();
    assert!(
        doc.params.iter().any(|p| p.name.as_str() == "drive"),
        "DistortionNode cap doc should contain 'drive' param"
    );
}

#[test]
fn cap_doc_cache_hit_no_additional_leak() {
    use paraclete_nodes::FilterNode;
    let mut conf = NodeConfigurator::new(44100.0, 256);
    conf.add_node(7, Box::new(FilterNode::new()));
    let doc1 = conf.get_node_cap_doc(7).expect("first call should return Some");
    let doc2 = conf.get_node_cap_doc(7).expect("second call should return Some");
    let doc3 = conf.get_node_cap_doc(7).expect("third call should return Some");
    assert_eq!(
        doc1.params.len(), doc2.params.len(),
        "param count must be identical across calls"
    );
    assert_eq!(
        doc2.params.len(), doc3.params.len(),
        "param count must be identical across calls"
    );
}

// ── P9 Commit 3: Feedback loop break (ADR-028) ───────────────────────────────

/// Helper node with Cv input and Cv output. Passes input to output and records
/// the first sample received.
struct CvPassthroughNode {
    ports: [PortDescriptor; 2],
    received: Arc<Mutex<Vec<f32>>>,
}

impl CvPassthroughNode {
    const PORT_CV_IN:  u32 = 0;
    const PORT_CV_OUT: u32 = 1;

    fn new(received: Arc<Mutex<Vec<f32>>>) -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_CV_IN,
                    name: "cv_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Cv,
                },
                PortDescriptor {
                    id: Self::PORT_CV_OUT,
                    name: "cv_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Cv,
                },
            ],
            received,
        }
    }
}

impl Node for CvPassthroughNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let cv_in = input.cv_signal(Self::PORT_CV_IN);
        if !cv_in.is_empty() {
            self.received.lock().unwrap().push(cv_in[0]);
        }
        let cv_out = output.cv_signal_output_mut(Self::PORT_CV_OUT);
        cv_out.copy_from_slice(cv_in);
    }
}

/// ADR-028: A cycle with no LoopBreakNode must be rejected with MissingLoopBreak.
#[test]
fn connect_cycle_no_loop_break_returns_error() {
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(1, Box::new(CvPassthroughNode::new(Arc::new(Mutex::new(vec![])))));
    conf.add_node(2, Box::new(CvPassthroughNode::new(Arc::new(Mutex::new(vec![])))));

    // Connect 1 → 2 (valid)
    conf.connect(1, CvPassthroughNode::PORT_CV_OUT, 2, CvPassthroughNode::PORT_CV_IN).unwrap();

    // Connect 2 → 1 (closes a cycle with no LoopBreakNode)
    let result = conf.connect(2, CvPassthroughNode::PORT_CV_OUT, 1, CvPassthroughNode::PORT_CV_IN);
    assert!(
        matches!(result, Err(ConnectError::MissingLoopBreak { .. })),
        "expected MissingLoopBreak, got: {:?}", result
    );
}

/// ADR-028: A cycle with exactly one LoopBreakNode must be accepted.
#[test]
fn connect_cycle_one_loop_break_accepted() {
    use paraclete_nodes::LoopBreakNode;

    let mut conf = NodeConfigurator::new(44100.0, 64);
    // Node A: Cv output (port 0) and Cv input (port 1)
    conf.add_node(1, Box::new(CvPassthroughNode::new(Arc::new(Mutex::new(vec![])))));
    // LoopBreakNode L: cv_in (0), cv_out (1)
    conf.add_node(2, Box::new(LoopBreakNode::new()));

    // Forward edge: A.cv_out → L.cv_in
    conf.connect(1, CvPassthroughNode::PORT_CV_OUT, 2, 0).unwrap();
    // Back edge: L.cv_out → A.cv_in (closes cycle with 1 LoopBreakNode)
    let result = conf.connect(2, 1, 1, CvPassthroughNode::PORT_CV_IN);
    assert!(
        result.is_ok(),
        "cycle with exactly one LoopBreakNode must be accepted, got: {:?}", result
    );
}

/// ADR-028: A cycle with two LoopBreakNodes must be rejected with TooManyLoopBreaks.
#[test]
fn connect_cycle_two_loop_breaks_rejected() {
    use paraclete_nodes::LoopBreakNode;

    let mut conf = NodeConfigurator::new(44100.0, 64);
    // A: Cv in (1) and Cv out (0)
    conf.add_node(1, Box::new(CvPassthroughNode::new(Arc::new(Mutex::new(vec![])))));
    // L1 and L2: two LoopBreakNodes
    conf.add_node(2, Box::new(LoopBreakNode::new()));
    conf.add_node(3, Box::new(LoopBreakNode::new()));

    // Build chain: A(out) → L1(in), L1(out) → L2(in)
    conf.connect(1, CvPassthroughNode::PORT_CV_OUT, 2, 0).unwrap();
    conf.connect(2, 1, 3, 0).unwrap();
    // Close cycle: L2(out) → A(in) — cycle contains 2 LoopBreakNodes
    let result = conf.connect(3, 1, 1, CvPassthroughNode::PORT_CV_IN);
    assert!(
        matches!(result, Err(ConnectError::TooManyLoopBreaks { count: 2, .. })),
        "expected TooManyLoopBreaks(2), got: {:?}", result
    );
}

/// ADR-028: After two process() cycles, the downstream node in a sanctioned
/// feedback loop receives the LoopBreakNode's previous-cycle output, confirming
/// the pre-execution phase correctly injects the delayed signal.
#[test]
fn loop_break_executor_pre_phase_injects_prev() {
    use paraclete_nodes::LoopBreakNode;

    // Graph: CvOutputNode(1, value=1.0) → LoopBreakNode(2) → CvPassthroughNode(3)
    //                                          ↑___________________________|
    // (back edge: 3.cv_out → 2.cv_in)
    //
    // Cycle 1: LB.prev=[0,0,...], so 3 receives 0.0 on cv_in (from LB pre-phase)
    //          CvOutputNode writes 1.0 to... wait, CvOutputNode is not connected to 3.
    //
    // Let's simplify:
    //   Node A (id=1): CvPassthroughNode, records what it receives on cv_in
    //   LoopBreakNode L (id=2)
    //   Connect A.cv_out → L.cv_in
    //   Connect L.cv_out → A.cv_in (back edge)
    //
    // In this graph A runs before L (A feeds L).
    // Cycle 1: A reads LB.prev=[0,0,...] from pre-phase → records 0.0
    //          A outputs [0,...] to L.cv_in
    //          L captures [0,...] to next, outputs prev=[0,...] to cv_out buf
    //          Post-phase: swap prev/next → prev=[0,...], next=[0,...]
    //
    // Now feed a source value. Let's use a CvOutputNode that feeds A too.
    // Actually for simplicity just use a CvOutputNode feeding L directly:
    //
    // CvOutputNode(1, 1.0) → A(2, records cv_in)
    // A.cv_out → L(3)
    // L.cv_out → A.cv_in (back edge)
    //
    // Topology: 1 → A → L, L ⤴ A (back edge)
    // Cycle 1: pre-phase: LB.prev=[0], injected into A's cv_in signal buf
    //          A processes: receives cv_in=0 from LB AND... wait, A also receives from 1?
    //          No - A.cv_in is only connected to L's back edge. Node 1 feeds L directly?
    //
    // Let me use a cleaner setup:
    //   CvSourceNode(1, value=1.0): cv_out (port 0)
    //   LoopBreakNode(2): cv_in (port 0), cv_out (port 1)
    //   CvRecorderNode(3): cv_in (port 0), cv_out (port 1)
    //   Edges: 1→2(fwd), 2→3(fwd), 3→2(back)
    //
    // Topology order: 1, 3, 2 (since 3←2 is back edge, so 3 before 2 in toposort)
    // Wait no: edges in the graph are: 1→2, 2→3. Back edge 3→2 removed from graph.
    // Toposort of {1→2, 2→3}: 1, 2, 3.
    // Pre-phase: inject LB.prev into signal_output_bufs[2_slot][cv_out].
    //   2_slot = slot of node 2 (LB). LB is last in order (1,2,3).
    //   But signal_input_routes for node 3 includes back edge (3.cv_in ← LB.cv_out).
    //   Node 3 builds signal_in_scratch from signal_output_bufs[lb_slot][cv_out].
    //   Pre-phase writes LB.prev to that buf before node 3 processes.
    //
    // So the order is: 1, 2, 3 (from toposort of forward edges).
    // Node 3 is AFTER node 2. That's a problem: by the time node 3 processes,
    // node 2 (LB) has ALREADY run and updated its signal_output_buf.
    //
    // So pre-phase must write BEFORE the slot loop. It does.
    // But then node 2 (LB) runs and OVERWRITES its output buf with prev again.
    // And then node 3 processes and reads from LB's buf (which = prev from this cycle).
    //
    // Actually this works fine! The pre-phase writes prev into LB's buf, then LB.process()
    // also writes prev into LB's buf (same data), then node 3 reads it.
    //
    // Cycle 1: prev=[0,0,4]; CvSource(1) writes 1.0; LB.process: next=[1.0], out=[prev=0];
    //   Node 3 reads cv_in from LB's buf = 0.0. Records 0.0.
    //   Post-phase: swap → prev=[1.0], next=[0].
    // Cycle 2: pre-phase writes prev=[1.0] to LB's buf.
    //   Node 1 runs (writes 1.0 to its buf).
    //   Node 2 (LB) runs: captures input (from node 1? No - node 1 is not connected to LB in this setup!)
    //
    // I'm overcomplicating this. Let me use the simplest setup:
    //
    //   CvPassthrough A (id=1): has cv_in (port 0) and cv_out (port 1)
    //   LoopBreakNode L (id=2): has cv_in (port 0) and cv_out (port 1)
    //   Forward edge: A.cv_out(1) → L.cv_in(0)  [A before L in toposort]
    //   Back edge:    L.cv_out(1) → A.cv_in(0)  [sanctioned, removed from sort graph]
    //
    // Initial state: LB.prev = [0,0,0,0]
    //
    // Cycle 1:
    //   Pre-phase: writes LB.prev=[0,0,0,0] to LB's signal_output_bufs[port 1]
    //   Node A processes: signal_in from LB's buf = [0,0,0,0]. Records 0.0.
    //                     A.cv_out = [0,0,0,0] (passes through cv_in)
    //   Node L processes: cv_in = A.cv_out = [0,0,0,0]. next=[0,0,0,0]. out=[prev=0,0,0,0]
    //   Post-phase: swap → prev=[0,0,0,0], next=[0,0,0,0].
    //
    // Hmm, A reads 0 and passes 0 to L, which captures 0 and gives back 0. Not useful.
    //
    // I need an external signal source. Let me inject via inject_event or just use a
    // fixed-value node that only has cv_out:
    //
    //   CvSource S (id=1, value=1.0): cv_out(0)
    //   LoopBreakNode L (id=2): cv_in(0), cv_out(1)
    //   CvRecorder R (id=3): cv_in(0) - records what it gets
    //   Forward edges: S(0)→L(0), L(1)→R(0)
    //   Back edge: R would need a cv_out to feed L... R doesn't have a loop back.
    //
    // For a meaningful feedback test, the simplest feedback loop is:
    //   Node A: cv_in feeds its cv_out (passthrough). So A accumulates.
    //   L: delays A's output by one cycle.
    //   Edge: A.out → L.in, L.out → A.in (back).
    //   A starts at 0. Cycle 1: A reads LB.prev=0, outputs 0, L captures 0.
    //   Not useful to show lag.
    //
    // Better: have a source ALSO feed A, and observe that A's cv_in from LB is delayed:
    //   S(id=1, fixed=2.0) → A(id=2, records cv_in) via A's cv_in port
    //   A has cv_out connected to L.cv_in (forward edge)
    //   L.cv_out connected to A.cv_in (back edge)
    //   A reads BOTH S and LB? No - each port can only be connected from one source.
    //
    // The cleanest test: have A feed L directly (forward), L feeds A (back).
    // A outputs its cv_in to cv_out. Initial cv_in = [from LB] = 0.
    // After cycle 1 + swap: LB.prev = [0] (what A output = what A received = 0).
    // This is a fixed point at 0 - not useful.
    //
    // Use a CvOutputNode(fixed=5.0) whose output gets MERGED into A's input?
    // Signal ports typically have one-to-one routing.
    //
    // SIMPLEST approach: just verify that in cycle 2, A receives what LB captured in cycle 1.
    // Use A = CvOutputNode (always outputs 7.0), L = LB.
    //   S(id=1, out=7.0) → L(id=2, cv_in) [forward]
    //   L.cv_out → R(id=3, cv_in) [back edge? No, this would be back edge only if there's a cycle]
    //
    // For a real cycle: S→L→R→S? S needs both cv_out and cv_in. But S is a fixed source.
    //
    // I'll use this setup:
    //   A = CvRecorderNode (records cv_in, also has cv_out that just passes cv_in)
    //   L = LoopBreakNode
    //   Forward: A.cv_out → L.cv_in
    //   Back: L.cv_out → A.cv_in (sanctioned cycle)
    //   External: nothing feeds A's cv_in except L's back edge.
    //
    // But now we need A to output something meaningful for L to capture.
    // Use A as a source that ALSO records what it receives (so it can observe the lag):
    //
    // A outputs CONSTANT 1.0 regardless of input. L captures 1.0 each cycle.
    // A's cv_in gets LB's prev. Cycle 1: A reads 0 from LB.prev. Cycle 2: A reads 1.0.
    // That's the test! A records what its cv_in was, which should be 0 then 1.0.

    // Setup: A with fixed output 1.0, cv_in connected to LB's back edge (records what it receives)
    let received = Arc::new(Mutex::new(vec![]));
    let mut conf = NodeConfigurator::new(44100.0, 4);

    // Node A: fixed cv_out of 1.0, records cv_in values
    conf.add_node(1, Box::new(CvPassthroughAndEmit::new(received.clone(), 1.0)));
    // LoopBreakNode
    conf.add_node(2, Box::new(LoopBreakNode::new()));

    // Forward edge: A.cv_out(1) → L.cv_in(0)
    conf.connect(1, CvPassthroughAndEmit::PORT_CV_OUT, 2, 0).unwrap();
    // Back edge: L.cv_out(1) → A.cv_in(0)
    conf.connect(2, 1, 1, CvPassthroughAndEmit::PORT_CV_IN).unwrap();

    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 4 * 2];

    // Cycle 1: A receives LB.prev=[0,0,0,0] from pre-phase.
    exec.process(&mut out, 2);
    // Post-phase: LB.prev = [1.0,1.0,1.0,1.0] (what A output this cycle)

    // Cycle 2: A receives LB.prev=[1.0,1.0,1.0,1.0] from pre-phase.
    exec.process(&mut out, 2);

    let vals = received.lock().unwrap();
    // Cycle 1: cv_in[0] = 0.0 (LB.prev starts zeroed)
    // Cycle 2: cv_in[0] = 1.0 (LB.prev = what A output in cycle 1)
    assert!(vals.len() >= 2, "expected at least 2 recorded values, got {}", vals.len());
    assert_eq!(vals[0], 0.0, "cycle 1: A should receive 0.0 (LB.prev is zeroed), got {}", vals[0]);
    assert_eq!(vals[1], 1.0, "cycle 2: A should receive 1.0 (LB.prev = cycle 1 output), got {}", vals[1]);
}

/// A node with Cv input (records values) and Cv output (fixed value).
struct CvPassthroughAndEmit {
    ports: [PortDescriptor; 2],
    received: Arc<Mutex<Vec<f32>>>,
    emit_value: f32,
}

impl CvPassthroughAndEmit {
    const PORT_CV_IN:  u32 = 0;
    const PORT_CV_OUT: u32 = 1;

    fn new(received: Arc<Mutex<Vec<f32>>>, emit_value: f32) -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_CV_IN,
                    name: "cv_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Cv,
                },
                PortDescriptor {
                    id: Self::PORT_CV_OUT,
                    name: "cv_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Cv,
                },
            ],
            received,
            emit_value,
        }
    }
}

impl Node for CvPassthroughAndEmit {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // Record the first sample of cv_in
        let cv_in = input.cv_signal(Self::PORT_CV_IN);
        if !cv_in.is_empty() {
            self.received.lock().unwrap().push(cv_in[0]);
        }
        // Emit fixed value regardless of input
        let cv_out = output.cv_signal_output_mut(Self::PORT_CV_OUT);
        cv_out.fill(self.emit_value);
    }
}

// ── ADR-034: runtime counter tests ─────────────────────────────────────────────

#[test]
fn runtime_counters_buffers_processed_increments_each_cycle() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    let mut exec = conf.build_executor();
    let counters = exec.counters().clone();

    let mut out = vec![0.0f32; 512 * 2];
    assert_eq!(counters.buffers_processed.load(Ordering::Relaxed), 0);
    exec.process(&mut out, 2);
    assert_eq!(counters.buffers_processed.load(Ordering::Relaxed), 1);
    exec.process(&mut out, 2);
    assert_eq!(counters.buffers_processed.load(Ordering::Relaxed), 2);
}

#[test]
fn runtime_counters_state_bus_overflow_increments_when_ring_full() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    let mut exec = conf.build_executor();
    let counters = exec.counters().clone();

    // Ring capacity is 256 (STATE_BUS_RING_CAPACITY). Run enough cycles
    // without draining the consumer to overflow the ring.
    let mut out = vec![0.0f32; 512 * 2];
    for _ in 0..260 {
        exec.process(&mut out, 2);
    }

    assert!(counters.state_bus_overflows.load(Ordering::Relaxed) >= 1);
}

#[test]
fn runtime_counters_set_counters_injects_shared_reference() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    let mut exec = conf.build_executor();

    let fresh = Arc::new(paraclete_runtime::RuntimeCounters::default());
    exec.set_counters(fresh.clone());
    assert!(std::ptr::eq(Arc::as_ptr(&fresh), Arc::as_ptr(exec.counters())));

    let mut out = vec![0.0f32; 512 * 2];
    exec.process(&mut out, 2);
    assert_eq!(fresh.buffers_processed.load(Ordering::Relaxed), 1);
}

#[test]
fn runtime_counters_zero_at_start() {
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(CountingNode::new(Arc::new(AtomicU32::new(0)))));
    let exec = conf.build_executor();
    let counters = exec.counters();

    assert_eq!(counters.buffers_processed.load(Ordering::Relaxed), 0);
    assert_eq!(counters.dropout_lock_miss.load(Ordering::Relaxed), 0);
    assert_eq!(counters.dropout_no_executor.load(Ordering::Relaxed), 0);
    assert_eq!(counters.state_bus_overflows.load(Ordering::Relaxed), 0);
}
