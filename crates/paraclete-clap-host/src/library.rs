// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap_sys::entry::clap_plugin_entry;
use clap_sys::ext::audio_ports::{clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::plugin::clap_plugin;

use paraclete_node_api::Node;

use crate::bridge::HostParamBridge;
use crate::host::HOST;
use crate::node::PluginNode;
use crate::HostError;

/// Wraps `libloading::Library` and calls `clap_entry.deinit` on drop.
pub(crate) struct LibraryHandle {
    #[allow(dead_code)] // kept alive to prevent library unload
    pub(crate) lib:    libloading::Library,
    pub(crate) deinit: Option<unsafe extern "C" fn()>,
}

unsafe impl Send for LibraryHandle {}
unsafe impl Sync for LibraryHandle {}

impl Drop for LibraryHandle {
    fn drop(&mut self) {
        if let Some(deinit) = self.deinit {
            // SAFETY: deinit must be called before unloading per CLAP spec.
            // The library is still loaded (we own it in this struct).
            unsafe { deinit(); }
        }
    }
}

/// Metadata for a single plugin in a `.clap` library.
#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    /// CLAP plugin ID string (e.g. `"audio.paraclete.machine.kick"`).
    pub id:          String,
    pub name:        String,
    pub vendor:      String,
    pub version:     String,
    pub features:    Vec<String>,
    /// Path to the `.clap` file or library this descriptor was loaded from.
    pub source_path: PathBuf,
}

/// A loaded `.clap` shared library. Holds the library open until all
/// `PluginNode` instances derived from it are dropped.
pub struct PluginLibrary {
    pub(crate) lib:         Arc<LibraryHandle>,
    pub(crate) factory:     *const clap_plugin_factory,
    pub(crate) descriptors: Vec<PluginDescriptor>,
}

// SAFETY: factory pointer is valid for the library's lifetime; library is Arc-managed.
unsafe impl Send for PluginLibrary {}
unsafe impl Sync for PluginLibrary {}

impl PluginLibrary {
    /// Load a `.clap` file (or macOS bundle directory).
    pub fn load(path: &Path) -> Result<Self, HostError> {
        let lib_path = resolve_lib_path(path)?;

        // Load the dynamic library.
        let lib = unsafe { libloading::Library::new(&lib_path) }
            .map_err(|e| HostError::Load(e.to_string()))?;

        // Find the CLAP entry point symbol.
        let entry_sym: libloading::Symbol<*const clap_plugin_entry> =
            unsafe { lib.get(b"clap_entry\0") }
                .map_err(|_| HostError::InvalidPlugin(
                    "symbol 'clap_entry' not found".into(),
                ))?;
        let entry: &clap_plugin_entry = unsafe { &**entry_sym };

        // Initialise — must be called before any other use per CLAP spec.
        let path_cstr = CString::new(lib_path.to_string_lossy().as_bytes())
            .unwrap_or_default();
        if let Some(init_fn) = entry.init {
            if !unsafe { init_fn(path_cstr.as_ptr()) } {
                return Err(HostError::InvalidPlugin("entry.init returned false".into()));
            }
        }

        let deinit = entry.deinit;

        // Get the plugin factory.
        let factory_ptr = match entry.get_factory {
            Some(f) => unsafe { f(CLAP_PLUGIN_FACTORY_ID.as_ptr()) },
            None    => std::ptr::null(),
        };
        if factory_ptr.is_null() {
            return Err(HostError::InvalidPlugin("get_factory returned null".into()));
        }
        let factory = factory_ptr as *const clap_plugin_factory;

        let lib = Arc::new(LibraryHandle { lib, deinit });

        // Enumerate plugin descriptors.
        let count = unsafe { ((*factory).get_plugin_count.unwrap())(factory) };
        let mut descriptors = Vec::new();

        for i in 0..count {
            let desc_ptr = unsafe { ((*factory).get_plugin_descriptor.unwrap())(factory, i) };
            if desc_ptr.is_null() { continue; }
            let desc = unsafe { &*desc_ptr };

            let id     = read_cstr(desc.id);
            let name   = read_cstr(desc.name);
            let vendor = read_cstr(desc.vendor);
            let ver    = read_cstr(desc.version);

            descriptors.push(PluginDescriptor {
                id,
                name,
                vendor,
                version: ver,
                features: vec![],
                source_path: lib_path.clone(),
            });
        }

        Ok(PluginLibrary { lib, factory, descriptors })
    }

