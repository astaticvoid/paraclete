# Paraclete — P7 Interface Specification

> **Implementation blueprint.** These are the contracts P7 implementation
> must satisfy. Do not deviate without updating the relevant ADR first.
>
> **P7 deliverable:** CLAP plugin mode. DAW transport sync. Project save/recall.
> `paraclete-node-api` v0.1.0 ships to crates.io. Per-voice rubato pitch
> resampling in Sampler. OQ-9 resolved.
> **Last updated:** June 2026
> **Baseline:** P6.5 complete — 289 tests, 0 failures
> **Depends on:** p0–p6 interfaces and reports — all prior types assumed
> present and correct.
> **References:** ADR-024 (CLAP wrapper), ADR-025 (project file format),
> ADR-019 (universal parameter control — P6.5 amendment), ADR-022 (node
> portability — P6.5 amendment), ADR-001 (license — GPL3/LGPL3 boundary)

-----

## How to use this document

**Implement in commit order.** Each commit is independently testable. The
node API changes in Commit 1 are a prerequisite for Commit 3 (project
save/recall needs `type_name()`). The `paraclete-clap` infrastructure in
Commit 4 is a prerequisite for Commits 5 and 6. Commit 7 (publication prep)
comes last, after the full API surface is stable.

**Portability rule applies throughout.** Every node change passes
`cargo tree -p paraclete-nodes` before merge. Signal port wiring and
ParameterBank usage must remain correct. See ADR-022.

**Parameter names are now long-term contracts.** From crates.io publication
onward, any rename to a declared parameter name is a breaking change. The
ADR-019 canonical names table is the reference. No new parameter names are
introduced in P7 without first checking that table.

**`paraclete-clap` is GPL3.** The LGPL3/GPL3 boundary is at `paraclete-node-api`.
See ADR-001 and ADR-024. All code in `paraclete-clap` is GPL3.

| Commit | Crate(s) | Deliverable |
|--------|----------|-------------|
| 1 | `paraclete-node-api`, `paraclete-nodes`, `paraclete-runtime` | `type_name()` + `published_state()` push-down (OQ-9) + all-node migration |
| 2 | `paraclete-nodes` | Per-voice rubato pitch resampling in Sampler |
| 3 | `paraclete-runtime`, `paraclete-app` | Project save/recall — RON format (ADR-025) |
| 4 | `paraclete-clap` (new), `paraclete-runtime` | `paraclete-clap` crate infrastructure; nih-plug removal; `set_transport_override()` |
| 5 | `paraclete-clap` | `SingleNodePlugin` — Sequencer as CLAP MIDI effect |
| 6 | `paraclete-clap` | `SubgraphPlugin` + five machine bank `.clap` binaries |
| 7 | `paraclete-node-api` | crates.io publication prep — v0.1.0 |

-----

# Part 1: Node API Evolution (Commit 1)

**Crates:** `paraclete-node-api`, `paraclete-nodes`, `paraclete-runtime`

Two additions to the `Node` trait. Both are pre-crates.io publication API
decisions; this is the last opportunity to make them without a semver break.

-----

## 1.1 `Node::type_name()`

Required by ADR-025: `NodeSnapshot.type_name` holds a stable human-readable
string for each node. The default implementation returns the Rust type name
(suitable for development), but nodes that want a stable display name across
refactors can override it.

```rust
// In paraclete-node-api/src/node.rs — add to the Node trait:

/// Returns a human-readable type label for this node.
/// Used in project file snapshots (NodeSnapshot.type_name) and in
/// diagnostic output. Not used for dispatch — project load is by NodeId.
///
/// Default: `std::any::type_name::<Self>()`.
/// Override to provide a stable name that does not change with module
/// path refactors (e.g. `"AnalogEngine"` rather than
/// `"paraclete_nodes::analog_engine::AnalogEngine"`).
fn type_name(&self) -> &'static str {
    std::any::type_name::<Self>()
}
```

No existing node needs to override this for P7. The default is sufficient
for v0.1.0 where module paths are stable. The override path is available
for third-party nodes.

**Test:**

```
node_type_name_default_is_nonempty
    Construct an InternalClock. Assert type_name() is non-empty.
    Regression: confirms the trait method compiles and returns a usable string.
```

-----

## 1.2 `Node::published_state()` — push-down signature (OQ-9)

**Current signature (P0–P6.5):**
```rust
fn published_state(&self) -> Vec<(String, f64)> { vec![] }
```

Every call allocates a `Vec` on the audio thread. With 8+ nodes publishing
state each cycle at ~172 cycles/second, this is ~1 700 allocations/second at
P6 scale. At the full P7 graph (13+ publishing nodes), it exceeds the
acceptable audio-thread allocation budget for a production instrument.

**New signature:**
```rust
// In the Node trait:

/// Publish runtime state to the state bus.
///
/// Called by the runtime each process cycle on the main thread after
/// audio processing. Push zero or more `(key, value)` pairs into `buf`.
/// `buf` is pre-allocated by the runtime and cleared before each call;
/// the node must not call `buf.clear()` itself.
///
/// Key strings follow the `/node/{id}/` namespace convention. The
/// runtime prepends the node's registered ID to any bare key.
///
/// Default: no-op (nodes that publish no state do nothing).
fn published_state(&self, buf: &mut Vec<(String, f64)>) {}
```

The `Vec` is pre-allocated once by the runtime per node slot and reused
every cycle. `Vec::clear()` retains the existing allocation. No audio-thread
allocation occurs after the first cycle.

**Runtime changes (`paraclete-runtime/src/executor.rs`):**

```rust
pub struct NodeExecutor {
    // ... existing fields unchanged ...

    /// Pre-allocated state output buffer, one per node slot.
    /// Cleared before each published_state() call; capacity is retained.
    state_bufs: Vec<Vec<(String, f64)>>,
}
```

In `build_executor()` (configurator.rs): initialise `state_bufs` with one
`Vec::new()` per slot.

In `NodeExecutor::process()`, after the per-slot DSP loop:

```rust
// State publication — main-thread phase (or end-of-process pass):
for (slot_idx, slot) in self.slots.iter().enumerate() {
    self.state_bufs[slot_idx].clear();
    slot.node.published_state(&mut self.state_bufs[slot_idx]);
    // forward self.state_bufs[slot_idx] to state bus
    // (same routing as before; the content is unchanged, only the
    // allocation strategy differs)
}
```

**Node migrations — all nodes currently implementing `published_state()`:**

The pattern for migration is mechanical. Replace:
```rust
// Before:
fn published_state(&self) -> Vec<(String, f64)> {
    vec![
        ("bpm".into(), self.bpm as f64),
        ("tick".into(), self.tick as f64),
    ]
}

// After:
fn published_state(&self, buf: &mut Vec<(String, f64)>) {
    buf.push(("bpm".into(),  self.bpm as f64));
    buf.push(("tick".into(), self.tick as f64));
}
```

Nodes requiring migration (those with non-default `published_state()` at P6.5):

| Node | Published keys |
|------|---------------|
| `InternalClock` | `bpm`, `tick`, `playing` |
| `Sequencer` | `active_step`, `pattern`, `loop_count` |
| `AnalogEngine` | `machine`, `decay`, `tune` (machine-dependent subset) |
| `FmEngine` | `machine`, `decay`, `tune` (machine-dependent subset) |
| `Sampler` | `loaded`, `root_note` (if tracked) |

Any node not in this table has no `published_state()` body to migrate.

**Done criterion:** All nodes compile, 289 tests pass, no new failures.

**Tests:**

```
node_type_name_default_is_nonempty
    (described above)

published_state_push_down_no_allocation_after_first_cycle
    Construct an InternalClock in an executor with one slot.
    Call executor.process() twice.
    Assert that state_bufs[0].capacity() after cycle 2 equals
    state_bufs[0].capacity() after cycle 1.
    (Capacity stable = no reallocation between cycles.)
```

-----

# Part 2: Per-voice Rubato Pitch Resampling (Commit 2)

**Crate:** `paraclete-nodes`
**File:** `paraclete-nodes/src/sampler.rs`

-----

## 2.1 Background

The P6 Sampler uses rubato at load time to convert the sample's native sample
rate to the session sample rate. At runtime, playback advances the read
position by a ratio corresponding to the MIDI note vs. the sample's root note,
using linear interpolation for sub-sample positions. Linear interpolation
introduces spectral artefacts that are audible on pitched transpositions of
more than ±3 semitones.

