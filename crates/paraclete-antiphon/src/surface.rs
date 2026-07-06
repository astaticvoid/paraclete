// SPDX-License-Identifier: GPL-3.0-or-later
//! TheoriaSurfaceNode — every connected Theoria client manifests through this
//! one device node, a peer to `LaunchpadNode` (ADR-031: surfaces are device
//! nodes; all clients multiplex last-writer-wins at W0, per-client nodes are W4).
//!
//! Data flow mirrors `LaunchpadNode` exactly:
//! - inbound: per-client WS read threads push pre-decoded `SurfaceEvent`s into
//!   fixed rtrb rings; `process()` drains all rings each cycle (audio thread,
//!   no JSON, no allocation).
//! - outbound: `update_output()`/script LEDs reach `TheoriaOutputHandle` on
//!   the main thread, which forwards batches to kerygma for fan-out.

use paraclete_node_api::{
    CapabilityDocument, Control, EncoderBehaviour, EncoderDescriptor, Event, Node,
    PadDescriptor, PortDescriptor, PortDirection, PortName, PortType, ProcessInput,
    ProcessOutput, Surface, SurfaceDescriptor, SurfaceEvent, SurfaceOutput,
    SurfaceOutputHandle, TimedEvent,
};

use crate::kerygma::{Kerygma, MAX_CLIENTS};
use crate::protocol::ClientMsg;

/// First touch-encoder control id (ids 90–97).
pub const ENCODER_ID_BASE: u32 = 90;
/// Number of touch encoders in the encoder row.
pub const ENCODER_COUNT: u32 = 8;

/// Inbound ring capacity per client slot.
pub const INBOUND_RING_CAPACITY: usize = 256;

/// Max events drained across all slots in one `process()` call. The
/// executor's per-node `EventOutputBuffer` holds 256 and *asserts* on
/// overflow; with 4 slot rings of 256 a multi-client burst could exceed it.
/// Leftovers stay in the rings for the next cycle (~5 ms later).
pub const DRAIN_BUDGET_PER_CYCLE: usize = 250;

// ── Wire → surface event mapping ──────────────────────────────────────────────

/// Translate a decoded client message into the `SurfaceEvent` the graph sees.
///
/// Event types match what the *physical* `LaunchpadNode` emits for the same
/// control ids — grid pads (0–63) are pads, scene (64–71) and control row
/// (72–79) are buttons — so the unmodified profile serves both surfaces.
/// Runs on WS read threads only.
///
/// Returns `None` for messages that carry no surface event (`hello`, `ping`,
/// semantic plane) and for out-of-range ids.
pub fn client_msg_to_surface_event(msg: &ClientMsg) -> Option<SurfaceEvent> {
    match msg {
        ClientMsg::PadDown { id, vel } if *id < 64 => {
            Some(SurfaceEvent::PadPressed { id: *id, velocity: *vel, pressure: 0 })
        }
        ClientMsg::PadDown { id, .. } if *id < 80 => {
            Some(SurfaceEvent::ButtonPressed { id: *id })
        }
        ClientMsg::PadUp { id } if *id < 64 => Some(SurfaceEvent::PadReleased { id: *id }),
        ClientMsg::PadUp { id } if *id < 80 => Some(SurfaceEvent::ButtonReleased { id: *id }),
        ClientMsg::Enc { id, delta }
            if (ENCODER_ID_BASE..ENCODER_ID_BASE + ENCODER_COUNT).contains(id) =>
        {
            let delta = (*delta).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            Some(SurfaceEvent::EncoderChanged { id: *id, value: 0, delta })
        }
        ClientMsg::EncPush { id, pressed }
            if (ENCODER_ID_BASE..ENCODER_ID_BASE + ENCODER_COUNT).contains(id) =>
        {
            Some(SurfaceEvent::EncoderPush { id: *id, pressed: *pressed })
        }
        _ => None,
    }
}

// ── Descriptor ────────────────────────────────────────────────────────────────

