# ADR-027: CLAP Host Architecture

**Date:** June 2026  
**Status:** Accepted — amended June 2026 (P8 implementation)

**Amendment (P8):** The `clack` crate referenced in the original ADR is a
placeholder on crates.io and is not a usable library. The P8 implementation
uses `clap-sys 0.5` (raw C bindings, same crate used in `paraclete-clap`) and
`libloading 0.8` (cross-platform dynamic library loading). All `clack`
references in this document have been replaced. ADR-024's mention of `clack`
as the future host-side dependency is also superseded.

-----

## Decision

Paraclete loads third-party `.clap` plugins as first-class `Node` instances
in the graph. A new `paraclete-clap-host` crate (GPL3) provides
`PluginLibrary` (loads a `.clap` shared library), `PluginDescriptor`
(plugin metadata), and `PluginNode` (wraps a loaded plugin as a `Node`).
`clap-sys` is used for CLAP FFI types and `libloading` for dynamic library
loading. P8 scope is generator plugins (audio output only); effect plugins
(audio input → audio output) are deferred to P9.

-----

## Context

ADR-003 established CLAP as the primary plugin format. ADR-024 established
`paraclete-clap` as the plugin-side adapter (Paraclete as a CLAP plugin
consumed by a DAW). That is the opposite direction from what this ADR covers.

**This ADR addresses Paraclete as a CLAP host** — the platform loads
third-party `.clap` plugins and presents each one as a `Node` in the audio
graph. A plugin node wired into the graph is indistinguishable from a
first-party node from the routing and parameter control perspective.

The roadmap has listed "CLAP Host" as the P8 scope deliverable from the
start. ADR-024 deferred this work explicitly with a reference to the `clack`
crate that proved incorrect; this ADR specifies the actual implementation.

-----

## Why a separate crate rather than extending `paraclete-clap`

`paraclete-clap` is the plugin-side adapter. Its modules synthesise
`ProcessInput` from CLAP buffer pointers and write `ProcessOutput` back.
The direction of translation is: CLAP in → Paraclete node.

The host-side adapter does the opposite: Paraclete events → CLAP buffers,
CLAP buffers → `ProcessOutput`. These are not symmetric operations; the
data models, CLAP extensions involved, and threading contracts differ.

Mixing plugin and host code in one crate would conflate two distinct roles.
A separate `paraclete-clap-host` crate makes the architecture explicit:
`paraclete-clap` is for plugins Paraclete exports; `paraclete-clap-host`
is for plugins Paraclete imports.

-----

## Why clap-sys + libloading for the host side

`clap-sys` (raw C bindings) is already proven in the codebase as the plugin-
side FFI layer (Commit 1 of P8). Using the same crate for the host side keeps
the workspace consistent and avoids a second CLAP ABI dependency.

`libloading` provides safe cross-platform dynamic library loading (Linux `dl`,
macOS `dyld`, Windows `LoadLibrary`). It handles the OS differences in `.clap`
file layout (macOS `.clap` bundles are directories containing a Mach-O binary
at `Contents/MacOS/<stem>`; Linux and Windows are plain shared library files).
It is MIT/Apache-2 licensed.

The combination gives a thin, correct FFI surface with no framework coupling.
The unsafe scope is contained: `PluginLibrary::load()` calls unsafe FFI to
initialise the CLAP entry point; `PluginNode::process()` calls the plugin's
audio callback; all other code is safe Rust.

-----

## P8 scope: generator plugins only

A generator plugin produces audio (audio output ports) from events (MIDI note
in). This maps directly to the existing generator node pattern: a `Node` with
event input and audio output.

An effect plugin also has audio input ports. Deferring effect plugins to P9
keeps P8 scope manageable. The P8 implementation notes the architectural
extension point clearly.

-----

## How `PluginNode` implements `Node`

`PluginNode` implements the `Node` trait entirely. The `capability_document()`
is synthesised from the CLAP plugin's declared parameter list; the resulting
`CapabilityDocument` uses `id_for_name(param_name_string)` as the Paraclete
param ID for each CLAP parameter. If the plugin uses canonical parameter names
from ADR-019 (e.g. `"cutoff"`, `"decay"`), those parameters become reachable
by any encoder mapped to `id_for_name("cutoff")` with no platform changes.

```rust
impl Node for PluginNode {
    fn activate(&mut self, sample_rate: f32, block_size: usize);
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput);
    fn deactivate(&mut self);
    fn capability_document(&self) -> CapabilityDocument;
    fn type_name(&self) -> &'static str { "PluginNode" }
    fn serialize(&self)   -> Vec<u8>  { /* CLAP state ext if present */ }
    fn deserialize(&mut self, data: &[u8]) { /* CLAP state ext if present */ }
}
```

`PluginNode` only depends on `paraclete-node-api` (LGPL3). It does not
depend on `paraclete-nodes` or `paraclete-runtime`. The portability rule
(ADR-022) applies to node crates; `paraclete-clap-host` is a platform crate
and is not subject to the node portability rule.

-----

## Parameter bridge: host side vs plugin side

`ClapParamBridge` in `paraclete-clap` (plugin side) maps sequential CLAP IDs
(0, 1, 2… assigned at plugin export time) to Paraclete `id_for_name` hashes.

`HostParamBridge` in `paraclete-clap-host` (host side) maps the third-party
plugin's native CLAP param IDs (the plugin author's choice, not sequential,
arbitrary u32 values) to Paraclete `id_for_name` hashes. The hash is derived
from the param name string the plugin declares. Two plugins that both declare
`"cutoff"` map to the same Paraclete param ID.

