// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared CLAP FFI boilerplate for all machine bank plugin binaries.
//!
//! Each binary calls `use paraclete_clap::ffi::*` and provides its own
//! static metadata and generator constructor. The lifecycle callbacks here
//! are identical for every machine bank plugin.

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_note, clap_event_param_value,
    CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::factory::plugin_factory::clap_plugin_factory;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::version::{clap_version, CLAP_VERSION_MAJOR, CLAP_VERSION_MINOR, CLAP_VERSION_REVISION};
use std::ffi::{c_char, c_void};

use crate::SubgraphPlugin;

pub use clap_sys::entry::clap_plugin_entry as ClapPluginEntry;

/// Wrapper to make a null-terminated `*const c_char` feature list `Sync`.
///
/// CLAP feature arrays point only to static string literals, so sharing
/// the array across threads is safe. Rust's blanket `Sync` impl does not
/// cover `*const c_char`, so we wrap and assert safety here.
#[repr(transparent)]
pub struct SyncFeatures<const N: usize>(pub [*const c_char; N]);
unsafe impl<const N: usize> Sync for SyncFeatures<N> {}

/// Convenience constant used in every binary's `clap_version` field.
pub const CLAP_VER: clap_version = clap_version {
    major:    CLAP_VERSION_MAJOR,
    minor:    CLAP_VERSION_MINOR,
    revision: CLAP_VERSION_REVISION,
};

/// Each binary implements this to provide its generator + metadata.
pub type MakePlugin = fn(sample_rate: f32, block_size: usize) -> SubgraphPlugin;

/// Heap-allocated state for a running plugin instance.
///
/// The `clap` field must be first: CLAP passes a `*const clap_plugin`
/// which is cast back to `*mut PluginWrapper`.
#[repr(C)]
pub struct PluginWrapper {
    pub clap:  clap_plugin,
    pub inner: SubgraphPlugin,
    /// Pre-allocated pool; cleared and reused each `process()` call.
    pub events_pool:   Vec<paraclete_node_api::TimedEvent>,
    /// Pre-allocated pool; cleared and reused each `process()` call.
    pub commands_pool: Vec<paraclete_node_api::NodeCommand>,
}

/// Build the `clap_plugin` vtable with all lifecycle callbacks wired up.
pub fn plugin_class() -> clap_plugin {
    clap_plugin {
        desc:              std::ptr::null(),
        plugin_data:       std::ptr::null_mut(),
        init:              Some(plugin_init),
        destroy:           Some(plugin_destroy),
        activate:          Some(plugin_activate),
        deactivate:        Some(plugin_deactivate),
        start_processing:  Some(plugin_start_processing),
        stop_processing:   Some(plugin_stop_processing),
        reset:             Some(plugin_reset),
        process:           Some(plugin_process),
        get_extension:     Some(plugin_get_extension),
        on_main_thread:    Some(plugin_on_main_thread),
    }
}

pub unsafe extern "C" fn plugin_init(_plugin: *const clap_plugin) -> bool {
    true
}

pub unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    drop(Box::from_raw(plugin as *mut PluginWrapper));
}

pub unsafe extern "C" fn plugin_activate(
    plugin:      *const clap_plugin,
    sample_rate: f64,
    _min_frames: u32,
    max_frames:  u32,
) -> bool {
    let w = &mut *(plugin as *mut PluginWrapper);
    w.inner.activate(sample_rate as f32, max_frames as usize);
    true
}

pub unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    let w = &mut *(plugin as *mut PluginWrapper);
    w.inner.deactivate();
}

pub unsafe extern "C" fn plugin_start_processing(_plugin: *const clap_plugin) -> bool {
    true
}

pub unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

pub unsafe extern "C" fn plugin_reset(_plugin: *const clap_plugin) {}

