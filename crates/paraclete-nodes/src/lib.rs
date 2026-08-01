// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L3 first-party nodes.

pub mod analog_engine;
pub mod delay;
pub mod distortion;
pub mod engine_dsp;
pub mod envelope;
pub mod filter;
pub mod fm_engine;
pub mod gateway;
pub mod internal_clock;
pub mod ladder;
pub mod lfo;
pub mod loop_break;
pub mod mapping;
pub mod mix;
pub mod oscillator;
pub mod pattern;
pub mod reverb;
pub mod sampler;
pub mod sequencer;
pub mod split;

pub use analog_engine::{AnalogEngine, AnalogMachine};
pub use delay::DelayNode;
pub use distortion::DistortionNode;
pub use envelope::EnvelopeNode;
pub use filter::FilterNode;
pub use fm_engine::{FmEngine, FmMachine};
pub use gateway::{ScriptingGatewayNode, ScriptEventConsumer};
pub use internal_clock::InternalClock;
pub use ladder::LadderFilterNode;
pub use lfo::LfoNode;
pub use loop_break::LoopBreakNode;
pub use mapping::SurfaceMappingNode;
pub use mix::MixNode;
pub use oscillator::{OscillatorNode, SineOscillator, midi_note_to_hz};
pub use pattern::{apply_preset, TrackPreset, TRACKS};
pub use reverb::ReverbNode;
pub use sampler::Sampler;
pub use sequencer::{Pattern, Sequencer, Step, StepParamLock, PAGE_SIZE};
pub use split::SplitNode;

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
            name: "SilentNode".into(), vendor: "Paraclete".into(), version: (0, 1, 0),
            ports: self.ports.to_vec(), params: vec![], extensions: vec![],
    view: None,
        }
    }
    fn negotiate(&mut self, _their_doc: &CapabilityDocument) -> ConnectionAgreement {
        ConnectionAgreement::baseline(44100.0, 512)
    }
}

/// Audio output sink. Accepts an audio input; the HAL reads summed audio
/// from the executor directly. This node is the graph terminus for audio
/// signal flow declared in instrument definition files.
pub struct AudioOutputNode {
    ports: [PortDescriptor; 1],
}

impl AudioOutputNode {
    pub const PORT_AUDIO_IN: u32 = 0;

    pub fn new() -> Self {
        Self {
            ports: [PortDescriptor {
                id: Self::PORT_AUDIO_IN,
                name: PortName::Static("audio_in"),
                direction: PortDirection::Input,
                port_type: PortType::Audio,
            }],
        }
    }
}

impl Default for AudioOutputNode {
    fn default() -> Self { Self::new() }
}

impl Node for AudioOutputNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn process(&mut self, _input: &ProcessInput, _output: &mut ProcessOutput) {}
    fn type_name(&self) -> &'static str { "AudioOutputNode" }
}

#[cfg(test)]
mod view_validation {
    use paraclete_node_api::{validate_view, CapabilityDocument};

    /// Every node in the crate that declares a view, validated (MM-C8,
    /// ADR-041 amendment 5 as widened).
    ///
    /// **Run over every `ViewPlugin`, not just the machine hosts** — #156 is
    /// the proof that the defect class is not specific to them: `Sampler`
    /// paged `loop` while its cap-doc declared only 8 params, `loop` not among
    /// them, so it drew a working, lockable control under a `param_{id}`
    /// label; `slice` was neither declared nor paged and so was unreachable
    /// despite driving the DSP.
    fn every_viewed_node() -> Vec<CapabilityDocument> {
        use crate::*;
        use paraclete_node_api::Node;
        let mut docs: Vec<CapabilityDocument> = Vec::new();
        for m in analog_engine::AnalogMachine::ALL {
            docs.push(analog_engine::AnalogEngine::new(m).capability_document());
        }
        for m in fm_engine::FmMachine::ALL {
            docs.push(fm_engine::FmEngine::new(m).capability_document());
        }
        docs.push(sampler::Sampler::new().capability_document());
        docs.push(filter::FilterNode::new().capability_document());
        docs.push(distortion::DistortionNode::new().capability_document());
        docs.push(reverb::ReverbNode::new().capability_document());
        docs.push(mix::MixNode::new(8).capability_document());
        docs
    }

    /// **The one outstanding defect, listed explicitly so it closes itself.**
    ///
    /// `machine` is declared on every machine host and paged by none of them,
    /// so it is genuinely unreachable from any surface — the validator is
    /// telling the truth. That is MM-C6 item 2, deliberately left open: where
    /// machine-select is declared is a performer-facing decision (ADR-041
    /// amendment 2 says the TRIG page but not *who* pages it, and either
    /// answer puts a new page ahead of SRC and shifts what page keys select).
    ///
    /// Listing it rather than relaxing the check means MM-C6 item 2 cannot
    /// land without this test failing and being updated — and anything *else*
    /// that regresses still fails today.
    const KNOWN_UNPAGED_MACHINE: &str =
        "`machine` (3775092334) is declared but appears on no page";

    #[test]
    fn every_node_view_declaration_is_valid() {
        let mut unexpected: Vec<String> = Vec::new();
        let mut machine_hosts = 0;
        for doc in every_viewed_node() {
            for d in validate_view(&doc) {
                if d.message.starts_with(KNOWN_UNPAGED_MACHINE) {
                    machine_hosts += 1;
                    continue;
                }
                unexpected.push(format!("{}: {d}", doc.name));
            }
        }
        assert!(
            unexpected.is_empty(),
            "invalid view declarations:\n  {}",
            unexpected.join("\n  ")
        );
        assert_eq!(
            machine_hosts, 6,
            "exactly the six machine hosts (3 analog + 3 FM) should still have \
             an unpaged `machine`. If this dropped to 0, MM-C6 item 2 landed — \
             delete the exemption. If it grew, a new host arrived without \
             paging its selector."
        );
    }

    /// The validator must actually catch the class it exists for, or the test
    /// above is decoration. Rebuilds #156 in a fixture: a page ref naming a
    /// param the node does not declare.
    #[test]
    fn the_validator_catches_an_undeclared_page_ref() {
        use crate::sampler;
        use paraclete_node_api::{Node, PageRef, Rule};
        use std::borrow::Cow;

        let mut doc = sampler::Sampler::new().capability_document();
        let mut rule = doc.view.clone().expect("sampler declares a view");
        let mut pages = rule.param_pages.to_vec();
        pages.push((
            0xDEAD_BEEF,
            PageRef { page: Cow::Borrowed("SRC"), slot: 7 },
        ));
        rule.param_pages = Cow::Owned(pages);
        doc.view = Some(Rule { ..rule });

        let defects = validate_view(&doc);
        assert!(
            defects.iter().any(|d| d.message.contains("3735928559")),
            "an undeclared page ref must be reported: {defects:?}"
        );
    }
}
