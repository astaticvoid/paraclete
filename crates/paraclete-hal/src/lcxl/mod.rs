// SPDX-License-Identifier: GPL-3.0-or-later
//! Novation Launch Control XL hardware node.
//!
//! Translates Factory Template 1 MIDI CC and Note messages into `SurfaceEvent`s
//! via a lock-free `rtrb` ring buffer (zero audio-thread locks or allocations)
//! and drives button/knob LEDs on the main thread via `LcxlOutputHandle`.
//!
//! ## Control ID Map
//!
//! ### Faders & Pots (`SurfaceEvent::FaderMoved { id, value }` — 16-bit `0..=65535`)
//! - `0..=7`:   Row 1 Pots (Send A, CC 13..=20)
//! - `8..=15`:  Row 2 Pots (Send B, CC 29..=36)
//! - `16..=23`: Row 3 Pots (Pan/Device, CC 49..=56)
//! - `24..=31`: Channel Faders 1–8 (CC 77..=84)
//!
//! ### Buttons (`SurfaceEvent::ButtonPressed` / `ButtonReleased { id }`)
//! - `0..=7`:   Bottom Row 1 (Track Focus 1–8, Notes 41..=44, 57..=60)
//! - `8..=15`:  Bottom Row 2 (Track Control 1–8, Notes 73..=76, 89..=92)
//! - `16`:      Device button (Note 105)
//! - `17`:      Mute button (Note 106)
//! - `18`:      Solo button (Note 107)
//! - `19`:      Record Arm button (Note 108)
//! - `20`:      Up button (CC 104)
//! - `21`:      Down button (CC 105)
//! - `22`:      Left button (CC 106)
//! - `23`:      Right button (CC 107)
//!
//! ### LEDs (`LedUpdate { control_id, color }`)
//! - `0..=19`:  Bottom Row 1 & 2 buttons + Device/Mute/Solo/Record Arm
//! - `20..=23`: Up/Down/Left/Right directional buttons
//! - `24..=47`: Row 1–3 pot LEDs (via Launch Control XL SysEx)

use midir::{MidiInputConnection, MidiOutputConnection};
use rtrb::{Consumer, Producer, RingBuffer};

use paraclete_node_api::{
    ButtonDescriptor, CapabilityDocument, Control, Event, FaderDescriptor, LedDescriptor, Node,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, RgbColor, Surface,
    SurfaceDescriptor, SurfaceEvent, SurfaceOutput, SurfaceOutputHandle, TimedEvent,
};

use crate::midi::{MidiDeviceError, MidiDeviceRegistry};

const PORT_PATTERN: &str = "Launch Control XL";

/// Number of physical pots (24) + faders (8).
pub const LCXL_FADER_COUNT: u32 = 32;
/// Number of physical buttons (16 bottom + 4 mode + 4 directional).
pub const LCXL_BUTTON_COUNT: u32 = 24;
/// Total addressable LEDs (24 buttons + 24 pots).
pub const LCXL_LED_COUNT: u32 = 48;

/// Factory Template 1 default 0-indexed MIDI channel (Channel 9 = 0x08).
const FACTORY_TEMPLATE_CHANNEL: u8 = 0x08;

/// Bottom row 1 & 2 + mode button note numbers in `id` order `0..=19`.
pub const BUTTON_NOTES: [u8; 20] = [
    // Row 1 (Track Focus 1..8) -> id 0..=7
    41, 42, 43, 44, 57, 58, 59, 60,
    // Row 2 (Track Control 1..8) -> id 8..=15
    73, 74, 75, 76, 89, 90, 91, 92,
    // Mode buttons: Device, Mute, Solo, Record Arm -> id 16..=19
    105, 106, 107, 108,
];

/// Convert 7-bit MIDI CC (`0..=127`) to 16-bit `0..=65535`.
#[inline]
pub fn cc7_to_u16(val: u8) -> u16 {
    let clamped = val.min(127) as u32;
    ((clamped * 65535 + 63) / 127) as u16
}

