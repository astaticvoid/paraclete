// SPDX-License-Identifier: GPL-3.0-or-later
//! MixNode — N stereo audio inputs → 1 stereo output.

use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit, PortDescriptor,
    PortDirection, PortType, ProcessInput, ProcessOutput,
};

pub struct MixNode {
    ports: Vec<PortDescriptor>,
    node_id: u32,
    num_inputs: usize,
    bank: ParameterBank,
    render_l: Vec<f32>,
    render_r: Vec<f32>,
}

impl MixNode {
    /// `n` stereo inputs. Each input gets param_id = input_index (gain 0.0–2.0).
    /// Master gain param_id = n (0.0–2.0, default 1.0).
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
        let mut params: Vec<ParamDescriptor> = (0..self.num_inputs).map(|i| ParamDescriptor {
            id: i as u32,
            name: "input_gain".into(),
            min: 0.0,
            max: 2.0,
            default: 1.0,
            stepped: false,
            unit: ParamUnit::Generic,
            display: None,
        }).collect();
        params.push(ParamDescriptor {
            id: self.num_inputs as u32,
            name: "master_gain".into(),
            min: 0.0,
            max: 2.0,
            default: 1.0,
            stepped: false,
            unit: ParamUnit::Generic,
            display: None,
        });
        CapabilityDocument {
            name: "MixNode",
            vendor: "Paraclete",
            version: (0, 4, 0),
            ports: self.ports.clone(),
            params,
            extensions: vec![],
        }
    }

    fn activate(&mut self, _sr: f32, block: usize) {
        self.bank = ParameterBank::from_capability_document(&self.capability_document());
        self.render_l = vec![0.0; block];
        self.render_r = vec![0.0; block];
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        let frames = input.block_size;
        self.render_l[..frames].fill(0.0);
        self.render_r[..frames].fill(0.0);

        let master = self.bank.get(self.num_inputs as u32) as f32;

        for (i, audio_in) in input.audio_inputs.iter().enumerate() {
            let gain = self.bank.get(i as u32) as f32 * master;
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
        let mut output = ProcessOutput {
            audio_outputs: &mut outs,
            signal_outputs: &mut [],
            events_out: &mut events_out,
        };
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
}