P7 replaces the per-sample linear interpolation with a per-voice windowed sinc
resampler from the `rubato` crate. The resampler is pre-allocated at
`activate()` and reused across NoteOn events within a voice slot.

-----

## 2.2 New Sampler Parameter

One new parameter is added to `Sampler::capability_document()`:

```rust
pub const PARAM_ROOT_NOTE: u32 = id_for_name("root_note");
// Range: 0.0–127.0 (MIDI note number), default 60.0 (C4)
// The MIDI note at which the sample was recorded. Playback at
// root_note produces 1:1 ratio (no transposition).
```

This parameter is readable via `CMD_SET_PARAM` / `CMD_BUMP_PARAM` and is
persisted via `serialize()` / `deserialize()` alongside the existing Sampler
state.

`PARAM_ROOT_NOTE` is a new canonical name — add it to the ADR-019 names table:

| Name | Concept | Typical range | Example nodes |
|------|---------|---------------|---------------|
| `"root_note"` | Sample root MIDI note | 0–127 (MIDI) | `Sampler` |

-----

## 2.3 `SamplerVoice` changes

```rust
// paraclete-nodes/src/sampler.rs

use rubato::{SincFixedOut, SincInterpolationParameters, SincInterpolationType,
             WindowFunction, Resampler};

struct SamplerVoice {
    // existing fields (active, position, sample_idx, env_phase, env_value, ...)
    // unchanged

    /// Per-voice sinc resampler. `None` when ratio == 1.0 (no resampling needed).
    resampler:    Option<SincFixedOut<f32>>,
    /// Current playback ratio: > 1.0 = pitch up, < 1.0 = pitch down.
    resamp_ratio: f64,
    /// Input staging buffer for the resampler (mono; channels = 1).
    resamp_in:    Vec<Vec<f32>>,
    /// Output staging buffer from the resampler.
    resamp_out:   Vec<Vec<f32>>,
    /// Fractional sample read cursor (replaces the integer `position` advance
    /// used in the linear interpolation path).
    read_pos:     f64,
}
```

-----

## 2.4 `Sampler::activate()` changes

Pre-allocate one `SamplerVoice::resampler` per voice slot at the worst-case
transposition ratio. At `activate()`:

```rust
fn activate(&mut self, sample_rate: f32, block_size: usize) {
    // existing: bank rebuild, sample load...

    let sinc_params = SincInterpolationParameters {
        sinc_len:           256,
        f_cutoff:           0.95,
        interpolation:      SincInterpolationType::Linear,
        oversampling_factor: 256,
        window:             WindowFunction::BlackmanHarris2,
    };

    for voice in &mut self.voices {
        // Pre-allocate at ratio 1.0; updated at NoteOn time.
        let rs = SincFixedOut::<f32>::new(
            1.0,                    // initial ratio
            2.0,                    // max_resample_ratio_relative (allows 2× change)
            sinc_params.clone(),
            block_size,
            1,                      // channels = 1 (mono sampler voice)
        ).expect("rubato init");

        let input_frames = rs.input_frames_next();
        voice.resampler   = Some(rs);
        voice.resamp_ratio = 1.0;
        voice.resamp_in   = vec![vec![0.0f32; input_frames + block_size]; 1];
        voice.resamp_out  = vec![vec![0.0f32; block_size]; 1];
        voice.read_pos    = 0.0;
    }
}
```

`SincInterpolationType::Linear` gives a better quality-to-CPU tradeoff than
`Nearest` and is sufficient for percussive material. Override to `Cubic` if
quality requirements increase.

-----

## 2.5 NoteOn handling

When a NoteOn event triggers a voice:

```rust
fn trigger_voice(&mut self, voice: &mut SamplerVoice, note: u8, velocity: f32) {
    let root_note = self.bank.get(PARAM_ROOT_NOTE) as f64;
    let pitch_offset = self.bank.get(PARAM_PITCH);   // semitones, from P5
    let semitones  = (note as f64 - root_note) + pitch_offset;
    let ratio      = 2.0_f64.powf(semitones / 12.0);

    voice.read_pos    = 0.0;
    voice.resamp_ratio = ratio;
    voice.env_value   = 0.0;
    voice.env_phase   = EnvPhaseSimple::Attack;
    voice.active      = true;

    if let Some(rs) = &mut voice.resampler {
        // Update ratio. ramp=false: apply immediately (no interpolation).
        rs.set_resample_ratio(ratio, false)
          .expect("valid ratio");
        rs.reset();
    }
}
```

If `ratio` is within ε of 1.0 (i.e. `(ratio - 1.0).abs() < 1e-4`), the
resampler can be bypassed: read samples directly from the buffer with integer
advance, as in the P6 path. This is an optional optimisation; the resampler
also handles ratio 1.0 correctly.

-----

## 2.6 Render loop

In the per-sample render path (`render_voice()` or equivalent):

```rust
fn render_voice_block(
    voice:       &mut SamplerVoice,
    sample_data: &[f32],
    output:      &mut [f32],
    block_size:  usize,
) {
    let Some(rs) = voice.resampler.as_mut() else {
        // Bypass path (ratio ≈ 1.0): direct copy with integer advance
        // (existing P6 logic)
        render_direct(voice, sample_data, output, block_size);
        return;
    };

    // Fill input staging buffer with the required number of source samples
    let needed = rs.input_frames_next();
    let src_start = voice.read_pos as usize;
    let src_end   = (src_start + needed).min(sample_data.len());
    let available = src_end - src_start;

    // Copy available source samples; zero-pad tail (end of sample)
    voice.resamp_in[0][..available]
        .copy_from_slice(&sample_data[src_start..src_end]);
    voice.resamp_in[0][available..needed].fill(0.0);

    voice.read_pos += needed as f64;

    // Run resampler → produces exactly block_size output samples
    let (_, out_count) = rs.process_into_buffer(
        &voice.resamp_in,
        &mut voice.resamp_out,
        None,
    ).expect("rubato process");

    // Apply amplitude envelope and write to output
    for i in 0..out_count {
        let env = advance_envelope(voice);    // existing envelope logic
        output[i] += voice.resamp_out[0][i] * env * voice.velocity;
    }

    // Mark voice done if source exhausted and envelope complete
    if src_start >= sample_data.len() && voice.env_phase == EnvPhaseSimple::Done {
        voice.active = false;
    }
}
```

-----

## 2.7 Tests

```
sampler_rubato_pitch_unity_is_unchanged
    Trigger Sampler with note == root_note (ratio 1.0). Confirm output
    frequency matches the sample's fundamental (same as P6 behaviour).

sampler_rubato_pitch_up_one_octave_doubles_frequency
    Trigger with note = root_note + 12. Measure fundamental frequency of
    output via zero-crossing count. Assert frequency ≈ 2× baseline.
    Tolerance: ±5 Hz at 440 Hz reference.

sampler_rubato_root_note_param_shifts_base_pitch
    Set root_note = 48 (C3). Trigger with MIDI note 60.
    Assert output pitch ≈ one octave above a trigger at root_note == 60,
    note 60 (which would be unity).
    Confirms root_note param affects the ratio calculation.

sampler_rubato_serialize_restores_root_note
    Set root_note = 72. serialize(). Create new Sampler. deserialize().
    Confirm bank.get(PARAM_ROOT_NOTE) == 72.0.
```

-----

# Part 3: Project Save/Recall (Commit 3)

**Crates:** `paraclete-runtime` (NodeConfigurator additions),
`paraclete-app` (types, save/load functions, keyboard wiring)

Implements ADR-025 in full. No changes to node implementations.

-----

## 3.1 `ron` crate

Add to workspace `Cargo.toml`:
```toml
ron = "0.8"
```

License: MIT/Apache-2.0. No GPL interaction.

-----

## 3.2 Project types

**File:** `paraclete-app/src/project.rs`

