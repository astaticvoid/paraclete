# AGENTS.md

## Quick-start reading order

`AGENTS.md` → `design/handoff.md` (task routing and guardrails) →
`design/roadmap.md` (current sequence) → relevant phase spec in
`design/phases/`.

## Build/test gotchas

```bash
# WRONG: builds only paraclete-app (sole default-member)
cargo build
# CORRECT:
cargo build --workspace

# WRONG: runs only paraclete-app's ~24 tests
cargo test
# CORRECT:
cargo test --workspace

# Likewise for check and clippy:
cargo check --workspace
cargo clippy --workspace

# Run a single test:
cargo test -p paraclete-runtime configurator_connect_rejects_two_node_cycle

# Pre-flight: generate drum samples (required before first `cargo run`)
cargo run -p gen-samples

# Shortcuts via .cargo/config.toml aliases:
cargo b   # = build --workspace
cargo t   # = test --workspace
cargo c   # = check --workspace
cargo cl  # = clippy --workspace
```

## Headless audio testing (ADR-033)

`tools/test-driver` renders the graph without hardware and asserts on the
result — use it to hear/verify sound changes without the app:

```bash
# Quick mode: trigger a voice, render, auto-play
cargo run -p test-driver -- --trigger kick --at 1.0 -d 3

# Scenario mode: timed commands + assertions (see tools/test-driver/tests/)
cargo run -p test-driver -- tools/test-driver/tests/kick_reverb_clean.yaml

# Regression baselines (ADR-035 Part A): fingerprint a scenario, diff later
cargo run -p test-driver -- <scenario>.yaml --update-baseline   # write <scenario>.baseline.json
cargo run -p test-driver -- <scenario>.yaml --check-baseline    # diff; exit 1 on drift
# Baseline runs use a DETERMINISTIC single-threaded render (not the wall-clock
# threaded path), so the peak/rms/dc + 50ms windowed-RMS envelope fingerprint is
# bit-stable run-to-run. Tolerances live in the .baseline.json (edit to loosen).

# Interactive mode: JSON-lines REPL for live engine interrogation
cargo run -p test-driver -- --interactive --instrument instrument.yaml
# stdin commands, one JSON object per line; responses on stdout:
#   {"cmd":"trigger","target":"kick","velocity":1.0}   engine mutations:
#   {"cmd":"set_param","target":"kick","param":"decay","value":0.3}   set/bump/
#   {"cmd":"read","path":"/node/20/param/decay"}   sequencer/chain, same as batch
#   {"cmd":"peak","window_ms":500}   read/dump/peak/render/quit are REPL-only
#   {"cmd":"dump"}   {"cmd":"render","output":"/tmp/x.wav"}   {"cmd":"quit"}
# Errors are non-fatal JSON ({"error":"..."}); the session continues.
```

Assertions: state-bus `eq`/`between`, live `peak_gte`/`peak_lt`, and
post-capture artifact scans `discontinuity_lt`/`dc_offset_lt`/`dropout_lt_ms`
(windowed by `from`/`until` seconds; NaN/Inf fail outright). Exit 0 pass,
1 assertion failure, 2 fatal. Caveat: timeline actions dispatch on wall
clock but capture time runs ~25% slower in debug builds — leave margin in
artifact windows around action times.

## Keyboard controls (terminal emulator)

When no Launchpad is connected, the 8x8 grid is keyboard-driven:

```
1 2 3 4 5 6 7 8   select active track row (0-7)
Q W E R T Y U I   toggle step pads in the active row
A S D F G H J K   scene buttons (page select; ids 64-71)
Z X C V B N M ,   top control row (modes/navigation; ids 72-79)
Tab               cycle input mode (Grid/Encoder/Piano)
Esc / Ctrl-C      quit
```

## Starting the app for paired tablet sessions

