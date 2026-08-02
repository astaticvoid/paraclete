// SPDX-License-Identifier: GPL-3.0-or-later
//! MixNode — N stereo audio inputs → 1 stereo output.

use std::borrow::Cow;
use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit, PortDescriptor,
    PortDirection, PortName, PortType, ProcessInput, ProcessOutput, StateBusValue,
    Rule, ViewPlugin, PageRef,
};

/// Name-derived id for input `i`'s gain param. The ids are stable across a
/// change in `num_inputs` (BUG-060): `master_gain` keeps its id under any
/// input count, and a shrunken count drops only the inputs that no longer
/// exist. The pre-fix scheme (`id: i as u32`, master at `num_inputs`) made
/// master's id collide with an input's the moment the count changed — a
/// project saved with 8 inputs and reloaded with 4 silently landed input 4's
/// gain on the master fader.
fn input_param_id(i: usize) -> u32 {
    ParamDescriptor::id_for_name(&format!("input_gain_{i}"))
}

pub struct MixNode {
    ports: Vec<PortDescriptor>,
    node_id: u32,
    num_inputs: usize,
    input_ids: Vec<u32>,
    master_id: u32,
    bank: ParameterBank,
    render_l: Vec<f32>,
    render_r: Vec<f32>,
}

impl MixNode {
    /// `n` stereo inputs. Input gains are `input_gain_0..n-1` (0.0–2.0),
    /// master gain is `master_gain` (0.0–2.0, default 1.0) — all ids
    /// name-derived, so persistence survives an input-count change.
    pub fn new(num_inputs: usize) -> Self {
        let mut ports = Vec::new();
        for i in 0..num_inputs {
            ports.push(PortDescriptor {
                id: i as u32,
                name: "audio_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Audio,
            });
        }
        ports.push(PortDescriptor {
            id: num_inputs as u32,
            name: "audio_out".into(),
            direction: PortDirection::Output,
            port_type: PortType::Audio,
        });
        Self {
            ports,
            node_id: 0,
            num_inputs,
            input_ids: (0..num_inputs).map(input_param_id).collect(),
            master_id: ParamDescriptor::id_for_name("master_gain"),
            bank: ParameterBank::empty(),
            render_l: Vec::new(),
            render_r: Vec::new(),
        }
    }

    pub fn port_audio_out(&self) -> u32 { self.num_inputs as u32 }
}

impl Node for MixNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn capability_document(&self) -> CapabilityDocument {
        let mut params: Vec<ParamDescriptor> = self.input_ids.iter().enumerate().map(|(i, id)| ParamDescriptor {
            id: *id,
            name: PortName::Dynamic(format!("input_gain_{i}")),
            min: 0.0,
            max: 2.0,
            default: 1.0,
            stepped: false,
            in_kit: true,
            unit: ParamUnit::Generic,
            display: None,
        }).collect();
        params.push(ParamDescriptor {
            id: self.master_id,
            name: "master_gain".into(),
            min: 0.0,
            max: 2.0,
            default: 1.0,
            stepped: false,
            in_kit: true,
            unit: ParamUnit::Generic,
            display: None,
        });
        CapabilityDocument {
            name: "MixNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 4, 0),
            ports: self.ports.clone(),
            params,
            extensions: vec![],
    view: Some(self.to_rule(0, &[])),
        }
    }

    fn activate(&mut self, _sr: f32, block: usize) {
        self.bank = ParameterBank::from_capability_document(&self.capability_document());
        self.render_l = vec![0.0; block];
        self.render_r = vec![0.0; block];
    }

    /// Bank only (#154) — this node is stateless between blocks.
    fn serialize(&self) -> Vec<u8> {
        self.bank.serialize()
    }

    fn deserialize(&mut self, data: &[u8]) {
        self.bank.deserialize(data);
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let frames = input.block_size;
        self.render_l[..frames].fill(0.0);
        self.render_r[..frames].fill(0.0);

        let master = self.bank.get(self.master_id) as f32;

        for (i, audio_in) in input
            .audio_inputs
            .iter()
            .take(self.input_ids.len())
            .enumerate()
        {
            let gain = self.bank.get(self.input_ids[i]) as f32 * master;
            if audio_in.channels() >= 1 {
                let ch = audio_in.channel(0);
                for f in 0..frames.min(ch.len()) {
                    self.render_l[f] += ch[f] * gain;
                }
            }
            if audio_in.channels() >= 2 {
                let ch = audio_in.channel(1);
                for f in 0..frames.min(ch.len()) {
                    self.render_r[f] += ch[f] * gain;
                }
            } else if audio_in.channels() >= 1 {
                // Mono → both channels
                let ch = audio_in.channel(0);
                for f in 0..frames.min(ch.len()) {
                    self.render_r[f] += ch[f] * gain;
                }
            }
        }

        if let Some(out) = output.audio_outputs.first_mut() {
            if out.channels() >= 1 {
                out.channel_mut(0)[..frames].copy_from_slice(&self.render_l[..frames]);
            }
            if out.channels() >= 2 {
                out.channel_mut(1)[..frames].copy_from_slice(&self.render_r[..frames]);
            }
        }
    }
}

