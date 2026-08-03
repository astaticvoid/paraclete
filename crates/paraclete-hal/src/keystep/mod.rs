// SPDX-License-Identifier: GPL-3.0-or-later
//! Arturia Keystep 37 — Surface node via MIDI.
//!
//! Standard MIDI over USB. Note On/Off → `Event::Midi2`. Mod wheel → FaderMoved.
//! No LED output — `take_output_handle()` returns `None`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use paraclete_node_api::midi::{u4, u7, ChannelVoice2, Channeled, Grouped, NoteOff, NoteOn};
use paraclete_node_api::{
    CapabilityDocument, Control, Event, FaderDescriptor, Node, PortDescriptor, PortDirection,
    PortName, PortType, ProcessInput, ProcessOutput, Surface, SurfaceDescriptor, SurfaceEvent,
    SurfaceOutput, TimedEvent, UmpMessage,
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
        name: "Keystep 37".into(),
        vendor: "Arturia".into(),
        controls: vec![
            // Pitch bend as fader id=0
            Control::Fader(FaderDescriptor {
                id: 0,
                name: PortName::Static("pitch_bend"),
                motorised: false,
            }),
            // Mod wheel as fader id=1
            Control::Fader(FaderDescriptor {
                id: 1,
                name: PortName::Static("mod_wheel"),
                motorised: false,
            }),
        ],
    }
}

/// Arturia Keystep 37 as a `Surface: Node`.
pub struct KeystepNode {
    ports: [PortDescriptor; 1],
    node_id: u32,
    _conn_in: midir::MidiInputConnection<()>,
    /// (midir µs timestamp, event). The timestamp lets `process()` map an
    /// event's arrival time to a best-effort intra-block sample offset
    /// (P11 C5b, ADR-039 Amd 2 — "sample-accurate" is otherwise
    /// aspirational). midir's `ts` epoch varies by backend (ALSA: µs since
    /// the Unix epoch; others: since the connection); only monotonic
    /// deltas between events are meaningful, which is all we use.
    incoming: Arc<Mutex<VecDeque<(u64, TimedEvent)>>>,
    surface: SurfaceDescriptor,
    /// µs timestamp (midir clock) of the newest event drained by the
    /// previous `process()` call — the reference for this block's start.
    /// 0 = never drained; the first drain has no reference and emits
    /// zero offsets.
    last_drain_ts: u64,
}

