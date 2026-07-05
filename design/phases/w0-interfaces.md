# Paraclete — W0 Interface Specification (Antiphon + Theoria grid POC)

> **Implementation blueprint.** These are the contracts W0 implementation must
> satisfy. Do not deviate without updating `interface-plan.md`/ADR-031 first —
> if a contract here proves wrong in practice, **stop, record the conflict in
> the phase report, and ask** rather than redesigning inline.
>
> **W0 deliverable:** the browser grid as a peer device. A tablet toggles steps
> on all 8 tracks of the running instrument while the Launchpad / terminal
> emulator is connected; both surfaces show the same LEDs; the unmodified
> `launchpad.rhai` serves both.
> **Last updated:** July 2026
> **Baseline:** P10 C0 shipped (404+ workspace tests, 0 failures)
> **Depends on:** ADR-031; `design/interface-plan.md`; the P9.5 control-id map.
> **Model routing:** Commit 1 = Opus-tier. Commit 2 = Opus for app wiring,
> Sonnet-fine for the client (it is fully specified below). See
> `design/handoff.md`.

---

## Commit sequence

| Commit | Crate(s) | Deliverable |
|--------|----------|-------------|
| **1** | `paraclete-antiphon` (new) | Protocol v0 types + serde; WS server (accept/read/write threads, token auth, client slots); `TheoriaSurfaceNode` (Surface, SPSC pool in, output handle out); `kerygma` LED broadcast; `tiny_http` static file serving. Full unit-test list below. No app changes. |
| **2** | `paraclete-app`, `web/` | App wiring (device 106, gateway 113, CLI flags, main-loop integration); `web/theoria/` zero-build vanilla-JS client (canvas grid, touch, reconnect, RTT overlay); exit-criteria verification + latency log. |

---

## Protocol v0 (wire format)

JSON text frames over WebSocket. Internally tagged: every message is an object
with `"t"`. Unknown `"t"` or malformed frames are **logged and dropped**, never
fatal. `protocol: 0` = unstable pre-freeze; clients must check it in `welcome`
and show an incompatibility error on mismatch.

Wire names are deliberately plain (naming policy, `interface-plan.md`).
Messages marked **[W1]** are specified now so W0 code leaves room, but are NOT
implemented in W0.

### Client → server

```jsonc
{"t":"hello","token":"<hex>","client":"theoria-web/0.1"}   // must be first frame
{"t":"pad_down","id":13,"vel":65535}   // ids 0–63 grid (row*8+col), 64–71 scene, 72–79 control
{"t":"pad_up","id":13}
{"t":"ping","ts":123456.7}             // ts is client-clock ms, echoed verbatim
{"t":"pad_pres","id":13,"v":41000}     // [W1+] continuous per-pad pressure, 16-bit; parse-and-drop
                                       // at W0. Reserved so protocol freeze (W4) cannot foreclose
                                       // poly aftertouch from pressure-capable clients/surfaces.
{"t":"enc","id":90,"delta":-3}         // [W1] ids 90–97; delta = accumulated detents this frame
{"t":"enc_push","id":90,"pressed":true}                       // [W1]
{"t":"set_param","node":20,"param":"cutoff","v":0.5}          // [W1] semantic plane
{"t":"bump_param","node":20,"param":"cutoff","delta":0.01}    // [W1]
{"t":"node_cmd","node":10,"cmd":16,"a0":3,"a1":0}             // [W1] declared node commands; pass-through
```

### Server → client

```jsonc
{"t":"welcome","protocol":0,"device_id":106,
 "nodes":[{"id":20,"type_tag":"analog_kick","name":"AnalogEngine",
           "params":[{"id":123,"name":"cutoff","min":0.0,"max":1.0,"default":0.5}]}],
 "transport":{"playing":true,"bpm":120.0}}
{"t":"led","updates":[{"id":13,"rgb":[255,64,0]}]}   // batched; full-surface batch sent after welcome
{"t":"pong","ts":123456.7}
{"t":"bye","reason":"bad token"}                      // then close
{"t":"state","updates":[{"path":"/node/20/cutoff","v":0.62}]}          // [W1] coalesced ≤ ~30 Hz
{"t":"context","slots":[{"enc":90,"node":20,"param":"cutoff"}]}        // [W1]
{"t":"topology","nodes":[…]}                          // [W1] same shape as welcome.nodes, after apply_patch
```

Rust: one `enum ClientMsg` / `enum ServerMsg` in `protocol.rs` with
`#[serde(tag = "t", rename_all = "snake_case")]` and `#[non_exhaustive]`.
Include the [W1] variants now (parsing them is free; W0 server replies to the
semantic-plane ones with a logged drop). Velocity/pressure are 16-bit on the
wire (MIDI 2.0 resolution, matches `SurfaceEvent`); W0 client always sends
`vel: 65535` (per-view velocity is W1, WQ-2).

### Auth & session

