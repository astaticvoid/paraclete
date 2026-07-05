use std::collections::HashMap;

use paraclete_node_api::{
    AudioBuffer, Control, Event, EventOutputBuffer, SurfaceOutput, ExtendedEventSlab,
    NodeCommand, ProcessInput, ProcessOutput, StateBusValue,
    SignalInputSlot, SignalOutputSlot, SignalPortKind,
    TransportInfo, TransportEvent, TimedEvent,
};

use crate::configurator::{LoopBackEdge, NodeOrDevice};
use crate::state_bus::StateBusUpdate;
use crate::message::ConfigMessage;
use crate::ring_buffer::Receiver;

/// Resolved loop-back edge in slot-index space. Used by the executor for the
/// pre/post execution phases (ADR-028).
struct ResolvedLoopBackEdge {
    /// Slot index of the `LoopBreakNode`.
    lb_slot: usize,
    /// Output port id on the `LoopBreakNode`.
    lb_out_port: u32,
    /// Slot index of the downstream node that receives the delayed signal.
    /// Stored for completeness and future diagnostics; not read by the current
    /// pre/post phases (signal routing via `signal_input_routes` handles delivery).
    #[allow(dead_code)]
    dst_slot: usize,
    /// Input port id on the downstream node.
    /// Stored for completeness and future diagnostics; not read by the current
    /// pre/post phases.
    #[allow(dead_code)]
    dst_in_port: u32,
}

pub struct NodeExecutor {
    nodes: Vec<NodeSlot>,
    event_routes: Vec<Vec<usize>>,
    /// For each slot position: the upstream slot indices that provide audio input.
    /// Pre-computed at build_executor() time; never changes at runtime.
    audio_routes: Vec<Vec<usize>>,
    incoming: Vec<Vec<TimedEvent>>,
    transport: TransportInfo,
    sample_rate: f32,
    block_size: usize,
    receiver: Receiver<ConfigMessage>,
    extended_event_slab: ExtendedEventSlab,
    state_bus_producer: rtrb::Producer<StateBusUpdate>,
    cmd_consumer: rtrb::Consumer<NodeCommand>,
    /// Per-node command buffer: node_id → Vec<NodeCommand>.
    /// Pre-allocated accumulator; cleared each cycle. No audio-thread allocation.
    pending_cmds: HashMap<u32, Vec<NodeCommand>>,
    /// Scratch buffer for collecting audio input pointers each process() cycle.
    /// Pre-allocated to the maximum number of audio sources any node has.
    audio_input_scratch: Vec<*const AudioBuffer>,

    /// Pre-allocated output buffers for non-audio signal ports.
    /// signal_output_bufs[slot_idx] = Vec<(port_id, kind, buffer)>.
    /// Zeroed at the start of each slot's process() call.
    signal_output_bufs: Vec<Vec<(u32, SignalPortKind, Vec<f32>)>>,

    /// Pre-computed signal routing. Built at build_executor() time; never mutated.
    /// signal_input_routes[slot_idx] = Vec<(dst_port, kind, src_slot, src_port)>.
    signal_input_routes: Vec<Vec<(u32, SignalPortKind, usize, u32)>>,

    /// Scratch slices for per-slot signal I/O — no audio-thread allocation.
    signal_out_scratch: Vec<SignalOutputSlot>,
    signal_in_scratch:  Vec<SignalInputSlot>,

    /// Pre-allocated state output buffer, one per node slot.
    /// Cleared before each `published_state()` call; capacity is retained across cycles.
    state_bufs: Vec<Vec<(String, StateBusValue)>>,

    /// Pre-allocated aggregate buffer for SPSC transfer.
    /// Filled each cycle by extending from each slot's `state_bufs` entry.
    /// `mem::take`'d into `StateBusUpdate` before pushing; leaves a zero-capacity
    /// Vec that grows back to stable size within a few cycles and then never
    /// reallocates again. One SPSC-ownership-transfer allocation per cycle is
    /// unavoidable with the current protocol; a return channel would eliminate it.
    agg_state_buf: Vec<(String, StateBusValue)>,

