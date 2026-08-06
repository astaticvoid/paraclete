---
name: antiphon-wire
description: Inspecting the Antiphon WebSocket/HTTP protocol from an agent. Use when verifying protocol changes against the running graph, checking wire fields, or debugging client-server communication.
---

# Inspecting the Antiphon Wire

**The WebSocket port is the HTTP port + 1.** The app prints only the HTTP URL (`http://host:7274/`); clients derive the WS port themselves. Connecting to 7274 gets an HTTP 200 and a failed upgrade, which reads like a broken handshake.

## No Python WebSocket library needed

There is no Python WebSocket library in this environment (`websockets` and `websocket-client` are both absent). A ~60 line raw RFC 6455 client suffices. Client frames must be masked; server frames are not.

## When to use

Prefer wire inspection over reading `protocol.rs` alone whenever a change adds or populates a wire field — the mapper and the assembler can each look right while disagreeing.

Use it to check a protocol change against the **running graph** rather than only against fixtures.

## What to check

- `hello` handshake
- `get_view_meta` response
- State bus path population
- Whether "no shipped node declares a TRIG page" is fact or assumption
