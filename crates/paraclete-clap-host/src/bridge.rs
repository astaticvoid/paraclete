// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::CStr;

use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS};
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::string_sizes::CLAP_NAME_SIZE;

use paraclete_node_api::capability::{CapabilityDocument, ParamDescriptor, ParamUnit};
use paraclete_node_api::port::{PortDescriptor, PortDirection, PortName, PortType};

pub(crate) struct HostParamEntry {
    pub clap_id: clap_id,
    pub paraclete_id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

/// Maps a loaded plugin's native CLAP param IDs to Paraclete param ID hashes
/// and synthesises the node's `CapabilityDocument`.
pub struct HostParamBridge {
    pub(crate) entries: Vec<HostParamEntry>,
}

impl HostParamBridge {
    /// Build a bridge by querying the plugin's `clap_plugin_params` extension.
    /// Returns an empty bridge if the extension is absent.
    ///
    /// # Safety
    /// `plugin` must be a valid, initialised CLAP plugin pointer.
    pub(crate) unsafe fn from_plugin(plugin: *const clap_plugin) -> Self {
        let get_ext = match (*plugin).get_extension {
            Some(f) => f,
            None => return Self { entries: vec![] },
        };

        let params_ptr = get_ext(plugin, CLAP_EXT_PARAMS.as_ptr());
        if params_ptr.is_null() {
            return Self { entries: vec![] };
        }
        let params = &*(params_ptr as *const clap_plugin_params);

        let count = match params.count {
            Some(f) => f(plugin),
            None => return Self { entries: vec![] },
        };

        let mut entries = Vec::new();
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();

        for i in 0..count {
            let mut info = std::mem::zeroed::<clap_param_info>();
            let ok = match params.get_info {
                Some(f) => f(plugin, i, &mut info),
                None => false,
            };
            if !ok {
                continue;
            }

            let name_bytes = &info.name[..CLAP_NAME_SIZE];
            let name = CStr::from_ptr(name_bytes.as_ptr())
                .to_string_lossy()
                .into_owned();

            let paraclete_id = ParamDescriptor::id_for_name(&name);
            if seen.contains(&paraclete_id) {
                #[cfg(debug_assertions)]
                eprintln!("HostParamBridge: param ID collision for '{name}' — skipped");
                continue;
            }
            seen.insert(paraclete_id);

            entries.push(HostParamEntry {
                clap_id: info.id,
                paraclete_id,
                name,
                min: info.min_value,
                max: info.max_value,
                default: info.default_value,
            });
        }

        Self { entries }
    }

    /// Construct a bridge from raw entries (useful for testing or manual wiring).
    /// Each tuple is `(clap_id, param_name, min, max, default)`.
    pub fn from_raw_params(raw: &[(u32, &str, f64, f64, f64)]) -> Self {
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let entries = raw
            .iter()
            .filter_map(|&(clap_id, name, min, max, default)| {
                let paraclete_id = ParamDescriptor::id_for_name(name);
                if !seen.insert(paraclete_id) {
                    return None;
                }
                Some(HostParamEntry {
                    clap_id,
                    paraclete_id,
                    name: name.to_string(),
                    min,
                    max,
                    default,
                })
            })
            .collect();
        Self { entries }
    }

    /// Paraclete param ID for the given native CLAP param ID. `None` if not found.
    pub fn paraclete_id_for(&self, clap_id: u32) -> Option<u32> {
        self.entries
            .iter()
            .find(|e| e.clap_id == clap_id)
            .map(|e| e.paraclete_id)
    }

    /// Native CLAP param ID for the given Paraclete param ID. `None` if not found.
    pub fn clap_id_for(&self, paraclete_id: u32) -> Option<u32> {
        self.entries
            .iter()
            .find(|e| e.paraclete_id == paraclete_id)
            .map(|e| e.clap_id)
    }

    /// Synthesise a `CapabilityDocument` from the bridge entries.
    /// Runtime plugin name/vendor are carried as owned `Cow<'static, str>` — no
    /// leak (U1 audit; was `Box::leak` when the fields were `&'static str`).
    pub fn to_capability_document(&self, name: &str, vendor: &str) -> CapabilityDocument {
        let params = self
            .entries
            .iter()
            .map(|e| ParamDescriptor {
                id: e.paraclete_id,
                name: PortName::Dynamic(e.name.clone()),
                min: e.min,
                max: e.max,
                default: e.default,
                stepped: false,
                unit: ParamUnit::Generic,
                display: None,
            })
            .collect();

        CapabilityDocument {
            // Runtime plugin names: owned Cow, no leak (U1 audit). Was Box::leak.
            name: name.to_string().into(),
            vendor: vendor.to_string().into(),
            version: (0, 1, 0),
            // Port id=1 matches PORT_AUDIO_OUT in PluginNode::ports().
            // Port id=0 is the events_in port declared in ports() but not in the cap doc.
            ports: vec![PortDescriptor {
                id: 1,
                name: PortName::Static("audio_out"),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            }],
            params,
            extensions: vec![],
            view: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