```rust
use serde::{Serialize, Deserialize};

/// Top-level project file. Version 1 is the initial format.
#[derive(Serialize, Deserialize)]
pub struct Project {
    pub version:  u32,
    pub metadata: ProjectMetadata,
    pub graph:    GraphSnapshot,
    pub profiles: ProfileBinding,
}

#[derive(Serialize, Deserialize)]
pub struct ProjectMetadata {
    /// Human-readable project name (filename stem by default).
    pub name:    String,
    /// BPM at save time. Informational; the InternalClock node's
    /// serialized state is authoritative on load.
    pub bpm:     f32,
    /// ISO 8601 creation timestamp (UTC).
    pub created: String,
}

#[derive(Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub nodes: Vec<NodeSnapshot>,
    pub edges: Vec<EdgeRecord>,
}

/// Per-node serialised state.
#[derive(Serialize, Deserialize)]
pub struct NodeSnapshot {
    /// Stable NodeId — must match the runtime graph's node ID.
    pub id:        u32,
    /// Human-readable label. Not used for dispatch. For diagnostics only.
    pub type_name: String,
    /// Raw output of Node::serialize(). Empty slice if the node has no
    /// persistent state.
    pub state:     Vec<u8>,
}

/// One edge record. Carried for validation; runtime topology takes
/// precedence on load. See ADR-025.
#[derive(Serialize, Deserialize)]
pub struct EdgeRecord {
    pub src_node: u32,
    pub src_port: u32,
    pub dst_node: u32,
    pub dst_port: u32,
}

#[derive(Serialize, Deserialize)]
pub struct ProfileBinding {
    /// Paths (relative to project root) of active Rhai profile scripts.
    pub active: Vec<String>,
}

#[derive(Debug)]
pub enum ProjectError {
    Io(std::io::Error),
    Parse(ron::error::SpannedError),
    UnknownVersion(u32),
}

impl From<std::io::Error> for ProjectError {
    fn from(e: std::io::Error) -> Self { ProjectError::Io(e) }
}
impl From<ron::error::SpannedError> for ProjectError {
    fn from(e: ron::error::SpannedError) -> Self { ProjectError::Parse(e) }
}
```

-----

## 3.3 `NodeConfigurator` additions

**File:** `paraclete-runtime/src/configurator.rs`

```rust
impl NodeConfigurator {
    /// Iterate all registered nodes in registration order.
    /// Yields (NodeId, shared reference to the dyn Node).
    pub fn all_nodes(&self) -> impl Iterator<Item = (u32, &dyn Node)> + '_;

    /// Iterate all edges in the graph.
    pub fn all_edges(&self) -> impl Iterator<Item = EdgeRecord> + '_;
}
```

`EdgeRecord` (re-exported from `paraclete-app`) or a local mirror type in
`paraclete-runtime`. To avoid the `paraclete-runtime` → `paraclete-app`
direction (which would create a dependency cycle), define a lightweight
`EdgeView` in `paraclete-runtime` and have `paraclete-app` convert it:

```rust
// paraclete-runtime/src/configurator.rs
pub struct EdgeView {
    pub src_node: u32,
    pub src_port: u32,
    pub dst_node: u32,
    pub dst_port: u32,
}

impl NodeConfigurator {
    pub fn all_nodes(&self) -> impl Iterator<Item = (u32, &dyn Node)> + '_;
    pub fn all_edges(&self) -> impl Iterator<Item = EdgeView> + '_;
}
```

Implementation uses the existing internal node registry and the petgraph edge
iterator.

-----

## 3.4 `save_project()` and `load_project()`

**File:** `paraclete-app/src/project.rs`

```rust
/// Serialize all node states into a RON project file.
/// Called on the main thread only. The audio engine need not be stopped.
pub fn save_project(
    path:     &std::path::Path,
    conf:     &NodeConfigurator,
    metadata: ProjectMetadata,
    profiles: ProfileBinding,
) -> Result<(), ProjectError> {
    let nodes: Vec<NodeSnapshot> = conf.all_nodes()
        .map(|(id, node)| NodeSnapshot {
            id,
            type_name: node.type_name().to_string(),
            state:     node.serialize(),
        })
        .collect();

    let edges: Vec<EdgeRecord> = conf.all_edges()
        .map(|e| EdgeRecord {
            src_node: e.src_node,
            src_port: e.src_port,
            dst_node: e.dst_node,
            dst_port: e.dst_port,
        })
        .collect();

    let project = Project {
        version: 1,
        metadata,
        graph: GraphSnapshot { nodes, edges },
        profiles,
    };

    let pretty_config = ron::ser::PrettyConfig::default();
    let ron_str = ron::ser::to_string_pretty(&project, pretty_config)
        .map_err(|e| ProjectError::Io(
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        ))?;
    std::fs::write(path, ron_str)?;
    Ok(())
}

/// Restore node states from a RON project file.
/// Called on the main thread only. Node IDs in the file are matched
/// against the runtime graph; unknown IDs are skipped with a warning.
/// The audio engine need not be stopped — deserialize() is main-thread.
pub fn load_project(
    path: &std::path::Path,
    conf: &mut NodeConfigurator,
) -> Result<Vec<String>, ProjectError> {
    let ron_str = std::fs::read_to_string(path)?;
    let project: Project = ron::de::from_str(&ron_str)?;

    if project.version != 1 {
        return Err(ProjectError::UnknownVersion(project.version));
    }

    let mut warnings = Vec::new();

    for snap in &project.graph.nodes {
        match conf.node_mut(snap.id) {
            Some(node) => node.deserialize(&snap.state),
            None       => warnings.push(format!(
                "Project file references unknown node id {} ({}); skipping.",
                snap.id, snap.type_name
            )),
        }
    }

    // Edge validation (informational — runtime topology takes precedence)
    // Emit a warning if saved edge count differs from runtime edge count.
    let runtime_edge_count = conf.all_edges().count();
    if runtime_edge_count != project.graph.edges.len() {
        warnings.push(format!(
            "Saved edge count ({}) differs from runtime topology ({}). \
             Runtime topology in use.",
            project.graph.edges.len(),
            runtime_edge_count,
        ));
    }

    Ok(warnings)  // caller logs warnings; non-empty list is not an error
}
```

**`NodeConfigurator::node_mut(id: u32)`** — this method likely already exists
from the P3 era. If not, add it:

```rust
/// Mutable access to a registered node by NodeId.
/// Returns None if the id is not registered.
pub fn node_mut(&mut self, id: u32) -> Option<&mut dyn Node>;
```

-----

## 3.5 Keyboard wiring in `paraclete-app`

In the main application loop (main thread), handle save/load keyboard shortcuts:

```rust
// In paraclete-app/src/main.rs or app.rs, in the main loop event handler:

// Ctrl+S / Cmd+S — save
if key_event.ctrl && key_event.code == KeyCode::Char('s') {
    let metadata = ProjectMetadata {
        name:    current_project_name.clone(),
        bpm:     current_bpm,
        created: chrono::Utc::now().to_rfc3339(),
    };
    let profiles = current_profile_binding.clone();
    match save_project(&project_path, &conf, metadata, profiles) {
        Ok(())   => eprintln!("Project saved: {}", project_path.display()),
        Err(e)   => eprintln!("Save failed: {:?}", e),
    }
}

// Ctrl+O / Cmd+O — load
if key_event.ctrl && key_event.code == KeyCode::Char('o') {
    match load_project(&project_path, &mut conf) {
        Ok(warnings) => {
            for w in &warnings { eprintln!("WARN: {}", w); }
            eprintln!("Project loaded: {}", project_path.display());
        }
        Err(e) => eprintln!("Load failed: {:?}", e),
    }
}
```

The project file path (`project_path`) is either the last saved path, a
command-line argument, or a default location (`~/.paraclete/default.ron`).
The exact UX is application-level; what matters is that `save_project` and
`load_project` are correct.

`chrono` crate (existing workspace dependency or added here) provides
`Utc::now().to_rfc3339()`.

-----

## 3.6 Tests

All tests live in `paraclete-app/tests/project_tests.rs`.

```
project_save_creates_valid_ron_file
    Construct a minimal NodeConfigurator with one InternalClock node (id=1).
    Call save_project(tmp_path, ...).
    Assert the file exists and ron::de::from_str::<Project>() succeeds.
    Assert project.version == 1.

project_save_then_load_restores_state
    Construct an InternalClock (id=1) and a Sequencer (id=2).
    Configure the Sequencer with a non-default step pattern (CMD_SET_STEP).
    save_project(tmp_path, ...).
    Construct fresh nodes (same ids), call load_project(tmp_path, &mut conf).
    Run one executor cycle. Assert the Sequencer's step pattern matches
    the saved pattern (read via published_state or direct field access in test).

project_load_unknown_node_id_skips_with_warning
    Save a project file. Manually edit the RON to reference node id=999.
    Call load_project(). Assert result is Ok(warnings) with a non-empty
    warnings vec. Assert no panic.

project_load_unknown_version_returns_error
    Write a RON string with version: 99. Call load_project().
    Assert Err(ProjectError::UnknownVersion(99)).
```

