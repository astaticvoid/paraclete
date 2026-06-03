# ADR-016: Multi-Track Sequencer Architecture

**Date:** May 2026  
**Status:** Accepted

## Decision

Multiple sequencer tracks are implemented as multiple `Sequencer` node instances
connected in parallel to one `InternalClock` source, not as a single `Sequencer`
node with an internal track array.

## Context

The P4 Launchpad profile requires 8 tracks visible simultaneously — one per row
of the 8×8 grid. The current `Sequencer` node has one track. This decision
determines how multi-track is implemented before P4 begins.

Two options were considered:

**Option A — Single node, internal track array**
One `Sequencer` node manages N tracks internally. The node's state includes an
array of patterns. The Launchpad profile talks to one node to control all tracks.

**Option B — Multiple node instances (chosen)**
Each track is a separate `Sequencer` node instance. The Launchpad profile
manages 8 node instances, one per row. Each connects to the same `InternalClock`.

## Rationale

Option B is correct for three reasons:

**1. Consistent with the platform model**
The platform is a graph of composable nodes. A drum machine is a graph of
sequencer nodes, sampler nodes, and effect nodes — not a monolithic drum machine
node. The composability is the point. If the "drum machine" is a single node,
the platform's architecture is bypassed rather than exercised.

**2. Each track is independently replaceable**
Track 0 (kick) is a `Sequencer` → `Sampler` chain. Track 7 (bass) could be a
`Sequencer` → `FmSynth` chain. With Option A, all tracks share one node type
and one sound source. With Option B, each track can have a different instrument.
This is what "integrated at all levels" means — the sequencer and the instrument
are paired per track, not separated into one sequencer controlling many instruments.

**3. The profile script manages the complexity**
The Rhai profile script tracks which `Sequencer` node corresponds to which
Launchpad row. The script selects the active track, routes XL knob movements
to the correct node's parameters, and handles pattern switching. This logic
belongs in the scripting layer, not in a monolithic sequencer node.

## State Bus Schema for Multi-Track

Each `Sequencer` node publishes to its own state bus namespace:

```
/node/{id}/state/current_step
/node/{id}/state/pattern_length
/node/{id}/state/playing
/node/{id}/state/track_name
```

The Launchpad profile subscribes to all 8 track nodes' `current_step` values
to update the step position indicators across all rows simultaneously.

## Track naming convention

Each `Sequencer` node is created with a `track_name` parameter in its
`CapabilityDocument`. The profile script uses this to display track names
and to maintain the row→node_id mapping.

```rust
// In paraclete-app P4:
let kick_seq_id = conf.add_node(Box::new(Sequencer::with_name("Kick")));
let snare_seq_id = conf.add_node(Box::new(Sequencer::with_name("Snare")));
// etc.
```

## Consequences

- P4 ships 8 `Sequencer` node instances connected to one `InternalClock`
- Each `Sequencer` connects to its own `Sampler` (or other instrument node)
- The Rhai profile script manages the 8-node array
- The state bus has 8 × N published paths (one set per track)
- Pattern switching switches patterns on individual nodes, not a global pattern
- Song mode (P5) chains patterns across all 8 nodes synchronously
- The `Sequencer` node itself does not change — multi-track is a graph topology,
  not a node feature

## Note on BPM

All 8 `Sequencer` nodes share one `InternalClock` node. Clock domain federation
(ADR-009) means individual tracks could eventually slave to different sub-domains
for polyrhythm. At P4 all tracks share the master clock.
