# AGENTS.md

## Quick-start reading order

`AGENTS.md` → **the open milestone on GitHub** (what to work on) →
the phase spec in `design/phases/` → relevant ADRs in `design/adr/`.

**Implementing? Start here:**

```bash
# Which phase is active? Ask GitHub — never hardcode a milestone name here,
# it goes stale the moment a phase closes (the exact failure "Phase
# transitions" below exists to prevent):
gh api repos/:owner/:repo/milestones --jq '.[] | "\(.number)  \(.title)  open=\(.open_issues)"'
gh issue list --milestone "<title from above>"   # the active phase's remaining work
gh issue list --state open --label bug           # what's actually broken
gh issue list --state open --label open-question
```

**Live state lives in GitHub Issues, not in `design/`** (migrated
2026-07-30 — see `design/README.md`). Open bugs, open questions, spec
gaps, spikes and session carryover are issues. `design/` holds only
append-only authority and record: ADRs, phase specs and reports, session
notes, reviews, and `roadmap.md` (the phase *plan*, not its status).

To resolve a `BUG-###` / `INFRA-###` from a code comment:
`gh issue list --search "BUG-042 in:title" --state all`. The IDs are
preserved verbatim in issue titles. `ADR-###` still resolves in-repo via
`design/adr/INDEX.md`.

`CLAUDE.md` is a symlink to this file, so Claude Code loads it
automatically; keep the content here, not there.

## Workflow

### Model routing

**Session orchestrator is DeepSeek V4 Pro.** The orchestrator holds session
context, makes delegation calls, verifies returned work for correctness, writes
gated commit messages and spec-conflict reconciliations, and runs design review
passes. It is never delegated down — a subagent does not become the orchestrator.

### Implementation delegation

**Pro owns implementation; Flash is a tool, not a subordinate.** Pro may
edit source files directly when it already has the context and the change is
surgical. Pro delegates to Flash when the work is mechanical,
well-specified, or would bloat Pro's context — at Pro's judgment, not by
rule.

**Git safety.** Pro may `git add` and `git commit`. Pro must never run
tree-mutating git: no `stash`, `reset`, `checkout`, `clean`, or `restore`.
These wipe uncommitted work and have caused data loss in this repo
(2026-07-30 review subagent incident).

**When to delegate vs when to edit directly:**

| Delegate to Flash | Edit directly (Pro) |
|---|---|
| Multi-file refactors, renames across crates | Targeted fix with fresh context |
| Feature implementation from a complete spec | Single-line logic fix you've already traced |
| Mechanical: add the same pattern to N files | Config files, docs, AGENTS.md |
| Work that would pollute Pro's context | When cold-start overhead > editing cost |
| Test addition, doc sweeps | Typo, clippy suppression, constant rename |

**The pattern for big features:** design/judgment → Pro. Write a spec.
Well-specified implementation (including cross-crate refactors) → Flash
from the spec. Review and integration → Pro. For everything else, Pro uses
judgment: delegate when it saves work, edit directly when context is hot.

**The delegation path.** Use the `implement` skill (`/implement` or
`run_skill('implement')`) as a convenience — it provides the project-rules
block and build/test commands to paste into the `task()` prompt. Delegate
with `write_paths` so subagents are path-bound:

```text
task(model='deepseek-flash', write_paths=['crates/foo/src/bar.rs', ...], prompt='''
<specific task, files, expected change>
<paste the project-rules + build/test block from /implement>
''')
```

**The structural gate is code review, not delegation.** Every commit
(whether Pro wrote it or Flash did) passes through a Flash subagent review
before landing. That's the quality check — see §Stage gates below. A
delegation audit (`grep '"model"'` on subagent meta files) is available
post-session but is not a gate.

### Stage gates

Every atomic change passes through gates before commit. The full checklist is
the `commit-gate` skill — invoke it before every commit. In summary:

| Gate | Requirement |
|------|------------|
| Tests | `cargo test --workspace` exit 0 |
| Clippy | Zero new warnings on touched crates |
| Live test | A functionality-changing commit carries a debug-harness live test proving the behavior end-to-end (test-driver scenario with assertions, run green before commit). If the harness cannot reach the functionality, that is a defect in the harness — fix the harness in the same commit. Doc/refactor commits with no behavior change are exempt; say so in the message. |
| Code review | Subagent review (Flash) on the diff, with spec/ADR context |
| Issue ref | `Fixes #N` / `Refs #N` in commit message |
| Design review | If ADR/spec/protocol changed: `design-review` pass (Flash) |
| Working tree | `git status` clean or explicitly accounted for |

**The live-test gate is about the running graph, not unit tests.** A unit test
proves a function; a live test proves the function works when the graph is
built, the clock is running, and the state bus is live — the debug harness
(`tools/test-driver`, ADR-033) renders and asserts on the result. Concretely:
the commit adds (or points at) a scenario under `tools/test-driver/tests/`
that drives the new behavior through the harness and asserts on its effect
(state-bus read, audio peak, artifact scan); the scenario must have been
mutation-checked — break the code the scenario covers and confirm the scenario
fails — before it counts as coverage. A live test that has never been seen to
fail is not evidence. "The debug frame doesn't allow testing this" is a
harness defect, filed and fixed, not a waiver.

**Testability is planned, not bolted on.** Every phase spec's commit plan maps
each commit to its live test (the P11 §4 test-coverage model), and the harness
verbs a phase needs land **before** the commits that need them — a phase that
adds app-level operations must first give test-driver the verbs to drive and
observe them. If a commit in the plan has no harness reach, the plan is
missing a harness commit; fix the plan, not the coverage.

**Code review is mandatory, not advisory.** The orchestrator must run a Flash
subagent review after every implementation commit or logical batch. Do not skip
it — it is the quality gate between implementation pushes. The review subagent
must receive: the commit under review, the binding spec/ADR sections, the exact
files changed, what changed and why, and the verification commands. It must be
forbidden from tree-mutating git (see §Review subagents below).

**Design review runs on design changes.** Any change to an ADR body, phase spec,
protocol definition, or architectural decision triggers a `design-review`
subagent pass. The review checks contradictions, unverified claims, missing edge
cases, guardrail violations, spec gaps, implementation order, and breaking
changes. Findings are severity-tagged; blockers must be resolved before the
design is ratified.

### Task routing by judgment density

