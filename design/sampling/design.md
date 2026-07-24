# Anamnesis — Experience Design (staged, append-only)

> Grows aspect-by-aspect like `design/theotokos/design.md`: **DETERMINED**
> (decided, implementable) / **HYPOTHESIS** (session-tested) / **OPEN**
> (OQ table). Architecture authority: ADR-040. Read order for a new
> session: `problem.md` → this file → OQ table → current AN phase spec.

---

## Stage 1 — 2026-07-23 — first-pass experience design

## §1. The performance loop — DETERMINED shape

Five verbs, each reachable from every surface:

```
   HEAR ──────► CAPTURE ──────► SHAPE ──────► ASSIGN ──────► PERFORM
 (monitor,    (arm/threshold/  (trim, gain,  (pool slot →   (sample+slice
  always-on    manual, retro-   slice grid,   track; sample   locks, scenes,
  recorders)   active grab)     bars tag)     locks)          morph, loops)
```

The Octatrack lesson baked into the shape: **capture is retroactive**.
Recorders run always; "sample it" means *keep what I just heard*, so the
performer never has to predict the future.

## §2. Feature map — the "lose nothing" checklist

| Octatrack capability | Anamnesis home | Status |
|---|---|---|
| Always-on track recorders | `RecorderNode` ring, taps any signal edge (graph wiring — track out, master, external in) | AN1 |
| Retroactive capture / transition trick | `Capture{last_bars}` app-op reads the recorder ring tail; FUNC+SAMPLING chord | AN1 |
| Resampling | Recorder tapped on the mix bus — same path, no special case | AN1 |
| Slices + slice locks | Pool slice table + existing stepped `slice` param + CMD 33–35 lock family | AN2 |
| Sample chains | A chain is just a sliced long sample — free with slices | AN2 |
| Scenes A/B + crossfader morph | Partial-kit snapshots + app-side morph on `/context/xfade` (ADR-039 KitStore extension) | AN2 |
| Parts (kit-per-pattern) | **Already designed** — ADR-039 kits + pattern binding | P11 |
| Pickup machines (live loop, overdub) | Recorder loop mode: pattern-length-synced, overdub, auto-assigned playback slot | AN3 |
| BPM-locked loops (timestretch) | `play_mode` param (repitch ships first; stretch DSP later, same surface) + `bars` tag in pool metadata | AN3 (surface AN0) |
| Retrig | P12 scope (sequencer grammar) — noted, not lost | P12 |
| Param slides | P12 scope — noted, not lost | P12 |
| Neighbor machines | **Already free** — Paraclete tracks *are* open graph chains (ADR-029) | done |
| LFO designer | MOD-page future; OQ-A6 | later |
| Cue outputs | Needs multi-out device support; OQ-A7 | later |
| Flex vs Static (RAM vs streaming) | All-RAM (modern baseline); streaming is an explicit non-goal until pool pressure proves need | n/a |

## §3. The pool — DETERMINED model

Project-level `SamplePool` (app-owned, ADR-040 §2): ordered slots, each
`{ id, name, source: File(path) | Ram(capture), frames, rate, bars: Option<f32>,
gain, slice_table: Vec<(start, end)> }`. Persisted in project RON (file
slots by path + metadata; RAM slots optionally frozen to files on save —
OQ-A2). The sampler's `sample` param becomes a **stepped index into the
pool** — p-lockable, so sample locks ride the existing lock commands
untouched. Slice tables live on the pool entry (per-sample, shared by
every sampler referencing it).

## §4. Capture experience — HYPOTHESIS (session-tested gestures)

**The SAMPLING screen** (Theotokos key `9`, ADR-038 gate now satisfied;
Theoria gets the same via `Rule`/W-track):

- **Source row:** external in · master · track 1–N (recorder taps are
  graph edges; the screen lists what is wired, cap-doc-driven).
- **Arm row:** manual (YES starts/stops) · threshold (dB + pre-roll from
  the ring — the ring means threshold capture *includes the transient*).
