use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, ConnectionRecord,
    Surface, SurfaceOutputHandle, NodeCommand,
    StateBusHandle, StateBusSubscription, StateBusValue,
    Node, PortDescriptor, PortDirection, PortType, SignalPortKind,
};

use crate::executor::NodeExecutor;
use crate::graph::{EdgeMeta, NodeId, NodeMeta, RuntimeGraph};
use crate::state_bus::StateBusUpdate;
use crate::message::ConfigMessage;
use crate::ring_buffer;

const RING_CAPACITY: usize = 256;
const STATE_BUS_RING_CAPACITY: usize = 256;
const NODE_CMD_RING_CAPACITY: usize = 512;

/// Error returned by `NodeConfigurator::connect()`.
#[derive(Debug)]
pub enum ConnectError {
    /// A port or node ID was not found, or the connection is invalid (type mismatch, wrong direction).
    InvalidConnection(String),
    /// A cycle was detected with no `LoopBreakNode` to sanction it (ADR-028).
    MissingLoopBreak {
        /// The node user IDs participating in the unsanctioned cycle.
        cycle: Vec<u32>,
    },
    /// A cycle was detected with more than one `LoopBreakNode` (ADR-028).
    TooManyLoopBreaks {
        /// The node user IDs participating in the cycle.
        cycle: Vec<u32>,
        /// Number of `LoopBreakNode`s found in the cycle.
        count: usize,
    },
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConnection(msg) => write!(f, "{msg}"),
            Self::MissingLoopBreak { cycle } => write!(
                f,
                "cycle detected involving nodes {:?} with no LoopBreakNode; \
                 add a LoopBreakNode in the cycle to sanction it (ADR-028)",
                cycle
            ),
            Self::TooManyLoopBreaks { cycle, count } => write!(
                f,
                "cycle detected involving nodes {:?} with {count} LoopBreakNodes; \
                 exactly one is required per cycle (ADR-028)",
                cycle
            ),
        }
    }
}

impl std::error::Error for ConnectError {}

/// A snapshot of one graph edge, used by `save_project()` for validation.
#[derive(Debug, Clone)]
pub struct EdgeView {
    pub src_node: u32,
    pub src_port: u32,
    pub dst_node: u32,
    pub dst_port: u32,
}

/// Records a sanctioned feedback loop back-edge (ADR-028).
///
/// Stored in `NodeConfigurator` and propagated to `NodeExecutor` at build time.
/// The edge runs from the `LoopBreakNode`'s output port to the downstream node's
/// input port. It is excluded from topological sort but included in signal routing.
#[derive(Debug, Clone)]
pub struct LoopBackEdge {
    /// Node ID of the `LoopBreakNode`.
    pub lb_id: u32,
    /// Output port on the `LoopBreakNode` (always 1 for `LoopBreakNode`).
    pub lb_out_port: u32,
    /// Node ID of the downstream node that receives the delayed signal.
    pub dst_id: u32,
    /// Input port on the downstream node.
    pub dst_in_port: u32,
}

/// Holds either a regular node or a hardware device.
pub enum NodeOrDevice {
    Node(Box<dyn Node>),
    Device(Box<dyn Surface>),
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

    pub(crate) fn as_node_ref(&self) -> &dyn Node {
        match self {
            NodeOrDevice::Node(n) => &**n,
            // Surface: Node; trait upcasting stabilised in Rust 1.86.
            NodeOrDevice::Device(d) => &**d as &dyn Node,
        }
    }

    pub(crate) fn as_node_mut(&mut self) -> &mut dyn Node {
        match self {
            NodeOrDevice::Node(n) => &mut **n,
            NodeOrDevice::Device(d) => &mut **d as &mut dyn Node,
        }
    }

}

/// Runs on the main thread. Owns the graph topology and manages node lifecycle.
pub struct NodeConfigurator {
    graph: RuntimeGraph,
    id_to_index: HashMap<u32, NodeIndex>,
    nodes: HashMap<NodeIndex, NodeOrDevice>,

    /// Maps node user_id → type_tag string (set via `add_node_tagged()`).
    type_tag_map: HashMap<u32, String>,