Route by the **judgment density** of the task, not its size. These are
Pro's *default* delegation targets, not assignments — see §Implementation
delegation above for when Pro edits directly.

- **Flash subagent** (Pro's default target for these) — mechanical, fully
  specified, verifiable by tests: bug fixes, feature implementation from a
  spec, test addition, report drafting, doc sweeps, closing issues from a
  commit diff. Pro may handle these directly when the change is surgical
  and context is hot.
- **Orchestrator (DeepSeek V4 Pro)** — judgment *within* the spec: multi-file
  integration design, data-model restructures, crate introduction,
  code-review delegation & adjudication (delegates to Flash, evaluates
  findings), design-review orchestration, spec-conflict reconciliation,
  commit message authorship, and direct editing of surgical changes.
- **Defer to the user** — protocol freezes; any deviation from a spec contract;
  any new ADR; re-ordering a phase's commits after a paired session; **anything
  that changes what a gesture the performer has already learned does** — a new
  page ahead of the existing ones, a remapped key, a changed default. Headless
  agents cannot judge these; a paired session is cheap next to relearning
  muscle memory.

### A deferred decision blocks its item, not its commit

When one part of a commit needs the user and the rest does not, **land the
rest**. Do everything independent of the answer, then say plainly which item
is outstanding, why, and what the options are — in the commit message *and* in
the phase spec, since the next agent reads the spec and not your session.

## Build/test gotchas

```bash
# WRONG: builds only paraclete-app (sole default-member)
cargo build
# CORRECT:
cargo build --workspace

# WRONG: runs only paraclete-app's 32 tests
cargo test
# CORRECT (991 tests as of 2026-08-01 — bare `cargo test` covers ~3% of them):
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

**Clippy is judged against a baseline, not against zero.** The workspace
carries pre-existing warnings. "Clean on touched crates" means *no new* ones:
capture `cargo clippy --workspace --all-targets` at HEAD, capture it again
with the change, and diff. Warnings only re-emit when a crate recompiles, so
`touch` the files you changed before the second run or the comparison lies.

### Mutation testing — the house standard for "is this test load-bearing?"

Tests here are routinely mutation-checked before being trusted: apply one
deliberate defect, confirm a *named* test fails, revert. Several commits have
found tests that passed on obviously broken code (MM-C4 found three; MM-C6
found a claim that lived only in a comment). Report which specific test killed
each mutant — "the suite is green" is not the claim being made.

Three ways the harness silently lies. All three have happened:

1. **Restoring with `cp`/`mv` gives the file the backup's older mtime.** Cargo
   compares mtimes, decides the crate is unchanged, and reruns the **mutant's**
   binary. A green run afterwards proves nothing. **`touch` after every write
   *and* every restore.**
2. **A mutant can hang instead of failing.** An iterator that stops advancing
   makes a test collect forever, and the run stalls rather than reporting.
   Wrap every `cargo test` / `cargo run` in `timeout`; treat exit 124 as a
   *killed* mutant.
3. **`cargo test` prints `error: test failed, to rerun pass ...` on an ordinary
   assertion failure.** A `grep -E "^error"` compile-check therefore misreads
   killed mutants as "did not compile". Match `^error\[|could not compile`.

A mutant killed by **no unit test but caught by an ADR-035 baseline** is a
finding worth writing down, not a gap to paper over — see the `analog_machines`
case in `design/phases/mm-machine-and-mod.md` (MM-C7).

**The converse is the trap: a baseline proves *stability*, not *behaviour*.**
It fingerprints whatever the scenario does, so a scenario that does nothing
fingerprints cleanly and passes forever. `plock_authoring` claimed in its own
header to "prove that authored p-locks move audio" and authored a lock that
could not fire (#170) — which is why #169 survived the whole MM phase. Before
trusting a baseline as coverage for a behaviour, check the scenario actually
exercises it: perturb the thing under test and confirm the fingerprint moves.
A baseline you have never seen fail is not evidence.

**`--check-baseline` reports drift on stderr.** A loop that runs it with
`2>/dev/null` and greps stdout for `FAIL` reports every scenario clean, no
matter what drifted. This happened twice in one session while checking the
#169 and #175 fixes; both conclusions survived re-checking with `2>&1`, but
they had not actually been verified when reported. Exit code is the reliable
signal (0 pass, 1 drift), and `2>&1` is what makes a grep honest. Same family
as the three mutation-harness lies below: the check appears to run and its
result means nothing.

**Two perturbations, not one** — that is what repairing `plock_authoring`
taught. Removing the p-lock moved the fingerprint, which only proves the
scenario does *something* the baseline notices. Reinstating the **defect**
(#169's per-cycle `node_locks.clear()`) and confirming it also moves is what
proves the baseline covers the bug. The first check passes on a scenario that
merely exercises a code path; the second is the one that would have caught
#169. Do both, and write down that you did — in the scenario header, where the
next agent reads it.

## Logging

`env_logger` is initialized at the top of `main()`.  All terminal output must go
through the `log` crate — no bare `eprintln!` / `println!` in library code
(exception: pre-main error paths, e.g. CLI argument failures before
`env_logger::init()`).

| Macro | When |
|---|---|
| `log::info!()` | Startup milestones, successful operations (audio started, profile loaded, RT scheduling granted) |
| `log::warn!()` | Non-fatal issues (Launchpad not found, RT scheduling denied, rtkit unavailable) |
| `log::error!()` | Fatal conditions that exit the process |

`RUST_LOG` controls verbosity at runtime (default: `info`).  The `[paraclete]`
prefix is **not** used in log messages — the logger adds its own module/target
prefix.

## Headless audio testing (ADR-033)

`tools/test-driver` renders the graph without hardware and asserts on the
result — use it to hear/verify sound changes without the app:

```bash
# Quick mode: trigger a voice, render, auto-play
cargo run -p test-driver -- --trigger analog_engine:kick --at 1.0 -d 3

# Scenario mode: timed commands + assertions (see tools/test-driver/tests/)
cargo run -p test-driver -- tools/test-driver/tests/kick_reverb_clean.yaml

# Regression baselines (ADR-035 Part A): fingerprint a scenario, diff later
cargo run -p test-driver -- <scenario>.yaml --update-baseline   # write <scenario>.baseline.json
cargo run -p test-driver -- <scenario>.yaml --check-baseline    # diff; exit 1 on drift
# Baseline runs use a DETERMINISTIC single-threaded render (not the wall-clock
# threaded path), so the peak/rms/dc + 50ms windowed-RMS envelope fingerprint is
# bit-stable run-to-run. Tolerances live in the .baseline.json (edit to loosen).
# Baseline mode does NOT evaluate a scenario's artifact assertions (main.rs:1280)
# — the fingerprint is the check. Nothing runs these automatically; there is no
# CI. Run all SEVEN before and after any DSP-touching change:
#   kick_reverb_clean   node 20 through mix+reverb          (analog Kick)
#   plock_authoring     node 10 -> 20, authored p-locks     (analog Kick)
#     ^ REAL p-lock coverage since #169/#170 — it was not before. It authored
#       `param_id: 1` (ids are FNV-1a hashes; decay = 3541427549) on step 2,
#       which carries no trig, so the fingerprint recorded a plain
#       4-on-the-floor and passed for a whole phase while #169 sat under it.
#       Now locks `decay` on step 4 by NAME, and was checked in both
#       directions before being trusted: removing the lock moves the
#       fingerprint, and so does reinstating #169's per-cycle clear.
#   analog_machines     nodes 21, 22, and both at once      (analog Snare, HiHat)
#   fm_machines         node 27 driven through all three    (FM Kick, Bell, Bass)
#   fx_chain            engine -> filter -> distortion       (uses instrument-fx.yaml)
#   sampler_chain       node 23 sweeping pitch/start/end/loop (uses instrument-fx.yaml)
#   lfo_sweep           nodes 20 + 27, lfo_dest tune and tone (analog + FM)
#     ^ the LFO's ONLY coverage. Every other baseline runs at lfo_dest 0 (off),
#       so the whole MOD block was unobserved until #175 — that fix changed the
#       pitch path in both engine families and all six others stayed green.
#       Touching the LFO host or either engine's pitch path? This is the one.
# The first four observe all six voice machines. `fx_chain` covers FilterNode
# and DistortionNode, which appear in no other instrument file — but read its
# header before trusting it: at 0.2% tolerance it catches filter-coefficient
# changes and MISSES filter-state re-sequencing, which is the hazard a
# sub-block restructure actually poses. `sampler_chain` is the Sampler's only
# coverage (it became possible at #159, which gave the instrument schema a
# sample path — before that a sampler in a fixture rendered silence and would
# have fingerprinted as zeros). #155 stays open for what is still uncovered.

# P11 LIVE scenarios (the app-level P11 ops run through the same
# PerformState the app main loop drains into — test-driver dispatch_any):
#   p11_kit_capture_apply   C0+C2: capture is opt-in via `in_kit` (a sound
#     param is captured and restored; a structural param is not) — the two
#     assertions prove both directions on one save.
#   p11_temp_save_reload    C3: AppTempSave/AppTempReload — pattern half
#     restored (audible 4-on-the-floor comes back after a clear) AND param
#     half (decay 0.1 exact on the bus).
#   p11_bind_kit_apply      C2: pattern-switch kit-apply fires outside
#     perform mode and is suppressed inside it; pattern lengths are
#     shortened to 2 steps because the default is 64 (~6.9s wraps) and
#     set_length acts on the active pattern only.
# All three are mutation-checked (each fails when the code it covers is
# broken) and are part of the Live-test gate for any P11-touching commit.

# Interactive mode: JSON-lines REPL for live engine interrogation
cargo run -p test-driver -- --interactive --instrument instrument.yaml
# stdin commands, one JSON object per line; responses on stdout:
#   {"cmd":"trigger","target":"analog_engine:kick","velocity":1.0}   engine mutations:
#   {"cmd":"set_param","target":"analog_engine:kick","param":"decay","value":0.3}   set/bump/
#   {"cmd":"read","path":"/node/20/param/decay"}   sequencer/chain, same as batch
#   {"cmd":"peak","window_ms":500}   read/dump/peak/render/quit are REPL-only
#   {"cmd":"dump"}   {"cmd":"render","output":"/tmp/x.wav"}   {"cmd":"quit"}
# Errors are non-fatal JSON ({"error":"..."}); the session continues.
```

**Naming a target.** A scenario's `target:` accepts a node id, a full type tag
(`analog_engine:kick`), a display name (`Kick`), a short type tag (`kick`), or
a qualified `type_tag/display_name` (`sequencer/Kick`); names are
case-insensitive. A name claimed by **more than one node is a hard error**
listing the candidates and a suggested unambiguous handle for each — it is not
resolved to one of them (INFRA-012: it used to be, silently, and a whole
scenario's commands went to a node that ignored them while still passing).
In the default `instrument.yaml` this makes `kick`/`snare`/`hihat`/`bass` and
the bare tag `sequencer` all ambiguous. Numeric ids are **not** validated
against the instrument — a typo'd id resolves and then silently no-ops
(INFRA-014).

**Naming a lock lane.** `set_lock_target` and `clear_step_lock` take either
`param: <name>` (resolved through `ParamDescriptor::id_for_name`, the way
`set_param` always has) or a raw `param_id`, and reject both together.
**Prefer the name.** A raw id is an FNV-1a hash — `decay` is 3541427549 — that
no reader can check by eye, and a wrong one is stored, emitted and silently
never matched; that is how `plock_authoring` authored a lock on a param that
does not exist and still passed for a phase (#170). Omitting both on
`clear_step_lock` still means "every lane on this step".

Assertions: state-bus `eq`/`between`, live `peak_gte`/`peak_lt`, and
post-capture artifact scans `discontinuity_lt`/`dc_offset_lt`/`dropout_lt_ms`
(windowed by `from`/`until` seconds; NaN/Inf fail outright). Exit 0 pass,
1 assertion failure, 2 fatal. Caveat: timeline actions dispatch on wall
clock but capture time runs ~25% slower in debug builds — leave margin in
artifact windows around action times.

## Command-line flags

There is no arg-parsing crate — `main()` scans `std::env::args()` by hand
(`crates/paraclete-app/src/main.rs:44`), so **an unknown flag is silently
ignored** and a typo'd `--no-tuii` just starts the TUI. There is no `--help`.

| Flag | Effect |
|---|---|
| `--instrument=<file>` | graph to load (default `instrument.yaml`) |
| `--load=<file>` | apply a saved project over the built graph, **before** the executor starts |
| `--save=<file>` | write the project **immediately at startup** (step 6 of `main()`, before the executor starts) — it is *not* save-on-quit, so it captures the freshly built (or `--load`ed) graph, never a session's edits |
| `--no-tui` | headless; no Theotokos, no emulator. Use for all automated testing |
| `--emulator` | legacy 8×8 Launchpad-emulator grid instead of Theotokos |
| `--no-emulator` | never fall back to the terminal emulator when no Launchpad is found (implied by Theotokos) |
| `--theotokos` | accepted no-op — Theotokos is the default |
| `--no-antiphon` | do not start the interface server |
| `--antiphon-port=<n>` | HTTP port (default `paraclete_antiphon::DEFAULT_PORT` = 7274). **WebSocket is this + 1** |
| `--theoria-dir=<dir>` | serve this web build instead of the embedded/on-disk default |
| `--token` | require a 6-digit session code. **Access is open on the LAN by default** (2026-07-10 user decision) |
| `--open` | accepted no-op (older notes) |
| `--dev-ui` | every 1000 main-loop ticks, dump each sequencer's `current_step`/`steps` to stderr |

## Keyboard controls (Theotokos is default)

**Theotokos runs by default.** `cargo run` starts the keyboard-first
performance terminal: a fixed seven-region panel (ADR-044 D1) with one
always-visible trig strip for the selected track, TRK/PTN hold-chords for
track/pattern select, an explicit ENC mode for the 8-encoder parameter
bank, and Tempo/Settings/Chain screens (ADR-019 command plane, ADR-038/
ADR-044 panel grammar; there is no dedicated Mute screen — mute state
lives on the track indicator, TRK+FUNC+trig toggles it). The legacy
Launchpad-emulator grid requires `--emulator`. To run without any terminal
UI: `--no-tui` (headless — use for debugging and test-driver).

```
Pads:     qwertyui/asdfghjk = Trig1-16   bare trig = play + select track (REC off/Live)
          Tab(hold)+trig = select track silently   p(hold)+trig = select pattern
Rec:      z = REC toggles Off↔Grid (step-entry)   z(hold)+x = REC+PLAY → Live (record)
          x/c = PLAY/STOP (Space=PLAY)   Shift+z/x/c = copy/clear/paste lane
Screens:  1-6 = param pages   7/9 = KIT/SAMPLING (reserved)   8 = Settings   0 = Tempo
          o = Chain   Esc = NO (also: back to Grid; disarms a held prefix)
          arrows = Tempo ±bpm / Chain cursor (no-op elsewhere)
Enc/Lock: n = toggle ENC mode (bare trig jogs encoder n; Ctrl = fine, Shift(FUNC) = coarse)
          m(hold)+trig = arm p-lock target (latched); hold a trig = momentary target (kitty)
          Outside ENC mode: Shift(FUNC)+trig = encoder n up/down, Ctrl+FUNC = fine
          (numpad slot A/B/C jog remains unwired — formally descoped, BUG-038/OQ-T24)
Other:    Shift+; = : line   ? = help   Backspace = clear locks   Ctrl-C = quit
```

```
**Headless debugging (use for all automated testing):**
```bash
cargo run -- --no-tui --no-emulator --no-antiphon
```

### Driving the panel from an agent (paired usability sessions)

Run the app in **kitty** with remote control so the agent can read the panel
and drive keys without spending user time. **Never through tmux** — its
`extended-keys` is CSI-u/modifyOtherKeys and does not proxy key *releases*,
which silently forces the sticky fallback and invalidates every hold/chord
result without saying so.

```bash
setsid kitty --listen-on unix:@tk4 -o allow_remote_control=yes \
  -e bash -c 'exec ./target/release/paraclete 2>>/tmp/tk4-app.log' &
kitty @ --to unix:@tk4 get-text          # read the panel
kitty @ --to unix:@tk4 get-text --ansi   # read it WITH colours/attributes
```

`get-text --ansi` is how you read state the plain text cannot show: active vs
empty step glyphs share `▓`/`░` but differ by colour, and the playhead and
lock focus are carried by `Modifier::REVERSED` (`[7;33m`) alone.

**Two send verbs, and the difference matters:**

| Verb | Delivers | Use for |
|---|---|---|
| `send-key q` / `send-key shift+z` | press **and** release | anything that should be a **tap**; FUNC (Shift) chords arrive intact |
| `send-text "q"` | press, **no release** | deliberately *latching* a hold prefix |

Composing them synthesizes a hold-chord — session #3's "chords genuinely need
the user's hands" is superseded:

```bash
kitty @ --to unix:@tk4 send-text $'\t'   # Tab press, no release -> TRK armed
kitty @ --to unix:@tk4 send-key q        # trig press+release while armed
kitty @ --to unix:@tk4 send-key tab      # press+release -> clears the arm
```

**Hazard:** using `send-text` for a tap latches the key as a held prefix, and
a latched REC makes every trig `Action::Noop` by design (`input.rs:749`).
That reads exactly like "step entry is broken". Use `send-key` for taps.

**Releasing a latch needs a third verb — `send-key` is a *re-press*.** The
`send-key tab` above only works because re-pressing TRK is harmless. It is
not harmless in general: `m` (Lock) latched with `send-text`, then "released"
with `send-key m`, reads as a deliberate second press, and re-pressing Lock
**clears the target you just set** (intercepted in `lib.rs::handle_keys`
before arming). The gesture then looks broken when it is the driving that is
wrong. Send the kitty keyboard-protocol release instead — `CSI <code>;1:3u`,
event type 3:

```bash
kitty @ --to unix:@tk5 send-text 'm'                 # press, no release -> Lock armed
kitty @ --to unix:@tk5 send-key g                    # trig -> sets the lock target
kitty @ --to unix:@tk5 send-text $'\x1b[109;1:3u'    # 109 = 'm', TRUE release -> target survives
```

Verified in session #5: with `send-key m` the target vanished; with the
release escape `L:s12` stayed latched as designed.

**The user is on the same keyboard.** In a paired session their keypresses and
yours are indistinguishable from `get-text`. Session #5 spent three probes
bisecting a transport stop that was the user pausing playback between rounds.
Unexplained state change during a paired session: **ask, don't bisect** — and
never file it as a defect on the strength of an agent-side observation alone.

**Not agent-testable:** the sticky-fallback path (D11's re-tap disarm and
400 ms guard) never runs in kitty, which delivers releases — judging it needs
a release-less terminal that is not tmux.

**Not a valid audio oracle:** `parecord` off the default sink monitor. It has
read digital silence while the user could plainly hear the pattern
(session #4). Use `test-driver` renders, which assert on a file.

### Legacy Launchpad emulator (`--emulator`)

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

### Inspecting the Antiphon wire from an agent

**The WebSocket port is the HTTP port + 1.** The app prints only the HTTP URL
(`http://host:7274/`); clients derive the WS port themselves
(`web/packages/app/src/app.tsx`, `wsUrl()`). Connecting to 7274 gets an
HTTP 200 and a failed upgrade, which reads like a broken handshake.

There is no Python WebSocket library in this environment (`websockets` and
`websocket-client` are both absent) and adding one is not worth it — a ~60
line raw RFC 6455 client is enough to drive `hello` + `get_view_meta` and dump
what the server actually sends. Client frames must be masked; server frames
are not. Use this to check a protocol change against the **running graph**
rather than only against fixtures: MM-C5 was verified this way, and it is how
"no shipped node declares a TRIG page" got established as fact rather than
assumption.

Prefer it over reading `protocol.rs` alone whenever a change adds or
populates a wire field — the mapper and the assembler can each look right
while disagreeing.

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

Platform crates (outside the five layers — all GPL3; `paraclete-node-api` is
the *only* LGPL3 crate in the workspace):
- `paraclete-antiphon` — interface server (WebSocket + HTTP, no tokio)
- `paraclete-clap` — Paraclete-as-CLAP-plugin (machine bank `.clap` binaries)
- `paraclete-clap-host` — Paraclete-as-CLAP-host (loads third-party `.clap` plugins as nodes)
- `paraclete-tui` — ratatui terminal UI (legacy `--emulator` grid)
- `paraclete-theotokos` — the default keyboard-first performance panel
  (`action.rs` / `input.rs` / `model.rs` / `render.rs`)
- `paraclete-view-assembly` — composite track-rule assembly shared by Antiphon
  and Theotokos (ADR-036). Depends only on L2, so the web and terminal views
  agree by construction; owns `CANONICAL_PAGE_ORDER` and sub-page slot width.
  **Page order lives here — do not re-declare it in a consumer** (the drift
  that `PageNav.tsx` already caused, learning 5 below).
- `paraclete-graph-nodes` — nodes that own an inner `NodeExecutor` (only crate allowed to depend on both `paraclete-nodes` and `paraclete-runtime`)
- `paraclete-machine-{kick,snare,fm-kick,fm-bell,fm-bass}` — one `.clap` binary
  each, wrapping an engine + `Sequencer` via `paraclete-clap::SubgraphPlugin`.
  Thin shims: adding a machine means a new crate here, not an edit to an
  existing one.

Tools (`tools/`, not shipped): `gen-samples` (pre-flight sample generation),
`test-driver` (ADR-033 headless render/assert harness), `lpx-debug`.

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
  `"attack"`, `"release"`, `"tune"`, `"machine"`.
- **`"machine"` is an *identity* param, not a setting** (ADR-041). It is a
  stepped selector over a machine-host engine's variants, its overlay carries
  `identity: true`, and it must be rejected as a p-lock target and as a
  scene-morph destination. The engines also refuse to switch on a `ParamLock`
  — they read the bank, not `get_param` — because the sequencer holds opaque
  `(node_id, param_id)` locks and cannot know it has an identity param.
- **`published_state()` push-down:** accepts `&mut Vec<(String, StateBusValue)>`,
  pushes into it. The old returning signature is forbidden (allocates per cycle).
- **`deserialize()` AFTER `activate()`** for ParameterBank nodes. `activate()`
  resets the bank to defaults; `deserialize()` re-applies saved values on top.
- **Node persistence is `ParameterBank::serialize`/`deserialize`** (#154), not
  a hand-written param list per node. If a node's whole persistable state is
  its bank, `Node::serialize` is a two-line delegate; if it has one extra
  field, append your own section after `bank.serialize()` — `deserialize`
  reads `count` pairs and ignores trailing bytes. `Sampler` predates this and
  keeps its own v3 format.
  **A param id is a persistence key, so it is append-only.** Both engines
  derive ids from `ParamDescriptor::id_for_name` (stable as long as the *name*
  is — renaming a param orphans every saved value for it, silently, because
  `set` no-ops on an unknown id). `FilterNode`/`DistortionNode`/`ReverbNode`
  use hand-assigned constants (`const PARAM_CUTOFF: u32 = 0`); each has a
  guard test pinning them. **Never derive an id from how many params a node
  declares** — `MixNode` used to (`id: i as u32` over a configurable count,
  with `master_gain` at `num_inputs`), which is why it was not wired to the
  helper until BUG-060; it now derives ids from per-input names
  (`input_gain_{i}`, `master_gain`), stable across a count change — keep them
  name-derived. Stepped params that index
  a table (`machine` → `AnalogMachine::ALL`, `lfo_dest` → the engine's dest
  table) persist the index, so those tables are append-only too, each with a
  guard test.
  **`lfo_dest` indexes a PER-MACHINE table** (#179): each machine offers
  exactly the params it reads, so there are six append-only tables across the
  two engine families, not two. A machine-invariant list left three to five of
  eight destinations inert on any given machine — five of eight on a HiHat —
  and selecting one was indistinguishable from a broken LFO.
  The split that makes a per-machine index safe to persist, and the thing to
  preserve if you touch this: the **bank** keeps the *union* width so a value
  belonging to a machine with a longer list is never truncated on load, while
  the **overlay** narrows what the encoder can reach to the active machine.
  Labels are the active machine's, and an index between the two is a gap —
  `LfoDestLabels` returns `""` there and `ParamDescriptor::value_labels` turns
  that into `None` so a client skips it rather than drawing a decoy.
  `lfo_dest` is **not** translated across a machine switch. Remapping by param
  name looks right and is not: a destination the other machine lacks collapses
  to off, so a Kick → HiHat → Kick round trip loses it, breaking the
  losslessness MM §6.2 guarantees. Leaving the number alone means the LFO goes
  quiet while the other machine is selected and returns exactly as it was.
- **ParamLock must NOT go through `bank.handle_commands()`.** Route to a
  `node_locks: Vec<(u32, f64)>` and check your param getter against it before
  falling back to the bank. Otherwise the locked value bleeds into subsequent
  steps.
  **A lock lives for the note, not the audio cycle** (#169). Clearing
  `node_locks` at the top of `process()` — which this entry used to prescribe —
  is wrong: a cycle is ~512 samples (~11 ms), a note is not. Params latched at
  trigger time (`tune`, velocity) kept the lock while params re-read per render
  span (`decay`, `open`, `tone`, the whole `lfo_*` block) reverted one cycle in,
  which is inaudible and looked like p-locks not working at all. Retire the set
  at the **next trigger** instead: set a `locks_pending` flag when a lock
  arrives, consume it in `retrigger()`, and clear when a trigger arrives with
  nothing pending. `activate()` must also clear, since a rebuild kills the voice
  that owned the lock. See `AnalogEngine::consume_pending_locks`.
  That divide — trigger-latched vs. re-read-per-span — is a standing hazard,
  not a one-off: it is also why an LFO on `tune` is a sample-and-hold rather
  than a sweep (#175). When adding a param, know which side it is on.
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
| 60 | `AudioOutput` (graph sink; every instrument file ends here) |
| 101–106 | Surface nodes (LaunchpadEmulator, Launchpad, DigitaktMidi, Keystep, SurfaceMapping, TheoriaSurface) |
| 110–113 | `ScriptingGatewayNode` (LP, DT, KS, Theoria) |
| 200 | `ReverbNode` |

> **Note (2026-07-21):** this table is the ID *convention* for the full
> 8-track graph. The default `instrument.yaml` wires **4 tracks** today —
> sequencers 10–13, voices 20–22 + 27 (no Samplers 23–26, no Distortion/
> Filter nodes). Clients must bind by discovery (`hello`/cap-docs), never
> by this table.
>
> `instrument-fx.yaml` is the second shipped graph: one sequencer (10) →
> `analog_engine:kick` (20) → filter (40) → distortion (30), plus a sampler
> track (11 → 23). It is the only fixture exercising `FilterNode`/
> `DistortionNode` — the `fx_chain` baseline renders it.
>
> **60 is also `Sampler`'s default `root_note`.** The collision is noted in
> `instrument.yaml:21`; when reading a bare `60` in a diff, check whether it
> is a node id or a MIDI note.

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
| 33 | `CMD_SET_LOCK_TARGET` | arg0: node_id (i64), arg1: param_id (f64, u32 exact) |
| 34 | `CMD_SET_STEP_LOCK` | arg0: step, arg1: value (f64) |
| 35 | `CMD_CLEAR_STEP_LOCK` | arg0: step, arg1: param_id (f64; −1 = all lanes) |
| 36 | `CMD_SET_STEP_VELOCITY` | arg0: step, arg1: velocity (0.0–1.0) |
| 37 | `CMD_SET_STEP_LENGTH` | arg0: step, arg1: length (f32 unit) |

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

**Before every commit, run the `commit-gate` skill.** It enforces:

1. `cargo test --workspace` green
2. `cargo clippy --workspace` clean on touched crates (diff against baseline)
3. **Live test: a functionality-changing commit's debug-harness scenario runs
   green** (see the Stage-gates Live-test gate; a harness gap is a harness
   defect, fixed in the same commit — never waived silently)
4. Subagent code review (Flash) on the diff — **mandatory, not advisory**
5. Issue referenced in commit message (`Fixes #N`)
6. Design review (via `design-review` skill) if ADR/spec/protocol changed
7. Working tree clean or explicitly accounted for

Design/doc changes in separate commits from code. Phase reports and ADR bodies
are append-only. Close the issue a commit resolves in that commit's message.

#### Review subagents: two rules learned the hard way

**1. Forbid tree-mutating git explicitly. "Do not make any edits" is not
enough.** On 2026-07-30 a review agent given exactly that ran `git stash`
(plus two `git reset`) to diff against HEAD and **wiped 11 files** of
uncommitted work while the parent was still editing. "Don't edit" reads as
"don't use Edit/Write"; git plumbing does not register as editing, and
stashing is the obvious way to compare against HEAD — so the failure is
likely, not exotic. Put this in the prompt verbatim:

> Do not run any git command that mutates the working tree or index — no
> stash, reset, checkout, clean, restore, or add. Read with `git diff` /
> `git show` only. To compare against HEAD use `git show HEAD:<path>`.

Prefer a worktree-isolated agent when the reviewer genuinely needs to build
both sides; that makes the hazard structurally impossible.

*Recovery, if it happens:* the work is not lost. `git stash list` shows it,
`git stash show --stat stash@{0}` confirms the file set. Reset any file you
edited *after* the stash, `git stash pop`, then re-apply that edit. The tell
is a "file was modified, either by the user or by a linter" notice for a file
you did not touch, plus a `git status` suddenly much shorter than it should
be — investigate git state, do not assume the user changed something.

**2. Give the fresh agent real context.** It has zero conversation history.
Name the commit under review, the spec/ADR sections that bind it (including
post-ratification amendments), the exact files, what changed and why, what to
check specifically, and how to run the tests and clippy itself. Terse "review
this diff" prompts to fresh-context agents produce shallow reviews; detailed
ones have caught a real defect in nearly every commit they have gated.

## Incidental findings — the scratch file

`.scratch/SESSION_NOTES.md` is a git-ignored scratchpad. When you discover a
defect, design issue, or filing-worthy observation that is **not** the current
task, append one line there before moving on — do not trust yourself to
remember it. At session start, read it; at session end, file any stranded
notes as GitHub issues. This is how "[while working on X I noticed Y]"
survives a session boundary.

**Before closing a session**, the agent must report any untracked files,
uncommitted changes, or stale trackers. The working tree must be either clean
or explicitly accounted for — never silent about dirt.

After every implementation session, the agent must explicitly propose which
trackers and documents need updating, then update **all** that apply before
the session is done — not just the obvious one. This is a mandatory check, not
a suggestion. Keep-current set:

| Where | Update when… |
|-----|--------------|
| **GitHub Issues** | a bug is found or fixed; an open question is answered; a spike concludes; a provisional implementation is replaced. Close the issue **in the commit that resolves it** (`Fixes #N` in the message), never in a later sweep |
| **GitHub Milestones** | a phase completes — close its milestone and open the next one. The open milestone *is* the "what to work on next" pointer |
| `design/adr/*` | a decision is **implemented** — update its `Status:` line and add an implementation note. The decision/context/alternatives body stays append-only (see below) |
| phase reports (`design/phases/*`) | a phase commit lands (append-only) |
| `design/roadmap.md` | the phase *sequence* or a design *gate* changes. **Not** for status — status is the milestone |
| `AGENTS.md` | a workflow, command, tool mode, node ID, or convention changes (e.g. a new test-driver mode) |

If a change touches code *and* an issue describing it, both are in scope in the
same session — a code commit that leaves its issue open is an incomplete
session.

### Phase transitions

Everything above fires when *work* changes. This fires when **which work is
active** changes — a different event, and historically the one that got missed.

**Whenever a phase becomes code-complete, is closed, or is superseded: close
its milestone and open the next one, in the same session.** The next agent
reads the open milestone, so an unclosed milestone points them at finished
work.

State that belongs on the milestone description, not in a doc:

1. where to start in the phase spec (which commit),
2. its design authority (ADRs, and any amended by a session),
3. anything deliberately parked, so it does not read as an oversight,
4. the places in *this* phase where making the tests pass is the wrong
   instinct — tests that encode the bug being fixed, invariants that must
   survive.

**Why this is structural now.** Until 2026-07-30 the pointer was a prose block
in `design/handoff.md`, and nothing about finishing a phase forced it to move.
On 2026-07-29 TK2.1 went code-complete, its report closed, TK2.2 was specced —
and `handoff.md` still read "TK2.1 is the active implementation phase. Start at
`design/phases/tk2.1-theotokos.md` C0". A cold-start agent following the
documented reading order would have re-implemented a finished phase. A
milestone cannot drift the same way: it is either open or closed, and its
issues are either done or not.

The same audit found project instructions were not reaching Claude Code at all,
because the repo had only `AGENTS.md` and Claude Code loads `CLAUDE.md` (now a
symlink to this file).

## Task routing

> **See §Workflow above.** The authoritative routing rules are in the Workflow
> section at the top of this file. This section is retained for the deferred-decision
> worked example below; the model names here (Opus/Sonnet) are historical.
> Current routing: DeepSeek V4 Pro = orchestrator + direct implementation;
> Flash = delegated implementation + mandatory code review.

### A deferred decision blocks its item, not its commit

When one part of a commit needs the user and the rest does not, **land the
rest**. Do everything independent of the answer, then say plainly which item
is outstanding, why, and what the options are — in the commit message *and* in
the phase spec, since the next agent reads the spec and not your session.

MM-C6 is the worked example: 3 of its 4 items shipped (`a9996c1`) while item 2
— where machine-select is declared, which shifts every page index — waits for
a session. MM-C7 then landed *ahead* of it, out of spec order, because it had
no dependency on the answer. Commit order in a phase spec is a plan, not a
constraint; when you depart from it, say so at the section heading so the tick
does not read as "skipped".

## Guardrails (all tiers)

1. **Specs win.** If the spec and reality conflict, stop, record the conflict
   in the phase report, and ask the user. Do not redesign inline. If a spec is
   silent on a detail, choose the boring option and note it in the report.
   **New tools/components outside existing phases: write an ADR first, get
   approval, then implement. Never jump to code.**

   **But a ratified spec is a frontloaded hypothesis, not a contract, and the
   difference changes how you *report*.** The user, during Theotokos session
   #3: *"I don't care about spec as canon. I purposefully ratified, knowing
   everything was up for revision once we built... I'm not holding to account
   imperfect impl."* The heavy up-front ADR + hostile-review process exists
   because agent build cycles are expensive to redo, not because the design is
   meant to be final. So:
   - Record findings as **current behaviour + `file:line` + what to build
     next.** Citations exist to make the next change cheap, never to assign
     fault.
   - Do **not** sort findings into "spec's fault" vs "implementation slip" —
     that distinction is noise here, and framing one that way drew an explicit
     correction. Never defend an implementation by citing the spec it
     faithfully followed.
   - A session finding that contradicts a ratified decision is the *expected
     output* of a usability session, not an escalation.
   - Distinguish "the design converged" from "the phase is done". Session #3
     signed off a redesign while leaving the phase open on 4 bugs.

   "Stop and ask" above still applies to a genuine fork in the work — a
   deviation from a spec *contract*, a protocol freeze, a new ADR. It is not a
   licence to stall on a detail where the boring option is obvious.
2. **Do not revisit named decisions** without the user: no tokio; no Web MIDI
   as primary transport; wire names stay plain; relative-only encoders;
   surfaces are device nodes; DAG + LoopBreakNode; ADR-019 naming contracts;
   the five-layer/license boundaries.
3. **Workflow discipline:** code review before every commit (subagent);
   design/doc changes in separate commits from code; stop at integration-test
   milestones to test with the user; append-only rules for ADRs and phase
   reports.
4. **Naming policy** (`design/interface-plan.md`): third-party marks never in
   identifiers/features/UI strings; house names (Antiphon, Theoria, kerygma,
   epiclesis, Ordo, Triptych) only in their assigned slots; wire/protocol and
   standard concepts keep plain names.
5. **Audio-thread rules are hard constraints:** no alloc/lock/block in
   `process()`; JSON never touches the audio thread.
6. **Universality check** (standing user directive): at every spec or
   implementation pass, ask what is being hard-coded that the vision does not
   require — fixed counts, surface-shaped engine caps, `&'static str` in
   published APIs, single-purpose fields. Flag findings even unprompted; file
   them before format freezes (serializers, wire protocol, crates.io APIs),
   where limitations become permanent.
7. **Defect-filing** (standing user directive): architectural defects found
   during design or review work are **filed as GitHub issues against the
   code** and the design is adjusted — never silently worked around in prose.
8. **Every commit:** `cargo test --workspace` green (bare `cargo test` only
   runs the app crate), clippy clean on touched crates, code review complete,
   issue referenced, design review if applicable. See `commit-gate` skill and
   §Commit workflow. Update the phase report as you go — not at the end.

## Design documents

- `design/README.md` — what lives in `design/` vs GitHub Issues, and how to
  resolve a historical reference. **Read this first if a doc cites a file
  that no longer exists.**
- `design/roadmap.md` — phase sequence and design gates (the *plan*; status
  lives on milestones)
- `design/adr/` — Architecture Decision Records. The **decision/context/
  alternatives body is append-only** — never rewrite a past decision. The
  `Status:` line and an appended implementation note *are* updated when the ADR
  is implemented (e.g. ADR-033 `proposed → accepted`).
- `design/review/` — post-phase code reviews and latent-issue audits
- `design/phases/` — per-phase specs and implementation reports (append-only).
  Naming: `<phase>-<topic>.md` is the spec, `<phase>-report.md` the report
- `design/sessions/` — paired usability session notes (append-only)
- `design/<feature>/` — pre-ADR problem/design pairs for a feature area
  (`theotokos/`, `sampling/`: `problem.md` states the problem, `design.md` the
  proposal). This is where a feature starts before it earns ADRs and a phase
- `design/specs/` — external-reference analyses (prior-art teardowns), not
  Paraclete contracts
- `design/architecture-core.md`, `architecture-evolving{,-append}.md`,
  `instrument-vision.md`, `interface-plan.md`, `prior-art-analysis.md` —
  standing architecture and vision documents; `interface-plan.md` is the
  authority for the naming policy in Guardrail 4

Open bugs, open questions, spikes, spec gaps and carryover are **GitHub
Issues** — see the reading order at the top of this file.

`Hardware*` was renamed to `Surface*` in July 2026. Historical docs use old
names — map accordingly; do not edit those documents.

## Design-process learnings (2026-07-23 hostile-review cycle)

Extracted from the ADR-038…043 review cycle (9 blockers found *after*
ratification; full reports summarized in `roadmap.md`). These are standing
process rules for design work in this repo:

1. **Hostile review comes BEFORE ratification, not after.** ADR-036 did it
   right (pre-ratification subagent review, findings folded, then ratified);
   the 038–043 batch ratified first and needed a normative amendments layer
   to repair. An ADR is not ratification-ready until an adversarial pass has
   line-verified its code claims.
2. **Verify dependency *behavior*, not existence.** The highest-value finding
   class was "the doc cites a real mechanism that doesn't behave as cited":
   crossterm never delivers lowercase+SHIFT; rtrb has no overwrite-oldest or
   non-consuming tail read; derived `Clone` on nested Vecs allocates;
   `publish_bank_state` mirrors only banks that publish. If a design leans on
   a library or idiom, read its source for the exact behavior claimed.
3. **"Zero engine changes" / "free by construction" / "already enabled" are
   red-flag phrases.** Every such claim in this cycle was refuted under
   review. Cost-free claims require file:line proof or must be softened.
4. **Input tests must feed events in the shape the real source emits.**
   BUG-035/036 were guarded by tests injecting synthetic events no terminal
   produces (lowercase char + SHIFT). For any event-driven boundary (keys,
   MIDI, protocol), derive test fixtures from the transport's documented
   encoding, not from what the match arm happens to accept.
5. **Before asserting "X needs no change", grep for duplicated constants.**
   TK2 C0's "web needs no change" missed `PageNav.tsx`'s private copy of the
   page order. Shared-crate constants do not prevent drift in consumers that
   hardcode their own.
6. **Per-commit compilability is part of spec review.** TK2's C2 as first
   written deleted types that later commits still referenced — a spec whose
   commit sequence cannot build green at every step fails the "less capable
   model implements without ambiguity" bar.
7. **Amending an execution-ready spec: normative §0, not a rewrite.** Fold
   post-ratification findings as a dated amendments section that explicitly
   wins over conflicting body text, and tag each amended decision in place
   (`*(amended — §0 An)*`). Keeps the append-only spirit while leaving one
   authoritative reading order.
8. **Split hostile review by domain across parallel fresh-context subagents.**
   Independent reviewers with no authorship attachment out-perform a fork of
   the authoring session; grade findings B/M/m with file:line evidence for
   both the doc claim and the code reality, and report verified-clean counts
   so coverage is visible.
9. **Declared-but-unenforced contracts rot silently.** `Rule` slot numbers
   were fiction (the merge ignores them) and `param_pages` refs are never
   validated against cap-docs (BUG-037). When a data contract has a consumer
   that ignores half of it, either enforce it (assertion) or mark the field
   as advisory in the contract's doc — never design new features against the
   unenforced half without checking.

## MCP tool selection

**Two config files, one server set.** Claude Code reads `.mcp.json`; opencode
reads the `mcp` block in `opencode.json`. They are maintained separately and
have already diverged (serena is `"enabled": false` in `opencode.json` while
`.mcp.json` starts it). **When you add or remove a server, edit both** — a
server that works in one harness and not the other reads as a broken install.

**Serena** (symbol-level code navigation/refactoring — LSP-backed):
- Requires `rust-analyzer` on `PATH` for Rust symbol resolution — install
  with `rustup component add rust-analyzer` if Serena's Rust tools return
  empty/degraded results.
- Use `find_symbol`, `find_referencing_symbols`, `find_declaration` for
  navigating Rust code at the symbol level.
- Use `rename_symbol` for cross-file renames (single atomic call).
- Use `replace_symbol_body`, `insert_before_symbol`, `insert_after_symbol`
  for surgical edits.
- Use `get_symbols_overview` for a file's top-level symbol outline.
- **Never** use Serena's `read_file`, `search_for_pattern`, `list_dir`,
  `find_file`, `replace_content` — opencode's built-in Read/Grep/Glob/
  Edit tools are superior and the Serena variants confuse tool selection.

**narsil** (structural analysis — call graph, control flow, data flow):
- Use `find_references`, `get_callers`/`get_callees`, `find_call_path`
  for understanding how code is wired together.
- Use `get_control_flow`, `get_data_flow`, `get_complexity` for
  understanding individual function internals.
- Use `find_dead_code`, `find_circular_imports` for code health.
- Serena and narsil complement each other: Serena = editing, narsil = analysis.

**context7** (`resolve-library-id`, `query-docs`):
- **Always** check context7 before writing code that depends on an external
  Rust crate (cpal, petgraph, rtrb, ratatui, clap, tree-sitter, tungstenite,
  etc.) to verify the current API surface. Training data is stale.

**Chrome DevTools**: The primary method for testing and debugging the Theoria
web UI (`web/` directory). Navigate to `http://localhost:7274` (antiphon),
inspect DOM/console/network. Not relevant for Rust-only changes.
Enabled in `.mcp.json` (Claude Code) already; under opencode it needs
`"enabled": true` in the `opencode.json` `mcp` block.
