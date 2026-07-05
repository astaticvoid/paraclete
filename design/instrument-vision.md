# Paraclete — Instrument Vision

> **Reference document.** This is the concrete instrument being built on the
> Paraclete platform. Every design decision that involves a tradeoff between
> generality and specificity uses this document as the tiebreaker.
>
> This document does not change the architecture. It anchors it.

---

## The Instrument

A dawless hardware instrument for live performance and studio production of
dark, aggressive, rhythmically complex electronic music — from the cold,
distorted machine-funk of classic electronic body music and industrial to the
hypnotic, cavernous, texture-driven palette of modern industrial techno. Tight
and mechanical, but alive. Performance-centric: the instrument responds to the
performer in real time, not playback of a pre-composed arrangement.

The instrument is Paraclete running on a laptop connected to hardware
controllers. No DAW. No mouse. The laptop is the whole instrument — sound
sources, sequencer, and effects — not a workstation.

It is a **full instrument, not just a sequencer.** Four pillars:

1. **Sequencing** — an Elektron-style sequencer threaded through every
   parameter. The heart of the instrument (detailed below).
2. **Synthesis** — a full subtractive analog voice, an ergonomic FM voice, and
   analog / FM / sampler drum modules.
3. **Effects** — a 70s/80s-flavored palette: characterful distortion,
   modulation, delay, and reverb, every one a node.
4. **Performance** — macros, mutes, fills, pattern switching, and live
   recording, all mouse-free from grids, keys, and encoders.

The goal is to occupy the role a DAW plays in a hardware-style live rig — the
single tool you compose and perform on — without being a linear audio
workstation. There is no audio-recording timeline and no mixdown stage:
arrangement *is* performance (pattern chaining), and anything outside the native
palette is hosted as a CLAP plugin rather than re-implemented.

---

## The Hardware

**Novation Launchpad (8×8 RGB grid)**
The primary sequencer and drum machine interface. 8 tracks visible
simultaneously. 8 steps per row per page (the DT2 has 16 trig keys per page; we adapt to
the Launchpad's 8-pad rows); scene buttons select page (up to 8 pages =
64 steps per track). In grid-edit mode, hold the page button and press step
keys to select which pages the pattern loops — so you can build a 4-page
pattern and loop only pages 1–2 while composing, then open it up live.
Step color encodes trig type: note trig, lock trig (param lock, no note),
and conditional trig each have a distinct color; locked steps blink.
Current step indicator sweeps the row during playback. Track selection,
mute (pattern-level and global), pattern switching, fill mode activation,
temporary save/reload — all from the grid without touching a mouse.

**Encoder controller (true relative / endless)**
Contextual parameter control. When a track is selected on the Launchpad, the
encoders remap to that track's instrument parameters. Kick selected → encoders
are pitch, decay, distortion, filter. Hat selected → encoders are pitch, length,
filter, reverb send. Macro pages of filters/effects switch the same encoders to
new targets. The controller follows the Launchpad's context automatically via
Rhai profile script.

This **requires true relative / endless encoders** — the same physical encoders
constantly remap across tracks, views, and macro pages, so absolute pots are
disqualified (every switch leaves the pot wrong; soft-takeover is unacceptable).
The Launch Control XL line is ruled out for this reason (Mk2 = absolute pots;
Mk3 = absolute CC/NRPN in custom modes). The target is a true-relative box
(e.g. Intech Grid EN16 Smooth, MIDI Fighter Twister, or OXI E16). Paraclete
consumes relative deltas via `CMD_BUMP_PARAM`, so the remap is seamless and the
controller is a swappable commodity. See the `controller-strategy` memory and
roadmap P9.5.

**Arturia Keystep 37**
Melodic and bassline input. Plays a synth or sampler track that sits over
the drum machine. Live keyboard playing, arpeggiation, and the Keystep's
own internal sequencer for independent melodic patterns. Standard MIDI over
USB — no special protocol needed.

---

## The Voices

Sound generation is four families of node — each a self-contained instrument the
sequencer drives, the parameter-lock system automates per step, and the macro
layer exposes to the encoders. The drum modules exist today; the melodic voices
are the largest unbuilt part of the instrument.

**Analog synth voice — full subtractive mono.**
A complete monophonic subtractive synthesizer modeled on the classic Sequential
Pro-One feature set, composed from Paraclete primitives:

