# Paraclete

An open-source musical computation runtime — a live, composable signal graph for
instruments, sequencers, controllers, and effects. Not a DAW. Not a plugin. A
runtime where everything is a node.

**Current phase: P4** — 8-track drum machine, real MIDI hardware (Launchpad, Digitakt,
Keystep), effect nodes (distortion + SVF filter), Rhai profile scripts. Falls back
to a terminal emulator when hardware is not connected.

---

## Quick start

```bash
# 1. Generate synthesized drum samples
cargo run -p gen-samples

# 2. Run the sequencer
cargo run

# 3. Run with step-counter display (no separate window needed)
cargo run -- --dev-ui
```

On first boot you should hear kick on beats 1 and 3, snare on 2 and 4 at 140 BPM.
Esc or Ctrl-C to stop.

---

## Keyboard controls (terminal emulator)

The terminal LaunchpadEmulator maps three keyboard rows to the 8×8 pad grid:

```
Row 0 → Track 0 (Kick)     Q  W  E  R  T  Y  U  I   steps 0–7
Row 1 → Track 1 (Snare)    A  S  D  F  G  H  J  K   steps 0–7
Row 2 → Track 2 (Hat CH)   Z  X  C  V  B  N  M  ,   steps 0–7
```

Each key press **toggles** that step on the corresponding track.

Tracks 3–7 (Hat OH, Perc A, Perc B, FX, Bass) have no keyboard row at P4.
Toggle their steps from a connected Launchpad or by editing the preset in
`crates/paraclete-app/src/main.rs`.

---

## Dev UI

```bash
cargo run -- --dev-ui
```

Prints to stderr once per second per track:

```
[dev-ui] Kick    step=Some(Int(3))  pattern=Some(Text("1000000010000000"))
[dev-ui] Snare   step=Some(Int(3))  pattern=Some(Text("0000100000001000"))
```

`pattern` is a 16-character bitfield: `'1'` = step active, `'0'` = inactive.
`step` is the current playback position (0–15).

---

## Test environment

### 1. Generate samples

```bash
cargo run -p gen-samples
```

Writes `samples/track0.wav` through `samples/track7.wav` (mono, 44100 Hz, 16-bit):

| File | Track | Sound |
|------|-------|-------|
| `track0.wav` | Kick | 120→40 Hz pitch sweep, 600 ms |
| `track1.wav` | Snare | Noise + 200 Hz body, 350 ms |
| `track2.wav` | Hat CH | Short noise burst, 90 ms |
| `track3.wav` | Hat OH | Longer noise burst, 380 ms |
| `track4.wav` | Perc A | 900 Hz woodblock click, 180 ms |
| `track5.wav` | Perc B | 320 Hz conga, 280 ms |
| `track6.wav` | FX | Metallic descending sweep, 500 ms |
| `track7.wav` | Bass | 60 Hz sub tone, 900 ms |

Custom samples can go in `samples/` with the same names. Any mono or stereo WAV
at any sample rate works — the Sampler resamples to 44100 Hz at load time.

### 2. Run the test session in tmux (agent/CI use)

```bash
# launch
tmux new-session -d -s paraclete -x 100 -y 30 \
  'cargo run -- --dev-ui 2>/tmp/paraclete.log'

# wait for ready
timeout 10 bash -c \
  'until tmux capture-pane -t paraclete -p | grep -q "Launchpad"; do sleep 0.2; done'

# verify clock is running (check pattern line appears)
timeout 8 bash -c \
  'until grep -q "pattern=" /tmp/paraclete.log; do sleep 0.2; done'
grep "Kick" /tmp/paraclete.log | tail -1

# send a key (toggle Kick step 0)
tmux send-keys -t paraclete 'q'
sleep 1.5
grep "Kick.*pattern" /tmp/paraclete.log | tail -2

# quit
tmux send-keys -t paraclete 'Escape'
tmux kill-session -t paraclete 2>/dev/null
```

Expected output after toggling step 0 on Kick:

```
[dev-ui] Kick    step=Some(Int(N)) pattern=Some(Text("1000000010000000"))
[dev-ui] Kick    step=Some(Int(M)) pattern=Some(Text("0000000010000000"))
```

### 3. Unit and integration tests