fn build_descriptor() -> SurfaceDescriptor {
    let mut controls = Vec::new();
    // 64 grid pads, row-major ids 0–63 (row 0 = top) — same map as the
    // Launchpad and the P9.5 emulator.
    for row in 0u8..8 {
        for col in 0u8..8 {
            controls.push(Control::Pad(PadDescriptor {
                id: (row as u32) * 8 + (col as u32),
                name: PortName::Static("pad"),
                row: Some(row),
                col: Some(col),
                velocity_sensitive: true,
                // WQ-2: browser touch has no finger-pressure API.
                pressure_sensitive: false,
                rgb: true,
            }));
        }
    }
    // 8 scene pads (right column): ids 64–71.
    for row in 0u8..8 {
        controls.push(Control::Pad(PadDescriptor {
            id: 64 + row as u32,
            name: PortName::Static("scene"),
            row: Some(row),
            col: None,
            velocity_sensitive: false,
            pressure_sensitive: false,
            rgb: true,
        }));
    }
    // 8 control-row pads (top): ids 72–79.
    for col in 0u8..8 {
        controls.push(Control::Pad(PadDescriptor {
            id: 72 + col as u32,
            name: PortName::Static("top"),
            row: None,
            col: Some(col),
            velocity_sensitive: false,
            pressure_sensitive: false,
            rgb: true,
        }));
    }
    // 8 relative touch encoders, ids 90–97, declared at W0 so W1 adds no
    // descriptor change (relative-only: ADR-031 named decision).
    for i in 0..ENCODER_COUNT {
        controls.push(Control::Encoder(EncoderDescriptor {
            id: ENCODER_ID_BASE + i,
            name: PortName::Static("enc"),
            has_push: true,
            behaviour: EncoderBehaviour::Relative,
        }));
    }
    SurfaceDescriptor { name: "Theoria Surface", vendor: "Paraclete", controls }
}

// ── Output handle ─────────────────────────────────────────────────────────────

/// Main-thread output half: drains the device-internal SPSC and forwards LED
/// batches to kerygma. Also services full-surface replays for new clients.
pub struct TheoriaOutputHandle {
    rx: rtrb::Consumer<SurfaceOutput>,
    kerygma: Kerygma,
}

impl SurfaceOutputHandle for TheoriaOutputHandle {
    fn tick(&mut self) {
        self.kerygma.service_replays();
        while let Ok(output) = self.rx.pop() {
            self.kerygma.broadcast_led_updates(&output.led_updates);
        }
    }

    fn deliver(&mut self, output: SurfaceOutput) {
        self.kerygma.broadcast_led_updates(&output.led_updates);
    }
}

// ── Node ──────────────────────────────────────────────────────────────────────

pub struct TheoriaSurfaceNode {
    ports: [PortDescriptor; 1],
    node_id: u32,
    /// One consumer per client slot; producers live with the WS read threads.
    /// Fixed length `MAX_CLIENTS` — iteration in `process()` never allocates.
    slots: Vec<rtrb::Consumer<SurfaceEvent>>,
    out_tx: rtrb::Producer<SurfaceOutput>,
    out_rx: Option<rtrb::Consumer<SurfaceOutput>>,
    kerygma: Option<Kerygma>,
    descriptor: SurfaceDescriptor,
}

impl TheoriaSurfaceNode {
    /// Build the node and the producer ends of its inbound rings (one per
    /// client slot, in slot order). The server owns the producers.
    pub fn new(kerygma: Kerygma) -> (Self, Vec<rtrb::Producer<SurfaceEvent>>) {
        let mut consumers = Vec::with_capacity(MAX_CLIENTS);
        let mut producers = Vec::with_capacity(MAX_CLIENTS);
        for _ in 0..MAX_CLIENTS {
            let (tx, rx) = rtrb::RingBuffer::<SurfaceEvent>::new(INBOUND_RING_CAPACITY);
            producers.push(tx);
            consumers.push(rx);
        }
        let (out_tx, out_rx) = rtrb::RingBuffer::<SurfaceOutput>::new(64);
        let node = Self {
            ports: [PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            node_id: 0,
            slots: consumers,
            out_tx,
            out_rx: Some(out_rx),
            kerygma: Some(kerygma),
            descriptor: build_descriptor(),
        };
        (node, producers)
    }
}

impl Node for TheoriaSurfaceNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn type_name(&self) -> &'static str {
        "TheoriaSurfaceNode"
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "TheoriaSurfaceNode",
            vendor: "Paraclete",
            version: (0, 1, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec!["paraclete.hardware"],
        }
    }

    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        // Drain every slot ring each cycle: fixed iteration, no allocation,
        // no JSON — events were decoded on the WS threads. The budget keeps a
        // multi-client burst below the executor's event-buffer assert; lower
        // slots drain first, which is acceptable last-writer-wins bias at W0.
        let mut budget = DRAIN_BUDGET_PER_CYCLE;
        for slot in &mut self.slots {
            while budget > 0 {
                match slot.pop() {
                    Ok(ev) => {
                        output.events_out.push(TimedEvent::new(0, Event::Surface(ev)));
                        budget -= 1;
                    }
                    Err(_) => break,
                }
            }
            if budget == 0 {
                break;
            }
        }
    }
}