-----

# Part 4: `paraclete-clap` Infrastructure (Commit 4)

**New crate:** `paraclete-clap` (GPL3)
**Runtime change:** `paraclete-runtime` — `NodeExecutor::set_transport_override()`

This commit creates the `paraclete-clap` crate with all shared infrastructure.
No plugin binaries yet (those are Commits 5 and 6).

-----

## 4.1 Crate structure

```
paraclete-clap/
├── Cargo.toml
├── src/
│   ├── lib.rs             (re-exports: SingleNodePlugin, SubgraphPlugin)
│   ├── plugin.rs          (CLAP entry point, factory, plugin lifecycle boilerplate)
│   ├── single_node.rs     (SingleNodePlugin impl — Commit 5)
│   ├── subgraph.rs        (SubgraphPlugin impl — Commit 6)
│   ├── bridge.rs          (ClapParamBridge)
│   ├── transport.rs       (translate_transport())
│   └── process_input.rs   (synthesise_process_input())
└── tests/
    └── clap_validator.rs  (clap-validator CI test — Commit 5)
```

**Cargo.toml:**
```toml
[package]
name    = "paraclete-clap"
version = "0.1.0"
edition = "2021"
license = "GPL-3.0"

[dependencies]
paraclete-node-api = { path = "../paraclete-node-api" }
paraclete-nodes    = { path = "../paraclete-nodes" }
paraclete-runtime  = { path = "../paraclete-runtime" }
clap-sys           = "0.3"

[dev-dependencies]
# clap-validator integration: see Commit 5
```

Remove `nih-plug` from workspace `Cargo.toml` (if present). If any existing
code references `nih_plug`, remove or replace it — there should be none at
P6.5 since nih-plug was never used in the nodes post-ADR-024 decision.

-----

## 4.2 `ClapParamBridge`

**File:** `paraclete-clap/src/bridge.rs`

```rust
use paraclete_node_api::{CapabilityDocument, ParamDescriptor};

/// Translates between CLAP parameter IDs (sequential u32, assigned at
/// plugin instantiation) and Paraclete parameter IDs (id_for_name() hash,
/// content-addressed). See ADR-024.
pub(crate) struct ClapParamBridge {
    /// (clap_id, paraclete_id, name, min, max, default)
    entries: Vec<ClapParamEntry>,
}

#[derive(Clone)]
pub(crate) struct ClapParamEntry {
    pub clap_id:      u32,
    pub paraclete_id: u32,
    pub name:         String,
    pub min:          f64,
    pub max:          f64,
    pub default_val:  f64,
}

impl ClapParamBridge {
    /// Build from a node's capability document.
    /// CLAP IDs are assigned sequentially (0, 1, 2, ...) in the order
    /// parameters appear in the document. This order is stable for the
    /// lifetime of a plugin instance and must not change between versions
    /// (parameter additions are append-only).
    pub fn from_capability_document(doc: &CapabilityDocument) -> Self {
        let entries = doc.parameters()
            .iter()
            .enumerate()
            .map(|(clap_id, param)| ClapParamEntry {
                clap_id:      clap_id as u32,
                paraclete_id: param.id,
                name:         param.name.to_string(),
                min:          param.min,
                max:          param.max,
                default_val:  param.default,
            })
            .collect();
        ClapParamBridge { entries }
    }

    /// Number of CLAP parameters exposed.
    pub fn len(&self) -> usize { self.entries.len() }

    /// Look up the Paraclete param_id for a given CLAP param ID.
    /// Returns None if clap_id is out of range.
    pub fn paraclete_id_for(&self, clap_id: u32) -> Option<u32> {
        self.entries.get(clap_id as usize).map(|e| e.paraclete_id)
    }

    /// Entry for a given CLAP param ID. Used when filling CLAP param_info.
    pub fn entry(&self, clap_id: u32) -> Option<&ClapParamEntry> {
        self.entries.get(clap_id as usize)
    }

    /// Build a NodeCommand::CMD_SET_PARAM from a CLAP ParamValue event value.
    pub fn make_set_param_command(&self, clap_id: u32, value: f64)
        -> Option<paraclete_node_api::NodeCommand>
    {
        let paraclete_id = self.paraclete_id_for(clap_id)?;
        Some(paraclete_node_api::NodeCommand {
            type_id: paraclete_node_api::CMD_SET_PARAM,
            arg0:    paraclete_id as f64,
            arg1:    value,
        })
    }
}
```

-----

## 4.3 `translate_transport()`

**File:** `paraclete-clap/src/transport.rs`

```rust
use paraclete_node_api::{TransportInfo, TransportEvent, TICKS_PER_BEAT};

/// Translate a CLAP transport event into Paraclete's (TransportInfo,
/// Option<TransportEvent>) pair. Called once per CLAP process() callback,
/// before the node or executor processes audio.
///
/// ADR-024 specifies this translation; this is the authoritative implementation.
pub fn translate_transport(
    t:           &clap_sys::clap_event_transport,
    block_size:  usize,
    sample_rate: f32,
) -> (TransportInfo, Option<TransportEvent>) {
    let playing = (t.flags & clap_sys::CLAP_TRANSPORT_IS_PLAYING) != 0;
    let bpm     = t.tempo as f32;

    // CLAP song position: beats × 2^31
    let beat_pos_f64 = t.song_pos_beats as f64 / (1u64 << 31) as f64;
    let current_tick = (beat_pos_f64 * TICKS_PER_BEAT as f64) as u64;

    let info = TransportInfo {
        bpm,
        ticks_per_beat: TICKS_PER_BEAT,
        current_tick,
        block_size,
        sample_rate,
        playing,
    };

    // Loop detection — CLAP provides loop start/end in beats.
    // Emit LoopStart / LoopEnd when looping flag is set.
    let looping = (t.flags & clap_sys::CLAP_TRANSPORT_IS_LOOP_ACTIVE) != 0;

    let event = match (playing, looping) {
        (true,  false) => Some(TransportEvent::GlobalStart),
        (false, _)     => Some(TransportEvent::Stop),
        (true,  true)  => {
            // When looping, GlobalStart is emitted on each loop iteration.
            // The Sequencer's existing loop-count logic handles this.
            Some(TransportEvent::GlobalStart)
        }
    };

    (info, event)
}
```

**Note:** `TransportEvent::GlobalStart` is emitted every process callback
while the DAW is playing. This matches how the standalone InternalClock works
— it emits `GlobalStart` each tick while playing. The Sequencer's
`cmd_loop_count` guard prevents it from resetting on every block. If a
`TransportEvent::LoopStart` variant is not yet defined in the event enum, add
it at this commit:

```rust
// In paraclete-node-api/src/event.rs — add if not present:
TransportEvent::LoopStart,
TransportEvent::LoopEnd,
```

-----

## 4.4 `synthesise_process_input()`

**File:** `paraclete-clap/src/process_input.rs`

Builds a `ProcessInput` from CLAP audio buffer pointers and the translated
transport. This is the adapter's core per-callback operation.

```rust
/// Synthesise a ProcessInput for one CLAP process() callback.
///
/// Safety: `clap_in` pointers must be valid for `block_size` samples.
/// The returned ProcessInput borrows from the caller's `commands` and
/// `events` slices; it does not own the audio data (which lives in CLAP
/// host memory).
pub(crate) unsafe fn synthesise_process_input<'a>(
    clap_in:    *const clap_sys::clap_audio_buffer,
    block_size: usize,
    transport:  TransportInfo,
    commands:   &'a [NodeCommand],
    events:     &'a [Event],
) -> ProcessInput<'a> {
    // Build audio input slice views from CLAP audio buffer pointers.
    // clap_audio_buffer.data32 is *mut *mut f32 (channel pointers).
    // For mono input (Sequencer MIDI effect): no audio inputs.
    // For subgraph output read-back: done via ProcessOutput, not ProcessInput.

    ProcessInput::from_raw(transport, commands, events, /* audio slices */)
}
```

The exact internal construction uses the same `from_raw` constructor path
established at P4.5 Fix 4 for the standalone executor. The CLAP version
wraps the C pointers in safe Rust slice references of known lifetime.

For the Sequencer single-node plugin (MIDI effect), there are no audio
inputs. `synthesise_process_input` builds an empty-audio `ProcessInput`
with transport and any incoming CLAP parameter automation events translated
to `NodeCommand` via `ClapParamBridge`.