```bash
cargo test                                   # all 169 tests
cargo test -p paraclete-node-api             # L2 contract tests
cargo test -p paraclete-runtime              # configurator + executor tests
cargo test -p paraclete-nodes                # node DSP and sequencer tests
cargo test -p paraclete-scripting            # Rhai builtins tests
cargo test -p paraclete-hal                  # MIDI translation tests
```

---

## Hardware setup

Connect devices before starting the app. The app detects by USB name substring:

| Device | Name substring | Fallback |
|--------|---------------|---------|
| Novation Launchpad MK2 | `"Launchpad"` | Terminal emulator |
| Elektron Digitakt | `"Digitakt"` | Skipped (no parameter control) |
| Arturia Keystep 37 | `"Keystep"` | Skipped (no bass input) |

### Launchpad MK2

Pad rows 0–7 map to tracks 0–7. Press a pad to toggle that step.
Top-right button (id 72) is mapped to Play/Stop in `profiles/launchpad.rhai`.

LED feedback from the scripts is accumulated correctly but not yet routed to the
physical device — pads reflect physical touch only, not step-active state.
This is a known P4 gap; fix is in P5.

### Digitakt (MIDI mode)

Set the Digitakt to send MIDI Out on a USB port. In `profiles/digitakt.rhai`,
encoders 0–3 control the selected track:

| Encoder | Parameter |
|---------|-----------|
| 0 | Distortion drive |
| 1 | Filter cutoff |
| 2 | Filter resonance |
| 3 | Distortion blend |

Track select buttons (8–15 on the Digitakt surface) update the selected track
in the state bus.

**Note:** A known P4 limitation means Digitakt events are tagged with the same
device ID as the Launchpad when both are connected. Both devices' handlers receive
all events; the handlers distinguish by event type (EncoderChanged vs PadPressed)
so there is no functional conflict in practice.

### Keystep 37

Notes play directly through the Bass track (track 7) without scripting.
The mod wheel sweeps the bass track's filter cutoff via `profiles/keystep.rhai`.

---

## Commands reference

```bash
# Build
cargo build
cargo build --release

# Run
cargo run                         # P4 graph, falls back to emulator
cargo run -- --dev-ui             # + step/pattern monitor on stderr

# Generate samples
cargo run -p gen-samples          # writes samples/track0–7.wav
cargo run -p gen-samples -- path/ # write to a custom directory

# Test
cargo test
cargo clippy
cargo check
```

---

## Project layout

```
crates/
  paraclete-node-api    L2 LGPL — Node API contract (third-party extensible)
  paraclete-runtime     L1 GPL  — graph, configurator/executor, state bus
  paraclete-hal         L0 GPL  — cpal audio, MIDI devices, terminal emulator
  paraclete-nodes       L3 GPL  — Sequencer, Sampler, effects, clock, gateway
  paraclete-scripting   L4 GPL  — Rhai sandbox, hardware event dispatch
  paraclete-app               — binary entry point (P4 graph)
tools/
  gen-samples/          — synthesized drum sample generator
profiles/               — Rhai hardware profile scripts
samples/                — WAV files loaded by the Sampler (not committed)
design/                 — architecture docs, ADRs, phase reports
```

Design documents: `design/architecture-core.md` for the stable reference,
`design/roadmap.md` for current phase scope and open questions.

---

## What works at P4

- 8-track graph: clock → sequencer → sampler → distortion → filter → mix → audio
- Terminal emulator: keyboard toggles steps in rows 0–2 (tracks 0–2)
- Profile scripts load and dispatch hardware events
- NodeCommand pipeline: keyboard → emulator → gateway → Rhai → CMD_TOGGLE_STEP → sequencer
- ParameterBank: CMD_SET_PARAM / CMD_BUMP_PARAM on distortion and filter nodes
- State bus subscriptions: pattern changes update the dev-ui within one clock cycle

## Known gaps at P4

- **LED feedback** not routed to physical Launchpad (P5)
- **Sampler has no ParameterBank** — encoder → pitch control not yet wired (P5)
- **Digitakt device_id tagging** — per-port routing requires ProcessInput changes (P5)
- **Signal ports** (CV, pitch modulation) return empty buffers — no CV connections yet (P5)
- **Song mode** not implemented (P5)