impl Surface for TheoriaSurfaceNode {
    fn descriptor(&self) -> &SurfaceDescriptor {
        &self.descriptor
    }

    fn update_output(&mut self, output: &SurfaceOutput) {
        // Legacy audio-thread path (not used once the handle is taken);
        // mirror LaunchpadNode: forward through the device-internal SPSC.
        let _ = self.out_tx.push(output.clone());
    }

    fn take_output_handle(&mut self) -> Option<Box<dyn SurfaceOutputHandle>> {
        let rx = self.out_rx.take()?;
        let kerygma = self.kerygma.take()?;
        Some(Box::new(TheoriaOutputHandle { rx, kerygma }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kerygma::ClientTable;
    use crate::protocol::{LedMsg, ServerMsg};
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, LedUpdate, RgbColor, TransportInfo,
    };
    use std::sync::{mpsc, Arc, Mutex};

    fn run_process(node: &mut TheoriaSurfaceNode, capacity: usize) -> Vec<TimedEvent> {
        let transport = TransportInfo::default();
        let extended = ExtendedEventSlab::empty();
        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: 512,
            extended_events: &extended,
            commands: &[],
        };
        let mut audio_refs: Vec<&mut AudioBuffer> = vec![];
        let mut ev_out = EventOutputBuffer::new(capacity);
        let mut output = ProcessOutput {
            audio_outputs: &mut audio_refs,
            signal_outputs: &mut [],
            events_out: &mut ev_out,
        };
        node.process(&input, &mut output);
        ev_out.as_slice().to_vec()
    }

    fn new_node() -> (TheoriaSurfaceNode, Vec<rtrb::Producer<SurfaceEvent>>) {
        let clients = Arc::new(Mutex::new(ClientTable::new()));
        TheoriaSurfaceNode::new(Kerygma::new(clients))
    }

    #[test]
    fn pad_msg_to_hardware_event_mapping() {
        // Grid pads → pad events.
        assert!(matches!(
            client_msg_to_surface_event(&ClientMsg::PadDown { id: 13, vel: 65535 }),
            Some(SurfaceEvent::PadPressed { id: 13, velocity: 65535, pressure: 0 })
        ));
        assert!(matches!(
            client_msg_to_surface_event(&ClientMsg::PadUp { id: 13 }),
            Some(SurfaceEvent::PadReleased { id: 13 })
        ));
        // Scene + control row → button events (matches LaunchpadNode; the
        // profile keys SHIFT/SEQ_EDIT on ButtonPressed).
        assert!(matches!(
            client_msg_to_surface_event(&ClientMsg::PadDown { id: 64, vel: 65535 }),
            Some(SurfaceEvent::ButtonPressed { id: 64 })
        ));
        assert!(matches!(
            client_msg_to_surface_event(&ClientMsg::PadUp { id: 79 }),
            Some(SurfaceEvent::ButtonReleased { id: 79 })
        ));
        // Encoders (mapping exists at W0; the server gates delivery until W1).
        assert!(matches!(
            client_msg_to_surface_event(&ClientMsg::Enc { id: 90, delta: -3 }),
            Some(SurfaceEvent::EncoderChanged { id: 90, delta: -3, .. })
        ));
        assert!(matches!(
            client_msg_to_surface_event(&ClientMsg::EncPush { id: 97, pressed: true }),
            Some(SurfaceEvent::EncoderPush { id: 97, pressed: true })
        ));
        // Out-of-range ids and non-surface messages map to nothing.
        assert!(client_msg_to_surface_event(&ClientMsg::PadDown { id: 80, vel: 1 }).is_none());
        assert!(client_msg_to_surface_event(&ClientMsg::Enc { id: 89, delta: 1 }).is_none());
        assert!(client_msg_to_surface_event(&ClientMsg::Ping { ts: 1.0 }).is_none());
        // pad_pres is reserved [W1+]: parse-and-drop.
        assert!(client_msg_to_surface_event(&ClientMsg::PadPres { id: 13, v: 41000 }).is_none());
    }

    #[test]
    fn surface_descriptor_counts() {
        let (node, _producers) = new_node();
        let desc = node.descriptor();
        assert_eq!(desc.name, "Theoria Surface");

        let pad_ids: Vec<u32> = desc
            .controls
            .iter()
            .filter_map(|c| match c {
                Control::Pad(p) => Some(p.id),
                _ => None,
            })
            .collect();
        assert_eq!(pad_ids.len(), 80, "64 grid + 8 scene + 8 control pads");
        assert_eq!(pad_ids, (0..80).collect::<Vec<u32>>(), "pad ids exact and ordered");

        let encoders: Vec<&EncoderDescriptor> = desc
            .controls
            .iter()
            .filter_map(|c| match c {
                Control::Encoder(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(encoders.len(), 8);
        for (i, e) in encoders.iter().enumerate() {
            assert_eq!(e.id, ENCODER_ID_BASE + i as u32);
            assert_eq!(e.behaviour, EncoderBehaviour::Relative);
            assert!(e.has_push);
        }
    }

    #[test]
    fn node_process_drains_all_slots() {
        let (mut node, mut producers) = new_node();
        producers[0]
            .push(SurfaceEvent::PadPressed { id: 1, velocity: 65535, pressure: 0 })
            .unwrap();
        producers[3]
            .push(SurfaceEvent::PadReleased { id: 9 })
            .unwrap();

        let events = run_process(&mut node, 16);
        assert_eq!(events.len(), 2, "events from both slots emerge in one process()");
        assert!(matches!(
            events[0].event,
            Event::Surface(SurfaceEvent::PadPressed { id: 1, .. })
        ));
        assert!(matches!(
            events[1].event,
            Event::Surface(SurfaceEvent::PadReleased { id: 9 })
        ));
    }

    #[test]
    fn node_process_is_allocation_free_shape() {
        // Zero clients connected (producers dropped): fixed-size iteration
        // over the slot rings completes without panic and emits nothing.
        let (mut node, producers) = new_node();
        drop(producers);
        let events = run_process(&mut node, 16);
        assert!(events.is_empty());
    }

    #[test]
    fn process_drain_respects_event_budget() {
        // A multi-client burst larger than the executor's 256-event buffer
        // must not overflow it (the buffer asserts): drain caps at the budget
        // and the remainder emerges on the next cycle.
        let (mut node, mut producers) = new_node();
        for _ in 0..INBOUND_RING_CAPACITY {
            producers[0].push(SurfaceEvent::PadReleased { id: 1 }).unwrap();
        }
        for _ in 0..INBOUND_RING_CAPACITY {
            producers[1].push(SurfaceEvent::PadReleased { id: 2 }).unwrap();
        }

        // 256-capacity buffer, same as the executor allocates per node.
        let first = run_process(&mut node, 256);
        assert_eq!(first.len(), DRAIN_BUDGET_PER_CYCLE, "drain stops at the budget");

        let second = run_process(&mut node, 256);
        let third = run_process(&mut node, 256);
        assert_eq!(
            first.len() + second.len() + third.len(),
            2 * INBOUND_RING_CAPACITY,
            "no events are lost — leftovers drain on later cycles"
        );
    }

    #[test]
    fn output_handle_forwards_led_updates() {
        let clients = Arc::new(Mutex::new(ClientTable::new()));
        let (mut node, _producers) = TheoriaSurfaceNode::new(Kerygma::new(Arc::clone(&clients)));

        // A connected client, replay already consumed.
        let (tx, rx) = mpsc::channel();
        clients.lock().unwrap().allocate(tx).unwrap();

        let mut handle = node.take_output_handle().expect("P4+ path");
        handle.tick(); // services the connect replay
        let _ = rx.try_recv();

        // Audio-thread side pushes a SurfaceOutput; tick() forwards it.
        node.update_output(&SurfaceOutput {
            led_updates: vec![LedUpdate { control_id: 5, color: RgbColor::BLUE }],
            display_updates: vec![],
        });
        handle.tick();

        let frame = rx.try_recv().expect("led batch reaches the client sender");
        let ServerMsg::Led { updates } = serde_json::from_str(&frame).unwrap() else {
            panic!("expected led frame, got {frame}")
        };
        assert_eq!(updates, vec![LedMsg { id: 5, rgb: [0, 0, 255] }]);

        // deliver() (script LED path) takes the same route.
        handle.deliver(SurfaceOutput {
            led_updates: vec![LedUpdate { control_id: 6, color: RgbColor::RED }],
            display_updates: vec![],
        });
        let frame = rx.try_recv().expect("delivered batch reaches the client");
        let ServerMsg::Led { updates } = serde_json::from_str(&frame).unwrap() else {
            panic!("expected led frame")
        };
        assert_eq!(updates, vec![LedMsg { id: 6, rgb: [255, 0, 0] }]);
    }

    #[test]
    fn take_output_handle_is_single_shot() {
        let (mut node, _producers) = new_node();
        assert!(node.take_output_handle().is_some());
        assert!(node.take_output_handle().is_none(), "second take yields None");
    }
}