- **Monitor toggle** (external in → mix, while armed).
- **Retro row:** `capture last 1/2/4/8 bars` — bar length from the clock.
- After capture: preview (YES), trim handles (arrows; coarse/fine via
  FUNC), `assign to track N` (TRK+trig), `save to pool` / discard (NO).

**The one-gesture transition trick (DETERMINED — frozen as an AN1
requirement at ADR-040 ratification, R2):**
`FUNC+9` = capture last 2 bars of the master into a RAM slot and assign
it to the active track's sampler, slice grid 16 *(defaults tunable)*.
One chord while the pattern plays; the next pattern can play it sliced.

## §5. Scenes & crossfader — DETERMINED model, HYPOTHESIS bindings

- A **scene** is a *partial* kit: only explicitly assigned
  `(node, param, value)` entries (Octatrack semantics — unassigned
  params stay live). Stored beside kits in the KitStore.
- **Crossfader** = one continuous value on `/context/xfade`; the app
  interpolates assigned params between scene A and B each main-loop
  tick and emits `CMD_SET_PARAM` (nodes' own smoothing declicks).
  Instrument-wide, engine-agnostic, surface-agnostic.
- Theotokos bindings (HYPOTHESIS): scenes armed from the KIT screen;
  crossfader on a held-ramp key pair (candidates: `,`/`.` — free in the
  continuous grid) with numpad `4/6` as alternates; snap-back on
  release vs latched is OQ-A4. Theoria gets a real slider; MIDI CC
  binding via profiles.
- Scene assign gesture: hold scene key + move an encoder = assign that
  param at its current value (the Octatrack grammar, one-to-one).

## §6. Live loop (pickup-style) — DETERMINED shape, AN3

Recorder loop mode: loop length = active pattern length (or ×2/÷2),
record → seamless wrap to overdub (feedback param), playback through an
auto-assigned pool slot on the same track's sampler, quantized
start/stop on pattern boundary. First recording defines the loop if no
pattern is playing (free length, bars tagged from the clock). Undo =
temp-save semantics on the loop buffer (one level, ADR-039 spirit).

## §7. Sampler SRC page — DETERMINED (Rule-driven)

The sampler's `Rule` page group gains the canonical SRC page shape:
`sample` (pool slot, stepped) · `slice` · `pitch` · `play_mode`
(repitch/stretch·loop/oneshot/reverse) · `start` · `end` · `volume` ·
`pan` — 8 params, one encoder bank page (ADR-038). `bars`-tagged
samples in stretch mode follow the clock BPM (DSP staged; surface
frozen now — requirement 6).

---

## Open Questions

| # | Question | Status | Where decided |
|---|---|---|---|
| OQ-A1 | Recorder ring length (RAM budget per recorder; candidate 16 bars @ 48k stereo ≈ 30 MB at 120 BPM) | OPEN | AN1 spec |
| OQ-A2 | RAM captures on project save: freeze to files, keep volatile, or ask | OPEN | AN0 spec |
| OQ-A3 | Threshold-arm pre-roll length | OPEN *(tunable)* | AN1 spec |
| OQ-A4 | Crossfader snap-back vs latch; curve shaping | OPEN | session |
| OQ-A5 | Slice grid sizes (4/8/16/32/64) + transient-detect slicing | grid DETERMINED, transient OPEN | AN2 spec |
| OQ-A6 | LFO designer (custom shapes) — MOD page future | OPEN | P12+ |
| OQ-A7 | Cue outputs (multi-out devices) | OPEN | later |
| OQ-A8 | Stretch algorithm choice (granular vs WSOLA) | OPEN | AN3 spec |

## Cross-references

`problem.md` · ADR-040 (architecture) · ADR-038 (SAMPLING screen, KIT
screen, encoder bank) · ADR-039 (kits/KitStore, app-op drain) · ADR-029
(dynamic topology — recorder taps are edges) · ADR-019/ADR-032 (param
plane, `Rule` pages) · P12 (retrig/slides act on this material)
