# Paraclete — P8 Interface Specification

> **Implementation blueprint.** These are the contracts P8 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
>
> **P8 deliverable:** P7 residuals resolved (gate). Instrument definition
> file (YAML). Terminal UI (`paraclete-tui`). `publish_context()` Rhai API.
> CLAP Host (`paraclete-clap-host`) — third-party `.clap` plugins as nodes.
> **Last updated:** June 2026  
> **Baseline:** P7 complete — 317 tests, 0 failures  
> **Depends on:** p0–p7 interfaces and reports — all prior types assumed
> present and correct.  
> **References:** ADR-026 (instrument definition + TUI), ADR-027 (CLAP host),
> ADR-024 (CLAP wrapper — machine bank FFI), ADR-025 (project file format),
> ADR-007 (scripting runtime), ADR-019 (universal parameter control),
> ADR-001 (license)

-----

## How to use this document

**Commit 1 is gated.** No new P8 work begins until Commit 1 passes
`cargo test --workspace` with zero failures. Commit 1 resolves all P7
deferrals. It has no new features; it is pure hardening.

**Implement in commit order.** Each commit is independently testable.
Commit 2 (`publish_context()`) depends on the scripting layer being stable
after Commit 1. Commit 3 (instrument definition) is a prerequisite for
Commit 6 (app wiring) which assembles all P8 deliverables together.
Commit 5 (CLAP Host) depends on ADR-027; the spec in this document is the
contract.

