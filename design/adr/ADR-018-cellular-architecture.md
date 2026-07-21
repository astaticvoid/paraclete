# ADR-018: Cellular Architecture — The Node is the Universal Primitive

**Date:** May 2026
**Status:** Accepted

---

## Decision

Every component that participates in data flow, state management, or event
routing within the platform is a **Node** — it implements the `Node` trait
and exists within the graph. There is no legitimate second class of platform
object. When a design appears to require a non-node component, the correct
response is to extend the Node API, not to create an exception.

One component is exempt by nature: the **scripting engine** (L4). It is the
live environment within which nodes operate — analogous to a Lisp interpreter
— not a cell within the system. It interacts with nodes exclusively through
their declared interfaces: capability documents, state bus, and the NodeCommand
channel. It never reaches into L1 internals.

---

## Context

The platform is a cellular, cybernetic system. Each node is a cell. Cells have
defined inputs and outputs. Cells communicate through connections. The whole
emerges from composition, not from central coordination.

This principle has been implicit in the design from P0 — `HardwareDevice: Node`
was the right model from the first phase, and "All hardware appears as graph
nodes above the HAL boundary" has been stated in architecture-core.md from
the beginning.

Two design pressures created or risked creating non-node abstractions:

**1. `NodeOrDevice` enum in the executor (P1)**

Hardware devices needed special dispatch for `update_output()`, so the
executor introduced `NodeOrDevice` to hold either a `Box<dyn Node>` or a
`Box<dyn HardwareDevice>`. This is an implementation-level bifurcation
tolerated as technical debt until Rust 1.76+ trait object upcasting is the
minimum version, at which point it is resolved.

**2. `HardwareController` proposal (P4 design)**

To solve the "update_output() on audio thread" problem — deferred from P3 —
a `HardwareController` trait was proposed that would have removed Launchpad
and XL from the graph entirely, routing their events through a separate
main-thread dispatch path. This proposal is **rejected**.

The problem it addressed is real. The solution was wrong: it solved a
threading constraint by creating a structural exception, then required the
whole system to accommodate that exception (separate SPSC paths, separate
event dispatch, separate registration API). The correct solution keeps
devices in the graph and resolves the threading constraint within the node
model using a device-internal output SPSC — the device creates an output
handle at construction time that the main thread holds and drains.

---

## Rationale

**Cellular architecture is not an aesthetic preference. It is what makes the
platform composable, testable, and open.**

A hardware controller that is a graph node:
- Can be connected to any other node's inputs without platform changes
- Can be replaced by a different controller with the same port contract
- Can be emulated by a software node for testing and development
- Has its parameters and state reachable by any other node generically
- Can participate in capability negotiation like any other node

A hardware controller outside the graph:
- Can only be reached through its specific dispatch path
- Cannot be connected, composed, or replaced without platform changes
- Cannot be generically tested
- Creates a two-tier system where "hardware" and "nodes" are different things

The same logic that led to "any pressure to add musical logic to the runtime
is a signal to extend the Node API" applies universally: **any pressure to
create a non-node platform component is a signal that the Node API needs
extension.** The question is never "should this be outside the graph" — it
is always "what does the Node API need in order to express this correctly."

---

## Parameters as Generic Hardware Contracts

This ADR also establishes the parameter control consequence of the cellular
principle.

A node declares its parameters in `capability_document()`. Those declarations
are **hints** — flexible contracts, not rigid specifications. The platform
treats every declared parameter as generically reachable: any hardware control
can adjust any declared parameter via `CMD_SET_PARAM` and `CMD_BUMP_PARAM`
without knowing the node's specific type.

This is the cellular model applied to parameter control:
- The node is the cell
- Its capability document is its declared interface
- Any other cell (hardware controller, sequencer, scripting layer) can reach
  its parameters through that interface without prior knowledge of its type
- When two nodes negotiate a richer shared vocabulary via `ConnectionAgreement`,
  richer behavior activates automatically
- When they don't, the generic form always works and nothing breaks

Parameters that are not declared in `capability_document()` are private to
the node. Parameters that are declared are reachable by the platform. This is
the boundary.

---

## Consequences

**Immediate (P4):**

- `HardwareController` trait is not introduced. `LaunchpadNode`,
  `LaunchControlXlNode`, and `KeystepNode` all implement `HardwareDevice: Node`
  and exist in the audio graph.
- The `update_output()` threading problem is resolved via device-internal
  SPSC. Each device creates an output handle at construction that the
  configurator holds. The audio-thread side pushes `HardwareOutput` to the
  internal ring; the main thread drains it and calls the physical output.
  The graph is not compromised.
- `CMD_SET_PARAM` and `CMD_BUMP_PARAM` are universal NodeCommand types handled
  by default Node trait machinery. Every node that declares parameters in
  `capability_document()` accepts these commands with no additional code.

**Ongoing:**

- Any future design that proposes a non-node platform component must first
  produce an ADR arguing why Node API extension is insufficient. Absent that
  argument, the proposal is invalid on its face.
- The `NodeOrDevice` enum is marked as technical debt in the executor.
  It is resolved when Rust 1.76+ is the minimum version, using trait object
  upcasting to eliminate the bifurcation. No API change for node authors.
- The scripting engine's exemption is permanent and non-expandable. It is the
  environment. No other component earns that status.

---

## References

- `architecture-core.md` — Design Principles §2 and §3 (updated alongside this ADR)
- ADR-008 — Node API Level (single trait, three implicit levels)
- ADR-014 — Capability Document (mandatory with defaults; parameters as contracts)
- ADR-015 — Connection Negotiation (bilateral; graceful degradation)
- ADR-017 — Effect Node Architecture (effects as plain nodes)
- P3 implementation report — "update_output() stays in executor at P3;
  moving hardware callbacks to main thread is a P4 item and requires a
  device dual-ownership design"

---

## Implementation note (appended 2026-07-21)

The "exemption is permanent and non-expandable… no other component earns
that status" line above (2026-05) has been **extended in practice** by
later decisions, all following its own test (environment/plumbing that
interacts only through declared interfaces — cap-docs, state bus,
NodeCommand channel — and never reaches into L1 internals):

- the **Antiphon interface server** (ADR-031 §"Nodes All the Way Down") —
  transport plumbing whose in-graph presence is its device/gateway nodes;
- **`paraclete-tui`** — a main-thread, read-only state-bus subscriber
  (ADR-026); display environment, no graph presence;
- **Theotokos** (ADR-036, proposed 2026-07-21) — the keyboard-first
  performance terminal, same standing claimed and justified there.

The bar set here still holds: each of these was granted the standing **by
an ADR arguing the environment shape**, not by accretion. Future non-node
components remain invalid without the same argument.
