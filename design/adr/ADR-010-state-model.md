# ADR-010: State Model — Cybernetic Hybrid

**Date:** May 2026  
**Status:** Accepted

## Decision

Cybernetic hybrid state model:

- **Private state:** node owns it entirely, runtime serialises/deserialises opaque blobs for save/recall
- **Published state:** node explicitly pushes named values onto the state bus — other nodes or the UI can subscribe
- **Orchestration state:** runtime owns graph topology, connection map, transport — readable by all, writable only through defined messages

## Rationale

Three pure options were considered:

**Node owns everything:** Simple, like VST. But nodes cannot observe each other’s state, UI cannot discover parameters, and the sequencer cannot discover lockable parameters on connected nodes. Rejected — insufficient for parameter locks.

**Shared state store:** All nodes read/write to a common addressed state space the runtime manages. Plan 9-like. Powerful but introduces contention, ordering problems, and makes the runtime a brain rather than a nervous system. Rejected — violates Principle 3 (Cybernetic Architecture).

**Hybrid:** Nodes own private state. They can publish named values to the bus. The runtime owns topology and transport. Chosen.

## The cybernetic principle

Computation happens at the edges. The runtime is a nervous system — it orchestrates without controlling. Nodes are intelligent agents. The state bus is the universal observable namespace, not a database.

## Consequences

- The state bus is the mechanism for GUI display, parameter lock discovery, hardware mapping, and cross-node communication
- Nodes that want to be lockable by the sequencer must implement `StatePublisher` and publish their parameters
- The save/recall format is: runtime topology blob + per-node private state blob + optional state bus snapshot