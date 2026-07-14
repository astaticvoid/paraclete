# ADR-033 — Headless Test Driver

**Status:** accepted — batch, quick, and interactive modes implemented
(interactive mode 2026-07-13, `92b8795`)
**Date:** 2026-07-11

> **Implementation note (2026-07-13):** batch + quick modes shipped earlier
> (INFRA-001 artifact assertions, INFRA-002 quick mode). Interactive mode now
> lands: a JSON-lines REPL sharing the batch engine stack via `build_context`,
> with a dedicated stdin reader thread feeding the ~1 ms main loop. One
> deviation from the spec below: the reader→main channel is an unbounded
> `std::sync::mpsc`, not the bounded `rtrb` SPSC of the threading-model section
> — the "main thread never blocks on stdin" guarantee still holds, and an
> unbounded queue loses no commands under a burst (see Open Question 1, now moot).
> Still unbuilt from the surrounding debug-posture push: audio diff/snapshot
> baselines and the structured per-node log channel (both roadmap Rank 2).
>
> **Implementation note (2026-07-13):** auto-play now uses platform-appropriate
> commands via `auto_play_command()`: `afplay` on macOS, `pw-play` on Linux.
> Previously the Linux path called `afplay` unconditionally, which always failed
> and required manual playback. Tested on Arch/KDE with PipeWire.

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
7. Assert on state bus values and audio metrics (agent-usable pass/fail)
8. Provide an interactive debug harness for live engine interrogation

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

Two modes, one binary:

1. **Batch mode** — load a YAML scenario, execute the timeline, check assertions,
   write WAV, exit with pass/fail code. Use for CI, regression tests, agent
   verification.

2. **Interactive mode** — stdin/stdout JSON REPL. The agent sends commands and
   polls state while audio renders in the background. Use for debugging,
   exploration, live engine interrogation.

Both modes share the same engine stack: instrument load → graph build →
executor spawn → audio capture thread.

#### Threading model

The core problem: blocking stdin kills the state bus drain loop. If the main
thread blocks on `stdin().read_line()`, the ring buffer from the audio thread
fills and silently drops state bus updates.

**Solution: dedicated reader thread.** A background thread reads stdin
line-by-line and sends parsed commands to the main thread via an SPSC channel
(`rtrb`). The main thread runs a tight ~1ms loop identical to batch mode:

```
Reader thread                Main thread                    Audio thread
─────────────                ───────────                    ────────────
block on stdin               build graph                    spawn ──→
    ↓                        build executor                    loop:
parse JSON command           ───────────────────────────        executor.process(&mut block)
    ↓                        loop (~1ms):                       capture_buf[write_idx] = block
send to channel                  try_recv channel              write_idx = (write_idx+1) % CAP
    ↓                            → if cmd: dispatch               ↓
loop                            drain state bus               loop
                                process_main_thread()
                                poll assertions
                                ↓
                            quit or duration elapsed → running = false
                            join threads
                            write WAV
                            play via afplay / ffplay
```

Key: the main thread never blocks. The reader thread never touches the engine.
State bus drain runs every ~1ms regardless of stdin activity.

#### Lock-free capture buffer

The capture buffer is a pre-allocated ring buffer (`[Vec<f32>; N]`) with an
atomic write index. The audio thread writes each block to the next slot and
advances the index. The main thread reads accumulated samples by checking
the index. No mutex in the audio path. Capacity: 4 seconds at 44.1kHz mono
(689 slots at block_size=512 — ~689 kB, trivially small).

```
Audio thread                          Main thread
────────────                          ───────────
slot = write_idx % CAP;               read_idx = last_read;
capture_buf[slot] = block_samples;    while read_idx < write_idx.load() {
write_idx.store(slot+1, Release);         process(capture_buf[read_idx % CAP]);
                                          read_idx += 1;
                                      }
```

#### Interactive mode protocol (JSON-lines)

Each message is one JSON object per line on stdin; responses on stdout.
Audio renders continuously; state is read on demand (poll, no push).