**Format decision for instrument definition:** YAML. Reasoning: YAML is
human-writable and human-readable without Rust knowledge, which is the
primary requirement in ADR-026 ("readable and writable without knowledge
of Rust"). TOML was considered but is awkward for arrays of heterogeneous
structs (each node entry has a different optional fields set). RON (the
project file format per ADR-025) is the correct choice for machine-written
runtime state but would surprise a non-Rust operator editing a config file.
YAML is the right fit. The `serde_yaml` crate provides serde integration.
The instrument file and the project file remain distinct files with distinct
formats and purposes (ADR-025 boundary still holds).

**Portability rule applies throughout.** `cargo tree -p paraclete-nodes`
must show no dependency on `paraclete-runtime` or `paraclete-scripting`
before merge for any commit touching `paraclete-nodes`.

**Parameter names are long-term contracts.** No new parameter name is
introduced in P8 without first checking the ADR-019 canonical names table.
New parameters for `PluginNode` are named by the plugin author, not by
Paraclete. This spec introduces no new first-party parameter names.

**`paraclete-tui` and `paraclete-clap-host` are GPL3.** They touch GPL3
platform code. See ADR-001. `paraclete-node-api` remains LGPL3.

| Commit | Crate(s) | Deliverable |
|--------|----------|-------------|
| 1 | `paraclete-node-api`, `paraclete-nodes`, `paraclete-clap` | P7 residuals: InternalClock stop response, machine bank real CLAP FFI, SILENCE fix |
| 2 | `paraclete-scripting` | `publish_context()` Rhai API |
| 3 | `paraclete-app` | Instrument definition file — YAML parser, graph builder |
| 4 | `paraclete-tui` (new) | Terminal UI — transport bar, encoder row, step row |
| 5 | `paraclete-clap-host` (new) | CLAP Host — `PluginLibrary`, `PluginNode` |
| 6 | `paraclete-app` | App wiring — TUI + instrument file + CLAP host integration |

-----

# Part 1: P7 Residuals (Commit 1 — GATED)

**Crates:** `paraclete-node-api`, `paraclete-nodes`, `paraclete-clap`

Commit 1 resolves three deferred items from P7. **No P8 feature work begins
until `cargo test --workspace` passes after this commit.** The three items
are isolated; they may be implemented in any order within the commit.

-----

## 1.1 InternalClock Transport Response (Finding 4)

**File:** `crates/paraclete-nodes/src/internal_clock.rs`

**Problem (P7 Finding 4):** `InternalClock` initialises `playing = true` and
ignores `TransportEvent` variants arriving on its incoming event slot.
In a `SubgraphPlugin` context, `SubgraphPlugin::process_block()` calls
`executor.inject_event_for_node(clock_id, global_stop_event)` when the DAW
transport stops. The executor delivers this event to `InternalClock`'s
incoming slot, but `InternalClock::process()` never reads it. The clock
continues ticking while the `Sequencer` (which does read transport events)
has stopped. When the DAW resumes, a second `global_start` is emitted by
`InternalClock::first_tick`; this double-start is currently idempotent but
is architecturally incorrect.

**Fix:** At the top of `InternalClock::process()`, before the tick logic,
scan `input.events()` for `Event::Transport(...)` variants:

```rust
// In InternalClock::process(), before tick logic:
for event in input.events() {
    match event {
        Event::Transport(TransportEvent::GlobalStop) => {
            self.playing = false;
        }
        Event::Transport(TransportEvent::GlobalStart) if !self.playing => {
            self.playing = true;
            self.first_tick = true;  // re-arm GlobalStart emission on next tick
        }
        _ => {}
    }
}
```

The `if !self.playing` guard on `GlobalStart` prevents `first_tick` from
being re-armed when the clock is already playing (the standalone startup
case where `InternalClock` emits its own `GlobalStart` on the first tick).

**Impact:** No change to standalone operation. In a `SubgraphPlugin`
context, `InternalClock` now stops ticking when the DAW transport stops.
Existing `subgraph_plugin_*` tests must still pass unchanged.

**Tests to add** (in `crates/paraclete-nodes/tests/`):

```
internal_clock_stops_on_global_stop_event
    Construct InternalClock + a single-slot NodeExecutor.
    Run 1 block (clock starts playing=true; emits clock ticks).
    Inject TransportEvent::GlobalStop via inject_event_for_node(clock_id, ...).
    Run 1 block.
    Assert: zero TransportEvent::Tick events in output for the second block.

internal_clock_resumes_after_global_stop_then_global_start
    Inject GlobalStop. Run 1 block (no ticks). Assert silent.
    Inject GlobalStart. Run enough blocks to cover at least one tick period.
    Assert: clock ticks are emitted after the start injection.
```

-----

## 1.2 Machine Bank Real CLAP FFI

**Crate:** `paraclete-clap`

**Problem (P7 stub binaries):** `kick.rs`, `snare.rs`, `fm_kick.rs`,
`fm_bell.rs`, `fm_bass.rs` are stub binary targets. Each must become a
working `.clap` cdylib. The `SubgraphPlugin` safe API from P7 is correct;
this commit adds the thin `unsafe extern "C"` wrapper layer.

### Cargo changes

In `crates/paraclete-clap/Cargo.toml`:

```toml
[dependencies]
clap-sys = { version = "0.3", features = [] }
# ... existing deps unchanged ...

[[bin]]
name = "kick"
crate-type = ["cdylib"]
# ... repeat for snare, fm_kick, fm_bell, fm_bass
```

The `clap-sys` crate version should be whatever is current on crates.io
at implementation time; `0.3.x` is the expected range as of June 2026.

### New module: `crates/paraclete-clap/src/ffi.rs`

Shared boilerplate for all machine bank plugin binaries. The entry point,
factory, and lifecycle callbacks are identical across all five plugins
except for the generator constructor call. All five binaries `use
paraclete_clap::ffi::*` and supply one function to the shared layer.

```rust
// crates/paraclete-clap/src/ffi.rs

use clap_sys::*;
use std::ffi::{c_char, c_void};
use crate::SubgraphPlugin;

/// Each binary target implements this to provide its generator.
pub type MakePlugin = fn(sample_rate: f32, block_size: usize) -> SubgraphPlugin;

/// Heap-allocated state for a running CLAP plugin instance.
/// The `clap` field must be first; CLAP passes a `*const clap_plugin`
/// which is cast back to `*mut PluginWrapper`.
#[repr(C)]
pub struct PluginWrapper {
    pub clap:  clap_plugin,
    pub inner: SubgraphPlugin,
}

/// Build the `clap_plugin_class` vtable. Called once per plugin type.
pub fn plugin_class() -> clap_plugin {
    clap_plugin {
        desc:           std::ptr::null(), // filled per-binary
        plugin_data:    std::ptr::null_mut(),
        init:           Some(plugin_init),
        destroy:        Some(plugin_destroy),
        activate:       Some(plugin_activate),
        deactivate:     Some(plugin_deactivate),
        start_processing: Some(plugin_start_processing),
        stop_processing:  Some(plugin_stop_processing),
        reset:          Some(plugin_reset),
        process:        Some(plugin_process),
        get_extension:  Some(plugin_get_extension),
        on_main_thread: Some(plugin_on_main_thread),
    }
}

pub unsafe extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    // No-op: SubgraphPlugin is fully constructed before init is called.
    let _ = plugin;
    true
}

pub unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    // Reconstruct the Box<PluginWrapper> and drop it.
    drop(Box::from_raw(plugin as *mut PluginWrapper));
}

pub unsafe extern "C" fn plugin_activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    _min_frames: u32,
    max_frames: u32,
) -> bool {
    let w = &mut *(plugin as *mut PluginWrapper);
    w.inner.activate(sample_rate as f32, max_frames as usize);
    true
}

pub unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    let w = &mut *(plugin as *mut PluginWrapper);
    w.inner.deactivate();
}

pub unsafe extern "C" fn plugin_start_processing(
    _plugin: *const clap_plugin,
) -> bool { true }

pub unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

pub unsafe extern "C" fn plugin_reset(_plugin: *const clap_plugin) {}

pub unsafe extern "C" fn plugin_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let w   = &mut *(plugin as *mut PluginWrapper);
    let p   = &*process;

    // Extract transport info from the CLAP process struct.
    let (transport_info, transport_event) = if !p.transport.is_null() {
        let t = &*p.transport;
        crate::transport::translate_transport(
            t.flags,
            t.tempo,
            t.song_pos_beats,
            w.inner.prev_playing(),
        )
    } else {
        use paraclete_node_api::{TransportInfo, TICKS_PER_BEAT};
        (TransportInfo {
            bpm: 120.0,
            ticks_per_beat: TICKS_PER_BEAT,
            current_tick: 0,
            block_size: p.frames_count as usize,
            sample_rate: w.inner.sample_rate(),
            playing: false,
        }, None)
    };

    // Collect CLAP input events (NoteOn, NoteOff, ParamValue).
    let mut external_events: Vec<paraclete_node_api::TimedEvent> = Vec::new();
    let mut commands: Vec<paraclete_node_api::NodeCommand> = Vec::new();
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
                    external_events.push(paraclete_node_api::TimedEvent {
                        sample_offset: hdr.time,
                        event: paraclete_node_api::Event::NoteOn {
                            channel: ev.channel as u8,
                            note:    ev.key    as u8,
                            velocity: (ev.velocity * 127.0) as u8,
                        },
                    });
                }
                CLAP_EVENT_NOTE_OFF => {
                    let ev = &*(raw as *const clap_event_note);
                    external_events.push(paraclete_node_api::TimedEvent {
                        sample_offset: hdr.time,
                        event: paraclete_node_api::Event::NoteOff {
                            channel: ev.channel as u8,
                            note:    ev.key    as u8,
                        },
                    });
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

    // Drive the subgraph.
    let audio = w.inner.process_block(
        transport_info,
        transport_event.as_ref(),
        &external_events,
        &commands,
    );

    // Write audio to CLAP output buffer (stereo interleaved or first channel).
    if !p.audio_outputs.is_null() && p.audio_outputs_count > 0 {
        let out_buf = &*p.audio_outputs;
        if out_buf.channel_count > 0 && !out_buf.data32.is_null() {
            let ch0 = *out_buf.data32;
            if !ch0.is_null() {
                let out_slice = std::slice::from_raw_parts_mut(
                    ch0, p.frames_count as usize
                );
                for (o, &s) in out_slice.iter_mut().zip(audio.iter()) {
                    *o = s;
                }
                // Duplicate to channel 1 if stereo.
                if out_buf.channel_count > 1 {
                    let ch1 = *out_buf.data32.add(1);
                    if !ch1.is_null() {
                        let out1 = std::slice::from_raw_parts_mut(
                            ch1, p.frames_count as usize
                        );
                        out1.copy_from_slice(&out_slice[..p.frames_count as usize]);
                    }
                }
            }
        }
    }

    CLAP_PROCESS_CONTINUE
}

pub unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    id: *const c_char,
) -> *const c_void {
    // No extensions at P8. State extension (save/load) deferred.
    let _ = id;
    std::ptr::null()
}

pub unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

/// CLAP factory helper — returns the plugin factory pointer as *const c_void.
pub fn make_factory_ptr(factory: &'static clap_plugin_factory) -> *const c_void {
    factory as *const clap_plugin_factory as *const c_void
}
```

### Binary template (`kick.rs`)

Each of the five binaries follows this template. Only the constant strings,
the generator constructor, and the machine variant name differ.

```rust
// crates/paraclete-clap/src/bin/kick.rs
use paraclete_clap::{SubgraphPlugin, ffi::*};
use paraclete_nodes::AnalogEngine;
use clap_sys::*;
use std::ffi::{c_char, c_void};

const CLAP_VERSION: clap_version = clap_version {
    major: CLAP_VERSION_MAJOR,
    minor: CLAP_VERSION_MINOR,
    revision: CLAP_VERSION_REVISION,
};

static PLUGIN_ID:   &[u8] = b"audio.paraclete.machine.kick\0";
static PLUGIN_NAME: &[u8] = b"Paraclete Kick\0";
static VENDOR:      &[u8] = b"Paraclete Audio\0";
static VERSION:     &[u8] = b"0.1.0\0";
static DESC:        &[u8] = b"Analog kick drum with step sequencer\0";

// Feature tags: CLAP_PLUGIN_FEATURE_INSTRUMENT + CLAP_PLUGIN_FEATURE_DRUM
static FEATURES: [*const c_char; 3] = [
    CLAP_PLUGIN_FEATURE_INSTRUMENT.as_ptr() as *const c_char,
    CLAP_PLUGIN_FEATURE_DRUM.as_ptr()       as *const c_char,
    std::ptr::null(),
];

static PLUGIN_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id:          PLUGIN_ID.as_ptr()   as *const c_char,
    name:        PLUGIN_NAME.as_ptr() as *const c_char,
    vendor:      VENDOR.as_ptr()      as *const c_char,
    url:         b"\0".as_ptr()       as *const c_char,
    manual_url:  b"\0".as_ptr()       as *const c_char,
    support_url: b"\0".as_ptr()       as *const c_char,
    version:     VERSION.as_ptr()     as *const c_char,
    description: DESC.as_ptr()        as *const c_char,
    features:    FEATURES.as_ptr(),
};

// ---- Factory ----

unsafe extern "C" fn factory_get_plugin_count(
    _factory: *const clap_plugin_factory,
) -> u32 { 1 }

unsafe extern "C" fn factory_get_plugin_descriptor(
    _factory: *const clap_plugin_factory,
    _index: u32,
) -> *const clap_plugin_descriptor { &PLUGIN_DESC }

unsafe extern "C" fn factory_create_plugin(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    use std::ffi::CStr;
    if CStr::from_ptr(plugin_id) != CStr::from_ptr(PLUGIN_ID.as_ptr() as *const c_char) {
        return std::ptr::null();
    }
    let _ = host;  // host callbacks are no-ops at P8
    // Temporary sample_rate/block_size; real values set in activate().
    let inner = SubgraphPlugin::new(
        Box::new(AnalogEngine::kick()),
        /* gen_id = */ 3,
        /* sample_rate = */ 48000.0,
        /* block_size = */ 512,
    );
    let mut wrapper = Box::new(PluginWrapper {
        clap: plugin_class(),
        inner,
    });
    wrapper.clap.desc         = &PLUGIN_DESC;
    wrapper.clap.plugin_data  = std::ptr::null_mut();
    Box::into_raw(wrapper) as *const clap_plugin
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count:      Some(factory_get_plugin_count),
    get_plugin_descriptor: Some(factory_get_plugin_descriptor),
    create_plugin:         Some(factory_create_plugin),
};

// ---- Entry point ----

unsafe extern "C" fn entry_init(_plugin_path: *const c_char) -> bool { true }
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    use std::ffi::CStr;
    if CStr::from_ptr(factory_id) == CStr::from_ptr(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char) {
        make_factory_ptr(&FACTORY)
    } else {
        std::ptr::null()
    }
}

#[no_mangle]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init:         Some(entry_init),
    deinit:       Some(entry_deinit),
    get_factory:  Some(entry_get_factory),
};
```

The other four binaries (`snare.rs`, `fm_kick.rs`, `fm_bell.rs`,
`fm_bass.rs`) are identical except:

| File | Generator call | PLUGIN_ID suffix | PLUGIN_NAME |
|------|----------------|-----------------|-------------|
| snare | `AnalogEngine::snare()` | `.snare` | `Paraclete Snare` |
| fm_kick | `FmEngine::fm_kick()` | `.fm_kick` | `Paraclete FM Kick` |
| fm_bell | `FmEngine::fm_bell()` | `.fm_bell` | `Paraclete FM Bell` |
| fm_bass | `FmEngine::fm_bass()` | `.fm_bass` | `Paraclete FM Bass` |

### `SubgraphPlugin` additions needed

`plugin_process()` calls `w.inner.prev_playing()`, `w.inner.sample_rate()`,
and `w.inner.gen_id()`. These accessors already exist per the P7 spec
(§6 SubgraphPlugin safe API). Confirm they are public. If not, add them
without changing any existing behaviour.

`SubgraphPlugin::process_block()` signature must accept the transport event
as an `Option<&TransportEvent>` (already the P7 spec). Confirm the
signature; no change needed if already correct.

**Test (counted):**

```
machine_bank_ffi_entry_not_null
    Confirm that `clap_entry.init`, `clap_entry.deinit`, and
    `clap_entry.get_factory` are all non-null.
    Proves the static is populated correctly and the binary links.
    Add to crates/paraclete-clap/tests/infrastructure_tests.rs.
```

**Tests (not counted — tagged `#[ignore]`, run via CI step):**

For each binary, after the binary builds:
```
clap_validator_kick_passes
clap_validator_snare_passes
clap_validator_fm_kick_passes
clap_validator_fm_bell_passes
clap_validator_fm_bass_passes
```

These use the `clap-validator` tool (already a dev-dependency per P7
Commit 5). Run:
```bash
cargo test --test clap_validator -- --ignored
```

-----

## 1.3 SILENCE Array Truncation Fix

**File:** `crates/paraclete-node-api/src/context.rs`

**Problem (P7 Commit 7 code review, deferred):** The `SILENCE` buffer is a
static array sized 4096. `context.rs` produces slices of it of length
`block_size`. If `block_size > 4096`, the slice access panics in debug and
silently truncates in release (the compiler may elide the bounds check on
`&SILENCE[..block_size]` if LLVM is confident the length is less). With
`block_size` currently hardcoded to 512 this is latent; with variable block
sizes imminent (CLAP plugins receive variable `max_frames_count`), this
must be fixed first.

**Fix:**

```rust
// Before:
static SILENCE: [f32; 4096] = [0.0_f32; 4096];

// After — 65536 covers all realistic DAW block sizes (typical max: 8192):
static SILENCE: [f32; 65536] = [0.0_f32; 65536];
```

Where the `SILENCE` slice is returned to callers, add an explicit
bounds assertion that fires in both debug and release:

```rust
// Wherever &SILENCE[..block_size] appears:
assert!(
    block_size <= SILENCE.len(),
    "block_size {} exceeds SILENCE buffer capacity {}",
    block_size,
    SILENCE.len(),
);
&SILENCE[..block_size]
```

Using `assert!` (not `debug_assert!`) is correct here: if a caller passes
a block_size that exceeds the buffer, this is a programming error that
would produce silent audio corruption in release builds. The assert fires
early with a useful message rather than producing wrong audio.

The 65536-frame size costs 256 KB of BSS (zeroed read-only data) — well
within acceptable binary size for a desktop audio platform.

**Test:**

```
silence_buffer_at_max_block_size_is_all_zeros
    Retrieve a silence slice of length 65536.
    Assert len == 65536 and all elements == 0.0.
    This confirms the array size and that no bounds check fires at the
    documented maximum.
```

-----

# Part 2: `publish_context()` Rhai API (Commit 2)

**Crate:** `paraclete-scripting`

**Reference:** ADR-026 §Profile Script Context Publication

Profile scripts call `publish_context()` when they assign or reassign an
encoder mapping. The TUI (Commit 4) subscribes to the resulting state bus
paths to label the encoder row.

-----

## 2.1 State Bus Paths

`publish_context(encoder_key, node_id, param_name)` writes two values to
the state bus:

| Path | Value | Type |
|------|-------|------|
| `/context/{encoder_key}/node` | `node_id` | `f64` |
| `/context/{encoder_key}/param` | `id_for_name(param_name)` as `f64` | `f64` |

`encoder_key` is a free-form string the profile script chooses
(e.g. `"encoder_0"`, `"enc_3"`). The TUI uses the pair `(node_id,
param_id_hash)` to look up the display name from the node's
`CapabilityDocument`.

**Why encode `param_name` as a hash rather than a string:** The state bus
stores `(String, f64)` pairs. Encoding the hash avoids requiring a new
variant in `StateBusValue`; the TUI resolves the display name from the
`CapabilityDocument`. If a future phase introduces a string-valued state bus
path, `publish_context()` can be updated to write the name string directly
with no Rhai API change.

-----

## 2.2 Rhai builtin signature

```rhai
// In a profile script:
publish_context("encoder_0", node_id, "decay");
// Writes:
//   /context/encoder_0/node  → node_id  (f64)
//   /context/encoder_0/param → id_for_name("decay")  (f64)
```

The three arguments are:
- `encoder_key: String` — identifies the encoder slot; matches the display
  position in the TUI encoder row
- `node_id: i64` — the Paraclete node ID this encoder now controls
- `param_name: String` — the parameter name; must be a name declared in
  the target node's `CapabilityDocument`

All three are required. Calling `publish_context` with wrong argument types
raises a Rhai runtime error (standard Rhai behaviour for mismatched argument
types).

-----

## 2.3 Implementation in `paraclete-scripting/src/builtins.rs`

```rust
// Register in ScriptingEngine::register_builtins() alongside existing builtins:

let state_bus_clone = state_bus.clone();
engine.register_fn(
    "publish_context",
    move |encoder_key: String, node_id: i64, param_name: String| {
        use paraclete_node_api::ParamDescriptor;
        let param_hash = ParamDescriptor::id_for_name(&param_name) as f64;
        let node_path  = format!("/context/{}/node",  encoder_key);
        let param_path = format!("/context/{}/param", encoder_key);
        state_bus_clone.borrow_mut().write(&node_path,  node_id as f64);
        state_bus_clone.borrow_mut().write(&param_path, param_hash);
    },
);
```

`ParamDescriptor::id_for_name()` is already a `pub const fn` (promoted in
P7 Commit 7). Calling it at runtime in a script builtin is correct — the
const-fn promotion means no allocation.

The `state_bus.borrow_mut().write()` pattern follows the existing
`state_write()` builtin in `builtins.rs`. No new patterns introduced.

-----

## 2.4 Profile script update

The existing `profiles/digitakt.rhai` (or equivalent) already assigns
encoder mappings via `send_cmd`. Update it to also call `publish_context`
at each assignment point. Example:

```rhai
// Before (P4/P6 pattern):
if enc_id == 0 { send_cmd(TRACK_DIST_IDS[track], CMD_BUMP_PARAM, id_for_name("drive"), delta); }

// After (P8 pattern — add publish_context alongside send_cmd):
if enc_id == 0 {
    publish_context(`encoder_${enc_id}`, TRACK_DIST_IDS[track], "drive");
    send_cmd(TRACK_DIST_IDS[track], CMD_BUMP_PARAM, id_for_name("drive"), delta);
}
```

`publish_context` is called any time the mapping changes, not every time the
encoder moves. Mapping changes occur on: track selection, page switch, mode
change. The call is idempotent if the mapping is the same as last time.

-----

## 2.5 Tests

```
publish_context_writes_node_and_param_to_state_bus
    Create a ScriptingEngine bound to a test StateBusHandle.
    Evaluate: publish_context("encoder_0", 42, "decay")
    Assert state_bus.read("/context/encoder_0/node")  == Some(42.0)
    Assert state_bus.read("/context/encoder_0/param") == Some(id_for_name("decay") as f64)

publish_context_overwrites_previous_mapping
    Evaluate: publish_context("encoder_0", 42, "decay")
    Evaluate: publish_context("encoder_0", 99, "cutoff")
    Assert /context/encoder_0/node  == Some(99.0)
    Assert /context/encoder_0/param == Some(id_for_name("cutoff") as f64)

publish_context_different_keys_do_not_collide
    publish_context("encoder_0", 10, "decay")
    publish_context("encoder_1", 20, "cutoff")
    Assert encoder_0/node == 10.0  and  encoder_1/node == 20.0
```

-----

# Part 3: Instrument Definition File (Commit 3)

**Crate:** `paraclete-app`

**Reference:** ADR-026 §Declarative Instrument Definition

The instrument definition file is a YAML document. It is the thing an
operator edits to change their instrument topology, BPM, initial parameters,
active profile scripts, and macro bindings. It is distinct from the RON
project file (ADR-025): the instrument file is topology and initial
configuration; the project file is runtime state deltas on top.

-----

## 3.1 YAML Schema

```yaml
# instrument.yaml
format_version: 1
name: "dawless-set-01"
bpm: 140.0

nodes:
  - id: 1
    type: internal_clock

  - id: 10
    type: sequencer
    display_name: "Kick"

  - id: 11
    type: sampler
    initial_params:
      root_note: 60.0

  - id: 12
    type: distortion

  - id: 13
    type: filter

  - id: 50
    type: mix
    channel_count: 8

  - id: 60
    type: audio_output

  # CLAP plugin node:
  - id: 70
    type: clap_plugin
    display_name: "Vendor Kick"
    plugin_id: "com.vendor.kick"
    # Optional: explicit path bypasses scan
    # plugin_path: "/usr/lib/clap/vendor.clap"

edges:
  - from: [1, "clock_out"]
    to:   [10, "clock_in"]
  - from: [10, "events_out"]
    to:   [11, "events_in"]
  - from: [11, "audio_out"]
    to:   [12, "audio_in"]
  - from: [12, "audio_out"]
    to:   [13, "audio_in"]
  - from: [13, "audio_out"]
    to:   [50, 0]

macros:
  - encoder: 0
    node: 12
    param: "drive"
  - encoder: 1
    node: 13
    param: "cutoff"
  - encoder: 2
    node: 13
    param: "resonance"

profiles:
  - "profiles/launchpad.rhai"
  - "profiles/digitakt.rhai"
  - "profiles/keystep.rhai"
```

Port names are case-insensitive. Numeric port references are also accepted:
`from: [10, 0]`. The string names are preferred for readability.

-----

## 3.2 Rust Types

**New file:** `crates/paraclete-app/src/instrument.rs`

```rust
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug)]
pub struct InstrumentDefinition {
    pub format_version: u32,
    pub name:           String,
    pub bpm:            f64,
    pub nodes:          Vec<NodeDef>,
    pub edges:          Vec<EdgeDef>,
    #[serde(default)]
    pub macros:         Vec<MacroDef>,
    #[serde(default)]
    pub profiles:       Vec<String>,
}

#[derive(Deserialize, Debug)]
pub struct NodeDef {
    pub id:           u32,
    #[serde(rename = "type")]
    pub type_tag:     String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub initial_params: HashMap<String, f64>,
    // For clap_plugin type:
    pub plugin_id:    Option<String>,
    pub plugin_path:  Option<String>,
    // For mix type:
    pub channel_count: Option<usize>,
}

#[derive(Deserialize, Debug)]
pub struct EdgeDef {
    pub from: (serde_yaml::Value, serde_yaml::Value), // (node_id, port)
    pub to:   (serde_yaml::Value, serde_yaml::Value),
}

#[derive(Deserialize, Debug, Clone)]
pub struct MacroDef {
    pub encoder: u32,
    pub node:    u32,
    pub param:   String,
}

#[derive(Debug)]
pub enum InstrumentError {
    Io(std::io::Error),
    Parse(serde_yaml::Error),
    UnknownVersion(u32),
    UnknownNodeType { type_tag: String },
    UnknownPort     { node: u32, port: String },
    DuplicateNodeId(u32),
    PluginNotFound  { plugin_id: String },
    MissingField    { node: u32, field: &'static str },
}

impl std::fmt::Display for InstrumentError { /* ... */ }
impl From<std::io::Error>  for InstrumentError { /* ... */ }
impl From<serde_yaml::Error> for InstrumentError { /* ... */ }
```

-----

## 3.3 Graph Builder

**New file:** `crates/paraclete-app/src/builder.rs`

```rust
use paraclete_runtime::NodeConfigurator;
use crate::instrument::{InstrumentDefinition, InstrumentError};

/// Returned by build_from_instrument(). Holds all node IDs so that
/// paraclete-app startup code and profile scripts can reference them.
pub struct InstrumentIds {
    pub clock:      u32,
    pub mix:        u32,
    pub output:     u32,
    pub sequencers: Vec<u32>,
    pub generators: Vec<u32>,
    pub effects:    Vec<u32>,
    /// All node IDs in declaration order — for script constant injection.
    pub all:        Vec<(String, u32)>,  // (display_name_or_type, id)
}

/// Build a NodeConfigurator graph from an InstrumentDefinition.
///
/// Node IDs are taken directly from the YAML `id` fields. The caller is
/// responsible for ensuring IDs are stable across reloads (required by
/// ADR-025 for project file compatibility).
///
/// For `clap_plugin` nodes: requires `paraclete-clap-host` to be available.
/// If no CLAP host is linked, declaring a `clap_plugin` node returns
/// `InstrumentError::UnknownNodeType`.
pub fn build_from_instrument(
    def: &InstrumentDefinition,
    conf: &mut NodeConfigurator,
) -> Result<InstrumentIds, InstrumentError>;

/// Load and parse an instrument definition YAML file.
pub fn load_instrument_definition(
    path: &std::path::Path,
) -> Result<InstrumentDefinition, InstrumentError>;
```

**Supported `type_tag` values at P8:**

| `type_tag` | Rust constructor | Notes |
|------------|-----------------|-------|
| `"internal_clock"` | `InternalClock::new(def.bpm)` | One per instrument |
| `"sequencer"` | `Sequencer::new()` or `Sequencer::with_name(name)` | |
| `"sampler"` | `Sampler::new()` | |
| `"analog_engine:kick"` | `AnalogEngine::kick()` | Variant after colon |
| `"analog_engine:snare"` | `AnalogEngine::snare()` | |
| `"analog_engine:hihat"` | `AnalogEngine::hihat()` | |
| `"fm_engine:kick"` | `FmEngine::fm_kick()` | |
| `"fm_engine:bell"` | `FmEngine::fm_bell()` | |
| `"fm_engine:bass"` | `FmEngine::fm_bass()` | |
| `"distortion"` | `DistortionNode::new()` | |
| `"filter"` | `FilterNode::new()` | |
| `"mix"` | `MixNode::new(channel_count)` | `channel_count` field required |
| `"audio_output"` | `AudioOutput::new()` | |
| `"clap_plugin"` | `PluginLibrary::instantiate(plugin_id)` | `plugin_id` required |

Unknown `type_tag` values return `InstrumentError::UnknownNodeType`.

**Port name resolution:**

String port references are resolved to the corresponding `PORT_*` constants
at build time. Case-insensitive. The following names are recognised:

| String | Resolves to |
|--------|------------|
| `"clock_out"`, `"clock_in"` | `InternalClock::PORT_CLOCK_OUT`, `Sequencer::PORT_CLOCK_IN` |
| `"events_out"`, `"events_in"` | `Sequencer::PORT_EVENTS_OUT`, node `PORT_EVENTS_IN` |
| `"audio_out"`, `"audio_in"` | `PORT_AUDIO_OUT`, `PORT_AUDIO_IN` |
| `"mod_out"`, `"mod_in"` | `PORT_MOD_OUT`, `PORT_MOD_IN` |
| Numeric string `"0"`, `"1"`, … | Port index directly |

Unrecognised string port names return `InstrumentError::UnknownPort`.

**Initial param application:**

After constructing each node, apply `initial_params` via
`CMD_SET_PARAM` before calling `conf.add_node()`:

```rust
for (param_name, value) in &node_def.initial_params {
    let param_id = ParamDescriptor::id_for_name(param_name);
    node.apply_initial_param(param_id, *value);
    // apply_initial_param calls bank.set() directly (bank is not yet
    // built at this point — call after activate() in the audio engine,
    // or store params and apply them via deserialize-equivalent at
    // activate() time).
}
```

Because `ParameterBank` is built at `activate()` time (not at
construction), initial params from the YAML must be delivered via
`Node::deserialize()` with a dedicated serialised form, or via a new
`set_initial_params(map)` method. **Decision:** add a
`set_initial_params(&mut self, params: &HashMap<String, f64>)` method to
the `Node` trait with a default no-op body. Nodes that use `ParameterBank`
override it to store the values and apply them at the next `activate()`.
This is a non-breaking trait addition (default no-op).

```rust
// In paraclete-node-api/src/node.rs — add to Node trait:

/// Apply initial parameter values from the instrument definition file.
/// Called after node construction, before activate().
/// The default implementation is a no-op.
/// Nodes with ParameterBank should store the values and apply them
/// in activate() before returning (after bank construction).
fn set_initial_params(&mut self, _params: &std::collections::HashMap<String, f64>) {}
```

-----

## 3.4 `paraclete-app` startup sequence change

Per ADR-026 §Consequences, the startup order becomes:

```
load_instrument_definition(path)
  → build_from_instrument(def, conf)
    → load_project(project_path, conf)   // optional; applies state deltas
      → conf.build_executor()            // consumes configurator
        → start_audio(executor)          // audio thread starts
          → load_profiles(scripting)     // evaluate .rhai files
            → start_tui(bus, ids)        // terminal UI starts (Commit 6)
```

**Macro pre-population:** After `load_profiles()`, iterate
`def.macros` and call `publish_context(encoder, node, param)` for each
declared macro. This ensures the TUI encoder row has a non-empty initial
state before the first hardware interaction.

```rust
for macro_def in &def.macros {
    scripting.eval(format!(
        r#"publish_context("encoder_{}", {}, "{}");"#,
        macro_def.encoder, macro_def.node, macro_def.param
    ))?;
}
```

-----

## 3.5 Tests

```
instrument_load_minimal_yaml_succeeds
    Parse a minimal YAML with one internal_clock and one audio_output.
    Assert Ok(InstrumentDefinition) with correct name and bpm.

instrument_build_single_chain_connects_correctly
    Build: clock → sequencer → sampler → audio_output.
    Assert Ok(ids) and that ids.clock, ids.sequencers[0] are non-zero
    and distinct.

instrument_initial_params_applied_before_activate
    Declare a sequencer with initial_params: {step_length: 8}.
    Build; activate nodes; assert the sequencer's step_length == 8.

instrument_unknown_node_type_returns_error
    Declare a node with type: "nonexistent".
    Assert Err(InstrumentError::UnknownNodeType { .. }).

instrument_unknown_version_returns_error
    Parse YAML with format_version: 99.
    Assert Err(InstrumentError::UnknownVersion(99)).
```

-----

# Part 4: Terminal UI — `paraclete-tui` (Commit 4)

**Crate:** `paraclete-tui` (new, GPL3)

**Reference:** ADR-026 §Terminal UI as Primary Feedback Surface

`paraclete-tui` is the instrument's display. It is not a developer tool.
It subscribes to the state bus and redraws at ~30 Hz. The display is
page-based, Elektron-style: transport bar, encoder row, step row.

-----

## 4.1 Library choice: `ratatui`

`ratatui` (the actively maintained fork of `tui-rs`) is the TUI library.
It provides a retained-mode widget model over a `crossterm` backend. The
`crossterm` crate handles terminal I/O and raw mode on all three OSes.

Alternative (`cursive`) was considered but uses a callback model that
makes it awkward to subscribe to an external event stream at a fixed rate.
`ratatui`'s immediate-mode render-per-frame model fits the 30 Hz subscriber
loop naturally.

**Dependencies:**
```toml
[dependencies]
ratatui    = "0.26"
crossterm  = "0.27"
paraclete-node-api = { path = "../paraclete-node-api" }
# paraclete-runtime for StateBusHandle:
paraclete-runtime  = { path = "../paraclete-runtime" }
```

`paraclete-tui` depends on `paraclete-runtime` for `StateBusHandle`. This
is a platform crate (GPL3); the TUI is GPL3. There is no portability
violation: `paraclete-tui` is not a node; the ADR-022 portability rule
applies to node crates only.

-----

## 4.2 Crate structure

```
crates/paraclete-tui/
├── Cargo.toml          (GPL3)
└── src/
    ├── lib.rs          (pub re-exports: TuiApp, TuiConfig, TuiError)
    ├── app.rs          (TuiApp — drives render loop, owns TuiState)
    ├── state.rs        (TuiState — cached values read from state bus)
    └── layout.rs       (render() — draws to ratatui Frame)
```

-----

## 4.3 `TuiConfig`

```rust
pub struct TuiConfig {
    /// Node ID of the InternalClock (for BPM + playing state).
    pub clock_id: u32,
    /// Node IDs of all Sequencer nodes, in track order.
    pub seq_ids: Vec<u32>,
    /// Number of encoder slots to display (2, 4, or 8).
    pub encoder_count: u8,
    /// Refresh rate target. Default: 30.
    pub fps: u8,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { clock_id: 0, seq_ids: vec![], encoder_count: 8, fps: 30 }
    }
}
```

-----

## 4.4 `TuiState`

```rust
pub struct TuiState {
    pub bpm:              f64,
    pub playing:          bool,
    pub current_step:     u8,           // 0-based
    pub active_track:     usize,        // 0-based index into seq_ids
    pub steps:            [bool; 16],   // active trigs for active track
    pub encoders:         Vec<EncoderSlot>,
    /// Set when any displayed value changes. Cleared after each render.
    pub dirty:            bool,
}

pub struct EncoderSlot {
    pub label:            String,   // display name resolved from cap doc
    pub param_id:         u32,      // id_for_name hash
    pub node_id:          u32,
    pub value:            f64,
    pub min:              f64,
    pub max:              f64,
    /// Set for ~500ms after value changes. TUI highlights the slot.
    pub recently_changed: bool,
    /// Timestamp (monotonic ms) of last change; 0 if never changed.
    pub changed_at_ms:    u64,
}
```

`TuiState` is fully owned by `TuiApp`. It is updated on the main thread
during `tick()` before rendering.

-----

## 4.5 `TuiApp`

```rust
pub struct TuiApp {
    state:   TuiState,
    bus:     Rc<RefCell<StateBusHandle>>,
    config:  TuiConfig,
    /// Node capability documents for param name lookup.
    /// Populated at construction from NodeConfigurator.
    cap_docs: HashMap<u32, CapabilityDocument>,
}

impl TuiApp {
    /// Construct the TUI app.
    ///
    /// `cap_docs`: map from node_id → CapabilityDocument, provided by
    /// paraclete-app at startup. Used to resolve param name strings from
    /// id_for_name hashes stored in the state bus.
    pub fn new(
        bus:      Rc<RefCell<StateBusHandle>>,
        config:   TuiConfig,
        cap_docs: HashMap<u32, CapabilityDocument>,
    ) -> Self;

    /// One tick: read state bus → update TuiState → render if dirty.
    /// Call at the configured fps rate (default: every ~33ms).
    pub fn tick(
        &mut self,
        terminal: &mut ratatui::Terminal<impl ratatui::backend::Backend>,
    ) -> Result<(), TuiError>;

    /// Tear down the TUI (restore terminal state).
    pub fn shutdown(&self) -> Result<(), TuiError>;
}
```

**`tick()` behaviour:**

1. Read `/node/{clock_id}/bpm` → `state.bpm`
2. Read `/node/{clock_id}/playing` → `state.playing` (0.0 = stopped, 1.0 = playing)
3. Read `/node/{seq_ids[active_track]}/current_step` → `state.current_step`
4. Read `/node/{seq_ids[active_track]}/steps` — step bitfield → `state.steps`
5. For each encoder slot i:
   - Read `/context/encoder_{i}/node` → `slot.node_id`
   - Read `/context/encoder_{i}/param` → `slot.param_id` (as hash)
   - Resolve display name: look up `cap_docs[node_id]` → find param with
     matching `id` → use `param.name` as `slot.label`
   - Read `/node/{slot.node_id}/{param_name}` → `slot.value`
   - Resolve `slot.min` / `slot.max` from the same CapabilityDocument
   - If value changed since last tick: set `recently_changed = true`,
     record `changed_at_ms`; clear `recently_changed` if
     `now_ms - changed_at_ms > 500`
6. If any value changed: set `state.dirty = true`
7. If `state.dirty`: call `terminal.draw(|f| layout::render(f, &self.state))`;
   clear `state.dirty`

**Active track selection:** The active track is controlled by the profile
scripts (they write `/script/selected_track` to the state bus when the
operator selects a track on the Launchpad). `tick()` reads
`/script/selected_track` and updates `state.active_track`. If not set,
defaults to 0.

-----

## 4.6 Layout

**`layout::render(frame, state)`:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ♩ 140.0 BPM  ▶ PLAYING   Step: 5 / 16   Track 1: Kick (Sequencer)  │
├─────────────────────────────────────────────────────────────────────┤
│  E0: drive       E1: cutoff      E2: resonance    E3: decay         │
│  [▓▓▓▓▓░░░░░]   [▓▓▓▓▓▓▓░░░]   [▓░░░░░░░░░]     [▓▓▓░░░░░░]      │
│   0.65            1200 Hz         0.12              0.200 s         │
├─────────────────────────────────────────────────────────────────────┤
│  ■ · · ■ · · ■ · ■ · · ■ · · · ■                         [5]       │
└─────────────────────────────────────────────────────────────────────┘
```

The terminal may be narrower than shown; the layout scales. On narrow
terminals (< 60 cols), the encoder row shows 4 slots; the step row wraps.
Minimum supported terminal width: 40 columns.

**Transport bar** (top row): BPM, playing/stopped indicator (▶ or ■),
current step / total steps, active track name + engine type.

**Encoder row** (middle section): `encoder_count` slots (default 8,
displayed in rows of 4 on narrow terminals). Each slot shows:
- Parameter label (from CapabilityDocument)
- Value bar (proportional fill between min and max)
- Numeric value with unit suffix (Hz for cutoff, s for decay/attack/release,
  % for wet/dry/resonance, plain number for others)
- Slot outline highlighted briefly (`recently_changed`) with a different
  border style

**Step row** (bottom): 16 cells, ■ = active trig, · = empty trig,
current playback position indicated by cell highlighting. Step count shown
at the right edge.

-----

## 4.7 `TuiError`

```rust
pub enum TuiError {
    Io(std::io::Error),
    Draw(String),
}
```

-----

## 4.8 Tests

TUI rendering cannot be meaningfully unit-tested without a real terminal.
Tests cover `TuiState` update logic and the state bus read path.

```
tui_state_updates_bpm_from_state_bus
    Construct a StateBusHandle. Write /node/1/bpm = 140.0.
    Create TuiApp with clock_id=1. Call tick() with a test backend.
    Assert state.bpm == 140.0.

tui_state_playing_flag_reflects_state_bus
    Write /node/1/playing = 1.0.
    After tick(): assert state.playing == true.
    Write /node/1/playing = 0.0.
    After tick(): assert state.playing == false.

tui_encoder_slot_resolves_param_label_from_cap_doc
    Create a CapabilityDocument with a "cutoff" param (min=20, max=20000, default=1000).
    Write /context/encoder_0/node = 42.0.
    Write /context/encoder_0/param = id_for_name("cutoff") as f64.
    Write /node/42/cutoff = 1200.0.
    After tick(): assert encoders[0].label == "cutoff"
                  and encoders[0].value == 1200.0.

tui_recently_changed_clears_after_500ms
    Write a new encoder value. Tick immediately: assert recently_changed == true.
    Advance mock clock by 501ms. Tick again: assert recently_changed == false.
```

-----

# Part 5: CLAP Host — `paraclete-clap-host` (Commit 5)

**Crate:** `paraclete-clap-host` (new, GPL3)

**Reference:** ADR-027

Paraclete loads third-party `.clap` plugins as `Node` instances. P8 scope:
generator plugins (audio output) only. Effect plugins (audio input) deferred
to P9 per ADR-027.

-----

## 5.1 Crate structure

```
crates/paraclete-clap-host/
├── Cargo.toml              (GPL3; deps: paraclete-node-api, clack)
└── src/
    ├── lib.rs              (pub re-exports: PluginLibrary, PluginDescriptor,
    │                        PluginNode, HostError, scan_clap_paths)
    ├── scan.rs             (scan_clap_paths — OS-conditional)
    ├── library.rs          (PluginLibrary, PluginDescriptor)
    ├── bridge.rs           (HostParamBridge)
    └── node.rs             (PluginNode — impl Node)
```

Workspace `Cargo.toml`:
```toml
[workspace.dependencies]
clack = { version = "0.1", features = ["host"] }
# clack version current as of June 2026; verify at implementation time
```

-----

## 5.2 `PluginDescriptor`

```rust
#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    /// CLAP plugin ID string (e.g. "com.vendor.plugin").
    pub id:          String,
    pub name:        String,
    pub vendor:      String,
    pub version:     String,
    pub features:    Vec<String>,
    /// Path to the `.clap` file this descriptor was loaded from.
    pub source_path: std::path::PathBuf,
}
```

-----

## 5.3 `PluginLibrary`

```rust
pub struct PluginLibrary {
    // clack-managed library handle (not pub)
    // ...
    descriptors: Vec<PluginDescriptor>,
}

