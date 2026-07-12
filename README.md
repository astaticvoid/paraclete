# Paraclete

An open-source musical computation runtime — a live, composable signal graph for
instruments, sequencers, controllers, and effects. Not a DAW. Not a plugin. A
runtime where everything is a node.

The closest analogies: Plan 9 (everything is a composable node), Emacs (live,
reprogrammable runtime), and Elektron hardware (hardware-first, mouse-free workflow).

**Current phase: P10 complete** — pattern engine: multi-pattern bank (8 patterns),
seamless cued switching + chain, multi-page (64-step) patterns with page-loop
window, per-track length & speed (polyrhythm), full sequencer state serialization
(v3). Builds on P9's dynamic topology, inner graphs, and loop break.

---

## Quick start

```bash
# 1. Generate synthesized drum samples (required on first run)
cargo run -p gen-samples

# 2. Run the sequencer
cargo run

# 3. Run with step/pattern display on stderr
cargo run -- --dev-ui
```

The graph is loaded from `instrument.yaml` (a 4-track synthesis drum machine at
140 BPM: kick, snare, hihat, FM bass → mix → reverb → output). Pass
`--instrument=<file>` to load a different one.

Sequencers start **empty** — program steps live with the terminal-emulator
keyboard rows (see below) or a connected Launchpad. Esc or Ctrl-C to stop.

Hardware devices (Novation Launchpad, Elektron Digitakt, Arturia Keystep) are
opened if present and fall back gracefully when absent.

---

## Keyboard controls (terminal emulator)

When no physical Launchpad is connected, the full 8×8 surface is keyboard-driven
via a track-cursor scheme:

```
1 2 3 4 5 6 7 8   select active track row (0–7)
Q W E R T Y U I   toggle the 8 step pads in the active row
A S D F G H J K   scene buttons (page select)
Z X C V B N M ,   top control row (modes / navigation)
Tab               cycle input mode      Esc / Ctrl-C  quit
```

Pick a track with the number row, then toggle its steps — all 8 tracks, the
scene buttons, and the control row are reachable. (Replaces the older 3-row
mapping that only reached tracks 0–2.)

---

## Project layout

```
crates/
  paraclete-node-api    L2 LGPL3 — Node API contract (third-party extensible)
  paraclete-runtime     L1 GPL3  — graph, configurator/executor, state bus
  paraclete-hal         L0 GPL3  — cpal audio, MIDI devices, terminal emulator
  paraclete-nodes       L3 GPL3  — Sequencer, AnalogEngine, FmEngine, Sampler, effects
  paraclete-scripting   L4 GPL3  — Rhai sandbox, hardware event dispatch
  paraclete-app              GPL3 — binary entry point
  paraclete-clap             GPL3 — Paraclete as a CLAP plugin (ADR-024)
  paraclete-clap-host        GPL3 — Paraclete as a CLAP host (ADR-027)
  paraclete-tui              GPL3 — ratatui terminal UI (ADR-026)
  paraclete-graph-nodes      GPL3 — nodes that own an inner executor (ADR-023)
  paraclete-antiphon         GPL3 — WebSocket interface server (ADR-031)
  paraclete-machine-*        GPL3 — single-node CLAP plugin crates (kick, snare, fm-kick, fm-bell, fm-bass)
tools/
  gen-samples/          — synthesized drum sample generator
profiles/               — Rhai hardware profile scripts
samples/                — WAV files loaded by Sampler (not committed)
design/                 — architecture docs, ADRs, phase reports
```

---

## Architecture

Paraclete uses a five-layer model. No layer may reach across another layer's boundary.

| Layer | Crate | License | Responsibility |
|-------|-------|---------|----------------|
| L0 HAL | `paraclete-hal` | GPL3 | Audio I/O via cpal, hardware emulator |
| L1 Runtime | `paraclete-runtime` | GPL3 | Node graph, scheduling, clock federation |
| L2 Node API | `paraclete-node-api` | **LGPL3** | The contract every node implements |
| L3 Nodes | `paraclete-nodes` | GPL3 | First-party node implementations |
| L4 Scripting | `paraclete-scripting` | GPL3 | Rhai scripting sandbox |

