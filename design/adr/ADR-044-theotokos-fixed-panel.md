# ADR-044 — Theotokos fixed panel and trig-first mode model

**Status:** 🟡 Proposed (2026-07-27; revised the same day after hostile
review) — awaits user ratification (R1–R5 below).
**Will supersede on ratification:** `design/theotokos/design.md` §5.1/§5.2
(reopened 2026-07-27); TK2 spec §0 A9 and A16, and §1 D11/D12. **No
accepted ADR is superseded:** D5 keeps ADR-038's grid-rec toggle and
ADR-039's REC+PLAY grammar, and D6 keeps ADR-038 structural change 2's
hold-chord mechanism. A proposed ADR supersedes nothing until ratified.
**Evidence:** `design/sessions/theotokos-2.md`, `design/phases/tk2-report.md`
(usability session #2, TK2 C10, held 2026-07-27).
**Implemented by:** `design/phases/tk2.1-theotokos.md`.

Third-party marks appear per house naming policy: design prose only, never
identifiers or UI strings.

---

## Context

TK2 shipped C0–C9 and reached code-complete. Session #2 confirmed the
plumbing underneath (live-trig command, encoder resolution, tempo
derivation, key remapping, mute state, live visualization) but rejected
two things the phase had treated as settled:

1. **The rendering.** The GRID screen draws every track at once, stacked
   as repeated blocks (`render.rs::render_seq_grid`, 5–6 `Line`s per
   track — each of the two logical rows drawn twice). Hands-on, that
   reads as an LED-grid emulator, not the fixed front panel §5.1's own
   skeleton describes — "transport header / *one* contextual window /
   mode line / echo area."
2. **The default mode.** The app launches REC-armed with the transport
   already running, so a trig key always writes a step and never sounds
   anything. design.md §3.A points 3–4 already say the opposite ("trigs
   are trigs everywhere", "on hardware, trigs always play") — those points
   are why the live-trig command (`CMD_TRIG_NOW`) was built at all. The
   shipped default inverts the intent the command exists to serve.

Per §6's convergence rule, a DETERMINED item reopens only on new hands-on
evidence. Session #2 is that evidence for both. This ADR freezes what
replaces them, plus the smaller session verdicts that ride along.

### Verified code facts this ADR is built on

Line-checked against TK2 HEAD (`be565b9`) and re-checked under hostile
review, per the standing "verify behavior, not existence" rule:

| Claim | Evidence |
|---|---|
| GRID renders all tracks stacked | `render.rs:272-297` — `for t in 0..track_names.len()` pushes 5 `Line`s per track (each logical row drawn twice) plus a 6th separator between tracks; `grid_structure_4_tracks_23_rows` (`render.rs:903`) pins 4×5+3 |
| Transport auto-starts on launch | `internal_clock.rs:82` — `with_domain` constructs with `playing: true`; `main.rs:233` documents the consequence |
| `grid_rec` defaults on | `model.rs:204`; `input.rs:629` routes trigs to `ToggleStep` when set |
| A live trig ignores which trig was pressed | `lib.rs:757` — `Action::LiveTrig { .. }` discards `col` and fires `active_track`'s sequencer. The variant already declares `col` (`action.rs:53`) |
| Composite pages jog with a fake 0..1 range | `model.rs:357` returns `(node_id, param_id, name, 0.0, 1.0)`; only the engine-local fallback (`model.rs:384`, `:400`) carries real `min`/`max`. The default instrument *does* take the composite path (engines attach a `view` Rule, `analog_engine.rs:322`; `assemble` returns `Some`, `view-assembly/src/lib.rs:98`), and its real ranges are wide — `tune` −24..24, `tone` 200..8000, `index` 0..8 |
| The fake range is also a clamp | `lib.rs:856` — the step-focus p-lock path computes `(current + signed).clamp(min, max)`, so a lock on a 200..8000 param truncates to 1.0 |
| `CompositeParam` carries no range metadata | `view-assembly/src/lib.rs:59-68` — no `min`/`max`/`stepped`/`unit` |
| `Model::caps` covers non-generator nodes | `cap_docs` is built over `ids.all` (`main.rs:265`), `classify_node` pushes every node in the instrument file (`builder.rs:193`), and the map is handed to Theotokos whole (`main.rs:408`). The local descriptor lookup D10 relies on needs **no cross-crate change** |
| `stepped` exists and is never read by the jog path | `capability.rs:93`; absent from `resolve_encoder_params` and `Tuning::jog_step` |
| Sticky re-press is a deliberate no-op | `input.rs:479-490` — §0 A9, because OS auto-repeat streams `Press` events indistinguishable from a second tap without kitty release events |
| Key chips are derivable | `Keymap.bindings` (`input.rs:253`), `key_name`/`key_from_name` round-trip every key the built-in table uses (`input.rs:146-199`), `TOP_TRIG_ROW`/`BOTTOM_TRIG_ROW` (`input.rs:236`, `:238`) |
| Fixed regions behave as D1 assumes | ratatui 0.26.3: `Length` outranks `Min` in the constraint solver (`layout.rs:1019`, `:1040`); `Paragraph` defaults to no wrapping and truncates (`paragraph.rs:97`) |
| **`CMD_CLOCK_STOP` emits no transport event** | `internal_clock.rs:146-148` sets `playing = false` and `:216` returns early. `Sequencer.playing` clears only on `flags.global_stop` (`sequencer.rs:908`), which nothing in the standalone app emits — so a sequencer latches `playing = true` on the first tick and never clears it. Filed as **BUG-041**; D8's stopped-transport behaviour depends on fixing it |

---

## Decisions

### D1 — Fixed regions; only the contextual window changes

Region heights do **not** vary by screen. Screens swap the contents of
exactly one region.

```
┌ transport  1 line   BPM · ▶/■ · REC○/▦/● · track · pattern · step ────────┐
│                                                                            │
│ contextual window   Min(0) — the ONLY region that changes per screen:      │
│   TRACK (default)   selected track: display name — engine, its active      │
│                     page's params, live envelope/LFO for that voice        │
│   PARAM (Pg1–6)     page tabs + sub-page indicator (§0 A11), 8 encoder     │
│                     cells (2×4), live env gauge, LFO phase — as TK2 C9     │
│                     already renders them, re-fitted to the new region      │
│   CHAIN / TEMPO / SETTINGS   as shipped in TK2 C6                          │
│                                                                            │
├ track indicator  1 line   ▸1 Kick   2 Snare   3 HiHat●   4 Bass    P1 ─────┤
│ trig strip       2 lines  selected track only, 8 cells per line            │
├ key legend       2 lines  [key] NAME chips, fixed position, never scrolls  ┤
├ echo area        1 line   messages, confirms, `:` command line ────────────┤
└ status line      1 line   screen · track · REC · armed prefix · slots ─────┘
```

The **status line is the mode line** §5.1's skeleton calls for and the
legibility contract s1/s2 established: it keeps the armed TRK/PTN prefix
(`render.rs:635` today — load-bearing under D11), step focus, slot
bindings and page position. The legend is a separate, static teaching
strip, not a status display. Nothing that TK2 renders today loses its home:
page tabs and the `¶n/m` sub-page indicator (§0 A11) stay inside the Param
contextual window.

The trig strip and track indicator are **persistent**: they render on
every screen. Session #2's "trigs disappearing in encoder mode" is a
layout bug under this decision, not a mode consequence.

### D2 — The strip shows one track

The trig strip renders the **selected track's** 16-step window as 2×8
cells (top line steps 1–8, bottom 9–16), keeping the shipped colour/state
rules (playhead, active, locked, focused) and the `page_window` stride
(`render.rs:307`) while re-cutting the cell body to make room for D3's
chips — the current 7-column `" ████ "` block run has no space for one.

Cross-track information is carried by the one-line track indicator: name,
selection marker, mute state, and the active pattern. When more tracks
exist than fit, the indicator windows around the selected track with `‹`/`›`
markers rather than wrapping or truncating silently.

### D3 — The key chip is drawn where the key acts

Every cell a physical key currently addresses carries that key's
character, resolved through the live `Keymap` (user binding wins;
otherwise the built-in table). When a mode changes what the trig keys
address, the chips move with the meaning:

| State | Chips on the track indicator | Chips on the step cells | Chips on encoder cells |
|---|---|---|---|
| `RecMode::Off` / `Live` on a non-Param screen (pads) | **yes** — `[q]1 Kick` | dimmed (display-only) | — |
| `RecMode::Grid` on a non-Param screen | — | **yes** — `[q]▓` | — |
| Param screen (any rec mode) | — | dimmed (display-only) | **yes** |

The invariant is one sentence and one test: *a key chip appears on the
cell that key would act on if pressed right now, and nowhere else.* Two
consequences follow and are normative:

- **Shadow-awareness.** `key_to_button` consults user bindings *before*
  the built-in table (`input.rs:380-388`), so the reverse lookup must not
  offer a default-table key that a user binding has claimed for a
  different button. Binding `q → Play` must remove `q`'s chip from
  `Trig1`, not leave it lying.
- **No chip without an action.** A pad column past the discovered track
  count gets no chip and is a silent no-op — not an echo per keypress
  (on the default 4-track instrument that would be 12 keys echoing).
- **Chip casing is display-only.** `key_name` is the storage form and is
  lowercase (`"tab"`, `"esc"`, `"space"` — `input.rs:176-199`); chips
  title-case the multi-character names (`[Tab]`, `[Esc]`, `[Space]`) and
  leave single characters as typed (`[q]`). The keymap file format is
  untouched.

### D4 — A labeled legend strip, not a hint line

Two fixed lines of `[key] NAME` chips (bright key, dim label), screen-aware
content from a declared per-screen priority list, always in the same
place; on overflow it truncates from the tail of that list. It never
wraps, scrolls or moves. This replaces the grey run-on hint line
(`render.rs:221-238`).

Three legend entries are **literal, not `key_label`-derived**: `:`, `?`
and `^C` have no `PanelButton` (`input.rs:18-64`) and bypass the keymap
entirely (`lib.rs:410-420`). So does the range chip `[1-6] PAGE`. They are
declared constants in the legend table, and the spec says so rather than
implying the whole strip is remap-aware.

### D5 — `RecMode { Off, Grid, Live }`; REC toggles, REC+PLAY escalates

`grid_rec: bool` becomes a three-state mode, default **`Off`**.

| Mode | Indicator | Trig keys | Transport interaction |
|---|---|---|---|
| `Off` (default) | `REC○` | pads (D6) | none |
| `Grid` | `REC▦` | write/clear steps of the selected track | none — programming works while playing |
| `Live` | `REC●` | pads **and** record, engine-side (D8) | records only while playing |

The gestures follow the reference box exactly:

- **REC toggles `Off ↔ Grid`**, and acts on press — grid recording arms
  or disarms immediately, with no dependence on what is pressed next.
- **REC held + PLAY enters `Live`.** REC pressed again leaves it, back to
  `Off`. The transport never changes the rec mode by itself, and grid
  recording is fully usable while the pattern plays.

The three states are what the reference box has (grid rec lit, live rec
blinking, neither); only the physical gesture differs, and this decision
keeps that gesture too. Because REC's own action fires on press, making
REC a held prefix costs nothing: unlike TRK/PTN, it never has to wait for
the next key to know what it meant.

**Degradation without key-release reporting.** The held-REC chord needs
release events, which only the kitty keyboard protocol provides
(`supports_keyboard_enhancement()` probe at startup, already stored on the
model and shown on the Settings screen). Where they are absent, `Live` is
reached by a transport-derived rule instead: **REC pressed while the
transport is running arms `Live`; REC pressed while stopped arms `Grid`**,
and arming sticks — a later PLAY does *not* convert `Grid` into `Live`.
Same bindings, degraded gesture, in the spirit of §4.3's ramp
degradation. What is explicitly **not** done is inferring the chord from a
timing window between two sequential presses: "REC then PLAY" (program,
then start playback) and "REC+PLAY" (live record) are both real
workflows, and separating them by milliseconds would guess wrong under
exactly the pressure a session applies.

*(An earlier draft of this ADR had REC cycling `Off → Grid → Live → Off`
to avoid needing a chord at all. Withdrawn: cycling from `Grid` back to
`Off` passes **through** `Live` while the transport runs, so any trig in
that window records — a footgun in precisely the state the cycle existed
to simplify. It also conflicted with ADR-038's grid-rec toggle and
ADR-039's REC+PLAY grammar; this decision conflicts with neither.)*

### D6 — In pad modes, trig N addresses track N

In `Off` and `Live`, a trig press:

1. sounds track N live (`CMD_TRIG_NOW` on **that** track's sequencer —
   today's handler ignores the column, `lib.rs:757`), and
2. makes track N the selected track, so the contextual window, trig strip
   and track indicator all follow the finger.

The grammar addresses all 16 trigs; columns past the discovered track
count are silent no-ops with no chip (D3). In `Grid`, trig N is step N of
the selected track — unchanged.

**This is the reference box's own behaviour** *(confirmed by the user,
2026-07-27)*: from power-on with no recording armed, pressing trig keys
1, 2, 3 sounds tracks 1, 2, 3, and the display follows each press to that
track's values **for whatever page is currently showing**. Holding TRK and
pressing the same keys selects those tracks **silently**.

So the two gestures are not duplicates with a redundant hold — they are
the audible and silent forms of one selection:

| Gesture (REC off) | Selects | Sounds |
|---|---|---|
| trig | yes | **yes** |
| TRK + trig | yes | **no** |

The silent form is the one a performance needs: re-pointing the encoders
and the contextual window mid-pattern without adding a stray hit. Both are
normative here, with exactly those semantics. **ADR-038 structural change
2** ("track select is a hold-chord, not a row") therefore stands
unamended, and TRK gains a defined reason to exist beyond `Grid` mode
rather than reading as vestigial.

**Selection follows the press, and only the subject changes.** Whichever
gesture selects, the screen and page you are on are preserved — a pad
press on the FLTR page leaves you on the FLTR page, now showing the new
track's filter values. The contextual window's subject is the selected
track; nothing about *which* view is open changes underneath you.

It *is* a mode split in the trig keys, which §3.A point 3 warns about.
Accepted deliberately: the split is between playing and programming the
instrument, it is announced by the REC indicator, and D3's chips keep the
current meaning on screen rather than in memory. §0 A16 is superseded.

### D7 — No transport at launch

Theotokos issues `CMD_CLOCK_STOP` once during startup, so the instrument
boots silent. Scope note, per BUG-041: this stops the *clock* (no ticks are
emitted, `internal_clock.rs:216`), but until BUG-041 is fixed the
sequencers' own `playing` flag never clears, so nothing may be built on
"the sequencer knows it is stopped" (see D8).

The engine-side default that causes the auto-start
(`internal_clock.rs:82`) is **not** changed here: it is load-bearing for
`tools/test-driver` scenarios (none issues `clock_start`), the two
committed ADR-035 baselines that ride them, `paraclete-clap`'s subgraph
(`subgraph.rs:201`) and `main.rs`'s static snapshot. Filed as **BUG-039**
so the wider question — should any surface auto-start, and does the clock
node or the app decide — is owned rather than lost in prose.

### D8 — Live record is engine-side, per ADR-039 decision 7

`Live` does **not** compute steps on the surface. Entering `Live` sends
`CMD_SET_PARAM live_rec = 1` to every track sequencer; leaving it sends
`0`. Recording then happens inside the sequencer: while `live_rec ≥ 0.5`
and the transport is running, a consumed `CMD_TRIG_NOW` records itself —
nearest-step quantization, note and velocity written, signed distance to
the grid captured as the step's micro-timing.

This adopts ADR-039 decision 7 whole — its mechanics ("a pending
`CMD_TRIG_NOW` (TK2 C1) records itself the same way when `live_rec` is
on"), whose rejected alternative is precisely the surface-side
`CMD_SET_STEP` path this ADR first drafted, **and** its grammar clause,
since D5 now arms `Live` with REC+PLAY exactly as that ADR states.

TK2.1 therefore **implements one slice of ADR-039 early** — the `live_rec`
param and the record-on-live-trig path. Not pulled with it: kits, temp
save, mute tiers, CMD 39–45. `live_rec` is a record-arm, not a sound
parameter, so it must be excluded from kit membership when ADR-039
amendment 1's opt-in flag lands.

Two honest bounds:

- **Timing.** `CMD_TRIG_NOW` is drained at block start
  (`sequencer.rs:1298`), so recorded micro-timing is exact to the
  sequencer's tick position, not to the keystroke. Sub-block accuracy
  waits on the HAL timestamping work ADR-039 amendment 2 names as P11
  scope.
- **Stopped transport.** "A pad press in `Live` while stopped sounds the
  voice and records nothing" is only implementable once BUG-041 is fixed;
  the phase spec puts that fix in the same commit rather than asserting
  behaviour the engine cannot currently express.

### D9 — Encoder mode is the Param screen, not a new toggle key

Session #2 asked for a toggle key replacing held-FUNC for encoder access.
This grants the ergonomics without adding a key: **on the Param screen the
trig rows are the encoder bank** — bare top-row key *n* = encoder *n* up,
bottom-row = down, no modifier held. The screen is reached by `1`–`6` and
left by `Esc`, so the toggle already exists and is already learned.
`FUNC+trig` keeps working from every screen as the quick-access shortcut.

**§0 A10 is unchanged and wins:** encoder jog — bare-trig-on-Param
included — resolves **only with no armed prefix**. While TRK is armed, a
bare trig selects a track and `FUNC`+trig toggles mute, on *every* screen
including Param. Without this carve-out D12's "only mute gesture" would be
unreachable exactly where D10 wanted FUNC for coarse.

Re-entry nuance: pressing the *active* PG key again cycles its sub-page
(§0 A1/A11), so `1`–`6` re-entry is a sub-page gesture, not a screen
toggle; `Esc` is the exit.

Consequence: trigs do not audition or edit steps while Param is open. The
strip stays visible there (display-only, dimmed chips). HYPOTHESIS for
session #3.

This also discharges §0 A7's open condition: A7 permitted one shared held
modifier for the encoder bank *conditional on session evidence that
held-FUNC sweeps are acceptable*. Session #2 returned the opposite
("held-FUNC for encoder access feels wrong"), and bare-trig-on-Param
satisfies design.md §4.4's modifier-free floor directly — so the floor no
longer depends on the numpad slots, and OQ-T24 becomes a free choice at
session #3 rather than a constrained one.

### D10 — Jog magnitude comes from the real parameter descriptor

Encoder resolution stops inventing a range. `resolve_encoder_params` looks
each `(node_id, param_id)` up in `Model::caps` and carries the descriptor's
real `min`, `max` and `stepped`; the 0..1 placeholder (`model.rs:357`)
survives only as a last-resort fallback when no capability document
declares the param, and such a cell renders dimmed so the condition is
visible rather than silent.

**design.md §4.2's constants are unchanged** — the session evidence ("no
variable step size") is fully explained by the fake range, not by the
tiers. The shipped `Tuning` defaults stand: Normal `range/128`, Fine
`range/1024` (`fine_divisor 8`), Coarse `range/32` (`coarse_multiplier 4`),
ramp dwell 150 ms, ×1.05 capped at ×8 (`model.rs:896-909`). §4.2's
step-size scaler (OQ-T4) remains unimplemented and open — not deleted.

What changes is *bindings* and *stepped*:

| Magnitude | Binding | Note |
|---|---|---|
| Fine | `FUNC+Ctrl`+trig off Param; `Ctrl`+trig on Param | keeps fine on the FUNC plane where FUNC is what opens the plane (ADR-038 §3.A point 7) |
| Normal | bare trig on Param; `FUNC`+trig elsewhere | |
| Coarse | `FUNC`+trig on Param | `Mag::Coarse` already exists (`model.rs:33`, `:918`) and is simply never produced (`input.rs:612`) — this is its first binding |

`stepped` params ignore the table and jog by exactly **1** per press;
today they inherit whatever fake range the composite path supplied, so an
`lfo_shape` (0..4) moves ≈0.008 per press. The defect is filed as
**BUG-040**.

Interim-ownership note: **ADR-041 amendment 3 already assigns
`stepped`/`options` population to the composite/view layer** so machine
encoders can label variants. D10's local cap-doc lookup is the interim fix
for Theotokos only; when ADR-041's composite work lands, the local lookup
defers to it. Recorded so the two do not silently become two sources of
truth (AGENTS.md learnings 5 and 9).

### D11 — Sticky-prefix re-tap disarms, guarded by a repeat window

§0 A9 is **reversed**: a second press of an armed TRK/PTN prefix disarms
it. A9's underlying observation still holds — without kitty release events
OS auto-repeat is indistinguishable from a deliberate second tap — so the
toggle is guarded by time: a same-prefix press within `repeat_guard_ms`
(default **400** *(tunable)*) of the previous same-prefix press is treated
as auto-repeat and ignored; beyond it, it disarms. Holding TRK cannot flap
the armed state (repeats arrive every ~30 ms), while tap-pause-tap disarms
as session #2 expects. The kitty path (physical release) is untouched.
The clock is injected, following the existing `JogTracker::press/repeat(now,
tick_ms)` precedent (`model.rs:~940`).

### D12 — The Mute screen is retired (OQ-T22 resolved)

`Screen::Mute` and `PanelButton::Mute` are removed. `TRK`+`FUNC`+trig is
the only mute gesture (and per D9 it keeps working on every screen);
per-track mute state moves to the track indicator, which is visible on
every screen — strictly more available than the screen it replaces. `m`
becomes unbound and available to `:bind`. This supersedes TK2 §1 D12's
`Screen` enum and §1 D11's button-name list; §0 A14 goes half-stale
(MUTE was "a screen + the TRK-held chord" and is now only the chord).

### D13 — Step focus and p-lock authoring have no gesture, and this phase does not invent one

design.md §4 point 6 (DETERMINED) and TK2 §1 D8 both assume a focused step
that encoder jog can route a p-lock to. **No gesture reaches it today:**
`Action::FocusStep` lost its key at TK2 C3's wiring flip (`Enter` now
resolves to `Yes`) and is documented unreachable (`action.rs:41`), so the
encoder p-lock branch (`lib.rs:848`), `ClearAllLocks` (`lib.rs:672`) and
Backspace's screen-independent lock-clear are all dead paths.

D6 makes this sharper, not looser: in the default `Off` mode no trig
addresses a step at all, so even a hold-step gesture would need `Grid`
mode. This phase **states the gap and changes nothing** — reviving p-lock
authoring is a grammar decision that belongs with session #3's evidence,
not a side effect of a layout pass. The dead paths stay, documented, for
whichever commit gives them a gesture (OQ-T27). What the phase does owe:
the C7 doc sweep must not describe p-locks as reachable.

### D14 — Retired button names degrade a keymap, they do not reject it

`Keymap::from_yaml` currently fails the whole file on an unrecognized
button name (`input.rs:297`, propagated with `?`), so D12 would turn one
stale `m: Mute` line into "none of your bindings load". Unknown button and
key names become **warn-and-skip**: the binding is dropped, the rest of the
file loads, and the echo area reports what was skipped. Structurally
invalid YAML still fails. This changes shipped ADR-037 behaviour and is
therefore stated as a decision, not folded silently into a commit.

---

## Ratification questions

| # | Question | Recommendation |
|---|---|---|
| **R1** | D5 — on terminals with no key-release reporting, should `Live` degrade to the transport-derived rule (REC while running arms Live), or simply be unavailable with an echo? | Degrade; an unreachable mode teaches nothing, and the rule is honest about what it does |
| **R2** | D8 — pull ADR-039 decision 7's `live_rec` slice into TK2.1 (so `RecMode::Live` does something), versus shipping `Off`/`Grid` only and waiting for the P11 phase spec? | Pull it forward; the slice is small and fully specified, and a cycle with a dead third state is worse than either |
| **R3** | D12/D14 — remove `Mute` from the `:bind` vocabulary entirely, with warn-and-skip loading for keymaps that still name it? | Yes; a chord has no single-button equivalent, so an alias would be a lie |
| **R4** | ~~D6 — confirm the reference behaviour the decision rests on~~ | **Resolved 2026-07-27.** Bare trig = select **and** sound; TRK+trig = select **silently**; the display follows the press without changing which page is open. Folded into D6 |
| **R5** | Session #2 recorded FUNC+transport copy/clear/paste as "converged (provisional) … revisit inside the general redesign pass". This ADR does **not** redesign it — see "Out of scope". Accept the deferral to session #3? | Yes — the surrounding grammar changes under this ADR, so redesigning the chord now would be designing against a surface nobody has played |

## Alternatives considered

- **REC cycles `Off → Grid → Live → Off`** (drafted, then withdrawn — see
  D5): cycling out of `Grid` passes through `Live` while the transport
  runs, and it conflicted with two accepted ADRs for no gain.
- **Deriving `Live` from `rec_armed × playing` unconditionally** — i.e. a
  later PLAY converts `Grid` into `Live`. Rejected: it makes programming
  impossible while the pattern loops. The D5 fallback keeps the derivation
  but freezes it at arming time, which does not have that effect.
- **Inferring the chord from a timing window** between sequential REC and
  PLAY presses. Rejected under D5: two real workflows separated by
  milliseconds.
- **Keep trig = step in pad mode** (any trig sounds the selected track;
  track select stays on TRK+trig — today's behavior). Rejected by the user
  in this drafting pass: no multi-track finger drumming.
- **Split by row** (bottom row = track pads, top row = step audition).
  Rejected: a row asymmetry no other mode has.
- **A dedicated ENC toggle key.** Rejected under D9 — no free key with a
  good mnemonic, and the Param screen already is the mode.
- **New jog constants** (`range/64`/`/512`/`/16`). Drafted, then withdrawn
  under review: design.md §4.2 is DETERMINED, ADR-038 §3.B says §4
  mechanics apply "unchanged", and the session evidence points at the fake
  range, not the tiers.
- **Surface-side live record** (read `current_step`, send `CMD_SET_STEP`).
  Drafted, then withdrawn: ADR-039 decision 7 lists exactly this as its
  rejected alternative.
- **Change `InternalClock`'s `playing` default to `false`.** Rejected
  under D7 — it moves the regression baselines, every driver scenario and
  the CLAP subgraph at once. BUG-039 instead.
- **Extend `CompositeParam` with range metadata** (the cross-crate fix).
  Deferred under D10 — ADR-041 amendment 3 already owns that work; the
  local lookup is interim.

## Consequences

- `render.rs` gains a fixed-region layout, a strip renderer, a track
  indicator, a chip resolver and a legend builder; `render_seq_grid`,
  `render_track_row` and `render_mute_screen` are deleted.
- `Model.grid_rec: bool` → `Model.rec: RecMode`; `Screen::Mute` and
  `PanelButton::Mute` leave the model, the keymap vocabulary, the help
  overlay and the `:bind` docs.
- `TheotokosConfig` gains per-track display names: today `track_names`
  carries cap-doc type names (`AnalogKick`), while the human labels
  (`Kick`) are `display_name` on the sequencer nodes in `instrument.yaml`
  and were never plumbed through (`main.rs:390`).
- `paraclete-nodes/sequencer.rs` gains ADR-039 decision 7's `live_rec`
  param and record path; `internal_clock.rs` gains the missing
  `global_stop` emission (BUG-041). These are the only engine changes, and
  the sequencer one is P11 scope consumed early — the P11 spec inherits it
  as shipped rather than re-planning it.
- Three defects filed under the standing directive: **BUG-039** (engine
  transport auto-start), **BUG-040** (encoder jog range/`stepped`),
  **BUG-041** (`CMD_CLOCK_STOP` emits no transport event, so a sequencer's
  `playing` never clears in the standalone app).
- **BUG-038** (arrow-cursor nav + numpad slot jog speced but never wired)
  is touched by D9/D10's rewrite of the encoder path and must be either
  wired or formally descoped in the same phase — not left dangling.
- design.md §5.1/§5.2 are rewritten as DETERMINED against this ADR on
  ratification, and Stage 5 records the reopening's resolution.
- **Out of scope, still open:** FUNC+transport copy/clear/paste ergonomics
  (session #2 "crazy workflow" — deferred to session #3 per R5); OQ-T24
  (numpad cluster, now unconstrained per D9); OQ-T23's screen-gated tap
  tempo; `:`-remap discoverability; OQ-T21 (KEYBD chromatic); design.md
  §4.2's step-size scaler (OQ-T4).

## Test seams

Per design.md §6, everything except feel is machine-checkable:

- **Pure mapping:** rec-mode cycling, pad→track resolution and clamp,
  armed-prefix precedence over encoder resolution (A10), magnitudes,
  sticky re-tap vs. auto-repeat (injected clock).
- **Resolution:** descriptor-accurate `min`/`max`/`stepped` against fixture
  capability documents, including the composite path that produced the
  0..1 placeholder; shadow-aware `key_label`.
- **Render:** `TestBackend` assertions for the one-track strip, chip
  placement per mode, legend chips, strip persistence on every screen.
- **Engine effect:** sequencer tests for `live_rec` and for `CMD_CLOCK_STOP`
  emitting `global_stop`; a `tools/test-driver` scenario (`set_param` and
  `trig_now` both exist — `main.rs:727`, `:809` — but its assertion
  vocabulary is numeric/audio only, so the scenario asserts audibly, not on
  the text step bitfield).
- **Feel:** usability session #3.

## Revision — 2026-07-27 (post-review, user-directed)

D5 was rewritten from the REC cycle to the reference box's own gestures
(REC toggles grid rec; REC held + PLAY escalates to live rec), with a
transport-derived fallback where key releases are unavailable. This
**removes** the review's two largest findings rather than answering them:
nothing supersedes ADR-038's grid-rec toggle or ADR-039's REC+PLAY
grammar any more, so R1 narrows to the fallback choice. D6's rationale
was corrected in the same pass — pad mode promotes the reference's own
TRK-held track layer to the REC-off default, so it converges with the
reference instead of diverging from it, and ADR-038 structural change 2
stands unamended.

## Review pass — 2026-07-27

Three independent fresh-context reviewers (code claims / design
consistency / implementability), per AGENTS.md learning 8. **15 B, 26 M,
27 m; 49+ code claims verified clean.** All blockers and majors are folded
above or into the phase spec. What changed materially in this ADR: the
REC-cycle became a contested ratification item (later resolved by dropping
the cycle — see the revision note above); §0 A10's precedence was restored over
D9/D10; §4.2's jog constants were restored; D3 gained shadow-awareness and
the no-chip-without-action rule; D4 declared its non-derivable entries; D9
discharged §0 A7; D10 recorded ADR-041 amendment 3's ownership; D13 (the
dead p-lock path) and D14 were added; BUG-041 was filed and D7/D8's stopped-transport claims were bounded
by it; the dropped FUNC+transport session verdict became R5.

## Cross-references

- `design/sessions/theotokos-2.md`, `design/phases/tk2-report.md` — evidence
- `design/phases/tk2.1-theotokos.md` — the commit blueprint
- ADR-036 (Theotokos), ADR-037 (key remapping), ADR-038 (Elektron
  convergence), ADR-039 (performance state — live record), ADR-041
  (machine identity — composite `stepped` ownership)
- design.md §3.A, §4.2/§4.4, §5.1/§5.2 (reopened), §6 (convergence rule)
