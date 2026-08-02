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
    /// BUG-047: the graph sink. Copies (and sums, for parallel sends) every
    /// audio input into this node's own `audio_out` — the executor's final
    /// output is the sum of sink nodes only, so a no-op here would silence
    /// the whole graph. A parallel send (engine → output + engine → reverb →
    /// output) lands as two input buffers and is summed here, which is how
    /// dry + wet reaches the master.
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        let Some(out) = output.audio_outputs.first_mut() else {
            return;
        };
        let frames = input.block_size.min(out.frames());
        for ch in 0..out.channels() {
            out.channel_mut(ch)[..frames].fill(0.0);
        }
        for audio_in in input.audio_inputs {
            // Mono inputs upmix to both channels, matching MixNode.
            // This changes audible output vs. the pre-BUG-047 behaviour
            // (mono chains rendered left-only; now they render center).
            if audio_in.channels() == 1 && out.channels() >= 2 {
                let src = audio_in.channel(0);
                for ch in 0..out.channels() {
                    let dst = out.channel_mut(ch);
                    for f in 0..frames.min(src.len()) {
                        dst[f] += src[f];
                    }
                }
            } else {
                let chs = audio_in.channels().min(out.channels());
                for ch in 0..chs {
                    let src = audio_in.channel(ch);
                    let dst = out.channel_mut(ch);
                    for f in 0..frames.min(src.len()) {
                        dst[f] += src[f];
                    }
                }
            }
        }
    }
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

    /// No exemptions. MM-C6 item 2 landed, so `machine` is paged on TRIG by
    /// every host and the last known defect is gone — the count this used to
    /// assert was 6, and the assertion existed precisely so that closing it
    /// would force this test to be updated rather than quietly pass.
    #[test]
    fn every_node_view_declaration_is_valid() {
        let mut defects: Vec<String> = Vec::new();
        for doc in every_viewed_node() {
            for d in validate_view(&doc) {
                defects.push(format!("{}: {d}", doc.name));
            }
        }
        assert!(
            defects.is_empty(),
            "invalid view declarations:\n  {}",
            defects.join("\n  ")
        );
    }

    /// The validator must actually catch the class it exists for, or the test
    /// above is decoration. Rebuilds #156 in a fixture: a page ref naming a
    /// param the node does not declare.
    /// #160 (BUG-057): a node's cap-doc must declare the same ports its
    /// `Node::ports()` returns.
    ///
    /// All three voice nodes had `ports: vec![]` in their cap-doc while the
    /// trait returned the real list. Chain derivation reads the **cap-doc**
    /// (`main.rs`'s `is_audio_out`), so it never left the engine and
    /// `CompositeView::chain` was empty for every track in the app — no
    /// per-track effect could appear in a track's pages at all. It stayed
    /// invisible because the default instrument wires no per-track effects.
    #[test]
    fn every_node_cap_doc_declares_the_ports_the_node_has() {
        use crate::*;
        use paraclete_node_api::Node;
        fn check(name: &str, node: &dyn Node) {
            let doc = node.capability_document();
            let from_trait: Vec<(u32, &str)> = node
                .ports()
                .iter()
                .map(|p| (p.id, p.name.as_str()))
                .collect();
            let from_doc: Vec<(u32, &str)> = doc
                .ports
                .iter()
                .map(|p| (p.id, p.name.as_str()))
                .collect();
            assert_eq!(
                from_doc, from_trait,
                "{name}: cap-doc ports disagree with Node::ports() — chain \
                 derivation reads the cap-doc, so this silently empties every \
                 track's chain"
            );
        }
        check("AnalogKick", &analog_engine::AnalogEngine::kick());
        check("AnalogSnare", &analog_engine::AnalogEngine::snare());
        check("AnalogHiHat", &analog_engine::AnalogEngine::hihat());
        check("FmKick", &fm_engine::FmEngine::kick());
        check("FmBell", &fm_engine::FmEngine::bell());
        check("FmBass", &fm_engine::FmEngine::bass());
        check("Sampler", &sampler::Sampler::new());
        check("FilterNode", &filter::FilterNode::new());
        check("DistortionNode", &distortion::DistortionNode::new());
        check("ReverbNode", &reverb::ReverbNode::new());
    }

    /// A source has an audio output and no audio *input*; an effect has both.
    /// That distinction is what `main.rs` uses to decide what is a track, and
    /// #160's second half was it asking only about the output — so every
    /// filter and distortion became its own track.
    #[test]
    fn sources_and_effects_are_distinguishable_by_their_ports() {
        use crate::*;
        use paraclete_node_api::{Node, PortDirection, PortType};
        let is_audio = |n: &dyn Node, dir: PortDirection| {
            n.ports()
                .iter()
                .any(|p| p.port_type == PortType::Audio && p.direction == dir)
        };
        for (name, n) in [
            ("AnalogKick", Box::new(analog_engine::AnalogEngine::kick()) as Box<dyn Node>),
            ("FmBass", Box::new(fm_engine::FmEngine::bass())),
            ("Sampler", Box::new(sampler::Sampler::new())),
        ] {
            assert!(is_audio(n.as_ref(), PortDirection::Output), "{name} has audio out");
            assert!(
                !is_audio(n.as_ref(), PortDirection::Input),
                "{name} must read as a source — the sampler's pitch/volume mod \
                 ports are Modulation, not Audio, on purpose"
            );
        }
        for (name, n) in [
            ("FilterNode", Box::new(filter::FilterNode::new()) as Box<dyn Node>),
            ("DistortionNode", Box::new(distortion::DistortionNode::new())),
        ] {
            assert!(is_audio(n.as_ref(), PortDirection::Input), "{name} has audio in");
            assert!(is_audio(n.as_ref(), PortDirection::Output), "{name} has audio out");
        }
    }

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
