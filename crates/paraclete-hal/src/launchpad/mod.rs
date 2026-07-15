// SPDX-License-Identifier: GPL-3.0-or-later
//! Novation Launchpad X — Surface node via MIDI.
//!
//! Port selection: Launchpad X exposes two ports; we use the "LPX MIDI" port
//! (standard programmer-mode MIDI), not the "LPX DAW" port.
//!
//! Pad numbering: Launchpad X sends notes 11–88 in a 10×row + col layout.
//! Row 8 (top physical row) → notes 81–88, row 1 (bottom) → notes 11–18.
//! We map these to pad IDs 0–63 with row 0 = top, matching the drum machine
//! convention (top row = Kick).
//!
//! Top round buttons: CC 91–98. Bottom-row round buttons (scene): notes 19,29,…,89.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use paraclete_node_api::{
    CapabilityDocument, Control, Event, LedUpdate, Node, PadDescriptor, PortDescriptor,
    PortDirection, PortName, PortType, ProcessInput, ProcessOutput, RgbColor, Surface,
    SurfaceDescriptor, SurfaceEvent, SurfaceOutput, SurfaceOutputHandle, TimedEvent,
};

use crate::midi::{MidiDeviceError, MidiDeviceRegistry};

// ── Surface ───────────────────────────────────────────────────────────────────

fn build_surface() -> SurfaceDescriptor {
    let mut controls = Vec::new();
    // 64 pads: 8×8 grid, row-major ids 0–63 (row 0 = top)
    for row in 0u8..8 {
        for col in 0u8..8 {
            controls.push(Control::Pad(PadDescriptor {
                id: (row as u32) * 8 + (col as u32),
                name: PortName::Static("pad"),
                row: Some(row),
                col: Some(col),
                velocity_sensitive: true,
                pressure_sensitive: false,
                rgb: true,
            }));
        }
    }
    // 8 scene buttons (right column): ids 64–71
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
    // 8 top buttons: ids 72–79
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
    SurfaceDescriptor {
        name: "Launchpad X".into(),
        vendor: "Novation".into(),
        controls,
    }
}

// ── Note → pad ID conversion ──────────────────────────────────────────────────

/// Launchpad X programmer-mode note → pad ID 0–63 (row 0 = physical top row).
///
/// The device sends notes in a 10×row + col pattern:
///   physical row 8 (top):    notes 81–88
///   physical row 1 (bottom): notes 11–18
///
/// We invert the row so top row = pad row 0 (Kick track in the drum machine).
fn lp_x_note_to_pad_id(note: u8) -> Option<u32> {
    let phys_row = note / 10; // 1–8 (1 = bottom, 8 = top)
    let col = note % 10; // 1–8
    if (1..=8).contains(&phys_row) && (1..=8).contains(&col) {
        let our_row = 8 - phys_row; // invert: phys 8 → row 0, phys 1 → row 7
        Some(our_row as u32 * 8 + (col - 1) as u32)
    } else {
        None // side/scene buttons — handled separately
    }
}

/// Scene button (right column) note → id 64–71 (row 0 = top).
fn lp_x_scene_note_to_id(note: u8) -> Option<u32> {
    let phys_row = note / 10;
    let col = note % 10;
    if col == 9 && (1..=8).contains(&phys_row) {
        let our_row = 8 - phys_row;
        Some(64 + our_row as u32)
    } else {
        None
    }
}

/// Top-button CC number (91–98) → id 72–79.
fn lp_x_top_cc_to_id(cc: u8) -> Option<u32> {
    if (91..=98).contains(&cc) {
        Some(72 + (cc - 91) as u32)
    } else {
        None
    }
}

// ── MIDI parsing ──────────────────────────────────────────────────────────────

