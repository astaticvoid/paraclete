# Paraclete

A real-time Rust audio, modulation, and event graph substrate—and a suite of
"no-DAW" live performance and composition instruments built directly on top of
it.

---

## Core Architecture: Substrate vs. Instruments

Paraclete separates the **universal signal-graph underlay** from the
**opinionated instruments and control surfaces** that drive it:

1. **The Graph Substrate (`paraclete-node-api`, `paraclete-runtime`, `paraclete-graph-nodes`, `paraclete-nodes`):**
   Every component—oscillators, filters, envelopes, LFOs, macro drum/FM/sampler
   engines, step sequencers, effects, clocks, and hardware MIDI controllers—is
   a first-class `Node` with typed ports (`Audio`, `Modulation`, `Midi2`,
   `Surface`, `Transport`).
   - `NodeConfigurator` (main thread, DAG & topological sort) communicates via
     lock-free SPSC rings (`rtrb`) with `NodeExecutor` (real-time audio thread:
     zero allocations, zero locks).
   - Arbitrary single-sample feedback loops are explicit via `LoopBreakNode`.
   - Subgraphs encapsulate into reusable macro nodes via `InnerGraphNode`.
   - Any node or subgraph compiles standalone to a `.clap` plugin
     (`paraclete-clap`) or hosts external `.clap` plugins
     (`paraclete-clap-host`).

2. **The Live Instruments & Surfaces (`paraclete-theotokos`, `paraclete-app`, `paraclete-antiphon`):**
   Rather than forcing a mouse-driven DAW timeline, Paraclete exposes the graph
   through dedicated hardware and keyboard interfaces:
   - **Theotokos (`paraclete-theotokos`):** Keyboard-first performance terminal
     designed around a ThinkPad keyboard. 16-step trig grid, parameter-lock
     authoring, conditional trigs, 8-pattern chains, kit recall, live sampling,
     and an 8-track drum strip (`Shift+8` / `MIX`).
   - **Launch Control XL Drum Machine (`lcxl_drum`):** 8-track x0x-style
     channel-strip drum machine. Each column maps 3 primary synthesis/filter
     pots and 1 volume fader to a drum voice, while the bottom 16 buttons toggle
     between Track Focus/Mute and a live 16-step x0x sequencer.
   - **Launchpad 8×8 Grid & Keystep:** Tactile pad matrix and chromatic/seq
     input via lock-free ALSA/CPAL MIDI nodes.
   - **Antiphon & Theoria (`paraclete-antiphon`, `web/`):** Zero-tokio
     WebSocket/HTTP state & topology bridge serving a companion web interface
     (acting as the bridge for visual graph patching until the native binary GUI
     host lands).

---

## Quick Start

```bash
# 1. Generate synthesized drum samples (one-time)
cargo run -p gen-samples

# 2. Run default 4-track instrument + Theotokos TUI
cargo run

# 3. Run full 8-track drum machine (for Launch Control XL + MIX view)
cargo run -- --instrument=instrument-8track.yaml
```

Hardware controllers (Launch Control XL, Launchpad X/MK2, Keystep, Digitakt) are
auto-detected at startup and skipped cleanly if disconnected.

---

## Controls

### Launch Control XL (8-Track Drum Machine)

| Control | Function |
|---|---|
| **Row 1–3 Knobs (Cols 1–8)** | Track 1–8 primary synthesis & filter macros (Tune/Pitch, Decay, Cutoff/Tone) |
| **Faders 1–8** | Track 1–8 mixer channel level (`MixNode` `input_gain_0..7`) |
| **Bottom Buttons (`Track` mode)** | Top row = Focus + audition track 1–8; Bottom row = Mute track 1–8 |
| **Bottom Buttons (`Seq` mode)** | Buttons 1–16 = Step 1–16 toggle for the focused track (with running LED playhead) |
| **Device Button** | Toggle bottom 16 buttons between `Track` mode and `Seq` (x0x) mode |
| **Mute Button** | Toggle Mute arm mode between Immediate Mute and Pattern Mute |
| **Send Select Up / Down** | Master Reverb wet/dry ± |
| **Track Select Left / Right** | Step focused track ← / → |

### Theotokos (Keyboard-First Terminal)

