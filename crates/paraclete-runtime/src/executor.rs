use paraclete_node_api::{
    AudioBuffer, Event, EventOutputBuffer, HardwareOutput, ExtendedEventSlab, StateBusValue,
    LedUpdate, ProcessInput, ProcessOutput, RgbColor, TransportInfo, TimedEvent,
};

use crate::configurator::NodeOrDevice;
use crate::state_bus::StateBusUpdate;
use crate::message::ConfigMessage;
use crate::ring_buffer::Receiver;

/// Runs on the audio thread.
///
/// Receives graph changes from `NodeConfigurator` via a lock-free ring buffer,
/// then executes the node graph in topological order. Never allocates, blocks,
/// or takes a lock during the audio path.
///
/// State bus updates are pushed to the main thread via an `rtrb` SPSC producer.
pub struct NodeExecutor {
    nodes: Vec<NodeSlot>,
    event_routes: Vec<Vec<usize>>,
    incoming: Vec<Vec<TimedEvent>>,
    transport: TransportInfo,
    sample_rate: f32,
    block_size: usize,
    receiver: Receiver<ConfigMessage>,
    extended_event_slab: ExtendedEventSlab,
    state_bus_producer: rtrb::Producer<StateBusUpdate>,
}

struct NodeSlot {
    #[allow(dead_code)]
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

    fn hw_update(&mut self, out: &HardwareOutput) {
        if let NodeOrDevice::Device(d) = &mut self.kind {
            d.update_output(out);
        }
    }

    fn published_state(&self) -> Vec<(String, StateBusValue)> {
        match &self.kind {
            NodeOrDevice::Node(m) => m.published_state(),
            NodeOrDevice::Device(d) => d.published_state(),
        }
    }
}

impl NodeExecutor {
    pub(crate) fn new(
        nodes: Vec<(u32, NodeOrDevice)>,
        event_routes: Vec<Vec<usize>>,
        receiver: Receiver<ConfigMessage>,
        sample_rate: f32,
        block_size: usize,
        state_bus_producer: rtrb::Producer<StateBusUpdate>,
    ) -> Self {
        let n = nodes.len();
        let slots = nodes
            .into_iter()
            .map(|(id, kind)| NodeSlot {
                id,
                kind,
                audio_out: AudioBuffer::new(2, block_size),
                events_out: EventOutputBuffer::new(256),
            })
            .collect();

        Self {
            nodes: slots,
            event_routes,
            incoming: vec![vec![]; n],
            transport: TransportInfo::default(),
            sample_rate,
            block_size,
            receiver,
            extended_event_slab: ExtendedEventSlab::empty(),
            state_bus_producer,
        }
    }

    fn apply_messages(&mut self) {
        while let Some(msg) = self.receiver.try_recv() {
            match msg {
                ConfigMessage::SetPlaying(v) => self.transport.playing = v,
                ConfigMessage::SetBpm(bpm) => self.transport.bpm = bpm,
                ConfigMessage::SetParam { .. } => { /* P2: route to node */ }
            }
        }
    }

    /// Build LED feedback for hardware devices from the current StateBus values.
    fn build_hardware_output(&self, entries: &[(String, StateBusValue)]) -> HardwareOutput {
        let mut led_updates = Vec::new();

        for (key, val) in entries {
            if key.ends_with("/state/current_step") {
                if let StateBusValue::Int(current_step) = val {
                    let current_step = *current_step as u32;
                    // TODO P4: replace with iteration over HardwareDevice::surface() descriptor
                    for pad in 0u32..16 {
                        led_updates.push(LedUpdate {
                            control_id: pad,
                            color: if pad == current_step {
                                RgbColor::GREEN
                            } else {
                                RgbColor::OFF
                            },
                        });
                    }
                }
            }
        }

        HardwareOutput { led_updates, display_updates: vec![] }
    }

    /// Execute one buffer cycle.
    pub fn process(&mut self, out_interleaved: &mut [f32], channels: usize) {
        self.apply_messages();

        let sample_rate = self.sample_rate;
        let block_size = self.block_size;

        for incoming in &mut self.incoming {
            incoming.clear();
        }

        let transport = &self.transport as *const TransportInfo;
        let slab = &self.extended_event_slab as *const ExtendedEventSlab;

        for slot_idx in 0..self.nodes.len() {
            let slot = &mut self.nodes[slot_idx];
            slot.audio_out.clear();
            slot.events_out.clear();

            let mut events: Vec<TimedEvent> = std::mem::take(&mut self.incoming[slot_idx]);

            // Sort events within same sample_offset by priority:
            // 1. ParamLock, 2. Transport/Tempo, 3. Midi2, 4. Hardware, 5. Extended
            events.sort_unstable_by_key(|e| {
                let priority: u8 = match e.event {
                    Event::ParamLock(_)  => 0,
                    Event::Transport(_)  => 1,
                    Event::Tempo(_)      => 1,
                    Event::Midi2(_)      => 2,
                    Event::Hardware(_)   => 3,
                    Event::Extended(_)   => 4,
                };
                (e.sample_offset, priority)
            });

            // SAFETY: transport and slab are read-only during process().
            let transport_ref = unsafe { &*transport };
            let slab_ref = unsafe { &*slab };

            // SAFETY: audio_out_ptr scoped to this loop body.
            let audio_out_ptr: *mut AudioBuffer = &mut slot.audio_out as *mut AudioBuffer;
            let audio_out_ref: &mut AudioBuffer = unsafe { &mut *audio_out_ptr };
            let mut outs: [&mut AudioBuffer; 1] = [audio_out_ref];

            // SAFETY: events_out_ptr derived from slot.events_out, distinct from kind.
            let events_out_ptr: *mut EventOutputBuffer = &mut slot.events_out;
            let events_out_ref: &mut EventOutputBuffer = unsafe { &mut *events_out_ptr };

            let input = ProcessInput {
                audio_inputs: &[],
                signal_inputs: &[],
                events: &events,
                transport: transport_ref,
                sample_rate,
                block_size,
                extended_events: slab_ref,
            };

            let mut output = ProcessOutput {
                audio_outputs: &mut outs,
                signal_outputs: &mut [],
                events_out: events_out_ref,
            };

            slot.process(&input, &mut output);

            if !self.event_routes[slot_idx].is_empty() {
                // SAFETY: events_out and incoming are disjoint fields; we only read
                // events_out[slot_idx] while writing to incoming[dst_idx] where
                // dst_idx != slot_idx (the graph is a DAG, no self-loops allowed).
                let outgoing: *const [TimedEvent] =
                    self.nodes[slot_idx].events_out.as_slice() as *const [TimedEvent];
                for &dst_idx in &self.event_routes[slot_idx] {
                    self.incoming[dst_idx].extend_from_slice(unsafe { &*outgoing });
                }
            }
        }

        // Collect published state from all nodes.
        let entries: Vec<(String, StateBusValue)> = self.nodes.iter()
            .flat_map(|s| s.published_state())
            .collect();

        // Push to SPSC ring buffer — non-blocking, drop if full (main thread is behind).
        if !entries.is_empty() {
            let _ = self.state_bus_producer.push(StateBusUpdate { entries: entries.clone() });
        }

        // Build LED feedback and call update_output on hardware devices (always).
        let hw_out = self.build_hardware_output(&entries);
        for slot in &mut self.nodes {
            slot.hw_update(&hw_out);
        }

        // Sum all node audio outputs into the interleaved output buffer.
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
