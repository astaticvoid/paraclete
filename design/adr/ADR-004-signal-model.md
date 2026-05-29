# ADR-004: Signal Model — Mixed-Rate Typed Ports

**Date:** May 2026  
**Status:** Accepted

## Decision

The signal model supports multiple rates via typed ports. Both block-rate and single-sample-rate processing are supported in the type system from day one. V1 implements block-rate only; single-sample is stubbed but not foreclosed.

## Signal types

|Type       |Rate                             |Use                            |
|-----------|---------------------------------|-------------------------------|
|AudioBuffer|Block (64–512 samples)           |Primary audio signal           |
|CvSignal   |Audio-rate, single-sample capable|Modular CV, feedback loops     |
|ControlRate|Modulation rate (sub-audio)      |LFOs, envelopes, automation    |
|EventStream|Event-driven, timestamped        |Notes, MIDI 2.0, custom events |
|ClockSignal|Transport-rate                   |Tempo, position, time signature|

## Rationale

Three options were considered:

1. **Block-rate only** — simple, fast, easy to implement. Closes the door on Reaktor/VCV Rack-style feedback loops. Rejected — violates Principle 1 (Stub Deep, Ship Shallow).
1. **Single-sample only** — maximum flexibility, enables all feedback topologies. High CPU cost, harder to optimise. Rejected for v1 — too complex for initial implementation.
1. **Mixed-rate with typed ports** — both rates represented in the type system, scheduler handles mixed-rate graphs. V1 ships block-rate only. Single-sample path added at Phase 9.

Option 3 was chosen.

## Consequences

- The L1 scheduler must be aware of port types at graph construction time
- The scheduler’s topology model must support cycles for CvSignal ports even in v1 (see ADR-005)
- Implicit conversion between compatible types is provided by the runtime (ControlRate → AudioBuffer via sample-and-hold) but explicit conversion nodes are preferred