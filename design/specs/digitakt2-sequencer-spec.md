# Digitakt II Sequencer — Standalone Specification

*Extracted and isolated from the Elektron Digitakt II User Manual (OS 1.15A). This document describes the sequencer as an independent system, abstracting away all sound synthesis, sampling, and audio processing concerns. References to "tracks" are generic sequencer tracks; references to "presets" or "samples" are treated as abstract voice/payload identifiers that the sequencer addresses.*

---

## 1. Data Model

### 1.1 Hierarchy

```
Project
  └── Songs (up to 16)
  └── Patterns (128 total — 8 banks × 16 patterns each)
        └── Tracks (16 per pattern)
              └── Trigs (up to 128 steps per track)
                    └── Parameter Locks (up to 80 distinct locked params per pattern)
```

### 1.2 Pattern

A pattern is the primary sequencer container. Each pattern holds:

- Trig data (note trigs and lock trigs) for all 16 tracks
- Per-trig parameter locks and conditional locks
- Per-track or per-pattern length and time signature settings
- BPM and swing settings
- Quantization settings
- Mute states (pattern-level)

128 patterns are available per project (banks A–H, 16 patterns each).

### 1.3 Track

Each of the 16 tracks is an independent sequencer lane with:

- Up to 128 steps across up to 8 pages (16 steps per page)
- Its own length, speed multiplier, and time signature (in Per Track mode)
- Its own trig data and parameter locks
- Micro timing per step
- Retrig settings per step (audio tracks only)

Tracks can be designated as either **audio tracks** or **MIDI tracks**. The sequencer logic is identical; the output destination differs.

### 1.4 Trig

A trig is a sequencer event. There are two types:

| Type | Description | Visual indicator |
|---|---|---|
| **Note Trig** | Fires a voice/payload | Red key |
| **Lock Trig** | Applies parameter locks without firing a note | Yellow key |

Unlit step positions contain no trig.

---

## 2. Transport and Playback

### 2.1 Controls

| Action | Description |
|---|---|
| PLAY | Start playback from current position |
| PLAY (while playing) | Pause playback |
| PLAY (while paused) | Resume playback |
| STOP | Stop all tracks; send effects tail out naturally |
| STOP + STOP | Stop all tracks with short fade-out of send effects |

### 2.2 Pattern Sequencing

Patterns switch **seamlessly** — a new pattern can be cued while the current one plays; it takes effect at the end of the current pattern's cycle. The queued pattern blinks in the UI until it activates.

Patterns can be changed and queued via MIDI Program Change messages (0–127 → patterns 1–128, i.e. A01–H16).

### 2.3 Pattern Pages and the Playhead

When a pattern has more than 16 steps, **Pattern Page LEDs** (up to 8) indicate:
- How many pages the pattern spans
- Which page is currently playing (flashing LED)

A light "runs" along the 16 trig keys in sync with playback across all pages at the set tempo. This provides a live visual of the playhead position.

---

## 3. Tempo and Timing

### 3.1 Tempo

- Range: sub-1 BPM to high BPM (integer + decimal steps)
- Coarse adjustment: DATA ENTRY knob A (hold+turn = 4 BPM steps)
- Fine adjustment: DATA ENTRY knob B (decimal steps)
- **Tap Tempo**: hold FUNC + tap TEMPO key in rhythm; averages after 4 taps, continuously updates
- **Nudge**: hold LEFT or RIGHT arrow from main screen to nudge tempo ±10% temporarily; release to revert
- **Prepared tempo** (project mode): hold FUNC while turning knob A; new tempo does not take effect until FUNC is released ("PREP." flashes on screen)
- **Tempo scope**: switchable between **project-global** (one BPM for all patterns) and **per-pattern**

### 3.2 Swing

- Range: 51%–80% (default 50% = straight)
- Set per-pattern via the TEMPO menu (DATA ENTRY knob D)
- Applied to the even 16th notes of each beat, creating a groove

### 3.3 Metronome

| Parameter | Description |
|---|---|
| METRO | On/off toggle (also: TEMPO + YES) |
| SIG | Time signature for the click (note value and beat count) |
| PREROLL | Number of bars the metronome plays before sequencer starts (in LIVE RECORDING only) |
| VOL | Click volume |