    /// DAW-provided transport override for the next `process()` call.
    /// When `Some`, replaces `self.transport` at the start of the next cycle
    /// and the optional event is injected into all slot event queues.
    /// Cleared after each `process()` call. Not set in standalone mode
    /// (InternalClock drives transport in that case). See ADR-024.
    transport_override: Option<(TransportInfo, Option<TransportEvent>)>,

    /// Reverse map: node user_id → slot index. Computed at construction; never changes.
    node_id_to_slot: HashMap<u32, usize>,

    /// Per-node events staged by `inject_event_for_node()`.
    /// Pairs of (slot_idx, event) applied to `incoming` after `incoming.clear()`.
    /// Pre-allocated; drained each cycle.
    extra_events: Vec<(usize, TimedEvent)>,

    /// NodeCommands staged by `inject_node_command()`.
    /// Applied to `pending_cmds` after `drain_commands()`.
    /// Pre-allocated; drained each cycle.
    extra_cmds: Vec<NodeCommand>,

    /// Sanctioned feedback loop back-edges (ADR-028), resolved to slot indices.
    /// Used in pre/post execution phases to inject delayed signal values and
    /// rotate buffers.
    back_edges: Vec<ResolvedLoopBackEdge>,
}

struct NodeSlot {
    id: u32,
    kind: NodeOrDevice,
    audio_out: AudioBuffer,
    events_out: EventOutputBuffer,
}

impl NodeSlot {
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        match &mut self.kind {
            NodeOrDevice::Node(m) => m.process(input, output),
            NodeOrDevice::Device(d) => d.process(input, output),
        }
    }

    fn hw_update(&mut self, out: &SurfaceOutput) {
        if let NodeOrDevice::Device(d) = &mut self.kind {
            d.update_output(out);
        }
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        match &self.kind {
            NodeOrDevice::Node(m) => m.published_state(buf),
            NodeOrDevice::Device(d) => d.published_state(buf),
        }
    }

    fn surface_pad_count(&self) -> Option<u32> {
        if let NodeOrDevice::Device(d) = &self.kind {
            Some(d.descriptor().controls.iter().filter(|c| matches!(c, Control::Pad(_))).count() as u32)
        } else {
            None
        }
    }

    fn hw_rgb_control_ids(&self) -> Vec<u32> {
        if let NodeOrDevice::Device(d) = &self.kind {
            d.descriptor().controls.iter().filter_map(|c| match c {
                Control::Pad(p) if p.rgb => Some(p.id),
                Control::Button(b) if b.rgb => Some(b.id),
                _ => None,
            }).collect()
        } else {
            vec![]
        }
    }
}

// SAFETY: NodeExecutor lives exclusively on the audio thread after being moved there.
// The non-Send field is audio_input_scratch: Vec<*const AudioBuffer>. These pointers
// are scratch-only: filled at the start of each slot's process() call and never read
// outside of that call. All other fields are Send (rtrb channels, the ring buffer
// Receiver, and the nodes themselves — which must have been Send to reach this point
// since they are constructed on the main thread and moved across a thread boundary).
unsafe impl Send for NodeExecutor {}