/// Map a Launch Control XL CC number (`data1`) and value (`data2`) to a `SurfaceEvent`.
pub fn decode_lcxl_cc(cc: u8, val: u8) -> Option<SurfaceEvent> {
    let value = cc7_to_u16(val);
    match cc {
        // Row 1 Pots (Send A) -> id 0..=7
        13..=20 => Some(SurfaceEvent::FaderMoved {
            id: (cc - 13) as u32,
            value,
        }),
        // Row 2 Pots (Send B) -> id 8..=15
        29..=36 => Some(SurfaceEvent::FaderMoved {
            id: (8 + (cc - 29)) as u32,
            value,
        }),
        // Row 3 Pots (Pan/Device) -> id 16..=23
        49..=56 => Some(SurfaceEvent::FaderMoved {
            id: (16 + (cc - 49)) as u32,
            value,
        }),
        // Channel Faders 1..8 -> id 24..=31
        77..=84 => Some(SurfaceEvent::FaderMoved {
            id: (24 + (cc - 77)) as u32,
            value,
        }),
        // Directional buttons (Up, Down, Left, Right) -> id 20..=23
        104..=107 => {
            let id = (20 + (cc - 104)) as u32;
            if val > 0 {
                Some(SurfaceEvent::ButtonPressed { id })
            } else {
                Some(SurfaceEvent::ButtonReleased { id })
            }
        }
        _ => None,
    }
}

/// Map a Launch Control XL Note number (`note`) and pressed state to a `SurfaceEvent`.
pub fn decode_lcxl_note(note: u8, pressed: bool) -> Option<SurfaceEvent> {
    let id = BUTTON_NOTES.iter().position(|&n| n == note)? as u32;
    if pressed {
        Some(SurfaceEvent::ButtonPressed { id })
    } else {
        Some(SurfaceEvent::ButtonReleased { id })
    }
}

/// Decode a raw 3-byte MIDI message from Launch Control XL into a `SurfaceEvent`.
pub fn decode_lcxl_midi(bytes: &[u8]) -> Option<SurfaceEvent> {
    if bytes.len() < 3 {
        return None;
    }
    let status_hi = bytes[0] & 0xF0;
    let data1 = bytes[1];
    let data2 = bytes[2];
    match status_hi {
        0xB0 => decode_lcxl_cc(data1, data2),
        0x90 => decode_lcxl_note(data1, data2 > 0),
        0x80 => decode_lcxl_note(data1, false),
        _ => None,
    }
}

/// Map an `RgbColor` to a Launch Control XL bi-color velocity byte:
/// - `12` (`0x0C`): Off
/// - `15` (`0x0F`): Red Full (`13` = Red Low)
/// - `60` (`0x3C`): Green Full (`28` = Green Low)
/// - `62` (`0x3E`): Yellow Full
/// - `63` (`0x3F`): Amber Full (`29` = Amber Low)
pub fn rgb_to_lcxl_velocity(color: RgbColor) -> u8 {
    if color.r == 0 && color.g == 0 && color.b == 0 {
        12 // Off
    } else if color.r > 0 && color.g > 0 {
        if color.g > color.r {
            62 // Yellow Full
        } else if color.r >= 128 || color.g >= 128 {
            63 // Amber Full
        } else {
            29 // Amber Low
        }
    } else if color.r > 0 {
        if color.r >= 128 {
            15 // Red Full
        } else {
            13 // Red Low
        }
    } else if color.g > 0 || color.b > 0 {
        if color.g >= 128 || color.b >= 128 {
            60 // Green Full
        } else {
            28 // Green Low
        }
    } else {
        12
    }
}

// ── LcxlOutputHandle ──────────────────────────────────────────────────────────

pub struct LcxlOutputHandle {
    conn_out: Option<MidiOutputConnection>,
    rx: Consumer<SurfaceOutput>,
}