### 3.4 External Sync

The Digitakt II can receive and transmit MIDI clock and DIN sync. Sync source and direction are configured in the MIDI/SYNC settings. Tempo can also be tapped via an external MIDI controller sending note C0 on the designated FX CONTROL MIDI channel.

---

## 4. Pattern Structure and Length

### 4.1 Per Pattern Mode

All 16 tracks share the same length and time signature.

| Parameter | Description |
|---|---|
| LENGTH | Number of steps (1–128, spread across up to 8 pages of 16) |
| SPEED | Playback rate multiplier: 1/8X, 1/4X, 1/2X, 3/4X, 1X, 3/2X, 2X |

When a pattern is extended (e.g. from 2 to 4 pages), new pages are automatically populated as copies of the existing pages.

**Speed use cases:**
- **2X**: Effectively doubles step resolution to 32nd notes
- **3/4X**: Triplet feel when synchronized with other instruments at the same BPM

### 4.2 Per Track Mode

Each track can have an independent length and speed. Two parameter columns are available:

**TRACK column** (affects the active track only):
| Parameter | Description |
|---|---|
| LENGTH | Steps for this track (1–128) |
| SPEED | Playback rate multiplier for this track |

**PATTERN column** (affects pattern-level behavior):
| Parameter | Description |
|---|---|
| CHANGE | How long the active pattern plays before switching to a cued/chained pattern. Default: end of pattern. |
| RESET | How many steps the pattern plays before all tracks reset and restart from step 1. INF = tracks loop independently forever without resetting. |

The CHANGE parameter overrides RESET when its value is less than the RESET value.

**Track length increment**: FUNC + UP/DOWN in the PAGE SETUP menu sets track length in steps from 2/16 to 128/128.

---

## 5. Recording Modes

The sequencer offers three recording modes for entering trigs.

### 5.1 Grid Recording Mode

Step-based, non-real-time composition on a visible 16-step grid.

**Entering the mode**: Press RECORD (key lights solid red)

**Operations**:
- Press a TRIG key → add a Note Trig at that step
- Press FUNC + TRIG key → add a Lock Trig at that step
- Quick-press an existing trig → remove it
- Hold a trig key slightly longer → enter edit state (without removing)
- Press RECORD again → exit mode

**Multi-page editing**: Press PAGE to switch the visible/editable page. Press and hold PAGE + TRIG 1–8 to jump directly to a page.

**Page loop selection**: In Grid Recording mode, hold PAGE + press TRIG 9–16 to select which pages the sequencer should loop during playback. Green keys show selected pages. Trig 9–16 correspond to pages 1–8.

**Trig shift**: FUNC + LEFT/RIGHT shifts all trigs on the active track forward or backward on the timeline.

**External MIDI input in Grid Recording**: Press and hold a TRIG key, then play a note on the external keyboard to set that trig's note and velocity. MIDI tracks accept up to 4-note chords per trig.

**Trig preview**: TRIG + YES previews a specific trig including all its parameter locks.

### 5.2 Live Recording Mode

Real-time input; the sequencer plays while trigs are entered.

**Entering the mode**: Hold RECORD + press PLAY (RECORD key flashes red)

**Auto-quantization**: Press PLAY a second time while holding RECORD to toggle auto-quantization on/off during entry.

**Operations**:
- Press TRIG keys in real time → note trigs recorded at current playhead position
- In KEYBOARD mode: pitch is determined by which TRIG key is pressed
- Turning DATA ENTRY knobs → recorded as parameter locks; lock trigs are created where no note trig exists
- Press NO + one or more TRIG keys → erase steps in real time as the sequencer passes them
- Hold NO + press DATA ENTRY knob → erase a specific parameter lock from all steps in real time

**External MIDI in Live Recording**: Notes played on a connected keyboard record NOTE, VELOCITY, and LENGTH (length = duration of key press). MIDI tracks accept up to 4-note chords.

**Exiting**: Press PLAY (sequencer keeps running); press STOP (stops recording and playback); press RECORD (switches to Grid Recording mode).

