use std::collections::HashMap;

use paraclete_node_api::{
    AudioBuffer, Control, Event, EventOutputBuffer, HardwareOutput, ExtendedEventSlab,
    NodeCommand, ProcessInput, ProcessOutput, StateBusValue,
    TransportInfo, TimedEvent,
};

use crate::configurator::NodeOrDevice;
use crate::state_bus::StateBusUpdate;
use crate::message::ConfigMessage;
use crate::ring_buffer::Receiver;

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
    cmd_consumer: rtrb::Consumer<NodeCommand>,
    /// Per-node command buffer: node_id → Vec<NodeCommand>.
    /// Pre-allocated accumulator; cleared each cycle. No audio-thread allocation.
    pending_cmds: HashMap<u32, Vec<NodeCommand>>,
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

    fn surface_pad_count(&self) -> Option<u32> {
        if let NodeOrDevice::Device(d) = &self.kind {
            Some(d.surface().controls.iter().filter(|c| matches!(c, Control::Pad(_))).count() as u32)
        } else {
            None
        }
    }

    fn hw_rgb_control_ids(&self) -> Vec<u32> {
        if let NodeOrDevice::Device(d) = &self.kind {
            d.surface().controls.iter().filter_map(|c| match c {
                Control::Pad(p) if p.rgb => Some(p.id),
                Control::Button(b) if b.rgb => Some(b.id),
                _ => None,
            }).collect()
        } else {
            vec![]
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
        cmd_consumer: rtrb::Consumer<NodeCommand>,
    ) -> Self {
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
            cmd_consumer,
            pending_cmds,
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
    }

    fn build_hardware_output(&self, _entries: &[(String, StateBusValue)]) -> HardwareOutput {
        // LED output is now owned by the scripting layer via deliver_script_output().
        // The executor no longer auto-generates step indicators.
        HardwareOutput::empty()
    }

    pub fn process(&mut self, out_interleaved: &mut [f32], channels: usize) {
        self.apply_messages();
        self.drain_commands();

        let sample_rate = self.sample_rate;
        let block_size  = self.block_size;

        for incoming in &mut self.incoming {
            incoming.clear();
        }

        let transport = &self.transport as *const TransportInfo;
        let slab = &self.extended_event_slab as *const ExtendedEventSlab;

        for slot_idx in 0..self.nodes.len() {
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
                    Event::Hardware(_)  => 3,
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
                audio_inputs: &[],
                signal_inputs: &[],
                events: &events,
                transport: transport_ref,
                sample_rate,
                block_size,
                extended_events: slab_ref,
                commands: cmds,
            };

            let mut output = ProcessOutput {
                audio_outputs: &mut outs,
                signal_outputs: &mut [],
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

        let entries: Vec<(String, StateBusValue)> = self.nodes.iter()
            .flat_map(|s| s.published_state())
            .collect();

        // Devices that implement take_output_handle() handle their own output on the
        // main thread; the legacy update_output() path serves the remaining devices.
        let hw_out = self.build_hardware_output(&entries);
        for slot in &mut self.nodes {
            slot.hw_update(&hw_out);
        }

        if !entries.is_empty() {
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