    /// Auto-incrementing ID counter for `add_node_tagged()`.
    next_auto_id: u32,

    tempo_source_domains: Vec<(u32, u32)>,
    next_domain_id: u32,

    state_bus: Rc<RefCell<StateBusHandle>>,
    state_bus_consumer: rtrb::Consumer<StateBusUpdate>,
    state_bus_producer_pending: Option<rtrb::Producer<StateBusUpdate>>,

    /// NodeCommand SPSC producer — messages sent to the executor each cycle.
    node_cmd_producer: rtrb::Producer<NodeCommand>,
    node_cmd_consumer_pending: Option<rtrb::Consumer<NodeCommand>>,

    /// SurfaceOutputHandles from devices that implement take_output_handle().
    /// Stored as (device_id, handle) so script LED output can be routed by device_id.
    output_handles: Vec<(u32, Box<dyn SurfaceOutputHandle>)>,

    /// Capability documents cached at `add_node()` time — keyed by user_id.
    /// Persists after `build_executor()`.
    cap_doc_cache: HashMap<u32, CapabilityDocument>,

    /// Sanctioned feedback loop back-edges (ADR-028).
    /// Edges that form a cycle with exactly one `LoopBreakNode` are accepted and
    /// stored here. They are excluded from topological sort but included in signal
    /// routing at `build_executor()` time.
    back_edges: Vec<LoopBackEdge>,

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
            type_tag_map: HashMap::new(),
            next_auto_id: 1000,
            tempo_source_domains: Vec::new(),
            next_domain_id: 0,
            state_bus: Rc::new(RefCell::new(StateBusHandle::new())),
            state_bus_consumer: consumer,
            state_bus_producer_pending: Some(producer),
            node_cmd_producer: cmd_prod,
            node_cmd_consumer_pending: Some(cmd_cons),
            output_handles: Vec::new(),
            cap_doc_cache: HashMap::new(),
            back_edges: Vec::new(),
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
        let cap_doc = slot.as_node_ref().capability_document();
        self.cap_doc_cache.insert(user_id, cap_doc);
        let idx = self.graph.add_node(NodeMeta { user_id });
        self.id_to_index.insert(user_id, idx);
        self.nodes.insert(idx, slot);
        log::debug!("add_node: user_id={user_id} → graph_idx={idx:?}");
        NodeId(idx)
    }

    pub fn add_node(&mut self, user_id: u32, node: Box<dyn Node>) -> NodeId {
        self.register(user_id, NodeOrDevice::Node(node))
    }

    /// Register a node and store its `type_tag` for project file v2 serialisation.
    ///
    /// Auto-assigns a new unique user ID (≥ 1000, incrementing). Returns the
    /// assigned ID. Use this for all programmatically-created nodes that may need
    /// to be reconstructed from a project file.
    pub fn add_node_tagged(
        &mut self,
        node: Box<dyn Node>,
        type_tag: impl Into<String>,
    ) -> u32 {
        let user_id = self.next_auto_id;
        self.next_auto_id += 1;
        self.register(user_id, NodeOrDevice::Node(node));
        self.type_tag_map.insert(user_id, type_tag.into());
        user_id
    }

    /// Returns the type_tag stored for `node_id` by `add_node_tagged()`.
    /// Returns `None` if the node was registered via `add_node()` (no tag stored).
    pub fn type_tag_for(&self, node_id: u32) -> Option<&str> {
        self.type_tag_map.get(&node_id).map(|s| s.as_str())
    }

    /// Store a type_tag for a node that was added via `add_node()` (explicit ID).
    /// Used by project load to associate the type_tag without changing the node ID.
    pub fn set_type_tag_for(&mut self, node_id: u32, type_tag: impl Into<String>) {
        self.type_tag_map.insert(node_id, type_tag.into());
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
    pub fn add_surface(
        &mut self,
        user_id: u32,
        mut device: Box<dyn Surface>,
    ) -> NodeId {
        if let Some(handle) = device.take_output_handle() {
            self.output_handles.push((user_id, handle));
        }
        self.register(user_id, NodeOrDevice::Device(device))
    }

    /// Remove a node by ID, severing all connected edges.
    ///
    /// Returns the node's `Box<dyn Node>` on success, or an error string if the
    /// ID was not found. Hardware devices cannot be removed via this method.
    pub fn remove_node(&mut self, user_id: u32) -> Result<Box<dyn Node>, String> {
        let idx = self.id_to_index.remove(&user_id)
            .ok_or_else(|| format!("no node with id {user_id}"))?;
        let mut slot = self.nodes.remove(&idx)
            .ok_or_else(|| format!("node {user_id} not in nodes map"))?;
        slot.deactivate();
        self.graph.remove_node(idx);
        self.cap_doc_cache.remove(&user_id);
        self.type_tag_map.remove(&user_id);
        // Also remove any back_edges referencing this node.
        self.back_edges.retain(|be| be.lb_id != user_id && be.dst_id != user_id);
        match slot {
            NodeOrDevice::Node(n) => Ok(n),
            NodeOrDevice::Device(_) => Err(format!("node {user_id} is a hardware device; use remove_surface")),
        }
    }

    /// Iterate all registered nodes in registration order (by user_id).
    /// Yields `(user_id, &dyn Node)`. Only valid before `build_executor()` — after
    /// that call, nodes have been moved to the executor and this returns nothing.
    pub fn all_nodes(&self) -> impl Iterator<Item = (u32, &dyn Node)> + '_ {
        let mut pairs: Vec<(u32, &dyn Node)> = self.id_to_index.iter()
            .filter_map(|(&user_id, idx)| {
                self.nodes.get(idx).map(|slot| (user_id, slot.as_node_ref()))
            })
            .collect();
        pairs.sort_by_key(|(id, _)| *id);
        pairs.into_iter()
    }

    /// Iterate all edges in the graph. Yields `EdgeView` records in arbitrary order.
    pub fn all_edges(&self) -> impl Iterator<Item = EdgeView> + '_ {
        self.graph.edge_references().map(|er| EdgeView {
            src_node: self.graph[er.source()].user_id,
            src_port: er.weight().src_port,
            dst_node: self.graph[er.target()].user_id,
            dst_port: er.weight().dst_port,
        })
    }

    /// Mutable access to a registered node by user_id for deserialisation.
    /// Returns `None` if the id is not registered or the node has been moved
    /// to the executor via `build_executor()`.
    pub fn node_mut(&mut self, id: u32) -> Option<&mut dyn Node> {
        let idx = *self.id_to_index.get(&id)?;
        self.nodes.get_mut(&idx).map(|slot| slot.as_node_mut())
    }

    /// Returns the `CapabilityDocument` for the node at `node_id`.
    /// Served from cache populated at `add_node()` time — remains valid after
    /// `build_executor()`.
    pub fn get_node_cap_doc(&self, node_id: u32) -> Option<CapabilityDocument> {
        self.cap_doc_cache.get(&node_id).cloned()
    }

    pub fn sample_rate(&self) -> f32   { self.sample_rate }
    pub fn block_size(&self)  -> usize { self.block_size  }

    pub fn connect(
        &mut self,
        src_id: u32,
        src_port: u32,
        dst_id: u32,
        dst_port: u32,
    ) -> Result<(), ConnectError> {
        let src = *self.id_to_index.get(&src_id)
            .ok_or_else(|| ConnectError::InvalidConnection(format!("no node with id {src_id}")))?;
        let dst = *self.id_to_index.get(&dst_id)
            .ok_or_else(|| ConnectError::InvalidConnection(format!("no node with id {dst_id}")))?;

        let src_port_desc = self.nodes[&src]
            .ports().iter().find(|p| p.id == src_port)
            .ok_or_else(|| ConnectError::InvalidConnection(format!("node {src_id} has no port {src_port}")))?;
        let src_port_type = src_port_desc.port_type;
        if src_port_desc.direction != PortDirection::Output {
            return Err(ConnectError::InvalidConnection(format!(
                "port {src_port} on node {src_id} is an Input — source port must be Output"
            )));
        }

        let dst_port_desc = self.nodes[&dst]
            .ports().iter().find(|p| p.id == dst_port)
            .ok_or_else(|| ConnectError::InvalidConnection(format!("node {dst_id} has no port {dst_port}")))?;
        if dst_port_desc.direction != PortDirection::Input {
            return Err(ConnectError::InvalidConnection(format!(
                "port {dst_port} on node {dst_id} is an Output — destination port must be Input"
            )));
        }
        if dst_port_desc.port_type != src_port_type {
            return Err(ConnectError::InvalidConnection(format!(
                "type mismatch: port {src_port} on node {src_id} is {:?}, \
                 port {dst_port} on node {dst_id} is {:?}",
                src_port_type, dst_port_desc.port_type
            )));
        }

        self.graph.add_edge(src, dst, EdgeMeta { src_port, dst_port, src_port_type });

        if petgraph::algo::is_cyclic_directed(&self.graph) {
            // Find SCCs — any SCC with >1 node (or a self-loop) is a cycle.
            let sccs = petgraph::algo::tarjan_scc(&self.graph);
            let cycle_nodes: Vec<NodeIndex> = sccs
                .into_iter()
                .filter(|scc| scc.len() > 1 || self.graph.contains_edge(scc[0], scc[0]))
                .flatten()
                .collect();

            // Map graph indices to user IDs.
            let cycle_user_ids: Vec<u32> = cycle_nodes.iter()
                .map(|ni| self.graph[*ni].user_id)
                .collect();

            // Count LoopBreakNodes in the cycle.
            let lb_count = cycle_nodes.iter()
                .filter(|ni| {
                    if let Some(slot) = self.nodes.get(ni) {
                        slot.as_node_ref().is_loop_break()
                    } else {
                        false
                    }
                })
                .count();

            match lb_count {
                0 => {
                    if let Some(edge) = self.graph.find_edge(src, dst) {
                        self.graph.remove_edge(edge);
                    }
                    return Err(ConnectError::MissingLoopBreak { cycle: cycle_user_ids });
                }
                1 => {
                    // Find the LoopBreakNode's graph index.
                    let lb_ni = *cycle_nodes.iter()
                        .find(|ni| self.nodes.get(ni)
                            .map_or(false, |s| s.as_node_ref().is_loop_break()))
                        .unwrap();
                    let lb_user_id = self.graph[lb_ni].user_id;

                    // Collect LB's outgoing edges to cycle members BEFORE removing src→dst,
                    // so we capture the edge even if src is LB (src→dst may be LB's out edge).
                    let cycle_set: std::collections::HashSet<NodeIndex> =
                        cycle_nodes.iter().copied().collect();
                    let lb_out_info: Vec<(NodeIndex, u32, u32)> = self.graph
                        .edges_directed(lb_ni, petgraph::Direction::Outgoing)
                        .filter(|e| cycle_set.contains(&e.target()))
                        .map(|e| (e.target(), e.weight().src_port, e.weight().dst_port))
                        .collect();

                    // Remove src→dst from sort graph.
                    if let Some(edge) = self.graph.find_edge(src, dst) {
                        self.graph.remove_edge(edge);
                    }

                    // Remove LB's outgoing cycle edges from sort graph and record them.
                    // If src→dst was LB's outgoing edge (lb_ni==src), it is already removed.
                    let mut src_dst_was_lb_out = false;
                    for (out_dst_ni, lb_out_port, dst_in_port) in &lb_out_info {
                        let out_dst_user_id = self.graph[*out_dst_ni].user_id;
                        self.back_edges.push(LoopBackEdge {
                            lb_id:       lb_user_id,
                            lb_out_port: *lb_out_port,
                            dst_id:      out_dst_user_id,
                            dst_in_port: *dst_in_port,
                        });
                        if lb_ni == src && *out_dst_ni == dst {
                            src_dst_was_lb_out = true; // already removed above
                        } else if let Some(eid) = self.graph.find_edge(lb_ni, *out_dst_ni) {
                            self.graph.remove_edge(eid);
                        }
                    }

                    // If the cycle-closing edge (src→dst) was NOT LB's outgoing edge,
                    // it is a valid forward edge (e.g., A→LB). Re-add it to the sort graph.
                    if !src_dst_was_lb_out {
                        self.graph.add_edge(src, dst, EdgeMeta { src_port, dst_port, src_port_type });
                    }
                    // Fall through to connection negotiation below.
                }
                count => {
                    if let Some(edge) = self.graph.find_edge(src, dst) {
                        self.graph.remove_edge(edge);
                    }
                    return Err(ConnectError::TooManyLoopBreaks { cycle: cycle_user_ids, count });
                }
            }
        }

        let src_doc = self.cap_doc_cache[&src_id].clone();
        let dst_doc = self.cap_doc_cache[&dst_id].clone();

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
    /// Also ticks all registered `SurfaceOutputHandle`s.
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
        output: std::collections::HashMap<u32, paraclete_node_api::SurfaceOutput>,
    ) {
        for (device_id, hw_out) in output {
            if let Some((_, handle)) = self.output_handles.iter_mut().find(|(id, _)| *id == device_id) {
                handle.deliver(hw_out);
            } else {
                eprintln!("[deliver] no output handle for device {device_id} ({} led updates dropped); registered: {:?}",
                    hw_out.led_updates.len(),
                    self.output_handles.iter().map(|(id, _)| *id).collect::<Vec<_>>());
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

    fn port_type_to_signal_kind(pt: PortType) -> SignalPortKind {
        match pt {
            PortType::Modulation => SignalPortKind::Modulation,
            PortType::Logic      => SignalPortKind::Logic,
            PortType::Cv         => SignalPortKind::Cv,
            PortType::Pitch      => SignalPortKind::Pitch,
            PortType::Phase      => SignalPortKind::Phase,
            _ => unreachable!(),
        }
    }

    fn is_signal_port_type(pt: PortType) -> bool {
        matches!(pt, PortType::Modulation | PortType::Logic
                     | PortType::Cv | PortType::Pitch | PortType::Phase)
    }

    /// Remove a specific edge from the graph. Returns `Ok(())` if the edge was
    /// found and removed, or `Err(String)` if any node ID is not found.
    pub fn disconnect(
        &mut self,
        src: u32,
        src_port: u32,
        dst: u32,
        dst_port: u32,
    ) -> Result<(), String> {
        let src_ni = *self.id_to_index.get(&src)
            .ok_or_else(|| format!("no node with id {src}"))?;
        let dst_ni = *self.id_to_index.get(&dst)
            .ok_or_else(|| format!("no node with id {dst}"))?;
        let edge = self.graph.edge_references()
            .find(|e| {
                e.source() == src_ni
                    && e.target() == dst_ni
                    && e.weight().src_port == src_port
                    && e.weight().dst_port == dst_port
            })
            .map(|e| e.id());
        if let Some(eid) = edge {
            self.graph.remove_edge(eid);
        }
        // Also remove from back_edges if this was a sanctioned back-edge.
        self.back_edges.retain(|be| {
            !(be.lb_id == src
                && be.lb_out_port == src_port
                && be.dst_id == dst
                && be.dst_in_port == dst_port)
        });
        Ok(())
    }

    /// Build a `NodeExecutor` from the current graph state.
    ///
    /// Unlike `build_executor()`, this method is callable multiple times. Each
    /// call creates fresh communication channels (ring buffer, state bus, node
    /// command SPSC) and installs their consumer ends in the new executor. The
    /// producer ends are retained in the configurator so `send()`, `send_command()`,
    /// and the state bus remain operational after the rebuild.
    ///
    /// Callers must ensure the audio thread is paused (via `AudioEngine::wait_paused()`)
    /// before calling this — the previous executor must be dropped or returned first.
    pub fn rebuild_executor(&mut self) -> NodeExecutor {
        // Create fresh channels for the new executor. The old channels (consumed by
        // the previous executor) are dropped when that executor is dropped.
        let (new_sender, new_receiver)     = ring_buffer::channel(RING_CAPACITY);
        let (new_sb_prod, new_sb_cons)     = rtrb::RingBuffer::<StateBusUpdate>::new(STATE_BUS_RING_CAPACITY);
        let (new_cmd_prod, new_cmd_cons)   = rtrb::RingBuffer::<NodeCommand>::new(NODE_CMD_RING_CAPACITY);

        // Replace configurator-side handles so `send()` and `send_command()` stay live.
        self.sender                    = new_sender;
        self.node_cmd_producer         = new_cmd_prod;
        self.state_bus_consumer        = new_sb_cons;

        // Install executor-side ends as pending so `build_executor()` can take them.
        self.pending_receiver          = Some(new_receiver);
        self.state_bus_producer_pending = Some(new_sb_prod);
        self.node_cmd_consumer_pending = Some(new_cmd_cons);

        self.build_executor()
    }

    /// Drain nodes from the executor back to the configurator after `apply_patch`
    /// takes the old executor. Nodes are re-inserted so `rebuild_executor()` can
    /// pick them up again.
    pub fn restore_nodes(&mut self, nodes: Vec<(u32, NodeOrDevice)>) {
        for (user_id, slot) in nodes {
            if let Some(&ni) = self.id_to_index.get(&user_id) {
                self.nodes.entry(ni).or_insert(slot);
            }
        }
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
        let mut signal_input_routes: Vec<Vec<(u32, SignalPortKind, usize, u32)>> = vec![vec![]; n];

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
                            audio_routes[dst_pos].push(slot_pos);
                        }
                    }
                    pt if Self::is_signal_port_type(pt) => {
                        if let Some(&dst_pos) = idx_to_slot.get(&edge.target()) {
                            let kind     = Self::port_type_to_signal_kind(pt);
                            let src_port = edge.weight().src_port;
                            let dst_port = edge.weight().dst_port;
                            signal_input_routes[dst_pos].push((dst_port, kind, slot_pos, src_port));
                        }
                    }
                    _ => {}
                }
            }
        }

        // Add sanctioned back-edges (ADR-028) to signal_input_routes.
        // These edges were removed from the sort graph but must still participate in
        // signal routing: the downstream node reads the LoopBreakNode's output buffer
        // (pre-populated by the pre-execution phase) via signal_in_scratch.
        for be in &self.back_edges {
            if let (Some(&lb_slot), Some(&dst_slot)) = (
                self.id_to_index.get(&be.lb_id).and_then(|ni| idx_to_slot.get(ni)),
                self.id_to_index.get(&be.dst_id).and_then(|ni| idx_to_slot.get(ni)),
            ) {
                signal_input_routes[dst_slot].push((
                    be.dst_in_port,
                    SignalPortKind::Cv,
                    lb_slot,
                    be.lb_out_port,
                ));
            }
        }

        let mut ordered: Vec<(u32, NodeOrDevice)> = Vec::with_capacity(n);
        for ni in &order {
            if let Some(slot) = self.nodes.remove(ni) {
                let user_id = self.graph[*ni].user_id;
                ordered.push((user_id, slot));
            }
        }

        // Allocate output buffers for each slot's signal output ports.
        let block_size = self.block_size;
        let mut signal_output_bufs: Vec<Vec<(u32, SignalPortKind, Vec<f32>)>> = vec![vec![]; n];
        for (slot_pos, (_, node_or_device)) in ordered.iter().enumerate() {
            for port in node_or_device.ports() {
                if port.direction == PortDirection::Output
                   && Self::is_signal_port_type(port.port_type)
                {
                    let kind = Self::port_type_to_signal_kind(port.port_type);
                    signal_output_bufs[slot_pos].push((port.id, kind, vec![0.0f32; block_size]));
                }
            }
        }

        let back_edges = self.back_edges.clone();

        NodeExecutor::new(
            ordered,
            event_routes,
            audio_routes,
            signal_output_bufs,
            signal_input_routes,
            receiver,
            self.sample_rate,
            self.block_size,
            state_bus_producer,
            cmd_consumer,
            back_edges,
        )
    }
}
