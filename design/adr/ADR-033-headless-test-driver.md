# ADR-033 — Headless Test Driver

**Status:** proposed (not yet implemented)
**Date:** 2026-07-11

## Context

Testing sound changes currently requires either (a) editing engine code and
writing `#[test]` functions that call engine internals, or (b) running the full
app and manually tapping keys while listening. Neither supports the workflow:
"load this sequence with these params, trigger this voice rapidly, let me hear
the result."

The project file format (ADR-025) persists topology and sequencer state but
does NOT persist engine/filter/effect parameter banks — those are reset to
defaults on load. A saved project can't capture "kick decay = 0.3, drive = 0.5"
without also programming per-step param locks, which is the wrong mechanism for
base-level sound design.

A headless test driver is needed that can:
1. Load instrument topology (from instrument.yaml or a project file)
2. Set any node's parameters to any values
3. Program sequencer state (steps, patterns, lengths)
4. Send live triggers at specific times
5. Render audio to a WAV file
6. Play it back immediately

No TUI. No hardware. No editing engine code.

## Decision

Create a `tools/test-driver/` binary that uses existing crate APIs. Input is a
versioned test scenario file (YAML, `format_version: 1`) that specifies
instrument to load, a timeline of timed actions, and render/play configuration.

### Why not just use project files?

Project files (ADR-025, RON) persist topology and sequencer step data but do
NOT persist non-sequencer parameter banks. A test scenario needs to set
`AnalogEngine::kick().decay = 0.3` before the first trigger — this requires
sending `CMD_SET_PARAM`, which project files don't capture. The format
overlap is limited to the instrument reference and sequencer steps — those can
be delegated to the instrument.yaml or sent as actions.

### Why a new format instead of Rhai?

Rhai profiles already drive the running app, but they can't capture audio
output or run headless without the full app loop. A dedicated binary is
simpler for the audio-capture case and doesn't require the scripting engine.

### Input format

Versioned YAML. `format_version: 1`.

```yaml
format_version: 1

# Required: instrument file to load (YAML, as used by `cargo run -- --instrument=...`)
instrument: instrument.yaml

# Optional: sample rate / block size (default: 44100 / 512)
sample_rate: 44100
block_size: 512

# Required: seconds to run after all timeline actions complete
duration_secs: 4

# Optional: output path (default: /tmp/paraclete_test.wav)
output: /tmp/kick_test.wav

# Optional: auto-play via system player (default: true)
play: true

# Timeline: actions executed at specific wall-clock seconds from start
timeline:
  # ── Parameter control ─────────────────────────────────
  - at: 0.0
    set_param:
      target: kick
      param: decay
      value: 0.3

  - at: 0.0
    set_param:
      target: kick
      param: drive
      value: 0.5

  - at: 1.0
    bump_param:
      target: filt0
      param: cutoff
      delta: 0.1

  # ── Sequencer programming ──────────────────────────────
  - at: 0.0
    toggle_step:
      target: seq0
      step: 0

  - at: 0.0
    toggle_step:
      target: seq0
      step: 4

  - at: 0.0
    toggle_step:
      target: seq0
      step: 8

  - at: 0.0
    toggle_step:
      target: seq0
      step: 12

  - at: 0.0
    set_step:
      target: seq0
      step: 0
      note: 36          # < 0 = off, ≥ 0 = note number

  - at: 0.0
    set_step_timing:
      target: seq0
      step: 4
      micro_offset: -12 # i8, ±47 ticks

  - at: 0.0
    set_pattern:
      target: seq0
      pattern: 1

  - at: 0.0
    set_length:
      target: seq0
      steps: 32

  - at: 0.0
    set_speed:
      target: seq0
      speed: 0.5

  - at: 0.0
    set_page_loop:
      target: seq0
      start_page: 0
      end_page: 1

  - at: 0.0
    clear:
      target: seq0

  # ── Live triggers ──────────────────────────────────────
  - at: 2.0
    trigger:
      target: kick
      note: 36          # optional, < 0 = engine default
      velocity: 1.0     # optional, default 0.79

  - at: 2.05
    trigger:
      target: kick

  - at: 2.10
    trigger:
      target: kick

# Optional: probe state bus values for debugging (logged to stderr)
probe:
  - at: 2.0
    path: "/node/10/state/current_step"
```

### Action reference

All actions support `target` as either:
- A numeric node ID (e.g. `20` for kick engine)
- A track name resolved from the instrument definition's `display_name` fields
  (e.g. `kick`, `snare`, `seq0`, `filt0`)

| Action | Maps to | Args |
|---|---|---|
| `set_param` | `CMD_SET_PARAM` (0) | target, param name, value |
| `bump_param` | `CMD_BUMP_PARAM` (1) | target, param name, delta |
| `trigger` | `CMD_TRIGGER` (19) | target, note (opt), velocity (opt) |
| `toggle_step` | `CMD_TOGGLE_STEP` (16) | target, step index |
| `set_step` | `CMD_SET_STEP` (17) | target, step, note |
| `clear` | `CMD_CLEAR` (18) | target |
| `set_pattern` | `CMD_SET_PATTERN` (27) | target, pattern index |
| `set_length` | `CMD_SET_LENGTH` (28) | target, steps |
| `set_speed` | `CMD_SET_SPEED` (29) | target, speed (0.125–2.0) |
| `set_page_loop` | `CMD_SET_PAGE_LOOP` (30) | target, start_page, end_page |
| `set_step_timing` | `CMD_SET_STEP_TIMING` (25) | target, step, micro_offset (±47) |
| `chain_push` | `CMD_CHAIN_PUSH` (31) | target, pattern index |
| `chain_clear` | `CMD_CHAIN_CLEAR` (32) | target |