    /// All plugins declared in this library.
    pub fn descriptors(&self) -> &[PluginDescriptor] {
        &self.descriptors
    }

    /// Instantiate a plugin by its CLAP ID string.
    ///
    /// Returns a `PluginNode` as `Box<dyn Node>`, ready to add to a
    /// `NodeConfigurator`. The node is not yet activated.
    pub fn instantiate(
        &self,
        plugin_id:   &str,
        sample_rate: f32,
        block_size:  usize,
    ) -> Result<Box<dyn Node>, HostError> {
        let desc = self.descriptors.iter()
            .find(|d| d.id == plugin_id)
            .ok_or_else(|| HostError::PluginNotFound {
                plugin_id: plugin_id.to_string(),
            })?;

        let id_cstr = CString::new(plugin_id).unwrap_or_default();
        let plugin: *const clap_plugin = unsafe {
            ((*self.factory).create_plugin.unwrap())(
                self.factory,
                &HOST,
                id_cstr.as_ptr(),
            )
        };
        if plugin.is_null() {
            return Err(HostError::Activate("create_plugin returned null".into()));
        }

        // Call plugin.init().
        if let Some(init_fn) = unsafe { (*plugin).init } {
            if !unsafe { init_fn(plugin) } {
                unsafe {
                    if let Some(destroy) = (*plugin).destroy { destroy(plugin); }
                }
                return Err(HostError::Activate("plugin.init returned false".into()));
            }
        }

        // Detect whether the plugin has audio input ports (effect vs generator).
        let has_audio_input = unsafe {
            if let Some(get_ext) = (*plugin).get_extension {
                let ap_ptr = get_ext(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr());
                if !ap_ptr.is_null() {
                    let ap = &*(ap_ptr as *const clap_plugin_audio_ports);
                    if let Some(count_fn) = ap.count {
                        count_fn(plugin, true) > 0 // is_input = true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        };

        // Build param bridge from the plugin's params extension.
        let bridge = unsafe { HostParamBridge::from_plugin(plugin) };
        let cap_doc = bridge.to_capability_document(&desc.name, &desc.vendor);

        Ok(Box::new(PluginNode::new(
            self.lib.clone(),
            plugin as *mut clap_plugin,
            cap_doc,
            bridge,
            sample_rate,
            block_size,
            has_audio_input,
        )))
    }
}

fn read_cstr(ptr: *const std::ffi::c_char) -> String {
    if ptr.is_null() { return String::new(); }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

/// Resolve the actual shared library path.
/// On macOS, `.clap` bundles are directories; the binary lives at
/// `Contents/MacOS/<stem>`. Plain files are used directly.
fn resolve_lib_path(path: &Path) -> Result<PathBuf, HostError> {
    if path.is_dir() {
        // macOS bundle convention
        let stem = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin");
        let binary = path.join("Contents").join("MacOS").join(stem);
        if binary.exists() {
            return Ok(binary);
        }
        // Fallback: first file in Contents/MacOS/
        let macos_dir = path.join("Contents").join("MacOS");
        if let Ok(mut entries) = std::fs::read_dir(&macos_dir) {
            if let Some(Ok(entry)) = entries.next() {
                return Ok(entry.path());
            }
        }
        Err(HostError::InvalidPlugin(format!(
            "bundle at '{}' has no binary in Contents/MacOS/",
            path.display()
        )))
    } else {
        Ok(path.to_path_buf())
    }
}
