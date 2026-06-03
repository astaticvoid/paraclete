# ADR-021: Launchpad X Protocol and Device Integration Learnings

**Date:** May 2026  
**Status:** Accepted  
**Context:** Discovered during P4 hardware integration. Documents the exact protocol
required for the Novation Launchpad X and the debugging process used to establish it.
Serves as the reference for future Novation device support.

---

## Source material

Novation Launchpad X Programmer's Reference Manual (29 pages).
Cached at project root during development; key extracts reproduced below.
GitHub reference: github.com/wvengen/lpx-controller (Rust, uses rmididings).

---

## Port selection

The Launchpad X exposes two USB MIDI interface pairs:

| Port | Purpose |
|------|---------|
| `LPX DAW In/Out` | DAW/Session mode (Ableton Live, etc.) |
| `LPX MIDI In/Out` | Note mode, Custom modes, **Programmer Mode LED control** |

**Rule:** Use LPX MIDI exclusively for Programmer Mode. Never open LPX DAW
for normal operation — opening that port signals "DAW connected" and puts the
device into Session mode, which fights with Programmer Mode LED control.

The DAW port IS used once on startup (then immediately closed) to send the
DAW mode disable reset. This clears any stuck state from a previous unclean exit.

---

## Initialization sequence

On `LaunchpadNode::open()`:

```
1. Open LPX DAW In output (brief)
   Send: F0 00 20 29 02 0C 10 00 F7   (DAW mode = Standalone)
   Close immediately.

2. Open LPX MIDI In output
   Send: F0 00 20 29 02 0C 00 7F F7   (Layout = Programmer Mode)
   Wait 100ms.

3. Open LPX MIDI Out input (for receiving pad presses).
```

On `LaunchpadOutputHandle::drop()`:

```
Send: F0 00 20 29 02 0C 10 00 F7   (DAW mode = Standalone)
Wait 30ms.
Send: F0 00 20 29 02 0C 00 01 F7   (Layout = Note mode)
```

This ensures the device is usable as a standalone instrument after the app exits.
**Never kill -9 the process** — it skips Drop and leaves the device in Programmer Mode.

---

## SysEx command reference

All messages use the header `F0 00 20 29 02 0C` followed by a command byte.

| Command | Bytes after header | Effect |
|---------|-------------------|--------|
| Layout select | `00 <layout> F7` | Switch layout (see below) |
| DAW mode | `10 <mode> F7` | 00=Standalone, 01=DAW |
| Prog/Live toggle | `0E <mode> F7` | 00=Live, 01=Programmer |
| Layout readback | `00 F7` | Device replies with current layout |
| Mode readback | `0E F7` | Device replies with current mode |
| LED SysEx | `03 <colourspecs> F7` | Set up to 81 LEDs with RGB colors |

Layout values: `00`=Session (DAW only), `01`=Note mode, `04`–`07`=Custom 1–4,
`7F`=Programmer mode.

**State probing (used in lpx-debug tool):** The readback commands make it possible
to verify device state programmatically without user observation. The device echoes
the command with the current value inserted: `F0 00 20 29 02 0C 00 <layout> F7`.

---

## Pad numbering (Programmer Mode)

The device uses a 10×row + col scheme, row 1 = physical bottom, row 8 = top:

```
Physical top row:    notes 81–88  (row 8, cols 1–8)
Physical row 7:      notes 71–78
...
Physical bottom row: notes 11–18  (row 1, cols 1–8)
Scene buttons (right column): notes 19, 29, 39, 49, 59, 69, 79, 89
Top round buttons:   notes 91–98  (NOT CC — Note On in Programmer Mode)
```

Our convention maps physical top row → our row 0 (drum machine top = first track):

```rust
our_row   = 8 - phys_row          // invert: top physical = row 0
our_col   = col - 1               // 0-indexed
pad_id    = our_row * 8 + our_col // 0–63
```

Reverse (pad_id → MIDI note):
```rust
phys_row  = 8 - (pad_id / 8)
note      = phys_row * 10 + (pad_id % 8) + 1
```

---

## LED control

In Programmer Mode, Note On on MIDI channel 1 (status byte `0x90`) sets LED color:
- Note number = pad address (same as input note numbers above)
- Velocity = palette index (0 = off, see Colour Palette in spec)