impl NodeExecutor {
    pub(crate) fn new(
        nodes: Vec<(u32, NodeOrDevice)>,
        event_routes: Vec<Vec<usize>>,
        audio_routes: Vec<Vec<usize>>,
        signal_output_bufs: Vec<Vec<(u32, SignalPortKind, Vec<f32>)>>,
        signal_input_routes: Vec<Vec<(u32, SignalPortKind, usize, u32)>>,
        receiver: Receiver<ConfigMessage>,
        sample_rate: f32,
        block_size: usize,
        state_bus_producer: rtrb::Producer<StateBusUpdate>,
        cmd_consumer: rtrb::Consumer<NodeCommand>,
        raw_back_edges: Vec<LoopBackEdge>,
    ) -> Self {
        let node_id_to_slot: HashMap<u32, usize> = nodes.iter().enumerate()
            .map(|(idx, (id, _))| (*id, idx))
            .collect();

        let n = nodes.len();
        let mut pending_cmds = HashMap::new();
        let slots = nodes.into_iter().map(|(id, kind)| {
            pending_cmds.insert(id, Vec::with_capacity(16));
            NodeSlot {
                id,
                kind,
                audio_out: AudioBuffer::new(2, block_size),
                events_out: EventOutputBuffer::new(256),
            }
        }).collect();

        // Resolve back-edge user IDs to slot indices.
        let back_edges: Vec<ResolvedLoopBackEdge> = raw_back_edges.into_iter()
            .filter_map(|be| {
                let lb_slot  = *node_id_to_slot.get(&be.lb_id)?;
                let dst_slot = *node_id_to_slot.get(&be.dst_id)?;
                Some(ResolvedLoopBackEdge {
                    lb_slot,
                    lb_out_port: be.lb_out_port,
                    dst_slot,
                    dst_in_port: be.dst_in_port,
                })
            })
            .collect();

        let max_audio_inputs  = audio_routes.iter().map(|r| r.len()).max().unwrap_or(0);
        let max_signal_outs   = signal_output_bufs.iter().map(|v| v.len()).max().unwrap_or(0);
        let max_signal_ins    = signal_input_routes.iter().map(|v| v.len()).max().unwrap_or(0);

        Self {
            nodes: slots,
            event_routes,
            audio_routes,
            incoming: (0..n).map(|_| Vec::with_capacity(16)).collect(),
            transport: TransportInfo::default(),
            sample_rate,
            block_size,
            receiver,
            extended_event_slab: ExtendedEventSlab::empty(),
            state_bus_producer,
            cmd_consumer,
            pending_cmds,
            audio_input_scratch: Vec::with_capacity(max_audio_inputs.max(1)),
            signal_output_bufs,
            signal_input_routes,
            signal_out_scratch: Vec::with_capacity(max_signal_outs.max(1)),
            signal_in_scratch:  Vec::with_capacity(max_signal_ins.max(1)),
            // Pre-seed per-slot capacity at 16 (covers nodes with up to ~8 bank params
            // plus a few state entries) and aggregate at 256 (covers ~32 nodes × 8
            // entries each without a first-cycle reallocation).
            state_bufs: (0..n).map(|_| Vec::with_capacity(16)).collect(),
            agg_state_buf: Vec::with_capacity(256),
            transport_override: None,
            node_id_to_slot,
            extra_events: Vec::with_capacity(64),
            extra_cmds:   Vec::with_capacity(64),
            back_edges,
        }
    }

    /// Inject DAW transport for the next `process()` cycle.
    ///
    /// Called by the CLAP adapter before each `process()` callback. The
    /// provided `TransportInfo` replaces the executor's internal transport for
    /// that cycle only. If an event is supplied it is prepended to the incoming
    /// event queue for every node slot.
    ///
    /// Not called in standalone mode — `InternalClock` drives transport there.
    pub fn set_transport_override(
        &mut self,
        info:  TransportInfo,
        event: Option<TransportEvent>,
    ) {
        self.transport_override = Some((info, event));
    }

    /// Stage a `TimedEvent` for delivery to `node_id` in the next `process()` call.
    ///
    /// The event is placed into the target node's incoming queue **after**
    /// `incoming.clear()` (and after any transport override event), so it
    /// survives the start-of-cycle reset. No-op if `node_id` is not in the graph.
    pub fn inject_event_for_node(&mut self, node_id: u32, event: TimedEvent) {
        if let Some(&slot_idx) = self.node_id_to_slot.get(&node_id) {
            debug_assert!(
                self.extra_events.len() < self.extra_events.capacity(),
                "extra_events at capacity ({}); next push will allocate on the audio thread",
                self.extra_events.capacity(),
            );
            self.extra_events.push((slot_idx, event));
        }
    }

