use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, ConnectionRecord,
    HardwareDevice, HardwareOutputHandle, NodeCommand,
    StateBusHandle, StateBusSubscription, StateBusValue,
    Node, PortDescriptor, PortDirection, PortType,
};

use crate::executor::NodeExecutor;
use crate::graph::{EdgeMeta, NodeId, NodeMeta, RuntimeGraph};
use crate::state_bus::StateBusUpdate;
use crate::message::ConfigMessage;
use crate::ring_buffer;

const RING_CAPACITY: usize = 256;
const STATE_BUS_RING_CAPACITY: usize = 256;
const NODE_CMD_RING_CAPACITY: usize = 512;

/// Holds either a regular node or a hardware device.
pub(crate) enum NodeOrDevice {
    Node(Box<dyn Node>),
    Device(Box<dyn HardwareDevice>),
}

impl NodeOrDevice {
    pub(crate) fn ports(&self) -> &[PortDescriptor] {
        match self {
            NodeOrDevice::Node(m) => m.ports(),
            NodeOrDevice::Device(d) => d.ports(),
        }
    }

    pub(crate) fn activate(&mut self, sample_rate: f32, block_size: usize) {
        match self {
            NodeOrDevice::Node(m) => m.activate(sample_rate, block_size),
            NodeOrDevice::Device(d) => d.activate(sample_rate, block_size),
        }
    }

    pub(crate) fn deactivate(&mut self) {
        match self {
            NodeOrDevice::Node(m) => m.deactivate(),
            NodeOrDevice::Device(d) => d.deactivate(),
        }
    }

    pub(crate) fn capability_document(&self) -> CapabilityDocument {
        match self {
            NodeOrDevice::Node(m) => m.capability_document(),
            NodeOrDevice::Device(d) => d.capability_document(),
        }
    }

    pub(crate) fn negotiate(&mut self, their_doc: &CapabilityDocument) -> ConnectionAgreement {
        match self {
            NodeOrDevice::Node(m) => m.negotiate(their_doc),
            NodeOrDevice::Device(d) => d.negotiate(their_doc),
        }
    }

    pub(crate) fn set_connection_record(&mut self, record: ConnectionRecord) {
        match self {
            NodeOrDevice::Node(m) => m.set_connection_record(record),
            NodeOrDevice::Device(d) => d.set_connection_record(record),
        }
    }
}

/// Runs on the main thread. Owns the graph topology and manages node lifecycle.
pub struct NodeConfigurator {
    graph: RuntimeGraph,
    id_to_index: HashMap<u32, NodeIndex>,
    nodes: HashMap<NodeIndex, NodeOrDevice>,

    tempo_source_domains: Vec<(u32, u32)>,
    next_domain_id: u32,

    state_bus: Rc<RefCell<StateBusHandle>>,
    state_bus_consumer: rtrb::Consumer<StateBusUpdate>,
    state_bus_producer_pending: Option<rtrb::Producer<StateBusUpdate>>,

    /// NodeCommand SPSC producer — messages sent to the executor each cycle.
    node_cmd_producer: rtrb::Producer<NodeCommand>,
    node_cmd_consumer_pending: Option<rtrb::Consumer<NodeCommand>>,

    /// HardwareOutputHandles from devices that implement take_output_handle().
    /// Stored as (device_id, handle) so script LED output can be routed by device_id.
    output_handles: Vec<(u32, Box<dyn HardwareOutputHandle>)>,

    sample_rate: f32,
    block_size: usize,
    sender: ring_buffer::Sender<ConfigMessage>,
    pending_receiver: Option<ring_buffer::Receiver<ConfigMessage>>,
}

