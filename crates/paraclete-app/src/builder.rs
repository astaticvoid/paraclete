// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use paraclete_clap_host::PluginLibrary;
use paraclete_node_api::Node;
use paraclete_nodes::{InternalClock, MixNode, Sampler, Sequencer};
use paraclete_runtime::NodeConfigurator;

use crate::instrument::{InstrumentDefinition, InstrumentError, NodeDef};
use crate::registry::{build_registry, NodeRegistry};
use crate::sample_pool::SamplePool;

#[derive(Debug)]
pub struct InstrumentIds {
    pub clock: u32,
    pub clock_domain_id: u32,
    pub mix: u32,
    pub output: u32,
    pub sequencers: Vec<u32>,
    pub generators: Vec<u32>,
    pub samplers: Vec<u32>,
    pub distortions: Vec<u32>,
    pub filters: Vec<u32>,
    pub effects: Vec<u32>,
    pub all: Vec<(String, u32)>,
}

/// Parse an instrument definition from a YAML string.
///
/// This is primarily a test/internal helper; prefer `load_instrument_definition`
/// for production use.
pub fn parse_instrument_definition(text: &str) -> Result<InstrumentDefinition, InstrumentError> {
    let def: InstrumentDefinition = serde_yml::from_str(text)?;
    if def.format_version != 1 {
        return Err(InstrumentError::UnknownVersion(def.format_version));
    }
    Ok(def)
}

pub fn resolve_instrument_path(path: &std::path::Path) -> std::path::PathBuf {
    if path.is_file() {
        return path.to_path_buf();
    }
    if path.is_dir() {
        let graph = path.join("graph.yaml");
        if graph.is_file() {
            return graph;
        }
        let inst = path.join("instrument.yaml");
        if inst.is_file() {
            return inst;
        }
    }
    if let Some(stem) = path.to_str() {
        let in_dir_bundle = std::path::Path::new("instruments").join(stem).join("graph.yaml");
        if in_dir_bundle.is_file() {
            return in_dir_bundle;
        }
        let in_dir_yaml = std::path::Path::new("instruments").join(format!("{stem}.yaml"));
        if in_dir_yaml.is_file() {
            return in_dir_yaml;
        }
        let in_dir_direct = std::path::Path::new("instruments").join(stem);
        if in_dir_direct.is_file() {
            return in_dir_direct;
        }
    }
    path.to_path_buf()
}

pub fn load_instrument_definition(
    path: &std::path::Path,
) -> Result<InstrumentDefinition, InstrumentError> {
    let resolved = resolve_instrument_path(path);
    let text = std::fs::read_to_string(resolved)?;
    parse_instrument_definition(&text)
}