Key palette entries: 0=off, 1=dim white, 5=red, 21=green, 67=blue, 87=bright green.

**Important:** Velocity 0 turns a pad off regardless of channel. Velocities 1–127
address the 128-colour palette.

**Threshold rule:** Colors with all channels < 32 map to velocity 0 (off). This
prevents near-black colors like (8,8,8) from mapping to a bright green due to
equal-channel dominant-channel logic.

**Future:** The LED SysEx format (`F0 00 20 29 02 0C 03 <specs> F7`) supports full
RGB (0–127 per channel) and is more accurate. Each colourspec: `03 <pad_index> <R> <G> <B>`.
Switching to SysEx is the correct long-term approach.

---

## The marching green bug — root cause and fix

**Symptom:** Every time Paraclete started, the Launchpad X displayed a green column
marching left to right in sync with the sequencer tempo. All attempts to enter
Programmer Mode appeared to have no effect.

**Root causes (two separate bugs):**

**Bug 1 — DAW mode stuck:** An earlier test iteration opened the DAW port and sent
`F0 00 20 29 02 0C 10 01 F7` (DAW mode enable). The app was killed with kill -9
before the cleanup SysEx was sent. The device stored this state and entered
Session/DAW mode on every subsequent power-on. Session mode has its own step display
which cannot be overridden by Programmer Mode commands sent to the MIDI port.

Fix: Send DAW mode disable on startup (via DAW port, then close it) and on exit
(via MIDI port in `Drop`). Never kill -9 the process.

**Bug 2 — The actual marching:** `build_hardware_output()` in the executor was P2
legacy code that auto-generated LED updates from state bus `current_step` values. It
lit the pad at column `current_step % 8` GREEN for every RGB control on every hardware
device, every audio cycle. This ran through the `update_output()` → SPSC → `tick()`
path and overrode everything the scripting layer sent.

Fix: `build_hardware_output()` returns `HardwareOutput::empty()`. The scripting layer
owns all LED output via `deliver_script_output()`.

**Diagnostic method:** The `tools/lpx-debug` binary uses SysEx readback to probe
device state at each initialization step. This allowed attributing Bug 1 vs Bug 2
without requiring visual user observation at every stage.

---

## Rhai 1.25 scripting limitations (discovered during profile development)

1. **Named functions from closures fail silently.** Calling a script-defined function
   from within a subscription callback (which executes as a `FnPtr::call`) works for
   one level but nested calls (`closure → fn_A → fn_B`) crash silently with no error
   logged. All logic that runs inside subscription callbacks must be inlined.

2. **`chars()` returns a range type, not an array.** `s.chars().len()` throws
   "Function not found: len (range)". Do not use `chars()`.

3. **`s[i]` with a variable integer works** and returns a `char`.
   Compare with char literals: `s[i] == '1'`.

4. **`s[0..1]` with literal integers works** and returns a `String`.
   `s[var..var+1]` with variable integers also works.

5. **`split("")` produces extra empty elements** (length 18 for a 16-char string).
   Do not use for character iteration.

6. **For-loop variable capture:** Closures created in a `for` loop share the loop
   variable. Use `let t = track;` inside the loop body to create a per-iteration
   binding that the closure captures correctly.

7. **Template strings with backticks work** for interpolation: `` `value=${x}` ``.

8. **`try/catch` works** and is useful for probing which operations succeed.

---

## Debug tooling

`cargo run -p lpx-debug` — walks through initialization phases, probing device
state at each step via SysEx readback. Writes timestamped log to `/tmp/lpx-debug.log`.
Exits cleanly via Ctrl-C with LP restored to standalone.

`debug_print(msg)` — Rhai builtin that writes to stderr. Add to profiles during
development to trace script execution.

---

## Future device support checklist

For any new Novation/MIDI device:
1. List MIDI ports from `system_profiler SPMIDIDataType` or `MidiDeviceRegistry`
2. Identify which port is for host control vs DAW vs standalone
3. Find the Programmer Mode or equivalent SysEx in the Programmer's Reference
4. Verify with a standalone midir test (no audio, output-only) before integrating
5. Add SysEx readback probing to `lpx-debug` for the new device
6. Add cleanup SysEx to the device node's `Drop` impl
7. Add initialization reset (clear stuck state) to `open()`