    /// Stage a `NodeCommand` for delivery in the next `process()` call.
    ///
    /// The command is inserted into `pending_cmds` **after** `drain_commands()`
    /// clears the map, so it is not wiped on the cycle boundary. No-op if
    /// `cmd.target_id` is not in the graph.
    pub fn inject_node_command(&mut self, cmd: NodeCommand) {
        if !self.node_id_to_slot.contains_key(&cmd.target_id) {
            return;
        }
        debug_assert!(
            self.extra_cmds.len() < self.extra_cmds.capacity(),
            "extra_cmds at capacity ({}); next push will allocate on the audio thread",
            self.extra_cmds.capacity(),
        );
        self.extra_cmds.push(cmd);
    }

    /// Serialise a node's private state. Returns the byte blob from `Node::serialize()`.
    /// Returns an empty `Vec` if `node_id` is not in the graph.
    pub fn serialize_node(&self, node_id: u32) -> Vec<u8> {
        if let Some(&slot_idx) = self.node_id_to_slot.get(&node_id) {
            match &self.nodes[slot_idx].kind {
                crate::configurator::NodeOrDevice::Node(n)   => n.serialize(),
                crate::configurator::NodeOrDevice::Device(d) => d.serialize(),
            }
        } else {
            vec![]
        }
    }

    /// Deserialise a node's private state from a previously saved byte blob.
    /// No-op if `node_id` is not in the graph.
    pub fn deserialize_node(&mut self, node_id: u32, data: &[u8]) {
        if let Some(&slot_idx) = self.node_id_to_slot.get(&node_id) {
            match &mut self.nodes[slot_idx].kind {
                crate::configurator::NodeOrDevice::Node(n)   => n.deserialize(data),
                crate::configurator::NodeOrDevice::Device(d) => d.deserialize(data),
            }
        }
    }

    fn apply_messages(&mut self) {
        while let Some(msg) = self.receiver.try_recv() {
            match msg {
                ConfigMessage::SetPlaying(v) => self.transport.playing = v,
                ConfigMessage::SetBpm(bpm)   => self.transport.bpm = bpm,
                ConfigMessage::SetParam { .. } => {}
            }
        }
    }

    fn drain_commands(&mut self) {
        for cmds in self.pending_cmds.values_mut() {
            cmds.clear();
        }
        while let Ok(cmd) = self.cmd_consumer.pop() {
            if let Some(cmds) = self.pending_cmds.get_mut(&cmd.target_id) {
                cmds.push(cmd);
            }
        }
        // Apply commands staged by inject_node_command() — after clearing so they
        // survive the cycle boundary.
        for cmd in self.extra_cmds.drain(..) {
            if let Some(cmds) = self.pending_cmds.get_mut(&cmd.target_id) {
                cmds.push(cmd);
            }
        }
    }

    fn build_hardware_output(&self) -> SurfaceOutput {
        // LED output is now owned by the scripting layer via deliver_script_output().
        // The executor no longer auto-generates step indicators.
        SurfaceOutput::empty()
    }

