# ADR-027: CLAP Host Architecture

**Date:** June 2026  
**Status:** Accepted — P8 implementation target

-----

## Decision

Paraclete loads third-party `.clap` plugins as first-class `Node` instances
in the graph. A new `paraclete-clap-host` crate (GPL3) provides
`PluginLibrary` (loads a `.clap` shared library), `PluginDescriptor`
(plugin metadata), and `PluginNode` (wraps a loaded plugin as a `Node`).
The `clack` crate is used for safe host-side CLAP bindings. P8 scope is
generator plugins (audio output only); effect plugins (audio input →
audio output) are deferred to P9.

-----

## Context

ADR-003 established CLAP as the primary plugin format. ADR-024 established
`paraclete-clap` as the plugin-side adapter (Paraclete as a CLAP plugin
consumed by a DAW). That is the opposite direction from what this ADR
covers.

**This ADR addresses Paraclete as a CLAP host** — the platform loads
third-party `.clap` plugins and presents each one as a `Node` in the
audio graph. A plugin node wired into the graph is indistinguishable from
a first-party node from the routing and parameter control perspective.

The roadmap has listed "CLAP Host" as the P8 scope deliverable from the
start. ADR-024 deferred this work explicitly: *"The `clack` crate (safe
CLAP host bindings) remains as a P8 dependency for loading third-party
CLAP plugins as nodes."* This ADR specifies the architecture.

-----

## Why a separate crate rather than extending `paraclete-clap`

`paraclete-clap` is the plugin-side adapter. Its modules synthesise
`ProcessInput` from CLAP buffer pointers and write `ProcessOutput` back.
The direction of translation is: CLAP in → Paraclete node.

The host-side adapter does the opposite: Paraclete events → CLAP buffers,
CLAP buffers → `ProcessOutput`. These are not symmetric operations; the
data models, CLAP extensions involved, and threading contracts differ.

Mixing plugin and host code in one crate would conflate two distinct roles,
making the boundary between them invisible to readers and making dependency
analysis harder. A separate `paraclete-clap-host` crate makes the
architecture explicit: `paraclete-clap` is for plugins Paraclete exports;
`paraclete-clap-host` is for plugins Paraclete imports.

License is the same (GPL3 in both cases), so there is no license reason
to combine them.

-----

## Why `clack` for the host side

`clap-sys` (raw C bindings) was used for the plugin side in ADR-024 because
the CLAP plugin entry point and factory are a small, fixed surface where the
unsafe FFI is unavoidable and manageable.

The host side is larger and involves more complex state management:
querying CLAP extensions, managing parameter info, driving the CLAP
process loop, handling the host callbacks the plugin may invoke. Writing
this entirely in `unsafe extern "C"` is feasible but produces a large
unsafe surface area with limited benefit.

`clack` wraps the CLAP host API in safe Rust, handling the lifetime and
threading invariants that the CLAP spec demands. It was already identified
as the correct host-side dependency in ADR-024. There is no reason to use
raw bindings here.

-----

## P8 scope: generator plugins only

A generator plugin produces audio (audio output ports) from events (MIDI
note in). This maps directly to the existing generator node pattern: a
`Node` with event input and audio output.

An effect plugin also has audio input ports. Wiring a `PluginNode` with
audio inputs requires the platform to route audio from upstream nodes into
the CLAP plugin's input buffers before calling `process()`. This is
architecturally correct but adds complexity: the `PluginNode` needs audio
input port declarations, and the executor must fill those ports from the
upstream audio buffers before calling the plugin. This is exactly the same
work as wiring any other audio effect node; it is not blocked by anything
in P8 other than schedule.

Deferring effect plugins to P9 keeps P8 scope manageable. The P8
implementation will note the architectural extension point clearly.

-----

## How `PluginNode` implements `Node`

`PluginNode` implements the `Node` trait entirely. From the graph's
perspective it is a normal node. The `capability_document()` is
synthesised from the CLAP plugin's declared parameter list; the resulting
`CapabilityDocument` uses `id_for_name(param_name_string)` as the
Paraclete param ID for each CLAP parameter. If the plugin uses canonical
parameter names from ADR-019 (e.g. `"cutoff"`, `"decay"`), those
parameters become reachable by any encoder mapped to
`id_for_name("cutoff")` with no platform changes.

The node API surface:

```rust
impl Node for PluginNode {
    fn activate(&mut self, sample_rate: f32, block_size: usize);
    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput);
    fn deactivate(&mut self);
    fn capability_document(&self) -> CapabilityDocument;
    fn type_name(&self) -> &'static str { "PluginNode" }
    // serialize/deserialize via CLAP state extension when present
}
```

`PluginNode` only depends on `paraclete-node-api` (LGPL3). It does not
depend on `paraclete-nodes` or `paraclete-runtime`. This keeps
`paraclete-clap-host` portable by the same logic that governs
`paraclete-nodes` (ADR-022): the wrapper is usable in any context that
provides a `Node` consumer.

-----

## Parameter bridge: host side vs plugin side

`ClapParamBridge` in `paraclete-clap` (plugin side) maps sequential CLAP
IDs (0, 1, 2… assigned at plugin export time) to Paraclete `id_for_name`
hashes. The sequential IDs are Paraclete's choice; they are assigned in
capability document declaration order.

`HostParamBridge` in `paraclete-clap-host` (host side) maps the third-party
plugin's native CLAP param IDs (which are the plugin author's choice, not
sequential, and may be arbitrary u32 values) to Paraclete `id_for_name`
hashes. The hash is derived from the param name string that the plugin
declares. Two plugins that both declare `"cutoff"` will map to the same
Paraclete param ID; a hardware encoder bound to `id_for_name("cutoff")`
controls both.