- App generates a random 16-byte hex token at startup and prints
  `Theoria: http://<lan-ip>:<port>/?t=<token>` to stderr (and the TUI transport
  bar if trivially reachable — else stderr only, note it in the report).
- First frame must be `hello` with the right token within 3 s, else
  `bye` + close. The static HTTP side is unauthenticated (it serves only the
  client bundle); the token gates the WS session. Bind `0.0.0.0`; the token is
  the gate. LAN tool; no TLS at v0 (recorded risk, WQ-1 family).

---

## Crate: `paraclete-antiphon`

GPL3 platform crate. **Dependencies:** `paraclete-node-api` (path),
`tungstenite = "0.21"`, `tiny_http = "0.12"`, `serde` + `serde_json`,
`rtrb = "0.3"`, `log`. **No tokio. No `paraclete-runtime`** (BUG-003 trigger:
if `StateBusHandle` is needed at W1, that fires the move-to-L2 fix first —
W0 needs no state bus).

```
crates/paraclete-antiphon/src/
  lib.rs        // pub use; AntiphonConfig { port, token, static_dir }
  protocol.rs   // ClientMsg / ServerMsg + serde round-trip tests
  server.rs     // listener thread, per-client read threads, ClientSlot table
  kerygma.rs    // outbound broadcast: LED batching now; state coalescing at W1
  surface.rs    // TheoriaSurfaceNode + TheoriaOutputHandle
  http.rs       // tiny_http thread serving static_dir (W1: embedded via include_dir)
```

### Threading & data flow (fixed at W0)

```
per-client WS read thread ──rtrb SPSC (slot i)──► TheoriaSurfaceNode::process()   [audio thread]
                                                        │ emits Event port → gateway 113 → Rhai
main thread: handle.tick() ◄── device-internal SPSC ◄── node pushes SurfaceOutput
      │
      └► kerygma: serialize led batch → per-client std::sync::mpsc → WS write thread
```

- **`MAX_CLIENTS = 4`** fixed slots. Each slot owns: one inbound `rtrb`
  producer (held by its read thread), one outbound `std::sync::mpsc` sender
  (held by kerygma), one write thread. Slot freed on disconnect. 5th
  connection → `bye {"reason":"full"}`.
- All clients multiplex onto the **one** surface node (ADR-031 §Alternatives:
  last-writer-wins; per-client nodes are W4).
- Ring capacities: inbound 256 events/slot; outbound unbounded mpsc is
  acceptable (main thread producer, W0 traffic is LED batches only).
- `process()` drains **all** slot rings each cycle (fixed iteration, no
  allocation), translating to `SurfaceEvent` and emitting on the event out
  port — mirror `LaunchpadNode`'s pattern exactly.
- JSON parse/serialize happens **only** on WS threads and main thread
  (`kerygma`), never the audio thread. The audio thread sees pre-decoded
  `SurfaceEvent` values from the rings.

### `TheoriaSurfaceNode`

- `Surface` impl. `SurfaceDescriptor`: name `"Theoria Surface"`, vendor
  `"Paraclete"`, 80 pads (grid 0–63 with row/col, scene 64–71, control 72–79 —
  same ids as the P9.5 emulator) **plus** 8 `EncoderDescriptor`s ids 90–97,
  `behaviour: Relative`, `has_push: true`, declared now so W1 adds no
  descriptor change. Pads: `velocity_sensitive: true`,
  `pressure_sensitive: false` (WQ-2).
- `take_output_handle()` → `Some(TheoriaOutputHandle)`; `tick()` drains the
  device-internal SPSC of `SurfaceOutput` and forwards `led_updates` to
  kerygma. `update_output()` is a no-op (P4+ path only).
- After `welcome`, kerygma sends one **full-surface** `led` batch from a
  main-thread shadow copy of the last known color per control (kerygma owns a
  `[Rgb; 98]` shadow table; updated on every batch; replayed to new clients).

### `kerygma` (broadcast module)

W0 scope: LED fan-out + shadow table + `welcome` snapshot assembly
(`nodes` built from data passed in at construction by the app — a
`Vec<NodeSummary>` the app assembles from the configurator's cap-doc cache;
antiphon does not talk to the configurator directly). W1 scope (spec'd, not
built): state-diff coalescing at ≥ 33 ms intervals from paths the app pumps in.

---

## Commit 2: app wiring + client

### App wiring (`paraclete-app`)

- Constants: `ID_THEORIA = 106`, `ID_GATEWAY_THEORIA = 113`; Rhai-injected
  constant `THEORIA_DEVICE_ID` (all profiles; same pattern as `LP_DEVICE_ID`).
- CLI: `--no-antiphon` (off switch; default ON), `--antiphon-port=7274`
  (default 7274), `--theoria-dir=web/theoria` (static root; relative to CWD).
