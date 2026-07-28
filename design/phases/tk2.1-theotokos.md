# Paraclete — TK2.1 Theotokos Specification (Panel Redesign)

> **DRAFT — 2026-07-27.** The redesign pass roadmap step 2.5 calls for.
> Written for implementation **without further design decisions**: every
> commit names its files, contracts, and tests. Where a value is a tuning
> knob, the default is stated and marked *(tunable)*.
>
> **Design authority:** ADR-044 (🟡 proposed — this spec does not start
> until it is ratified), `design/sessions/theotokos-2.md`,
> `design/phases/tk2-report.md`, design.md §3.A/§5.1/§5.2/§6.
> **Baseline:** TK2 code-complete (C0–C9, `be565b9`; 96 crate tests green).
> **Exit:** `cargo test --workspace` green after every commit; agent smoke
> run at C6; **usability session #3 at C7** — notes in
> `design/sessions/theotokos-3.md`, report in
> `design/phases/tk2.1-report.md`.
>
> Third-party marks appear per house naming policy: design prose only,
> never identifiers or UI strings.

---

## §1. Scope

In: the panel layout (persistent one-track trig strip, track indicator,
contextual window, key chips, legend strip), the rec-mode/pad model,
live record, encoder-mode access, descriptor-accurate jog, sticky-prefix
re-tap, Mute-screen retirement.

Out (unchanged by this phase): the input two-tier pipeline
(`key_to_button` → `HeldState` → `button_to_action`), TRK/PTN hold
grammar, FUNC+transport copy/clear/paste, the `:` command line and
`Keymap` persistence, Tempo/Chain/Settings screens, and every engine node
except the sequencer's `live_rec` slice in C3 (ADR-039 decision 7, pulled
forward — see that commit for what is explicitly *not* pulled with it).

Still open after this phase: OQ-T24 (numpad cluster), OQ-T23's
screen-gated tap tempo, OQ-T21 (KEYBD), `:`-remap discoverability.

---

## §2. Decisions inherited from ADR-044

D1 fixed regions · D2 one-track strip · D3 key chip where the key acts ·
D4 legend strip · D5 `RecMode{Off,Grid,Live}`, REC cycles · D6 trig N =
track N in pad modes · D7 no transport at launch · D8 live record is
engine-side (`live_rec`, ADR-039 D7) · D9 encoder mode = the Param
screen · D10 jog from
the real descriptor · D11 sticky re-tap disarms behind a repeat guard ·
D12 Mute screen retired.

Two of these reverse earlier normative text and must be reflected in the
docs they contradict (C6): **§0 A9** (sticky re-press was a no-op) and
**§0 A16** (trigs are always trigs on Grid *and Param*).

---

## §3. Target render (the contract C0–C1 build toward)

```
 128.0 BPM  ■  REC○  Kick  P1  Step:1  Len:16                    ← transport
                                                                 ─────────────
 Kick — AnalogEngine                                             ← contextual
   tune 0.50 ▓▓▓░  decay 0.32 ▓▓░░  drive 0.10 ▓░░░               window
   AMP ENV ▶ ████████░░░░░░░░                                     (per screen)
                                                                 ─────────────
 TRK [q]1 Kick  [w]2 Snare  [e]3 HiHat●  [r]4 Bass      PTN P1   ← track line
  1-8  [q]▓ [w]░ [e]░ [r]▓ [t]░ [y]░ [u]░ [i]░                   ← trig strip
  9-16 [a]░ [s]░ [d]░ [f]░ [g]░ [h]░ [j]░ [k]░                     (2 lines)
                                                                 ─────────────
 [Tab] TRK   [p] PTN   [z] REC   [x] PLAY   [c] STOP   [1-6] PAGE ← legend
 [o] SONG   [Enter] YES   [Esc] NO/BACK   [:] CMD   [?] HELP        (2 lines)
 :                                                               ← echo
 GRID      Kick  REC○  P1/1                                      ← status line
```

Above, `RecMode::Off` — the chips sit on the track line (keys are pads)
and the step cells' chips are dimmed. In `RecMode::Grid` the chips swap:
bright on the step cells, absent from the track line. On the Param screen
they sit on the encoder cells. Region heights never change.

---

## §4. Commit sequence

Every commit: `cargo test --workspace` green, `cargo clippy --workspace`
clean on touched crates, no new deps, no audio-thread I/O. Each commit
compiles on its own — where a rename would break callers mid-sequence the
step that renames also updates them (per the standing per-commit
compilability rule). File references are current as of `be565b9`.

### C0 — Fixed regions, one-track strip, track indicator (render only)

