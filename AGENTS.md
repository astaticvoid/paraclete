# AGENTS.md

## Quick-start

```bash
gh api repos/:owner/:repo/milestones --jq '.[] | "\(.number)  \(.title)  open=\(.open_issues)"'
gh issue list --milestone "<title>"
gh issue list --state open --label bug
```

Live state lives in GitHub Issues. `design/` holds append-only authority: ADRs, phase specs/reports, session notes, `roadmap.md`.

Resolve `BUG-###` / `INFRA-###` from code comments: `gh issue list --search "BUG-042 in:title" --state all`. `ADR-###` resolves in-repo via `design/adr/INDEX.md`.

`CLAUDE.md` is a symlink to this file.

## Build/test

```bash
cargo build --workspace     # bare `cargo build` only builds paraclete-app
cargo test --workspace      # bare `cargo test` runs only 32 of 991 tests
cargo check --workspace
cargo clippy --workspace

cargo test -p <crate> <test_name>   # single test
cargo run -p gen-samples            # pre-flight: generate drum samples

# Aliases: cargo b / cargo t / cargo c / cargo cl
```

Clippy is judged against a baseline, not zero. Capture before/after, touch changed files, diff.

## Logging

`env_logger` at top of `main()`. Use `log::info!`/`warn!`/`error!` — no bare `eprintln!`/`println!` in library code. **Default is `error`** — `log::info!` is silent until `RUST_LOG=info`.

## Architecture

Five-layer model. No layer may reach across another.

| Layer | Crate | License |
|-------|-------|---------|
| L0 HAL | `paraclete-hal` | GPL3 |
| L1 Runtime | `paraclete-runtime` | GPL3 |
| L2 Node API | `paraclete-node-api` | **LGPL3** (third-party boundary) |
| L3 Nodes | `paraclete-nodes` | GPL3 |
| L4 Scripting | `paraclete-scripting` | GPL3 |
| App | `paraclete-app` | GPL3 |

Platform crates (all GPL3): `paraclete-antiphon`, `paraclete-clap`, `paraclete-clap-host`, `paraclete-tui`, `paraclete-theotokos`, `paraclete-view-assembly`, `paraclete-graph-nodes`, `paraclete-machine-*`.

**Configurator/Executor split:** `NodeConfigurator` (main thread, owns DAG) sends `ConfigMessage`s via lock-free rtrb to `NodeExecutor` (audio thread, executes nodes). The audio thread never allocates, blocks, or takes a lock.

## Hard constraints

1. `process()` must never allocate, block, or take a lock. JSON never touches the audio thread.
2. No layer reaches across another.
3. DAG rejects cycles unless exactly one `LoopBreakNode` is in the cycle.
4. Every component is a Node (ADR-018).
5. No tokio. Blocking tungstenite + rtrb only.
6. DSP source: MIT/Apache-licensed or from scratch. Mutable Instruments firmware (MIT) is primary reference.
7. Third-party marks never in feature names, identifiers, or UI strings.

## Key conventions