pub fn build_from_instrument(
    def: &InstrumentDefinition,
    conf: &mut NodeConfigurator,
    libraries: &HashMap<String, Arc<PluginLibrary>>,
    sample_pool: &mut SamplePool,
) -> Result<InstrumentIds, InstrumentError> {
    let sr = conf.sample_rate();
    let block = conf.block_size();
    let mut seen_ids: HashSet<u32> = HashSet::new();
    let mut ids = InstrumentIds {
        clock: 0,
        clock_domain_id: 0,
        mix: 0,
        output: 0,
        sequencers: Vec::new(),
        generators: Vec::new(),
        samplers: Vec::new(),
        distortions: Vec::new(),
        filters: Vec::new(),
        effects: Vec::new(),
        all: Vec::new(),
    };

    // Map from node_id → (port_name_lowercase → port_id) for name resolution.
    // Pre-populate with any hardware/endpoint nodes already registered in `conf`
    // before `build_from_instrument` (e.g. `AudioInputNode` id 61).
    let mut port_names: HashMap<u32, HashMap<String, u32>> = HashMap::new();
    for (existing_id, existing_node) in conf.all_nodes() {
        let map: HashMap<String, u32> = existing_node
            .ports()
            .iter()
            .map(|p| (p.name.as_str().to_ascii_lowercase(), p.id))
            .collect();
        port_names.insert(existing_id, map);
    }

    let registry = build_registry();

    for node_def in &def.nodes {
        if !seen_ids.insert(node_def.id) {
            return Err(InstrumentError::DuplicateNodeId(node_def.id));
        }
        let mut node =
            construct_node(node_def, def.bpm, sr, block, libraries, sample_pool, &registry)?;
        if !node_def.initial_params.is_empty() {
            node.set_initial_params(&node_def.initial_params);
        }

        let node_port_map: HashMap<String, u32> = node
            .ports()
            .iter()
            .map(|p| (p.name.as_str().to_ascii_lowercase(), p.id))
            .collect();
        port_names.insert(node_def.id, node_port_map);

        classify_node(node_def, &mut ids);
        if node_def.type_tag == "internal_clock" {
            let (_node_id, domain_id) = conf.add_tempo_source(node_def.id, node);
            ids.clock_domain_id = domain_id;
        } else {
            conf.add_node(node_def.id, node);
        }
        // Record the tag for project-file v2 saves and the Antiphon `welcome`
        // node snapshot (clients key editor views off type_tag).
        conf.set_type_tag_for(node_def.id, &node_def.type_tag);
    }

    for edge in &def.edges {
        let from_node_id =
            yaml_to_u32(&edge.from.0).ok_or_else(|| InstrumentError::UnknownPort {
                node: 0,
                port: format!("{:?}", edge.from.0),
            })?;
        let to_node_id = yaml_to_u32(&edge.to.0).ok_or_else(|| InstrumentError::UnknownPort {
            node: 0,
            port: format!("{:?}", edge.to.0),
        })?;

        let from_port = resolve_port(from_node_id, &edge.from.1, port_names.get(&from_node_id))?;
        let to_port = resolve_port(to_node_id, &edge.to.1, port_names.get(&to_node_id))?;

        conf.connect(from_node_id, from_port, to_node_id, to_port)
            .map_err(|e| InstrumentError::ConnectionError(e.to_string()))?;
    }

    Ok(ids)
}

fn construct_node(
    node_def: &NodeDef,
    bpm: f64,
    sample_rate: f32,
    block_size: usize,
    libraries: &HashMap<String, Arc<PluginLibrary>>,
    sample_pool: &mut SamplePool,
    registry: &NodeRegistry,
) -> Result<Box<dyn Node>, InstrumentError> {
    let tag = node_def.type_tag.as_str();
    let node: Box<dyn Node> = match tag {
        "internal_clock" => Box::new(InternalClock::with_bpm(bpm)),
        "sequencer" => {
            let seq = match &node_def.display_name {
                Some(name) => Sequencer::with_name(name),
                None => Sequencer::new(),
            };
            match node_def.default_note {
                Some(note) => Box::new(seq.with_default_note(note)),
                None => Box::new(seq),
            }
        }
        // #159: an instrument file may name the sample. AN0 (D10): the path
        // now registers into the pool at construction time — main thread,
        // pre-audio-thread-start, so no asset-plane traffic is needed for
        // this initial load — instead of the node loading its own file.
        // The node's own id doubles as its initial pool slot: node ids are
        // already unique within an instrument, so this needs no separate
        // slot-allocation state threaded through the builder. Diverges from
        // the `sample` param's declared 0-127 range (`ParamDescriptor` in
        // sampler.rs) if a Sampler's node id is ever >= 128 — harmless today
        // (only fixture in use is id 23), but worth revisiting if node ids
        // and pool slots need to diverge (e.g. a dynamic-topology feature
        // assigning large ids, `sampler.rs`'s `activate()` comment on P9 C4).
        "sampler" => {
            let mut s = Sampler::new();
            if let Some(path) = node_def.sample.as_deref() {
                match sample_pool.load_file(node_def.id as usize, path, sample_rate) {
                    Ok((entry, samples)) => {
                        let data = Arc::new(paraclete_node_api::SampleData {
                            samples,
                            rate: entry.rate,
                            frames: entry.frames,
                        });
                        s.swap_sample(node_def.id, data);
                        s.set_base_sample(node_def.id);
                    }
                    Err(e) => eprintln!(
                        "[paraclete] sampler {}: failed to load {path:?}: {e}",
                        node_def.id
                    ),
                }
            }
            Box::new(s)
        }
        "mix" => {
            let n = node_def
                .channel_count
                .ok_or(InstrumentError::MissingField {
                    node: node_def.id,
                    field: "channel_count",
                })?;
            Box::new(MixNode::new(n))
        }
        "clap_plugin" => {
            let plugin_id = node_def
                .plugin_id
                .as_deref()
                .ok_or(InstrumentError::MissingField {
                    node: node_def.id,
                    field: "plugin_id",
                })?;
            let lib = libraries
                .get(plugin_id)
                .ok_or_else(|| InstrumentError::PluginNotFound {
                    plugin_id: plugin_id.to_string(),
                })?;
            lib.instantiate(plugin_id, sample_rate, block_size)
                .map_err(|e| InstrumentError::ConnectionError(e.to_string()))?
        }
        other => registry.build(other).ok_or_else(|| InstrumentError::UnknownNodeType {
            type_tag: other.to_string(),
        })?,
    };
    Ok(node)
}

