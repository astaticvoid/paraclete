use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, ConnectionRecord,
    HardwareDevice, StateBusHandle, StateBusSubscription, StateBusValue,
    Node, PortDescriptor, PortType,
};

use crate::executor::NodeExecutor;
use crate::graph::{EdgeMeta, NodeId, NodeMeta, RuntimeGraph};
use crate::state_bus::StateBusUpdate;
use crate::message::ConfigMessage;
use crate::ring_buffer;

const RING_CAPACITY: usize = 256;
const STATE_BUS_RING_CAPACITY: usize = 256;

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
/// Sends incremental changes to `NodeExecutor` via a lock-free ring buffer
/// so the audio thread is never blocked.
///
/// State bus updates from the executor arrive via an `rtrb` SPSC ring buffer.
/// Call `process_state_bus()` between audio cycles to drain updates and notify
/// subscriptions.
pub struct NodeConfigurator {
    graph: RuntimeGraph,
    id_to_index: HashMap<u32, NodeIndex>,
    nodes: HashMap<NodeIndex, NodeOrDevice>,

    /// Registered TempoSource clock domain providers: (user_id, domain_id).
    tempo_source_domains: Vec<(u32, u32)>,
    next_domain_id: u32,

    /// Stable state bus handle — shared with the scripting engine.
    state_bus: Rc<RefCell<StateBusHandle>>,
    /// Consumer side of the executor → configurator SPSC ring buffer.
    state_bus_consumer: rtrb::Consumer<StateBusUpdate>,
    /// Producer side held until `build_executor()` — then moved into the executor.
    state_bus_producer_pending: Option<rtrb::Producer<StateBusUpdate>>,

    sample_rate: f32,
    block_size: usize,
    sender: ring_buffer::Sender<ConfigMessage>,
    pending_receiver: Option<ring_buffer::Receiver<ConfigMessage>>,
}

