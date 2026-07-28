# ADR-044 — Theotokos fixed panel and trig-first mode model

**Status:** 🟡 Proposed (2026-07-27) — awaits user ratification (R1–R3 below).
**Supersedes in part:** `design/theotokos/design.md` §5.1/§5.2 (reopened
2026-07-27), TK2 spec §0 A9 and §0 A16, ADR-038 D1's Mute-screen half.
**Evidence:** `design/sessions/theotokos-2.md`, `design/phases/tk2-report.md`
(usability session #2, TK2 C10, held 2026-07-27).
**Implemented by:** `design/phases/tk2.1-theotokos.md` (drafted with this ADR).

Third-party marks appear per house naming policy: design prose only, never
identifiers or UI strings.

---

## Context

TK2 shipped C0–C9 and reached code-complete. Session #2 confirmed the
plumbing underneath (live-trig command, encoder resolution, tempo
derivation, key remapping, mute state, live visualization) but rejected
two things the phase had treated as settled:

1. **The rendering.** The GRID screen draws every track at once, stacked
   as repeated two-row blocks (`render.rs::render_seq_grid`, one block per
   track). Hands-on, that reads as an LED-grid emulator, not the fixed
   front panel §5.1's own skeleton describes — "transport header / *one*
   contextual window / mode line / echo area."
2. **The default mode.** The app launches REC-armed with the transport
   already running, so a trig key always writes a step and never sounds
   anything. design.md §3.A points 3–4 already say the opposite ("trigs
   are trigs everywhere", "on hardware, trigs always play") — those points
   are why the live-trig command (D5/`CMD_TRIG_NOW`) was built in the
   first place. The shipped default inverts the intent the command exists
   to serve.

Per §6's convergence rule, a DETERMINED item reopens only on new hands-on
evidence. Session #2 is that evidence for both. This ADR freezes what
replaces them, plus the four smaller session verdicts that ride along
(sticky-prefix re-tap, Mute-screen retirement, encoder-mode access,
jog resolution).

### Verified code facts this ADR is built on

Claims below were line-checked against TK2 HEAD (`be565b9`), per the
standing "verify dependency behavior, not existence" rule:

| Claim | Evidence |
|---|---|
| GRID renders all tracks stacked | `render.rs:272-297` — `for t in 0..track_names.len()` pushes 4–5 `Line`s per track |
| Transport auto-starts on launch | `internal_clock.rs:82` — `with_domain` constructs with `playing: true`; `main.rs:233` documents the consequence |
| `grid_rec` defaults on | `model.rs:204` (`grid_rec: true`); `input.rs:629` routes trigs to `ToggleStep` when set |
| A live trig ignores which trig was pressed | `lib.rs:757-766` — `Action::LiveTrig { .. }` discards `col` and fires `active_track`'s sequencer |
| Composite pages jog with a fake 0..1 range | `model.rs:357` — the composite branch returns `(node_id, param_id, name, 0.0, 1.0)`; only the engine-local fallback (`model.rs:384`, `model.rs:400`) carries real `min`/`max`. The default 4-track instrument uses composite views, so in session #2 *every* encoder had range 1.0 |
| `CompositeParam` carries no range metadata | `paraclete-view-assembly/src/lib.rs:59-68` — no `min`/`max`/`stepped`/`unit` fields (hence the placeholder above). `Model::caps` is keyed by arbitrary node id (`model.rs:747` already looks up a composite param's own node), so Theotokos can resolve the real descriptor locally — no cross-crate change is needed |
| `stepped` params exist and are jogged fractionally today | `capability.rs:93` declares `stepped: bool`; `resolve_encoder_params` never reads it |
| Sticky re-press is a deliberate no-op | `input.rs:479-490` — §0 A9, because OS auto-repeat streams `Press` events indistinguishable from a second tap without kitty release events |
| Key chips are derivable | `Keymap.bindings: HashMap<KeyBinding, PanelButton>` (`input.rs:249`) plus the built-in `TOP_TRIG_ROW`/`BOTTOM_TRIG_ROW` tables (`input.rs:232-235`) — a reverse lookup is a pure function over data that already exists |

---

## Decisions

### D1 — Fixed regions; only the contextual window changes

The terminal is divided into regions whose heights do **not** vary by
screen. Screens swap the contents of exactly one region.

```
┌ transport  1 line   BPM · ▶/■ · REC○/▦/● · track · pattern · step · CPU ──┐
│                                                                            │
│ contextual window   Min(0) — the ONLY region that changes per screen:      │
│   TRIG (default)    selected track: engine name, its TRIG-page params,     │
│                     live envelope/LFO for that voice                       │
│   PARAM (Pg1–6)     8 encoder cells (2×4) + live env gauge + LFO phase     │
│   CHAIN / TEMPO / SETTINGS   as shipped in TK2 C6                          │
│                                                                            │
├ track indicator  1 line   ▸1 Kick   2 Snare   3 HiHat●   4 Bass    P1 ─────┤
│ trig strip       2 lines  selected track only, 8 cells per line            │
├ key legend       2 lines  [key] NAME chips, fixed position, never scrolls  ┤
└ echo area        1 line   messages, confirms, `:` command line ────────────┘
                    status line 1 line (unchanged from TK2 C3)
```

The trig strip and track indicator are **persistent**: they render on
every screen, including Param, Chain, Tempo and Settings. Session #2's
"trigs disappearing in encoder mode" is a layout bug under this decision,
not a mode consequence.

### D2 — The strip shows one track

The trig strip renders the **selected track's** 16-step window as 2×8
cells (top line steps 1–8, bottom 9–16), with playhead, trig state, lock
markers and step focus exactly as TK2 renders them today — for one track,
not all of them. Cross-track information (names, mute state, which track
is selected, active pattern) is carried by the single-line track
indicator above it.

This is the literal reading of §5.1's skeleton; the all-tracks stack was a
literal reading of §5.2's "two rows per track" applied to every track at
once.

### D3 — The key chip is drawn where the key acts

Every cell a physical key currently addresses carries that key's
character, resolved through the live `Keymap` (user binding wins;
otherwise the built-in table). When a mode changes what the trig keys
address, the chips move with the meaning:

| State | Chips on the track indicator | Chips on the step cells | Chips on encoder cells |
|---|---|---|---|
| `RecMode::Off` / `Live` on Grid (pads) | **yes** — `[q]1 Kick` | dimmed (display-only) | — |
| `RecMode::Grid` | — | **yes** — `[q]▓` | — |
| Param screen (any rec mode) | — | dimmed (display-only) | **yes** |

The invariant is one sentence and one test: *a key chip appears on the
cell that key would act on if pressed right now, and nowhere else.*
It is also the answer to session #2's "trig cells don't show their mapped
key" finding, and it degrades correctly under user remapping.

### D4 — A labeled legend strip, not a hint line

Two fixed lines of `[key] NAME` chips (bright key, dim label), screen-aware
content, always in the same place. On overflow the strip truncates by a
declared priority order — it never wraps, never scrolls, and never moves.
This replaces the grey run-on hint line (`render.rs:221-238`).

### D5 — `RecMode { Off, Grid, Live }`; REC cycles

`grid_rec: bool` becomes a three-state mode, default **`Off`**.

| Mode | Indicator | Trig keys | Transport interaction |
|---|---|---|---|
| `Off` (default) | `REC○` | pads (D6) | none |
| `Grid` | `REC▦` | write/clear steps of the selected track | none — step programming works while playing |
| `Live` | `REC●` | pads **and** record, engine-side (D8) | records only while playing |

The REC button cycles `Off → Grid → Live → Off`. The transport never
changes the rec mode and the rec mode never starts the transport.

This is a deliberate deviation from session #2's literal "PLAY+REC
together = live record" wording, chosen at the user's direction during
this drafting pass: deriving Live from `rec_armed × playing` would make it
impossible to program steps while the pattern loops, and making REC a
third hold prefix would saddle the most-used button with the kitty-less
sticky one-shot delay. Marked HYPOTHESIS for session #3 (§6 convergence
rule) — the cycle is a grammar claim, not an architectural one.

### D6 — In pad modes, trig N addresses track N

In `Off` and `Live`, a trig press:

1. sounds track N live (`CMD_TRIG_NOW` on **that** track's sequencer —
   today's handler ignores the column, `lib.rs:757`), and
2. makes track N the selected track, so the contextual window, trig strip
   and track indicator all follow the finger.

Columns past the discovered track count are a no-op plus the existing
`no track N` echo (TK2 D9's clamp, reused verbatim). In `Grid`, trig N is
step N of the selected track — unchanged.

This *is* a mode split in the trig keys, which §3.A point 3 warns about.
It is accepted deliberately: the split is between "play the instrument"
and "program the instrument", it is announced by the REC indicator, and
D3's key chips make the current meaning visible on-screen at all times
rather than leaving it to memory. §0 A16 ("trigs are always trigs on Grid
and Param") is superseded accordingly.

### D7 — No transport at launch

Theotokos issues `CMD_CLOCK_STOP` once during startup, so the instrument
boots silent and stopped. The engine-side default that causes the
auto-start (`InternalClock::with_domain` → `playing: true`,
`internal_clock.rs:82`) is **not** changed here: it is load-bearing for
`tools/test-driver` scenarios, regression baselines, the CLAP subgraph and
`main.rs`'s static snapshot. It is filed as **BUG-039** so the wider
question ("should any surface auto-start?") is owned somewhere, rather
than being silently worked around in this ADR's prose.

### D8 — Live record is engine-side, per ADR-039 decision 7

`Live` does **not** compute steps on the surface. Entering `Live` sends
`CMD_SET_PARAM live_rec = 1` to every track sequencer; leaving it sends
`0`. Recording then happens inside the sequencer: while `live_rec ≥ 0.5`
and the transport is playing, a consumed `CMD_TRIG_NOW` records itself —
nearest-step quantization, note and velocity written, signed distance to
the grid captured as the step's micro-timing.

This is ADR-039 decision 7 as accepted ("a pending `CMD_TRIG_NOW` (TK2 C1)
records itself the same way when `live_rec` is on — Theotokos REC+PLAY
needs no extra path"), whose rejected alternative is precisely the
surface-side `CMD_SET_STEP` path. TK2.1 therefore **implements that one
slice of ADR-039 early** (the `live_rec` param and the record-on-live-trig
path — no kits, no temp save, no mute tiers, none of CMD 39–45) so the
rec-mode model is complete; the P11 phase spec inherits it as shipped
rather than re-planning it.

Timing bound, stated honestly per ADR-039's own amendment 2: `CMD_TRIG_NOW`
is delivered at block start, so the recorded micro-timing is exact to the
sequencer's tick position at command-drain time, not to the keystroke.
Sub-block accuracy waits on the HAL timestamping work ADR-039 names as
P11 scope. A pad press in `Live` with the transport stopped sounds the
voice and records nothing.

### D9 — Encoder mode is the Param screen, not a new toggle key

Session #2 asked for a toggle key replacing held-FUNC for encoder access.
This ADR grants the ergonomics without adding a key: **on the Param
screen the trig rows are the encoder bank** — bare top-row key *n* =
encoder *n* up, bottom-row = down, no modifier held. The screen is
already reached by `1`–`6` and left by `Esc`, so the toggle exists and is
already learned. `FUNC+trig` keeps working from every screen as the
quick-access shortcut (and doubles as the coarse magnitude on Param, D10).

Consequence: trigs do not audition or edit steps while the Param screen is
open. The strip stays visible there (display-only, dimmed chips, D3), so
you can still read the pattern while editing sound. HYPOTHESIS for
session #3.

### D10 — Jog magnitude comes from the real parameter descriptor

Encoder resolution stops inventing a range. `resolve_encoder_params` looks
each `(node_id, param_id)` up in `Model::caps` and carries the real `min`,
`max` and `stepped` from the `ParamDescriptor`; the 0..1 placeholder
(`model.rs:357`) survives only as a last-resort fallback when no
capability document declares the param, and a cell resolved that way is
rendered dimmed so the condition is visible rather than silent.

| Magnitude | Binding | Step |
|---|---|---|
| Fine | `Ctrl` + trig | `range/512` |
| Normal | bare trig on Param; `FUNC`+trig elsewhere | `range/64` |
| Coarse | `FUNC` + trig on Param | `range/16` |

`stepped` params ignore the table and jog by exactly **1** per press
(algorithm and machine selectors are unusable otherwise: an 8-value
selector currently moves 8/128 ≈ 0.06 per press). Ramp and acceleration
(`Tuning::jog_step`, dwell 150 ms, ×1.05 capped at ×8) are unchanged.
The defect this replaces is filed as **BUG-040**.

### D11 — Sticky-prefix re-tap disarms, guarded by a repeat window

§0 A9 (same-prefix re-press is a no-op) is **reversed**: a second press of
an armed TRK/PTN prefix disarms it. A9's underlying observation still
holds — without kitty release events, OS auto-repeat is indistinguishable
from a deliberate second tap — so the toggle is guarded by time: a
same-prefix press arriving within `repeat_guard_ms` (default **400 ms**,
*tunable*) of the previous same-prefix press is treated as auto-repeat and
ignored; beyond it, it disarms. Holding TRK therefore cannot flap the
armed state (repeats arrive every ~30 ms), while tap-pause-tap disarms as
session #2 expects. The kitty path (physical release) is untouched.

### D12 — The Mute screen is retired (OQ-T22 resolved)

`Screen::Mute` and `PanelButton::Mute` are removed. `TRK`+`FUNC`+trig is
the only mute gesture; per-track mute state becomes a marker on the track
indicator line (D2), which is visible on every screen — strictly more
available than the screen it replaces. `m` becomes an unbound key,
available to `:bind`, and drops out of the `:bind` button vocabulary.

---

## Ratification questions

| # | Question | Recommendation |
|---|---|---|
| **R1** | D9 — is "encoder mode = the Param screen" the right reading of session #2's toggle-key request, or do you want a dedicated ENC key that overlays the encoder bank on the Grid screen? | As written (no new key; the screen is the toggle) |
| **R2** | D8 — pulling ADR-039 decision 7's `live_rec` slice into TK2.1 (so `RecMode::Live` does something) versus shipping `Off`/`Grid` only and waiting for the P11 phase spec | Pull it forward; the slice is small and fully specified, and a REC cycle with a dead third state is worse than either |
| **R3** | D12 — remove `Mute` from the `:bind` vocabulary entirely, or keep the button name bindable as an alias for the mute chord? | Remove; a chord has no single-button equivalent, so an alias would be a lie |

## Alternatives considered

- **Derive Live from `rec_armed × playing`** (session #2's literal
  wording). Rejected during drafting: it makes step programming
  impossible while the pattern plays.
- **REC as a third hold prefix** (`Hold::Rec`, REC+PLAY = Live). Rejected:
  on kitty-less terminals the most-used button would inherit the sticky
  one-shot delay, and a bare REC tap could not resolve until the next key.
- **Keep trig = step in pad mode** (any trig sounds the selected track,
  track select stays on TRK+trig — today's behavior). Rejected by the user
  in this drafting pass: it does not deliver multi-track finger drumming.
- **Split by row** (bottom row = track pads, top row = step audition).
  Rejected: introduces a row asymmetry no other mode has.
- **A dedicated ENC toggle key.** Rejected under D9 — no free key with a
  good mnemonic, and the Param screen already is the mode.
- **Change `InternalClock`'s `playing` default to `false`.** Rejected
  under D7 — it would move regression baselines and every test-driver
  scenario in one step, for a Theotokos-scoped complaint. Filed as
  BUG-039 instead.
- **Surface-side live record** (read `current_step` from the bus, send
  `CMD_SET_STEP`). Drafted first, then withdrawn: ADR-039 decision 7 is
  accepted and lists exactly this as its rejected alternative ("micro-
  timing from a UI tick is quantization noise"). D8 follows the accepted
  ADR instead.
- **Ship `Off`/`Grid` only, defer `Live` to P11.** Rejected under R2 — a
  three-state cycle whose third state does nothing is a worse teaching
  surface than either complete option.

## Consequences

- `render.rs` gains a fixed-region layout, a strip renderer, a track
  indicator, a chip resolver and a legend builder; `render_seq_grid`'s
  all-tracks path and `render_mute_screen` are deleted.
- `Model.grid_rec: bool` → `Model.rec: RecMode`; `Screen::Mute` and
  `PanelButton::Mute` disappear from the model, the keymap vocabulary,
  the help overlay and the `:bind` documentation.
- `RenderData` grows chip/label fields and loses `mute_states`' screen —
  the data stays, its home moves to the track indicator.
- `paraclete-nodes/sequencer.rs` gains ADR-039 decision 7's `live_rec`
  bank param and its record-on-live-trig path — the only engine change in
  this phase, and P11 scope consumed early (D8). The P11 phase spec must
  record it as shipped, not re-plan it.
- Two defects filed under the standing directive: **BUG-039** (engine
  transport auto-start), **BUG-040** (encoder jog range/stepped).
- design.md §5.1/§5.2 are rewritten as DETERMINED against this ADR when it
  is ratified, and its Stage 5 note records the reopening's resolution.
- **Out of scope, still open:** OQ-T24 (numpad cluster fate — session #3),
  OQ-T23's screen-gating friction on tap tempo, `:` remap discoverability
  (D11 of TK2, deferred by user request), OQ-T21 (KEYBD chromatic).

## Test seams

Per design.md §6, everything except feel is machine-checkable here:

- **Pure mapping:** rec-mode cycling, pad→track resolution and clamp,
  encoder magnitudes, sticky re-tap vs. auto-repeat (injectable clock).
- **Resolution:** descriptor-accurate min/max/stepped against fixture
  capability documents, including the composite path that produced the
  0..1 placeholder.
- **Render:** `TestBackend` buffer assertions for one-track strip, chip
  placement per mode, legend chips, persistence of the strip on every
  screen.
- **Engine effect:** sequencer tests for `live_rec` (records the nearest
  step, writes micro-timing, ignores live trigs while stopped), plus a
  `tools/test-driver` scenario driving `set_param live_rec` + `trig_now`
  — both actions already exist in the driver (`main.rs:750`, `:809`).
- **Feel:** usability session #3.

## Cross-references

- `design/sessions/theotokos-2.md`, `design/phases/tk2-report.md` — evidence
- `design/phases/tk2.1-theotokos.md` — the commit blueprint
- ADR-036 (Theotokos), ADR-038 (Elektron convergence), ADR-037 (key remapping)
- design.md §3.A (deficiency review), §5.1/§5.2 (reopened), §6 (convergence rule)
