# ADR-026: Declarative Instrument Definition and Terminal UI

**Date:** June 2026
**Status:** Accepted — P8 design target

-----

## Decision

Instrument graphs are defined in a human-readable text file (YAML or
equivalent). The terminal interface is the primary feedback surface for
the dawless instrument — prioritised over any graphical UI. The terminal
renders Elektron-style live feedback: active parameter page, encoder-to-
parameter mappings, and values as they change.

-----

## Context

The current model requires graph topology to be written in Rust inside
`paraclete-app`. Instrument definition, macro assignments, and hardware
mappings are split across Rust startup code and Rhai profile scripts.
There is no feedback surface beyond the hardware LEDs driven by those
scripts.

Paraclete is a hardware-first dawless instrument. The operator interacts
with physical controllers, not a mouse. The feedback they need — what
parameter they are editing, what value it is at, what the sequencer is
doing — should be visible in a terminal at all times. This is the model
Elektron hardware uses on its own screen; the terminal is that screen.

-----

## The Two Decisions

### 1 — Declarative instrument definition

A text file (YAML as the reference format; exact format resolved at P8
design time) declares the graph topology, initial parameter values, active
profile scripts, and macro assignments. The runtime reads this file at
startup and constructs the `NodeConfigurator` from it. Node IDs are stable
(assigned by declaration order or explicitly in the file).

This is a startup-only interpretation of topology-from-file. The graph is
fixed after load; runtime topology changes remain a P9+ concern. The RON
project file (ADR-025) continues to capture runtime state deltas on top
of whatever the instrument file establishes.

The instrument file is the thing an operator edits to change their
instrument. It should be readable and writable without knowledge of Rust.

Macro assignments belong in the instrument file, not buried in Rhai scripts.
A macro is a declared binding: encoder N controls parameter P on node X.
Declaring macros in the instrument file makes them visible to the terminal
UI without parsing Rhai.

### 2 — Terminal UI as primary feedback surface

A new crate (`paraclete-tui`) renders the instrument state to a terminal.
It is a subscriber to the state bus. It runs on the main thread alongside
the existing application loop, redrawing at a rate appropriate for a live
performance tool (~30 Hz).

The display is page-based, following the active hardware context. The
reference model is an Elektron instrument screen:

- Transport bar: BPM, playing/stopped, current step position
- Track header: active track name and engine type
- Encoder row: up to 8 encoder slots, each showing parameter name,
  current value, and unit; the slot that most recently changed is
  highlighted briefly
- Step row: 16-step view of the active sequencer, showing trig state
  and the current playback position

The terminal UI is not a developer tool. It is the instrument's display.

-----

## Profile Script Context Publication

The terminal UI needs to know which parameters the encoders are currently
mapped to. This context lives in the Rhai profile scripts (the scripts
decide, per hardware event, which encoder controls which parameter). The
clean path: profile scripts call a `publish_context()` scripting API
function whenever they assign or reassign an encoder mapping. The TUI
reads the published context from the state bus.

This is a small addition to the scripting layer API. It should be
added before the profile scripts are written for P8, so that scripts
are authored with context publication as a first-class responsibility
from the start.

```rhai
// In a profile script, when encoder 2 is assigned to decay on node 5:
publish_context("encoder_2", node_id, "decay");
send_cmd(node_id, CMD_SET_PARAM, id_for_name("decay"), value);
```

The state bus path for context is `/context/encoder_{n}/node` and
`/context/encoder_{n}/param`. The TUI subscribes to these paths and
uses them to label the encoder row.

-----

## What Is Not Decided Here

The exact YAML schema, the TUI library choice, the full terminal layout,
and the instrument file load path are all resolved at P8 design time.
This ADR captures intent and establishes constraints; the P8 interface
spec provides the full implementation contract.

-----

## Consequences

**P8:**
- `paraclete-tui` crate created
- Instrument file format specced and a YAML (or equivalent) parser
  integrated into `paraclete-app`
- `publish_context()` added to the Rhai scripting API
- Profile scripts updated to call `publish_context()` on encoder
  assignment
- ADR-025 amended to clarify the relationship between the instrument
  file (topology + initial params) and the project file (runtime state)
- `paraclete-app` startup sequence changed: load instrument file →
  build graph → load project state → start audio engine → start TUI

**Ongoing:**
- All future profile scripts are written with `publish_context()` as
  a standard call alongside encoder assignment
- The terminal display is the reference for operator feedback decisions.
  If a piece of information matters to the operator, it appears in the
  terminal before it appears anywhere else

-----

## References

- ADR-010 — State Model (state bus as inter-layer communication)
- ADR-018 — Cellular Architecture (hardware reaches any declared parameter)
- ADR-019 — Universal Parameter Control (canonical parameter names)
- ADR-025 — Project File Format (runtime state; topology boundary)
- `instrument-vision.md` — the concrete instrument being built; the
  operator's experience is the design tiebreaker
