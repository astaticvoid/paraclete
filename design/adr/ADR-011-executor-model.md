# ADR-011: Executor Model — Single-Threaded Push

**Date:** May 2026  
**Status:** Accepted

## Decision

The L1 NodeExecutor uses a single-threaded push model for P0 through at least P5. A partitioned parallel executor is explicitly deferred, not foreclosed.

## Context

Three execution models were considered:

**Pull model:** Leaf nodes (sinks) request data from upstream nodes lazily. Elegant but produces deep unpredictable call stacks, complicates cycle handling, and does not extend cleanly to parallel execution.

**Push model:** The executor performs a topological sort of the graph and walks it in dependency order, pushing data from sources to sinks. Each node is called exactly once per buffer cycle. Predictable, cache-friendly, cycle handling is explicit via loop-break nodes.

**Partition model:** The graph is pre-analysed into independent subgraphs assigned to thread pools, with synchronisation at merge points. Maximum CPU utilisation on multi-core hardware but significant real-time synchronisation complexity.

## Rationale

Push model chosen because:

- Maps directly to petgraph topological sort already committed to in ADR-005
- Handles cycles explicitly via loop-break nodes
- Is the proven pattern in HexoDSP, dasp_graph, and VCV Rack
- The `Node` trait design does not change when upgrading to partitioned execution later

## Path to partition model

The executor is entirely internal to `paraclete-runtime` (L1). Nodes in `paraclete-node-api` (L2) never see the executor directly. The upgrade path at P6 or later:

1. Replace the single-threaded walk loop in `NodeExecutor` with a partition analyser and thread pool
1. Add `Send + Sync` bounds to the `Node` trait — additive, not breaking
1. Nodes that cannot be made `Send` (e.g. some FFI nodes) declare this in their `CapabilityDocument` and are scheduled single-threaded

No node rewrites required. No API breaks. The architecture supports it — the decision is explicitly deferred, not foreclosed.

## Consequences

- P0 executor is a simple topological walk loop
- All nodes run on the audio thread sequentially
- No thread synchronisation overhead in early phases
- Profile at P6 with real multi-node graphs before deciding if partition model is justified