```bash
# The LaunchpadEmulator requires a TTY even with --no-tui.
# Starting in the background with & will kill the process when the
# shell exits. Use setsid to detach into a new session:

# Build and start in background (fully detached):
# Use --no-emulator for headless mode (no TTY/emulator required):
setsid cargo run --release -- --no-tui --no-emulator --theoria-dir=web/packages/app/dist \
  >> /tmp/paraclete.log 2>&1 &

# Server prints the tablet URL to stderr on startup, e.g.:
#   [paraclete] Theoria: http://192.168.4.40:7274/

# Verify it's listening:
timeout 2 bash -c 'echo >/dev/tcp/127.0.0.1/7274' && echo "up" || echo "down"

# Or keep the process alive while freeing the terminal:
# (emulator will print TUI grid to stdout but app won't crash)
```

### Shutting down and verifying audio health

```bash
# Kill paraclete
pkill -9 paraclete

# After shutdown, verify pipewire sink is still real hardware.
# If ONLY auto_null shows, pipewire is stranded — restart it.
# (Known issue: paraclete can leave pipewire on a null sink after heavy use.)
# Linux only:
count=$(pactl list short sinks 2>/dev/null | grep -c alsa_output)
if [ "$count" -eq 0 ]; then
    echo "WARNING: no real audio sink — restarting pipewire"
    systemctl --user restart pipewire pipewire-pulse
fi
```

## Architecture: five-layer model

No layer may reach across another. Hard constraint.

| Layer | Crate | License | Role |
|-------|-------|---------|------|
| L0 HAL | `paraclete-hal` | GPL3 | Audio I/O (cpal), MIDI, terminal emulator |
| L1 Runtime | `paraclete-runtime` | GPL3 | Node graph, scheduling, clock federation |
| L2 Node API | `paraclete-node-api` | **LGPL3** | Contract every node implements; third-party boundary |
| L3 Nodes | `paraclete-nodes` | GPL3 | Sequencer, engines, effects, samplers |
| L4 Scripting | `paraclete-scripting` | GPL3 | Rhai sandbox, profile scripts |
| App | `paraclete-app` | GPL3 | Binary entry point, graph wiring |

Platform crates (outside the five layers):
- `paraclete-antiphon` — interface server (WebSocket + HTTP, no tokio)
- `paraclete-clap` — Paraclete-as-CLAP-plugin (machine bank `.clap` binaries)
- `paraclete-clap-host` — Paraclete-as-CLAP-host (loads third-party `.clap` plugins as nodes)
- `paraclete-tui` — ratatui terminal UI
- `paraclete-graph-nodes` — nodes that own an inner `NodeExecutor` (only crate allowed to depend on both `paraclete-nodes` and `paraclete-runtime`)

## Configurator / Executor split

- **`NodeConfigurator`** — main thread. Owns graph topology (petgraph DAG),
  manages node lifecycle. Sends incremental changes over a lock-free ring buffer.
- **`NodeExecutor`** — audio thread. Receives `ConfigMessage`s, executes nodes
  in topological order, sums audio output. Never allocates, blocks, or takes a mutex.

## Hard constraints

1. **Audio thread:** `process()` must never allocate, block, or take a lock.
   JSON never touches the audio thread.
2. **Layer boundaries** (see above). The LGPL3 boundary at L2 is the third-party
   extensibility contract.
3. **DAG:** `connect()` rejects cycles unless exactly one `LoopBreakNode` is in
   the cycle.
4. **Every component is a Node** (ADR-018). No non-node platform objects.
5. **No tokio.** Blocking tungstenite + rtrb only. No Web MIDI as primary transport.
6. **DSP source policy:** MIT/Apache-licensed or written from scratch.
   Mutable Instruments firmware (MIT) is primary reference.
7. **Naming:** third-party marks never appear in feature names, identifiers, or
   UI strings. House vocabulary: Antiphon, Theoria, kerygma, *pages*, *grid*, *chain*.

## Non-obvious conventions

- **`NodeConfigurator` has 4 registrations:** `add_node()` (standard),
  `add_node_tagged()` (preferred — stores type_tag for v2 projects),
  `add_surface()` (controllers), `add_tempo_source()` (clock master).
- **Parameter names are canonical across all nodes.** Use `const CUTOFF_ID: u32 =
  ParamDescriptor::id_for_name("cutoff");` — the function is `const fn`.
  Canonical: `"cutoff"`, `"resonance"`, `"drive"`, `"wet"`, `"dry"`, `"decay"`,
  `"attack"`, `"release"`, `"tune"`.
