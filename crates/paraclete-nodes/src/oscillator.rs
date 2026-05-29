use std::f64::consts::TAU;

use paraclete_node_api::{
    Event, Node, PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
    UmpMessage,
    midi::ChannelVoice2,
};

/// A monophonic sine oscillator driven by MIDI 2.0 NoteOn/NoteOff events.
///
/// The first real audio-producing node. Demonstrates the full event-to-audio
/// path: NoteOn → frequency + gate, NoteOff → gate closed, audio rendered
/// from a running phase accumulator.
pub struct SineOscillator {
    ports: [PortDescriptor; 2],
    phase: f64,
    frequency: f64,
    gate: bool,
    amplitude: f32,
    sample_rate: f32,
}

impl SineOscillator {
    pub const PORT_EVENTS_IN: u32 = 0;
    pub const PORT_AUDIO_OUT: u32 = 1;

    pub fn new() -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_EVENTS_IN,
                    name: "events_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Event,
                },
                PortDescriptor {
                    id: Self::PORT_AUDIO_OUT,
                    name: "audio_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Audio,
                },
            ],
            phase: 0.0,
            frequency: 440.0,
            gate: false,
            amplitude: 0.0,
            sample_rate: 44100.0,
        }
    }

    fn handle_ump(&mut self, ump: UmpMessage) {
        match ump {
            UmpMessage::ChannelVoice2(cv2) => match cv2 {
                ChannelVoice2::NoteOn(note_on) => {
                    let note = u8::from(note_on.note_number());
                    self.frequency = midi_note_to_hz(note);
                    self.amplitude = note_on.velocity() as f32 / 65535.0;
                    self.gate = true;
                }
                ChannelVoice2::NoteOff(_) => {
                    self.gate = false;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

impl Default for SineOscillator {
    fn default() -> Self { Self::new() }
}

impl Node for SineOscillator {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate;
        self.phase = 0.0;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        for timed in input.events {
            if let Event::Midi2(ump) = timed.event {
                self.handle_ump(ump);
            }
        }

        if output.audio_outputs.is_empty() {
            return;
        }

        let audio_out = &mut output.audio_outputs[0];
        let phase_inc = self.frequency / self.sample_rate as f64;

        for frame in 0..input.block_size {
            let sample = if self.gate {
                (self.phase * TAU).sin() as f32 * self.amplitude
            } else {
                0.0
            };

            for ch in 0..audio_out.channels() {
                audio_out.channel_mut(ch)[frame] = sample;
            }

            self.phase = (self.phase + phase_inc) % 1.0;
        }
    }
}

/// Convert a MIDI note number to frequency in Hz.
/// Note 69 = A4 = 440 Hz. Equal temperament.
pub fn midi_note_to_hz(note: u8) -> f64 {
    440.0 * 2.0_f64.powf((note as f64 - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TimedEvent, TransportInfo,
        UmpMessage,
        midi::{ChannelVoice2, Channeled, Grouped, NoteOff, NoteOn, u4, u7},
    };

    fn make_note_on(note: u8, velocity: u16) -> UmpMessage {
        let mut msg = NoteOn::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note & 0x7F));
        msg.set_velocity(velocity);
        UmpMessage::from(ChannelVoice2::from(msg))
    }

    fn make_note_off(note: u8) -> UmpMessage {
        let mut msg = NoteOff::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note & 0x7F));
        msg.set_velocity(0);
        UmpMessage::from(ChannelVoice2::from(msg))
    }

    fn run_osc(osc: &mut SineOscillator, events: &[TimedEvent]) -> Vec<f32> {
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let audio_ptr: *mut AudioBuffer = &mut audio as *mut AudioBuffer;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];

        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &[],
            events,
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        osc.process(&input, &mut output);
        audio.channel(0).to_vec()
    }

    #[test]
    fn sine_oscillator_produces_silence_when_gate_is_closed() {
        let mut osc = SineOscillator::new();
        osc.activate(44100.0, 64);
        let samples = run_osc(&mut osc, &[]);
        assert!(samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn sine_oscillator_produces_non_zero_samples_after_note_on() {
        let mut osc = SineOscillator::new();
        osc.activate(44100.0, 64);
        let note_on = TimedEvent::new(0, Event::Midi2(make_note_on(69, 32768)));
        let samples = run_osc(&mut osc, &[note_on]);
        assert!(samples.iter().any(|&s| s.abs() > 1e-6));
    }

    #[test]
    fn sine_oscillator_gate_closes_on_note_off() {
        let mut osc = SineOscillator::new();
        osc.activate(44100.0, 64);
        run_osc(&mut osc, &[TimedEvent::new(0, Event::Midi2(make_note_on(69, 32768)))]);
        let samples = run_osc(&mut osc, &[TimedEvent::new(0, Event::Midi2(make_note_off(69)))]);
        assert!(samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn sine_oscillator_frequency_changes_on_note_on() {
        let mut osc_a4 = SineOscillator::new();
        osc_a4.activate(44100.0, 64);
        let samples_a4 = run_osc(&mut osc_a4, &[TimedEvent::new(0, Event::Midi2(make_note_on(69, 32768)))]);

        let mut osc_c4 = SineOscillator::new();
        osc_c4.activate(44100.0, 64);
        let samples_c4 = run_osc(&mut osc_c4, &[TimedEvent::new(0, Event::Midi2(make_note_on(60, 32768)))]);

        let differ = samples_a4.iter().zip(&samples_c4).any(|(a, b)| (a - b).abs() > 1e-6);
        assert!(differ);
    }

    #[test]
    fn midi_note_to_hz_a4_is_440() {
        assert!((midi_note_to_hz(69) - 440.0).abs() < 0.001);
    }

    #[test]
    fn midi_note_to_hz_c4_is_approximately_261_63() {
        let hz = midi_note_to_hz(60);
        assert!((hz - 261.626).abs() < 0.01, "got {hz}");
    }
}