`crates/paraclete-theotokos/src/render.rs`:

- `render()`'s `Layout::vertical` becomes seven constraints:
  `Length(1)` transport, `Min(0)` contextual window, `Length(1)` track
  indicator, `Length(2)` trig strip, `Length(2)` legend, `Length(1)` echo,
  `Length(1)` status. (Transport is `Length(2)` today for a single line —
  `render.rs:95-102`.)
- New `render_track_indicator()` — one line: per track, selection marker
  `▸`, index, name, mute dot (`●` muted / nothing when live), then the
  active pattern and, when `chain_len > 0`, `▸chain N`. Data already
  exists on `RenderData` (`track_names`, `mute_states`, `active_pattern`,
  `chain_len`).
- New `render_trig_strip()` — the **selected track only**, two lines of 8
  cells, reusing `render_track_row`'s existing glyph/colour rules
  (playhead, active, locked, focused) and the `page_window` stride. It
  renders on **every** screen, from `render()` directly, never from a
  screen branch.
- `render_seq_grid()` (`render.rs:272-297`) and `render_mute_screen()`
  (`render.rs:190-204`) are deleted. `Screen::Grid`'s contextual window
  becomes `render_track_context()`: the selected track's display name,
  its active page's params (name + bar + value, reusing
  `render_encoder_cell`'s cell shape), and the existing envelope section.
- `Screen::Mute`'s match arm is deleted; `Screen::Mute` itself goes in C5
  (it is still constructible until `PanelButton::Mute` is removed) — until
  then it falls through to `render_track_context`.

**Tests** (`render.rs` `TestBackend`):
`trig_strip_renders_only_selected_track`,
`track_indicator_lists_tracks_with_mute_markers`,
`trig_strip_and_track_line_render_on_every_screen`,
`region_heights_are_identical_across_screens`,
`grid_context_window_shows_selected_track_name`.

### C1 — Key chips and the legend strip

`crates/paraclete-theotokos/src/input.rs`:

- New `const DEFAULT_BINDINGS: &[(KeyCode, PanelButton)]` — the §2 panel
  table as data. `key_to_button`'s built-in fall-through and the new
  reverse lookup both read it, so the two directions cannot drift (the
  A17 lesson: grep for duplicated constants).
- New `pub fn key_label(keymap: &Keymap, button: PanelButton) ->
  Option<String>` — user bindings first (when several keys map to one
  button, the lowest `key_name` wins, so output is deterministic), else
  `DEFAULT_BINDINGS`, else `None`.

`render.rs`:

- `RenderData` gains `trig_key_labels: Vec<Option<String>>` (16) and
  `track_key_labels: Vec<Option<String>>`, filled by `lib.rs` from
  `key_label` against the live `Keymap`.
- Chip placement per ADR-044 D3: bright chips on the cells the keys act on
  right now, dimmed on the display-only ones.
- `render_legend()` is rewritten as `[key] NAME` chips built from a
  screen-aware priority list; on overflow it truncates from the tail of
  that list. No wrapping, no scrolling, fixed two lines.

**Tests:** `key_label_prefers_user_binding`,
`key_label_falls_back_to_default_table`,
`default_bindings_match_key_to_button` (drift guard, both directions),
`strip_cells_show_key_chips`, `legend_renders_labeled_chips`.

### C2 — `RecMode`, pads, silent launch (D5/D6/D7)