-----

## 4.5 `plugin.rs` — CLAP entry point and factory boilerplate

**File:** `paraclete-clap/src/plugin.rs`

The CLAP plugin entry point is a static symbol. Each plugin binary has its
own `clap_entry` that references a factory for that binary's plugin type.
The factory is generic; each binary specialises it.

```rust
// Shared by both single_node.rs and subgraph.rs binaries:

/// CLAP plugin entry point. Each .clap binary exports this symbol.
/// Generated via a macro to avoid per-binary boilerplate:
#[macro_export]
macro_rules! clap_plugin_entry {
    ($descriptor:expr, $create_fn:expr) => {
        mod _clap_entry {
            use super::*;

            static FACTORY: clap_sys::clap_plugin_factory = clap_sys::clap_plugin_factory {
                get_plugin_count:      Some(factory_count),
                get_plugin_descriptor: Some(factory_descriptor),
                create_plugin:         Some(factory_create),
            };

            unsafe extern "C" fn factory_count(
                _: *const clap_sys::clap_plugin_factory
            ) -> u32 { 1 }

            unsafe extern "C" fn factory_descriptor(
                _: *const clap_sys::clap_plugin_factory, idx: u32
            ) -> *const clap_sys::clap_plugin_descriptor {
                if idx == 0 { &$descriptor } else { std::ptr::null() }
            }

            unsafe extern "C" fn factory_create(
                _: *const clap_sys::clap_plugin_factory,
                host: *const clap_sys::clap_host,
                id: *const std::ffi::c_char,
            ) -> *const clap_sys::clap_plugin {
                // Verify ID matches; call the specialised create function
                $create_fn(host, id)
            }

            unsafe extern "C" fn entry_init(_path: *const std::ffi::c_char) -> bool { true }
            unsafe extern "C" fn entry_deinit() {}
            unsafe extern "C" fn entry_get_factory(
                id: *const std::ffi::c_char
            ) -> *const std::ffi::c_void {
                if clap_sys::str_matches(id, clap_sys::CLAP_PLUGIN_FACTORY_ID) {
                    &FACTORY as *const _ as *const std::ffi::c_void
                } else {
                    std::ptr::null()
                }
            }

            #[no_mangle]
            pub static clap_entry: clap_sys::clap_plugin_entry =
                clap_sys::clap_plugin_entry {
                    clap_version: clap_sys::CLAP_VERSION,
                    init:         Some(entry_init),
                    deinit:       Some(entry_deinit),
                    get_factory:  Some(entry_get_factory),
                };
        }
    }
}
```

Each plugin binary uses this macro once with its descriptor and create function.

-----

## 4.6 `NodeExecutor::set_transport_override()`

**File:** `paraclete-runtime/src/executor.rs`

The standalone runtime uses `InternalClock` as the transport source. The
CLAP adapter needs to inject DAW transport without an `InternalClock` node in
the subgraph.

```rust
// In NodeExecutor:

/// DAW-provided transport override. When Some, this transport is used
/// for the next process() call in place of any InternalClock output.
/// Cleared after each process() call (set again before the next one).
transport_override: Option<(TransportInfo, Option<TransportEvent>)>,

impl NodeExecutor {
    /// Set DAW transport for the next process() cycle.
    /// Called by the CLAP adapter before each process() callback.
    /// Not called in standalone mode (InternalClock drives transport).
    pub fn set_transport_override(
        &mut self,
        info:  TransportInfo,
        event: Option<TransportEvent>,
    ) {
        self.transport_override = Some((info, event));
    }
}
```

In `NodeExecutor::process()`:
```rust
// At the start of each process cycle, resolve transport:
let (transport, transport_event) = self.transport_override.take()
    .unwrap_or_else(|| {
        // Standalone: read from InternalClock node's last output
        (self.current_transport.clone(), self.current_transport_event.take())
    });
// Distribute `transport` and `transport_event` to all ProcessInputs
// for this cycle (same as the existing InternalClock path).
```

The `InternalClock` node, when present in the graph, writes its output to
`self.current_transport` as a side effect of its `process()` call. When
`set_transport_override()` is called, that cached value is ignored.

-----

## 4.7 Tests

```
bridge_from_cap_doc_assigns_sequential_ids
    Build a CapabilityDocument with params ["cutoff", "resonance", "drive"].
    ClapParamBridge::from_capability_document(&doc).
    Assert bridge.len() == 3.
    Assert bridge.paraclete_id_for(0) == Some(id_for_name("cutoff")).
    Assert bridge.paraclete_id_for(1) == Some(id_for_name("resonance")).
    Assert bridge.paraclete_id_for(2) == Some(id_for_name("drive")).

bridge_unknown_clap_id_returns_none
    bridge.paraclete_id_for(99) == None.

bridge_make_set_param_command_correct
    bridge.make_set_param_command(0, 1200.0) returns Some(NodeCommand {
        type_id: CMD_SET_PARAM,
        arg0: id_for_name("cutoff") as f64,
        arg1: 1200.0,
    }).

translate_transport_playing_emits_global_start
    Call translate_transport() with flags = CLAP_TRANSPORT_IS_PLAYING.
    Assert info.playing == true.
    Assert event == Some(TransportEvent::GlobalStart).

translate_transport_stopped_emits_stop
    Call translate_transport() with flags = 0 (not playing).
    Assert info.playing == false.
    Assert event == Some(TransportEvent::Stop).
```

-----

# Part 5: `SingleNodePlugin` (Commit 5)

**File:** `paraclete-clap/src/single_node.rs`

Wraps one portable node as a CLAP instrument or MIDI effect.
The first (and for P7 only) use case: `Sequencer` as a CLAP MIDI effect.

-----

## 5.1 `SingleNodePlugin` struct

```rust
pub struct SingleNodePlugin {
    /// The wrapped node. Owned; created at plugin instantiation.
    node:        Box<dyn Node>,
    /// CLAP ↔ Paraclete parameter translation table.
    bridge:      ClapParamBridge,
    /// Cached configuration; set at `clap_plugin.init()`.
    sample_rate: f32,
    block_size:  usize,
    /// Staging buffer for NodeCommands built from CLAP param events.
    cmd_buf:     Vec<NodeCommand>,
    /// Staging buffer for Events built from CLAP note events.
    event_buf:   Vec<Event>,
}
```

The `SingleNodePlugin` is heap-allocated at `factory_create()` and passed to
the DAW as a `*const clap_plugin`. The standard pattern:

```rust
unsafe extern "C" fn factory_create_single_node(
    _host: *const clap_sys::clap_host,
    _id:   *const std::ffi::c_char,
) -> *const clap_sys::clap_plugin {
    let plugin = Box::new(SingleNodePlugin::new(/* node constructor */));
    Box::into_raw(plugin) as *const clap_sys::clap_plugin
}
```

-----

## 5.2 CLAP lifecycle