impl ViewPlugin for MixNode {
    fn to_rule(&self, _node_id: u64, _sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule {
        let mut pages = Vec::with_capacity(self.num_inputs as usize + 1);
        for i in 0..self.num_inputs {
            pages.push((self.input_ids[i], PageRef { page: Cow::Borrowed("FX"), slot: i as u8 }));
        }
        pages.push((self.master_id, PageRef { page: Cow::Borrowed("FX"), slot: self.num_inputs as u8 }));
        Rule {
            name: Cow::Borrowed("Mix"),
            page_groups: Cow::Owned(vec![Cow::Borrowed("FX")]),
            param_pages: Cow::Owned(pages),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    fn run_mix(mix: &mut MixNode, inputs: &[AudioBuffer]) -> AudioBuffer {
        let block = 64usize;
        let mut out = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let refs: Vec<&AudioBuffer> = inputs.iter().collect();
        let out_ptr: *mut AudioBuffer = &mut out;
        let out_ref: &mut AudioBuffer = unsafe { &mut *out_ptr };
        let mut outs = [out_ref];

        let input = ProcessInput {
            audio_inputs: &refs,
            signal_inputs: &[],
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
        mix.process(&input, &mut output);
        out
    }

    #[test]
    fn mix_node_sums_inputs_with_unity_gain() {
        let mut mix = MixNode::new(2);
        mix.activate(44100.0, 64);

        let mut a = AudioBuffer::new(2, 64);
        let mut b = AudioBuffer::new(2, 64);
        a.channel_mut(0).fill(0.5);
        b.channel_mut(0).fill(0.5);

        let out = run_mix(&mut mix, &[a, b]);
        assert!((out.channel(0)[0] - 1.0).abs() < 1e-5);
    }

    /// Every declared param, not just a spot check — a hand-written
    /// serializer that forgot a slot would pass one of these.
    #[test]
    fn mix_params_survive_a_save_and_load() {
        let mut saved = MixNode::new(8);
        saved.activate(44100.0, 64);
        for (i, id) in saved.input_ids.iter().enumerate() {
            saved.bank.set(*id, 0.5 + i as f64 * 0.1);
        }
        saved.bank.set(saved.master_id, 0.8);

        let mut loaded = MixNode::new(8);
        loaded.activate(44100.0, 64);
        loaded.deserialize(&saved.serialize());

        for (i, id) in loaded.input_ids.iter().enumerate() {
            assert_eq!(loaded.bank.get(*id), 0.5 + i as f64 * 0.1, "input {i}");
        }
        assert_eq!(loaded.bank.get(loaded.master_id), 0.8, "master_gain");
    }

    /// The exact failure BUG-060 exists for: with positional ids, reloading
    /// a project saved with 8 inputs under a 4-input graph dropped
    /// `master_gain` (id 8) and silently landed input 4's gain on the master
    /// fader. Name-derived ids keep master on master and drop only the
    /// inputs that no longer exist.
    #[test]
    fn a_saved_mix_reloaded_with_fewer_inputs_keeps_master_and_drops_extras() {
        let mut saved = MixNode::new(8);
        saved.activate(44100.0, 64);
        for (i, id) in saved.input_ids.iter().enumerate() {
            saved.bank.set(*id, 0.1 + i as f64 * 0.2);
        }
        saved.bank.set(saved.master_id, 0.7);

        let mut loaded = MixNode::new(4);
        loaded.activate(44100.0, 64);
        loaded.deserialize(&saved.serialize());

        for (i, id) in loaded.input_ids.iter().enumerate() {
            assert_eq!(loaded.bank.get(*id), 0.1 + i as f64 * 0.2, "input {i}");
        }
        assert_eq!(
            loaded.bank.get(loaded.master_id),
            0.7,
            "master_gain must not be replaced by a shrunken input count"
        );
        // The dropped inputs no longer exist — their saved gains must not
        // land on anything (input_gain_4's id is unknown to a 4-input mix).
        assert_eq!(loaded.bank.get(1701754572), 0.0, "input_gain_4 must be dropped");
    }

    /// The ids are name-derived and now written into project files, so a
    /// rename of `input_gain_{i}` / `master_gain` would silently orphan every
    /// saved value for it (AGENTS.md: a param id is a persistence key,
    /// append-only). Pin the literals; extend, never reorder or rename.
    #[test]
    fn the_persisted_mix_param_ids_are_stable() {
        assert_eq!(
            MixNode::new(8).input_ids,
            vec![
                1634644096, // input_gain_0
                1651421715, // input_gain_1
                1668199334, // input_gain_2
                1684976953, // input_gain_3
                1701754572, // input_gain_4
                1718532191, // input_gain_5
                1735309810, // input_gain_6
                1752087429, // input_gain_7
            ]
        );
        assert_eq!(MixNode::new(8).master_id, 1181736839); // master_gain
        assert_eq!(MixNode::new(4).master_id, 1181736839,
            "master_gain's id must not depend on the input count");
    }

    /// INFRA-015: the ViewPlugin impl must be reachable — to_rule() is useless
    /// if capability_document never assigns it.
    #[test]
    fn mix_node_capability_document_has_view_rule() {
        let mix = MixNode::new(4);
        let doc = mix.capability_document();
        assert!(doc.view.is_some(), "view must be Some — INFRA-015");
        let rule = doc.view.unwrap();
        assert_eq!(rule.name, "Mix");
        assert_eq!(rule.page_groups.len(), 1);
        assert_eq!(rule.page_groups[0], "FX");
        // 4 input gains + master = 5 slots on the FX page
        assert_eq!(rule.param_pages.len(), 5);
    }
}