impl PluginLibrary {
    /// Load a `.clap` shared library file.
    /// Returns `Err` if the file cannot be opened or does not export
    /// a valid CLAP entry point.
    pub fn load(path: &std::path::Path) -> Result<Self, HostError>;

    /// All plugins declared in this library.
    pub fn descriptors(&self) -> &[PluginDescriptor];

    /// Instantiate a plugin by its CLAP ID string.
    ///
    /// Returns a `PluginNode` (as `Box<dyn Node>`) ready to be added to
    /// a `NodeConfigurator`. The returned node is not yet activated;
    /// `activate()` is called by the runtime before `process()`.
    ///
    /// `sample_rate` and `block_size` are hints; the node stores them and
    /// uses them if activated before the runtime calls `Node::activate()`.
    pub fn instantiate(
        &self,
        plugin_id:   &str,
        sample_rate: f32,
        block_size:  usize,
    ) -> Result<Box<dyn Node>, HostError>;
}
```

**`PluginLibrary::load()` implementation notes:**

Use `clack::host::PluginBundle` or equivalent API to load the dynamic
library and enumerate its factory. Walk the factory's plugin list to build
`descriptors`. The CLAP spec requires that the entry point `init()` be
called before any other use; `clack` handles this automatically.

-----

## 5.4 `HostParamBridge`

```rust
pub struct HostParamBridge {
    entries: Vec<HostParamEntry>,
}

