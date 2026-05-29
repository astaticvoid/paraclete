use std::collections::HashMap;

use paraclete_node_api::{
    Event, HardwareEvent, Node, PortDescriptor, PortDirection, PortType, ProcessInput,
    ProcessOutput, TimedEvent,
};

use crate::{build_note_on, build_note_off};

/// Translates `HardwareEvent`s into `Midi2` events.
///
/// Sits between a hardware controller node (e.g. `LaunchpadEmulator`) and a
/// sound node (e.g. `SineOscillator`). At P1 the mapping is hardcoded.
/// At P4 it becomes scriptable via Rhai.
pub struct HardwareMappingNode {
    ports: [PortDescriptor; 2],
    pad_to_note: HashMap<u32, u8>,
    channel: u8,
    group: u8,
}

impl HardwareMappingNode {
    /// Default chromatic mapping: pad 0 → C4 (60), pad 1 → C#4 (61), …
    pub fn default_chromatic(channel: u8) -> Self {
        let pad_to_note = (0u32..64).map(|id| (id, 60u8.saturating_add(id as u8))).collect();
        Self::from_map(pad_to_note, channel)
    }

    pub fn from_map(map: HashMap<u32, u8>, channel: u8) -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: 0,
                    name: "events_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Event,
                },
                PortDescriptor {
                    id: 1,
                    name: "events_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Event,
                },
            ],
            pad_to_note: map,
            channel,
            group: 0,
        }
    }
}

impl Node for HardwareMappingNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        for timed in input.events {
            match timed.event {
                Event::Hardware(HardwareEvent::PadPressed { id, velocity, .. }) => {
                    if let Some(&note) = self.pad_to_note.get(&id) {
                        output.events_out.push(TimedEvent::new(
                            timed.sample_offset,
                            Event::Midi2(build_note_on(self.group, self.channel, note, velocity)),
                        ));
                    }
                }
                Event::Hardware(HardwareEvent::PadReleased { id }) => {
                    if let Some(&note) = self.pad_to_note.get(&id) {
                        output.events_out.push(TimedEvent::new(
                            timed.sample_offset,
                            Event::Midi2(build_note_off(self.group, self.channel, note)),
                        ));
                    }
                }
                // All other event types pass through unchanged.
                other => output.events_out.push(TimedEvent::new(timed.sample_offset, other)),
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, EventOutputBuffer, ExtendedEventSlab, Event, HardwareEvent,
        TimedEvent, TransportInfo,
        midi::ChannelVoice2,
    };

    fn run_mapper(mapper: &mut HardwareMappingNode, events: &[TimedEvent]) -> Vec<TimedEvent> {
        let block = 64usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(64);
        let transport = TransportInfo::default();
        let extended = ExtendedEventSlab::empty();

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
            extended_events: &extended,
        };
        let mut output = ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        };
        mapper.process(&input, &mut output);
        events_out.as_slice().to_vec()
    }

    #[test]
    fn mapping_node_translates_pad_pressed_to_midi2_note_on() {
        let mut mapper = HardwareMappingNode::default_chromatic(0);
        let events = [TimedEvent::new(
            0,
            Event::Hardware(HardwareEvent::PadPressed { id: 0, velocity: 16000, pressure: 0 }),
        )];
        let out = run_mapper(&mut mapper, &events);

        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].event, Event::Midi2(_)));
        if let Event::Midi2(ump) = out[0].event {
            assert!(matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))));
        }
    }

    #[test]
    fn mapping_node_translates_pad_released_to_midi2_note_off() {
        let mut mapper = HardwareMappingNode::default_chromatic(0);
        let events = [TimedEvent::new(0, Event::Hardware(HardwareEvent::PadReleased { id: 0 }))];
        let out = run_mapper(&mut mapper, &events);

        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].event, Event::Midi2(_)));
        if let Event::Midi2(ump) = out[0].event {
            assert!(matches!(ump, UmpMessage::ChannelVoice2(ChannelVoice2::NoteOff(_))));
        }
    }

    #[test]
    fn mapping_node_passes_unknown_events_through_unchanged() {
        let mut mapper = HardwareMappingNode::default_chromatic(0);
        let events = [TimedEvent::new(0, Event::Tempo(120.0))];
        let out = run_mapper(&mut mapper, &events);

        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].event, Event::Tempo(_)));
    }

    #[test]
    fn mapping_node_ignores_pads_not_in_its_map() {
        let mapper_map: HashMap<u32, u8> = [(0, 60)].into_iter().collect();
        let mut mapper = HardwareMappingNode::from_map(mapper_map, 0);

        // Pad 99 is not in the map.
        let events = [TimedEvent::new(
            0,
            Event::Hardware(HardwareEvent::PadPressed { id: 99, velocity: 16000, pressure: 0 }),
        )];
        let out = run_mapper(&mut mapper, &events);
        assert!(out.is_empty());
    }
}
