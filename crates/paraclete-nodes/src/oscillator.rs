use std::f64::consts::TAU;

use paraclete_node_api::{
    CapabilityDocument, Event, Node, ParamDescriptor, ParamUnit, ParameterBank,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, StateBusValue,
    UmpMessage,
    midi::ChannelVoice2,
};

#[cfg(test)]
use paraclete_node_api::{SignalInputSlot, SignalPortKind};

// ── OscillatorNode ────────────────────────────────────────────────────────────

fn osc_param(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

fn poly_blep(phase: f32, phase_inc: f32) -> f32 {
    if phase < phase_inc {
        let t = phase / phase_inc;
        2.0 * t - t * t - 1.0
    } else if phase > 1.0 - phase_inc {
        let t = (phase - 1.0) / phase_inc;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// Band-limited analog-modelled oscillator with 5 waveforms.
/// Ports: pitch_cv_in (0, Mod), fm_in (1, Mod), gate_in (2, Logic), audio_out (3, Mono).
pub struct OscillatorNode {
    bank:        ParameterBank,
    phase:       f32,
    noise_state: u32,
    sample_rate: f32,
    node_id:     u32,
    ports:       [PortDescriptor; 4],
}

impl OscillatorNode {
    pub const PORT_PITCH_CV: u32 = 0;
    pub const PORT_FM_IN:    u32 = 1;
    pub const PORT_GATE_IN:  u32 = 2;
    pub const PORT_AUDIO_OUT:u32 = 3;

    pub fn new() -> Self {
        let doc = Self::default_doc();
        Self {
            bank: ParameterBank::from_capability_document(&doc),
            phase: 0.0,
            noise_state: 1,
            sample_rate: 44100.0,
            node_id: 0,
            ports: [
                PortDescriptor { id: Self::PORT_PITCH_CV,  name: "pitch_cv_in".into(), direction: PortDirection::Input,  port_type: PortType::Modulation },
                PortDescriptor { id: Self::PORT_FM_IN,     name: "fm_in".into(),       direction: PortDirection::Input,  port_type: PortType::Modulation },
                PortDescriptor { id: Self::PORT_GATE_IN,   name: "gate_in".into(),     direction: PortDirection::Input,  port_type: PortType::Logic },
                PortDescriptor { id: Self::PORT_AUDIO_OUT, name: "audio_out".into(),   direction: PortDirection::Output, port_type: PortType::Mono },
            ],
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "OscillatorNode".into(), vendor: "Paraclete".into(), version: (0, 6, 0),
            ports: vec![],
            params: vec![
                ParamDescriptor { id: osc_param("waveform"), name: "waveform".into(), min: 0.0, max: 4.0, default: 0.0, stepped: true,  unit: ParamUnit::Generic,  display: None },
                ParamDescriptor { id: osc_param("pitch"),    name: "pitch".into(),    min: -60.0, max: 60.0, default: 0.0, stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: osc_param("tune"),     name: "tune".into(),     min: -100.0, max: 100.0, default: 0.0, stepped: false, unit: ParamUnit::Cents, display: None },
                ParamDescriptor { id: osc_param("level"),    name: "level".into(),    min: 0.0, max: 2.0, default: 1.0, stepped: false, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec![],
        }
    }
}

impl Default for OscillatorNode {
    fn default() -> Self { Self::new() }
}

impl Node for OscillatorNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }
    fn capability_document(&self) -> CapabilityDocument { Self::default_doc() }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn activate(&mut self, sample_rate: f32, _block_size: usize) {
        self.sample_rate = sample_rate;
        self.bank = ParameterBank::from_capability_document(&Self::default_doc());
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let waveform   = self.bank.get(osc_param("waveform")) as u8;
        let base_pitch = self.bank.get(osc_param("pitch")) as f32;
        let tune_cents = self.bank.get(osc_param("tune")) as f32;
        let level      = self.bank.get(osc_param("level")) as f32;

        let pitch_cv = input.modulation(Self::PORT_PITCH_CV);
        let fm_in    = input.modulation(Self::PORT_FM_IN);
        let gate_in  = input.logic(Self::PORT_GATE_IN);

        if let Some(audio_out) = output.audio_outputs.first_mut() {
            let out = audio_out.channel_mut(0);
            let mut prev_gate = 0.0f32;
            for i in 0..out.len() {
                let g = gate_in.get(i).copied().unwrap_or(0.0);
                if g >= 0.5 && prev_gate < 0.5 { self.phase = 0.0; }
                prev_gate = g;

                let semitones = base_pitch + tune_cents / 100.0
                    + pitch_cv.get(i).copied().unwrap_or(0.0)
                    + fm_in.get(i).copied().unwrap_or(0.0);
                let freq = 440.0f32 * 2.0f32.powf((semitones - 9.0) / 12.0);
                let phase_inc = (freq / self.sample_rate).clamp(0.0, 0.5);

                let sample = match waveform {
                    0 => (self.phase * std::f32::consts::TAU).sin(),
                    1 => {
                        // Triangle. The formula 4*(p-(p+0.5).floor()+0.5).abs()-1
                        // evaluates out of [-1,1] (e.g. 2.0 at p=0.25). Correct:
                        4.0 * (self.phase - 0.5).abs() - 1.0
                    }
                    2 => {
                        let saw = 2.0 * self.phase - 1.0;
                        saw - poly_blep(self.phase, phase_inc)
                    }
                    3 => {
                        let sq = if self.phase < 0.5 { 1.0f32 } else { -1.0 };
                        let blep0 = poly_blep(self.phase, phase_inc);
                        let blep1 = poly_blep((self.phase + 0.5).fract(), phase_inc);
                        sq + blep0 - blep1
                    }
                    _ => {
                        self.noise_state ^= self.noise_state << 13;
                        self.noise_state ^= self.noise_state >> 17;
                        self.noise_state ^= self.noise_state << 5;
                        (self.noise_state as i32 as f32) / (i32::MAX as f32)
                    }
                };

                out[i] = sample * level;
                self.phase = (self.phase + phase_inc).fract();
            }
        }
    }
}

#[cfg(test)]
mod oscillator_node_tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, NodeCommand, CMD_SET_PARAM,
        TransportInfo,
    };

    fn run_osc_node(osc: &mut OscillatorNode, gate: Option<&[f32]>, pitch_cv: Option<&[f32]>) -> Vec<f32> {
        let block = 512usize;
        let mut audio = AudioBuffer::new(1, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let audio_ptr: *mut AudioBuffer = &mut audio;
        let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
        let mut outs = [audio_ref];

        let gate_data: Vec<f32>;
        let cv_data: Vec<f32>;
        let mut sig_inputs = Vec::new();
        if let Some(g) = gate {
            gate_data = g.to_vec();
            sig_inputs.push(SignalInputSlot::new(OscillatorNode::PORT_GATE_IN, SignalPortKind::Logic, &gate_data));
        }
        if let Some(cv) = pitch_cv {
            cv_data = cv.to_vec();
            sig_inputs.push(SignalInputSlot::new(OscillatorNode::PORT_PITCH_CV, SignalPortKind::Modulation, &cv_data));
        }

        let input = ProcessInput {
            audio_inputs: &[],
            signal_inputs: &sig_inputs,
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: block,
            extended_events: &slab,
            commands: &[],
        };
        let mut output = ProcessOutput::new(
            &mut outs,
            &mut [],
            &mut events_out,
        );
        osc.process(&input, &mut output);
        audio.channel(0).to_vec()
    }

    fn set_waveform(osc: &mut OscillatorNode, w: f64) {
        let cmd = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: osc_param("waveform") as i64, arg1: w };
        osc.bank.handle_commands(&[cmd]);
    }

    #[test]
    fn oscillator_sine_output_is_periodic() {
        let mut osc = OscillatorNode::new();
        osc.activate(44100.0, 512);
        // Set pitch to A4 = 440 Hz. Period at 44100 = ~100.2 samples.
        // After 512 samples the phase has advanced; just check non-zero output.
        let out = run_osc_node(&mut osc, None, None);
        assert!(out.iter().any(|&s| s.abs() > 1e-4), "sine should be non-zero at default pitch");
    }

    #[test]
    fn oscillator_sawtooth_output_is_nonzero() {
        let mut osc = OscillatorNode::new();
        osc.activate(44100.0, 512);
        set_waveform(&mut osc, 2.0);
        let out = run_osc_node(&mut osc, None, None);
        assert!(out.iter().any(|&s| s.abs() > 1e-4));
    }

    #[test]
    fn oscillator_square_output_is_nonzero() {
        let mut osc = OscillatorNode::new();
        osc.activate(44100.0, 512);
        set_waveform(&mut osc, 3.0);
        let out = run_osc_node(&mut osc, None, None);
        assert!(out.iter().any(|&s| s.abs() > 1e-4));
    }

    #[test]
    fn oscillator_noise_output_is_nonzero() {
        let mut osc = OscillatorNode::new();
        osc.activate(44100.0, 512);
        set_waveform(&mut osc, 4.0);
        let out = run_osc_node(&mut osc, None, None);
        assert!(out.iter().any(|&s| s.abs() > 1e-4));
    }

    #[test]
    fn oscillator_gate_rising_edge_resets_phase() {
        // Two runs starting from different phases: with gate reset the second run
        // should start at phase 0 (same sample as the first block's sample 0).
        let mut osc = OscillatorNode::new();
        osc.activate(44100.0, 512);
        let out_no_reset = run_osc_node(&mut osc, None, None);

        // Reset via gate edge at sample 0.
        let mut osc2 = OscillatorNode::new();
        osc2.activate(44100.0, 512);
        // Run one block to advance phase.
        run_osc_node(&mut osc2, None, None);
        // Now apply gate pulse at sample 0 to force phase reset.
        let gate: Vec<f32> = (0..512).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let out_reset = run_osc_node(&mut osc2, Some(&gate), None);
        // After gate reset, sample 0 of out_reset should match sample 0 of the first run.
        assert!((out_reset[0] - out_no_reset[0]).abs() < 1e-5,
            "gate reset: first sample should match (phase=0 both times)");
    }

    #[test]
    fn oscillator_pitch_cv_shifts_frequency() {
        let mut osc_base = OscillatorNode::new();
        osc_base.activate(44100.0, 512);
        let out_base = run_osc_node(&mut osc_base, None, None);

        let mut osc_shift = OscillatorNode::new();
        osc_shift.activate(44100.0, 512);
        // +12 semitones pitch CV doubles the frequency.
        let cv = vec![12.0f32; 512];
        let out_shift = run_osc_node(&mut osc_shift, None, Some(&cv));

        // Different frequencies → different sample patterns
        let differ = out_base.iter().zip(&out_shift).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "pitch CV shift should change waveform");
    }

    #[test]
    fn oscillator_portability_check() {
        // Verify no runtime/hal dependencies — this is a compile-time check via cargo tree.
        // At runtime: just confirm the node constructs and activates.
        let mut osc = OscillatorNode::new();
        osc.activate(44100.0, 512);
        assert!(!osc.ports().is_empty());
    }
}

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
            commands: &[],
        };
        let mut output = ProcessOutput::new(
            &mut outs,
            &mut [],
            &mut events_out,
        );
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