struct HostParamEntry {
    /// Native CLAP param ID as declared by the plugin.
    clap_id:      clap_id,
    /// Paraclete param ID: id_for_name(param_name_str).
    paraclete_id: u32,
    /// Display name from the CLAP params extension.
    name:         String,
    min:          f64,
    max:          f64,
    default:      f64,
}

impl HostParamBridge {
    /// Query the plugin's CLAP params extension and build the bridge.
    /// Must be called after the plugin is created but before activate().
    pub fn from_plugin(plugin: &/* clack plugin handle */) -> Self;

    /// Paraclete param ID for a native CLAP param ID. None if not found.
    pub fn paraclete_id_for(&self, clap_id: clap_id) -> Option<u32>;

    /// CLAP param ID for a Paraclete param ID. None if not found.
    pub fn clap_id_for(&self, paraclete_id: u32) -> Option<clap_id>;

    /// Build a CapabilityDocument from this bridge (synthesised from CLAP params).
    pub fn to_capability_document(&self) -> CapabilityDocument;
}
```

`id_for_name(param_name_str)` is used to compute `paraclete_id`. If a
plugin declares `"cutoff"` as a parameter name, `paraclete_id` will equal
`id_for_name("cutoff")` — the same value that the canonical ADR-019 table
produces. A hardware encoder mapped to `id_for_name("cutoff")` will reach
this plugin's cutoff without any profile change.

If a plugin's param names conflict (two params hash to the same u32), the
second is silently skipped. Log the collision in debug builds.

-----

## 5.5 `PluginNode`

```rust
pub struct PluginNode {
    /// clack plugin instance handle (not pub)
    // ...
    /// Synthesised from HostParamBridge at instantiation.
    cap_doc:     CapabilityDocument,
    bridge:      HostParamBridge,
    /// Pre-allocated audio output buffer [channels][frames].
    audio_out:   Vec<Vec<f32>>,
    sample_rate: f32,
    block_size:  usize,
}

