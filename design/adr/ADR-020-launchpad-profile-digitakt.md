# ADR-020: Launchpad X Profile — Digitakt-Inspired Two-Mode Layout

**Date:** May 2026
**Status:** Accepted

## Decision

The Launchpad X profile implements a two-mode surface modelled on the Elektron
Digitakt's step/trigger button layout. The 16 pads in the top two grid rows
are context-sensitive: in trigger mode they represent tracks; in sequence mode
they represent the 16 steps of the selected track. Two scene buttons provide
mode control.

## Layout

```
Pad row 0  [0 ][1 ][2 ][3 ][4 ][5 ][6 ][7 ]  [64 = SEQ EDIT]
Pad row 1  [8 ][9 ][10][11][12][13][14][15]  [65 = SHIFT   ]
Rows 2–7   (dark — reserved for future expansion)
Top buttons  unused in this profile
```

## Trigger Mode (default — SEQ EDIT off)

Pads 0–7 each represent one track (track index = pad index).
Pads 8–15 are dark (placeholder for tracks 8–15, not yet implemented).

**Pad colours:**
- Track with active steps: dim amber (`#302000`)
- Track with no active steps: off
- Recently triggered (within ~150 ms): bright white flash

**Interactions:**
- Press pad 0–7 → immediately trigger that track's sound (CMD_TRIGGER to Sampler)
- SHIFT + press pad 0–7 → select that track and switch to sequence mode
- Press SEQ EDIT (64) → switch to sequence mode (retains last selected track)

## Sequence Mode (SEQ EDIT lit)

The 16 pads display the full 16-step pattern of the selected track.

**Pad colours:**
- Active step: blue (`#0040FF`)
- Inactive step: dim (`#080808`)
- Current playback position: green (bank 0, steps 0–7) or cyan (bank 1, steps 8–15)

**Interactions:**
- Press any pad → toggle that step on/off (sends CMD_TOGGLE_STEP)
- SHIFT held + press pad 0–7 → select that track without toggling a step
- Press SEQ EDIT (64) → return to trigger mode

## SHIFT Modifier (ID 65)

Held to redirect pad presses to track selection in both modes.
In trigger mode, SHIFT + pad automatically enters sequence mode for that track.
In sequence mode, SHIFT + pad switches the displayed track without toggling.

## Visual indicator — selected track

In sequence mode, the SHIFT button (65) shows a dim colour corresponding to
the selected track index. A brief hold of SHIFT lights pads 0–7 in track-
select colours (selected = white, others = dim amber/off) to confirm which
track is being edited.

## Live trigger flash

In trigger mode, a pad flashes white when its track fires. Implemented via
a `last_trig` counter published to `/node/{id}/state/last_trig` by the
Sequencer on each NoteOn emission. `state_alive` with a 150 ms window drives
the flash from the `current_step` subscription callback.

## CMD_TRIGGER

`CMD_TRIGGER = 19` (node-specific, above universal 0–15) on the Sampler node.
Received via NodeCommand; plays the default note at full velocity immediately.
Allows the scripting layer to audition tracks in trigger mode without routing
through the sequencer.

## Multiple templates

This profile is one template for the Launchpad X. A future overview template
(all 8 tracks × 8 steps simultaneously) can be loaded via `reload_profile`.
A dedicated button or long-press gesture switches between templates.

## Rationale

Replicating Elektron muscle memory during the development phase provides
immediate usability without requiring UX design work for a novel interface.
The architecture supports richer grid layouts (split-view, parameter control)
as later templates once the core sequencing workflow is validated.

## Consequences

- `Sequencer::published_state()` gains `/node/{id}/state/last_trig`
- `Sampler` gains `CMD_TRIGGER = 19`
- `profiles/launchpad.rhai` is a full rewrite; the prior multi-track overview
  profile should be saved as `profiles/launchpad_overview.rhai` before overwrite