fn classify_node(node_def: &NodeDef, ids: &mut InstrumentIds) {
    let label = node_def
        .display_name
        .clone()
        .unwrap_or_else(|| node_def.type_tag.clone());
    ids.all.push((label, node_def.id));

    match node_def.type_tag.as_str() {
        "internal_clock" => ids.clock = node_def.id,
        "mix" => ids.mix = node_def.id,
        "audio_output" => ids.output = node_def.id,
        "sequencer" | "sequencer_cv" => ids.sequencers.push(node_def.id),
        "sampler"
        | "analog_engine:kick"
        | "analog_engine:snare"
        | "analog_engine:hihat"
        | "analog_engine:tom"
        | "analog_engine:clap"
        | "fm_engine:kick"
        | "fm_engine:bell"
        | "fm_engine:bass"
        | "fm_voice"
        | "oscillator"
        | "clap_plugin" => ids.generators.push(node_def.id),
        "distortion" | "filter" | "ladder_filter" | "reverb" | "delay" => {
            ids.effects.push(node_def.id)
        }
        _ => {}
    }
    // Typed sub-lists (used for profile script constant injection).
    match node_def.type_tag.as_str() {
        "sampler" => ids.samplers.push(node_def.id),
        "distortion" => ids.distortions.push(node_def.id),
        "filter" | "ladder_filter" => ids.filters.push(node_def.id),
        _ => {}
    }
}

fn resolve_port(
    node_id: u32,
    value: &serde_yml::Value,
    port_map: Option<&HashMap<String, u32>>,
) -> Result<u32, InstrumentError> {
    match value {
        serde_yml::Value::Number(n) => {
            n.as_u64()
                .map(|v| v as u32)
                .ok_or_else(|| InstrumentError::UnknownPort {
                    node: node_id,
                    port: format!("{n:?}"),
                })
        }
        serde_yml::Value::String(s) => {
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
            Err(InstrumentError::UnknownPort {
                node: node_id,
                port: s.clone(),
            })
        }
        other => Err(InstrumentError::UnknownPort {
            node: node_id,
            port: format!("{other:?}"),
        }),
    }
}

fn yaml_to_u32(v: &serde_yml::Value) -> Option<u32> {
    match v {
        serde_yml::Value::Number(n) => n.as_u64().map(|v| v as u32),
        serde_yml::Value::String(s) => s.parse::<u32>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_constructs_every_registered_node_type_tag() {
        let reg = build_registry();
        let tags = reg.known_type_tags();
        assert!(!tags.is_empty());

        let mut yaml = String::from("format_version: 1\nname: all_tags\nbpm: 120.0\nnodes:\n");
        for (idx, tag) in tags.iter().enumerate() {
            let id = (idx as u32) + 1;
            yaml.push_str(&format!("  - id: {id}\n    type: {tag}\n"));
            if *tag == "mix" {
                yaml.push_str("    channel_count: 4\n");
            }
        }
        yaml.push_str("edges: []\n");

        let def = parse_instrument_definition(&yaml).expect("parse all_tags yaml");
        let mut conf = NodeConfigurator::new(44100.0, 64);
        let mut pool = SamplePool::default();
        let ids = build_from_instrument(&def, &mut conf, &HashMap::new(), &mut pool)
            .expect("every registered type_tag must construct in build_from_instrument");
        assert_eq!(ids.all.len(), tags.len());
    }
}