impl NodeConfigurator {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let (sender, receiver) = ring_buffer::channel(RING_CAPACITY);
        let (producer, consumer) = rtrb::RingBuffer::<StateBusUpdate>::new(STATE_BUS_RING_CAPACITY);
        let (cmd_prod, cmd_cons) = rtrb::RingBuffer::<NodeCommand>::new(NODE_CMD_RING_CAPACITY);
        Self {
            graph: RuntimeGraph::new(),
            id_to_index: HashMap::new(),
            nodes: HashMap::new(),
            tempo_source_domains: Vec::new(),
            next_domain_id: 0,
            state_bus: Rc::new(RefCell::new(StateBusHandle::new())),
            state_bus_consumer: consumer,
            state_bus_producer_pending: Some(producer),
            node_cmd_producer: cmd_prod,
            node_cmd_consumer_pending: Some(cmd_cons),
            output_handles: Vec::new(),
            sample_rate,
            block_size,
            sender,
            pending_receiver: Some(receiver),
        }
    }

    fn register(&mut self, user_id: u32, mut slot: NodeOrDevice) -> NodeId {
        assert!(
            !self.id_to_index.contains_key(&user_id),
            "duplicate node id {user_id} — each node must have a unique id"
        );
        match &mut slot {
            NodeOrDevice::Node(m) => m.set_node_id(user_id),
            NodeOrDevice::Device(d) => d.set_node_id(user_id),
        }
        slot.activate(self.sample_rate, self.block_size);
        let idx = self.graph.add_node(NodeMeta { user_id });
        self.id_to_index.insert(user_id, idx);
        self.nodes.insert(idx, slot);
        log::debug!("add_node: user_id={user_id} → graph_idx={idx:?}");
        NodeId(idx)
    }

    pub fn add_node(&mut self, user_id: u32, node: Box<dyn Node>) -> NodeId {
        self.register(user_id, NodeOrDevice::Node(node))
    }

    pub fn add_tempo_source(
        &mut self,
        user_id: u32,
        node: Box<dyn Node>,
    ) -> (NodeId, u32) {
        let domain_id = self.next_domain_id;
        self.next_domain_id += 1;
        self.tempo_source_domains.push((user_id, domain_id));
        let id = self.add_node(user_id, node);
        (id, domain_id)
    }

    /// Register a hardware device. Calls `take_output_handle()` immediately;
    /// if `Some`, the handle is stored and ticked each main-loop iteration.
    pub fn add_hardware_device(
        &mut self,
        user_id: u32,
        mut device: Box<dyn HardwareDevice>,
    ) -> NodeId {
        if let Some(handle) = device.take_output_handle() {
            self.output_handles.push((user_id, handle));
        }
        self.register(user_id, NodeOrDevice::Device(device))
    }

    pub fn remove_node(&mut self, user_id: u32) {
        if let Some(idx) = self.id_to_index.remove(&user_id) {
            if let Some(mut slot) = self.nodes.remove(&idx) {
                slot.deactivate();
            }
            self.graph.remove_node(idx);
        }
    }

    pub fn connect(
        &mut self,
        src_id: u32,
        src_port: u32,
        dst_id: u32,
        dst_port: u32,
    ) -> Result<(), String> {
        let src = *self.id_to_index.get(&src_id)
            .ok_or_else(|| format!("no node with id {src_id}"))?;
        let dst = *self.id_to_index.get(&dst_id)
            .ok_or_else(|| format!("no node with id {dst_id}"))?;

        let src_port_desc = self.nodes[&src]
            .ports().iter().find(|p| p.id == src_port)
            .ok_or_else(|| format!("node {src_id} has no port {src_port}"))?;
        let src_port_type = src_port_desc.port_type;
        if src_port_desc.direction != PortDirection::Output {
            return Err(format!(
                "port {src_port} on node {src_id} is an Input — source port must be Output"
            ));
        }

        let dst_port_desc = self.nodes[&dst]
            .ports().iter().find(|p| p.id == dst_port)
            .ok_or_else(|| format!("node {dst_id} has no port {dst_port}"))?;
        if dst_port_desc.direction != PortDirection::Input {
            return Err(format!(
                "port {dst_port} on node {dst_id} is an Output — destination port must be Input"
            ));
        }
        if dst_port_desc.port_type != src_port_type {
            return Err(format!(
                "type mismatch: port {src_port} on node {src_id} is {:?}, \
                 port {dst_port} on node {dst_id} is {:?}",
                src_port_type, dst_port_desc.port_type
            ));
        }

        self.graph.add_edge(src, dst, EdgeMeta { src_port, dst_port, src_port_type });

        if petgraph::algo::is_cyclic_directed(&self.graph) {
            if let Some(edge) = self.graph.find_edge(src, dst) {
                self.graph.remove_edge(edge);
            }
            return Err(format!(
                "connecting node {src_id}:{src_port} → {dst_id}:{dst_port} \
                 would create a cycle. Loop-break nodes are required (P9)."
            ));
        }

        let src_doc = self.nodes[&src].capability_document();
        let dst_doc = self.nodes[&dst].capability_document();

        let src_agreement = self.nodes.get_mut(&src).unwrap().negotiate(&dst_doc);
        let dst_agreement = self.nodes.get_mut(&dst).unwrap().negotiate(&src_doc);

        let reconciled = ConnectionAgreement {
            sample_rate: self.sample_rate,
            block_size: self.block_size,
            channels: 2,
            space_ids: src_agreement.space_ids.into_iter()
                .chain(dst_agreement.space_ids)
                .collect(),
            initial_transport: None,
            lockable_params: dst_agreement.lockable_params,
        };

        let src_user_id = self.graph[src].user_id;
        let dst_user_id = self.graph[dst].user_id;

        self.nodes.get_mut(&src).unwrap().set_connection_record(ConnectionRecord {
            agreement: reconciled.clone(),
            partner_id: dst_user_id,
            local_port_id: src_port,
        });
        self.nodes.get_mut(&dst).unwrap().set_connection_record(ConnectionRecord {
            agreement: reconciled,
            partner_id: src_user_id,
            local_port_id: dst_port,
        });

        Ok(())
    }

    /// Send a `NodeCommand` to a graph node. Delivered on the next audio cycle.
    /// Returns `Err` if the ring buffer is full — caller should retry next tick.
    pub fn send_command(&mut self, cmd: NodeCommand) -> Result<(), NodeCommand> {
        match self.node_cmd_producer.push(cmd) {
            Ok(()) => Ok(()),
            Err(_) => Err(cmd),
        }
    }

    pub fn state_bus_read(&self, path: &str) -> Option<StateBusValue> {
        self.state_bus.borrow().read(path).cloned()
    }

    pub fn state_bus_write(&mut self, path: &str, value: StateBusValue) {
        self.state_bus.borrow_mut().write(path, value);
    }

    pub fn state_bus_subscribe(&self, path: &str) -> StateBusSubscription {
        self.state_bus.borrow().subscribe(path)
    }

    pub fn state_bus_poll_subscription(
        &self,
        sub: &mut StateBusSubscription,
    ) -> Option<StateBusValue> {
        self.state_bus.borrow().poll_subscription(sub).cloned()
    }

    pub fn state_bus_handle(&self) -> Rc<RefCell<StateBusHandle>> {
        Rc::clone(&self.state_bus)
    }

    /// Drain executor state bus updates and apply them to the handle.
    /// Also ticks all registered `HardwareOutputHandle`s.
    /// Call from the main loop between audio cycles.
    pub fn process_main_thread(&mut self) {
        let mut bus = self.state_bus.borrow_mut();
        while let Ok(update) = self.state_bus_consumer.pop() {
            bus.apply_updates(update.entries);
        }
        drop(bus);
        for (_, handle) in &mut self.output_handles {
            handle.tick();
        }
    }

    /// Route script-generated LED output (from `set_led()` calls) to the matching
    /// hardware device's output handle. Call after `process_subscriptions()`.
    pub fn deliver_script_output(
        &mut self,
        output: std::collections::HashMap<u32, paraclete_node_api::HardwareOutput>,
    ) {
        for (device_id, hw_out) in output {
            if let Some((_, handle)) = self.output_handles.iter_mut().find(|(id, _)| *id == device_id) {
                handle.deliver(hw_out);
            }
        }
    }

    /// Legacy alias for `process_main_thread()`.
    pub fn process_state_bus(&mut self) {
        self.process_main_thread();
    }

    pub fn send(&self, msg: ConfigMessage) -> bool {
        self.sender.try_send(msg).is_ok()
    }

    /// Build a `NodeExecutor` from the current graph state. Can only be called once.
    pub fn build_executor(&mut self) -> NodeExecutor {
        let receiver = self.pending_receiver.take()
            .expect("build_executor called twice");
        let state_bus_producer = self.state_bus_producer_pending.take()
            .expect("build_executor called twice");
        let cmd_consumer = self.node_cmd_consumer_pending.take()
            .expect("build_executor called twice");

        let order = crate::graph::execution_order(&self.graph)
            .expect("graph has a cycle — connect() should have rejected it");

        let idx_to_slot: HashMap<NodeIndex, usize> = order
            .iter().enumerate().map(|(pos, &ni)| (ni, pos)).collect();

        let n = order.len();
        let mut event_routes: Vec<Vec<usize>> = vec![vec![]; n];
        let mut audio_routes: Vec<Vec<usize>> = vec![vec![]; n];

        for slot_pos in 0..n {
            let ni = order[slot_pos];
            for edge in self.graph.edges(ni) {
                match edge.weight().src_port_type {
                    PortType::Event | PortType::Clock => {
                        if let Some(&dst_pos) = idx_to_slot.get(&edge.target()) {
                            event_routes[slot_pos].push(dst_pos);
                        }
                    }
                    PortType::Audio => {
                        if let Some(&dst_pos) = idx_to_slot.get(&edge.target()) {
                            // Record slot_pos as an audio source for dst_pos.
                            audio_routes[dst_pos].push(slot_pos);
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut ordered: Vec<(u32, NodeOrDevice)> = Vec::with_capacity(n);
        for ni in &order {
            if let Some(slot) = self.nodes.remove(ni) {
                let user_id = self.graph[*ni].user_id;
                ordered.push((user_id, slot));
            }
        }

        NodeExecutor::new(
            ordered,
            event_routes,
            audio_routes,
            receiver,
            self.sample_rate,
            self.block_size,
            state_bus_producer,
            cmd_consumer,
        )
    }
}
