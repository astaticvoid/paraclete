# ADR-031: Antiphon — the interface server and protocol

**Status:** Accepted (July 2026)
**Context documents:** `design/interface-plan.md` (accepted plan), ADR-014
(capability document), ADR-018 (cellular architecture), ADR-019 (universal
parameter control), ADR-026 (TUI / `publish_context()`).

---

## Context

The instrument needs a rich bidirectional control surface: a tablet/browser
client as the primary control-and-editing device (full experience for
tablet-only users), the terminal as a permanent first-class surface, real
hardware (Launchpad, MIDI controllers) as peers, and a headless driver for CI.
Each surface needs the same things from the engine: live state, parameter
metadata, LED/context feedback, and a validated way to send commands.

Building the web client its own bespoke bridge would duplicate what the TUI
already does in-process and what every future client would need again. The
platform's existing vocabulary — `HardwareEvent`, `SurfaceDescriptor`,
`HardwareOutput`/`LedUpdate`, `CapabilityDocument`, ADR-019 commands, the
StateBus — already describes everything a surface is.

## Decision

**One presentation-agnostic interface server — Antiphon (`paraclete-antiphon`)
— serving every non-MIDI client over pluggable transports, speaking the
platform's existing surface vocabulary.**

1. **Antiphon is a platform crate** (GPL3, like `paraclete-tui`), not a layer.
   Its device-node side links `paraclete-node-api` (L2) only, per ADR-022.
2. **Transports are pluggable:** WebSocket (`tungstenite`, blocking threads +
   `rtrb` SPSCs — the `midir` pattern; **no tokio**) for remote clients, and
   an in-process channel (the same message types, no socket, no JSON) for the
   terminal client and test drivers. `tiny_http` serves the embedded web
   client bundle so the instrument remains one binary.
3. **Two command planes:**
   - **Surface plane** — device-shaped events (`pad_down`, `enc`, …) that
     become `HardwareEvent`s and flow through a `ScriptingGatewayNode` to Rhai
     profiles, exactly like physical hardware. Profiles cannot tell glass from
     plastic except by capability.
   - **Semantic plane** — ADR-019-shaped commands (`set_param`, `bump_param`,
     name-resolved and validated against the target's `CapabilityDocument`)
     plus declared node commands. The semantic plane may only carry what nodes
     already declare — no client-only side doors into the engine.
4. **Surfaces manifest as device nodes.** Connected clients are represented in
   the graph by a `HardwareDevice` node with a gateway, exactly like
   `LaunchpadNode`. The Antiphon server itself is transport plumbing — the
   same architectural standing as `midir`'s callback threads. This is the
   scripting-engine exemption shape (environment, not node), not a new class
   of platform object; ADR-018 holds.
5. **Downstream feed:** session snapshot on connect (device map, live node
   list with `type_tag` + cap-doc snapshots), batched LED updates (the same
   RGB feed physical devices get), coalesced state diffs (~30 Hz), encoder
   context, and topology updates after `apply_patch`. Clients bind to nodes
   **by discovery, never by hard-coded id**.
6. **Protocol names stay plain on the wire** (`hello`, `state`, `led`,
   `pad_down`). Evocative house names are reserved for our own components:
   the state/LED broadcast module inside Antiphon is `kerygma`
   (proclamation); the headless protocol test driver is `epiclesis`
   (invocation). See the naming policy in `design/interface-plan.md`.

## Alternatives considered

- **Web MIDI (browser as MIDI controller).** Rejected as primary: needs a
  loopback driver per OS, 7-bit ceilings, and no channel for cap-docs, state
  mirrors, or context — the bidirectional editor is impossible without
  inventing a SysEx protocol. May return as an optional input adapter (W4).
- **OSC over UDP.** No browser-native UDP; forces a gateway anyway.
- **tokio/axum stack.** Capable but imports an async runtime into a process
  whose concurrency model is deliberately threads-plus-SPSC; the blocking
  pattern already proven with `midir` is sufficient at stage/studio scale.
- **Bespoke web bridge now, generalize later.** Rejected: the terminal-client
  commitment (interface plan, WT milestone) makes the second consumer certain,
  and retrofitting transport-agnosticism after a WS-only v1 is a rewrite.
- **Per-client dynamic device nodes at v0.** Deferred: requires `apply_patch`
  integration on connect/disconnect. v0 multiplexes all clients onto one
  fixed surface node (last-writer-wins, like two hands on one box); dynamic
  per-client nodes revisit at W4.

## Consequences

**Positive:** one protocol serves web, terminal, and CI; profiles serve glass
and plastic alike; the LGPL-like separation repeats at the client (protocol
core separable from app); the encoder story detaches from hardware purchases
(touch deltas are true-relative by construction).

**Negative / accepted costs:** a second product surface (TS client) with its
own bug tail — sized early by W0's narrow exit criteria; JSON serialization
cost on the WS path (binary deferred until profiling demands); v0's
single-surface multiplexing means clients share one device identity until W4;
the protocol is explicitly unstable (`protocol: 0`) until the W4 freeze.

## Implementation

W0 blueprint: `design/phases/w0-interfaces.md`. Client/view architecture
(Theoria, the view-plugin extension layer) is a separate decision → ADR-032,
authored with W2.
