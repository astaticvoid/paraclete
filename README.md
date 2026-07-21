# Paraclete

A live signal graph for instruments, sequencers, controllers, and effects.
Everything is a node.

---

## Quick start

```bash
# 1. Generate synthesized drum samples (one-time)
cargo run -p gen-samples

# 2. Run
cargo run
```

Loads `instrument.yaml`: a 4-track drum machine (kick, snare, hihat, FM bass →
mix → reverb → output) at 140 BPM. Use `--instrument=<file>` for a different
graph.

Sequencers start empty — program steps with the keyboard (below) or a connected
Launchpad. Esc or Ctrl-C to stop.

Hardware devices (Novation Launchpad, Elektron Digitakt, Arturia Keystep) open
if connected and are skipped otherwise.

---

## Keyboard controls

### Theotokos (keyboard-first performance: `--theotokos`)

`cargo run -- --theotokos` starts the keyboard-first modal performance terminal.
Home-row steps, top-row tracks, vim-style parameter jog, live terminal graphics.

**Global (all modes):**

| Key | Action |
|---|---|
| `Space` | play / stop |
| `Tab` / `Shift-Tab` | cycle modes (SEQ ↔ PERF) |
| `q w e r u i o p` | select track 1–8 |
| `Ctrl-C` | quit |

**SEQ mode — pattern editing:**

| Key | Action |
|---|---|
| `a s d f j k l ;` | toggle steps 1–8 on the active track |
| `z x c v m , . /` | toggle steps 9–16 (each key below its home-row counterpart) |
| `[` / `]`  or `-` / `=` | previous / next 16-step page window (for extended patterns) |

**PERF mode — parameter performance:**

| Key | Action |
|---|---|
| `1`–`6` | select param page (SRC / FLTR / AMP / FX / MOD / TRIG) |
| `j` / `k` | slot A jog down / up |
| `,` / `.` | slot B jog down / up |
| `J` / `K` | slot A fine jog (Shift) |

### Emulator (legacy grid mirror)

When no Launchpad is connected, the 8x8 grid maps to the keyboard:
```
1 2 3 4 5 6 7 8   select track row (0–7)
Q W E R T Y U I   toggle steps in the active row
A S D F G H J K   scene buttons (page select)
Z X C V B N M ,   control row (modes / navigation)
Tab               cycle input mode      Esc / Ctrl-C  quit
```

---

## Building and testing

```bash
# Build (must use --workspace)
cargo build --workspace
cargo build --workspace --release

# Test
cargo test --workspace
cargo clippy --workspace
```

---

## Architecture

Five-layer model. No layer reaches across another.

| Layer | Crate | License | Responsibility |
|-------|-------|---------|----------------|
| L0 HAL | `paraclete-hal` | GPL3 | Audio I/O (cpal), MIDI, terminal emulator |
| L1 Runtime | `paraclete-runtime` | GPL3 | Node graph, scheduling, clock |
| L2 Node API | `paraclete-node-api` | **LGPL3** | Contract every node implements; third-party boundary |
| L3 Nodes | `paraclete-nodes` | GPL3 | Sequencer, engines, effects, samplers |
| L4 Scripting | `paraclete-scripting` | GPL3 | Rhai sandbox, hardware profiles |

The runtime splits into `NodeConfigurator` (main thread, graph topology) and
`NodeExecutor` (audio thread, no allocation, no locks). All cross-thread
communication uses a lock-free ring buffer.

Full architecture reference: `design/architecture-core.md`.

---

## Commands

```bash
# Run
cargo run                              # instrument.yaml + TUI
cargo run -- --instrument=my.yaml      # custom instrument
cargo run -- --no-tui                  # no terminal UI
cargo run -- --dev-ui                  # step/pattern on stderr
cargo run -- --load=project.ron        # restore saved state
cargo run -- --save=project.ron        # save state on exit
cargo run -- --theoria-dir=web/packages/app/dist  # serve web client
cargo run -- --theotokos              # keyboard-first performance terminal

# Web client
cd web && npm install && npm run build

# Generate drum samples
cargo run -p gen-samples

# CLAP plugins (output: target/debug/*.clap)
cargo build --workspace -p paraclete-clap

# Headless testing
cargo run -p test-driver -- --trigger kick --at 1.0 -d 3
cargo run -p test-driver -- tools/test-driver/tests/kick_reverb_clean.yaml
```

