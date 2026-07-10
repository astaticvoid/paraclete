// SPDX-License-Identifier: GPL-3.0-or-later
//! Arturia Keystep 37 — Surface node via MIDI.
//!
//! Standard MIDI over USB. Note On/Off → `Event::Midi2`. Mod wheel → FaderMoved.
//! No LED output — `take_output_handle()` returns `None`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use paraclete_node_api::{
    CapabilityDocument, Control, Surface,
    SurfaceEvent, SurfaceOutput, Node, PortDescriptor, PortDirection, PortName,
    PortType, ProcessInput, ProcessOutput, SurfaceDescriptor, TimedEvent, Event,
    UmpMessage, FaderDescriptor,
};
use paraclete_node_api::midi::{
    ChannelVoice2, Channeled, Grouped, NoteOn, NoteOff, u4, u7,
};

use crate::midi::{MidiDeviceError, MidiDeviceRegistry};

fn build_note_on(note: u8, velocity: u8) -> UmpMessage {
    let mut msg = NoteOn::<[u32; 4]>::new();
    msg.set_group(u4::new(0));
    msg.set_channel(u4::new(0));
    msg.set_note_number(u7::new(note & 0x7F));
    msg.set_velocity((velocity as u16) << 9);
    UmpMessage::from(ChannelVoice2::from(msg))
}

fn build_note_off(note: u8) -> UmpMessage {
    let mut msg = NoteOff::<[u32; 4]>::new();
    msg.set_group(u4::new(0));
    msg.set_channel(u4::new(0));
    msg.set_note_number(u7::new(note & 0x7F));
    msg.set_velocity(0);
    UmpMessage::from(ChannelVoice2::from(msg))
}

fn build_surface() -> SurfaceDescriptor {
    SurfaceDescriptor {
        name: "Keystep 37",
        vendor: "Arturia",
        controls: vec![
            // Pitch bend as fader id=0
            Control::Fader(FaderDescriptor { id: 0, name: PortName::Static("pitch_bend"), motorised: false }),
            // Mod wheel as fader id=1
            Control::Fader(FaderDescriptor { id: 1, name: PortName::Static("mod_wheel"), motorised: false }),
        ],
    }
}

/// Arturia Keystep 37 as a `Surface: Node`.
pub struct KeystepNode {
    ports: [PortDescriptor; 1],
    node_id: u32,
    _conn_in: midir::MidiInputConnection<()>,
    incoming: Arc<Mutex<VecDeque<TimedEvent>>>,
    surface: SurfaceDescriptor,
}

impl KeystepNode {
    pub fn open() -> Result<Self, MidiDeviceError> {
        let incoming = Arc::new(Mutex::new(VecDeque::<TimedEvent>::new()));
        let incoming_cb = Arc::clone(&incoming);

        let reg = MidiDeviceRegistry::new()?;
        let conn_in = reg.open_input("Keystep", move |_ts, bytes, _| {
            for ev in parse_keystep_midi(bytes) {
                if let Ok(mut q) = incoming_cb.try_lock() {
                    q.push_back(ev);
                }
            }
        })?;

        Ok(Self {
            ports: [PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            node_id: 0,
            _conn_in: conn_in,
            incoming,
            surface: build_surface(),
        })
    }
}

fn parse_keystep_midi(bytes: &[u8]) -> Vec<TimedEvent> {
    if bytes.len() < 2 { return vec![]; }
    let status = bytes[0] & 0xF0;
    let note = bytes[1];
    let vel  = bytes.get(2).copied().unwrap_or(0);

    match status {
        0x90 if vel > 0 => vec![TimedEvent::new(0, Event::Midi2(build_note_on(note, vel)))],
        0x90 | 0x80     => vec![TimedEvent::new(0, Event::Midi2(build_note_off(note)))],
        0xB0 if note == 1 => {
            // Mod wheel CC1
            vec![TimedEvent::new(0, Event::Surface(SurfaceEvent::FaderMoved {
                id: 1,
                value: (vel as u16) << 9,
            }))]
        }
        0xE0 => {
            // Pitch bend: 14-bit from two bytes
            let pb = if bytes.len() >= 3 {
                ((bytes[2] as u16) << 7) | (bytes[1] as u16)
            } else { 0 };
            vec![TimedEvent::new(0, Event::Surface(SurfaceEvent::FaderMoved {
                id: 0,
                value: pb,
            }))]
        }
        _ => vec![],
    }
}

impl Node for KeystepNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "KeystepNode",
            vendor: "Paraclete/Arturia",
            version: (0, 4, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec!["paraclete.hardware"],
        }
    }

    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        if let Ok(mut q) = self.incoming.try_lock() {
            while let Some(ev) = q.pop_front() {
                output.events_out.push(ev);
            }
        }
    }
}

impl Surface for KeystepNode {
    fn descriptor(&self) -> &SurfaceDescriptor { &self.surface }
    fn update_output(&mut self, _: &SurfaceOutput) {}
    // No output handle — Keystep has no LED feedback.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keystep_note_on_produces_midi2_event() {
        let events = parse_keystep_midi(&[0x90, 60, 100]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, Event::Midi2(_)));
    }

    #[test]
    fn keystep_note_off_via_zero_velocity() {
        let events = parse_keystep_midi(&[0x90, 60, 0]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, Event::Midi2(_)));
    }

    #[test]
    fn keystep_mod_wheel_produces_fader_event() {
        let events = parse_keystep_midi(&[0xB0, 1, 64]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, Event::Surface(SurfaceEvent::FaderMoved { id: 1, .. })));
    }
}