- **`published_state()` push-down:** accepts `&mut Vec<(String, StateBusValue)>`,
  pushes into it. The old returning signature is forbidden (allocates per cycle).
- **`deserialize()` AFTER `activate()`** for ParameterBank nodes. `activate()`
  resets the bank to defaults; `deserialize()` re-applies saved values on top.
- **ParamLock must NOT go through `bank.handle_commands()`.** Route to a
  per-cycle `node_locks: Vec<(u32, f64)>` cleared at the top of each `process()`;
  check your param getter against it before falling back to the bank. Otherwise
  the locked value bleeds into subsequent steps.
- **`serde_yml` not `serde_yaml`.** `serde_yaml` was removed in P9; do not add it back.
- **`SurfaceOutputHandle` pattern:** implement `take_output_handle()` returning
  `Some(Box<dyn SurfaceOutputHandle>)`. The handle is ticked on the main thread,
  not the audio thread. Use this for all new hardware nodes.
- **CLAP host uses `clap-sys` + `libloading`**, not the `clack` crate.
- **`publish_bank_state()` caches paths in `OnceLock`** — no `format!` on the
  audio thread after the first cycle (BUG-007).

## Event delivery ordering

Within the same `sample_offset`, the executor delivers events in this order:

1. `ParamLockEvent` — parameter overrides before note triggers
2. `TransportEvent` — position updates
3. `Midi2` — notes and controllers
4. `Surface` — pad events
5. `Extended` — custom

`ParamLockEvent` is routed by `node_id` match, not graph edges.

## StateBus canonical paths

| Path | Meaning | Writer |
|------|---------|--------|
| `/node/{id}/param/{name}` | live parameter value | `publish_bank_state()` |
| `/node/{id}/state/{key}` | node-internal state | node `published_state()` |
| `/transport/*` | clock domain state | clock |
| `/context/*` | encoder context | profiles |
| `/surface/{id}/*` | per-surface state (was `/hw/*`) | devices |
| `/script/*` | profile scratch; numeric values mirrored to Antiphon | scripts |

## App graph node IDs (hard-coded)

| ID | Node |
|----|------|
| 1 | `InternalClock` |
| 2 | `MixNode` |
| 10–17 | `Sequencer[0–7]` |
| 20–22 | `AnalogEngine` (kick/snare/hihat) |
| 23–26 | `Sampler[3–6]` |
| 27 | `FmEngine::bass()` |
| 30–37 | `DistortionNode[0–7]` |
| 40–47 | `FilterNode[0–7]` |
| 101–106 | Surface nodes (LaunchpadEmulator, Launchpad, DigitaktMidi, Keystep, SurfaceMapping, TheoriaSurface) |
| 110–113 | `ScriptingGatewayNode` (LP, DT, KS, Theoria) |
| 200 | `ReverbNode` |

## Main loop sequence (order matters)

Each ~1 ms iteration:
1. `conf.process_main_thread()` — drain state bus SPSC; tick SurfaceOutputHandles
2. Drain per-device gateway SPSCs into shared event buffer
3. `scripting.dispatch_surface_event(ev)` — dispatch to Rhai handlers
4. `scripting.process_subscriptions(&bus)` — fire state bus callbacks
5. `conf.send_command(cmd)` — flush NodeCommands to audio thread
6. `conf.deliver_script_output(led_output)` — route LED/output to hardware

## Sequencer node commands (beyond CMD_SET_PARAM/CMD_BUMP_PARAM)

| type_id | Constant | Purpose |
|---------|----------|---------|
| 16 | `CMD_TOGGLE_STEP` | arg0: step index |
| 17 | `CMD_SET_STEP` | arg0: step, arg1: < 0 = off, ≥ 0 = note |
| 18 | `CMD_CLEAR` | — |
| 19 | `CMD_TRIGGER` | cross-instrument trigger; arg0: note (< 0 = default), arg1: velocity 0-1 |
| 23 | `CMD_SET_FILL_A` | — |
| 24 | `CMD_SET_FILL_B` | — |
| 25 | `CMD_SET_STEP_TIMING` | arg0: step, arg1: micro_offset (i8, ±47 ticks) |
| 26 | `CMD_SET_STEP_CONDITION` | packed probability/repeat/fill |
| 27 | `CMD_SET_PATTERN` | arg0: pattern index |
| 28 | `CMD_SET_LENGTH` | arg0: steps (1–64), arg1: pattern index |
| 29 | `CMD_SET_SPEED` | arg1: speed multiplier 0.125–2.0 |
| 30 | `CMD_SET_PAGE_LOOP` | arg0: start_page, arg1: end_page |
| 31 | `CMD_CHAIN_PUSH` | arg0: pattern index (volatile, capacity 8) |
| 32 | `CMD_CHAIN_CLEAR` | — |