| Key | Action |
|---|---|
| `q w e r t y u i` / `a s d f g h j k` | Trig 1–8 / Trig 9–16 |
| `Tab` (hold) + trig | Select track 1–8 silently |
| `p` (hold) + trig | Select pattern 1–8 |
| `z` | Toggle `REC` (`Off ↔ Grid` step entry) |
| `z` (hold) + `x` | Toggle `Live` record + start playback |
| `x` / `c` (`Space`) | PLAY / STOP |
| `1`–`6` | Parameter pages (`TRIG`, `SRC`, `FLTR`, `AMP`, `FX`, `MOD`) |
| `7` / `9` | `KIT` screen / `SAMPLING` screen (`Shift+9` = live transition trick) |
| `8` / `Shift+8` | `SETTINGS` / `MIX` (8-track drum strip & step overview) |
| `0` / `o` / `v` | `TEMPO` / `CHAIN` (song mode) / `KEYBD` |
| `n` | Toggle `ENC` mode (top row increments, bottom row decrements encoders 1–8) |
| `m` (hold) + trig | Arm parameter-lock target step (or hold step directly in kitty) |
| `y` / `Y` | Lossless pattern yank (copy) / paste (preserves locks, micro-timing, conditions, note) |
| `:w [file]` / `:e <file>` | Save project (`.ron` + freezes live RAM captures) / Load project at runtime |
| `Shift+Enter` / `Shift+Esc` | Temp-save state / Temp-reload state |
| `Esc` / `Ctrl-C` | Back out to Grid (clear lock target) / Quit |

---

## Build, Test, and Headless Verification

Always pass `--workspace` (bare `cargo test` only runs root binary tests):

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace

# Offline DSP test-driver & regression baselines
cargo run -p test-driver -- --trigger analog_engine:kick --at 1.0 -d 3
cargo run -p test-driver -- tools/test-driver/tests/kick_reverb_clean.yaml

# Build Web UI bridge (Theoria)
cd web && npm install && npm run build
cargo run -- --theoria-dir=web/packages/app/dist
```

---

## Workspace Layout

| Layer / Role | Crate | License | Description |
|---|---|---|---|
| **L0 HAL** | `paraclete-hal` | GPL3 | CPAL audio I/O, lock-free MIDI surfaces (LCXL, Launchpad, Keystep), terminal |
| **L1 Runtime** | `paraclete-runtime` | GPL3 | DAG configurator, lock-free audio executor, state bus |
| **L2 Node API** | `paraclete-node-api` | **LGPL3** | `Node` trait, `ParameterBank`, `CapabilityDocument`, UMP/commands |
| **L3 Nodes** | `paraclete-nodes` | GPL3 | Sequencer, Analog/FM/Sampler voices, filters, delay, reverb, primitives |
| **L3 Subgraphs** | `paraclete-graph-nodes` | GPL3 | `InnerGraphNode` (hierarchical nested executors) |
| **L4 Scripting** | `paraclete-scripting` | GPL3 | Rhai controller scripts & state-bus subscriptions |
| **Views** | `paraclete-view-assembly` | GPL3 | Capability-driven parameter page & overlay assembly |
| **Terminal UI** | `paraclete-theotokos` | GPL3 | Keyboard-first performance workstation & multi-track mixer |
| **Wire Server** | `paraclete-antiphon` | GPL3 | Blocking WebSocket/HTTP state & topology server |
| **Plugins** | `paraclete-clap` / `-host` | GPL3 | Export nodes/subgraphs as `.clap` plugins; host external `.clap` plugins |
| **Binary** | `paraclete-app` | GPL3 | Main runtime, project persistence, hardware surface orchestration |

---

## Realtime Audio Setup (Linux)

Paraclete requests `SCHED_FIFO` realtime priority for the audio thread via
`rtkit` (PipeWire/PulseAudio) or `pthread_setschedparam`:

- **Arch:** `sudo pacman -S realtime-privileges rtkit && sudo usermod -a -G realtime $USER`
- **Debian/Ubuntu:** Included with `pipewire` / `pulseaudio` (`rtkit`)
- **Fedora:** `sudo dnf install realtime-setup && sudo usermod -a -G realtime $USER`
- **Manual capability:** `sudo setcap cap_sys_nice=eip target/release/paraclete`

---

## License

`crates/paraclete-node-api` is **LGPL-3.0-or-later** (the third-party node boundary).
All other crates are **GPL-3.0-or-later**. See `LICENSE` and `crates/paraclete-node-api/LICENSE`.