impl Node for PluginNode {
    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate = sample_rate;
        self.block_size  = block_size;
        // Resize audio_out if needed.
        // Call plugin.activate(sample_rate, min_frames=1, max_frames=block_size).
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // 1. Apply CMD_SET_PARAM / CMD_BUMP_PARAM via bank (or directly via
        //    CLAP param flush for precise host→plugin value updates).
        // 2. Translate Paraclete events → CLAP input events (NoteOn, NoteOff).
        // 3. Call plugin.process() with empty audio inputs, audio_out as output.
        // 4. Copy audio_out[0] → output.audio_output_mut(0).
    }

    fn deactivate(&mut self) {
        // Call plugin.deactivate().
    }

    fn capability_document(&self) -> CapabilityDocument {
        self.cap_doc.clone()
    }

    fn type_name(&self) -> &'static str { "PluginNode" }

    fn serialize(&self) -> Vec<u8> {
        // Use CLAP state extension if the plugin supports it.
        // Return empty Vec if the extension is absent.
        vec![]
    }

    fn deserialize(&mut self, data: &[u8]) {
        // Pass data to CLAP state extension if present; silently no-op otherwise.
        if data.is_empty() { return; }
        // ... clack state load call ...
    }
}
```

**`process()` event translation:**

```rust
// In PluginNode::process():

