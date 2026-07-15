// SPDX-License-Identifier: GPL-3.0-or-later

mod bridge;
mod host;
mod library;
mod node;
mod scan;

pub use bridge::HostParamBridge;
pub use library::{PluginDescriptor, PluginLibrary};
pub use node::PluginNode;
pub use scan::scan_clap_paths;

#[derive(Debug)]
pub enum HostError {
    /// Dynamic library could not be opened.
    Load(String),
    /// File does not export a valid CLAP entry point.
    InvalidPlugin(String),
    /// No plugin with this ID in the library.
    PluginNotFound { plugin_id: String },
    /// Plugin activation failed.
    Activate(String),
    /// Required CLAP extension absent.
    MissingExtension(&'static str),
    /// Two plugin params hash to the same Paraclete param ID.
    ParamIdCollision { param_a: String, param_b: String },
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(s) => write!(f, "load error: {s}"),
            Self::InvalidPlugin(s) => write!(f, "invalid plugin: {s}"),
            Self::PluginNotFound { plugin_id } => write!(f, "plugin not found: {plugin_id}"),
            Self::Activate(s) => write!(f, "activate error: {s}"),
            Self::MissingExtension(ext) => write!(f, "missing extension: {ext}"),
            Self::ParamIdCollision { param_a, param_b } => {
                write!(f, "param id collision: '{param_a}' and '{param_b}'")
            }
        }
    }
}

impl std::error::Error for HostError {}
