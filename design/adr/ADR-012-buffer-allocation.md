# ADR-012: Buffer Allocation — Per-Connection, Main Thread

**Date:** May 2026  
**Status:** Accepted

## Decision

Audio buffers are allocated once per connection at graph construction time on the main thread (NodeConfigurator). The NodeExecutor passes buffer references to nodes. No allocation ever occurs on the audio thread.

## Context

Three options were considered:

**Node-owned output buffers:** Each node pre-allocates its own output buffers at initialisation. Simple but wastes memory on unconnected ports and complicates buffer lifetime.

**Runtime buffer pool:** A fixed pool of buffers allocated at startup, assigned to connections per cycle and returned after. Flexible but requires pool sizing at startup and adds management complexity.

**Per-connection allocation:** At graph construction time (main thread, allocation is fine), the runtime allocates exactly one buffer per connection. Buffers live for the lifetime of the connection. The executor passes references — no allocation, no pool management, no lifetime complexity.

## Rationale

Per-connection allocation chosen because:

- Zero allocation on the audio thread — hard constraint met trivially
- Simple lifetime model — buffer lives as long as the connection
- No pool sizing decisions at startup
- Pattern used by dasp_graph and HexoDSP
- Buffer pool is an optimisation that can be layered on top later if profiling justifies it

## The `ProcessContext` contract

When the executor calls a node’s `process()` function it passes:

- `inputs: &[&AudioBuffer]` — one reference per connected input port, in port declaration order
- `outputs: &mut [&mut AudioBuffer]` — one reference per connected output port
- `events: &[TimedEvent]` — pre-filtered slice for this node from the cycle’s event buffer
- `transport: &TransportInfo` — current clock position, BPM, time signature, domain ID

The node never allocates. It reads from inputs and writes to outputs.

## Consequences

- Graph topology changes (add/remove connections) happen on the main thread only
- NodeConfigurator allocates and deallocates buffers when connections change
- NodeExecutor holds references into the configurator’s allocation — communicated via the lock-free ring buffer
- Buffer size (block size) is fixed per session and set at audio device initialisation
- Variable block sizes (e.g. end-of-pattern boundary) are a P5 concern, not P0