// 1. CMD_SET_PARAM / CMD_BUMP_PARAM — apply to a local ParameterBank
//    (built from cap_doc at activate()), then flush changed values to
//    the plugin via the CLAP params extension flush callback.
self.local_bank.handle_commands(input.commands);

// 2. MIDI events → CLAP input event list
for timed in input.events() {
    match &timed.event {
        Event::NoteOn  { channel, note, velocity } => {
            // push clap_event_note (CLAP_EVENT_NOTE_ON) to input list
        }
        Event::NoteOff { channel, note } => {
            // push clap_event_note (CLAP_EVENT_NOTE_OFF) to input list
        }
        _ => {}
    }
}

// 3. Call plugin.process()
```

**Audio port count:** At P8, `PluginNode` exposes exactly one audio output
port (`PORT_AUDIO_OUT = 0`). If the plugin declares more channels (stereo),
the two channels are mixed down to mono for `output.audio_output_mut(0)`.
Full multi-channel support is deferred.

**`PluginNode` portability note:** `PluginNode` is in `paraclete-clap-host`
which depends on both `paraclete-node-api` and `clack`. The ADR-022
portability rule applies to `paraclete-nodes`, not to `paraclete-clap-host`.
`cargo tree -p paraclete-nodes` must not show `paraclete-clap-host` as a
dependency.

-----

## 5.6 `scan_clap_paths()`

```rust
/// Return paths to all `.clap` files found in OS-standard directories.
/// Does not load or validate the files; callers use PluginLibrary::load().
pub fn scan_clap_paths() -> Vec<std::path::PathBuf>;
```

OS-standard directories (from the CLAP specification):

```rust
#[cfg(target_os = "linux")]
fn clap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/lib/clap"),
        PathBuf::from("/usr/local/lib/clap"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".clap"));
    }
    dirs
}