The LGPL3 boundary at L2 is intentional: third parties can write closed-source
nodes by implementing the Node API without being forced to open-source their work.

The runtime splits into `NodeConfigurator` (main thread, owns graph topology) and
`NodeExecutor` (audio thread, allocation-free, lock-free). All cross-thread
communication goes through a lock-free ring buffer.

See `design/architecture-core.md` for the full reference.

---

## Commands

```bash
# Build (must use --workspace; paraclete-app is the sole default-member)
cargo build --workspace
cargo build --workspace --release

# Run
cargo run                              # graph from instrument.yaml; ratatui TUI by default
cargo run -- --instrument=my.yaml      # load a specific instrument definition
cargo run -- --no-tui                  # skip the TUI (stderr logging instead)
cargo run -- --dev-ui                  # step/pattern monitor on stderr
cargo run -- --load=project.ron        # restore saved project state (RON)
cargo run -- --save=project.ron        # save project state on exit
cargo run -- --theoria-dir=web/packages/app/dist  # serve Theoria web client

# Web client (Theoria)
cd web && npm install && npm run build    # build web/packages/app/dist

# Generate samples
cargo run -p gen-samples               # writes samples/track0–7.wav
cargo run -p gen-samples -- path/      # write to a custom directory

# Build CLAP plugins (output: target/debug/*.clap)
cargo build --workspace -p paraclete-clap

# Test
cargo test --workspace
cargo test -p paraclete-runtime
cargo test -p paraclete-node-api
cargo clippy --workspace
cargo check --workspace
```

---

## Hardware setup

Connect devices before starting. Detected by USB name substring:

| Device | Name substring | Fallback |
|--------|---------------|---------|
| Novation Launchpad X / MK2 | `"Launchpad"` | Terminal emulator |
| Elektron Digitakt | `"Digitakt"` | Skipped |
| Arturia Keystep 37 | `"Keystep"` | Skipped |

Profile scripts in `profiles/` are loaded at startup and control LED feedback,
encoder mappings, and pad routing via Rhai.

---

## What works at P10

- 8-track graph: clock → sequencer → instrument → distortion → filter → mix → audio
- Synthesis engines: `AnalogEngine` (kick, snare, hihat), `FmEngine` (bass, bell, kick)
- Sampler with Hermite pitch playback (symphonia for file loading)
- Step sequencer with swing, fill A/B, per-step probability and micro-timing,
  8-pattern bank, cued switching + chain, multi-page (64-step) patterns,
  page-loop window, per-track length & speed (polyrhythm)
- Sequencer as a CV source: per-step CV locks with sample-and-hold output
- Reverb on the master bus
- Declarative instrument definition in YAML (`--instrument`); set initial params per node
- Terminal UI (`paraclete-tui`): transport bar, live encoder values, 16-step playhead
  with pattern/page indicator
- Dynamic topology: add/remove nodes and edges at runtime (`apply_patch`, ~5 ms silence)
- Single-sample feedback via `LoopBreakNode` (one `LoopBreakNode` per cycle is legal)
- GraphNode / `InnerGraphNode`: a node that owns a nested executor
- Project save/recall in RON format, v3 with full sequencer state (`--load`/`--save`)
- CLAP plugin output: `SingleNodePlugin`, `SubgraphPlugin`, five machine-bank `.clap` binaries
- CLAP host: load third-party `.clap` plugins as nodes (`paraclete-clap-host`)
- Antiphon interface server: WebSocket + HTTP, Theoria web client (`--theoria-dir`)
- Rhai profile scripts with state bus subscriptions and LED feedback

---

## License

`crates/paraclete-node-api` is licensed under the **GNU Lesser General Public
License v3** (LGPL3). All other crates are licensed under the **GNU General
Public License v3** (GPL3).

See `LICENSE` and `crates/paraclete-node-api/LICENSE`.
