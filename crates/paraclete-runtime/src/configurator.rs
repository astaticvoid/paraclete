use std::collections::HashMap;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;

use paraclete_node_api::{HardwareDevice, StateBusValue, Node, PortDescriptor, PortType};

use crate::executor::NodeExecutor;
use crate::graph::{EdgeMeta, NodeId, NodeMeta, RuntimeGraph};
use crate::state_bus::{self, StateBusSnapshot, StateBusSubscription};
use crate::message::ConfigMessage;
use crate::ring_buffer;

const RING_CAPACITY: usize = 256;

/// Holds either a regular node or a hardware device. Both implement `Node` via
/// the `HardwareDevice: Node` supertrait, so `process()` is callable on both.
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
}

/// Runs on the main thread. Owns the graph topology and manages node lifecycle.
/// Sends incremental changes to `NodeExecutor` via a lock-free ring buffer
/// so the audio thread is never blocked.
pub struct NodeConfigurator {
    graph: RuntimeGraph,
    id_to_index: HashMap<u32, NodeIndex>,
    nodes: HashMap<NodeIndex, NodeOrDevice>,

    /// Registered TempoSource clock domain providers: (user_id, domain_id).
    tempo_source_domains: Vec<(u32, u32)>,
    next_domain_id: u32,

    /// Shared StateBus snapshot — written by executor, read by main thread.
    state_bus_snapshot: StateBusSnapshot,

    sample_rate: f32,
    block_size: usize,
    sender: ring_buffer::Sender<ConfigMessage>,
    pending_receiver: Option<ring_buffer::Receiver<ConfigMessage>>,
}

impl NodeConfigurator {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let (sender, receiver) = ring_buffer::channel(RING_CAPACITY);
        Self {
            graph: RuntimeGraph::new(),
            id_to_index: HashMap::new(),
            nodes: HashMap::new(),
            tempo_source_domains: Vec::new(),
            next_domain_id: 0,
            state_bus_snapshot: state_bus::new_snapshot(),
            sample_rate,
            block_size,
            sender,
            pending_receiver: Some(receiver),
        }
    }

    fn register(&mut self, user_id: u32, mut slot: NodeOrDevice) -> NodeId {
        // Notify the node of its assigned ID (for StateBus path construction).
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
    /// The node participates in the graph as a regular Node AND is registered
    /// as a clock domain authority. Returns `(NodeId, domain_id)`.
    ///
    /// Higher-priority TempoSource nodes take precedence in clock federation
    /// (see ADR-009). At P2, only one internal clock domain is supported.
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

    /// Register a hardware device. Participates in the graph as a `Node`
    /// AND receives `HardwareOutput` callbacks after each cycle.
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
    /// Looks up the port type on the source node and stores it in the edge
    /// so the executor can build event-routing tables at build_executor() time.
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

        Ok(())
    }

    /// Read a value from the StateBus snapshot. Returns `None` if the path
    /// has not been published yet.
    pub fn state_bus_read(&self, path: &str) -> Option<StateBusValue> {
        self.state_bus_snapshot.read().ok()?.get(path).cloned()
    }

    /// Subscribe to a StateBus path.
    pub fn state_bus_subscribe(&self, path: &str) -> StateBusSubscription {
        StateBusSubscription::new(path)
    }

    /// Expose the snapshot Arc for direct inspection (e.g. in tests).
    pub fn state_bus_snapshot_ref(&self) -> &crate::state_bus::StateBusSnapshot {
        &self.state_bus_snapshot
    }

    /// Send a transport command to the executor.
    /// Returns `false` if the ring buffer is full.
    pub fn send(&self, msg: ConfigMessage) -> bool {
        self.sender.try_send(msg).is_ok()
    }

    /// Build a `NodeExecutor` from the current graph state.
    ///
    /// Nodes are moved into the executor in topological order. Also computes
    /// the event-routing table so the executor can route events between nodes
    /// without re-querying the graph each cycle.
    ///
    /// Panics if called twice (the ring-buffer receiver has already been moved).
    pub fn build_executor(&mut self) -> NodeExecutor {
        let receiver = self
            .pending_receiver
            .take()
            .expect("build_executor called twice on the same NodeConfigurator");

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
            self.state_bus_snapshot.clone(),
        )
    }
}