impl NodeConfigurator {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let (sender, receiver) = ring_buffer::channel(RING_CAPACITY);
        let (producer, consumer) = rtrb::RingBuffer::<StateBusUpdate>::new(STATE_BUS_RING_CAPACITY);
        Self {
            graph: RuntimeGraph::new(),
            id_to_index: HashMap::new(),
            nodes: HashMap::new(),
            tempo_source_domains: Vec::new(),
            next_domain_id: 0,
            state_bus: Rc::new(RefCell::new(StateBusHandle::new())),
            state_bus_consumer: consumer,
            state_bus_producer_pending: Some(producer),
            sample_rate,
            block_size,
            sender,
            pending_receiver: Some(receiver),
        }
    }

    fn register(&mut self, user_id: u32, mut slot: NodeOrDevice) -> NodeId {
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

    /// Register a node. `activate()` is called immediately on the calling thread.
    pub fn add_node(&mut self, user_id: u32, node: Box<dyn Node>) -> NodeId {
        self.register(user_id, NodeOrDevice::Node(node))
    }

    /// Register a TempoSource clock domain provider.
    ///
    /// Returns `(NodeId, domain_id)`.
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

    /// Register a hardware device.
    pub fn add_hardware_device(
        &mut self,
        user_id: u32,
        device: Box<dyn HardwareDevice>,
    ) -> NodeId {
        self.register(user_id, NodeOrDevice::Device(device))
    }

    /// Remove a node and all its connections.
    pub fn remove_node(&mut self, user_id: u32) {
        if let Some(idx) = self.id_to_index.remove(&user_id) {
            if let Some(mut slot) = self.nodes.remove(&idx) {
                slot.deactivate();
            }
            self.graph.remove_node(idx);
        }
    }

    /// Connect an output port on `src` to an input port on `dst`.
    ///
    /// Runs the Negotiable handshake: calls `negotiate()` on both nodes and
    /// delivers `ConnectionRecord`s to both sides.
    ///
    /// Returns an error if the connection would create a cycle (ADR-005).
    pub fn connect(
        &mut self,
        src_id: u32,
        src_port: u32,
        dst_id: u32,
        dst_port: u32,
    ) -> Result<(), String> {
        let src = *self
            .id_to_index
            .get(&src_id)
            .ok_or_else(|| format!("no node with id {src_id}"))?;
        let dst = *self
            .id_to_index
            .get(&dst_id)
            .ok_or_else(|| format!("no node with id {dst_id}"))?;

        let src_port_type = self.nodes[&src]
            .ports()
            .iter()
            .find(|p| p.id == src_port)
            .map(|p| p.port_type)
            .unwrap_or(PortType::Audio);

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

        // ── Negotiable handshake ──────────────────────────────────────────────
        // Get capability documents from both sides (immutable borrows).
        let src_doc = self.nodes[&src].capability_document();
        let dst_doc = self.nodes[&dst].capability_document();

        // Each side negotiates with the other's capability document.
        let src_agreement = self.nodes.get_mut(&src).unwrap().negotiate(&dst_doc);
        let dst_agreement = self.nodes.get_mut(&dst).unwrap().negotiate(&src_doc);

        // Reconcile: use runtime audio config; lockable_params come from dst
        // (the instrument declares what the sequencer can lock).
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

        // Deliver connection records to both sides.
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

    /// Read a value from the StateBus. Returns `None` if the path has no value.
    pub fn state_bus_read(&self, path: &str) -> Option<StateBusValue> {
        self.state_bus.borrow().read(path).cloned()
    }

    /// Write a value to the StateBus from the main thread.
    pub fn state_bus_write(&mut self, path: &str, value: StateBusValue) {
        self.state_bus.borrow_mut().write(path, value);
    }

    /// Subscribe to a StateBus path.
    pub fn state_bus_subscribe(&self, path: &str) -> StateBusSubscription {
        self.state_bus.borrow().subscribe(path)
    }

    /// Poll a subscription: returns `Some(value)` if changed since last poll.
    pub fn state_bus_poll_subscription(
        &self,
        sub: &mut StateBusSubscription,
    ) -> Option<StateBusValue> {
        self.state_bus.borrow().poll_subscription(sub).cloned()
    }

    /// Get a shared reference to the state bus handle.
    /// Pass this to the scripting engine to enable state bus access from scripts.
    pub fn state_bus_handle(&self) -> Rc<RefCell<StateBusHandle>> {
        Rc::clone(&self.state_bus)
    }

    /// Drain executor state bus updates and apply them to the handle.
    ///
    /// Call this from the main loop between audio cycles. Keeps the main-thread
    /// state bus view up to date and is required for `state_bus_read()` to
    /// reflect the latest executor-published values.
    pub fn process_state_bus(&mut self) {
        let mut bus = self.state_bus.borrow_mut();
        while let Ok(update) = self.state_bus_consumer.pop() {
            bus.apply_updates(update.entries);
        }
    }

    /// Send a transport command to the executor.
    /// Returns `false` if the ring buffer is full.
    pub fn send(&self, msg: ConfigMessage) -> bool {
        self.sender.try_send(msg).is_ok()
    }

    /// Build a `NodeExecutor` from the current graph state.
    ///
    /// Moves nodes into the executor in topological order. Panics if called twice.
    pub fn build_executor(&mut self) -> NodeExecutor {
        let receiver = self
            .pending_receiver
            .take()
            .expect("build_executor called twice on the same NodeConfigurator");

        let state_bus_producer = self
            .state_bus_producer_pending
            .take()
            .expect("build_executor called twice");

        let order = crate::graph::execution_order(&self.graph)
            .expect("graph has a cycle — connect() should have rejected it");

        let idx_to_slot: HashMap<NodeIndex, usize> = order
            .iter()
            .enumerate()
            .map(|(pos, &ni)| (ni, pos))
            .collect();

        let n = order.len();
        let mut event_routes: Vec<Vec<usize>> = vec![vec![]; n];

        for slot_pos in 0..n {
            let ni = order[slot_pos];
            for edge in self.graph.edges(ni) {
                if matches!(edge.weight().src_port_type, PortType::Event | PortType::Clock) {
                    if let Some(&dst_pos) = idx_to_slot.get(&edge.target()) {
                        event_routes[slot_pos].push(dst_pos);
                    }
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
            receiver,
            self.sample_rate,
            self.block_size,
            state_bus_producer,
        )
    }
}
