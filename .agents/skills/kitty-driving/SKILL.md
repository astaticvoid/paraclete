---
name: kitty-driving
description: Driving the Theotokos panel from an agent via kitty remote control. Use for paired usability sessions, reading panel state, or automating keyboard input to the running app.
---

# Driving the Panel from an Agent

Run the app in **kitty** with remote control. **Never through tmux** — its `extended-keys` doesn't proxy key *releases*, which silently forces the sticky fallback and invalidates every hold/chord result.

## Setup

```bash
setsid kitty --listen-on unix:@tk4 -o allow_remote_control=yes \
  -e bash -c 'exec ./target/release/paraclete 2>>/tmp/tk4-app.log' &
kitty @ --to unix:@tk4 get-text          # read the panel
kitty @ --to unix:@tk4 get-text --ansi   # read WITH colours/attributes
```

`get-text --ansi` reads state the plain text cannot show: active vs empty step glyphs share `▓`/`░` but differ by colour, and the playhead and lock focus are carried by `Modifier::REVERSED` (`[7;33m`) alone.

## Two send verbs — the difference matters

| Verb | Delivers | Use for |
|---|---|---|
| `send-key q` / `send-key shift+z` | press **and** release | anything that should be a **tap**; FUNC (Shift) chords arrive intact |
| `send-text "q"` | press, **no release** | deliberately *latching* a hold prefix |

### Composing hold-chords

```bash
kitty @ --to unix:@tk4 send-text $'\t'   # Tab press, no release -> TRK armed
kitty @ --to unix:@tk4 send-key q        # trig press+release while armed
kitty @ --to unix:@tk4 send-key tab      # press+release -> clears the arm
```

### Hazard

Using `send-text` for a tap latches the key as a held prefix. A latched REC makes every trig `Action::Noop` by design (`input.rs:749`). That reads exactly like "step entry is broken". Use `send-key` for taps.

## Releasing a latch

`send-key` is a *re-press*, not a release. `send-key tab` only works because re-pressing TRK is harmless. It is not harmless in general: `m` (Lock) latched with `send-text`, then "released" with `send-key m`, reads as a deliberate second press and **clears the target you just set**.

Send the keyboard-protocol release instead — `CSI <code>;1:3u`, event type 3:

```bash
kitty @ --to unix:@tk5 send-text 'm'                 # press, no release -> Lock armed
kitty @ --to unix:@tk5 send-key g                    # trig -> sets the lock target
kitty @ --to unix:@tk5 send-text $'\x1b[109;1:3u'    # 109 = 'm', TRUE release -> target survives
```

Verified: with `send-key m` the target vanished; with the release escape `L:s12` stayed latched.

## The user is on the same keyboard

In a paired session their keypresses and yours are indistinguishable from `get-text`. Unexplained state change during a paired session: **ask, don't bisect** — and never file it as a defect on the strength of an agent-side observation alone.

## Panel readout gotchas

- The top line `P1/4` is PAGE/page_count, NOT pattern
- Pattern state is inferred from Len changing
- The `decay ▶ 0%` row is the live ENVELOPE level gauge (0% at rest), NOT the param value
- Read param values from the ENC bank cells (name bar value format)

## Not agent-testable

The sticky-fallback path (D11's re-tap disarm and 400 ms guard) never runs in kitty, which delivers releases — judging it needs a release-less terminal that is not tmux.

**Not a valid audio oracle:** `parecord` off the default sink monitor. It has read digital silence while the user could plainly hear the pattern. Use `test-driver` renders, which assert on a file.