```jsonc
// ── Errors ───────────────────────────────────────────────────
// Invalid commands or targets return error:
// → {"error": "unknown command: bad_cmd"}
// → {"error": "target not found: nonexistent"}
// → {"error": "parameter 'decay' not found on node 20"}
// → {"error": "node 10 does not implement CMD_TRIGGER"}

// ── Set a parameter ──────────────────────────────────────────
{"cmd": "set_param", "target": "kick", "param": "decay", "value": 0.3}
// → {"ok": true}

// ── Live trigger ──────────────────────────────────────────────
{"cmd": "trigger", "target": "kick", "note": 36, "velocity": 1.0}
// → {"ok": true}
{"cmd": "trigger", "target": "kick"}
// → {"ok": true}  // note defaults to engine's last_note, velocity to 0.79

// ── Sequencer control ─────────────────────────────────────────
{"cmd": "toggle_step", "target": "seq0", "step": 3}
// → {"ok": true}

{"cmd": "set_pattern", "target": "seq0", "pattern": 2}
// → {"ok": true}

// ── Poll state bus (on-demand, no push subscription) ──────────
{"cmd": "read", "path": "/node/10/state/current_step"}
// → {"path": "/node/10/state/current_step", "value": 3}

{"cmd": "read", "path": "/node/20/param/decay"}
// → {"path": "/node/20/param/decay", "value": 0.3}

// Nonexistent path:
{"cmd": "read", "path": "/nonexistent"}
// → {"error": "no value at path /nonexistent"}

// ── Read audio peak ───────────────────────────────────────────
{"cmd": "peak", "window_ms": 500}
// → {"peak": 0.34, "window_ms": 500}

// ── Dump all known state bus paths ────────────────────────────
{"cmd": "dump"}
// → {"paths": {"/node/10/state/current_step": 3, "/node/20/param/decay": 0.3, ...}}

// ── Render accumulated audio to WAV ───────────────────────────
{"cmd": "render", "output": "/tmp/debug.wav"}
// → {"ok": true, "output": "/tmp/debug.wav", "samples": 132096}

// ── Shutdown ─────────────────────────────────────────────────
{"cmd": "quit"}
// → {"ok": true}
```

`read` returns the value cached from the last `process_main_thread()` drain.
May be up to ~1ms stale. `dump` returns all currently known paths (same source).
`peak` scans the lock-free capture buffer up to the current write index.
`render` writes all captured audio to a WAV and does NOT reset the buffer
(use for checkpoints during a long session). The buffer is a ring with a
4-second capacity; old blocks are overwritten.

#### Error responses

All commands can return `{"error": "..."}` instead of `{"ok": true}`. Exit
code 2 is reserved for fatal errors (instrument not found, graph build
failure, audio thread panic). Interactive mode errors are non-fatal — the
session continues.

#### Null audio backend

The audio thread is a simple loop: allocate block buffer, call
`executor.process(&mut block, channels)`, write to capture ring. No cpal
stream, no audio device. This lives in the test driver itself — NOT in
`paraclete-hal` (L0). The L0 HAL is for hardware I/O; a null backend is purely
a test concern.

```
let mut block = vec![0.0f32; block_size * channels];
while running.load(Ordering::SeqCst) {
    let mut ex = executor.lock().unwrap(); // Mutex only on process() call, dropped after
    ex.process(&mut block, channels);
    drop(ex);
    // push block to lock-free capture ring
}
```

#### Node ID resolution: track names vs numeric IDs

The `target` field accepts either:
1. An integer node ID (e.g., `20`) — used directly
2. A string track name (e.g., `"kick"`) — resolved from the loaded instrument

Resolution order:
1. Exact match against `node.display_name` (case-insensitive)
2. Exact match against `node.type_tag` (only if display_name is None)
3. If no match → error response

If multiple nodes share the same resolved name, the first match wins with a
warning on stderr. Ambiguity is a configuration problem, not a resolution bug.

#### Added actions (from review)

| Action | Maps to | Args |
|---|---|---|
| `set_fill_a` | `CMD_SET_FILL_A` (23) | target, active (bool) |
| `set_fill_b` | `CMD_SET_FILL_B` (24) | target, active (bool) |
| `set_step_condition` | `CMD_SET_STEP_CONDITION` (26) | target, step, probability (u8), repeat_n (u8), repeat_m (u8), fill (u8) |
| `set_length` | `CMD_SET_LENGTH` (28) | target, steps, pattern (optional, default: active) |

### Project file verification

Investigated (2026-07-11): AnalogEngine, FmEngine, FilterNode, DistortionNode,
and ReverbNode all own `ParameterBank` but inherit the default `serialize()`
returning `vec![]`. Their parameters are NOT persisted in project files. Only
Sampler overrides `serialize()` to capture its bank. The ADR's premise that
project files cannot express "kick decay = 0.3, drive = 0.5" is correct.

