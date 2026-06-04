// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L3 first-party nodes.

pub mod delay;
pub mod distortion;
pub mod filter;
pub mod gateway;
pub mod internal_clock;
pub mod mapping;
pub mod mix;
pub mod oscillator;
pub mod pattern;
pub mod reverb;
pub mod sampler;
pub mod sequencer;

pub use delay::DelayNode;
pub use distortion::DistortionNode;
pub use filter::FilterNode;
pub use gateway::{ScriptingGatewayNode, ScriptEventConsumer};
pub use internal_clock::InternalClock;
pub use mapping::HardwareMappingNode;
pub use mix::MixNode;
pub use oscillator::{SineOscillator, midi_note_to_hz};
pub use pattern::{apply_preset, TrackPreset, TRACKS};
pub use reverb::ReverbNode;
pub use sampler::Sampler;
pub use sequencer::{Sequencer, Step, StepParamLock};

use paraclete_node_api::{
    CapabilityDocument, ConnectionAgreement, Node, PortDescriptor, PortDirection,
    PortName, PortType, ProcessInput, ProcessOutput, UmpMessage,
    midi::{ChannelVoice2, Channeled, Grouped, NoteOff, NoteOn, u4, u7},
};

pub(crate) fn build_note_on(group: u8, channel: u8, note: u8, velocity: u16) -> UmpMessage {
    let mut msg = NoteOn::<[u32; 4]>::new();
    msg.set_group(u4::new(group & 0xF));
    msg.set_channel(u4::new(channel & 0xF));
    msg.set_note_number(u7::new(note & 0x7F));
    msg.set_velocity(velocity);
    UmpMessage::from(ChannelVoice2::from(msg))
}

pub(crate) fn build_note_off(group: u8, channel: u8, note: u8) -> UmpMessage {
    let mut msg = NoteOff::<[u32; 4]>::new();
    msg.set_group(u4::new(group & 0xF));
    msg.set_channel(u4::new(channel & 0xF));
    msg.set_note_number(u7::new(note & 0x7F));
    msg.set_velocity(0);
    UmpMessage::from(ChannelVoice2::from(msg))
}

pub struct SilentNode {
    ports: [PortDescriptor; 1],
}

impl SilentNode {
    pub const PORT_AUDIO_OUT: u32 = 0;

    pub fn new() -> Self {
        Self {
            ports: [PortDescriptor {
                id: Self::PORT_AUDIO_OUT,
                name: PortName::Static("audio_out"),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            }],
        }
    }
}

impl Default for SilentNode {
    fn default() -> Self { Self::new() }
}

impl Node for SilentNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, _output: &mut ProcessOutput) {}
    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "SilentNode", vendor: "Paraclete", version: (0, 1, 0),
            ports: self.ports.to_vec(), params: vec![], extensions: vec![],
        }
    }
    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        ConnectionAgreement::baseline()
    }
}
