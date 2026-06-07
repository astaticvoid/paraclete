// SPDX-License-Identifier: GPL-3.0-or-later
//! NodeRegistry — maps type-tag strings to zero-argument constructors (ADR-029).
//!
//! `build_registry()` registers all first-party node types. CLAP plugin nodes
//! are not registered here — they require a `PluginLibrary` argument and are
//! inserted into the graph directly.

use std::collections::HashMap;
use std::sync::Arc;

use paraclete_node_api::Node;

pub struct NodeRegistry {
    constructors: HashMap<String, Arc<dyn Fn() -> Box<dyn Node> + Send + Sync>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        NodeRegistry {
            constructors: HashMap::new(),
        }
    }

    /// Register a type-tag with a zero-argument constructor.
    pub fn register(
        &mut self,
        type_tag: impl Into<String>,
        ctor: impl Fn() -> Box<dyn Node> + Send + Sync + 'static,
    ) {
        self.constructors.insert(type_tag.into(), Arc::new(ctor));
    }

    /// Construct a new node for the given type_tag.
    /// Returns `None` if the type_tag is not registered.
    pub fn build(&self, type_tag: &str) -> Option<Box<dyn Node>> {
        self.constructors.get(type_tag).map(|ctor| ctor())
    }

    /// Return all registered type tags, sorted lexicographically.
    pub fn known_type_tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self.constructors.keys().map(|s| s.as_str()).collect();
        tags.sort();
        tags
    }
}

impl Default for NodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the standard registry with all first-party node types.
///
/// Nodes with machine-variant constructors (AnalogEngine, FmEngine) are
/// registered under separate type-tags per machine variant.
pub fn build_registry() -> NodeRegistry {
    use paraclete_nodes::{
        AnalogEngine, AudioOutputNode, DelayNode, DistortionNode, EnvelopeNode, FilterNode,
        FmEngine, InternalClock, LadderFilterNode, LfoNode, LoopBreakNode, MixNode,
        OscillatorNode, ReverbNode, Sampler, Sequencer, SplitNode,
    };
    use paraclete_graph_nodes::InnerGraphNode;

    let mut r = NodeRegistry::new();

    r.register("internal_clock",      || Box::new(InternalClock::new()));
    r.register("sequencer",           || Box::new(Sequencer::new()));
    r.register("sequencer_cv",        || Box::new(Sequencer::with_cv_outputs(1)));
    r.register("sampler",             || Box::new(Sampler::new()));
    r.register("loop_break",          || Box::new(LoopBreakNode::new()));
    r.register("distortion",          || Box::new(DistortionNode::new()));
    r.register("filter",              || Box::new(FilterNode::new()));
    r.register("ladder_filter",       || Box::new(LadderFilterNode::new()));
    r.register("oscillator",          || Box::new(OscillatorNode::new()));
    r.register("envelope",            || Box::new(EnvelopeNode::new()));
    r.register("lfo",                 || Box::new(LfoNode::new()));
    r.register("reverb",              || Box::new(ReverbNode::new()));
    r.register("delay",               || Box::new(DelayNode::new()));
    r.register("mix",                 || Box::new(MixNode::new(8)));
    r.register("split",               || Box::new(SplitNode::new()));
    r.register("audio_output",        || Box::new(AudioOutputNode::new()));
    r.register("analog_engine:kick",  || Box::new(AnalogEngine::kick()));
    r.register("analog_engine:snare", || Box::new(AnalogEngine::snare()));
    r.register("analog_engine:hihat", || Box::new(AnalogEngine::hihat()));
    r.register("fm_engine:kick",      || Box::new(FmEngine::kick()));
    r.register("fm_engine:bell",      || Box::new(FmEngine::bell()));
    r.register("fm_engine:bass",      || Box::new(FmEngine::bass()));
    r.register("inner_graph",         || Box::new(InnerGraphNode::new()));

    r
}
