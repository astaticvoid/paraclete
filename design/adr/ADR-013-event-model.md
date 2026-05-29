# ADR-013: Event Model — Typed Enum with Extended Slab

**Date:** May 2026  
**Status:** Accepted  

## Decision

Events are represented as a `TimedEvent` struct containing a `sample_offset` and a typed `Event` enum. Common event types are fixed-size enum variants. Rich custom events use an `Extended` variant that references a pre-allocated slab. No heap allocation occurs on the audio thread.

## Context

Four options were considered:

**Trait objects (`Box<dyn Event>`):** Maximally flexible but allocates on the heap. Immediately ruled out.

**Fixed enum:** No allocation, fixed size, simple. Not extensible — third parties cannot add custom event types. Violates Plan 9 extensibility principle.

**Tagged byte buffer (LV2 atom style):** Real-time safe, arbitrary size. Loses Rust type safety, requires a mini serialisation system, unsafe code spread throughout.

**Typed enum with extended slab (chosen):** Common cases are typed enum variants the compiler optimises aggressively. The `Extended` variant is an escape hatch pointing into a pre-allocated byte slab. Unsafe code is contained in one small auditable function.

The model is inspired by UTF-8's self-describing variable-length encoding and CLAP's event header system — contiguous buffer, each entry carries its own size, unknown types can be safely skipped.

## Event type definition

```rust
#[derive(Clone, Copy)]
pub struct TimedEvent {
    pub sample_offset: u32,
    pub event: Event,
}

#[derive(Clone, Copy)]
pub enum Event {
    /// MIDI 2.0 Universal MIDI Packet.
    /// Covers all performance events: note on/off, per-note expression,
    /// pitch bend, pressure, channel controllers, program change, sysex.
    /// UMP encodes message type in words[0] >> 28 — no further decomposition needed.
    Midi2(midi2::UmpMessage),

    /// Sequencer parameter lock. Overrides a parameter value for one step.
    ParamLock(ParamLockEvent),

    /// Clock domain state change — loop point, stop, start, reposition.
    Transport(TransportEvent),

    /// Tempo change in BPM.
    Tempo(f64),

    /// Custom extended event. Points into the cycle's ExtendedEventSlab.
    /// Placed last in the enum — later match arms are treated as less likely
    /// by LLVM, keeping the common variants in the hot path.
    /// space_id is negotiated at connection time via ConnectionAgreement.
    /// Stub at P0 — infrastructure ships at Phase 3.
    Extended(ExtendedEventRef),
}
```

## Why Midi2(UmpMessage) not NoteOn/NoteOff variants

An earlier draft of this ADR decomposed MIDI events into separate enum variants:
`NoteOn(NoteOnEvent)`, `NoteOff(NoteOffEvent)`, `ParamChange(ParamChangeEvent)`.

This was superseded when `midi2` was adopted as a hard dependency in `paraclete-node-api`
(see ADR-002 consequences). UMP is a fixed 128-bit packet that already encodes message
type in the top nibble of the first word. Decomposing it further would:

- Duplicate the MIDI 2.0 type hierarchy in Paraclete's own types
- Require translation at every HAL boundary and CLAP integration point
- Diverge from how hardware and CLAP both deal with MIDI natively

Nodes that need to handle specific message types match on the UMP message type
using the `midi2` crate's accessor methods:

```rust
match event.event {
    Event::Midi2(ump) => {
        use midi2::ChannelVoice2;
        if let Some(note_on) = ump.try_note_on() {
            self.handle_note_on(note_on);
        }
    }
    Event::ParamLock(e) => self.handle_param_lock(e),
    _ => {}
}
```

## Why Extended is last, not #[cold]

`#[cold]` is valid on functions in Rust, not on enum variants. The original ADR
incorrectly documented `#[cold]` as applicable to the `Extended` variant.

The correct approach for cold-path hinting is:

1. Place `Extended` last in the enum — LLVM treats later exhaustive match arms
   as less likely and optimises accordingly.

2. For explicit cold-path annotation, wrap the handling in a function:

```rust
#[cold]
#[inline(never)]
fn handle_extended_event(r: &ExtendedEventRef, slab: &ExtendedEventSlab) {
    // extended event handling
}
```

Nodes that handle extended events call this function from their match arm.
No P0 action required — this is a future optimisation when profiling justifies it.

## Extended event slab

A fixed-size byte slab is pre-allocated per processing cycle on the main thread.
Nodes emitting rich custom events write into it before the cycle via the configurator.
On the audio thread it is read-only. The unsafe pointer arithmetic for reading the slab
is contained in a single `ExtendedEventSlab::read(offset, size) -> &[u8]` function
behind a safe API.

## space_id assignment

space_ids for extended events are negotiated at connection time via the
`ConnectionAgreement` (see ADR-015). Core Paraclete events use the typed enum
variants and do not require space_ids. space_ids are local to a connection —
there is no global registry.

## Consequences

- Nodes match on `event.event` with a standard Rust match expression — fully idiomatic
- All MIDI 2.0 performance events are handled via `Midi2(UmpMessage)` using midi2 crate accessors
- Core non-MIDI events (param locks, transport, tempo) have their own typed variants
- Third-party nodes can define arbitrarily rich custom events via the extended slab
- The audio thread never allocates
- New core event types are added by extending the `Event` enum — this is a breaking
  change to the enum, so the set of core types should be considered carefully before
  stabilising the L2 API
