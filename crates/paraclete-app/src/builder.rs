// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use paraclete_clap_host::PluginLibrary;
use paraclete_nodes::{
    AnalogEngine, AudioOutputNode, DistortionNode, FilterNode, FmEngine, InternalClock, MixNode,
    ReverbNode, Sampler, Sequencer,
};
use paraclete_node_api::Node;
use paraclete_runtime::NodeConfigurator;

use crate::instrument::{InstrumentDefinition, InstrumentError, NodeDef};

#[derive(Debug)]
pub struct InstrumentIds {
    pub clock:           u32,
    pub clock_domain_id: u32,
    pub mix:             u32,
    pub output:          u32,
    pub sequencers:      Vec<u32>,
    pub generators:      Vec<u32>,
    pub samplers:        Vec<u32>,
    pub distortions:     Vec<u32>,
    pub filters:         Vec<u32>,
    pub effects:         Vec<u32>,
    pub all:             Vec<(String, u32)>,
}

/// Parse an instrument definition from a YAML string.
///
/// This is primarily a test/internal helper; prefer `load_instrument_definition`
/// for production use.
pub fn parse_instrument_definition(
    text: &str,
) -> Result<InstrumentDefinition, InstrumentError> {
    let def: InstrumentDefinition = serde_yaml::from_str(text)?;
    if def.format_version != 1 {
        return Err(InstrumentError::UnknownVersion(def.format_version));
    }
    Ok(def)
}

pub fn load_instrument_definition(
    path: &std::path::Path,
) -> Result<InstrumentDefinition, InstrumentError> {
    let text = std::fs::read_to_string(path)?;
    parse_instrument_definition(&text)
}

pub fn build_from_instrument(
    def:       &InstrumentDefinition,
    conf:      &mut NodeConfigurator,
    libraries: &HashMap<String, Arc<PluginLibrary>>,
) -> Result<InstrumentIds, InstrumentError> {
    let sr    = conf.sample_rate();
    let block = conf.block_size();
    let mut seen_ids: HashSet<u32> = HashSet::new();
    let mut ids = InstrumentIds {
        clock:           0,
        clock_domain_id: 0,
        mix:             0,
        output:          0,
        sequencers:      Vec::new(),
        generators:      Vec::new(),
        samplers:        Vec::new(),
        distortions:     Vec::new(),
        filters:         Vec::new(),
        effects:         Vec::new(),
        all:             Vec::new(),
    };

    // Map from node_id → (port_name_lowercase → port_id) for name resolution.
    let mut port_names: HashMap<u32, HashMap<String, u32>> = HashMap::new();

    for node_def in &def.nodes {
        if !seen_ids.insert(node_def.id) {
            return Err(InstrumentError::DuplicateNodeId(node_def.id));
        }
        let mut node = construct_node(node_def, def.bpm, sr, block, libraries)?;
        if !node_def.initial_params.is_empty() {
            node.set_initial_params(&node_def.initial_params);
        }

        let node_port_map: HashMap<String, u32> = node.ports().iter().map(|p| {
            (p.name.as_str().to_ascii_lowercase(), p.id)
        }).collect();
        port_names.insert(node_def.id, node_port_map);

        classify_node(node_def, &mut ids);
        if node_def.type_tag == "internal_clock" {
            let (_node_id, domain_id) = conf.add_tempo_source(node_def.id, node);
            ids.clock_domain_id = domain_id;
        } else {
            conf.add_node(node_def.id, node);
        }
    }

    for edge in &def.edges {
        let from_node_id = yaml_to_u32(&edge.from.0).ok_or_else(|| {
            InstrumentError::UnknownPort { node: 0, port: format!("{:?}", edge.from.0) }
        })?;
        let to_node_id = yaml_to_u32(&edge.to.0).ok_or_else(|| {
            InstrumentError::UnknownPort { node: 0, port: format!("{:?}", edge.to.0) }
        })?;

        let from_port = resolve_port(
            from_node_id,
            &edge.from.1,
            port_names.get(&from_node_id),
        )?;
        let to_port = resolve_port(
            to_node_id,
            &edge.to.1,
            port_names.get(&to_node_id),
        )?;

        conf.connect(from_node_id, from_port, to_node_id, to_port)
            .map_err(InstrumentError::ConnectionError)?;
    }

    Ok(ids)
}

