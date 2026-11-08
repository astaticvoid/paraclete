// SPDX-License-Identifier: GPL-3.0-or-later
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug)]
pub struct InstrumentDefinition {
    pub format_version: u32,
    pub name: String,
    pub bpm: f64,
    pub nodes: Vec<NodeDef>,
    pub edges: Vec<EdgeDef>,
    #[serde(default)]
    pub macros: Vec<MacroDef>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub lcxl: Option<LcxlDef>,
}

/// Declarative Launch Control XL surface mapping for an instrument.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct LcxlDef {
    /// Per-track column knob overrides (up to 8 columns, in track order).
    #[serde(default)]
    pub tracks: Vec<LcxlTrackDef>,
    /// FX layer Row 3 master effect knob targets (up to 8 columns, `0..8`).
    #[serde(default)]
    pub master_fx: Vec<ParamTargetDef>,
}

/// Per-track column knob mapping on Launch Control XL.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct LcxlTrackDef {
    /// Normal layer knobs: `[row1, row2, row3]`.
    #[serde(default)]
    pub normal: Vec<ParamTargetDef>,
    /// FX layer track knobs: `[row1_fx, row2_fx]`.
    #[serde(default)]
    pub fx: Vec<ParamTargetDef>,
}

/// A `(node_id, param_name)` binding target for hardware controls.
#[derive(Deserialize, Debug, Clone)]
pub struct ParamTargetDef {
    pub node: u32,
    pub param: String,
}

#[derive(Deserialize, Debug)]
pub struct NodeDef {
    pub id: u32,
    #[serde(rename = "type")]
    pub type_tag: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub initial_params: HashMap<String, f64>,
    pub plugin_id: Option<String>,
    pub plugin_path: Option<String>,
    pub channel_count: Option<usize>,
    /// Sequencer only: note for steps never explicitly set (BUG-022) —
    /// match the downstream engine's trigger reference (36 for the synth
    /// drum engines; omit for samplers, whose root_note is 60).
    pub default_note: Option<u8>,
    /// Sampler only (#159 / INFRA-019): path to the `.wav` this sampler loads,
    /// relative to the working directory — e.g. `samples/kick.wav`, which
    /// `cargo run -p gen-samples` produces.
    ///
    /// Without this a `sampler` node loads nothing and renders **silence**,
    /// and since baseline mode does not evaluate a scenario's assertions
    /// (`test-driver/src/main.rs`), a sampler scenario would fingerprint as
    /// zeros and pass forever. That is the trap #155 exists to avoid, and it
    /// is why `Sampler` had no regression coverage at all until this existed.
    pub sample: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct EdgeDef {
    pub from: (serde_yml::Value, serde_yml::Value),
    pub to: (serde_yml::Value, serde_yml::Value),
}

#[derive(Deserialize, Debug, Clone)]
pub struct MacroDef {
    pub encoder: u32,
    pub node: u32,
    pub param: String,
}

#[derive(Debug)]
pub enum InstrumentError {
    Io(std::io::Error),
    Parse(serde_yml::Error),
    UnknownVersion(u32),
    UnknownNodeType { type_tag: String },
    UnknownPort { node: u32, port: String },
    ConnectionError(String),
    DuplicateNodeId(u32),
    PluginNotFound { plugin_id: String },
    MissingField { node: u32, field: &'static str },
}

impl std::fmt::Display for InstrumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Parse(e) => write!(f, "YAML parse error: {e}"),
            Self::UnknownVersion(v) => write!(f, "unknown format_version: {v}"),
            Self::UnknownNodeType { type_tag } => write!(f, "unknown node type: {type_tag}"),
            Self::UnknownPort { node, port } => write!(f, "unknown port '{port}' on node {node}"),
            Self::ConnectionError(msg) => write!(f, "connection error: {msg}"),
            Self::DuplicateNodeId(id) => write!(f, "duplicate node id: {id}"),
            Self::PluginNotFound { plugin_id } => write!(f, "CLAP plugin not found: {plugin_id}"),
            Self::MissingField { node, field } => {
                write!(f, "node {node} missing required field '{field}'")
            }
        }
    }
}

impl From<std::io::Error> for InstrumentError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_yml::Error> for InstrumentError {
    fn from(e: serde_yml::Error) -> Self {
        Self::Parse(e)
    }
}

impl std::error::Error for InstrumentError {}