**Post-recording quantization**: Recorded trigs can be quantized retroactively via the QUANTIZE MENU (FUNC + TRIG PARAMETERS).

### 5.3 Step Recording Mode

Semi-automatic step advancement; each note input advances the cursor to the next step.

**Entering the mode**: Press RECORD + STOP (RECORD key double-blinks red)

**Two sub-modes** (toggle with STOP while holding RECORD):

**STANDARD mode**:
- Press a TRIG key to position the cursor at a step (green double-blinking key)
- Hold FUNC + press TRIG 1–16 → add a note trig to the current step on the corresponding track; cursor advances
- Use LEFT/RIGHT to move cursor or skip steps
- Press NO at a step → remove trig or add a rest; cursor advances
- Chromatic input: toggle KEYBOARD mode, then hold KEYBOARD + press a lit TRIG key → records that pitch at the current step

**JUMP mode**:
- The LEN parameter (TRIG PAGE 1) controls note length
- Each trig added advances the cursor by the note length distance (1/16 advances 1 step, 1/8 advances 2 steps, etc.)
- LEN is automatically parameter-locked on every trig placed in this mode

**Common Step Recording behaviors**:
- PLAY while in Step Recording → listen without exiting; STOP to stop without exiting
- External MIDI input is supported; velocity is captured and parameter-locked
- Hold YES while placing a trig → trig length is locked to the duration the key is held
- Adding a note trig over an existing one preserves existing parameter locks

---

## 6. Trig Parameters

### 6.1 Trig Page 1 — Note Properties

These are the **default** values for all trigs on a track. Any individual trig can override them via parameter locks.

| Parameter | Description |
|---|---|
| NOTE | Pitch of the note (C0–G10). Press+turn to snap to scale notes. |
| VEL | Velocity of the trig (0–127) |
| LEN | Length of the note trig (fractional, rational, and integer multiples of a step) |
| PROB | Probability the trig fires (0–100%). Re-evaluated on every pass. Can be locked per-trig. |
| LFO.T | Whether a trig fires the LFO |
| FLT.T | Whether a trig fires the filter envelope |
| FILL | Trig is conditioned on FILL mode state (ON = plays when FILL active; OFF = plays when FILL inactive) |
| COND | Trig Condition for conditional playback logic (see Section 8) |

### 6.2 Trig Page 2 — Retrig and Portamento

Retrigs are available on audio tracks only; not available on MIDI tracks.

| Parameter | Description |
|---|---|
| RTRG | Retrig on/off for this track's default trig behavior |
| VFAD | Velocity fade across the retrig sequence (-64 to +64; negative = fade out, positive = fade in) |
| LEN | Duration of the retrig velocity envelope in step fractions (0.125–INF) |
| RATE | Retrig rate: 1/1, 1/2, 1/3, 1/4, 1/5, 1/6, 1/8, 1/10, 1/12, 1/16, 1/20, 1/24, 1/32, 1/40, 1/48, 1/64, 1/80 |
| PTIM | Portamento time |
| PORT | Portamento on/off |

**Retrig customization per step**: In Grid Recording mode, navigate to Trig Page 2, then hold one or more TRIG keys to set retrig options for those specific steps.

**Retrig rates via TRIG mode**: In RETRIGS trig mode, the 16 TRIG keys each trigger the active track with a specific retrig rate (1/1 through 1/80).

---

## 7. Micro Timing

Micro timing shifts an individual trig ahead or behind the beat at sub-step resolution.

**In Grid Recording mode**: Hold one or more TRIG keys, then press LEFT or RIGHT to access the micro timing pop-up. The offset for the selected step(s) is shown and adjustable with LEFT/RIGHT. Release the TRIG key(s) to exit.

Micro timing applies to both audio and MIDI tracks. Settings are stored per-step in the active pattern.

---

## 8. Trig Conditions and Conditional Locks

A conditional lock is a parameter lock applied to the COND parameter on any trig. It defines a logical rule determining whether the trig fires on a given pass.

### 8.1 Adding a Conditional Lock

1. In Grid or Step Recording mode, place a trig
2. Hold the trig key to access the COND parameter on Trig Page 1
3. Turn DATA ENTRY knob H to select a condition

### 8.2 Available Conditions