```rust
impl SingleNodePlugin {
    unsafe extern "C" fn plugin_init(plugin: *const clap_sys::clap_plugin) -> bool {
        // Retrieve SingleNodePlugin from plugin.plugin_data.
        // sample_rate and block_size are not yet known at init() — deferred
        // to the audio thread configuration step (not a CLAP lifecycle call;
        // done in activate()).
        true
    }

    unsafe extern "C" fn plugin_activate(
        plugin:      *const clap_sys::clap_plugin,
        sample_rate: f64,
        _min_frames: u32,
        max_frames:  u32,
    ) -> bool {
        let p = &mut *((*plugin).plugin_data as *mut SingleNodePlugin);
        p.sample_rate = sample_rate as f32;
        p.block_size  = max_frames as usize;
        p.bridge = ClapParamBridge::from_capability_document(
            &p.node.capability_document()
        );
        p.node.activate(p.sample_rate, p.block_size);
        true
    }

    unsafe extern "C" fn plugin_deactivate(plugin: *const clap_sys::clap_plugin) {
        let p = &mut *((*plugin).plugin_data as *mut SingleNodePlugin);
        p.node.deactivate();
    }

    unsafe extern "C" fn plugin_destroy(plugin: *const clap_sys::clap_plugin) {
        // Drop the plugin by reconstructing the Box and letting it fall out of scope.
        let _ = Box::from_raw((*plugin).plugin_data as *mut SingleNodePlugin);
    }

    unsafe extern "C" fn plugin_process(
        plugin:   *const clap_sys::clap_plugin,
        process:  *const clap_sys::clap_process,
    ) -> clap_sys::clap_process_status {
        let p = &mut *((*plugin).plugin_data as *mut SingleNodePlugin);

        // 1. Translate transport
        let (transport, transport_event) = if !(*process).transport.is_null() {
            translate_transport(
                &*(*process).transport,
                (*process).frames_count as usize,
                p.sample_rate,
            )
        } else {
            (TransportInfo::default(), None)
        };

        // 2. Translate CLAP input events → NodeCommands + Events
        p.cmd_buf.clear();
        p.event_buf.clear();
        let in_events = (*process).in_events;
        let count = ((*in_events).size.unwrap())(in_events);
        for i in 0..count {
            let header = &*((*in_events).get.unwrap())(in_events, i);
            match header.type_ {
                clap_sys::CLAP_EVENT_PARAM_VALUE => {
                    let ev = &*(header as *const _ as *const clap_sys::clap_event_param_value);
                    if let Some(cmd) = p.bridge.make_set_param_command(ev.param_id, ev.value) {
                        p.cmd_buf.push(cmd);
                    }
                }
                clap_sys::CLAP_EVENT_NOTE_ON => {
                    let ev = &*(header as *const _ as *const clap_sys::clap_event_note);
                    p.event_buf.push(Event::NoteOn {
                        channel:  ev.channel as u8,
                        note:     ev.key as u8,
                        velocity: (ev.velocity * 127.0) as u8,
                    });
                }
                clap_sys::CLAP_EVENT_NOTE_OFF => {
                    let ev = &*(header as *const _ as *const clap_sys::clap_event_note);
                    p.event_buf.push(Event::NoteOff {
                        channel: ev.channel as u8,
                        note:    ev.key as u8,
                    });
                }
                _ => { /* ignore other event types */ }
            }
        }
        if let Some(ev) = transport_event {
            p.event_buf.push(Event::Transport(ev));
        }

        // 3. Synthesise ProcessInput and call node.process()
        let input  = synthesise_process_input(
            (*process).audio_inputs,
            (*process).frames_count as usize,
            transport,
            &p.cmd_buf,
            &p.event_buf,
        );
        let mut output_buf = /* pre-allocated audio output scratch */;
        let mut proc_output = ProcessOutput::from_scratch(&mut output_buf);
        p.node.process(&input, &mut proc_output);

        // 4. Write audio output to CLAP output buffers (for MIDI effect: no audio)
        // 5. Write MIDI note output events from proc_output to CLAP out_events
        //    (Sequencer emits NoteOn/NoteOff events at step times)

        clap_sys::CLAP_PROCESS_CONTINUE
    }
}
```

-----

## 5.3 CLAP extensions

**Params extension** (`clap_plugin_params`):
```
get_count()    → bridge.len() as u32
get_info(idx)  → fill clap_param_info from bridge.entry(idx)
get_value(id)  → node.bank().get(bridge.paraclete_id_for(id)?) as f64
                 (cast to f64 for CLAP)
value_to_text  → format!("{:.3}", value)  // plain numeric for P7
text_to_value  → value.parse::<f64>().ok()
flush()        → process CMD_SET_PARAM commands from in_events
```

**Note ports extension** (`clap_plugin_note_ports`):
```
// For Sequencer as MIDI effect:
count(is_input=true)  → 1  (receives MIDI notes from DAW)
count(is_input=false) → 1  (outputs MIDI notes at step triggers)
get(is_input=true,  idx=0) → { id=0, name="MIDI In",  preferred_dialect=CLAP_NOTE_DIALECT_MIDI }
get(is_input=false, idx=0) → { id=0, name="MIDI Out", preferred_dialect=CLAP_NOTE_DIALECT_MIDI }
```

**State extension** (`clap_plugin_state`):
```
save(stream)  → node.serialize() → write bytes to stream
load(stream)  → read bytes from stream → node.deserialize(&bytes)
```

ADR-025 notes this mapping explicitly: no additional serialization format
needed.

-----

## 5.4 Sequencer plugin binary

The Sequencer plugin binary is a separate Cargo target in `paraclete-clap`:

```toml
[[bin]]
name    = "paraclete_sequencer"
path    = "src/bin/sequencer.rs"
crate-type = ["cdylib"]   # .clap is a shared library
```

`src/bin/sequencer.rs`:
```rust
use paraclete_clap::{SingleNodePlugin, clap_plugin_entry};
use paraclete_nodes::Sequencer;

// Descriptor (static strings, ASCII, null-terminated via cstr! macro):
static SEQUENCER_DESCRIPTOR: clap_sys::clap_plugin_descriptor = clap_sys::clap_plugin_descriptor {
    clap_version: clap_sys::CLAP_VERSION,
    id:           b"com.paraclete.sequencer\0".as_ptr() as _,
    name:         b"Paraclete Sequencer\0".as_ptr() as _,
    vendor:       b"Paraclete\0".as_ptr() as _,
    features:     /* CLAP_PLUGIN_FEATURE_NOTE_EFFECT */,
    // ... other fields
};

clap_plugin_entry!(SEQUENCER_DESCRIPTOR, create_sequencer_plugin);

fn create_sequencer_plugin(
    _host: *const clap_sys::clap_host,
    _id:   *const std::ffi::c_char,
) -> *const clap_sys::clap_plugin {
    let p = Box::new(SingleNodePlugin::new(Box::new(Sequencer::new())));
    Box::into_raw(p) as _
}
```

-----

## 5.5 `clap-validator` CI test

**File:** `paraclete-clap/tests/clap_validator.rs`

```rust
/// CI integration test: validates the Sequencer plugin binary with
/// clap-validator. Requires the plugin binary to be built first.
/// Run with: cargo test --test clap_validator -- --ignored
/// (tagged ignored so it does not block unit-test runs that haven't built
/// the cdylib target)
#[test]
#[ignore]
fn clap_validator_sequencer_passes() {
    let binary = std::env::var("PARACLETE_CLAP_BINARY")
        .unwrap_or_else(|_| "target/debug/paraclete_sequencer.clap".to_string());

    let status = std::process::Command::new("clap-validator")
        .arg("validate")
        .arg(&binary)
        .status()
        .expect("clap-validator not found — install with: cargo install clap-validator");

    assert!(status.success(), "clap-validator reported failures");
}
```

-----

## 5.6 Tests

```
single_node_plugin_init_activate_deactivate_no_panic
    Construct a SingleNodePlugin wrapping a Sequencer.
    Call activate(44100.0, 512), then deactivate(). Assert no panic.
    Confirms the lifecycle path compiles and does not crash.

single_node_plugin_process_passes_transport_to_node
    Create a SingleNodePlugin wrapping a Sequencer.
    Activate. Synthesise a mock CLAP process with playing=true, bpm=120.
    Call plugin_process(). Confirm no panic.
    (Full Sequencer output verification is in the executor integration tests;
    this verifies the CLAP → ProcessInput translation path.)

single_node_plugin_state_roundtrip
    Configure a Sequencer with a non-default step pattern.
    Call plugin state save() → bytes.
    Construct fresh SingleNodePlugin. Call plugin state load(bytes).
    Activate. Process one cycle. Assert the Sequencer's pattern matches
    the saved pattern.
    (This is the critical test for the CLAP state extension correctness.)
```

-----

# Part 6: `SubgraphPlugin` + Machine Bank Plugins (Commit 6)

**File:** `paraclete-clap/src/subgraph.rs`

Wraps a `NodeExecutor` subgraph as a single CLAP instrument. Used for all
five machine bank plugins.

-----

## 6.1 `SubgraphPlugin` struct

```rust
pub struct SubgraphPlugin {
    /// The subgraph executor. Drives Sequencer + generator node.
    executor:     NodeExecutor,
    /// NodeId of the generator node whose parameters are exposed as
    /// CLAP parameters.
    exposed_node: u32,
    /// Parameter bridge for the exposed node.
    bridge:       ClapParamBridge,
    sample_rate:  f32,
    block_size:   usize,
    cmd_buf:      Vec<NodeCommand>,
    event_buf:    Vec<Event>,
    /// Pre-allocated audio output scratch (block_size stereo frames).
    audio_out:    Vec<f32>,
}
```

-----

## 6.2 Machine bank subgraph construction

For each machine bank plugin, the subgraph is:

```
[Sequencer] → [GeneratorNode] → (audio output)
```

