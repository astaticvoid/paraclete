# Paraclete — W1 Interface Specification (Theoria MVP: a playable surface)

> **Implementation blueprint.** Contracts only — deviations stop, get recorded
> in the phase report, and go to the user. Protocol messages were already
> shape-frozen in `w0-interfaces.md` ([W1] variants); this spec activates them.
>
> **W1 deliverable:** the tablet is a playable instrument surface — grid +
> touch encoder row with live names/values + transport — usable tablet-only
> and beside the Launchpad. Ends in **paired session #1**.
> **Last updated:** July 2026
> **Baseline:** P10 C0 shipped (407 tests); W0 exit criteria met.
> **Model routing:** C0/C1 Sonnet-capable (mechanical, spec'd); C2/C3 Opus;
> C4 Opus (client architecture) with Sonnet-fine view code; C5 is a user
> session. See `design/handoff.md`.

---

## Commit sequence

| Commit | Crate(s) | Deliverable |
|--------|----------|-------------|
| **0** | `paraclete-nodes` | `CMD_TRIGGER` (type 19) implemented — pads can live-trigger voices (BUG-011 leftover) |
| **1** | `paraclete-node-api`, `paraclete-nodes`, `paraclete-scripting`, `paraclete-tui`, `profiles/` | State-path unification + BUG-007 path-string caching |
| **2** | `paraclete-antiphon`, `paraclete-app` | kerygma state mirror v1: `state` / `context` / `topology` downstream |
| **3** | `paraclete-antiphon` | Semantic plane live: `set_param` / `bump_param` / `node_cmd` with cap-doc validation |
| **4** | `web/` | `theoria-web` proper: TS workspace, encoder row, transport, reconnect/PWA, embedded bundle |
| **5** | — | Exit criteria + **paired session #1** (`design/sessions/s1.md`) |

---

## Commit 0: `CMD_TRIGGER` (universal command type 19)

Session 0 found trigger mode has never produced sound: profiles send command
19 and no node implements it. Make it a **universal instrument command**
(document in CLAUDE.md command table):

- `CMD_TRIGGER = 19`: `arg0` = note number (`< 0` → node's default/last note,
  pass -1 from profiles), `arg1` = velocity 0.0–1.0 (0.0 → default 0.79).
- Implement in `AnalogEngine`, `FmEngine`, `Sampler` (`handle_commands` /
  command loop — same retrigger path as a `NoteOn`). Effects/others ignore it
  (unknown-command silence is the existing contract).
- **Tests:** per engine: `cmd_trigger_produces_audio` (send 19, process one
  block, assert non-silent output), `cmd_trigger_negative_note_uses_default`.

## Commit 1: state-path unification (+ BUG-007)

The mirror is about to freeze paths into a protocol; fix the schema first.

**New canonical scheme (single source of truth — document in CLAUDE.md):**

| Path | Meaning | Writer |
|---|---|---|
| `/node/{id}/param/{name}` | live parameter value (was `/node/{id}/{name}`) | `publish_bank_state()` |
| `/node/{id}/state/{key}` | node-internal state (steps, current_step, …) | unchanged |
| `/transport/*`, `/context/*` | unchanged | clock / profiles |
| `/surface/{id}/*` | per-surface state (was `/hw/*`) | devices |
| `/script/*` | profile-private scratch | scripts (unchanged) |

- `publish_bank_state()`: emit `/node/{id}/param/{name}`; **BUG-007 fix folded
  in** — cache the formatted `Vec<String>` of paths, built lazily on first
  `published_state()` call after `set_node_id()` (or at `activate()`), zipped
  with `iter_values()`; zero allocation on the audio thread thereafter.
  Sequencer's own `format!` paths in `published_state()` get the same
  treatment (cache at activate).
- Script sandbox: `write_sandboxed()` allows `/node/{id}/param/*` (unchanged —
  the rule was already right; the publisher was wrong). Reject `/surface/*`
  (replacing `/hw/*` in the reject list).
- Update readers: `paraclete-tui` encoder row, all `profiles/*.rhai`
  subscriptions/reads, any tests asserting old paths.
- **Tests:** `publish_bank_state_uses_param_prefix`,
  `publish_bank_state_allocates_no_paths_after_first_call` (structural: same
  `&str` pointers across two calls), sandbox accept/reject table updated.

## Commit 2: kerygma state mirror v1

Main-thread only; no new threads.

- `AntiphonHandle::pump(bus: &StateBusHandle, now_ms: u64)` called every main
  loop iteration (step 1.5, after `process_main_thread()`): reads the
  **subscription set** = union of `/node/*/param/*`, `/node/*/state/*`,
  `/transport/*`, `/context/*` for nodes present in the `hello` snapshot.
  Implementation: kerygma keeps a `HashMap<String, StateBusValue>` shadow;
  each pump diffs changed paths (use `StateBusHandle::subscribe` if it scales,
  else full-scan diff — decide by profiling, record choice in report).
- Coalescing: accumulate diffs; flush at ≥ 33 ms since last flush (caller
  passes a monotonic `now_ms`; antiphon does no clock reads of its own) as one
  `{"t":"state","updates":[…]}` batch to all clients. Cap 256 updates/batch;
  overflow → flush immediately and continue (no drops).
- `context` message: emitted when any `/context/*` path changes (profile
  called `publish_context()`), full 8-slot snapshot each time.
- `topology`: `apply_patch()` call sites in the app send the refreshed node
  summary through `AntiphonHandle::publish_topology(nodes)`.
- **Tests:** diff-shadow correctness (change 2 of 5 paths → batch has exactly
  2), coalescing window (two pumps 10 ms apart → one flush), overflow flush,
  context snapshot shape.

## Commit 3: semantic plane

- `set_param {node, param, v}` → resolve `param` against the cap-doc snapshot
  for `node` (name → id via the same `id_for_name` values already snapshotted
  in `hello`); clamp `v` to [min,max]; emit `NodeCommand{CMD_SET_PARAM}` via a
  new SPSC from antiphon to the main loop, flushed with script commands in
  main-loop step 5. Unknown node/param → log + drop (never disconnect).
- `bump_param {node, param, delta}` → `CMD_BUMP_PARAM`, delta clamped to
  ±0.5 per message.
- `node_cmd {node, cmd, a0, a1}` → pass-through `NodeCommand` after node-id
  existence check. `cmd < 16` (universal range) rejected here — semantic
  plane must use the typed messages for those.
- **Tests:** name resolution, clamping, unknown-target drop, cmd<16 rejection,
  command SPSC drains into main-loop flush order.

## Commit 4: `theoria-web` proper

- **Workspace:** `web/package.json` (npm workspaces) → `web/packages/core`
  (protocol types, connection, state store, gesture lib — transport-agnostic,
  no DOM) and `web/packages/app` (Preact + vite + canvas views). Port the W0
  vanilla client; W0's `web/theoria/` is deleted in the same commit.
- **Encoder row** (canvas, ids 90–97): vertical drag = coarse (1 detent per
  8 px), two-finger or edge-zone drag = fine (1 detent per 24 px, sent with
  `fine:true`? No — **client-side scaling only**: fine mode just sends fewer
  detents; the wire stays `{"t":"enc","id":90,"delta":n}`). Deltas coalesced
  per animation frame. Encoder cell renders: param name + live value + unit
  from the state mirror + `context` (same data the TUI encoder row uses).
  Flash treatment on external change (sequencer p-lock) — 300 ms highlight.
- **Profile side:** `launchpad.rhai` (or a shared include) maps
  `EncoderChanged` ids 90–97 → `CMD_BUMP_PARAM` on the context-mapped
  param, delta × per-param step = (max−min)/256 per detent. This is the same
  handler the P9.5 virtual encoders and a future hardware box use.
- **Transport bar + track header:** play state, BPM (read-only at W1), track
  select buttons (mirror `/script/lp/selected` both ways — selecting on glass
  drives the same state the grid profile uses).
- **Velocity setting** (WQ-2): per-view slider (default 100%) scaling
  `pad_down` velocity.
- Reconnect/staleness/token/PWA carried from W0; **embedded bundle**:
  `include_dir!` of `web/packages/app/dist` behind a `embed-ui` cargo feature
  (default on for release; `--theoria-dir` still overrides for dev).
- No view plugins yet (that's W2/ADR-032). The encoder row and grid are
  core-stratum views.

## Commit 5: exit criteria (user session)

- [ ] **Tablet-only parity:** program, tweak (encoders), save, reload a beat
      with no physical controller attached
- [ ] Contextual flow: select kick → encoders show kick params → drag changes
      the sound; values live-update under sequencer p-locks
- [ ] Grid + encoders work beside the physical Launchpad, one profile
- [ ] Pads live-trigger voices (CMD_TRIGGER) in trigger mode, glass and plastic
- [ ] Staleness/reconnect verified by pulling Wi-Fi mid-jam
- [ ] RTT + state-mirror latency figures in `w1-report.md`; BUG-006 trigger
      evaluated (state-bus churn profile)
- [ ] **Paired session #1** runs the vision's "A Session" as far as the
      current engine allows; findings → `design/sessions/s1.md` + explicit
      roadmap delta (P10 C2–C5 ordering re-validated)

---

## Deferred (do not build in W1)

Param pages / editor views (W2), view-plugin registry (ADR-032), pattern
views (W3), `theoria-term` (WT), Web MIDI adapter (W4), per-client device
nodes (W4), binary protocol, TLS.
