// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    clap_event_header, clap_event_note, clap_event_param_value, clap_input_events,
    clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::plugin::clap_plugin;
use clap_sys::process::clap_process;

use paraclete_node_api::capability::CapabilityDocument;
use paraclete_node_api::context::{ProcessInput, ProcessOutput};
use paraclete_node_api::midi::ChannelVoice2;
use paraclete_node_api::parameter::ParameterBank;
use paraclete_node_api::port::{PortDescriptor, PortDirection, PortName, PortType};
use paraclete_node_api::{Event, Node, UmpMessage};

use crate::bridge::HostParamBridge;
use crate::library::LibraryHandle;

const PORT_EVENTS_IN:        u32 = 0;
const PORT_AUDIO_IN:         u32 = 1; // only present when has_audio_input
const PORT_AUDIO_OUT:        u32 = 1; // generator (no audio input)
const PORT_AUDIO_OUT_EFFECT: u32 = 2; // effect (has audio input)

/// Maximum number of note events translated per process() block.
const MAX_EVENTS: usize = 64;

/// Passed as `ctx` in the stack-allocated `clap_input_events` struct.
struct EventCtx {
    ptrs:  *const *const clap_event_header,
    count: usize,
}

unsafe extern "C" fn event_list_size(list: *const clap_input_events) -> u32 {
    let ctx = &*((*list).ctx as *const EventCtx);
    ctx.count as u32
}

unsafe extern "C" fn event_list_get(
    list:  *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let ctx = &*((*list).ctx as *const EventCtx);
    if index as usize >= ctx.count { return std::ptr::null(); }
    unsafe { *ctx.ptrs.add(index as usize) }
}

unsafe extern "C" fn out_events_nop(
    _list:  *const clap_output_events,
    _event: *const clap_event_header,
) -> bool { true }

/// A loaded CLAP plugin instance wrapped as a Paraclete `Node`.
///
/// Generator plugins (no audio input): ports 0=events_in, 1=audio_out.
/// Effect plugins (has audio input):   ports 0=events_in, 1=audio_in, 2=audio_out.
pub struct PluginNode {
    /// Keeps the shared library loaded as long as this node exists.
    _lib:            Arc<LibraryHandle>,
    /// Raw CLAP plugin handle.
    plugin:          *mut clap_plugin,
    cap_doc:         CapabilityDocument,
    bridge:          HostParamBridge,
    /// Cached port list returned from ports().
    port_list:       Vec<PortDescriptor>,
    /// Pre-allocated audio output buffer — block_size samples.
    audio_buf:       Vec<f32>,
    /// Pre-allocated audio input buffer — non-empty only when has_audio_input.
    audio_in_buf:    Vec<f32>,
    /// Whether this plugin has an audio input port (effect) or not (generator).
    has_audio_input: bool,
    /// Pre-allocated note event pool — cleared and filled each process() call.
    note_buf:        Vec<clap_event_note>,
    /// Pre-allocated param value event pool — one slot per bridge entry.
    param_buf:       Vec<clap_event_param_value>,
    /// Unified pointer index over param_buf then note_buf — rebuilt each call.
    ptr_idx:         Vec<*const clap_event_header>,
    /// Last values flushed to the plugin — parallel to bridge.entries.
    flushed_values:  Vec<f64>,
    /// Local parameter bank; handles CMD_SET_PARAM / CMD_BUMP_PARAM.
    local_bank:      ParameterBank,
    sample_rate:     f32,
    block_size:      usize,
    activated:       bool,
}

// SAFETY: The CLAP threading model matches Paraclete's:
//   activate/deactivate → main thread (matches Node::activate/deactivate calls)
//   process            → audio thread (matches NodeExecutor::process)
// The raw plugin pointer is never accessed concurrently.
unsafe impl Send for PluginNode {}

