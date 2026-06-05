// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete Snare Machine — CLAP instrument (AnalogEngine::snare + Sequencer).
//!
//! Build with: cargo build -p paraclete-machine-snare
//! Output: target/debug/libparaclete_machine_snare.dylib (rename to .clap)

use paraclete_clap::{SubgraphPlugin, ffi::*};
use paraclete_nodes::AnalogEngine;
use clap_sys::factory::plugin_factory::clap_plugin_factory;
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use std::ffi::{c_char, c_void, CStr};

static PLUGIN_ID:   &[u8] = b"audio.paraclete.machine.snare\0";
static PLUGIN_NAME: &[u8] = b"Paraclete Snare\0";
static VENDOR:      &[u8] = b"Paraclete Audio\0";
static VERSION:     &[u8] = b"0.1.0\0";
static DESC:        &[u8] = b"Analog snare drum with step sequencer\0";

static FEATURES: SyncFeatures<3> = SyncFeatures([
    clap_sys::plugin_features::CLAP_PLUGIN_FEATURE_INSTRUMENT.as_ptr() as *const c_char,
    clap_sys::plugin_features::CLAP_PLUGIN_FEATURE_DRUM.as_ptr()       as *const c_char,
    std::ptr::null(),
]);

static PLUGIN_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VER,
    id:          PLUGIN_ID.as_ptr()   as *const c_char,
    name:        PLUGIN_NAME.as_ptr() as *const c_char,
    vendor:      VENDOR.as_ptr()      as *const c_char,
    url:         b"\0".as_ptr()       as *const c_char,
    manual_url:  b"\0".as_ptr()       as *const c_char,
    support_url: b"\0".as_ptr()       as *const c_char,
    version:     VERSION.as_ptr()     as *const c_char,
    description: DESC.as_ptr()        as *const c_char,
    features:    FEATURES.0.as_ptr(),
};

unsafe extern "C" fn factory_get_plugin_count(
    _factory: *const clap_plugin_factory,
) -> u32 { 1 }

unsafe extern "C" fn factory_get_plugin_descriptor(
    _factory: *const clap_plugin_factory,
    _index: u32,
) -> *const clap_plugin_descriptor { &PLUGIN_DESC }

unsafe extern "C" fn factory_create_plugin(
    _factory:  *const clap_plugin_factory,
    _host:     *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    if CStr::from_ptr(plugin_id) != CStr::from_ptr(PLUGIN_ID.as_ptr() as *const c_char) {
        return std::ptr::null();
    }
    let inner = SubgraphPlugin::new(Box::new(AnalogEngine::snare()), 3, 48000.0, 512);
    let mut wrapper = Box::new(PluginWrapper {
        clap:          plugin_class(),
        inner,
        events_pool:   Vec::with_capacity(64),
        commands_pool: Vec::with_capacity(16),
    });
    wrapper.clap.desc        = &PLUGIN_DESC;
    wrapper.clap.plugin_data = std::ptr::null_mut();
    Box::into_raw(wrapper) as *const clap_plugin
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count:      Some(factory_get_plugin_count),
    get_plugin_descriptor: Some(factory_get_plugin_descriptor),
    create_plugin:         Some(factory_create_plugin),
};

unsafe extern "C" fn entry_init(_plugin_path: *const c_char) -> bool { true }
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    use clap_sys::factory::plugin_factory::CLAP_PLUGIN_FACTORY_ID;
    if CStr::from_ptr(factory_id) == CLAP_PLUGIN_FACTORY_ID {
        make_factory_ptr(&FACTORY)
    } else {
        std::ptr::null()
    }
}

#[no_mangle]
pub static clap_entry: clap_sys::entry::clap_plugin_entry = clap_sys::entry::clap_plugin_entry {
    clap_version: CLAP_VER,
    init:         Some(entry_init),
    deinit:       Some(entry_deinit),
    get_factory:  Some(entry_get_factory),
};