    pub fn process(&mut self, out_interleaved: &mut [f32], channels: usize) {
        self.apply_messages();
        self.drain_commands();

        let sample_rate = self.sample_rate;
        let block_size  = self.block_size;

        for incoming in &mut self.incoming {
            incoming.clear();
        }

        // Applied after incoming.clear() so the injected TransportEvent survives
        // into the per-slot loop; applied after apply_messages() so the CLAP
        // transport wins over any ring-buffer messages.
        if let Some((info, event)) = self.transport_override.take() {
            self.transport = info;
            if let Some(te) = event {
                let timed = paraclete_node_api::TimedEvent::new(0, paraclete_node_api::Event::Transport(te));
                for queue in &mut self.incoming {
                    queue.push(timed);
                }
            }
        }

        // Apply per-node events staged by inject_event_for_node() — after
        // transport_override so injected events land alongside (not before) it.
        for (slot_idx, event) in self.extra_events.drain(..) {
            if slot_idx < self.incoming.len() {
                self.incoming[slot_idx].push(event);
            }
        }

        let transport = &self.transport as *const TransportInfo;
        let slab = &self.extended_event_slab as *const ExtendedEventSlab;

        // Pre-execution phase (ADR-028): for each sanctioned back-edge, copy the
        // LoopBreakNode's "previous cycle" slice into its signal output buffer.
        // This ensures the downstream node reads the correct delayed value when its
        // signal_in_scratch is built from signal_input_routes, which points to this buffer.
        for be in &self.back_edges {
            let lb_slot  = be.lb_slot;
            let out_port = be.lb_out_port;
            // Obtain the prev slice from the LoopBreakNode via the trait method.
            // SAFETY: we borrow nodes[lb_slot] immutably here; it is not aliased by
            // any other borrow at this point (we are outside the main process loop).
            let prev_data: &[f32] = match &self.nodes[lb_slot].kind {
                NodeOrDevice::Node(n) => n.loop_break_prev(),
                NodeOrDevice::Device(d) => d.loop_break_prev(),
            };
            // Copy into signal_output_bufs[lb_slot] for the matching port.
            if let Some(buf_entry) = self.signal_output_bufs[lb_slot]
                .iter_mut()
                .find(|(port_id, _, _)| *port_id == out_port)
            {
                let out_buf = &mut buf_entry.2;
                let len = prev_data.len().min(out_buf.len());
                out_buf[..len].copy_from_slice(&prev_data[..len]);
            }
        }

        for slot_idx in 0..self.nodes.len() {
            // Collect audio input pointers before the mutable borrow of self.nodes[slot_idx].
            // audio_routes[slot_idx] contains only src_idx < slot_idx (topological order),
            // so those nodes are already processed and their audio_out buffers are stable.
            self.audio_input_scratch.clear();
            for &src_idx in &self.audio_routes[slot_idx] {
                self.audio_input_scratch.push(&self.nodes[src_idx].audio_out as *const AudioBuffer);
            }
            // SAFETY: *const AudioBuffer and &AudioBuffer have identical representation
            // for sized types (both thin pointers). The referenced buffers remain valid
            // and unmodified for the duration of this slot's process() call.
            let audio_inputs: &[&AudioBuffer] = unsafe {
                std::slice::from_raw_parts(
                    self.audio_input_scratch.as_ptr() as *const &AudioBuffer,
                    self.audio_input_scratch.len(),
                )
            };

            // Build signal output slots for this slot (scoped to release borrows before
            // the slot mutable borrow below; raw pointer slices are extracted after).
            {
                let (scratch, out_bufs) = (&mut self.signal_out_scratch, &mut self.signal_output_bufs);
                scratch.clear();
                for (port_id, kind, buf) in out_bufs[slot_idx].iter_mut() {
                    buf.fill(0.0);
                    scratch.push(SignalOutputSlot::new(*port_id, *kind, buf));
                }
            }
            // Build signal input slots from upstream output buffers.
            {
                let (scratch, routes, out_bufs) = (
                    &mut self.signal_in_scratch,
                    &self.signal_input_routes,
                    &self.signal_output_bufs,
                );
                scratch.clear();
                for &(dst_port, kind, src_slot, src_port) in &routes[slot_idx] {
                    for (port_id, _, buf) in &out_bufs[src_slot] {
                        if *port_id == src_port {
                            scratch.push(SignalInputSlot::new(dst_port, kind, buf));
                            break;
                        }
                    }
                }
            }
            // Extract raw slice pointers so signal I/O can be passed to process()
            // without conflicting with the mutable borrow of self.nodes[slot_idx] below.
            // SAFETY: signal_out_scratch and signal_in_scratch are separate fields from
            // nodes; their backing Vec data is stable (no push/pop past this point per
            // cycle). The *mut f32 pointers inside each SignalOutputSlot refer to
            // signal_output_bufs[slot_idx], not to nodes; no aliasing.
            let sig_out_ptr = self.signal_out_scratch.as_mut_ptr();
            let sig_out_len = self.signal_out_scratch.len();
            let sig_in_ptr  = self.signal_in_scratch.as_ptr();
            let sig_in_len  = self.signal_in_scratch.len();

            let slot = &mut self.nodes[slot_idx];
            slot.audio_out.clear();
            slot.events_out.clear();

            // Take ownership to avoid an aliasing conflict with `slot`; the vec
            // is returned at end of the loop body to preserve its allocation.
            let mut events: Vec<TimedEvent> = std::mem::take(&mut self.incoming[slot_idx]);

            events.sort_unstable_by_key(|e| {
                let priority: u8 = match e.event {
                    Event::ParamLock(_) => 0,
                    Event::Transport(_) => 1,
                    Event::Tempo(_)     => 1,
                    Event::Midi2(_)     => 2,
                    Event::Surface(_)  => 3,
                    Event::Extended(_)  => 4,
                    _ => 5,
                };
                (e.sample_offset, priority)
            });

            // SAFETY: transport and slab are read-only during process().
            let transport_ref = unsafe { &*transport };
            let slab_ref      = unsafe { &*slab };

            // SAFETY: audio_out_ptr scoped to this loop body.
            let audio_out_ptr: *mut AudioBuffer = &mut slot.audio_out as *mut AudioBuffer;
            let audio_out_ref: &mut AudioBuffer = unsafe { &mut *audio_out_ptr };
            let mut outs: [&mut AudioBuffer; 1] = [audio_out_ref];

            // SAFETY: events_out_ptr is distinct from kind.
            let events_out_ptr: *mut EventOutputBuffer = &mut slot.events_out;
            let events_out_ref: &mut EventOutputBuffer = unsafe { &mut *events_out_ptr };

            let node_id = slot.id;
            let cmds = self.pending_cmds.get(&node_id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            let input = ProcessInput {
                audio_inputs,
                // SAFETY: sig_in_ptr/sig_in_len from self.signal_in_scratch; stable for
                // the duration of this call; separate field from nodes.
                signal_inputs: unsafe { std::slice::from_raw_parts(sig_in_ptr, sig_in_len) },
                events: &events,
                transport: transport_ref,
                sample_rate,
                block_size,
                extended_events: slab_ref,
                commands: cmds,
            };

            let mut output = ProcessOutput {
                audio_outputs: &mut outs,
                // SAFETY: sig_out_ptr/sig_out_len from self.signal_out_scratch; stable for
                // the duration of this call; SignalOutputSlot::ptr fields point into
                // signal_output_bufs[slot_idx], distinct from nodes.
                signal_outputs: unsafe { std::slice::from_raw_parts_mut(sig_out_ptr, sig_out_len) },
                events_out: events_out_ref,
            };

            slot.process(&input, &mut output);

            // Put the (now empty) vec back so its heap allocation survives to
            // the next cycle — avoids a realloc on every process() call.
            events.clear();
            self.incoming[slot_idx] = events;

            if !self.event_routes[slot_idx].is_empty() {
                // SAFETY: events_out and incoming are disjoint fields; DAG guarantees no self-loops.
                let outgoing: *const [TimedEvent] =
                    self.nodes[slot_idx].events_out.as_slice() as *const [TimedEvent];
                for &dst_idx in &self.event_routes[slot_idx] {
                    self.incoming[dst_idx].extend_from_slice(unsafe { &*outgoing });
                }
            }
        }

        // Post-execution phase (ADR-028): swap the LoopBreakNode's prev/next buffers
        // for each sanctioned back-edge. This makes the data captured during this
        // cycle's process() call (in `next`) available via `loop_break_prev()` in
        // the next cycle's pre-execution phase.
        for be in &self.back_edges {
            let lb_slot = be.lb_slot;
            match &mut self.nodes[lb_slot].kind {
                NodeOrDevice::Node(n) => n.loop_break_swap(),
                NodeOrDevice::Device(d) => d.loop_break_swap(),
            }
        }

        // Collect published state into pre-allocated per-slot buffers. Vec::clear()
        // retains capacity so per-slot buffers never reallocate after the first cycle.
        self.agg_state_buf.clear();
        for (slot_idx, slot) in self.nodes.iter().enumerate() {
            self.state_bufs[slot_idx].clear();
            slot.published_state(&mut self.state_bufs[slot_idx]);
            self.agg_state_buf.extend_from_slice(&self.state_bufs[slot_idx]);
        }

        // Devices that implement take_output_handle() handle their own output on the
        // main thread; the legacy update_output() path serves the remaining devices.
        let hw_out = self.build_hardware_output();
        for slot in &mut self.nodes {
            slot.hw_update(&hw_out);
        }

        if !self.agg_state_buf.is_empty() {
            // mem::take transfers ownership to the StateBusUpdate without a collect().
            // agg_state_buf grows back to stable capacity within a few cycles.
            let entries = std::mem::take(&mut self.agg_state_buf);
            let _ = self.state_bus_producer.push(StateBusUpdate { entries });
        }

        let frames = self.block_size;
        debug_assert_eq!(out_interleaved.len(), frames * channels);
        out_interleaved.fill(0.0);

        for slot in &self.nodes {
            let buf = &slot.audio_out;
            for frame in 0..frames {
                for ch in 0..channels.min(buf.channels()) {
                    out_interleaved[frame * channels + ch] += buf.channel(ch)[frame];
                }
            }
        }
    }

    pub fn transport(&self) -> &TransportInfo {
        &self.transport
    }

    /// Extract all nodes from this executor and return them as `(user_id, NodeOrDevice)` pairs.
    ///
    /// Used by `apply_patch()` to return nodes to the `NodeConfigurator` before
    /// rebuilding the executor with new topology. The executor is empty afterward
    /// (any further `process()` calls would produce silence).
    pub fn drain_nodes(mut self) -> Vec<(u32, NodeOrDevice)> {
        // Build a reverse map: slot_idx → user_id from node_id_to_slot.
        let mut slot_to_id: Vec<u32> = vec![0u32; self.nodes.len()];
        for (&uid, &slot_idx) in &self.node_id_to_slot {
            if slot_idx < slot_to_id.len() {
                slot_to_id[slot_idx] = uid;
            }
        }
        // `std::mem::take` leaves `self.nodes` empty. When `self` is later dropped,
        // `Drop` iterates `self.nodes` (now empty) — no double-deactivation occurs.
        // Nodes are returned still-activated; the configurator keeps them alive.
        let slots: Vec<NodeSlot> = std::mem::take(&mut self.nodes);
        slots
            .into_iter()
            .enumerate()
            .map(|(idx, slot)| {
                let user_id = if idx < slot_to_id.len() { slot_to_id[idx] } else { slot.id };
                (user_id, slot.kind)
            })
            .collect()
    }

    /// Returns the capacity of the pre-allocated state buffer for slot `idx`.
    /// Used by tests to verify no reallocation occurs after the first cycle.
    pub fn state_buf_capacity(&self, idx: usize) -> usize {
        self.state_bufs[idx].capacity()
    }
}

impl Drop for NodeExecutor {
    fn drop(&mut self) {
        for slot in &mut self.nodes {
            match &mut slot.kind {
                NodeOrDevice::Node(m) => m.deactivate(),
                NodeOrDevice::Device(d) => d.deactivate(),
            }
        }
    }
}
