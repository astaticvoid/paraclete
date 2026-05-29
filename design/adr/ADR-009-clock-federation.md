# ADR-009: Clock Domain Federation

**Date:** May 2026  
**Status:** Accepted

## Decision

No single mandatory clock owner. The runtime maintains a primary domain. Any node implementing `TempoSource` can register a clock domain. The runtime federates between domains — events crossing boundaries are timestamped in both coordinate systems.

## Priority order (descending)

1. DAW host transport (when running as CLAP plugin)
1. Ableton Link (when active)
1. External hardware clock (CV or MIDI clock input)
1. Internal transport (standalone default)
1. Node-local sub-domain (polyrhythmic sequencer, etc.)

## Rationale

Most audio frameworks impose a global clock. This makes polyrhythm, independent sequencer clocks, and external sync painful or impossible to do cleanly. VST2 had no proper transport model — it was bolted on via special MIDI messages.

The Elektron sequencer — the platform’s first deliverable — requires this model. Parameter locks, conditional trigs, and micro-timing are all forms of local time opinion within a broader clock context.

Clock domain ownership is negotiated at runtime, not assigned at compile time. A source can yield to a higher-priority source. Domains never fight — they federate.

## Open question

OQ-6 (unresolved): Elektron micro-timing is expressed as step-internal offsets, not sub-clock domains. Whether this fits cleanly into the federation model or requires a sequencer-local representation needs resolution before Phase 2.