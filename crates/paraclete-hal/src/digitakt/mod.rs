// SPDX-License-Identifier: GPL-3.0-or-later
//! Elektron Digitakt in MIDI mode — Surface node.
//!
//! Encoders use relative encoding (127=CCW, 1–63=CW).
//! No LED output — `take_output_handle()` returns `None`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use paraclete_node_api::{
    ButtonDescriptor, CapabilityDocument, Control, EncoderBehaviour, EncoderDescriptor, Event,
    Node, PortDescriptor, PortDirection, PortName, PortType, ProcessInput, ProcessOutput, Surface,
    SurfaceDescriptor, SurfaceEvent, SurfaceOutput, TimedEvent,
};

use crate::midi::{MidiDeviceError, MidiDeviceRegistry};

fn build_surface() -> SurfaceDescriptor {
    let mut controls = Vec::new();
    // 8 track encoders (relative), ids 0–7
    for i in 0u32..8 {
        controls.push(Control::Encoder(EncoderDescriptor {
            id: i,
            name: PortName::Static("encoder"),
            has_push: false,
            behaviour: EncoderBehaviour::Relative,
        }));
    }
    // 8 track buttons, ids 8–15
    for i in 0u32..8 {
        controls.push(Control::Button(ButtonDescriptor {
            id: 8 + i,
            name: PortName::Static("track_btn"),
            rgb: false,
        }));
    }
    // Transport: Play=16, Stop=17, Record=18
    for (i, name) in [(16u32, "play"), (17, "stop"), (18, "record")] {
        controls.push(Control::Button(ButtonDescriptor {
            id: i,
            name: PortName::Static(name),
            rgb: false,
        }));
    }
    SurfaceDescriptor {
        name: "Digitakt MIDI".into(),
        vendor: "Elektron".into(),
        controls,
    }
}

/// Digitakt MIDI mode as a `Surface: Node`.
pub struct DigitaktMidiNode {
    ports: [PortDescriptor; 1],
    node_id: u32,
    _conn_in: midir::MidiInputConnection<()>,
    incoming: Arc<Mutex<VecDeque<SurfaceEvent>>>,
    surface: SurfaceDescriptor,
}

impl DigitaktMidiNode {
    pub fn open() -> Result<Self, MidiDeviceError> {
        let incoming = Arc::new(Mutex::new(VecDeque::<SurfaceEvent>::new()));
        let incoming_cb = Arc::clone(&incoming);

        let reg = MidiDeviceRegistry::new()?;
        let conn_in = reg.open_input("Digitakt", move |_ts, bytes, _| {
            if let Some(ev) = parse_digitakt_midi(bytes) {
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

/// Decode relative encoder CC value from Digitakt.
/// Value 64 = center/no-movement. Value 0 should not occur in relative mode.
fn decode_relative_delta(value: u8) -> i16 {
    match value {
        1..=63 => value as i16,
        65..=127 => (value as i16) - 128,
        0 | 64 => 0, // 64 = no movement (center detent); 0 should not occur
        _ => 0,      // values > 127 are not valid 7-bit MIDI CC
    }
}

fn parse_digitakt_midi(bytes: &[u8]) -> Option<SurfaceEvent> {
    if bytes.len() < 3 {
        return None;
    }
    let status = bytes[0] & 0xF0;
    let note = bytes[1];
    let vel = bytes[2];

    match status {
        0xB0 => {
            // CC: encoders (CC 0–7 or similar mapping)
            let delta = decode_relative_delta(vel);
            Some(SurfaceEvent::EncoderChanged {
                id: note as u32,
                value: 0,
                delta,
            })
        }
        0x90 if vel > 0 => Some(SurfaceEvent::ButtonPressed { id: note as u32 }),
        0x90 | 0x80 => Some(SurfaceEvent::ButtonReleased { id: note as u32 }),
        _ => None,
    }
}

impl Node for DigitaktMidiNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }
    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "DigitaktMidiNode".into(),
            vendor: "Paraclete/Elektron".into(),
            version: (0, 4, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec!["paraclete.hardware".into()],
            view: None,
        }
    }

    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        if let Ok(mut q) = self.incoming.try_lock() {
            while let Some(ev) = q.pop_front() {
                output
                    .events_out
                    .push(TimedEvent::new(0, Event::Surface(ev)));
            }
        }
    }
}

impl Surface for DigitaktMidiNode {
    fn descriptor(&self) -> &SurfaceDescriptor {
        &self.surface
    }
    fn update_output(&mut self, _: &SurfaceOutput) {}
    // No output handle — Digitakt has no LED feedback in standard MIDI mode.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digitakt_decode_relative_clockwise() {
        assert_eq!(decode_relative_delta(1), 1);
        assert_eq!(decode_relative_delta(63), 63);
    }

    #[test]
    fn digitakt_decode_relative_counter_clockwise() {
        assert_eq!(decode_relative_delta(127), -1);
        assert_eq!(decode_relative_delta(65), -63);
    }

    #[test]
    fn digitakt_decode_center_detent() {
        assert_eq!(decode_relative_delta(64), 0);
    }
}