- Two oscillators with multiple waveshapes (saw, triangle, pulse) and
  pulse-width control; hard sync; the second oscillator doubles as a modulation
  source at low rates
- A noise source in the mixer
- A self-oscillating four-pole (24 dB/oct) lowpass filter with key tracking and
  envelope amount
- Two envelopes (filter, amplifier) and a dedicated LFO
- A modulation routing section (the Pro-One's "Direct" mod section) sending
  envelope / LFO / oscillator-B to oscillator pitch, pulse-width, and filter
  cutoff — faithful to the hardware's routing, not a general patch matrix
- Glide (portamento) and an integrated arpeggiator / step sequencer

This is the lead and bassline engine — the driving, resonant, aggressive
monophonic core. "Full features" means the real instrument's capability set, not
a reduced approximation.

**FM voice — simplified, ergonomic, melodic.**
A four-operator FM synth with a fixed set of algorithms, designed for fast
sound-shaping from a handful of macro controls rather than per-operator menu
diving. Metallic tones, bells, hard digital basses, detuned stacks. The
ergonomic counterpart to the analog voice, not a clone of the FM drum engine.

**Drum modules — analog, FM, sampler.**
The percussion engines already at the core: an analog-style drum synth (kick,
snare, hat machines), an FM percussion engine, and a sampler with high-quality
pitch resampling. Each exposes its parameters to per-step locks and to the macro
layer for hands-on performance.

The analog drum synth's destination is **machine breadth**, in the spirit of
Elektron's Analog Rytm: a *family* of machine algorithms per drum type —
several kicks (hard / classic / snappy), several snares, toms, claps,
hats — selectable per track, not one fixed model each. The current
one-machine-per-type engine (July 2026) is the seed, not the scope. Layering
an analog voice with a sample on the same track is already graph topology
(ADR-016), not an engine feature. Mutable Instruments peaks/plaits drum
models (MIT) are the sanctioned algorithm source for the family.

**Paraphony / polyphony.**
The synth voices are monophonic by default — correct for basslines and leads.
Pads, drones, and the slow atmospheric chords of the modern palette need at
least paraphonic voice allocation. That is a first-class goal, not an
afterthought: the cavernous, evolving side of the sound depends on it.

---

## The Sequencer

The sequencer is modeled on the Digitakt II, adapted for 8 tracks and the
Launchpad grid. These are the target capabilities, phased across P5–P6.

**Pattern structure**
- Up to 64 steps per track (8 pages × 8 steps on the Launchpad grid)
- Per-track independent length and speed multiplier (1/8×–2×)
- Per-pattern or per-track mode; tracks can loop at different lengths (polyrhythm)
- Page loop selection: choose which subset of a multi-page pattern loops during
  playback — build a 4-page pattern, loop pages 1–2 while composing, open to
  all 4 pages live for a drop
- Pattern change is seamless: cue the next pattern while the current one plays;
  it switches at the end of the current cycle

**Trig types**
- **Note trig** — fires a note at that step (default; shown in blue when active)
- **Lock trig** — applies parameter locks at that step without firing a note
  (shown in yellow; key technique for subtle automation)
- Steps with parameter locks blink to distinguish them from unlocked steps

**Trig conditions** — each step can have one condition that gates whether it fires:

| Condition | Meaning |
|-----------|---------|
| `A:B` | Fire on the A-th pass out of every B (e.g. 1:2 = every other loop) |
| `PRE` / `!PRE` | Fire if the previous conditional on this track fired (or didn't) |
| `NEI` / `!NEI` | Fire if the previous conditional on the neighbor track fired (or didn't) |
| `1ST` / `!1ST` | Only the first loop / all loops except the first |
| `LST` / `!LST` | Only the last loop before a pattern change |
| `FILL` | Only when fill mode is active |
| `PROB` | Per-step probability 0–100%; re-rolled on every pass |

Conditions and probability are independent and can be combined.

**Fill mode** — a temporary one-cycle variation layer. Hold the fill button for
an ad-hoc fill; latch it on for a full cycle; release to return. Trigs marked
FILL=ON only fire during fill; FILL=OFF trigs fire only outside fill. This lets
you bake an alternate pattern into the same track without a separate pattern slot.

**Mute system** — two independent mute contexts:
- *Pattern mute* — per-pattern mute state, saved with the pattern (magenta keys)
- *Global mute* — cross-pattern mute state, saved with the project (green keys)
Prepared mute: stage mute changes while holding a modifier; execute atomically on
release. This is how you build tension and drop in live performance.

**Pattern chaining** — chain up to 8 patterns in sequence for song-level structure
without entering a song editor. Volatile: a chain is a performance gesture, not
a saved arrangement.

**Euclidean mode** — two independent pulse generators with boolean combination
(OR/XOR/AND/SUB) and per-generator rotation. Generates rhythmically interesting
patterns from a count-and-rotate algorithm. Can be converted to regular trigs for
further manual editing.

**Micro-timing** — shift individual trigs ahead or behind the grid at sub-step
resolution. Not quantization looseness; deliberate groove placement.

**Swing** — applied to even 16th notes of each beat, 51–80%. Per-pattern.

**Retrig** — fire the voice multiple times within a single step at a configurable
rate (1/2 through 1/80) with optional velocity fade.

**Temporary save / reload** — a volatile snapshot of the current pattern state.
Save before a risky live tweak; reload to revert if it goes wrong. Not
auto-save, not persistent across power cycles — purely a live performance
safety net. The equivalent of a quick-save in a video game: you take risks
you wouldn't take without it.

**Perform Kit mode** — decouple parameter tweaks from persistent storage during
a live set. Parameter changes are not auto-saved, and when you switch patterns
the current tweaked state is *retained* rather than replaced by the new
pattern's saved state. This lets you build a continuously evolving sound
across multiple pattern changes without accidental resets. Reload the original
kit at any moment, or commit the changes permanently if you like where you
landed.

---

## A Session

You sit down. The three devices are connected. Paraclete starts.

The Launchpad shows 8 rows — one per track. Rows are dim. The InternalClock
is at 140 BPM.

You press pads in row 0 (kick track) to set steps. Active steps glow. You
press play. The current step indicator sweeps across row 0. The kick fires.

You select row 1 (snare). Press steps. The Digitakt encoders shift context —
they now control the snare's distortion drive, filter cutoff, and resonance.
You turn the cutoff encoder. The snare tightens. Kick and snare together.

You build up rows 2–5 (hats, percussion). The pattern plays. You press a
scene button — page 2. The row now shows steps 9–16 for all 8 tracks. You
add variation. The kick track now has 16 steps of character.

You want the hat track to run twice as fast. You set that track's speed to 2×.
The hat plays 32nd notes while the kick plays 16ths. The grid still shows 8
steps per page for the hat — the track's internal clock doubles them.

You hold a step pad on the kick and turn a Digitakt encoder — a parameter
lock. That step plays with extra drive. Beat 3 hits different from the others.
You add a lock trig on step 7: filter closes, no note fires — a quiet moment
that makes step 8 feel harder. The step blinks yellow to show it's a lock trig.

You set step 5's condition to 2:4 — it fires every other loop. The groove
breathes. You set step 13 on the hat to PROB 50% — it fires half the time.
The pattern is alive, not mechanical.

You hold the fill button. A variation layer activates for one cycle. FILL=ON
trigs — a crash, a roll — fire. FILL=OFF trigs drop out. Release. The pattern
returns to its base state. The transition was a single gesture.

Before you start tweaking in earnest, you hit temporary save — a single
gesture that snapshots the pattern state. You experiment freely. Something
goes wrong. You reload. You're back. The safety net made the risk possible.

You turn on Perform Kit mode. Now your encoder tweaks are live but unsaved.
You switch to pattern B — the drum machine changes, but your distortion and
filter settings from pattern A carry over. The sound evolves continuously
rather than snapping back to a saved preset. When you land somewhere good,
you commit it.

You cue pattern C on the Launchpad. It blinks, waiting. At the end of the
current loop it switches. You chain A → B → C — verse, build, drop.

You pick up the Keystep. Row 7 (bass track) is a sampler. You play a bassline
over the running pattern. The notes go into the sequencer. The Keystep's
arpeggiator locks a melodic loop into the pattern.

At no point have you touched a mouse.

---

## The Sound

The span runs from the cold, distorted, sequenced machine-funk of classic
electronic body music and industrial to the hypnotic, cavernous, texture-driven
palette of modern industrial techno. Two ends of one lineage. Specific
characteristics:

- **Distortion everywhere** — kicks saturated, hats clipped, basslines driven.
  Not clean. The signal chain has grit at every stage.
- **Pedal-chain aesthetic** — reverb, delay, chorus, phaser as modular effect
  nodes chained in series. Wet, washed, textured; from tight slapback to
  bottomless dub-techno space.
- **Tight and aggressive rhythm** — the sequencer is precise but not
  quantised-feeling. Micro-timing, swing, conditional trigs create movement
  within the grid without looseness.
- **Controlled randomness** — not chaotic but alive. Steps that fire sometimes.
  Parameters that drift slightly. The pattern is a probability distribution,
  not a fixed sequence.
- **Slow evolution and atmosphere** — drones, pads, and reverb tails that
  develop over many bars under the rhythm. Hypnotic repetition, not constant
  novelty.
- **Dynamic performance** — the instrument responds to the performer. Muting
  tracks, switching patterns, live parameter tweaking, real-time recording.
  The music changes as you play.

**The effects palette.** The character lives as much in the effects as in the
voices, and targets a 70s/80s analog flavor with a dub-techno spatial
sensibility:

- **Saturation and distortion in variety** — soft tube-style warmth, hard diode
  clipping, wavefolding, and bit / sample-rate reduction
- **Modulation effects** — chorus, phaser, and flanger (bucket-brigade flavored)
- **Time effects with character** — bucket-brigade (BBD) analog delay and tape
  echo with feedback coloration; spring and plate reverb alongside a clean
  algorithmic hall for cavernous, evolving space

Every effect is a node; wet/dry and bypass are sequenceable parameters, so the
whole chain is performable and per-step lockable.

---

## What "Most Powerful Sequencer Integrated at All Levels" Means

The sequencer is not just a note trigger. It threads through the whole system:

- **Parameter locks reach any node** — any published parameter on any connected
  node is lockable per step. Distortion drive, reverb size, filter cutoff —
  all sequenceable per step. Lock trigs apply locks without firing a note,
  enabling pure automation steps.

- **Conditional trigs create structure without copies** — A:B, PRE/NEI
  cross-track dependencies, 1ST/LST loop-awareness, FILL, and PROB let a
  single 16-step pattern generate weeks of variation without duplicating
  patterns or writing generative code.

- **Polyrhythm is topology** — per-track length and speed multipliers mean
  8 tracks can run 8 independent loop lengths simultaneously. No special
  "polyrhythm mode" — it falls out of the per-track parameters.

- **Fill mode is a composition tool** — FILL=ON trigs baked into the same
  pattern as the main loop. One button activates the variation layer; release
  restores the base. Live performance expression with zero setup.

- **Page loop selection narrows the window** — a 64-step pattern can loop only
  its first 16 steps while you build, then open up to the full length live.
  Complexity is revealed, not reset.

- **Temporary save makes risk free** — snapshot before a risky tweak; revert
  if it goes wrong. The instrument responds to boldness because the safety net
  is always one gesture away.

- **Perform Kit mode sustains evolution** — parameter state persists across
  pattern changes. The sound of the set evolves continuously; no saved state
  intrudes unless you want it to.

- **The sequencer as a CV source** — step values as signal outputs. A sequence
  of pitch values drives a filter cutoff at audio rate. The pattern is also
  a modulation source. (P9)

- **Nested sequencers** — a sequencer driving another sequencer's parameters.
  Step length modulated by a second pattern. Conditional trigs based on the
  state of another track. Emergent complexity from simple rules. (P9)

- **Scripted behaviour** — Rhai scripts that read sequencer state and modify
  it in response to performance input. Algorithmic variation. Generative
  fills. Pattern mutation under performer control. (P4+)

Phase targets: parameter locks (P3); conditional trigs + micro-timing + swing +
fill (P5); CV source + nested sequencers (P9); multi-pattern + pages +
polyrhythm + chaining (P10); mute + perform kit + live record (P11); retrig +
euclidean (P12). The architecture supports all of them from day one.

---

## Platform vs Instrument

The instrument described here is built on the Paraclete platform. The platform
is designed for others to build on — the node API is LGPL3, the HAL supports
any controller, the scripting layer allows custom profiles without touching
Rust.

When there is a tradeoff between making the platform more general and making
this instrument better, the instrument wins until it is playable as the full
instrument described here. Once the architecture is proven and the instrument
stands on its own, the platform gets the investment it needs to be genuinely open.

The instrument is the proof that the platform works. If you can make industrial
techno with it without touching a mouse, the platform succeeded.
