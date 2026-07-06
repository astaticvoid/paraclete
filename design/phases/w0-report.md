# W0 Implementation Report — Antiphon + Theoria grid POC

> Phase report for `design/phases/w0-interfaces.md`. Append-only once W0 ships.
> Started July 2026 (Commit 1).

## Commit 1 — `paraclete-antiphon` crate

**Shipped:** protocol v0 types (`protocol.rs`), WS server with token handshake
and 4-slot client table (`server.rs`), `TheoriaSurfaceNode` +
`TheoriaOutputHandle` (`surface.rs`), kerygma LED broadcast with full-surface
shadow replay (`kerygma.rs`), `tiny_http` static serving (`http.rs`),
`AntiphonServer::spawn` entry point (`lib.rs`).

**Tests:** all 14 spec-named tests present and green, plus 4 unspec'd
(`route_frame_emits_pad_events_and_pongs`, `take_output_handle_is_single_shot`,
`broadcast_reaches_all_connected_clients_and_updates_shadow`,
`resolve_rejects_traversal_and_serves_index`) and one `#[ignore]`-tagged
localhost socket smoke test (`smoke_ws_session_over_localhost` — hello→welcome,
shadow replay on tick, ping→pong; passes). Workspace: 425 passed, 0 failed
(was 407). Clippy clean on the new crate.

**Deviations / implementation choices (recorded per spec preamble):**

1. **One combined I/O thread per client** instead of the spec sketch's split
   read/write threads. tungstenite's `WebSocket` cannot be safely split across
   threads: `read()` auto-queues pong replies to WS-level pings and flushes
   them on the read path, so a separate writer thread on a cloned socket could
   interleave bytes mid-frame. The combined loop (15 ms read timeout, outbound
   `mpsc` drained each pass) keeps one exclusive socket owner. Data flow is
   unchanged from the spec diagram: read side → rtrb ring → node;
   kerygma → mpsc → same thread's write half. Cost: ≤ ~15 ms added outbound
   latency, inside the W1 33 ms coalescing window; measured RTT to be logged
   at Commit 2.
2. **WS port = HTTP port + 1.** The spec has a single `--antiphon-port` and a
   printed `http://…` URL; tiny_http and tungstenite cannot share one port
   without hand-rolling the upgrade path over tiny_http's parsed request.
   HTTP serves on `port` (default 7274), WS binds `port + 1`; the client
   derives the WS port from `location.port`. Single-port upgrade is a W1+
   consolidation option if it ever matters (recorded here, not scheduled).
3. **Scene/control ids (64–79) map to `ButtonPressed`/`ButtonReleased`**,
   matching the physical `LaunchpadNode` and the profile's handlers. Filed
   **BUG-014**: the P9.5 emulator emits `PadPressed` for those ids and is the
   odd one out.
4. `enc`/`enc_push` are parsed and mapped by the pure translation function
   (spec test 4 requires it) but gated to `Drop` in `route_frame` at W0 —
   delivery activates at W1 with the encoder row.
5. `hello` timeout is enforced via a 3 s socket read timeout covering the WS
   upgrade + first frame (a stalled TCP connect cannot pin a thread).
6. Token comparison is a plain string equality — not constant-time. LAN tool,
   16-byte random token, W0 posture (WQ-1 family); revisit at W4 with TLS.

7. `update_output()` is not a strict no-op: it pushes into the device-internal
   SPSC, mirroring `LaunchpadNode`'s legacy path. Never called once the output
   handle is taken; kept for symmetry.

**Pre-commit review findings (fixed before commit):**

- **MAJOR — audio-thread panic on multi-client event burst:** `process()`
  drained up to 4×256 ring events into the executor's 256-capacity
  `EventOutputBuffer`, which asserts on overflow. Fixed with
  `DRAIN_BUDGET_PER_CYCLE = 250`; leftovers stay in the rings for the next
  cycle. Regression test `process_drain_respects_event_budget`.
- Cleanup-order race (slot freed before producer returned → fast reconnect
  bounced) and producer leak on client-thread panic: both fixed by replacing
  manual cleanup with an RAII `SlotGuard` (producer returned first, runs on
  unwind).
- Non-hello first frame now gets `bye "expected hello"` (was misreported as
  a timeout); replay batch skips the phantom 80–89 id gap; unused `log` dep
  removed.

**Recorded risks (accepted at W0, WQ-1 family):**

- `ws.send()` has no write timeout: a silently-dead peer (zero TCP window)
  can pin its slot and let the unbounded outbound mpsc grow until TCP gives
  up. Revisit with a write timeout or mpsc cap if observed in practice.
- The accept loop spawns one pre-auth thread per connection with no cap, and
  the 3 s hello timeout bounds each read syscall, not the handshake total —
  a byte-trickling LAN client can hold pre-auth threads open.
- Token comparison is not constant-time (see deviation 6).
- Diagnostics are `eprintln!` per codebase convention; once the TUI owns the
  terminal these prints corrupt it — a codebase-wide issue (LaunchpadNode
  logs per LED update today). W1 follow-up: TUI-safe diagnostics sink.

**New workspace dependencies:** `tungstenite 0.21`, `tiny_http 0.12`,
`serde_json 1` (all MIT/Apache-compatible).

## Commit 2 — app wiring + client