pub unsafe extern "C" fn plugin_process(
    plugin:  *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let w = &mut *(plugin as *mut PluginWrapper);
    let p = &*process;

    let (transport_info, transport_event) = if !p.transport.is_null() {
        let t = &*p.transport;
        crate::transport::translate_transport(
            t.flags,
            t.tempo,
            t.song_pos_beats,
            w.inner.prev_playing(),
        )
    } else {
        (paraclete_node_api::TransportInfo::default(), None)
    };

    w.events_pool.clear();
    w.commands_pool.clear();
    let external_events = &mut w.events_pool;
    let commands = &mut w.commands_pool;

    if !p.in_events.is_null() {
        let in_ev = &*p.in_events;
        let count = (in_ev.size.unwrap())(p.in_events);
        for i in 0..count {
            let raw = (in_ev.get.unwrap())(p.in_events, i);
            if raw.is_null() { continue; }
            let hdr = &*(raw as *const clap_event_header);
            match hdr.type_ {
                CLAP_EVENT_NOTE_ON => {
                    let ev = &*(raw as *const clap_event_note);
                    use paraclete_node_api::{Event, TimedEvent, UmpMessage};
                    use paraclete_node_api::midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7};
                    let mut msg = NoteOn::<[u32; 4]>::new();
                    msg.set_group(u4::new(0));
                    msg.set_channel(u4::new((ev.channel as u8) & 0xF));
                    msg.set_note_number(u7::new((ev.key as u8) & 0x7F));
                    msg.set_velocity((ev.velocity * 65535.0) as u16);
                    let ump = UmpMessage::from(ChannelVoice2::from(msg));
                    external_events.push(TimedEvent::new(
                        hdr.time,
                        Event::Midi2(ump),
                    ));
                }
                CLAP_EVENT_NOTE_OFF => {
                    let ev = &*(raw as *const clap_event_note);
                    use paraclete_node_api::{Event, TimedEvent, UmpMessage};
                    use paraclete_node_api::midi::{ChannelVoice2, Channeled, Grouped, NoteOff, u4, u7};
                    let mut msg = NoteOff::<[u32; 4]>::new();
                    msg.set_group(u4::new(0));
                    msg.set_channel(u4::new((ev.channel as u8) & 0xF));
                    msg.set_note_number(u7::new((ev.key as u8) & 0x7F));
                    let ump = UmpMessage::from(ChannelVoice2::from(msg));
                    external_events.push(TimedEvent::new(
                        hdr.time,
                        Event::Midi2(ump),
                    ));
                }
                CLAP_EVENT_PARAM_VALUE => {
                    let ev = &*(raw as *const clap_event_param_value);
                    if let Some(cmd) = w.inner.bridge()
                        .make_set_param_command(ev.param_id, ev.value, w.inner.gen_id())
                    {
                        commands.push(cmd);
                    }
                }
                _ => {}
            }
        }
    }

    let audio = w.inner.process_block(
        &transport_info,
        transport_event.as_ref(),
        &external_events,
        &commands,
    );

    if !p.audio_outputs.is_null() && p.audio_outputs_count > 0 {
        let out_buf = &*p.audio_outputs;
        if out_buf.channel_count > 0 && !out_buf.data32.is_null() {
            let ch0 = *out_buf.data32;
            if !ch0.is_null() {
                let frames = p.frames_count as usize;
                let out_slice = std::slice::from_raw_parts_mut(ch0, frames);
                for (o, &s) in out_slice.iter_mut().zip(audio.iter()) {
                    *o = s;
                }
                if out_buf.channel_count > 1 {
                    let ch1 = *out_buf.data32.add(1);
                    if !ch1.is_null() {
                        let out1 = std::slice::from_raw_parts_mut(ch1, frames);
                        out1.copy_from_slice(&audio[..frames]);
                    }
                }
            }
        }
    }

    CLAP_PROCESS_CONTINUE
}

pub unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    _id: *const c_char,
) -> *const c_void {
    std::ptr::null()
}

pub unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

/// Returns the plugin factory pointer as `*const c_void` for use in `get_factory`.
pub fn make_factory_ptr(factory: &'static clap_plugin_factory) -> *const c_void {
    factory as *const clap_plugin_factory as *const c_void
}

/// Build a `clap_plugin_entry` with all function pointers set.
///
/// Callers provide their own `entry_init`, `entry_deinit`, and `entry_get_factory`
/// functions. This helper exists so tests can verify the pattern.
pub fn make_entry(
    init:        unsafe extern "C" fn(*const c_char) -> bool,
    deinit:      unsafe extern "C" fn(),
    get_factory: unsafe extern "C" fn(*const c_char) -> *const c_void,
) -> clap_plugin_entry {
    clap_plugin_entry {
        clap_version: CLAP_VER,
        init:         Some(init),
        deinit:       Some(deinit),
        get_factory:  Some(get_factory),
    }
}
