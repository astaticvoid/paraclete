# paraclete-node-api

The public node contract for the [Paraclete](https://github.com/paraclete-audio/paraclete)
audio processing platform.

Implement `Node` to write a portable audio processing node. Nodes written against
this API are compatible with the Paraclete runtime, any `paraclete-clap` plugin
wrapper, and any future platform host.

## License

LGPL-3.0-or-later. Third-party nodes may link against this crate and remain
closed-source. Only the API crate itself is LGPL; the runtime and built-in nodes
are GPL-3.0. See the [repository](https://github.com/paraclete-audio/paraclete) for details.

## Quick Start

```toml
[dependencies]
paraclete-node-api = "0.1"
```

## Example: Gain Node

```rust
use paraclete_node_api::{
    CapabilityDocument, Node, ParameterBank, ParamDescriptor, ParamUnit,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput,
};

const GAIN_ID: u32 = ParamDescriptor::id_for_name("gain");

pub struct GainNode {
    bank:  ParameterBank,
    ports: Vec<PortDescriptor>,
}

impl GainNode {
    pub fn new() -> Self {
        let ports = vec![
            PortDescriptor { id: 0, name: "in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
            PortDescriptor { id: 1, name: "out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
        ];
        Self { bank: ParameterBank::empty(), ports }
    }
}

impl Node for GainNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {
        self.bank = ParameterBank::from_capability_document(&self.capability_document());
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);
        let gain = self.bank.get(GAIN_ID) as f32;
        if let (Some(src), Some(dst)) = (input.audio_inputs.get(0), output.audio_outputs.get_mut(0)) {
            for ch in 0..dst.channels() {
                for (o, &s) in dst.channel_mut(ch).iter_mut().zip(src.channel(ch)) {
                    *o = s * gain;
                }
            }
        }
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "GainNode", vendor: "example", version: (0, 1, 0),
            ports: self.ports.clone(),
            params: vec![ParamDescriptor {
                id: GAIN_ID, name: "gain".into(),
                min: 0.0, max: 2.0, default: 1.0,
                stepped: false, unit: ParamUnit::Generic, display: None,
            }],
            extensions: vec![],
        }
    }
}
```

## Architecture

Paraclete uses a five-layer architecture. `paraclete-node-api` is **Layer 2 (L2)**
— the LGPL-licensed boundary. Third-party nodes only need to link against L2.

```
L0 HAL       (GPL3) — hardware I/O
L1 Runtime   (GPL3) — node graph, scheduling
L2 Node API  (LGPL3) ← this crate
L3 Nodes     (GPL3) — first-party node implementations
L4 Scripting (GPL3) — Rhai live scripting
```

## Parameter Naming

Parameter IDs are stable hashes of name strings. Use the canonical names from
ADR-019 so any hardware encoder mapped to a name reaches every node that
declares it:

| Name | Typical use |
|------|-------------|
| `"cutoff"` | Filter cutoff frequency |
| `"resonance"` | Filter resonance / Q |
| `"drive"` | Saturation / overdrive amount |
| `"wet"` / `"dry"` | Effect wet/dry mix |
| `"decay"` / `"attack"` / `"release"` | Envelope times |
| `"tune"` | Pitch offset in semitones |

**Parameter names published to crates.io are long-term contracts — renaming
after publication is a semver-breaking change.**
