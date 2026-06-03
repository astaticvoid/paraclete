# ADR-017: Effect Node Architecture

**Date:** May 2026  
**Status:** Accepted

## Decision

Effects are plain `Node` implementations with audio input and output ports.
No dedicated `AudioEffect` trait is introduced. A series effect chain is a
graph topology — nodes connected in series. Wet/dry mixing is a `MixNode`.
Send/return routing is a graph pattern, not a node feature.

An optional `AudioEffect` marker trait is introduced for discovery purposes
only — so the scripting layer and GUI can identify effect nodes. It carries
no required methods.

## Context

Industrial/techno/hardcore requires distortion, saturation, reverb, delay,
chorus, and phaser as first-class components of the signal chain. These need
to arrive before the instrument is musically useful — not at P9 with the
modular graph.

The question is whether effects get a dedicated interface or whether they are
plain nodes with audio ports.

## Option A — Dedicated `AudioEffect` trait

```rust
pub trait AudioEffect: Node {
    fn wet_dry(&self) -> f32;
    fn set_wet_dry(&mut self, value: f32);
    fn bypass(&self) -> bool;
    fn set_bypass(&mut self, value: bool);
}
```

Effects implement this trait. The runtime knows they are effects and can
handle wet/dry and bypass automatically.

**Problem:** Wet/dry and bypass are parameters. They should be lockable by the
sequencer, automatable, and exposed via the state bus like any other parameter.
Putting them on a trait method bypasses the parameter lock system. Every effect
would need special-casing in the sequencer for wet/dry that other parameters
don't need.

## Option B — Plain nodes, wet/dry as parameters (chosen)

A distortion node is just a `Node` with:
- Audio input port
- Audio output port
- `drive` parameter (lockable)
- `wet` parameter (lockable)
- `bypass` parameter (stepped bool, lockable)

Wet/dry mixing happens inside the node using the `wet` parameter. Bypass is a
parameter. The sequencer can lock both per step. The XL can control both.
Nothing special.

A series effect chain:

```
Sampler → Distortion → Filter → Reverb → AudioOutput
```

Each node is a plain `Node`. The chain is a graph topology, not a feature.

## The `AudioEffect` marker trait

For discovery — so Rhai scripts and the GUI can identify effect nodes and
offer them in effect-slot UI — a marker trait with no required methods:

```rust
/// Marker trait. Identifies a node as an audio effect for discovery purposes.
/// Carries no required methods. Declare in CapabilityDocument extensions.
pub trait AudioEffect: Node {}
```

Effects also declare `"paraclete.effect"` in their `CapabilityDocument.extensions`.
The Launchpad profile script can discover effect nodes by scanning for this
extension rather than by querying the trait (which requires downcasting).

## Send/return routing

A send/return is a graph topology:

```
Track → [split] → Dry path → [mix]  → Output
                → Send knob → Reverb → [mix]
```

The `SplitNode` (P5+) copies a signal to multiple outputs. The `MixNode`
combines multiple inputs. These are plain nodes. No special send/return
infrastructure needed.

At P4, send routing is manual — the Rhai profile script wires the graph.
At P9, the modular graph UI makes this visual and interactive.

## First-party effect nodes — shipping order

| Node | Phase | Purpose |
|---|---|---|
| `DistortionNode` | P4 | Drive/saturation. Essential for industrial sound. |
| `FilterNode` | P4 | State-variable filter. Resonant sweep. |
| `DelayNode` | P5 | Tempo-synced delay. |
| `ReverbNode` | P5 | Algorithmic reverb. Freeverb algorithm (MIT). |
| `ChorusNode` | P6 | Shoegaze texture. |
| `PhaserNode` | P6 | Sweep effect. |
| `CompressorNode` | P6 | Dynamics control. |
| `MixNode` | P4 | N inputs → 1 output. Needed for send routing. |
| `SplitNode` | P5 | 1 input → N outputs. Needed for send routing. |

`DistortionNode` and `FilterNode` move to P4 because the instrument is not
musically useful without them. A dry sampler through a clean signal chain
does not sound like industrial techno.

## Consequences

- No new trait required before P4 — effects ship as plain `Node` implementations
- Wet/dry and bypass are parameters, fully integrated with the lock system
- Effect chains are graph topologies — the composability model is exercised
- `AudioEffect` marker trait is additive — can be added without breaking existing nodes
- `DistortionNode` and `FilterNode` ship at P4 alongside the Launchpad profile
- The signal chain at P4: `Sampler → Distortion → Filter → AudioOutput`
- Send routing deferred to P5 when `SplitNode` and `MixNode` ship