`model.rs`: `pub enum RecMode { Off, Grid, Live }`; `Model.grid_rec: bool`
→ `Model.rec: RecMode` (default `Off`, replacing `model.rs:204`'s `true`);
`input.rs::ScreenState.grid_rec` → `rec`.

`action.rs`: `Action::ToggleGridRec` → `Action::CycleRecMode`.
`Action::LiveTrig { col }`'s `col` becomes meaningful.

`input.rs::button_to_action`: the trig arm branches on `rec` —
`Grid` → `ToggleStep { col }`; `Off`/`Live` → `LiveTrig { col }`. The
`Screen::Mute` retarget stays until C5.

`lib.rs`:

- `Action::LiveTrig { col }` (`lib.rs:757`) resolves track `col`: past the
  discovered track count → `no track N` echo, no command (TK2 D9's clamp
  wording, reused). Otherwise set `active_track = col` (with the existing
  `/script/theotokos/selected` publish, OQ-T9) **and** emit `CMD_TRIG_NOW`
  on *that* track's sequencer.
- Startup emits `CMD_CLOCK_STOP` (`action.rs:5`) to `clock_id` once,
  before the first render, so the instrument boots stopped (D7).
- `Action::CycleRecMode` advances `Off → Grid → Live → Off`.
- Transport indicators (`render.rs:243`, `:622`) read `RecMode`:
  `REC○`/`REC▦`/`REC●`, dim/red/bright-red.

**Tests** (pure): `rec_cycles_off_grid_live`,
`pad_mode_trig_resolves_to_live_trig_with_column`,
`grid_mode_trig_toggles_step`.
(injection): `pad_press_selects_track_and_trigs_that_track`,
`pad_beyond_track_count_echoes_no_track`,
`startup_emits_clock_stop`, `rec_indicator_shows_three_states`.

### C3 — Live record, engine-side (D8 — ADR-039 decision 7)

This is the phase's only engine change, and it is **P11 scope pulled
forward**: ADR-039 (accepted 2026-07-23) decision 7 already owns live
record and names the surface-side `CMD_SET_STEP` path as its rejected
alternative. Implement that decision's slice only — no kits, no temp
save, no mute tiers, none of the reserved CMD 39–45.

`crates/paraclete-nodes/src/sequencer.rs`:

- New `live_rec` bank param (trig-gate shape, same as `mute`).
- While `live_rec ≥ 0.5` **and** the transport is playing, a consumed
  pending live trig (`CMD_TRIG_NOW`, TK2 C1 — `sequencer.rs`'s
  `live_gate_samples_left` path) records itself into the active pattern:
  nearest step activated, note and velocity written, signed distance to
  the grid stored as that step's micro-timing (`CMD_SET_STEP_TIMING`'s
  ±47-tick field). Stopped transport records nothing; the trig still
  sounds. Pattern writes stay on the existing step-mutation code paths —
  no allocation on the audio thread.
- Timing bound to state in the report, per ADR-039's own amendment 2:
  `CMD_TRIG_NOW` is drained at block start, so the captured offset is
  exact to the sequencer's tick position, not to the keystroke. Sub-block
  accuracy waits on the HAL timestamping work ADR-039 names as P11 scope.

`crates/paraclete-theotokos/src/lib.rs`: entering `RecMode::Live` sends
`CMD_SET_PARAM live_rec = 1.0` to every track sequencer; leaving it sends
`0.0`. No step computation anywhere on the surface.

`tools/test-driver`: scenario `sequencer_live_rec.yaml` using the existing
`set_param` and `trig_now` actions (`tools/test-driver/src/main.rs:750`,
`:809`) — no driver changes needed.

**Tests** (sequencer): `live_rec_records_trig_now_at_nearest_step`,
`live_rec_writes_micro_timing_offset`,
`live_rec_ignores_trig_now_while_stopped`,
`live_rec_off_leaves_pattern_untouched`,
`live_rec_trig_still_sounds_when_recording`.
(theotokos): `entering_live_arms_live_rec_on_every_track`,
`leaving_live_disarms_live_rec`.

### C4 — Encoder mode on Param + descriptor-accurate jog (D9/D10)