#[cfg(target_os = "macos")]
fn clap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Library/Audio/Plug-Ins/CLAP"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home)
            .join("Library/Audio/Plug-Ins/CLAP"));
    }
    dirs
}

#[cfg(target_os = "windows")]
fn clap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Ok(pf) = std::env::var("COMMONPROGRAMFILES") {
        dirs.push(PathBuf::from(pf).join("CLAP"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Programs\\Common\\CLAP"));
    }
    dirs
}
```

`scan_clap_paths()` calls `clap_search_dirs()`, walks each existing
directory (non-recursive; `.clap` extension), and returns the collected
paths. Non-existent directories are silently skipped.

-----

## 5.7 `HostError`

```rust
#[derive(Debug)]
pub enum HostError {
    /// Dynamic library could not be opened (OS error).
    Load(String),
    /// File does not export a valid CLAP entry point.
    InvalidPlugin(String),
    /// No plugin with this ID in the library.
    PluginNotFound { plugin_id: String },
    /// Plugin activation failed (plugin returned false).
    Activate(String),
    /// Required CLAP extension absent (e.g. `clap_plugin_params`).
    MissingExtension(&'static str),
    /// CLAP param name collision (two params hash to same Paraclete ID).
    ParamIdCollision { param_a: String, param_b: String },
}

impl std::fmt::Display for HostError { /* ... */ }
impl std::error::Error  for HostError {}
```

-----

## 5.8 Tests

The `PluginLibrary::load()` path requires a real `.clap` binary. Tests use
the machine bank `.clap` files produced by Commit 1 of P8 — these are
Paraclete's own plugins and are available in the build output. This creates
a dependency between Commit 5 tests and Commit 1 artifacts; the tests are
tagged accordingly and run after Commit 1 builds cleanly.

```
host_param_bridge_from_cap_doc_round_trips
    Build a CapabilityDocument with decay (min=0.01, max=4.0, default=0.2)
    and cutoff (min=20.0, max=20000.0, default=1000.0).
    Build a HostParamBridge by simulating a plugin with those params.
    Assert bridge.paraclete_id_for(clap_id_for_decay) ==
        Some(id_for_name("decay")).

host_param_bridge_to_capability_document_has_correct_ranges
    Assert cap_doc.params[0].min == 0.01 and .max == 4.0 for decay.

scan_clap_paths_returns_vec_no_panic
    Call scan_clap_paths(). Assert the return value is Ok or an empty Vec —
    no panic even if no CLAP directories exist.

plugin_node_from_kick_clap_produces_audio
    (tagged #[ignore] — requires Commit 1 .clap build artifact)
    Load the kick.clap binary via PluginLibrary::load().
    Instantiate a PluginNode. Activate at 48000 Hz / 512 frames.
    Inject a NoteOn event. Run 3 process() blocks.
    Assert max absolute value of audio output > 1e-5.

plugin_node_capability_document_has_params
    (tagged #[ignore] — requires kick.clap)
    Load kick.clap. Assert capability_document() has at least one param.
    Assert id_for_name("decay") is in the param list (AnalogEngine::kick
    declares "decay").

plugin_node_serialize_deserialize_roundtrip
    (tagged #[ignore] — requires kick.clap)
    serialize() → deserialize() round trip. Assert no panic.
```

-----

# Part 6: App Wiring (Commit 6)

**Crate:** `paraclete-app`

Commit 6 assembles all P8 deliverables into the working application. The
audio engine, TUI, instrument definition loader, and CLAP host all come
together here.

-----

## 6.1 `paraclete-app` dependency additions

```toml
[dependencies]
paraclete-tui       = { path = "../paraclete-tui" }
paraclete-clap-host = { path = "../paraclete-clap-host" }
serde_yaml          = "0.9"
```

-----

## 6.2 Startup sequence

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // 1. Load instrument definition
    let def = load_instrument_definition(&args.instrument_path)?;

    // 2. Optional: scan for and pre-load CLAP plugins declared in def
    let mut libraries: HashMap<String, PluginLibrary> = HashMap::new();
    for node_def in &def.nodes {
        if node_def.type_tag == "clap_plugin" {
            let plugin_id = node_def.plugin_id.as_deref()
                .ok_or("clap_plugin node missing plugin_id")?;
            if !libraries.contains_key(plugin_id) {
                let path = resolve_plugin_path(node_def, &args)?;
                libraries.insert(plugin_id.to_string(), PluginLibrary::load(&path)?);
            }
        }
    }

    // 3. Build graph
    let mut conf = NodeConfigurator::new();
    let ids = build_from_instrument(&def, &mut conf, &libraries)?;

    // 4. Load project state (optional)
    if let Some(project_path) = &args.project_path {
        let warnings = load_project(project_path, &mut conf)?;
        for w in &warnings { eprintln!("WARN: {}", w); }
    }

    // 5. Build executor + start audio
    let executor = conf.build_executor();
    let bus_handle = executor.state_bus_handle();
    let _audio_stream = start_audio(executor, def.bpm as f32)?;

    // 6. Load profile scripts
    let mut scripting = ScriptingEngine::new();
    scripting.bind_state_bus(bus_handle.clone());
    scripting.set_constants_from_ids(&ids);
    for profile_path in &def.profiles {
        scripting.eval_file(profile_path)?;
    }

    // 7. Pre-populate macro context from instrument definition
    for macro_def in &def.macros {
        scripting.eval(&format!(
            r#"publish_context("encoder_{}", {}, "{}");"#,
            macro_def.encoder, macro_def.node, macro_def.param
        ))?;
    }

    // 8. Collect capability documents for TUI
    let cap_docs: HashMap<u32, CapabilityDocument> = ids.all.iter()
        .filter_map(|(_, node_id)| {
            conf.get_node_cap_doc(*node_id).map(|doc| (*node_id, doc))
        })
        .collect();

    // 9. Start TUI
    let tui_config = TuiConfig {
        clock_id:      ids.clock,
        seq_ids:       ids.sequencers.clone(),
        encoder_count: 8,
        fps:           30,
    };
    let mut terminal = setup_terminal()?;
    let mut tui = TuiApp::new(bus_handle.clone(), tui_config, cap_docs);

    // 10. Main loop
    let tick_duration = std::time::Duration::from_millis(33);
    loop {
        let t0 = std::time::Instant::now();

        scripting.process_subscriptions(&bus_handle);
        let output = scripting.take_pending_output();
        apply_hardware_output(output);

        tui.tick(&mut terminal)?;

        if check_quit_signal() { break; }
        if let Some(project_path) = &args.save_path {
            save_project(project_path, &conf, &def)?;
        }

        let elapsed = t0.elapsed();
        if elapsed < tick_duration {
            std::thread::sleep(tick_duration - elapsed);
        }
    }

    tui.shutdown()?;
    restore_terminal(terminal)?;
    Ok(())
}
```

**`resolve_plugin_path()`:** If the `NodeDef` has a `plugin_path`, use it
directly. Otherwise, call `scan_clap_paths()` and find the first library
that contains a plugin matching `plugin_id`. If no match, return
`InstrumentError::PluginNotFound { plugin_id }`.

**`NodeConfigurator::get_node_cap_doc(node_id)`:** New method; returns
`Option<CapabilityDocument>` by calling `capability_document()` on the
node at that ID before `build_executor()` is called. After build, returns
None (nodes are moved into the executor). Add to
`paraclete-runtime/src/configurator.rs`.

-----

## 6.3 CLI flags update

Add `--instrument` flag (required if no default path). Keep existing
`--load` and `--save` flags. Add `--no-tui` flag that skips TUI startup
(useful for headless/CI runs).

```
paraclete --instrument=instrument.yaml
paraclete --instrument=instrument.yaml --load=project.ron --save=project.ron
paraclete --instrument=instrument.yaml --no-tui
```

-----

## 6.4 Tests

```
app_wiring_load_instrument_and_build_no_panic
    Load a minimal instrument.yaml (clock + audio_output, no CLAP nodes).
    Call build_from_instrument(). Assert Ok(ids) and ids.clock != 0.

app_wiring_macro_publish_context_populates_state_bus
    Build a minimal instrument with one macro (encoder: 0, node: X, param: "decay").
    Run the macro pre-population step.
    Assert state_bus /context/encoder_0/node == X as f64
    and /context/encoder_0/param == id_for_name("decay") as f64.

app_wiring_no_tui_flag_skips_terminal_init
    Run main loop iteration with --no-tui.
    Assert no crossterm::terminal::enable_raw_mode() was called.
    (Use a mock terminal or check the TuiApp is not constructed.)

app_wiring_project_save_load_roundtrip
    Build instrument → start audio (mock) → save project.
    Load same project. Assert Ok with zero warnings.
```

-----

# P8 Done Criteria

All criteria must pass before P8 is marked complete.

## Gate (Commit 1)

- `cargo test --workspace` passes with 317 + (≥ 4 new) tests after Commit 1, 0 failures
- `InternalClock` stops ticking (zero clock events emitted) in the block after `GlobalStop` injection
- `InternalClock` resumes ticking after `GlobalStart` injection following a stop
- All five machine bank binaries (`kick`, `snare`, `fm_kick`, `fm_bell`, `fm_bass`) build to `.clap` cdylibs without linker errors
- `clap_entry` is a correctly populated `clap_plugin_entry` in all five binaries (non-null callbacks)
- Silence buffer returns a slice of length 65536 without panic at the maximum block size

## Scripting (Commit 2)

- `publish_context("encoder_0", 42, "decay")` in a Rhai script writes
  `/context/encoder_0/node = 42.0` and
  `/context/encoder_0/param = id_for_name("decay") as f64` to the state bus
- Calling `publish_context` with a different mapping overwrites the previous one
- `publish_context` with different `encoder_key` values does not collide

## Instrument Definition (Commit 3)

- A valid `instrument.yaml` with all supported node types parses without error
- `build_from_instrument()` constructs and connects an 8-track drum machine
  graph from a YAML file (no Rust code changes required to add/remove tracks)
- `initial_params` values from the YAML are applied and survive `activate()`
- A YAML with `format_version: 99` returns `InstrumentError::UnknownVersion(99)`
- A YAML with an unknown `type_tag` returns `InstrumentError::UnknownNodeType`
- The `NodeDef` for a `clap_plugin` node without `plugin_id` returns
  `InstrumentError::MissingField`

## Terminal UI (Commit 4)

- `TuiApp::tick()` reads BPM, playing state, active step, and encoder
  context from the state bus and updates `TuiState` correctly
- Encoder slot label is resolved from the `CapabilityDocument` using the
  `id_for_name` hash stored in `/context/encoder_N/param`
- `recently_changed` is set on value change and cleared after 500 ms
- `TuiApp::shutdown()` restores terminal state without panic
- The layout renders without panic on a 40-column terminal (minimum width)

## CLAP Host (Commit 5)

- `PluginLibrary::load(path)` succeeds for a valid `.clap` binary
- `PluginLibrary::descriptors()` returns at least one `PluginDescriptor`
  for the machine bank binaries
- `PluginNode` produced by `instantiate()` activates, processes a NoteOn
  event, and produces non-silent audio (max > 1e-5 after 3 blocks)
- `PluginNode::capability_document()` returns a non-empty `CapabilityDocument`
  with at least one parameter matching the plugin's declared params
- `scan_clap_paths()` does not panic when CLAP directories do not exist
- `PluginNode::serialize()` and `deserialize()` round-trip without panic

## App Wiring (Commit 6)

- `paraclete --instrument=instrument.yaml` starts, runs the main loop for 3
  seconds, and exits cleanly with `--no-tui`
- BPM from the instrument YAML is used as the `InternalClock` initial BPM
- Macros declared in `instrument.yaml` are visible in the state bus under
  `/context/encoder_N/...` before the first hardware interaction
- A CLAP plugin declared in `instrument.yaml` is loaded and its audio
  appears in the mix output
- `--load` and `--save` flags still work with an instrument file present
- `cargo tree -p paraclete-nodes` still shows no dependency on
  `paraclete-runtime`, `paraclete-scripting`, `paraclete-tui`, or
  `paraclete-clap-host` (portability rule unchanged)
- `cargo tree -p paraclete-node-api` shows no dependency on `paraclete-nodes`,
  `paraclete-runtime`, or any platform crate (LGPL3 boundary clean)

## Tests

| State | Tests | Running Total |
|-------|-------|---------------|
| P7 ship | — | 317 |
| Commit 1a (InternalClock stop) | +2 | 319 |
| Commit 1b (machine bank FFI) | +1 | 320 |
| Commit 1c (SILENCE fix) | +1 | 321 |
| Commit 2 (publish_context) | +3 | 324 |
| Commit 3 (instrument definition) | +5 | 329 |
| Commit 4 (TUI) | +4 | 333 |
| Commit 5 (CLAP Host) | +6 | 339 |
| Commit 6 (app wiring) | +4 | 343 |
| **P8 target** | | **≥ 340** |

Tests tagged `#[ignore]` (clap-validator, PluginNode integration) are not
counted in the 343 total. They run as separate CI steps.

All 317 existing tests continue to pass unchanged through every commit.

-----

# P8 Known Gaps and Deferred Items

**Effect-type CLAP plugins (P9):** `PluginNode` at P8 is audio-output-only.
Plugins that require audio input (compressors, reverbs, etc.) need `PluginNode`
to expose audio input ports wired from upstream nodes. This is the standard
effect-node pattern (ADR-017) applied to externally-loaded plugins. Deferred
to P9.

**CLAP GUI extension (deferred):** No GUI is exposed for plugin nodes at P8.
The TUI is the feedback surface. Full CLAP GUI (`clap_plugin_gui`) support
requires platform windowing and is a P9+ concern. Deferred per P7 known gaps.

**Full host callbacks (deferred):** `PluginNode` implements no-op host
callbacks at P8 (latency change, request restart, log, timer). Plugins that
require real host responses may behave unexpectedly. Full host callback
implementation deferred to P9.

**CLAP note expressions (deferred):** `PluginNode` translates `NoteOn` and
`NoteOff` events only. CLAP NoteExpression events (pitch bend, pressure,
slide) are not translated. Deferred to P9.

**Preset management (deferred):** `clap_plugin_preset_discovery` is not
implemented. Plugin preset recall uses `Node::serialize()` / `deserialize()`
via the CLAP state extension when the plugin supports it; plugins without
the state extension start with their own defaults. Full preset management
deferred to P9.

**Hot plugin reload (deferred):** `PluginLibrary` is loaded at startup and
held for the session. Rescanning the CLAP path or reloading a plugin without
restarting the application is deferred.

**Multi-channel audio output from plugins (deferred):** At P8, `PluginNode`
mixes all output channels to mono before writing to `output.audio_output_mut(0)`.
Full stereo / multi-channel routing requires typed stereo audio ports and is
deferred to P9.

**`SubgraphPlugin` CLAP machine slot swapping (still deferred):** ADR-024
noted this as P8+ work depending on `GraphNode` (ADR-023). `GraphNode` is
P9. Still deferred.

**HiHatMachine CLAP binary (deferred):** `AnalogEngine::hihat()` is not
included in the five machine bank binaries. It can be added as a sixth binary
target at P8+ if warranted.

**Dynamic graph topology (deferred to P9+):** The instrument file supports
startup-only topology. Runtime node add/remove is a P9 concern per ADR-025.

**`Sequencer::serialize()` P5 fields (still deferred):** `TrigCondition` and
`StepTiming` are not serialised. Pre-existing limitation noted at P7. Deferred.

**Signed micro-timing (still deferred):** Negative `micro_offset` is not
fully supported. Deferred from P5, P6, P7; still deferred.