| Condition | Description |
|---|---|
| PRE | True if the most recently evaluated conditional trig on this track was true (PRE itself is not counted) |
| !PRE | True when PRE is false |
| NEI | True if the most recently evaluated conditional trig on the **neighbor track** (track N-1) was true |
| !NEI | True when NEI is false |
| 1ST | True only on the first loop of the pattern |
| !1ST | True on all loops except the first |
| LST | True on the last loop before a pattern change |
| !LST | True on all loops except the last before a pattern change |
| A:B | True on the A-th pass out of every B passes (cycle repeats indefinitely) |
| !A:B | True when A:B is false |
| FILL | True when FILL mode is active (separate from COND; set on TRIG PAGE 1) |

**A:B examples**:
- 1:2 → fires on passes 1, 3, 5, 7, …
- 2:2 → fires on passes 2, 4, 6, 8, …
- 2:4 → fires on passes 2, 6, 10, …
- 4:7 → fires on passes 4, 11, 18, …

The per-trig PROB (probability) parameter is distinct from COND and can be combined with it.

---

## 9. Parameter Locks

Parameter locks allow individual trigs to override any track parameter value at that specific step.

### 9.1 Scope and Limits

- Any parameter on the TRIG, SRC, FLTR, AMP, FX, or MOD pages can be locked
- **Up to 80 distinct parameters** can be locked per pattern
- A parameter counts as one locked parameter regardless of how many trigs lock it (e.g. locking filter cutoff on all 128 steps still only consumes 1 of the 80 slots)

### 9.2 Adding Locks in Grid Recording

1. Press RECORD to enter Grid Recording mode
2. Place or locate a trig; hold the trig key
3. Turn the DATA ENTRY knob for the parameter to lock
4. The parameter display inverts to indicate a lock is present
5. The trig key blinks red (note trig) or yellow (lock trig) to indicate locked state

**Visual indicator**: Holding a locked trig in Grid Recording causes the PARAMETER page keys to light red, showing which pages contain locks.

**Bulk lock**: Hold TRIG + PAGE (or TRK), then turn a DATA ENTRY knob → applies a parameter lock to all trigs on the current page.

### 9.3 Adding Locks in Live Recording

Turn DATA ENTRY knobs while the sequencer plays; parameter locks are recorded at the current playhead position. Lock trigs are automatically inserted on steps that don't already have a note trig.

### 9.4 Removing Locks

- **Single lock**: Hold TRIG + press the DATA ENTRY knob of the locked parameter
- **All locks on a trig**: Remove the note trig and re-enter it (re-entry clears all locks)
- **Real-time erase in Live Recording**: Hold NO + press and hold the DATA ENTRY knob for the parameter to remove

### 9.5 Sample Locks

A special case of parameter lock: the voice/payload assigned to a trig can be changed per-step. Hold a note trig and turn LEVEL/DATA to open the pool list; select a payload to assign it to that step only.

### 9.6 Preset Locks

The voice preset assigned to any individual note trig can be swapped to any preset in the project's pool. Hold the note trig and turn LEVEL/DATA to scroll the pool list. Hold the trig to display which preset is assigned.

---

## 10. Euclidean Sequencer Mode

An alternative trig-generation system using two independent pulse generators.

**Activating**: Press FUNC + AMP to open the SEQUENCER menu; set EUC to ON. The RECORD key turns blue; generated trigs appear as blue TRIG keys.

When Euclidean mode is enabled, any manually placed trigs on the track are hidden and inactive. They reappear when the mode is switched off. Manual trig input is blocked while in Euclidean mode, but parameter locking on generated trigs remains available.

**Converting to regular trigs**: Hold FUNC while turning Euclidean mode off; any previously placed manual trigs are discarded and the Euclidean pattern becomes regular trigs.

### 10.1 Parameters

| Parameter | Description |
|---|---|
| PL1 | Pulse generator 1: number of pulses (trigs) placed on the track |
| PL2 | Pulse generator 2: number of pulses placed on the track |
| LEN | Track length in steps (only available in Per Track mode) |
| EUC | Euclidean mode on/off |
| R01 | Rotation for generator 1: shifts generator 1's trigs forward or backward on the timeline |
| R02 | Rotation for generator 2 |
| TRO | Track rotation: shifts all trigs from both generators simultaneously |
| OP | Boolean operator combining the two generators' outputs |