impl LcxlOutputHandle {
    fn send_led_updates(&mut self, output: &SurfaceOutput) {
        let Some(conn) = self.conn_out.as_mut() else {
            return;
        };
        for update in &output.led_updates {
            let led_id = update.control_id;
            let vel = rgb_to_lcxl_velocity(update.color);
            if (led_id as usize) < BUTTON_NOTES.len() {
                let note = BUTTON_NOTES[led_id as usize];
                let msg = [0x90 | FACTORY_TEMPLATE_CHANNEL, note, vel];
                if let Err(e) = conn.send(&msg) {
                    log::warn!("[lcxl] MIDI LED note send error: {e}");
                }
            } else if (20..24).contains(&led_id) {
                let cc = (104 + (led_id - 20)) as u8;
                let msg = [0xB0 | FACTORY_TEMPLATE_CHANNEL, cc, vel];
                if let Err(e) = conn.send(&msg) {
                    log::warn!("[lcxl] MIDI LED CC send error: {e}");
                }
            } else if (24..48).contains(&led_id) {
                let pot_idx = (led_id - 24) as u8;
                let sysex = [
                    0xF0,
                    0x00,
                    0x20,
                    0x29,
                    0x02,
                    0x11,
                    0x78,
                    FACTORY_TEMPLATE_CHANNEL,
                    pot_idx,
                    vel,
                    0xF7,
                ];
                if let Err(e) = conn.send(&sysex) {
                    log::warn!("[lcxl] MIDI LED SysEx send error: {e}");
                }
            }
        }
    }
}

impl SurfaceOutputHandle for LcxlOutputHandle {
    fn tick(&mut self) {
        while let Ok(output) = self.rx.pop() {
            self.send_led_updates(&output);
        }
    }

    fn deliver(&mut self, output: SurfaceOutput) {
        self.send_led_updates(&output);
    }
}

// ── LcxlNode ──────────────────────────────────────────────────────────────────

/// Graph node wrapping a Novation Launch Control XL MIDI controller.
pub struct LcxlNode {
    ports: [PortDescriptor; 1],
    node_id: u32,
    _conn_in: Option<MidiInputConnection<()>>,
    conn_out: Option<MidiOutputConnection>,
    incoming_rx: Consumer<SurfaceEvent>,
    hw_out_rx: Option<Consumer<SurfaceOutput>>,
    surface: SurfaceDescriptor,
}

impl LcxlNode {
    /// Open the Launch Control XL input and output ports via `MidiDeviceRegistry`.
    pub fn open() -> Result<Self, MidiDeviceError> {
        let (mut incoming_tx, incoming_rx) = RingBuffer::<SurfaceEvent>::new(256);

        let reg_out = MidiDeviceRegistry::new()?;
        let conn_out = reg_out.open_output(PORT_PATTERN)?;

        let reg_in = MidiDeviceRegistry::new()?;
        let conn_in = reg_in.open_input(PORT_PATTERN, move |_ts, bytes, _| {
            if let Some(ev) = decode_lcxl_midi(bytes) {
                let _ = incoming_tx.push(ev);
            }
        })?;

        let (_tx, rx) = RingBuffer::<SurfaceOutput>::new(64);

        Ok(Self {
            ports: [PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            node_id: 0,
            _conn_in: Some(conn_in),
            conn_out: Some(conn_out),
            incoming_rx,
            hw_out_rx: Some(rx),
            surface: build_surface(),
        })
    }

    /// Construct a headless Launch Control XL node with a caller-held event producer
    /// (for deterministic integration tests without hardware).
    pub fn headless() -> (Self, Producer<SurfaceEvent>) {
        let (incoming_tx, incoming_rx) = RingBuffer::<SurfaceEvent>::new(256);
        let (_tx, rx) = RingBuffer::<SurfaceOutput>::new(64);
        let node = Self {
            ports: [PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            node_id: 0,
            _conn_in: None,
            conn_out: None,
            incoming_rx,
            hw_out_rx: Some(rx),
            surface: build_surface(),
        };
        (node, incoming_tx)
    }
}

fn build_surface() -> SurfaceDescriptor {
    let mut controls = Vec::with_capacity(
        (LCXL_FADER_COUNT as usize) + (LCXL_BUTTON_COUNT as usize) + (LCXL_LED_COUNT as usize),
    );
    for id in 0..LCXL_FADER_COUNT {
        controls.push(Control::Fader(FaderDescriptor {
            id,
            name: "fader".into(),
            motorised: false,
        }));
    }
    for id in 0..LCXL_BUTTON_COUNT {
        controls.push(Control::Button(ButtonDescriptor {
            id,
            name: "button".into(),
            rgb: false,
        }));
    }
    for id in 0..LCXL_LED_COUNT {
        controls.push(Control::Led(LedDescriptor {
            id,
            name: "led".into(),
            rgb: false,
        }));
    }
    SurfaceDescriptor {
        name: "Novation Launch Control XL".into(),
        vendor: "Novation".into(),
        controls,
    }
}

impl Node for LcxlNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "LcxlNode".into(),
            vendor: "Paraclete/Novation".into(),
            version: (0, 1, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec!["paraclete.hardware".into()],
            view: None,
        }
    }

    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        while let Ok(ev) = self.incoming_rx.pop() {
            output
                .events_out
                .push(TimedEvent::new(0, Event::Surface(ev)));
        }
    }
}

