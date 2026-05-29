# ADR-005: Scheduler Must Support Cycles from P0

**Date:** May 2026  
**Status:** Accepted

## Decision

The L1 scheduler’s internal topology representation must support cyclic graphs from Phase 0, even though single-sample execution (which requires cycles) is not implemented until Phase 9.

## Context

VCV Rack’s single-sample processing enables feedback loops — a fundamental requirement for modular synthesis that cannot be retrofitted into a block-rate scheduler that assumes a DAG. VCV Rack uses explicit loop-break nodes (single-sample delay nodes) to allow cycles while preserving deterministic execution order.

Meadowlark’s failure analysis shows that foundational assumptions, once load-bearing, are extremely expensive to change. A P0 scheduler that assumes a DAG will require a full rewrite at P9.

## Decision detail

- The scheduler uses petgraph for topology internally
- Cycle detection is implemented at connection time and reports cycles to the caller
- In v1 (P0–P8), connecting CvSignal ports in a cycle is rejected at runtime with an error — but the rejection is a runtime check, not an architectural impossibility
- At P9, explicit loop-break nodes are introduced. When a loop-break node is present in a cycle, the scheduler accepts the cycle and handles it via the loop-break’s single-sample delay semantics
- The loop-break node approach (explicit, composable) is preferred over automatic latency compensation (implicit, fragile)

## Consequences

- P0 scheduler code must use petgraph and must not assume DAG topology
- No `topological_sort` call that panics on cycles — use the cycle-aware variant
- The loop-break node is a first-class node type, not a scheduler hack