### 10.2 Boolean Operators

| Operator | Behavior |
|---|---|
| OR | All trigs from generator 1 or 2 are placed |
| XOR | Trigs from either generator, except where both would occupy the same step |
| AND | Only steps where both generators produce a trig |
| SUB | Generator 1's trigs, minus any steps where generator 2 also fires |

---

## 11. Quantize Menu

Post-recording quantization for micro-timed and off-grid trigs.

**Access**: FUNC + TRIG PARAMETERS

| Parameter | Description |
|---|---|
| TRACK | Quantizes all trigs on the active track in real time. Higher value = stronger quantization. Press TRIG 1–16 to select which track. |
| PATTERN | Quantizes all trigs across all tracks in real time. Higher value = stronger quantization. |

---

## 12. Fill Mode

Fill mode creates a temporary variation in the pattern — typically used for drum fills or transitional material.

**Activation options**:
- YES + PAGE → activates for one full pattern cycle (activates at the next loop, remains active for one cycle)
- Hold PAGE (while not in Grid Recording mode) → active for as long as PAGE is held
- Hold PAGE + YES, then release PAGE before releasing YES → **latch** Fill mode on
- Press PAGE again → unlatch

**Integration with trigs**: Trigs with FILL set to ON only fire when Fill mode is active. Trigs with FILL set to OFF only fire when Fill mode is inactive. This enables parallel sequences on the same track.

---

## 13. Mute System

Two independent mute contexts exist.

### 13.1 Global Mute Mode

- Muted tracks are muted **across all patterns**
- TRIG keys light **green** (lit = unmuted, unlit = muted)
- Muted tracks with trigs: TRIG key **blinks green** during playback
- **Enter**: FUNC + TRK
- **Exit**: FUNC + TRK
- Mute state saved with the project

### 13.2 Pattern Mute Mode

- Muted tracks are muted **only in the active pattern**
- TRIG keys light **magenta** (lit = unmuted, unlit = muted)
- Muted tracks with trigs: TRIG key **blinks magenta** during playback
- **Enter**: FUNC + double-press TRK
- **Exit**: FUNC + TRK
- Mute state saved with the pattern

### 13.3 Prepared Muting

In any mute mode: hold FUNC + press TRIG keys → marks tracks for pending mute/unmute. The action executes when FUNC is released.

UI indicators for prepared state:
- Square = unmuted
- Horizontal line = muted
- Plus sign = prepared to unmute
- X = prepared to mute

### 13.4 Quick Mute

FUNC + TRACK keys → instant mute/unmute without entering a mute mode.

The device remembers the last-used mute mode and returns to it on the next mute access.

---

## 14. Trig Modes

Trig modes change what the 16 TRIG keys do when pressed. The selected mode applies across all tracks.

**Cycle modes**: FUNC + UP/DOWN to select the active trig mode.

All trig mode actions can be recorded in Live Recording mode.

| Mode | TRIG key behavior |
|---|---|
| TRACKS | Default. Each TRIG key fires the corresponding track (1–16) |
| VELOCITIES | All 16 TRIG keys fire the active track at velocities 8, 16, 24, … 127 |
| RETRIGS | All 16 TRIG keys fire the active track at retrig rates from 1/1 to 1/80 |
| SLICES | TRIG keys trigger slices on the active track (for Grid/Slice machines). PAGE advances through groups of 16 slices. |
| PRESET POOL | TRIG keys trigger the 16 presets in pool slots 1–16. PAGE + RIGHT/LEFT navigates to other groups of 16. |

**Velocity mapping** (VELOCITIES mode):
Trig 1–16 → Velocity 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 127

**Retrig rate mapping** (RETRIGS mode):
Trig 1–16 → 1/1, 1/2, 1/3, 1/4, 1/5, 1/6, 1/8, 1/12, 1/16, 1/20, 1/24, 1/32, 1/40, 1/48, 1/64, 1/80

---

## 15. Keyboard Mode

Keyboard mode enables chromatic/melodic input directly from the 16 TRIG keys.