impl PluginNode {
    pub(crate) fn new(
        lib:             Arc<LibraryHandle>,
        plugin:          *mut clap_plugin,
        cap_doc:         CapabilityDocument,
        bridge:          HostParamBridge,
        sample_rate:     f32,
        block_size:      usize,
        has_audio_input: bool,
    ) -> Self {
        let mut port_list = vec![
            PortDescriptor {
                id:        PORT_EVENTS_IN,
                name:      PortName::Static("events_in"),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            },
        ];
        if has_audio_input {
            port_list.push(PortDescriptor {
                id:        PORT_AUDIO_IN,
                name:      PortName::Static("audio_in"),
                direction: PortDirection::Input,
                port_type: PortType::Audio,
            });
            port_list.push(PortDescriptor {
                id:        PORT_AUDIO_OUT_EFFECT,
                name:      PortName::Static("audio_out"),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            });
        } else {
            port_list.push(PortDescriptor {
                id:        PORT_AUDIO_OUT,
                name:      PortName::Static("audio_out"),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            });
        }
        Self {
            _lib:            lib,
            plugin,
            cap_doc,
            bridge,
            port_list,
            audio_buf:       Vec::new(),
            audio_in_buf:    Vec::new(),
            has_audio_input,
            note_buf:        Vec::with_capacity(MAX_EVENTS),
            param_buf:       Vec::new(),   // allocated at activate()
            ptr_idx:         Vec::with_capacity(MAX_EVENTS),
            flushed_values:  Vec::new(),   // allocated at activate()
            local_bank:      ParameterBank::empty(),
            sample_rate,
            block_size,
            activated:       false,
        }
    }

    pub fn bridge(&self) -> &HostParamBridge { &self.bridge }
}

impl Drop for PluginNode {
    fn drop(&mut self) {
        // Ensure plugin is deactivated and destroyed on the main thread (drop thread).
        // CLAP lifecycle: stop_processing → deactivate → destroy.
        if self.activated {
            if let Some(stop) = unsafe { (*self.plugin).stop_processing } {
                unsafe { stop(self.plugin); }
            }
            if let Some(deact) = unsafe { (*self.plugin).deactivate } {
                unsafe { deact(self.plugin); }
            }
        }
        if let Some(destroy) = unsafe { (*self.plugin).destroy } {
            unsafe { destroy(self.plugin); }
        }
    }
}

