// SPDX-License-Identifier: GPL-3.0-or-later
//! ScriptingGatewayNode — single-device bridge from graph events to the scripting engine.
//!
//! One gateway per hardware device. Each gateway is constructed with its device_id
//! and tags every incoming SurfaceEvent with that ID. The ScriptEventConsumer is
//! drained on the main thread and fed to the scripting engine.
//!
//! Having one gateway per device (rather than a fan-in gateway) ensures events are
//! always tagged with the correct source device_id. A multi-device fan-in requires
//! ProcessInput to carry per-port metadata, which is deferred to P5 (ADR-018).

use paraclete_node_api::{
    CapabilityDocument, Event, SurfaceEventMsg, Node,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
};

/// Consumer side of the gateway SPSC. Drain this from the main loop.
pub struct ScriptEventConsumer {
    rx: rtrb::Consumer<SurfaceEventMsg>,
}

impl ScriptEventConsumer {
    pub fn drain(&mut self, out: &mut Vec<SurfaceEventMsg>) {
        while let Ok(msg) = self.rx.pop() {
            out.push(msg);
        }
    }
}

/// Single-device gateway node. Connect exactly one hardware device to port 0.
/// All incoming SurfaceEvents are tagged with `device_id` and sent to the SPSC.
pub struct ScriptingGatewayNode {
    ports: [PortDescriptor; 1],
    device_id: u32,
    tx: rtrb::Producer<SurfaceEventMsg>,
}

impl ScriptingGatewayNode {
    pub fn new(device_id: u32, capacity: usize) -> (Self, ScriptEventConsumer) {
        let (tx, rx) = rtrb::RingBuffer::<SurfaceEventMsg>::new(capacity);
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
            name: "ScriptingGatewayNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 4, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec!["paraclete.gateway".into()],
    view: None,
        }
    }

    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        for timed in input.events {
            if let Event::Surface(hw_ev) = timed.event {
                let _ = self.tx.push(SurfaceEventMsg { device_id: self.device_id, event: hw_ev });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab, SurfaceEvent,
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
        let mut output = ProcessOutput::new(
            &mut audio_refs,
            &mut [],
            &mut ev_out,
        );
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
    fn gateway_process_tags_surface_event_with_device_id() {
        let (mut gw, mut consumer) = ScriptingGatewayNode::new(77, 64);
        let events = vec![TimedEvent::new(
            0,
            Event::Surface(SurfaceEvent::ButtonPressed { id: 5 }),
        )];
        run_process(&mut gw, &events);

        let mut out = Vec::new();
        consumer.drain(&mut out);
        assert_eq!(out.len(), 1, "gateway must forward the Surface event");
        assert_eq!(out[0].device_id, 77, "device_id must match construction arg");
        assert!(matches!(out[0].event, SurfaceEvent::ButtonPressed { id: 5 }));
    }

    #[test]
    fn gateway_process_ignores_non_surface_events() {
        use paraclete_node_api::ExtendedEventRef;
        let (mut gw, mut consumer) = ScriptingGatewayNode::new(1, 64);
        // Extended events are not Hardware — should not be forwarded.
        let events = vec![TimedEvent::new(0, Event::Extended(
            ExtendedEventRef { space_id: 0, event_type: 0, slab_offset: 0, size: 0 },
        ))];
        run_process(&mut gw, &events);

        let mut out = Vec::new();
        consumer.drain(&mut out);
        assert!(out.is_empty(), "non-Surface events must not be forwarded");
    }

    /// P4.5 Fix 2 done criterion — two gateways with distinct device_ids do not
    /// cross-contaminate: events from device A arrive only in gateway A's consumer,
    /// tagged with A's device_id; events from device B arrive only in gateway B's.
    /// Spec: one_gateway_per_device_correct_device_id
    #[test]
    fn two_gateways_each_receive_only_their_devices_events() {
        let (mut gw_a, mut consumer_a) = ScriptingGatewayNode::new(10, 64);
        let (mut gw_b, mut consumer_b) = ScriptingGatewayNode::new(20, 64);

        let ev_a = vec![TimedEvent::new(0, Event::Surface(SurfaceEvent::PadPressed { id: 1, velocity: 64, pressure: 0 }))];
        let ev_b = vec![TimedEvent::new(0, Event::Surface(SurfaceEvent::PadPressed { id: 2, velocity: 64, pressure: 0 }))];

        run_process(&mut gw_a, &ev_a);
        run_process(&mut gw_b, &ev_b);

        let mut out_a = Vec::new();
        let mut out_b = Vec::new();
        consumer_a.drain(&mut out_a);
        consumer_b.drain(&mut out_b);

        assert_eq!(out_a.len(), 1);
        assert_eq!(out_a[0].device_id, 10, "gateway A events must carry device_id 10");
        assert!(matches!(out_a[0].event, SurfaceEvent::PadPressed { id: 1, .. }));

        assert_eq!(out_b.len(), 1);
        assert_eq!(out_b[0].device_id, 20, "gateway B events must carry device_id 20");
        assert!(matches!(out_b[0].event, SurfaceEvent::PadPressed { id: 2, .. }));
    }

    #[test]
    fn consumer_drain_empties_the_buffer() {
        let (mut gw, mut consumer) = ScriptingGatewayNode::new(1, 64);
        gw.tx.push(SurfaceEventMsg {
            device_id: 1,
            event: SurfaceEvent::ButtonPressed { id: 5 },
        }).ok();
        let mut out = Vec::new();
        consumer.drain(&mut out);
        assert_eq!(out.len(), 1);
        out.clear();
        consumer.drain(&mut out);
        assert!(out.is_empty(), "second drain must be empty");
    }
}