**Enter/Exit**: Press KEYBOARD key.

### 15.1 Scale and Layout

Configured via FUNC + KEYBOARD (KEYBOARD SETUP MENU):

| Parameter | Description |
|---|---|
| KB SCALE | Musical scale filter; only in-scale notes are playable. Chromatic = all 12 notes. See Appendix D of source manual for all scales. |
| ROOT NOTE | Root note of the chosen scale |
| KB FOLD | OFF: piano-like layout (only some TRIG keys lit, resembling a keyboard octave). ON: all 16 TRIG keys play notes; lower row (TRIG 9–16) = lower octave root upward through scale; upper row continues up. |

The full pitch range spans approximately 10 octaves (C0–G10). The keyboard can be transposed ±4 to 5 octaves from middle position.

**Octave transpose**: KEYBOARD + UP/DOWN (or just UP/DOWN if U/D KEY MODE is set to KB OCT).

Keyboard mode state (current octave position) is not stored per pattern; it persists in the last-set state.

### 15.2 External MIDI Keyboard Integration

An external keyboard connected to the Digitakt II can play the active track chromatically at all times (not only in Keyboard mode), provided the external keyboard and the Auto MIDI channel are configured to the same MIDI channel.

---

## 16. Copy, Paste, and Clear Operations

The following objects support copy (FUNC + RECORD), paste (FUNC + STOP), and clear (FUNC + PLAY). Paste and clear are undone by repeating the key combination.

| Object | Context | Notes |
|---|---|---|
| Pattern | Grid Recording OFF | Copy active pattern to another location in the same or another bank |
| Track | Grid Recording ON | Copies trig data, parameter locks for the track |
| Track Page | Grid Recording ON | PAGE + RECORD / PAGE + STOP / PAGE + PLAY — affects only the active 16-step page |
| Parameter Page | Any | PARAMETER + RECORD/STOP/PLAY — copies all settings on a single parameter page |
| Trig | Grid Recording ON | Hold TRIG + RECORD to copy; hold destination TRIG + STOP to paste. Multiple trigs can be held simultaneously for multi-trig copy. Hold TRIG + PLAY to clear parameter locks. |

**Non-destructive pattern copy**: Without leaving the active pattern, press PTN, then:
- TRIG + RECORD → copy the selected pattern
- TRIG + STOP → paste to the selected slot
- TRIG + PLAY → clear the selected pattern

The clipboard holds one item at a time.

---

## 17. Temporary Save / Reload

Provides a non-permanent snapshot for live performance safety.

| Action | Key combo | Description |
|---|---|---|
| Temporary Save | FUNC + YES | Saves current pattern state to a volatile restore point (not written to persistent storage) |
| Temporary Reload | FUNC + NO | Reloads from the last temporary save; if none exists, reloads from the last permanent save |

The temporary save is lost on project change or power-off. It does not replace permanent save.

---

## 18. Perform Kit Mode

A performance mode that decouples parameter changes from persistent storage and pattern changes.

**Toggle**: FUNC + PRESET/KIT (a flashing "P" appears next to the pattern name when active)

**Behavior**:
- Parameter tweaks are not auto-saved
- When switching patterns, the current (tweaked) parameter state is retained instead of loading the new pattern's kit
- Allows continuous, evolving performance across multiple patterns without resets or accidental saves

**Actions in Perform Kit Mode**:
- PRESET/KIT + NO → reload the current pattern's original kit immediately
- PRESET/KIT + YES → open the SAVE (KIT) menu to commit changes permanently

Temporary save/reload still functions in this mode. Changes are lost on power-off unless explicitly saved.

---

## 19. Pattern Chaining

A chain is a temporary, ordered sequence of patterns that loops.

**Chain capacity**: Up to 64 patterns, drawn from any bank (A–H).

### 19.1 Creating a Chain (Primary method)

1. Press PTN, then FUNC + YES to open the chain creator
2. Use LEFT/RIGHT to select bank; press TRIG 1–16 to add patterns in sequence
3. FUNC + LEFT to remove the last pattern from the chain
4. YES to confirm and close; NO to cancel
5. Press PLAY to start the chain (it loops at the end)