## Web client (Theoria)

```bash
cd web && npm install && npm run build    # → web/packages/app/dist
cargo run -- --theoria-dir=web/packages/app/dist
cargo build -p paraclete-app --release --features embed-ui  # embed dist/ in binary
```

Phone adaptation: `@media (max-width: 700px)` in `styles.css` restacks the
W2 layout vertically (track column → top strip, page nav + transport wrap to
two rows); TRIG pads wrap to ~160 px columns in `Grid.tsx` (keyed on the
same breakpoint via `matchMedia`, never on container width); encoder canvas
fonts scale with cell width. `@media (max-height: 500px)` compacts the rail
and pads `safe-area-inset-left/right` for landscape phones. Tablet layout is
untouched — verify any rail/grid CSS change at 390×844, 768×1024 (iPad
portrait), and 1024×768 before committing.

## Commit workflow

**The agent must proactively commit after each logical unit of work** (bug
fix, feature, doc update). Do NOT leave uncommitted changes accumulating across
sessions — stacked dirty files from multiple sessions become impossible to
untangle. Pushing to a remote still requires explicit user approval.

Every commit: `cargo test --workspace` green, `cargo clippy --workspace` clean
on touched crates. Design/doc changes in separate commits from code. Phase
reports and `bugs.md` are append-only.

**After each implementation commit** (or logical batch of commits), the agent
must pause and offer a **subagent code review** before proceeding to the next
commit or closing the session. The subagent reads the diff (or the affected
files), checks conformance to ADRs, layer boundaries, audio-thread rules,
naming conventions, and test coverage, and returns findings. Do not skip this
step — it is the quality gate between implementation pushes.

**Before closing a session**, the agent must report any untracked files,
uncommitted changes, or stale trackers. The working tree must be either clean
or explicitly accounted for — never silent about dirt.

After every implementation session, the agent must explicitly propose which
design documents need updating, then update **all** that apply before the
session is done — not just the obvious one. This is a mandatory check, not a
suggestion. Keep-current set (with what changes in each):

| Doc | Update when… |
|-----|--------------|
| `design/roadmap.md` | a phase/rank ships, is reprioritized, or a status changes |
| `design/bugs.md` | a bug/INFRA item is found, resolved, or a gating assumption changes (append-only; also refresh the top Status block) |
| `design/adr/*` | a decision is **implemented** — update its `Status:` line and add an implementation note. The decision/context/alternatives body stays append-only (see below) |
| phase reports (`design/phases/*`) | a phase commit lands (append-only) |
| `AGENTS.md` | a workflow, command, tool mode, node ID, or convention changes (e.g. a new test-driver mode) |
| `design/handoff.md` | task routing or a model-tier guardrail changes |

If a change touches code *and* the tool/tracker/roadmap that describe it, all of
those are in scope in the same session — a code commit that leaves the tracker
stale is an incomplete session.

## Design documents

- `design/handoff.md` — task routing by model tier; read before implementing
- `design/roadmap.md` — current phase scope and sequence
- `design/bugs.md` — append-only bug tracker
- `design/adr/` — Architecture Decision Records. The **decision/context/
  alternatives body is append-only** — never rewrite a past decision. The
  `Status:` line and an appended implementation note *are* updated when the ADR
  is implemented (e.g. ADR-033 `proposed → accepted`).
- `design/review/` — post-phase code reviews and latent-issue audits
- `design/phases/` — per-phase specs and implementation reports (append-only)

`Hardware*` was renamed to `Surface*` in July 2026. Historical docs use old
names — map accordingly; do not edit those documents.
