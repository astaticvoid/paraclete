// SPDX-License-Identifier: GPL-3.0-or-later
//! ScriptingGatewayNode — single-device bridge from graph events to the scripting engine.
//!
//! One gateway per hardware device. Each gateway is constructed with its device_id
//! and tags every incoming HardwareEvent with that ID. The ScriptEventConsumer is
//! drained on the main thread and fed to the scripting engine.
//!
//! Having one gateway per device (rather than a fan-in gateway) ensures events are
//! always tagged with the correct source device_id. A multi-device fan-in requires
//! ProcessInput to carry per-port metadata, which is deferred to P5 (ADR-018).

use paraclete_node_api::{
    CapabilityDocument, Event, HardwareEventMsg, Node,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
};

/// Consumer side of the gateway SPSC. Drain this from the main loop.
pub struct ScriptEventConsumer {
    rx: rtrb::Consumer<HardwareEventMsg>,
}

impl ScriptEventConsumer {
    pub fn drain(&mut self, out: &mut Vec<HardwareEventMsg>) {
        while let Ok(msg) = self.rx.pop() {
            out.push(msg);
        }
    }
}

/// Single-device gateway node. Connect exactly one hardware device to port 0.
/// All incoming HardwareEvents are tagged with `device_id` and sent to the SPSC.
pub struct ScriptingGatewayNode {
    ports: [PortDescriptor; 1],
    device_id: u32,
    tx: rtrb::Producer<HardwareEventMsg>,
}

impl ScriptingGatewayNode {
    pub fn new(device_id: u32, capacity: usize) -> (Self, ScriptEventConsumer) {
        let (tx, rx) = rtrb::RingBuffer::<HardwareEventMsg>::new(capacity);
        let gateway = Self {
            ports: [PortDescriptor {
                id: 0,
                name: "hw_events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            }],
            device_id,
            tx,
        };
        let consumer = ScriptEventConsumer { rx };
        (gateway, consumer)
    }
}

impl Node for ScriptingGatewayNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn set_node_id(&mut self, _id: u32) {}

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "ScriptingGatewayNode",
            vendor: "Paraclete",
            version: (0, 4, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec!["paraclete.gateway"],
        }
    }

    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        for timed in input.events {
            if let Event::Hardware(hw_ev) = timed.event {
                let _ = self.tx.push(HardwareEventMsg { device_id: self.device_id, event: hw_ev });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab, HardwareEvent,
        ProcessInput, ProcessOutput, TimedEvent, TransportInfo,
    };

    fn make_input<'a>(
        events: &'a [TimedEvent],
        transport: &'a TransportInfo,
        extended: &'a ExtendedEventSlab,
    ) -> ProcessInput<'a> {
        ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events,
            transport,
            sample_rate: 44100.0,
            block_size: 512,
            extended_events: extended,
            commands: &[],
        }
    }

    fn run_process(gw: &mut ScriptingGatewayNode, events: &[TimedEvent]) {
        let transport = TransportInfo::default();
        let extended  = ExtendedEventSlab::empty();
        let input = make_input(events, &transport, &extended);
        let mut audio_out: Vec<AudioBuffer> = vec![];
        let mut audio_refs: Vec<&mut AudioBuffer> = audio_out.iter_mut().collect();
        let mut ev_out = EventOutputBuffer::new(16);
        let mut output = ProcessOutput {
            audio_outputs: &mut audio_refs,
            signal_outputs: &mut [],
            events_out: &mut ev_out,
        };
        gw.process(&input, &mut output);
    }

    #[test]
    fn gateway_has_one_input_port() {
        let (gw, _consumer) = ScriptingGatewayNode::new(42, 64);
        assert_eq!(gw.ports().len(), 1);
        assert_eq!(gw.ports()[0].id, 0);
        assert_eq!(gw.ports()[0].direction, PortDirection::Input);
    }

    #[test]
    fn gateway_process_tags_hardware_event_with_device_id() {
        let (mut gw, mut consumer) = ScriptingGatewayNode::new(77, 64);
        let events = vec![TimedEvent::new(
            0,
            Event::Hardware(HardwareEvent::ButtonPressed { id: 5 }),
        )];
        run_process(&mut gw, &events);

        let mut out = Vec::new();
        consumer.drain(&mut out);
        assert_eq!(out.len(), 1, "gateway must forward the Hardware event");
        assert_eq!(out[0].device_id, 77, "device_id must match construction arg");
        assert!(matches!(out[0].event, HardwareEvent::ButtonPressed { id: 5 }));
    }

    #[test]
    fn gateway_process_ignores_non_hardware_events() {
        use paraclete_node_api::ExtendedEventRef;
        let (mut gw, mut consumer) = ScriptingGatewayNode::new(1, 64);
        // Extended events are not Hardware — should not be forwarded.
        let events = vec![TimedEvent::new(0, Event::Extended(
            ExtendedEventRef { space_id: 0, event_type: 0, slab_offset: 0, size: 0 },
        ))];
        run_process(&mut gw, &events);

        let mut out = Vec::new();
        consumer.drain(&mut out);
        assert!(out.is_empty(), "non-Hardware events must not be forwarded");
    }

    #[test]
    fn consumer_drain_empties_the_buffer() {
        let (mut gw, mut consumer) = ScriptingGatewayNode::new(1, 64);
        gw.tx.push(HardwareEventMsg {
            device_id: 1,
            event: HardwareEvent::ButtonPressed { id: 5 },
        }).ok();
        let mut out = Vec::new();
        consumer.drain(&mut out);
        assert_eq!(out.len(), 1);
        out.clear();
        consumer.drain(&mut out);
        assert!(out.is_empty(), "second drain must be empty");
    }
}
