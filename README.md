# Paraclete

An open-source musical computation runtime — a live, composable signal graph for
instruments, sequencers, controllers, and effects. Not a DAW. Not a plugin. A
runtime where everything is a node.

The closest analogies: Plan 9 (everything is a composable node), Emacs (live,
reprogrammable runtime), and Elektron hardware (hardware-first, mouse-free workflow).

**Current phase: P9 complete** — modular patchable graph (dynamic topology),
single-sample feedback loop break, sequencer as a CV source, and a GraphNode
that owns a nested executor. Builds on P8's YAML instrument definitions, the
ratatui terminal UI, and the CLAP host. 391 tests.

**Next: P10 — Pattern Engine.** Multi-pattern + seamless switching + chaining,
multi-page (64-step) patterns, per-track length & speed (polyrhythm), and full
serialization of sequencer state. See `design/roadmap.md` and
`design/phases/p10-interfaces.md`.

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

On first boot: kick on beats 1 and 3, snare on 2 and 4, hihat on all 16 steps,
bass on beat 1 — 140 BPM. Esc or Ctrl-C to stop.

Hardware devices (Novation Launchpad, Elektron Digitakt, Arturia Keystep) are
opened if present and fall back gracefully when absent.

---

## Keyboard controls (terminal emulator)

When no physical Launchpad is connected, three keyboard rows map to the pad grid:

```
Row 0 → Track 0 (Kick)    Q  W  E  R  T  Y  U  I   steps 0–7
Row 1 → Track 1 (Snare)   A  S  D  F  G  H  J  K   steps 0–7
Row 2 → Track 2 (HiHat)   Z  X  C  V  B  N  M  ,   steps 0–7
```

Each key press toggles that step. Tracks 3–7 require a physical Launchpad.

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
# Build
cargo build
cargo build --release

# Run
cargo run                              # graph from instrument.yaml; ratatui TUI by default
cargo run -- --instrument=my.yaml      # load a specific instrument definition
cargo run -- --no-tui                  # skip the TUI (stderr logging instead)
cargo run -- --dev-ui                  # step/pattern monitor on stderr
cargo run -- --load=project.ron        # restore saved project state (RON)
cargo run -- --save=project.ron        # save project state on exit

# Generate samples
cargo run -p gen-samples               # writes samples/track0–7.wav
cargo run -p gen-samples -- path/      # write to a custom directory

# Build CLAP plugins (output: target/debug/*.clap)
cargo build -p paraclete-clap

# Test
cargo test
cargo test -p paraclete-runtime
cargo test -p paraclete-node-api
cargo clippy
cargo check
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

## What works at P9

- 8-track graph: clock → sequencer → instrument → distortion → filter → mix → audio
- Synthesis engines: `AnalogEngine` (kick, snare, hihat), `FmEngine` (bass, bell, kick)
- Sampler with per-voice rubato pitch resampling (symphonia + rubato)
- Step sequencer with swing, fill A/B, per-step probability and micro-timing
  (single 16-step pattern per track — the full pattern engine lands in P10)
- Sequencer as a CV source: per-step CV locks with sample-and-hold output
- Reverb on the master bus
- Declarative instrument definition in YAML (`--instrument`); set initial params per node
- Terminal UI (`paraclete-tui`): transport bar, live encoder values, step row
- Dynamic topology: add/remove nodes and edges at runtime (`apply_patch`, ~5 ms silence)
- Single-sample feedback via `LoopBreakNode` (one `LoopBreakNode` per cycle is legal)
- GraphNode / `InnerGraphNode`: a node that owns a nested executor
- Project save/recall in RON format, v2 with topology (`--load`/`--save`)
- CLAP plugin output: `SingleNodePlugin`, `SubgraphPlugin`, five machine-bank `.clap` binaries
- CLAP host: load third-party `.clap` plugins as nodes (`paraclete-clap-host`)
- Rhai profile scripts with state bus subscriptions and LED feedback
- 391 tests (389 distinct), 0 failures

---

## License

`crates/paraclete-node-api` is licensed under the **GNU Lesser General Public
License v3** (LGPL3). All other crates are licensed under the **GNU General
Public License v3** (GPL3).

See `LICENSE` and `crates/paraclete-node-api/LICENSE`.