`input.rs`: on `Screen::Param(_)`, a bare trig resolves to
`EncoderJog { col: col % 8, dir, mag }` (top row `Next`, bottom `Prev`) —
no FUNC needed. Magnitude: `Ctrl` → `Fine`, `FUNC` → `Coarse` (new
`Mag::Coarse` binding on this screen), otherwise `Normal`. `FUNC`+trig on
every other screen keeps today's meaning. §0 A16's Param half is
superseded (record it in C6's doc sweep).

`model.rs::resolve_encoder_params` (`model.rs:343`): the composite branch
stops returning the `0.0, 1.0` placeholder (`model.rs:357`). Each
`(node_id, param_id)` is looked up in `Model::caps` — already keyed by
arbitrary node id (`model.rs:747`) — and carries the descriptor's real
`min`, `max` and `stepped`. Unresolvable params keep 0..1 **and** are
flagged so `render_encoder_cell` can dim them. The return tuple grows a
`stepped: bool` field; both call sites (`lib.rs:827`, the render path)
update in this commit.

`model.rs::Tuning`: `jog_step` gains the coarse tier and a `stepped`
short-circuit — `stepped` params move exactly 1 per press, ignoring range,
ramp and magnitude. Normal `range/64`, fine `range/512`, coarse
`range/16` *(tunable)*.

Contextual window on Param: the 8 encoder cells plus the C9 live envelope
gauge and LFO phase track, so live visualization sits inside the encoder
view (session #2 finding H4).

**Tests:** `param_screen_bare_trig_jogs_encoder`,
`func_trig_still_jogs_from_grid_screen`,
`param_func_is_coarse_ctrl_is_fine`,
`encoder_uses_descriptor_range_on_composite_pages`,
`stepped_param_jogs_by_exactly_one`,
`unresolvable_param_falls_back_and_renders_dim`,
`param_window_shows_encoders_with_env_and_lfo`.

### C5 — Sticky re-tap, Mute retirement (D11/D12)

`input.rs::HeldState`: `on_press` for an already-armed same prefix
disarms, unless the press arrives within `repeat_guard_ms` (default
**400** *(tunable)*) of the previous same-prefix press, which is treated
as OS auto-repeat and ignored. `HeldState` gains a `last_prefix_press:
Option<Instant>`-shaped field; the clock is injected (the existing jog
trackers already take `now`/`tick_ms` — follow that pattern) so the
behaviour is testable without sleeping. Kitty path untouched.

`PanelButton::Mute`, `Screen::Mute` and their `BUTTON_NAMES` entry
(`input.rs:126`) are deleted, with the `m` binding, the Mute arm in
`button_to_action` (`input.rs:664`), the trig retarget (`input.rs:624`)
and the help/legend entries. Mute state is already on the track indicator
(C0). A `:bind`/`from_yaml` file naming `Mute` now errors with the
existing `unknown button` message — covered by a test, since existing
`keymap.yaml` files may name it.

**Tests:** `sticky_prefix_retap_disarms`,
`sticky_prefix_autorepeat_within_guard_does_not_disarm`,
`sticky_prefix_esc_still_disarms`,
`mute_button_name_is_rejected_by_bind`,
`mute_state_visible_on_track_indicator`,
`trk_func_trig_still_toggles_mute` (the surviving path).

### C6 — Docs + smoke gate

No new features. Run the app on the default 4-track instrument, fix paper
cuts the test suites cannot see (the C7 precedent), file engine issues in
`design/bugs.md` per the standing directive. Doc sweep:

- `AGENTS.md` + `README.md` key tables → the TK2.1 grammar (rec cycle,
  pads, encoder-on-Param, no `m`).
- Help overlay (`render.rs:488`) regenerated: rec modes, pads, encoder
  mode, floor-quantized live record, no Mute screen.
- design.md §5.1/§5.2 rewritten as DETERMINED against ADR-044; Stage 5
  note records the reopening's resolution; §0 A9/A16 marked superseded in
  the TK2 spec's amendment section (append, don't rewrite).
- ADR-044 `Status:` → ✅ accepted with an implementation note.
- `design/phases/tk2.1-report.md` + roadmap.

### C7 — Usability session #3 (user-paired, no code)

Held after C6. Produces `design/sessions/theotokos-3.md` +
`tk2.1-report.md`, with an explicit converged / revise / park verdict per
hypothesis and any grammar re-cuts.

---

## §5. Hypotheses under test in session #3

| Hypothesis | Source |
|---|---|
| The fixed panel (one-track strip + separate contextual window + legend) reads as a hardware front panel, not an LED grid | ADR-044 D1/D2 |
| Key chips make the current meaning of the trig rows self-evident without the help overlay | D3 |
| REC cycling `Off → Grid → Live` is learnable and beats a PLAY+REC chord | D5 |
| Trig N = track N in pad modes gives real finger-drumming without a mode error | D6 |
| Engine-side live record (ADR-039 D7) feels tight enough at 120–140 bpm with block-start command delivery | D8 |
| The Param screen *is* the encoder mode — no separate toggle key is missed | D9 |
| Descriptor-accurate jog (incl. stepped = 1) fixes "no variable step size" | D10 |
| Sticky re-tap with a 400 ms repeat guard behaves as the hand expects | D11 |
| **Carried from session #2, re-judged under the new layout:** TRK/PTN physical feel; encoder-bank simultaneity; numpad cluster fate (OQ-T24) | session #2 parked items |

## §6. Open questions

| # | Question | Status |
|---|---|---|
| OQ-T24 | Numpad slot cluster fate | OPEN — session #3, now judgeable against a stable layout |
| OQ-T23b | Tap tempo behind a screen is friction — global chord? | OPEN — session #3 |
| OQ-T25 | Live-record erase gesture (ADR-039 lists "hold NO?" as an open UX question) | OPEN — P11 phase spec, unless session #3 forces it sooner |
| OQ-T26 | Should pad mode address tracks beyond 8 (two rows of pads) once instruments exceed 8 tracks? | OPEN — no instrument reaches it today |
| OQ-T21 | KEYBD chromatic grammar | OPEN — TK3 |
| OQ-T12 | WT convergence | OPEN — after session #3 (three sessions held) |