- Startup (after hardware devices, before executor build):
  `AntiphonServer::spawn(config, node_summaries)` →
  (`TheoriaSurfaceNode`, `AntiphonHandle`);
  `conf.add_surface(ID_THEORIA, node)`; add
  `ScriptingGatewayNode::new(ID_GATEWAY_THEORIA, 256)` wired
  `theoria events_out → gateway events_in`; drain its consumer in main-loop
  step 2 with the others.
- **`launchpad.rhai` is not modified.** The gateway's events dispatch to the
  same handlers keyed by pad id; the profile's device-id checks must accept
  `THEORIA_DEVICE_ID` wherever they accept `LP_DEVICE_ID` — if the profile
  hard-gates on `LP_DEVICE_ID`, add the alternation **in the profile** (one
  line), not in Rust. LED output: the app mirrors `SurfaceOutput` destined
  for the Launchpad/emulator to the Theoria node as well — implement by
  registering the Theoria device as an additional recipient in
  `deliver_script_output()` for LED commands addressed to `LP_DEVICE_ID`
  (main-thread code, allocation is fine there). Record in the report if the
  existing routing makes a cleaner hook available.

### Client: `web/theoria/` (zero-build at W0 — deliberate)

Plain ES modules, **no toolchain** (vite + TS workspace arrives at W1; W0
removes build-system risk from the handoff):

```
web/theoria/
  index.html    // <canvas>, fullscreen viewport meta, dark background
  theoria.js    // everything below; ~300 lines is the expected scale
```

- Canvas-rendered surface: 8×8 grid + scene column (right) + control row
  (top), matching the emulator layout. ≥ 44 px cells at tablet sizes
  (`canvas` sized from `visualViewport`, redrawn on resize).
- Pointer events (multi-touch): `pointerdown` on a cell → `pad_down`,
  `pointerup`/`pointercancel`/leave → `pad_up`. Track active pointers so a
  drag off a cell releases it (no stuck pads — same guarantee the emulator's
  held-key map gives).
- LED batches set cell colors; unknown ids ignored.
- Connection: token from `?t=`; on close → grey **STALE** banner over the
  whole surface within 500 ms (usability floor: a dead surface must look
  dead), auto-reconnect with 1 s → 2 s → 4 s (cap 5 s) backoff, `hello` replay.
- RTT overlay (corner text): `ping` every 2 s, show rolling median of last 10.
  Touch→LED latency: on `pad_down`, record `performance.now()`; on the first
  `led` batch containing that id, log the delta to console and fold it into
  the overlay. This is the WQ-1 measurement instrument.
- `protocol !== 0` in welcome → hard error screen, no reconnect loop.

---

## Tests (Commit 1, all in `paraclete-antiphon`)

1. `protocol_round_trip_client_msgs` — every `ClientMsg` variant JSON round-trips
2. `protocol_round_trip_server_msgs` — same for `ServerMsg`
3. `protocol_unknown_tag_is_error_not_panic`
4. `pad_msg_to_hardware_event_mapping` — pad_down/up/enc/enc_push → correct `SurfaceEvent`
5. `surface_descriptor_counts` — 80 pads + 8 relative encoders, ids exact
6. `node_process_drains_all_slots` — events pushed into 2 slot rings both emerge from the event port in one `process()`
7. `node_process_is_allocation_free_shape` — drains with zero slots connected without panic; fixed-size iteration (structural test)
8. `output_handle_forwards_led_updates` — `SurfaceOutput` pushed → `tick()` → kerygma receives the batch
9. `kerygma_shadow_replays_full_surface_to_new_client` — set 3 LEDs, connect, first batch after welcome carries them
10. `hello_bad_token_gets_bye` — server logic level (feed frames through the handshake fn, no real socket)
11. `hello_must_be_first_frame`
12. `fifth_client_rejected_full`
13. `slot_freed_on_disconnect_and_reusable`
14. `w1_variants_parse_but_are_dropped` — `set_param` parses; handler logs+drops at W0

Handshake/slot tests exercise the pure functions (frame → decision), not live
sockets. One `#[ignore]`-tagged smoke test may open a real localhost socket
pair (mirrors the clap-validator precedent).

## Exit criteria (Commit 2 — verified with the user, paired-session style)

- [ ] Tablet toggles steps on **all 8 tracks**; audio reflects it
- [ ] Launchpad/terminal emulator connected simultaneously; **both** surfaces
      show the same LED state; either can toggle
- [ ] `launchpad.rhai` unmodified (or: only the device-id alternation line)
- [ ] Kill the app → client shows STALE within 500 ms; restart → auto-reconnects
- [ ] Median RTT and touch→LED figures logged in the phase report (`w0-report.md`)
- [ ] `cargo test --workspace` green; zero clippy warnings in the new crate

## Explicitly deferred to W1 (do not build early)

State mirror pumping, semantic-plane execution, encoder UI, embedded bundle
(`include_dir`), per-view velocity, TS/vite workspace, Preact chrome, any view
beyond the grid. The [W1] protocol variants exist only as parse-and-drop.