fn parse_launchpad_midi(bytes: &[u8]) -> Option<SurfaceEvent> {
    if bytes.len() < 3 {
        return None;
    }
    let status = bytes[0] & 0xF0;
    let _channel = bytes[0] & 0x0F;
    let note = bytes[1];
    let vel = bytes[2];

    match status {
        0x90 => {
            // 8×8 grid pad
            if let Some(id) = lp_x_note_to_pad_id(note) {
                return if vel > 0 {
                    Some(SurfaceEvent::PadPressed {
                        id,
                        velocity: (vel as u16) << 9,
                        pressure: 0,
                    })
                } else {
                    Some(SurfaceEvent::PadReleased { id })
                };
            }
            // Right-column scene buttons
            if let Some(id) = lp_x_scene_note_to_id(note) {
                return if vel > 0 {
                    Some(SurfaceEvent::ButtonPressed { id })
                } else {
                    Some(SurfaceEvent::ButtonReleased { id })
                };
            }
            // Top row buttons: Note On notes 91–98 in Programmer Mode
            if let Some(id) = lp_x_top_cc_to_id(note) {
                return if vel > 0 {
                    Some(SurfaceEvent::ButtonPressed { id })
                } else {
                    Some(SurfaceEvent::ButtonReleased { id })
                };
            }
            None
        }
        0x80 => {
            if let Some(id) = lp_x_note_to_pad_id(note) {
                return Some(SurfaceEvent::PadReleased { id });
            }
            if let Some(id) = lp_x_scene_note_to_id(note) {
                return Some(SurfaceEvent::ButtonReleased { id });
            }
            if let Some(id) = lp_x_top_cc_to_id(note) {
                return Some(SurfaceEvent::ButtonReleased { id });
            }
            None
        }
        0xB0 => {
            // Scene buttons (right column) send CC in Programmer Mode: CC 19,29,...,89
            if let Some(id) = lp_x_scene_note_to_id(note) {
                return if vel > 0 {
                    Some(SurfaceEvent::ButtonPressed { id })
                } else {
                    Some(SurfaceEvent::ButtonReleased { id })
                };
            }
            // Top round buttons — some firmware sends CC 91–98
            if let Some(id) = lp_x_top_cc_to_id(note) {
                return if vel > 0 {
                    Some(SurfaceEvent::ButtonPressed { id })
                } else {
                    Some(SurfaceEvent::ButtonReleased { id })
                };
            }
            None
        }
        _ => None,
    }
}

// ── Output handle ─────────────────────────────────────────────────────────────

pub struct LaunchpadOutputHandle {
    conn_out: midir::MidiOutputConnection,
    rx: rtrb::Consumer<SurfaceOutput>,
    initialised: bool,
}

/// Velocity index for the Launchpad X's 128-colour static palette.
fn rgb_to_lp_x_velocity(c: RgbColor) -> u8 {
    // Threshold: treat anything below 32 on all channels as off.
    let max = c.r.max(c.g).max(c.b);
    if max < 32 {
        return 0;
    }

    // Map to nearest LPX palette entry by dominant channel.
    if c == RgbColor::GREEN {
        return 87;
    }
    if c == RgbColor::RED {
        return 5;
    }
    if c == RgbColor::BLUE {
        return 67;
    }
    if c.g >= c.r && c.g >= c.b {
        87
    } else if c.r >= c.b {
        5
    } else {
        67
    }
}

/// pad ID 0–63 → Launchpad X programmer-mode note (row 0 = top → phys row 8).
fn pad_id_to_lp_x_note(id: u32) -> u8 {
    let our_row = id / 8; // 0 = top
    let col = id % 8; // 0 = left
    let phys_row = 8 - our_row; // invert back
    (phys_row * 10 + col + 1) as u8
}

impl LaunchpadOutputHandle {
    fn send_led_updates(&mut self, output: &SurfaceOutput) {
        for update in &output.led_updates {
            let note = if update.control_id < 64 {
                pad_id_to_lp_x_note(update.control_id)
            } else if update.control_id < 72 {
                let row = update.control_id - 64;
                ((8 - row) * 10 + 9) as u8
            } else if update.control_id < 80 {
                91 + (update.control_id - 72) as u8
            } else {
                continue;
            };
            let vel = rgb_to_lp_x_velocity(update.color);
            eprintln!(
                "[lpx] note={note} vel={vel} (ctrl_id={})",
                update.control_id
            );
            let result = self.conn_out.send(&[0x90, note, vel]);
            if result.is_err() {
                eprintln!("[lpx] send failed!");
            }
        }
    }
}

