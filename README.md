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

`cargo run` opens **Theotokos**, the keyboard-first performance terminal
(below). Sequencers start empty — program steps with the keyboard or a
connected Launchpad. `Ctrl-C` stops; `Esc` is NO/back-out inside Theotokos,
not quit.

Hardware devices (Novation Launchpad, Elektron Digitakt, Arturia Keystep) open
if connected and are skipped otherwise.

---

## Keyboard controls

### Theotokos (keyboard-first performance — the default)

`cargo run` starts the keyboard-first performance terminal; `--theotokos` is
accepted but does nothing, since this is what you get unless you ask for
`--emulator` (legacy grid) or `--no-tui` (headless).
A fixed seven-region panel (ADR-044 D1) with one always-visible 2×8 trig
strip for the *selected* track, TRK/PTN hold-chords for track/pattern
select, an explicit ENC mode for the 8-encoder parameter bank, a shared
p-lock target (latched or momentary), and dedicated Tempo/Settings/Chain
screens. There is no dedicated Mute screen — mute state lives on the
always-visible track indicator; TRK+FUNC+trig toggles it.

**Pads + rec:**

| Key | Action |
|---|---|
| `q w e r t y u i` | Trig 1–8 (top row) |
| `a s d f g h j k` | Trig 9–16 (bottom row) |
| bare trig (REC off/Live) | play + select that track |
| `Tab` (hold) + trig | select track silently |
| `p` (hold) + trig | select pattern |
| `z` | REC — toggles `Off ↔ Grid` (step-entry); bare trig writes/clears a step in `Grid` |
| `z` (hold) + `x` | REC + PLAY — escalates to `Live` (engine-side live record) and starts the transport |
| `x` / `c` | PLAY / STOP (`Space` = PLAY alias) |
| `Shift+z` / `Shift+x` / `Shift+c` | copy / clear / paste the active lane |
| `-` / `=` | previous / next 16-step page window |

**Screens:**

| Key | Action |
|---|---|
| `1`–`6` | param pages, canonical order TRIG SRC FLTR AMP FX MOD |
| `7` / `9` | KIT / SAMPLING (reserved) |
| `8` | SETTINGS (read-only: bpm, kitty status, track/pattern counts, version) |
| `0` | TEMPO (`Enter` taps tempo) |
| `o` | SONG — opens the Chain screen |
| `v` | KEYBD (reserved, chromatic input) |
| `Esc` | NO — also returns to the Grid screen from any other screen, and clears a set p-lock target |
| arrows | navigate the current screen (Tempo: ±bpm; Chain: cursor — no-op elsewhere) |

**Encoder mode + p-lock (ADR-044 D9/D15):**

| Key | Action |
|---|---|
| `n` | toggle ENC mode — while on, a bare trig jogs encoder *n* (top row up, bottom row down) on **any** screen |
| `Ctrl` / `Shift` (FUNC) + trig, in ENC mode | fine / coarse jog |
| `Shift` (FUNC) + top/bottom-row key *n*, ENC off | jog encoder *n* up / down (`Ctrl+FUNC` = fine) |
| `m` (hold) + trig, in `Grid` | arm a p-lock target (latched — same trig again, `m` again, or `Esc` clears it) |
| hold a trig in `Grid` (kitty terminals) | set the p-lock target for the duration of the hold (momentary) |

While a p-lock target is set, encoder jog / numpad slots / `:set` write a
per-step lock instead of the live value.

Numpad slot A/B/C jog remains unwired — formally descoped, not merely
deferred; ADR-044 D9's ENC mode gives the encoder bank its own
modifier-free path, so the numpad cluster's fate is an open question for
a future usability session, not a wiring gap. See BUG-038 —
`gh issue list --search "BUG-038 in:title" --state all`.

**Other:**

| Key | Action |
|---|---|
| `Shift+;` (`:`) | command line |
| `?` | help |
| `Backspace` | clear locks |
| `Ctrl-C` | quit |

### Emulator (legacy grid mirror: `--emulator`)

`cargo run -- --emulator` replaces Theotokos with the 8x8 Launchpad grid
mapped to the keyboard. Theotokos does not fall back to it, so this flag is
the only way in:
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
cargo run                              # instrument.yaml + Theotokos
cargo run -- --instrument=my.yaml      # custom instrument
cargo run -- --emulator                # legacy Launchpad grid instead
cargo run -- --no-tui                  # headless, no terminal UI
cargo run -- --dev-ui                  # step/pattern on stderr
cargo run -- --load=project.ron        # restore saved state
cargo run -- --save=project.ron        # write state at startup (not on quit)
cargo run -- --theoria-dir=web/packages/app/dist  # serve web client
cargo run -- --token                   # require a 6-digit code (LAN is open by default)

# Web client
cd web && npm install && npm run build

# Generate drum samples
cargo run -p gen-samples

# CLAP plugins — the machine-* crates are the cdylibs; `paraclete-clap` is a
# plain lib and builds no plugin on its own. Rename the artifact to `.clap`:
#   target/debug/libparaclete_machine_kick.so  (.dylib on macOS)
cargo build -p paraclete-machine-kick

# Headless testing
cargo run -p test-driver -- --trigger analog_engine:kick --at 1.0 -d 3
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
| Novation Launchpad X / MK2 | `"Launchpad"` | Skipped (`--emulator` for the grid) |
| Elektron Digitakt | `"Digitakt"` | Skipped |
| Arturia Keystep 37 | `"Keystep"` | Skipped |

Hardware behavior is scripted in Rhai profiles (`profiles/`).

---

## What works

- Up to 8 tracks: clock → sequencer → synth → distortion → filter → mix → audio
  (the shipped `instrument.yaml` wires 4; `instrument-fx.yaml` shows a
  filter/distortion chain)
- Analog engine (kick, snare, hihat), FM engine (bass, bell, kick)
- Sampler with Hermite pitch playback (symphonia WAV loading)
- Step sequencer: swing, fill A/B, per-step probability and micro-timing,
  8-pattern bank with cued switching and chain, multi-page (64-step) patterns,
  page-loop windows, per-track length and speed
- Per-step CV locks with sample-and-hold output
- Reverb on master bus
- Declarative instrument definitions in YAML
- Theotokos performance terminal: seven-region panel, trig strip, hold-chords,
  encoder mode, p-locks (the legacy `--emulator` grid remains)
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
  paraclete-tui              GPL3 — legacy emulator grid UI (display-only)
  paraclete-theotokos        GPL3 — keyboard-first performance terminal (default)
  paraclete-graph-nodes      GPL3 — nodes owning an inner executor
  paraclete-antiphon         GPL3 — WebSocket interface server
  paraclete-view-assembly    GPL3 — page/rule assembly shared by web + terminal
  paraclete-machine-*        GPL3 — one CLAP plugin per machine (5 crates)
tools/
  gen-samples/          — drum sample generator
  test-driver/          — headless test/CI harness
  lpx-debug/            — Launchpad X protocol probe
profiles/               — Rhai hardware profile scripts
samples/                — WAV files (not committed)
web/                    — Theoria web client (npm workspaces: core, app)
design/                 — architecture docs, ADRs, phase specs/reports
                          (open bugs + questions are GitHub Issues)
```

---

## License

`crates/paraclete-node-api` is LGPL3. All other crates are GPL3.
See `LICENSE` and `crates/paraclete-node-api/LICENSE`.