`HostParamBridge` also synthesises the `CapabilityDocument` for the
`PluginNode`. The bridge is built once when the plugin is instantiated and
does not change.

-----

## Plugin discovery

`scan_clap_paths()` returns `Vec<PathBuf>` of `.clap` files found in
OS-standard directories:

| OS | Directories |
|----|-------------|
| Linux | `~/.clap/`, `/usr/lib/clap/`, `/usr/local/lib/clap/` |
| macOS | `~/Library/Audio/Plug-Ins/CLAP/`, `/Library/Audio/Plug-Ins/CLAP/` |
| Windows | `%LOCALAPPDATA%\Programs\Common\CLAP\`, `C:\Program Files\Common Files\CLAP\` |

A `.clap` path can also be declared explicitly in the instrument definition
file (ADR-026), bypassing the scan. When an `instrument.yaml` node of type
`clap_plugin` specifies a `plugin_path`, that path is loaded directly.
Either discovery mode returns a `PluginLibrary` that can be queried for its
`PluginDescriptor` list and used to instantiate `PluginNode`.

-----

## Threading contracts

CLAP imposes strict thread requirements. `PluginNode` must respect them:

| Operation | CLAP thread | `Node` method called from |
|-----------|------------|--------------------------|
| `init()` | main thread | — (called inside `PluginLibrary::instantiate()`) |
| `activate()` | main thread | `Node::activate()` |
| `process()` | audio thread | `Node::process()` |
| `deactivate()` | main thread | `Node::deactivate()` |
| `destroy()` | main thread | `Drop for PluginNode` |

The Paraclete runtime already calls `activate()` and `deactivate()` from the
main thread and `process()` from the audio thread. These contracts align
without any additional thread management in `PluginNode`.

The host callbacks the plugin may invoke during `process()` (e.g. latency
change, parameter flush) are handled by the `clack`-provided host stub.
For P8, all such callbacks are no-ops with a debug log; full host callback
handling is deferred.

-----

## LGPL3 / GPL3 boundary

| Crate | License | Notes |
|-------|---------|-------|
| `paraclete-node-api` | LGPL3 | `PluginNode` implements this |
| `paraclete-clap-host` | GPL3 | New crate; depends on clack + node-api |
| `clack` | MIT | Safe CLAP host bindings |

`PluginNode` itself only links against `paraclete-node-api` (LGPL3). The
GPL3 surface is in `paraclete-clap-host`, not in the `Node` implementation.
A third-party node author who wraps a CLAP plugin using `PluginNode` as a
library dependency would incur GPL3 from `paraclete-clap-host`; this is
appropriate because they are writing platform host code, not a portable
node.

-----

## Crate structure

```
paraclete-clap-host/
├── Cargo.toml             (GPL3; deps: paraclete-node-api, clack)
└── src/
    ├── lib.rs             (pub re-exports: PluginLibrary, PluginDescriptor,
    │                       PluginNode, HostError, scan_clap_paths)
    ├── scan.rs            (scan_clap_paths() — OS-conditional implementation)
    ├── library.rs         (PluginLibrary — loads .clap, lists descriptors,
    │                       instantiates PluginNode)
    ├── bridge.rs          (HostParamBridge — CLAP params → CapabilityDocument)
    └── node.rs            (PluginNode — implements Node)
```

-----

## Deferred

- **Effect plugins (audio input):** `PluginNode` audio input ports, P9.
- **CLAP GUI extension:** No GUI at P8. Deferred per P7 known gaps.
- **CLAP note expressions:** CLAP NoteExpression events, P9+.
- **Full host callbacks:** Latency change, request restart, etc. No-ops at P8.
- **Preset management:** `clap_plugin_preset_discovery`, P9+.
- **Hot-loading / rescan at runtime:** Plugins are loaded at startup only. P9+.
- **`SubgraphPlugin` CLAP machine slot swapping:** ADR-024 noted this as P8+;
  it depends on `GraphNode` (ADR-023) which is P9. Still deferred.

-----

## Consequences

**P8:**
- `paraclete-clap-host` crate created as described
- `clack` added to workspace dependencies (MIT)
- `scan_clap_paths()` function implemented (OS-conditional)
- `PluginLibrary::load()` + `PluginLibrary::instantiate()` implemented
- `PluginNode` implements `Node`; generator plugins usable in graph
- Instrument definition YAML supports `clap_plugin` node type (ADR-026)
- `paraclete-app` startup loads declared CLAP plugins via `PluginLibrary`

**Ongoing:**
- Any new parameter declared by a third-party plugin that matches a
  canonical name from ADR-019 is automatically reachable by existing
  hardware profiles — no profile changes needed
- CLAP plugin state extension maps to `Node::serialize()` /
  `Node::deserialize()` — project save/recall (ADR-025) works for plugin
  nodes that implement the CLAP state extension

-----

## References

- ADR-001 — License (GPL3 / LGPL3 boundary)
- ADR-003 — Plugin format (CLAP primary)
- ADR-009 — Clock federation (DAW transport priority)
- ADR-014 — Capability document
- ADR-019 — Universal parameter control (canonical names, HostParamBridge)
- ADR-022 — Node portability (portability rule)
- ADR-024 — CLAP plugin wrapper (plugin side; `ClapParamBridge`; clack note)
- ADR-026 — Instrument definition and TUI (clap_plugin in instrument.yaml)
- `clack` crate — safe CLAP host bindings for Rust