impl Drop for LaunchpadOutputHandle {
    fn drop(&mut self) {
        // Revert LP to Standalone + Note mode on clean exit.
        // Prevents the device from staying stuck in a host-controlled mode.
        let _ = self
            .conn_out
            .send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x10, 0x00, 0xF7]);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let _ = self
            .conn_out
            .send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x00, 0x01, 0xF7]);
    }
}

impl SurfaceOutputHandle for LaunchpadOutputHandle {
    fn tick(&mut self) {
        while let Ok(output) = self.rx.pop() {
            self.send_led_updates(&output);
        }
        self.initialised = true;
    }

    fn deliver(&mut self, output: SurfaceOutput) {
        eprintln!("[lpx-deliver] {} LED updates", output.led_updates.len());
        self.send_led_updates(&output);
    }
}

// ── LaunchpadNode ─────────────────────────────────────────────────────────────

pub struct LaunchpadNode {
    ports: [PortDescriptor; 1],
    node_id: u32,
    _conn_in: midir::MidiInputConnection<()>,
    conn_out: Option<midir::MidiOutputConnection>,
    incoming: Arc<Mutex<VecDeque<SurfaceEvent>>>,
    hw_out_tx: rtrb::Producer<SurfaceOutput>,
    hw_out_rx: Option<rtrb::Consumer<SurfaceOutput>>,
    surface: SurfaceDescriptor,
}

impl LaunchpadNode {
    /// Open the Launchpad X on its standard MIDI port ("LPX MIDI").
    /// Falls back to any port containing "Launchpad" if the X is not found.
    pub fn open() -> Result<Self, MidiDeviceError> {
        let incoming = Arc::new(Mutex::new(VecDeque::<SurfaceEvent>::new()));
        let incoming_cb = Arc::clone(&incoming);

        // Step 1: Reset any stuck DAW/Session state via the DAW port.
        // The spec requires: "When the DAW/software exits, it should send SysEx to revert
        // the device to Standalone mode." A previous kill -9 may have left the device in
        // DAW mode, which overrides Programmer Mode. We reset it here on every open().
        if let Ok(reg) = MidiDeviceRegistry::new() {
            let daw_port = if reg
                .output_port_names()
                .iter()
                .any(|n| n.contains("LPX DAW"))
            {
                "LPX DAW"
            } else {
                ""
            };
            if !daw_port.is_empty() {
                if let Ok(mut c) = MidiDeviceRegistry::new().and_then(|r| r.open_output(daw_port)) {
                    let _ = c.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x10, 0x00, 0xF7]);
                    eprintln!("[launchpad] reset: DAW mode → Standalone");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }

        // Step 2: Use MIDI port for all subsequent control.
        let port_out = if Self::port_exists_out("LPX MIDI") {
            "LPX MIDI"
        } else {
            "Launchpad"
        };
        let port_in = if Self::port_exists_in("LPX MIDI") {
            "LPX MIDI"
        } else {
            "Launchpad"
        };

        let reg_out = MidiDeviceRegistry::new()?;
        let mut conn_out = reg_out.open_output(port_out)?;

        // Step 3: Enter Programmer Mode.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = conn_out.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x00, 0x7F, 0xF7]);
        eprintln!("[launchpad] Programmer Mode SysEx sent");
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Step 4: Open input.
        let reg_in = MidiDeviceRegistry::new()?;
        let conn_in = reg_in.open_input(port_in, move |_ts, bytes, _| {
            if let Some(ev) = parse_launchpad_midi(bytes) {
                if let Ok(mut q) = incoming_cb.try_lock() {
                    q.push_back(ev);
                }
            }
        })?;

        let (tx, rx) = rtrb::RingBuffer::<SurfaceOutput>::new(64);

        Ok(Self {
            ports: [PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            node_id: 0,
            _conn_in: conn_in,
            conn_out: Some(conn_out),
            incoming,
            hw_out_tx: tx,
            hw_out_rx: Some(rx),
            surface: build_surface(),
        })
    }

    fn port_exists_in(substr: &str) -> bool {
        MidiDeviceRegistry::new()
            .map(|r| r.input_port_names().iter().any(|n| n.contains(substr)))
            .unwrap_or(false)
    }

    fn port_exists_out(substr: &str) -> bool {
        MidiDeviceRegistry::new()
            .map(|r| r.output_port_names().iter().any(|n| n.contains(substr)))
            .unwrap_or(false)
    }
}

// ── Node + Surface impls ───────────────────────────────────────────────

impl Node for LaunchpadNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }
    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "LaunchpadNode".into(),
            vendor: "Paraclete/Novation".into(),
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