### Architecture

```
tools/test-driver/
├── Cargo.toml        — depends on paraclete-app, paraclete-runtime,
│                       paraclete-node-api, paraclete-hal, serde_yml
└── src/
    ├── main.rs       — CLI entry, orchestration
    ├── scenario.rs   — deserialize test scenario YAML (format_version gated)
    ├── resolve.rs    — track name → node ID resolution from instrument def
    └── wav.rs        — hand-rolled 16-bit mono WAV writer (no hound dep)
```

The binary is NOT added to `default-members` — it's a tool, not a shipping
binary. Built with `cargo build -p test-driver` or run with
`cargo run -p test-driver`.

Audio capture: a spawned thread calls `executor.process()` on a per-block
output buffer and pushes samples into an `Arc<Mutex<Vec<f32>>>`. The Mutex is
acceptable because this is a test tool, not a real-time audio path. The main
thread sends commands via `conf.send_command()` (lock-free ring buffer) and
drains the state bus via `conf.process_main_thread()`.

```
Main thread                         Audio thread
──────────                          ────────────
build graph
build executor
                                    spawn ──→ loop:
sleep(~1ms)                                     executor.process(&mut block)
  drain state bus                               capture.lock().push(block)
  dispatch timeline actions                         ↓
  send commands via ring buffer                   loop
    ↓
  (ring buffer → executor)
    ↓
after duration: running = false
join audio thread
write WAV
play via afplay / ffplay
```

### CLI

```bash
# Render and play a scenario
cargo run -p test-driver -- test.yaml

# Render only, don't play
cargo run -p test-driver -- test.yaml --no-play

# Override output path
cargo run -p test-driver -- test.yaml -o /tmp/my_test.wav
```

### Quick mode (one-liner, no YAML file)

For rapid iteration without writing a YAML file:

```bash
# Multiple triggers at specific times
cargo run -p test-driver -- -i instrument.yaml \
  --trigger kick --trigger kick --trigger kick \
  --at 0.0 --at 0.05 --at 0.10 \
  -d 3

# Set params + trigger
cargo run -p test-driver -- -i instrument.yaml \
  --set-param kick decay 0.3 \
  --trigger kick --at 0.0 \
  -d 2
```

Quick mode converts CLI flags to an in-memory `Scenario` struct internally.

## Consequences

### Audio capture thread

`NodeExecutor::process()` requires `&mut self`. We wrap it in `Arc<Mutex<>>`
for the capture thread. The main thread never calls `process()` — it only
sends commands through the ring buffer and drains the state bus. This is safe.

The capture captures only the left channel (mono) for simplicity. The default
instrument outputs stereo; left channel is the summed mix.

### No `hound` dependency

The WAV writer is hand-rolled (~40 lines for 16-bit mono PCM header + data).
This avoids adding a dependency to the workspace for one tool's output format.

### Assertions (agent-usable)

An agent needs to verify correctness, not just listen. The `assert:` section
enables pass/fail exit codes:

```yaml
assert:
  # State bus: exact match
  - at: 2.0
    path: "/node/10/state/current_step"
    eq: 0

  # State bus: numeric range
  - at: 3.0
    path: "/node/20/param/decay"
    between: [0.25, 0.35]

  # Audio: not silent (peak sample in last 500ms)
  - at: 2.0
    peak_gte: 0.05
    window_ms: 500

  # Audio: no clipping
  - at: 3.0
    peak_lt: 0.99
    window_ms: 1000
```

Exit codes: **0** = all assertions passed + audio rendered, **1** = assertion
failure (logs which one failed), **2** = error (instrument not found, graph
build failure, etc.).

The `probe:` section remains for non-failing debug output.

State bus assertions check the value at the time the main loop reaches the
`at` timestamp (via `conf.state_bus_read(path)`). Audio assertions scan the
captured buffer from `at_secs - window_ms` to `at_secs` and check peak.
Audio assertions are only available when `duration_secs` covers the check
window.

### Workspace membership

`tools/test-driver/` is added to the workspace members list but NOT to
`default-members`. It's built only when explicitly targeted.

## Alternatives considered

### Using project files (RON) as input

Rejected. Project files don't persist engine/filter/effect parameter banks —
they reset to defaults on load. A test scenario needs to set "kick decay =
0.3" before the first trigger, which requires `CMD_SET_PARAM`. The project
file format would need extending to cover all node parameter banks, which is
out of scope for a test tool.

### Extending Rhai profiles

Rejected. Rhai profiles run in the full app with TUI, hardware, and the main
loop. Capturing audio output from within a Rhai script would require adding
audio-capture APIs to the scripting engine, which is a larger change than a
dedicated binary.

### Adding to paraclete-app as a CLI mode

Rejected. The app already has `--no-tui` but still uses cpal for audio output.
Adding audio capture and test scenario parsing to the main binary would
couple test infrastructure to the shipping binary. A separate tool is cleaner.

## Open questions

1. **Track name resolution scope**: Should track names resolve from the
   `display_name` field in instrument.yaml, or from a hardcoded mapping? The
   instrument file is the authority — `display_name` is the contract.

2. **Stereo output**: V1 captures left channel only. Should V2 expose
   per-track stems (capture each node's output separately)?

3. **Project file integration**: Should the test driver also accept a project
   file (`--project=file.ron`) to load topology + sequencer state, then layer
   timeline actions on top? V2 question.