impl Node for PluginNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.port_list
    }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate  = sample_rate;
        self.block_size   = block_size;
        self.audio_buf    = vec![0.0f32; block_size];
        if self.has_audio_input {
            self.audio_in_buf = vec![0.0f32; block_size];
        }

        // Build bank from cap_doc; init flushed_values to plugin defaults.
        self.local_bank    = ParameterBank::from_capability_document(&self.cap_doc);
        self.flushed_values = self.bridge.entries.iter().map(|e| e.default).collect();
        // Pre-allocate param_buf (one slot per bridge param) and widen ptr_idx.
        let n_params = self.bridge.entries.len();
        self.param_buf = Vec::with_capacity(n_params.max(1));
        self.ptr_idx   = Vec::with_capacity(MAX_EVENTS + n_params);

        let ok = if let Some(act) = unsafe { (*self.plugin).activate } {
            unsafe { act(self.plugin, sample_rate as f64, 1, block_size as u32) }
        } else {
            true
        };
        if ok {
            if let Some(sp) = unsafe { (*self.plugin).start_processing } {
                unsafe { sp(self.plugin); }
            }
            self.activated = true;
        }
    }

    fn deactivate(&mut self) {
        if self.activated {
            if let Some(stop) = unsafe { (*self.plugin).stop_processing } {
                unsafe { stop(self.plugin); }
            }
            if let Some(deact) = unsafe { (*self.plugin).deactivate } {
                unsafe { deact(self.plugin); }
            }
            self.activated = false;
        }
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // ── 1. Apply commands; flush changed param values to plugin ────────
        self.local_bank.handle_commands(input.commands);

        self.note_buf.clear();
        self.param_buf.clear();
        self.ptr_idx.clear();

        // Emit CLAP_EVENT_PARAM_VALUE for each param that changed since last cycle.
        // Param events go first (time=0) so they precede any note events.
        for (i, entry) in self.bridge.entries.iter().enumerate() {
            let Some(flushed) = self.flushed_values.get_mut(i) else { continue; };
            let current = self.local_bank.get(entry.paraclete_id);
            if (current - *flushed).abs() > 1e-10 {
                *flushed = current;
                let idx = self.param_buf.len();
                // SAFETY: param_buf was pre-allocated at activate() with bridge.entries.len()
                // capacity; we push at most one entry per bridge param, so no realloc.
                self.param_buf.push(clap_event_param_value {
                    header: clap_event_header {
                        size:     std::mem::size_of::<clap_event_param_value>() as u32,
                        time:     0,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_:    CLAP_EVENT_PARAM_VALUE,
                        flags:    0,
                    },
                    param_id:   entry.clap_id,
                    cookie:     core::ptr::null_mut(),
                    note_id:    -1,
                    port_index: -1,
                    channel:    -1,
                    key:        -1,
                    value:      current,
                });
                let hdr_ptr = unsafe {
                    &(*self.param_buf.as_ptr().add(idx)).header as *const clap_event_header
                };
                self.ptr_idx.push(hdr_ptr);
            }
        }

        // ── 2. Build CLAP note event list ──────────────────────────────────
        for timed in input.events {
            if self.note_buf.len() >= MAX_EVENTS { break; }
            if let Event::Midi2(ref ump) = timed.event {
                if let Some(ev) = ump_to_note_event(timed.sample_offset, ump) {
                    let idx = self.note_buf.len();
                    self.note_buf.push(ev);
                    // SAFETY: push succeeded within pre-allocated capacity;
                    // the heap address of note_buf[idx] is stable for this call.
                    let hdr_ptr = unsafe {
                        &(*self.note_buf.as_ptr().add(idx)).header
                            as *const clap_event_header
                    };
                    self.ptr_idx.push(hdr_ptr);
                }
            }
        }

        // ── 3. Build CLAP process structs on the stack (no allocation) ─────
        let ctx = EventCtx {
            ptrs:  self.ptr_idx.as_ptr(),
            count: self.ptr_idx.len(),
        };
        let in_events = clap_input_events {
            ctx:  &ctx as *const EventCtx as *mut _,
            size: Some(event_list_size),
            get:  Some(event_list_get),
        };
        let out_events = clap_output_events {
            ctx:      std::ptr::null_mut(),
            try_push: Some(out_events_nop),
        };

        // Audio output buffer — channel pointer array built on stack.
        let frames = self.block_size;
        self.audio_buf.fill(0.0);
        let ch_ptr: *mut f32 = self.audio_buf.as_mut_ptr();
        let mut out_ch_ptrs: [*mut f32; 1] = [ch_ptr];
        let mut out_audio = clap_audio_buffer {
            data32:        out_ch_ptrs.as_mut_ptr(),
            data64:        std::ptr::null_mut(),
            channel_count: 1,
            latency:       0,
            constant_mask: 0,
        };

        // Effect path: copy graph audio input into audio_in_buf.
        // Must happen before building the clap_process struct so the buffer
        // contents are ready when the plugin reads them.
        if self.has_audio_input {
            if let Some(audio_in) = input.audio_inputs.first() {
                let copy_len = self.audio_in_buf.len().min(audio_in.channel(0).len());
                self.audio_in_buf[..copy_len].copy_from_slice(&audio_in.channel(0)[..copy_len]);
            }
        }
        // Declare audio input buffer structs here so they outlive the
        // clap_process passed to process_fn below. For generator plugins
        // (has_audio_input=false), audio_inputs is null and the plugin never
        // reads in_audio; for effect plugins the pointer is valid.
        //
        // SAFETY: cast *const→*mut required by the C API; CLAP plugins treat
        // audio_inputs as read-only (no mutation through this pointer).
        let in_ch_ptr: *const f32 = self.audio_in_buf.as_ptr();
        let mut in_ch_ptrs: [*mut f32; 1] = [in_ch_ptr as *mut f32];
        let in_audio = clap_audio_buffer {
            data32:        in_ch_ptrs.as_mut_ptr(),
            data64:        std::ptr::null_mut(),
            channel_count: 1,
            latency:       0,
            constant_mask: 0,
        };

        let proc = clap_process {
            steady_time:         -1,
            frames_count:        frames as u32,
            transport:           std::ptr::null(),
            audio_inputs:        if self.has_audio_input { &in_audio } else { std::ptr::null() },
            audio_inputs_count:  self.has_audio_input as u32,
            audio_outputs:       &mut out_audio,
            audio_outputs_count: 1,
            in_events:  &in_events,
            out_events: &out_events,
        };

        // ── 4. Call plugin.process() ───────────────────────────────────────
        if let Some(process_fn) = unsafe { (*self.plugin).process } {
            unsafe { process_fn(self.plugin, &proc); }
        }

        // ── 5. Copy mono output to graph audio output buffer ───────────────
        if !output.audio_outputs.is_empty() {
            let out = output.audio_outputs[0].channel_mut(0);
            let copy_len = out.len().min(self.audio_buf.len());
            out[..copy_len].copy_from_slice(&self.audio_buf[..copy_len]);
        }
    }

    fn capability_document(&self) -> CapabilityDocument {
        self.cap_doc.clone()
    }

    fn type_name(&self) -> &'static str { "PluginNode" }

    fn serialize(&self) -> Vec<u8> {
        // CLAP state extension deferred to P9.
        vec![]
    }

    fn deserialize(&mut self, data: &[u8]) {
        // CLAP state extension deferred to P9.
        let _ = data;
    }
}