### CLAP plugin limitation (v1)

The `build_from_instrument()` function in `paraclete-app` accepts a
`&HashMap<String, Arc<PluginLibrary>>` parameter for CLAP plugin instruments.
CLAP plugin loading logic (`load_plugin_libraries()`) lives in `main.rs` (a
binary target), not in the library. The test driver cannot load CLAP plugins
in v1. Documented limitation; move the loading logic into `paraclete-app`'s
library in a follow-up.

### WAV clipping protection

Samples are clamped to `[-1.0, 1.0]` before i16 conversion. The header is
written at the end of capture (total size known) — no seek-back needed.

### Quick mode parsing

The `--trigger` / `--at` quick mode uses `clap` (already a workspace dependency
via crossterm's transitive deps, no new crate). Positional `--at` values are
paired positionally with `--trigger` values. Mismatched counts produce a CLI
error before execution starts.

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

## Hostile review resolution (2026-07-11)

Critical issues found and fixed in spec:

| # | Issue | Resolution |
|---|---|---|
| 1 | Blocking stdin kills state bus drain | Dedicated reader thread + channel; main loop polls both |
| 2 | `watch` incompatible with polling model | Replaced with `read`/`poll` — agent polls on demand |
| 3 | `Arc<Mutex<>>` on capture glitches audio | Lock-free atomic ring buffer; no Mutex in audio path |
| 4 | Project file premise unverified | Verified: 5 of 6 nodes don't serialize ParameterBank; premise correct |
| 5 | CLAP plugins unsupported | V1 limitation; move loading logic to library in follow-up |

High issues fixed:

| # | Issue | Resolution |
|---|---|---|
| 8 | No surface node policy | V1 skips surface nodes with warning |
| 11 | Missing fill/condition commands | Added `set_fill_a`, `set_fill_b`, `set_step_condition` |
| 13 | Track name ambiguity | Specified resolution order; first match wins with warning |
| 15 | WAV no clipping | Samples clamped to [-1.0, 1.0] before i16 |
| 21 | Quick mode CLI ambiguous | Uses clap; mismatched counts error before execution |
| 25 | Null backend in L0 wrong | Moved to test driver itself, not paraclete-hal |
| 33 | Unbounded capture buffer growth | Ring buffer, 4-second capacity |

Remaining acknowledged:

| # | Issue | Disposition |
|---|---|---|
| 9 | bump_param delta unvalidated | Deferred — ParameterBank clamps internally |
| 10 | trigger on non-instrument nodes | Error response: `"node N does not implement CMD_TRIGGER"` |
| 16 | Co-timed trigger/set_param ordering | Documented: FIFO ring buffer order; same-timestamp actions dispatch in YAML order |
| 17 | peak command scans under reader-thread drain | Acceptable — scans are µs, drain is ~1ms, no audio thread contention |
| 20 | Assertion timing precision ±11ms | Acceptable — same as production app timing |
| 22 | Stereo capture loses panned audio | Documented: mono capture from left channel only (stereo: v2) |
| 34 | set_length missing pattern arg | Fixed: pattern optional, defaults to active |
| 35 | StateBusValue::Text with newline breaks JSON | Not an issue — state bus values are numeric or short ASCII; Text values with newlines rejected at publish time |

## Open questions

1. **Interactive mode channel backpressure**: if the agent sends commands
   faster than the main loop can process them (e.g., 100 commands in one burst),
   the SPSC channel from the reader thread may fill. Capacity TBD — 64 entries
   gives ~64ms of burst headroom at 1ms drain rate. Overflow policy: drop
   oldest (FIFO) or block the reader thread? Drop oldest with a warning on
   stderr is the conservative default.

2. **Stereo output**: V1 captures left channel only. V2 should expose
   per-named-output capture (e.g., `{"cmd":"record","node":"kick","duration":3}`)
   for stem rendering. Deferred.

3. **Project file integration**: The test driver should also accept a project
   file (`--project=file.ron`) to load topology + sequencer state, then layer
   timeline actions on top. Deferred — requires fixing engine param
   serialization first (BUG-027).

4. **surface/hardware node policy**: What happens when a loaded instrument
   contains surface nodes (e.g., Launchpad)? The executor calls `process()` on
   them. For MIDI nodes with no connection, `activate()` may fail or they may
   no-op. V1 policy: surface nodes are skipped at graph build time with a
   warning. They don't participate in audio capture and don't block execution.