impl Surface for LaunchpadNode {
    fn descriptor(&self) -> &SurfaceDescriptor {
        &self.surface
    }

    fn update_output(&mut self, output: &SurfaceOutput) {
        let _ = self.hw_out_tx.push(SurfaceOutput {
            led_updates: output
                .led_updates
                .iter()
                .map(|u| LedUpdate {
                    control_id: u.control_id,
                    color: u.color,
                })
                .collect(),
            display_updates: vec![],
        });
    }

    fn take_output_handle(&mut self) -> Option<Box<dyn SurfaceOutputHandle>> {
        let conn_out = self.conn_out.take()?;
        let rx = self.hw_out_rx.take()?;
        Some(Box::new(LaunchpadOutputHandle {
            conn_out,
            rx,
            initialised: false,
        }))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lp_x_top_left_pad_maps_to_id_0() {
        // Physical top row, leftmost pad: note 81
        assert_eq!(lp_x_note_to_pad_id(81), Some(0));
    }

    #[test]
    fn lp_x_top_right_pad_maps_to_id_7() {
        // Physical top row, rightmost pad: note 88
        assert_eq!(lp_x_note_to_pad_id(88), Some(7));
    }

    #[test]
    fn lp_x_bottom_left_pad_maps_to_id_56() {
        // Physical bottom row, leftmost pad: note 11
        assert_eq!(lp_x_note_to_pad_id(11), Some(56));
    }

    #[test]
    fn lp_x_note_roundtrips_through_pad_id() {
        // Pad IDs 0–63 should round-trip through lp_x_note_to_pad_id and pad_id_to_lp_x_note.
        for row in 0u8..8 {
            for col in 0u8..8 {
                let id = (row as u32) * 8 + col as u32;
                let note = pad_id_to_lp_x_note(id);
                assert_eq!(
                    lp_x_note_to_pad_id(note),
                    Some(id),
                    "roundtrip failed for row={row} col={col} id={id} note={note}"
                );
            }
        }
    }

    #[test]
    fn lp_x_top_buttons_map_to_ids_72_79() {
        assert_eq!(lp_x_top_cc_to_id(91), Some(72));
        assert_eq!(lp_x_top_cc_to_id(98), Some(79));
        assert_eq!(lp_x_top_cc_to_id(90), None);
    }

    #[test]
    fn parse_note_on_top_left_pad_gives_pressed_id_0() {
        // note 81, vel > 0 → PadPressed id=0
        let ev = parse_launchpad_midi(&[0x90, 81, 100]);
        assert!(matches!(ev, Some(SurfaceEvent::PadPressed { id: 0, .. })));
    }

    #[test]
    fn parse_note_on_vel_zero_gives_released() {
        let ev = parse_launchpad_midi(&[0x90, 81, 0]);
        assert!(matches!(ev, Some(SurfaceEvent::PadReleased { id: 0 })));
    }

    #[test]
    fn parse_top_button_cc_gives_button_pressed() {
        let ev = parse_launchpad_midi(&[0xB0, 91, 127]);
        assert!(matches!(ev, Some(SurfaceEvent::ButtonPressed { id: 72 })));
    }

    #[test]
    fn parse_scene_button_cc_gives_button_pressed() {
        // CC 89 = scene button row 0 (physical top) = id 64 = SEQ EDIT
        let ev = parse_launchpad_midi(&[0xB0, 89, 127]);
        assert!(matches!(ev, Some(SurfaceEvent::ButtonPressed { id: 64 })));
    }

    #[test]
    fn parse_scene_button_cc_release_gives_button_released() {
        let ev = parse_launchpad_midi(&[0xB0, 89, 0]);
        assert!(matches!(ev, Some(SurfaceEvent::ButtonReleased { id: 64 })));
    }
}