### 19.2 Legacy Chain Method

1. Hold PTN + press TRIG 1–16 for the first pattern
2. Release PTN; press subsequent TRIG keys while holding the previous one
3. Press PLAY to start

Chains can be created while the sequencer is running. Chains are **volatile** — they are lost when a new chain or pattern/song is selected, and do not persist across power cycles.

---

## 20. Song Mode

Song mode is a linear arrangement of patterns with per-row control.

**Capacity**: Up to 99 rows per song; up to 16 songs per project. Songs auto-save as they are edited.

### 20.1 Song Row Parameters

| Parameter | Description |
|---|---|
| SONG ROW | Row number (1–99) |
| LABEL | Structural keyword (Verse, Chorus, Fill, etc.) |
| PTN | Pattern assigned to this row |
| ROW PLAY COUNT | How many times this row repeats before advancing |
| ROW LENGTH | How many steps to play from the assigned pattern (2–1024 steps; default = full pattern length) |
| ROW TEMPO | BPM for this row (default: inherits from assigned pattern). Any row-level override propagates as a global override. Swing is always per-row. |
| ROW MUTE | Per-track mute state for this row (inherits from pattern's mute state by default) |
| END | Terminal row: sets behavior when song finishes (LOOP from beginning, or STOP) |

Song POINTER shows the current time and playhead position in the song arrangement.

### 20.2 Song Editing Operations

| Action | Key combo |
|---|---|
| Insert row below | FUNC + DOWN |
| Delete selected row | FUNC + UP |
| Copy row | FUNC + RECORD |
| Paste row | FUNC + STOP |
| Reset row to defaults | FUNC + PLAY |

### 20.3 Song Playback Controls

| Action | Key combo |
|---|---|
| Enter Song mode | SONG + TRIG 1–16 |
| Open Song Edit | FUNC + SONG |
| Loop current row | SONG + LEFT (press again to return to normal) |
| Jump to row | SONG + UP / SONG + DOWN |
| Exit Song mode | Select a pattern via PTN + TRIG |

Press STOP to pause at current position; PLAY resumes from there. Double STOP returns playhead to the song beginning.

---

## 21. MIDI Track Sequencing

MIDI tracks follow the same sequencer rules as audio tracks with the following differences:

- **Output destination**: MIDI OUT or USB port instead of internal voice
- **Per-trig chords**: Up to 4 simultaneous notes per trig
- **Note input from external keyboard**: First note sets velocity for all notes on the trig; last released note sets the trig length
- **Retrig**: Not available on MIDI tracks
- **Portamento**: Not available on MIDI tracks
- **Track parameters**: SRC page carries MIDI channel, program, and aftertouch; FLTR and AMP pages carry up to 16 freely assignable MIDI CC parameters and their values
- **Multiple tracks on same channel**: Permitted; lowest-numbered track has priority on parameter conflicts

---

## 22. MIDI Input Control

An external MIDI device can control sequencer behavior:

| MIDI message | Sequencer action |
|---|---|
| Note C0–D#1 (notes 0–15) | Fire track 1–16 (default channel mapping) |
| Note E2–C7 (notes 16–84) | Fire active track chromatically (C5 = middle C) |
| Program Change 0–127 | Select pattern A01–H16 |
| MIDI Clock | Sync sequencer tempo |
| Note C0 on FX CONTROL channel | Tap tempo |

CC and NRPN messages can control internal parameters as specified in the MIDI appendix of the source manual.

---

## 23. Sequencer State Indicators Summary

| Visual cue | Meaning |
|---|---|
| Solid red TRIG key | Note trig at this step |
| Blinking red TRIG key | Note trig with parameter lock(s) |
| Yellow TRIG key | Lock trig at this step |
| Blinking yellow TRIG key | Lock trig with parameter lock(s) |
| Blue TRIG key (in Euclidean mode) | Euclidean-generated trig |
| Unlit TRIG key | Empty step |
| RECORD key solid red | Grid Recording mode active |
| RECORD key flashing red | Live Recording mode active |
| RECORD key double-blinking red | Step Recording mode active |
| RECORD key blue | Euclidean mode active |
| Flashing PATTERN PAGE LED | Currently active/playing page |
| Lit PATTERN PAGE LED | Pattern has this many pages |
| Green TRIG keys (in Mute mode) | Global Mute — lit = unmuted |
| Magenta TRIG keys (in Mute mode) | Pattern Mute — lit = unmuted |
| Flashing "P" next to pattern name | Perform Kit mode active |
| Flashing bank/pattern in top-left | Pattern cued; waiting to switch |

---

## 24. Key Combination Reference (Sequencer-Relevant)

| Action | Key combination |
|---|---|
| **Transport** | |
| Start / pause | PLAY |
| Stop | STOP |
| Stop with fade | STOP + STOP |
| **Modes** | |
| Grid Recording | RECORD |
| Live Recording | RECORD + PLAY |
| Step Recording (Standard) | RECORD + STOP |
| Step Recording (Jump) | RECORD + STOP + STOP (toggle) |
| **Pattern** | |
| Select pattern | PTN + TRIG 1–16 |
| Select bank + pattern | PTN + LEFT/RIGHT + TRIG 1–16 |
| Create chain | PTN + FUNC + YES |
| **Track** | |
| Select active track | TRK + TRIG 1–16 |
| **Pages** | |
| Page Setup menu | FUNC + PAGE |
| Toggle per-pattern / per-track | (in Page Setup) FUNC + YES |
| Select edit page | PAGE + TRIG 1–8 |
| Select loop pages | (in Grid Rec) PAGE + TRIG 9–16 |
| **Trig** | |
| Add note trig | TRIG key (in Grid Rec) |
| Add lock trig | FUNC + TRIG (in Grid Rec) |
| Trig shift | FUNC + LEFT/RIGHT (in Grid Rec) |
| Preview trig | TRIG + YES |
| **Parameter Locks** | |
| Lock parameter on trig | Hold TRIG + turn DATA ENTRY knob |
| Remove single lock | Hold TRIG + press DATA ENTRY knob |
| Bulk lock (all trigs on page) | TRIG + PAGE, turn knob |
| Quantize menu | FUNC + TRIG PARAMETERS |
| **Micro Timing** | |
| Access micro timing | (in Grid Rec) Hold TRIG + LEFT/RIGHT |
| **Mute** | |
| Global Mute mode | FUNC + TRK |
| Pattern Mute mode | FUNC + double-press TRK |
| Quick Mute | FUNC + TRACK keys |
| Prepared mute | (in Mute mode) FUNC + TRIG keys |
| **Fill** | |
| Fill for one cycle | YES + PAGE |
| Hold Fill | Hold PAGE (not in Grid Rec) |
| Latch Fill | Hold PAGE + YES, release PAGE |
| Unlatch Fill | PAGE |
| **Trig Mode** | |
| Cycle trig modes | FUNC + UP / FUNC + DOWN |
| **Keyboard** | |
| Toggle Keyboard mode | KEYBOARD |
| Keyboard Setup menu | FUNC + KEYBOARD |
| Transpose keyboard octave | KEYBOARD + UP / KEYBOARD + DOWN |
| **Song** | |
| Enter Song mode | SONG + TRIG 1–16 |
| Song Edit screen | FUNC + SONG |
| Loop current song row | SONG + LEFT |
| Jump song row | SONG + UP / SONG + DOWN |
| **Copy / Paste / Clear** | |
| Copy | FUNC + RECORD |
| Paste | FUNC + STOP |
| Clear | FUNC + PLAY |
| Copy row (Song Edit) | FUNC + RECORD |
| **Save / Reload** | |
| Temporary save pattern | FUNC + YES |
| Temporary reload pattern | FUNC + NO |
| **Euclidean** | |
| Sequencer menu (Euclidean params) | FUNC + AMP |
| Convert Euclidean → regular trigs | Hold FUNC + turn Euclidean mode off |
| **Perform Kit** | |
| Toggle Perform Kit mode | FUNC + PRESET/KIT |
| Reload kit | PRESET/KIT + NO |
| Save modified kit | PRESET/KIT + YES |
| **Control All** | |
| Apply parameter change to all tracks | Hold TRK + turn DATA ENTRY knob |
| Revert control all changes | Press NO before releasing TRK |