- **Parameter names are canonical:** `ParamDescriptor::id_for_name("cutoff")` is `const fn`. Canonical: `"cutoff"`, `"resonance"`, `"drive"`, `"wet"`, `"dry"`, `"decay"`, `"attack"`, `"release"`, `"tune"`, `"machine"`.
- **`"machine"` is an identity param** (ADR-041) — stepped selector, rejected as p-lock target.
- **`published_state()` push-down:** `&mut Vec<(String, StateBusValue)>`, push into it. Never allocate per cycle.
- **`deserialize()` AFTER `activate()`** for ParameterBank nodes.
- **Node persistence is `ParameterBank::serialize`/`deserialize`** (#154).
- **Param id = persistence key = append-only.** Derive from name, never from param count.
- **`lfo_dest` indexes a PER-MACHINE table** (#179). Six append-only tables. Bank keeps union width; overlay narrows.
- **ParamLock:** route to `node_locks: Vec<(u32, f64)>`, NOT through `bank.handle_commands()`. Lock lives for the note (#169) — clear at next trigger, not top of `process()`.
- **`serde_yml` not `serde_yaml`.** Removed in P9.
- **`publish_bank_state()` caches in `OnceLock`** — no `format!` on audio thread after first cycle (BUG-007).
- **`SurfaceOutputHandle`:** `take_output_handle()` returning `Some(Box<dyn SurfaceOutputHandle>)`. Ticked on main thread.
- **CLAP host:** `clap-sys` + `libloading`, not `clack`.

## Node IDs (hard-coded)

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
| 60 | `AudioOutput` (also Sampler's default `root_note`) |
| 101–106 | Surface nodes |
| 110–113 | `ScriptingGatewayNode` |
| 200 | `ReverbNode` |

Default `instrument.yaml` wires 4 tracks (seq 10–13, voices 20–22 + 27). `instrument-fx.yaml` adds filter + distortion.

## Event ordering (same `sample_offset`)

1. `ParamLockEvent` (routed by `node_id`, not graph edges)
2. `TransportEvent`
3. `Midi2`
4. `Surface`
5. `Extended`

## StateBus paths

| Path | Writer |
|------|--------|
| `/node/{id}/param/{name}` | `publish_bank_state()` |
| `/node/{id}/state/{key}` | node `published_state()` |
| `/transport/*` | clock |
| `/context/*` | profiles |
| `/surface/{id}/*` | devices |
| `/script/*` | scripts |

## Command-line flags

No arg-parsing crate — unknown flags are silently ignored.

| Flag | Effect |
|---|---|
| `--instrument=<file>` | graph to load (default `instrument.yaml`) |
| `--load=<file>` | apply saved project before executor starts |
| `--save=<file>` | write project immediately at startup |
| `--no-tui` | headless |
| `--emulator` | legacy 8×8 grid |
| `--no-antiphon` | no interface server |
| `--antiphon-port=<n>` | HTTP port (default 7274). **WS = port + 1** |
| `--theoria-dir=<dir>` | web build directory |
| `--token` | require 6-digit session code |

## Keyboard controls (Theotokos)

```
Pads:     qwertyui/asdfghjk = Trig1-16
Rec:      z = REC Off↔Grid   z(hold)+x = Live
          x/c = PLAY/STOP (Space=PLAY)
Screens:  1-6 = param   7 = KIT   8 = Settings   0 = Tempo   o = Chain
          Esc = NO
Enc/Lock: n = ENC mode   m(hold)+trig = arm p-lock target
Other:    Shift+; = :   ? = help   Backspace = clear locks   Ctrl-C = quit
          FUNC+Enter = temp save   FUNC+Esc = temp reload
          FUNC+KIT = perform toggle   TRK+FUNC+trig = pattern mute
```

## Sequencer commands (type_id)

16=TOGGLE_STEP, 17=SET_STEP, 18=CLEAR, 19=TRIGGER, 25=SET_STEP_TIMING, 26=SET_STEP_CONDITION, 27=SET_PATTERN, 28=SET_LENGTH, 29=SET_SPEED, 30=SET_PAGE_LOOP, 31=CHAIN_PUSH, 32=CHAIN_CLEAR, 33=SET_LOCK_TARGET, 34=SET_STEP_LOCK, 35=CLEAR_STEP_LOCK, 36=SET_STEP_VELOCITY, 37=SET_STEP_LENGTH, 38=TRIG_NOW, 39=TEMP_SAVE, 40=TEMP_RELOAD, 41=SET_PATTERN_MUTE, 42=PREPARE_MUTE, 43=PREPARE_PATTERN_MUTE, 44=LIVE_ERASE

## Main loop (each ~1ms)

1. `conf.process_main_thread()` — drain state bus SPSC; tick SurfaceOutputHandles
2. Drain per-device gateway SPSCs
3. `scripting.dispatch_surface_event(ev)`
4. `scripting.process_subscriptions(&bus)`
5. `conf.send_command(cmd)`
6. `conf.deliver_script_output(led_output)`

## Web client (Theoria)

```bash
cd web && npm install && npm run build
cargo run -- --theoria-dir=web/packages/app/dist
```

Verify responsive at 390×844, 768×1024, 1024×768.

## Design documents

- `design/README.md` — what lives where
- `design/roadmap.md` — phase sequence (plan, not status)
- `design/adr/` — ADRs (append-only body)
- `design/phases/` — specs and reports (append-only)
- `design/sessions/` — paired usability notes (append-only)

## Guardrails

1. **Specs win.** If spec and reality conflict, stop and record. New tools outside existing phases: write an ADR first.
2. **Do not revisit named decisions** without the user: no tokio; no Web MIDI; wire names plain; relative-only encoders; surfaces are device nodes; DAG + LoopBreakNode; ADR-019 naming; five-layer boundaries.
3. **Audio-thread rules are hard constraints.**
4. **Naming policy:** third-party marks never in identifiers/features/UI strings.
5. **Defect-filing:** architectural defects filed as GitHub issues, never silently worked around.

## Skills (loaded on-demand)

| Skill | When to load |
|-------|-------------|
| `test-driver` | Testing DSP, running baselines, writing scenarios |
| `mutation-testing` | Writing/verifying tests, trusting coverage |
| `kitty-driving` | Driving the panel from an agent, paired sessions |
| `antiphon-wire` | Inspecting WebSocket protocol, verifying wire fields |
| `commit-workflow` | Pre-commit checklist, code review, session close |