---

## Realtime audio (Linux)

Paraclete requests `SCHED_FIFO` realtime scheduling on the audio thread for
glitch-free playback under load.  Two paths are tried automatically at startup:

1. **rtkit** — D-Bus service on PipeWire/PulseAudio desktops.  Works with zero
   configuration on Debian, Ubuntu, Fedora.
2. **Raw `pthread_setschedparam`** — works if the user has `rtprio` limits
   (common on Arch via `realtime-privileges`).

**Setup per distro:**

| Distro | Package / config |
|---|---|
| Arch | `sudo pacman -S realtime-privileges rtkit && sudo usermod -a -G realtime $USER` (log out/in) |
| Debian / Ubuntu | `rtkit` is installed with `pipewire` or `pulseaudio` — no extra steps |
| Fedora | `sudo dnf install realtime-setup && sudo usermod -a -G realtime $USER` (log out/in) |
| Other | `sudo setcap cap_sys_nice=eip target/release/paraclete` |

If neither path succeeds, audio runs under normal scheduling with a warning —
fine for light use, may produce underruns under heavy load.

---

## Hardware

Connect devices before starting. Detected by USB name substring:

| Device | Name substring | Fallback |
|--------|---------------|---------|
| Novation Launchpad X / MK2 | `"Launchpad"` | Terminal emulator |
| Elektron Digitakt | `"Digitakt"` | Skipped |
| Arturia Keystep 37 | `"Keystep"` | Skipped |

Hardware behavior is scripted in Rhai profiles (`profiles/`).

---

## What works

- 8-track graph: clock → sequencer → synth → distortion → filter → mix → audio
- Analog engine (kick, snare, hihat), FM engine (bass, bell, kick)
- Sampler with Hermite pitch playback (symphonia WAV loading)
- Step sequencer: swing, fill A/B, per-step probability and micro-timing,
  8-pattern bank with cued switching and chain, multi-page (64-step) patterns,
  page-loop windows, per-track length and speed
- Per-step CV locks with sample-and-hold output
- Reverb on master bus
- Declarative instrument definitions in YAML
- Terminal UI: transport bar, encoder values, 16-step playhead
- Dynamic topology at runtime (`apply_patch`, ~5 ms silence)
- Single-sample feedback via `LoopBreakNode`
- `InnerGraphNode`: nodes that own a nested executor
- Project save/recall in RON (v3 with full sequencer state)
- CLAP plugin output (`SingleNodePlugin`, `SubgraphPlugin`, five machine-bank `.clap` files)
- CLAP host: load third-party `.clap` plugins as nodes
- WebSocket + HTTP interface server + web client
- Rhai scripting for hardware profiles and state bus subscriptions
- Headless test-driver with interactive REPL, regression baselines, and
  structured per-node debug logging

---

## Project layout

```
crates/
  paraclete-node-api    L2 LGPL3 — Node API (third-party boundary)
  paraclete-runtime     L1 GPL3  — graph, configurator/executor, state bus
  paraclete-hal         L0 GPL3  — cpal audio, MIDI, terminal emulator
  paraclete-nodes       L3 GPL3  — sequencer, engines, effects, samplers
  paraclete-scripting   L4 GPL3  — Rhai sandbox, hardware event dispatch
  paraclete-app              GPL3 — binary entry point
  paraclete-clap             GPL3 — CLAP plugin output
  paraclete-clap-host        GPL3 — CLAP host (loads third-party .clap files)
  paraclete-tui              GPL3 — terminal UI (display-only)
  paraclete-theotokos        GPL3 — keyboard-first performance terminal
  paraclete-graph-nodes      GPL3 — nodes owning an inner executor
  paraclete-antiphon         GPL3 — WebSocket interface server
  paraclete-machine-*        GPL3 — single-node CLAP plugins
tools/
  gen-samples/          — drum sample generator
  test-driver/          — headless test/CI harness
profiles/               — Rhai hardware profile scripts
samples/                — WAV files (not committed)
design/                 — architecture docs, ADRs, phase reports
```

---

## License

`crates/paraclete-node-api` is LGPL3. All other crates are GPL3.
See `LICENSE` and `crates/paraclete-node-api/LICENSE`.