fn construct_node(
    node_def:   &NodeDef,
    bpm:        f64,
    sample_rate: f32,
    block_size:  usize,
    libraries:  &HashMap<String, Arc<PluginLibrary>>,
) -> Result<Box<dyn Node>, InstrumentError> {
    let tag = node_def.type_tag.as_str();
    let node: Box<dyn Node> = match tag {
        "internal_clock" => Box::new(InternalClock::with_bpm(bpm)),
        "sequencer" => match &node_def.display_name {
            Some(name) => Box::new(Sequencer::with_name(name)),
            None       => Box::new(Sequencer::new()),
        },
        "sampler"             => Box::new(Sampler::new()),
        "analog_engine:kick"  => Box::new(AnalogEngine::kick()),
        "analog_engine:snare" => Box::new(AnalogEngine::snare()),
        "analog_engine:hihat" => Box::new(AnalogEngine::hihat()),
        "fm_engine:kick"      => Box::new(FmEngine::kick()),
        "fm_engine:bell"      => Box::new(FmEngine::bell()),
        "fm_engine:bass"      => Box::new(FmEngine::bass()),
        "distortion"   => Box::new(DistortionNode::new()),
        "filter"       => Box::new(FilterNode::new()),
        "mix" => {
            let n = node_def.channel_count.ok_or(InstrumentError::MissingField {
                node: node_def.id,
                field: "channel_count",
            })?;
            Box::new(MixNode::new(n))
        }
        "audio_output" => Box::new(AudioOutputNode::new()),
        "reverb"       => Box::new(ReverbNode::new()),
        "clap_plugin" => {
            let plugin_id = node_def.plugin_id.as_deref()
                .ok_or(InstrumentError::MissingField { node: node_def.id, field: "plugin_id" })?;
            let lib = libraries.get(plugin_id)
                .ok_or_else(|| InstrumentError::PluginNotFound {
                    plugin_id: plugin_id.to_string(),
                })?;
            lib.instantiate(plugin_id, sample_rate, block_size)
                .map_err(|e| InstrumentError::ConnectionError(e.to_string()))?
        }
        _ => return Err(InstrumentError::UnknownNodeType {
            type_tag: tag.to_string(),
        }),
    };
    Ok(node)
}

fn classify_node(node_def: &NodeDef, ids: &mut InstrumentIds) {
    let label = node_def.display_name.clone()
        .unwrap_or_else(|| node_def.type_tag.clone());
    ids.all.push((label, node_def.id));

    match node_def.type_tag.as_str() {
        "internal_clock" => ids.clock  = node_def.id,
        "mix"            => ids.mix    = node_def.id,
        "audio_output"   => ids.output = node_def.id,
        "sequencer"      => ids.sequencers.push(node_def.id),
        "sampler"
        | "analog_engine:kick"
        | "analog_engine:snare"
        | "analog_engine:hihat"
        | "fm_engine:kick"
        | "fm_engine:bell"
        | "fm_engine:bass"
        | "clap_plugin" => ids.generators.push(node_def.id),
        "distortion"
        | "filter"
        | "reverb" => ids.effects.push(node_def.id),
        _ => {}
    }
    // Typed sub-lists (used for profile script constant injection).
    match node_def.type_tag.as_str() {
        "sampler"     => ids.samplers.push(node_def.id),
        "distortion"  => ids.distortions.push(node_def.id),
        "filter"      => ids.filters.push(node_def.id),
        _ => {}
    }
}

fn resolve_port(
    node_id: u32,
    value: &serde_yaml::Value,
    port_map: Option<&HashMap<String, u32>>,
) -> Result<u32, InstrumentError> {
    match value {
        serde_yaml::Value::Number(n) => {
            n.as_u64().map(|v| v as u32).ok_or_else(|| InstrumentError::UnknownPort {
                node: node_id,
                port: format!("{n:?}"),
            })
        }
        serde_yaml::Value::String(s) => {
            let lower = s.to_ascii_lowercase();
            if let Ok(n) = lower.parse::<u32>() {
                return Ok(n);
            }
            if let Some(map) = port_map {
                if let Some(&id) = map.get(lower.as_str()) {
                    return Ok(id);
                }
                // "audio_out" aliases "audio_out_l" for generator nodes (e.g. AnalogEngine).
                if lower == "audio_out" {
                    if let Some(&id) = map.get("audio_out_l") {
                        return Ok(id);
                    }
                }
            }
            Err(InstrumentError::UnknownPort { node: node_id, port: s.clone() })
        }
        other => Err(InstrumentError::UnknownPort {
            node: node_id,
            port: format!("{other:?}"),
        }),
    }
}

fn yaml_to_u32(v: &serde_yaml::Value) -> Option<u32> {
    match v {
        serde_yaml::Value::Number(n) => n.as_u64().map(|v| v as u32),
        serde_yaml::Value::String(s) => s.parse::<u32>().ok(),
        _ => None,
    }
}
