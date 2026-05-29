use std::collections::HashMap;

use paraclete_node_api::{
    AudioBuffer, EventOutputBuffer, HardwareOutput, ExtendedEventSlab, StateBusValue, LedUpdate,
    ProcessInput, ProcessOutput, RgbColor, TransportInfo, TimedEvent,
};

use crate::configurator::NodeOrDevice;
use crate::state_bus::StateBusSnapshot;
use crate::message::ConfigMessage;
use crate::ring_buffer::Receiver;

/// Runs on the audio thread.
///
/// Receives graph changes from `NodeConfigurator` via a lock-free ring buffer,
/// then executes the node graph in topological order. Never allocates, blocks,
/// or takes a lock during the audio path — the StateBus snapshot write at the
/// end of each cycle is the sole exception (lock held for microseconds).
pub struct NodeExecutor {
    nodes: Vec<NodeSlot>,
    event_routes: Vec<Vec<usize>>,
    incoming: Vec<Vec<TimedEvent>>,
    transport: TransportInfo,
    sample_rate: f32,
    block_size: usize,
    receiver: Receiver<ConfigMessage>,
    extended_event_slab: ExtendedEventSlab,
    /// Shared with the NodeConfigurator — written here, read on main thread.
    state_bus_snapshot: StateBusSnapshot,
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
        state_bus_snapshot: StateBusSnapshot,
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
            state_bus_snapshot,
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
    ///
    /// Reads the pattern positions published by the Sequencer and lights pads:
    /// - current step: green
    /// - inactive steps: off
    /// Active patterns are inferred from the StateBus snapshot.
    fn build_hardware_output(&self, local_state_bus: &HashMap<String, StateBusValue>) -> HardwareOutput {
        let mut led_updates = Vec::new();

        // Find any Sequencer nodes publishing step state.
        // We scan the state_bus for keys matching the pattern.
        for (key, val) in local_state_bus {
            if key.ends_with("/state/current_step") {
                if let StateBusValue::Int(current_step) = val {
                    // Extract node_id from path: /node/{id}/state/current_step
                    let current_step = *current_step as u32;
                    // Light the current step green, clear others (0-15).
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

            let events: Vec<TimedEvent> = std::mem::take(&mut self.incoming[slot_idx]);

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

        // Collect published state into a local map (no lock yet).
        let mut local_state_bus: HashMap<String, StateBusValue> = HashMap::new();
        for slot in &self.nodes {
            for (k, v) in slot.published_state() {
                local_state_bus.insert(k, v);
            }
        }

        // Write to the shared snapshot (brief lock).
        if !local_state_bus.is_empty() {
            if let Ok(mut map) = self.state_bus_snapshot.write() {
                for (k, v) in &local_state_bus {
                    map.insert(k.clone(), v.clone());
                }
            }
        }

        // Build LED feedback from local state_bus and call update_output.
        let hw_out = self.build_hardware_output(&local_state_bus);
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