`HostParamBridge` also synthesises the `CapabilityDocument` for `PluginNode`.
The bridge is built once at instantiation time.

`CMD_SET_PARAM` and `CMD_BUMP_PARAM` are applied via a `ParameterBank` built
from the synthesised `CapabilityDocument` at `activate()` time. Changed
parameter values are flushed to the plugin via `CLAP_EVENT_PARAM_VALUE` events
prepended to the input event list in `process()` (dirty-detection via a
parallel `flushed_values` Vec in `HostParamBridge`).

-----

## Plugin discovery

`scan_clap_paths()` returns `Vec<PathBuf>` of `.clap` files/directories found
in OS-standard locations:

| OS | Directories |
|----|-------------|
| Linux | `~/.clap/`, `/usr/lib/clap/`, `/usr/local/lib/clap/` |
| macOS | `~/Library/Audio/Plug-Ins/CLAP/`, `/Library/Audio/Plug-Ins/CLAP/` |
| Windows | `%LOCALAPPDATA%\Programs\Common\CLAP\`, `%COMMONPROGRAMFILES%\CLAP\` |

A `.clap` path can also be declared explicitly in the instrument definition
file (ADR-026), bypassing the scan.

**macOS bundle handling:** On macOS, `.clap` bundles are directories. When
`PluginLibrary::load()` receives a directory path, it resolves the actual
shared library at `<bundle>/<stem>.clap/Contents/MacOS/<stem>` before calling
`libloading::Library::new()`.

-----

## Threading contracts

CLAP imposes strict thread requirements. `PluginNode` maps them directly to
the Paraclete `Node` lifecycle, which already satisfies them:

| Operation | CLAP thread | `Node` method |
|-----------|------------|---------------|
| `init()` | main thread | inside `PluginLibrary::instantiate()` |
| `activate()` | main thread | `Node::activate()` |
| `process()` | audio thread | `Node::process()` |
| `stop_processing()` → `deactivate()` | main thread | `Node::deactivate()` |
| `destroy()` | main thread | `Drop for PluginNode` |

**CLAP lifecycle invariant:** `stop_processing()` must be called before
`deactivate()`, and `deactivate()` before `destroy()`. `PluginNode::Drop`
enforces the full order: `stop_processing → deactivate → destroy`.
Omitting `stop_processing` before `deactivate` is a CLAP lifecycle violation
that surfaced as a code review finding during P8 implementation.

**Library lifetime:** `LibraryHandle` (wrapping `libloading::Library`) is
`Arc`-shared between `PluginLibrary` and all derived `PluginNode` instances.
The library is kept alive — and its `deinit()` entry point called on drop —
only when all derived nodes are also dropped. Multi-plugin bundles use
`Arc::clone` so all plugins from the same library share the handle.

-----

## LGPL3 / GPL3 boundary

| Crate | License | Notes |
|-------|---------|-------|
| `paraclete-node-api` | LGPL3 | `PluginNode` implements this |
| `paraclete-clap-host` | GPL3 | New crate; depends on clap-sys + libloading |
| `clap-sys` | MIT | Raw CLAP C bindings |
| `libloading` | MIT/Apache-2 | Cross-platform dynamic library loading |

-----

## Crate structure

```
paraclete-clap-host/
├── Cargo.toml             (GPL3; deps: paraclete-node-api, clap-sys, libloading)
└── src/
    ├── lib.rs             (pub re-exports: PluginLibrary, PluginDescriptor,
    │                       PluginNode, HostError, scan_clap_paths)
    ├── scan.rs            (scan_clap_paths() — OS-conditional)
    ├── library.rs         (PluginLibrary, LibraryHandle, PluginDescriptor)
    ├── bridge.rs          (HostParamBridge — native CLAP IDs → CapabilityDocument)
    └── node.rs            (PluginNode — impl Node)
```

-----

## Deferred

- **Effect plugins (audio input):** P9.
- **CLAP GUI extension:** P9+.
- **CLAP note expressions:** P9+.
- **Full host callbacks** (latency change, request restart, log, timer): no-ops at P8; P9+.
- **Preset management** (`clap_plugin_preset_discovery`): P9+.
- **Hot-loading / rescan at runtime:** startup-only at P8; P9+.
- **Multi-channel audio output:** mono mix-down at P8; P9.
- **`SubgraphPlugin` machine slot swapping:** depends on `GraphNode` (ADR-023), P9.

-----

## Consequences

**P8:**
- `paraclete-clap-host` crate created as described
- `clap-sys` (existing workspace dep) and `libloading` added to workspace
- `scan_clap_paths()` implemented (OS-conditional)
- `PluginLibrary::load()` + `PluginLibrary::instantiate()` implemented
- `PluginNode` implements `Node`; generator plugins usable in graph
- Instrument definition YAML supports `clap_plugin` node type

**Ongoing:**
- Any third-party plugin declaring canonical parameter names (ADR-019) is
  automatically reachable by existing hardware profiles
- CLAP state extension maps to `Node::serialize()` / `Node::deserialize()`
  — project save/recall (ADR-025) works for plugins that support it

-----

## References

- ADR-001 — License (GPL3 / LGPL3 boundary)
- ADR-003 — Plugin format (CLAP primary)
- ADR-009 — Clock federation
- ADR-014 — Capability document
- ADR-019 — Universal parameter control (canonical names, HostParamBridge)
- ADR-022 — Node portability
- ADR-024 — CLAP plugin wrapper (plugin side; clack reference therein superseded)
- ADR-026 — Instrument definition and TUI (clap_plugin in instrument.yaml)
- `clap-sys` — raw CLAP C API bindings
- `libloading` — cross-platform dynamic library loading