impl KeystepNode {
    pub fn open() -> Result<Self, MidiDeviceError> {
        let incoming = Arc::new(Mutex::new(VecDeque::<(u64, TimedEvent)>::new()));
        let incoming_cb = Arc::clone(&incoming);

        let reg = MidiDeviceRegistry::new()?;
        let conn_in = reg.open_input("Keystep", move |ts, bytes, _| {
            for ev in parse_keystep_midi(bytes) {
                if let Ok(mut q) = incoming_cb.try_lock() {
                    q.push_back((ts, ev));
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
            last_drain_ts: 0,
            surface: build_surface(),
        })
    }
}

/// P11 C5b (ADR-039 Amd 2): map a midir µs arrival timestamp to a
/// best-effort intra-block sample offset against `block_start_ts` (the
/// newest timestamp seen at the previous drain = this block's start).
/// `0` start (never drained), stale events, and the first drain all map
/// to offset 0; the result is clamped to `block_size − 1` (one block of
/// jitter at most).
fn arrival_offset(ts: u64, block_start_ts: u64, sample_rate: f32, block_size: u32) -> u32 {
    if block_start_ts == 0 || ts <= block_start_ts || block_size == 0 {
        return 0;
    }
    let samples = ((ts - block_start_ts) as f64 * sample_rate as f64 / 1_000_000.0).floor() as u32;
    samples.min(block_size - 1)
}

fn parse_keystep_midi(bytes: &[u8]) -> Vec<TimedEvent> {
    if bytes.len() < 2 {
        return vec![];
    }
    let status = bytes[0] & 0xF0;
    let note = bytes[1];
    let vel = bytes.get(2).copied().unwrap_or(0);

    match status {
        0x90 if vel > 0 => vec![TimedEvent::new(0, Event::Midi2(build_note_on(note, vel)))],
        0x90 | 0x80 => vec![TimedEvent::new(0, Event::Midi2(build_note_off(note)))],
        0xB0 if note == 1 => {
            // Mod wheel CC1
            vec![TimedEvent::new(
                0,
                Event::Surface(SurfaceEvent::FaderMoved {
                    id: 1,
                    value: (vel as u16) << 9,
                }),
            )]
        }
        0xE0 => {
            // Pitch bend: 14-bit from two bytes
            let pb = if bytes.len() >= 3 {
                ((bytes[2] as u16) << 7) | (bytes[1] as u16)
            } else {
                0
            };
            vec![TimedEvent::new(
                0,
                Event::Surface(SurfaceEvent::FaderMoved { id: 0, value: pb }),
            )]
        }
        _ => vec![],
    }
}

impl Node for KeystepNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }
    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "KeystepNode".into(),
            vendor: "Paraclete/Arturia".into(),
            version: (0, 4, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec!["paraclete.hardware".into()],
            view: None,
        }
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // P11 C5b (ADR-039 Amd 2): map each queued event's midir µs
        // timestamp to a best-effort intra-block sample offset against the
        // previous drain's newest timestamp (= this block's start). Events
        // that arrived before that reference (or on the never-drained
        // first call) get offset 0. The result is ≤ 1 block of jitter —
        // ~11.6 ms at 44.1 kHz — which the sequencer's own tick-based
        // micro-timing already refines further.
        if let Ok(mut q) = self.incoming.try_lock() {
            let block_start_ts = self.last_drain_ts;
            let mut max_ts = block_start_ts;
            let sr = input.sample_rate.max(1.0);
            let block = input.block_size.max(1) as u32;
            while let Some((ts, mut ev)) = q.pop_front() {
                max_ts = max_ts.max(ts);
                ev.sample_offset = arrival_offset(ts, block_start_ts, sr, block);
                output.events_out.push(ev);
            }
            self.last_drain_ts = max_ts;
        }
    }
}

impl Surface for KeystepNode {
    fn descriptor(&self) -> &SurfaceDescriptor {
        &self.surface
    }
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
        assert!(matches!(
            events[0].event,
            Event::Surface(SurfaceEvent::FaderMoved { id: 1, .. })
        ));
    }

    // P11 C5b — arrival-offset mapping (ADR-039 Amd 2).

    #[test]
    fn arrival_offset_maps_us_delta_to_samples() {
        // At 44100 Hz, 1 ms = 44.1 samples.
        assert_eq!(arrival_offset(1_001_000, 1_000_000, 44100.0, 512), 44);
        // Clamped to block_size − 1 (a full block of jitter at most).
        assert_eq!(arrival_offset(101_000_000, 1_000_000, 44100.0, 512), 511);
    }

    #[test]
    fn arrival_offset_first_drain_and_stale_events_are_zero() {
        // First drain (block_start_ts 0) has no reference — zero.
        assert_eq!(arrival_offset(5_000, 0, 44100.0, 512), 0);
        // An event older than the block start (arrived before the
        // previous drain) is already "in the past" — zero.
        assert_eq!(arrival_offset(1_000, 2_000, 44100.0, 512), 0);
        // Simultaneous arrival — zero.
        assert_eq!(arrival_offset(2_000, 2_000, 44100.0, 512), 0);
    }

    #[test]
    fn arrival_offset_progresses_within_a_block() {
        // Two events in the same block, 5 ms apart at 44.1 kHz = 220 samples.
        let a = arrival_offset(1_000_000, 995_000, 44100.0, 512);
        let b = arrival_offset(1_005_000, 995_000, 44100.0, 512);
        assert_eq!(a, 220);
        assert_eq!(b, 441);
        assert!(b > a, "later arrival → later offset within the block");
    }
}