/// Convert a UMP NoteOn/NoteOff message to a CLAP note event.
fn ump_to_note_event(offset: u32, ump: &UmpMessage) -> Option<clap_event_note> {
    if let UmpMessage::ChannelVoice2(cv2) = ump {
        match cv2 {
            ChannelVoice2::NoteOn(n) => {
                use paraclete_node_api::midi::Channeled;
                Some(clap_event_note {
                    header: clap_event_header {
                        size:     std::mem::size_of::<clap_event_note>() as u32,
                        time:     offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_:    CLAP_EVENT_NOTE_ON,
                        flags:    0,
                    },
                    note_id:    -1,
                    port_index: 0,
                    channel:    u8::from(n.channel()) as i16,
                    key:        u8::from(n.note_number()) as i16,
                    velocity:   n.velocity() as f64 / 65535.0,
                })
            }
            ChannelVoice2::NoteOff(n) => {
                use paraclete_node_api::midi::Channeled;
                Some(clap_event_note {
                    header: clap_event_header {
                        size:     std::mem::size_of::<clap_event_note>() as u32,
                        time:     offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_:    CLAP_EVENT_NOTE_OFF,
                        flags:    0,
                    },
                    note_id:    -1,
                    port_index: 0,
                    channel:    u8::from(n.channel()) as i16,
                    key:        u8::from(n.note_number()) as i16,
                    velocity:   0.0,
                })
            }
            _ => None,
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires an effect .clap binary"]
    fn plugin_node_effect_has_audio_in_port() {
        // Would load an effect plugin and assert ports() contains an Audio/Input port.
        // Skipped: no effect binary available in CI.
    }

    #[test]
    #[ignore = "requires a generator .clap binary"]
    fn plugin_node_generator_no_audio_in_port() {
        // Would load a generator and assert no Audio/Input port.
        // Skipped: no binary available in CI.
    }
}