No `InternalClock` — DAW transport is injected via
`executor.set_transport_override()`.

```rust
fn build_machine_bank_subgraph(
    generator: Box<dyn Node>,
    sample_rate: f32,
    block_size:  usize,
) -> (NodeExecutor, u32 /* exposed_node_id */) {
    let mut conf = NodeConfigurator::new();

    let seq_id  = conf.add_node(Box::new(Sequencer::new()));
    let gen_id  = conf.add_node(generator);

    // Sequencer audio output → generator note/event input
    conf.connect(seq_id, Sequencer::PORT_EVENT_OUT,
                 gen_id, /* generator event in port */).unwrap();

    // Generator audio output → plugin audio output (no MixNode for
    // single-voice machine bank; SubgraphPlugin reads gen_id's output directly)

    let executor = conf.build_executor(sample_rate, block_size);
    (executor, gen_id)
}
```

The `SubgraphPlugin::process()` callback:
1. Calls `executor.set_transport_override(transport, event)` with the DAW transport.
2. Translates CLAP param automation events for `exposed_node` to `NodeCommand`
   via `bridge.make_set_param_command()`, injected into the executor's command
   queue for `exposed_node`.
3. Calls `executor.process()`.
4. Reads audio output from `exposed_node`'s audio output port.
5. Writes audio to CLAP audio output buffers.

-----

## 6.3 CLAP extensions for `SubgraphPlugin`

Same extension set as `SingleNodePlugin`:

**Params:** Parameters from `exposed_node.capability_document()` only.
The Sequencer's parameters (step data etc.) are not exposed at P7.

**Audio ports extension** (`clap_plugin_audio_ports`):
```
count(is_input=false)  → 1  (mono or stereo audio output)
get(is_input=false, idx=0) → { id=0, name="Audio Out", channel_count=1 (mono) }
```

No audio input (machine banks are instruments, not effects).

**State extension:** Serialises the entire subgraph state.
The `SubgraphPlugin::serialize()` calls `Node::serialize()` on each node in
the executor and concatenates them with a simple framing:
```
[4 bytes: node_count]
  for each node: [4 bytes: node_id] [4 bytes: blob_len] [blob_len bytes: state]
```
This is a SubgraphPlugin-internal format; it does not need to match the
project file format (ADR-025 covers the standalone application, not the CLAP
plugin).

-----

## 6.4 Five machine bank plugin targets

Five `[[bin]]` targets in `paraclete-clap/Cargo.toml`, one per machine:

```toml
[[bin]]
name = "paraclete_kick"
path = "src/bin/kick.rs"
crate-type = ["cdylib"]

[[bin]]
name = "paraclete_snare"
path = "src/bin/snare.rs"
crate-type = ["cdylib"]

[[bin]]
name = "paraclete_fm_kick"
path = "src/bin/fm_kick.rs"
crate-type = ["cdylib"]

[[bin]]
name = "paraclete_fm_bell"
path = "src/bin/fm_bell.rs"
crate-type = ["cdylib"]

[[bin]]
name = "paraclete_fm_bass"
path = "src/bin/fm_bass.rs"
crate-type = ["cdylib"]
```

Each binary constructs its `SubgraphPlugin` with the matching generator:

| Binary | Generator | Exposed parameters |
|--------|-----------|-------------------|
| `paraclete_kick` | `AnalogEngine::kick()` | `tune`, `punch`, `decay`, `drive`, `tone` |
| `paraclete_snare` | `AnalogEngine::snare()` | `tune`, `snap`, `noise`, `decay`, `tone` |
| `paraclete_fm_kick` | `FmEngine::kick()` | `tune`, `punch`, `decay`, `feedback`, `drive` |
| `paraclete_fm_bell` | `FmEngine::bell()` | `tune`, `ratio`, `index`, `decay`, `feedback` |
| `paraclete_fm_bass` | `FmEngine::bass()` | `tune`, `ratio`, `index`, `attack`, `decay`, `drive` |

These parameter sets are **specified in ADR-022** and are already implemented
in the respective `capability_document()` methods. No node changes.

Example (`src/bin/kick.rs`):
```rust
use paraclete_clap::{SubgraphPlugin, clap_plugin_entry};
use paraclete_nodes::AnalogEngine;

static KICK_DESCRIPTOR: clap_sys::clap_plugin_descriptor = clap_sys::clap_plugin_descriptor {
    id:      b"com.paraclete.kick-machine\0".as_ptr() as _,
    name:    b"Paraclete Kick Machine\0".as_ptr() as _,
    vendor:  b"Paraclete\0".as_ptr() as _,
    features: /* CLAP_PLUGIN_FEATURE_INSTRUMENT */,
    // ...
};

clap_plugin_entry!(KICK_DESCRIPTOR, create_kick_plugin);

fn create_kick_plugin(
    _host: *const clap_sys::clap_host,
    _id:   *const std::ffi::c_char,
) -> *const clap_sys::clap_plugin {
    let p = Box::new(SubgraphPlugin::new(
        Box::new(AnalogEngine::kick()),
        /* sample_rate and block_size filled in at activate() */
    ));
    Box::into_raw(p) as _
}
```

-----

## 6.5 Tests

```
subgraph_plugin_activate_process_produces_audio
    Construct SubgraphPlugin wrapping AnalogEngine::kick().
    activate(44100.0, 512).
    Inject a NoteOn event (note=60, velocity=127) in event_buf.
    Call subgraph_process() with playing=true transport.
    Assert audio output is non-silent (max sample > 1e-5) after 3 process blocks.

subgraph_plugin_param_command_reaches_generator
    Construct SubgraphPlugin wrapping AnalogEngine::kick().
    Activate. Call plugin_process() with a CLAP ParamValue event setting
    decay to 0.01 (very short).
    Process 10 blocks.
    Assert audio output decays to silence within ~441 samples.
    (Verifies the CLAP automation → ClapParamBridge → NodeCommand → bank path.)

subgraph_plugin_state_roundtrip
    Configure kick machine with a non-default decay (0.5 s).
    Serialize SubgraphPlugin state → bytes.
    Construct fresh SubgraphPlugin. Deserialize bytes.
    Activate. Process one block with NoteOn.
    Assert audio decays at ~0.5s rate (not default).

fm_kick_plugin_produces_audio
    Same as subgraph_plugin_activate_process_produces_audio for FmEngine::kick().

fm_bell_plugin_long_decay
    Construct SubgraphPlugin wrapping FmEngine::bell().
    Inject NoteOn. Process 200 blocks (≈ 2.3 s at 44100/512).
    Assert audio is still non-silent at block 150.
    (FmBellMachine default decay ≥ 1.5 s — P6 done criterion.)

fm_bass_plugin_responds_to_pitch
    Construct SubgraphPlugin wrapping FmEngine::bass().
    Inject NoteOn note=60. Record peak frequency.
    Inject NoteOn note=72. Record peak frequency.
    Assert frequency ratio ≈ 2.0 (one octave).
```

-----

# Part 7: crates.io Publication Prep (Commit 7)

**Crate:** `paraclete-node-api`

This commit prepares `paraclete-node-api` for its first public release.
No new types or methods. Documentation and metadata only, except for the
version bump.

-----

## 7.1 Version bump

In `paraclete-node-api/Cargo.toml`:
```toml
[package]
name    = "paraclete-node-api"
version = "0.1.0"         # bumped from 0.0.x
license = "LGPL-3.0"
description = "Node API for the Paraclete audio processing platform"
repository  = "https://github.com/paraclete-audio/paraclete"
keywords    = ["audio", "dsp", "plugin", "clap", "node"]
categories  = ["multimedia::audio", "api-bindings"]
```

-----

## 7.2 API surface audit

All public types in `paraclete-node-api` are reviewed for stability.
The following are confirmed stable at v0.1.0:

| Type / fn | Status |
|-----------|--------|
| `Node` trait (all methods) | **Stable** |
| `ProcessInput` / `ProcessOutput` | **Stable** |
| `CapabilityDocument` / `ParamDescriptor` | **Stable** |
| `ParameterBank` | **Stable** |
| `NodeCommand` / `CMD_SET_PARAM` / `CMD_BUMP_PARAM` | **Stable** |
| `Event` enum (all variants used at P6.5) | **Stable** |
| `TransportInfo` / `TransportEvent` | **Stable** |
| `PortDescriptor` / `PortType` / `PortDirection` | **Stable** |
| `ConnectionAgreement` / `Negotiable` | **Stable** |
| `id_for_name()` | **Stable** — hash function is a permanent contract |
| Signal port accessors (`modulation()`, `logic()`, `mod_output_mut()`, etc.) | **Stable** |
| MIDI re-exports (`paraclete_node_api::midi::*`) | **Stable** |
| `type_name()` (added Commit 1) | **Stable** |
| `published_state(buf)` (changed Commit 1) | **Stable** |