impl Surface for LcxlNode {
    fn descriptor(&self) -> &SurfaceDescriptor {
        &self.surface
    }

    fn update_output(&mut self, _output: &SurfaceOutput) {}

    fn take_output_handle(&mut self) -> Option<Box<dyn SurfaceOutputHandle>> {
        let rx = self.hw_out_rx.take()?;
        Some(Box::new(LcxlOutputHandle {
            conn_out: self.conn_out.take(),
            rx,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_lcxl_pots_and_faders_maps_all_four_rows() {
        // Row 1 pot 0 (CC 13)
        let Some(SurfaceEvent::FaderMoved { id, value }) = decode_lcxl_midi(&[0xB8, 13, 0]) else {
            panic!("expected FaderMoved");
        };
        assert_eq!(id, 0);
        assert_eq!(value, 0);

        // Row 2 pot 7 (CC 36)
        let Some(SurfaceEvent::FaderMoved { id, value }) = decode_lcxl_midi(&[0xB8, 36, 127])
        else {
            panic!("expected FaderMoved");
        };
        assert_eq!(id, 15);
        assert_eq!(value, 65535);

        // Row 3 pot 3 (CC 52)
        let Some(SurfaceEvent::FaderMoved { id, value }) = decode_lcxl_midi(&[0xB8, 52, 64]) else {
            panic!("expected FaderMoved");
        };
        assert_eq!(id, 19);
        assert_eq!(value, cc7_to_u16(64));

        // Channel Fader 8 (CC 84) -> id 31
        let Some(SurfaceEvent::FaderMoved { id, value }) = decode_lcxl_midi(&[0xB8, 84, 127])
        else {
            panic!("expected FaderMoved");
        };
        assert_eq!(id, 31);
        assert_eq!(value, 65535);
    }

    #[test]
    fn decode_lcxl_buttons_maps_bottom_16_and_mode_and_nav() {
        // Bottom row 1 col 0 (Note 41) -> id 0
        assert!(matches!(
            decode_lcxl_midi(&[0x98, 41, 127]),
            Some(SurfaceEvent::ButtonPressed { id: 0 })
        ));
        assert!(matches!(
            decode_lcxl_midi(&[0x88, 41, 0]),
            Some(SurfaceEvent::ButtonReleased { id: 0 })
        ));
        // Bottom row 2 col 7 (Note 92) -> id 15
        assert!(matches!(
            decode_lcxl_midi(&[0x98, 92, 127]),
            Some(SurfaceEvent::ButtonPressed { id: 15 })
        ));
        // Device button (Note 105) -> id 16
        assert!(matches!(
            decode_lcxl_midi(&[0x98, 105, 127]),
            Some(SurfaceEvent::ButtonPressed { id: 16 })
        ));
        // Left nav button (CC 106) -> id 22
        assert!(matches!(
            decode_lcxl_midi(&[0xB8, 106, 127]),
            Some(SurfaceEvent::ButtonPressed { id: 22 })
        ));
        assert!(matches!(
            decode_lcxl_midi(&[0xB8, 106, 0]),
            Some(SurfaceEvent::ButtonReleased { id: 22 })
        ));
    }

    #[test]
    fn rgb_to_lcxl_velocity_maps_colors() {
        assert_eq!(rgb_to_lcxl_velocity(RgbColor::OFF), 12);
        assert_eq!(rgb_to_lcxl_velocity(RgbColor::RED), 15);
        assert_eq!(rgb_to_lcxl_velocity(RgbColor::GREEN), 60);
        assert_eq!(
            rgb_to_lcxl_velocity(RgbColor {
                r: 255,
                g: 128,
                b: 0
            }),
            63
        );
    }
}