**Shipped:**

- App wiring: `ID_THEORIA = 106`, `ID_GW_THEORIA = 113`; flags `--no-antiphon`,
  `--antiphon-port` (default 7274), `--theoria-dir` (default `web/theoria`);
  spawn after hardware devices, before executor build; gateway drained in
  main-loop step 2; random 16-byte hex token printed as
  `Theoria: http://<lan-ip>:<port>/?t=<token>` (stderr only — the TUI
  transport bar was not trivially reachable, per spec fallback).
- `welcome` snapshot assembled by `collect_node_summaries()` from the cap-doc
  cache; antiphon never touches the configurator. Builder now calls
  `set_type_tag_for()` for every YAML node, so snapshots carry real tags
  (`analog_engine:kick` …) — this also un-breaks type_tags for project-file
  v2 saves of instrument-built graphs, which previously had none.
- LED mirroring: script output addressed to the Launchpad/emulator device is
  cloned onto `ID_THEORIA` in the main loop (merge via `entry().and_modify()`
  so direct-to-Theoria output is preserved).
- **Profile:** `launchpad.rhai` changed by exactly one line —
  `on_surface_event([LP_DEVICE_ID, THEORIA_DEVICE_ID], |event| …)`. Supported
  by a new **array overload** of `on_surface_event` in `paraclete-scripting`
  (one inline handler registered for N device ids; FnPtr cloned per id).
  Rationale: Rhai's inline-only constraint (see memory/profile comments) makes
  it impossible for a profile to share one closure across two registration
  calls; the overload is a generic capability, not a Theoria special case
  (universality check: any future surface joins a handler the same way).
- `web/theoria/`: zero-build canvas client per spec — 8×8 grid + scene column
  + control row, multi-touch pointer tracking with drag-off release, LED
  batches, STALE banner ≤ 500 ms, 1→2→4→5 s reconnect backoff with hello
  replay, protocol-mismatch hard error, RTT + touch→LED overlay.

**Verified headless (epiclesis-style Node driver, localhost):** hello→welcome
(protocol 0, device 106, 12 nodes with real params) in ~22 ms from socket
open; full-surface replay (88 ids); SEQ_EDIT via glass lights green;
step-toggle round trip **touch→profile→CMD_TOGGLE_STEP→state-bus→set_led→
mirror→WS echo in 24–34 ms** on localhost. Driver script preserved in the
session scratchpad; a reusable epiclesis harness is W1 C5 material.

**Pre-commit review findings, Commit 2 (fixed before commit):**

- **MAJOR — restart→reconnect was impossible:** the token was regenerated per
  run, so a client auto-reconnecting after an app restart got
  `bye "bad token"` and hard-errored. Fixed: the token persists in
  `.antiphon-token` (CWD, gitignored; delete to rotate); verified two
  consecutive runs print the same URL. `fastrand` is not a CSPRNG —
  acceptable under the recorded W0 LAN posture.
- Welcome snapshot moved after project load/save (step 6.5): a v2 `--load`
  that restores topology is now reflected in `welcome.nodes`, and the Theoria
  surface/gateway stay out of `--save` project files. `playing: true` in the
  static transport snapshot is truthful (InternalClock auto-starts) and
  commented as replaced by the W1 state mirror.
- Mirror ordering: mirrored Launchpad updates now go *first* in the merged
  batch so a profile's direct-to-Theoria write wins last-write-wins
  downstream (mattered for W1 encoder-context colors).
- Scripting array overload: ids deduped within a call (a repeated id — e.g.
  two absent devices both injected as 0 — would double-fire handlers, making
  toggle surfaces look dead), negative ids rejected with a log line.
- Client: pong-liveness watchdog (2 missed pongs → close → STALE + reconnect,
  covers silent Wi-Fi death), `bye "full"` keeps retrying instead of
  hard-erroring, `canvas { touch-action: none }` belt-and-braces, draw-loop
  and overlay perf nits.
- Spec-letter note: the spec's `ScriptingGatewayNode::new(ID_GATEWAY_THEORIA,
  256)` would tag events with the gateway id (113) and break the profile
  match; the ctor takes the *device* id, so the implementation passes
  `ID_THEORIA` (106) — the correct reading of the spec's intent.

**Rhai finding (recorded):** `Engine::call_fn` re-evaluates the AST top level
before invoking `on_load`, so top-level statements in profiles run twice.
Existing profiles are safe (everything lives inside `fn on_load()`); the new
scripting test documents the idiom.

## Exit criteria

- [x] Tablet (client) toggles steps; audio graph reflects it — verified
      headless via WS driver + dev-ui/LED echo; **tablet-hardware pass
      pending user session**
- [ ] Launchpad + Theoria simultaneously, same LEDs — needs physical hardware
      (user session; s0-launchpad-debug.md LED confirmation still pending)
- [x] `launchpad.rhai`: one-line device-id alternation only
- [x] Kill app → STALE ≤ 500 ms; restart → auto-reconnect (token persists in
      `.antiphon-token` across restarts — verified; on-device check in user
      session)
- [x] RTT + touch→LED figures logged (localhost: 24–34 ms full round trip;
      LAN figures to be added from the user session)
- [x] `cargo test --workspace` green (427 passed, 0 failed); clippy clean