No types are removed. No types are marked `#[doc(hidden)]` without justification.

**Breaking change guard:** Before publication, verify with `cargo semver-checks`
(or equivalent) that no accidental breaking changes exist relative to the
last internal version.

-----

## 7.3 Documentation pass

All public items receive a doc comment if not already present. Minimum
standard: one sentence explaining what the item is and when to use it.

Priority items (check these explicitly):

```rust
/// Called once per audio buffer. The primary DSP method.
/// `input` provides audio, events, commands, and transport for this block.
/// `output` receives audio and signal outputs.
/// Must be real-time safe: no allocation, no blocking, no I/O.
fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput);

/// Pre-allocate resources. Called once before the first process() call.
/// Allocate any Vec or buffer here. Do not allocate in process().
fn activate(&mut self, sample_rate: f32, block_size: usize);

/// The node's declared parameter and port surface.
/// Called at connection time and by the platform for introspection.
/// Must not allocate in process(); cache the result at activate() if needed.
fn capability_document(&self) -> CapabilityDocument;

/// Content-addressed parameter ID. Hash of the name string.
/// Stable across versions: `id_for_name("cutoff")` always returns the same
/// value. The canonical parameter names are defined in ADR-019.
pub const fn id_for_name(name: &str) -> u32;
```

-----

## 7.4 README.md for `paraclete-node-api`

Create `paraclete-node-api/README.md`:

```markdown
# paraclete-node-api

The public node contract for the [Paraclete](https://github.com/paraclete-audio/paraclete)
audio processing platform.

Implement `Node` to write a portable audio processing node. Nodes written against
this API are compatible with the Paraclete runtime, any `paraclete-clap` plugin
wrapper, and any future platform host.

## License

LGPL-3.0. Third-party nodes may link against this crate and distribute
under their own license. See `LICENSE-LGPL`.

## Example

```rust
use paraclete_node_api::*;

pub struct GainNode { bank: ParameterBank }

impl Node for GainNode {
    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.bank = ParameterBank::from_capability_document(
            &self.capability_document()
        );
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands());
        let gain = self.bank.get(id_for_name("gain")) as f32;
        let inp  = input.audio_inputs().get(0).copied().unwrap_or(&[]);
        let out  = output.audio_output_mut(0);
        for (o, &s) in out.iter_mut().zip(inp.iter()) { *o = s * gain; }
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument::builder()
            .param("gain", 0.0, 2.0, 1.0)
            .audio_in(0, "in")
            .audio_out(0, "out")
            .build()
    }
}
\```
```

-----

## 7.5 Dry-run verification

Before marking Commit 7 done:

```bash
# Verify the package is publishable
cargo publish --dry-run -p paraclete-node-api

# Verify documentation builds without warnings
cargo doc --no-deps -p paraclete-node-api 2>&1 | grep -i warn

# Run all tests one final time
cargo test --workspace
```

All three commands must exit cleanly.

-----

# P7 Done Criteria

All criteria must pass before P7 is marked complete.

## Functionality

- `InternalClock::published_state()` calls `buf.push()` instead of returning
  a Vec; runtime state_bufs[0].capacity() is stable across cycles 2+ (OQ-9 resolved)
- `save_project()` writes a valid RON file for the P6 graph (13 nodes, 8+
  tracks wired)
- `load_project()` on the saved file restores all Sequencer step patterns
  and all AnalogEngine/FmEngine machine parameters without error
- Loading a project file with an unknown node ID produces `Ok(warnings)`
  with a non-empty warnings list — no panic, no failed load
- Sampler triggered at MIDI note root_note+12 produces output at 2× the
  fundamental frequency of root_note (±5 Hz at 440 Hz reference)
- `SingleNodePlugin` wrapping a Sequencer initialises, processes, and
  de-initialises without panic under the clap-validator lifecycle test
- `SubgraphPlugin` wrapping `AnalogEngine::kick()` produces non-silent
  audio output when NoteOn events are injected and DAW transport is playing
- All five machine bank plugin targets build to `.clap` shared libraries
  without linker errors
- `cargo publish --dry-run -p paraclete-node-api` exits 0

## Architecture

- `cargo tree -p paraclete-nodes` still shows no dependency on
  `paraclete-runtime`, `paraclete-scripting`, or any L0 crate (portability
  rule unchanged through P7)
- `cargo tree -p paraclete-node-api` shows no dependency on
  `paraclete-nodes` or `paraclete-runtime` (the LGPL3 boundary is clean)
- `paraclete-clap/Cargo.toml` license field is `GPL-3.0`
- nih-plug is not present in the workspace dependency tree

## Tests

| State | Tests |
|-------|-------|
| P6.5 ship | 289 |
| Commit 1 (type_name, published_state migration) | +2 → 291 |
| Commit 2 (rubato per-voice) | +4 → 295 |
| Commit 3 (project save/recall) | +4 → 299 |
| Commit 4 (CLAP infrastructure) | +5 → 304 |
| Commit 5 (SingleNodePlugin) | +3 → 307 |
| Commit 6 (SubgraphPlugin + machine banks) | +6 → 313 |
| Commit 7 (publication prep) | +0 → 313 |
| **P7 target** | **≥ 310** |

All 289 existing tests continue to pass unchanged through every commit.

The `clap_validator_sequencer_passes` test (Commit 5) is tagged `#[ignore]`
and is not counted in the 313 total. It runs as a separate CI step.

-----

# P7 Known Gaps and Deferred Items

**AnalogEngine / FmEngine polyphony (deferred to P8+):**
Both engines are monophonic with retrigger at P7. The melodic bass track
(FmBassMachine) retriggers on overlapping notes. Polyphonic voice management
for synthesis engines requires a voice-allocation layer. Deferred; not
blocking any P7 deliverable.

**HiHatMachine plugin (deferred):**
`AnalogEngine::hihat()` is not included in the machine bank `.clap` targets
for P7. ADR-024's machine list has five entries. The hihat machine can be
added at P8 or whenever a sixth plugin target is warranted.

**Sequencer step programming via CLAP (deferred to P8+):**
The machine bank SubgraphPlugin exposes the generator node's parameters but
not the Sequencer's step data. There is no CLAP mechanism to program
individual sequencer steps in P7. Step editing requires the full standalone
Paraclete application. A CLAP custom extension for step programming is a
future concern.

**LFO tempo sync (deferred):**
`LfoNode` runs free (no TransportInfo subscription). Beat-synced LFO rates
require `TransportInfo` input. In a CLAP plugin context, the executor now
provides TransportInfo via `set_transport_override()`, so the prerequisite
infrastructure exists. The LfoNode change itself is deferred — not blocking P7.

**LadderFilterNode HP/BP modes (deferred):**
Ladder topology LP only. HP and BP modes derivable from ladder state but
not commonly found in analog hardware equivalents. Deferred.

**Signed micro-timing in Sequencer (deferred):**
Negative `micro_offset` pushes events later (same as positive). Full
signed micro-timing requires splitting emission across the step boundary.
Deferred from P5 and P6; still deferred at P7.

**4-operator FM algorithms (deferred):**
FmEngine uses 2-operator PM. 4-operator algorithms (Digitone topology)
require algorithm routing. Deferred.

**CLAP GUI extension (deferred to P9+):**
No GUI is exposed by the P7 CLAP plugins. The CLAP GUI extension
(`clap_plugin_gui`) and full-window plugin UI are deferred until the
P9 GUI milestone.

**Dynamic graph topology from project file (deferred to P9+):**
ADR-025 notes: the fixed-topology load model means project files are not
portable across binary versions that reorder node construction. This is
acceptable through P8. A topology-from-file model is P9+ when the graph
becomes user-configurable at runtime.

**`paraclete-nodes` crates.io publication (deferred):**
Only `paraclete-node-api` ships to crates.io at P7. `paraclete-nodes` (GPL3,
first-party nodes) may be published in a future phase when there is demand
for it as a standalone crate. Not blocking P7.
