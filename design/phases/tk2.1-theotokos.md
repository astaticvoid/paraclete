# Paraclete — TK2.1 Theotokos Specification (Panel Redesign)

> **DRAFT — 2026-07-27**, revised the same day after a three-domain
> hostile review. The redesign pass roadmap step 2.5 calls for.
> Written for implementation without further design decisions **once
> ADR-044's R1–R5 are answered** — R1 determines C1's no-kitty fallback,
> R2 determines whether C3 exists, R3 determines C6. Every commit names its files,
> contracts, the existing sites it must update to stay green, and its
> tests. Where a value is a tuning knob, the default is stated *(tunable)*.
>
> **Design authority:** ADR-044 (🟡 proposed — **do not start C0 until it
> is ratified**), `design/sessions/theotokos-2.md`,
> `design/phases/tk2-report.md`, design.md §3.A/§4/§5.1/§5.2/§6, and the
> TK2 spec's normative §0 (A10 and A11 remain in force; A9/A16 are
> superseded by ADR-044).
> **Baseline:** TK2 code-complete (C0–C9, `be565b9`; 96 crate tests green).
> **Exit:** `cargo test --workspace` green after every commit; agent smoke
> run at C7; **usability session #3 at C8** — notes in
> `design/sessions/theotokos-3.md`, report in
> `design/phases/tk2.1-report.md`.
>
> Third-party marks appear per house naming policy: design prose only,
> never identifiers or UI strings.

---

## §1. Scope

**In:** the panel layout (persistent one-track trig strip, track
indicator, contextual window, key chips, legend strip), per-track display
names, the rec-mode/pad model, engine-side live record, the missing
`global_stop` emission, encoder-mode access, descriptor-accurate jog,
sticky-prefix re-tap, Mute-screen retirement, keymap degradation, and
p-lock authoring's Theotokos-local half.

**Out, unchanged:** the input two-tier pipeline (`key_to_button` →
`HeldState` → `button_to_action`), TRK/PTN hold grammar and §0 A10's
precedence, the `:` command line and `Keymap` persistence, Tempo/Chain/
Settings screens, design.md §4.2's jog constants and its step-size scaler
(OQ-T4, still unimplemented), every engine node except the two named
changes in C3.

**Out, deferred with a citation:** FUNC+transport copy/clear/paste
ergonomics — session #2 recorded it "converged (provisional) … revisit
inside the general redesign pass" (`theotokos-2.md:25`). ADR-044 R5 asks
the user to confirm deferring it to session #3 instead, because the
grammar around it changes under this phase. It is **not** silently
dropped; it is a listed non-decision.

---

## §2. Decisions inherited from ADR-044

D1 fixed regions (status line = the mode line) · D2 one-track strip ·
D3 chip where the key acts, shadow-aware, none without an action ·
D4 legend strip with declared literal entries · D5 `RecMode{Off,Grid,Live}`,
REC toggles `Off↔Grid`, REC+PLAY escalates to `Live` *(fallback per R1)* ·
D6 trig N = track N in pad modes ·
D7 no transport at launch · D8 live record engine-side (`live_rec`,
ADR-039 D7) *(pending R2)* · D9 encoder access = an explicit ENC mode (`n`), §0 A10
intact · D15 p-lock via a shared lock target (`m`), momentary where
releases are reported · D10 descriptor-accurate jog, §4.2 constants unchanged ·
D11 sticky re-tap disarms behind a repeat guard · D12 Mute retired ·
D13 *(superseded by D15)* · D14 retired button names warn-and-skip.

---

## §3. Target render (the contract C0–C2 build toward)

```
 128.0 BPM  ■  REC○  Kick  P1  Step:1  Len:16                    ← transport
 Kick — AnalogKick                                               ← contextual
   tune  0.00 ▓▓░░   decay 0.32 ▓░░░   tone 0.41 ▓▓░░              window
   AMP ENV ▶ ████████░░░░░░░░
 ▸[q]1 Kick  [w]2 Snare  [e]3 HiHat●  [r]4 Bass          PTN P1  ← track line
  1-8 [q]▓ [w]░ [e]░ [r]▓ [t]░ [y]░ [u]░ [i]░                    ← trig strip
 9-16 [a]░ [s]░ [d]░ [f]░ [g]░ [h]░ [j]░ [k]░                      (2 lines)
 [Tab] TRK  [p] PTN  [z] REC  [x] PLAY  [c] STOP  [1-6] PAGE     ← legend
 [-/=] WIN  [o] SONG  [0] TEMPO  [Enter] YES  [:] CMD  [?] HELP    (2 lines)
 :                                                               ← echo
 GRID     Kick  REC○  TRK…  P1/1                                 ← status line
```

Shown in `RecMode::Off`: chips are bright on the track line (keys are
pads) and dimmed on the step cells. In `Grid` they swap. On Param they sit
on the encoder cells. Region heights never change.

**Exact cell formats** (so no implementer invents them):

| Element | Format | Width |
|---|---|---|
| Trig cell | `[k]g` + one space — `k` = chip char (or space when no chip), `g` = state glyph | 5 |
| Trig row | `%4s ` row label (` 1-8`, `9-16`) + 8 cells | 44 |
| Track entry | `▸` or ` `, then `[k]` (pad modes only), `N Name`, `●` if muted, two spaces | variable |
| Encoder cell | `[k]name bar value` inside the existing `Ratio(1,4)` cell | ≤20 |

State glyphs keep the shipped colour rules (`render.rs:332-350`): playhead
yellow, active+locked green, active cyan, locked white, empty dark gray —
`▓` active, `░` empty, focus/playhead carried by `Modifier::REVERSED`
rather than a wider block, since the 7-column `" ████ "` run leaves no room
for a chip. **Minimum width 60 columns**: below it the track line drops
names (keeping `[k]N`), and below 48 the strip drops its row labels. The
track line windows around the selected track with `‹`/`›` when the tracks
do not fit.

---

## §4. Commit sequence

Every commit: `cargo test --workspace` green, `cargo clippy --workspace`
clean on touched crates, no new deps, no audio-thread allocation. Each
commit compiles on its own — the "**Update to stay green**" list in each
commit is exhaustive as of `be565b9` and is part of that commit's scope,
not follow-up work. File references are current as of `be565b9`.

### C0 — Fixed regions, one-track strip, track indicator, display names

`crates/paraclete-app/src/main.rs` + `crates/paraclete-theotokos/src/lib.rs`:
`TheotokosConfig` gains `display_names: Vec<String>` — the sequencer
nodes' `display_name` from the instrument file (`instrument.yaml:24,27,30,33`
→ `Kick`/`Snare`/`HiHat`/`Bass`), which is **not** what `track_names`
carries today (that is the cap-doc type name, `AnalogKick`, via
`main.rs:390-397`). `collect_node_summaries` (`main.rs:755-762`) already
has the label-preference logic to copy. `Model::tracks` carries both;
the track line and contextual-window header use the display name, the
header's second half uses the engine name.

`render.rs`:

- `render()`'s `Layout::vertical` becomes seven constraints: `Length(1)`
  transport (it is `Length(2)` for one line today, `render.rs:95-102`),
  `Min(0)` contextual window, `Length(1)` track indicator, `Length(2)`
  trig strip, `Length(2)` legend, `Length(1)` echo, `Length(1)` status.
- New `render_track_indicator()` per §3, reading `track_names`/
  `display_names`, `mute_states`, `active_pattern`, `chain_len` — all
  already on `RenderData` (`render.rs:22-91`).
- New `render_trig_strip()` — the **selected track only**, two lines of 8
  cells per §3, keeping `render_track_row`'s colour/state rules and the
  `page_window * PAGE_SIZE * 2 + row_off` stride (`render.rs:307`) but
  re-cutting the cell body. Rendered from `render()` directly, on every
  screen, never from a screen branch.
- Deleted: `render_seq_grid` (`:272`), `render_track_row` (`:299` — it is
  not reusable at the new cell width), `render_mute_screen` (`:190`).
- `Screen::Grid` → new `render_track_context()`: header
  `{display_name} — {engine_name}`, then the **active page's** params
  (`resolve_encoder_params`, first 4 *(tunable)*) as name + bar + value,
  then the existing envelope section. When the track has no composite view
  and no `Rule` pagination the resolver returns empty (`model.rs:365`,
  `:369`) — render the header and the line `no page params` rather than an
  empty pane.
- The dispatch `match data.screen` is exhaustive (`render.rs:98-105`):
  `Screen::Mute` needs an explicit arm to `render_track_context` until C6
  deletes the variant.

**Update to stay green:** `grid_structure_4_tracks_23_rows`
(`render.rs:903`) — its name pins the stack C0 deletes; rename to
`strip_structure_is_two_rows` and re-assert. The five `RenderData`
construction sites (`render.rs:703` `for_test`, `:765`, `:827`, `:910`,
and `lib.rs:314`) gain the new field.

**Tests:** `trig_strip_renders_only_selected_track`,
`track_indicator_lists_tracks_with_mute_markers`,
`trig_strip_and_track_line_render_on_every_screen`,
`region_heights_are_identical_across_screens`,
`track_context_shows_display_name_and_engine_name`,
`track_context_without_page_params_renders_placeholder`,
`narrow_terminal_drops_names_before_chips`.

### C1 — `RecMode`, pads, silent launch (D5/D6/D7)

`model.rs`: `pub enum RecMode { Off, Grid, Live }`; `Model.grid_rec: bool`
→ `Model.rec: RecMode` (default `Off`, replacing `model.rs:204`'s `true`).
`input.rs::ScreenState.grid_rec` → `rec` (`input.rs:546`).

`action.rs`: `Action::ToggleGridRec` → `Action::ToggleRec`, plus
`Action::EnterLiveRec`.

`input.rs`: `Hold` gains a `Rec` variant, but **only on the kitty path**
(`on_kitty_press`/`on_kitty_release`, `input.rs:509-539`) — REC's own
action fires on press either way, so unlike TRK/PTN it never waits for the
next key and the sticky fallback never arms it. `PanelButton::Play` while
`Hold::Rec` is held resolves to `Action::EnterLiveRec` (which also starts
the transport, as PLAY otherwise would). Where the kitty probe is false,
`Action::ToggleRec` resolves by transport state instead
(D5's fallback): REC while running arms `Live`, REC while stopped arms
`Grid`, and a later PLAY does not convert one into the other.

`input.rs::button_to_action`: the trig arm (`input.rs:627-633`) branches on
`rec` — `Grid` → `ToggleStep { col }`; `Off`/`Live` → `LiveTrig { col }`
(the variant already declares `col`, `action.rs:53`). The `Screen::Mute`
retarget (`:624`) stays until C6. **§0 A10 is unchanged**: the armed-prefix
branch (`input.rs:582-593`) still precedes this one, so TRK+trig selects
and TRK+FUNC+trig mutes exactly as today.

`lib.rs`:

- `Action::LiveTrig { col }` (`lib.rs:757`) resolves track `col`: past the
  discovered track count → **silent no-op** (no echo — on a 4-track
  instrument 12 keys would otherwise echo per press; D3 also gives those
  columns no chip). Otherwise set `active_track = col`, set
  `selected_changed = true` (do **not** inline a `bus.borrow_mut()` — the
  `&*bus_ref` borrow spans the dispatch loop, `lib.rs:389`, and the
  publish already runs after `drop(bus_ref)`, `lib.rs:954-963`), and emit
  `CMD_TRIG_NOW` on **that** track's sequencer. Order is normative: the
  trig lands on the newly selected track.
- `Action::ToggleRec` toggles `Off ↔ Grid` on the kitty path, or applies
  the transport-derived rule on the fallback path; `Action::EnterLiveRec`
  sets `Live` and starts the transport; REC from `Live` returns to `Off`.
- `TheotokosApp::new` pushes `CMD_CLOCK_STOP` (`action.rs:5`) for
  `clock_id` onto `self.pending`, so the first command drain stops the
  clock. Note the honest bound: `main.rs:553-560` ticks before draining,
  so frame 1 still paints `playing = true`; "boots stopped" means from the
  first drain, not the first frame.
- Transport and status indicators (`render.rs:243`, `:622`) read
  `RecMode`: `REC○` dark gray / `REC▦` red / `REC●` bright red.

**Update to stay green** (every `grid_rec` reader; all are compile errors):
`model.rs:59,204` · `input.rs:546`, `:627-633`, `:719` (`default_grid()`
helper, used by 6 tests), `:845`, `:1002` (`rec_toggles_grid_recording` →
rename `rec_toggles_off_and_grid` and re-assert), `:1010`
(`trig_with_grid_rec_off_is_live_trig` → rename
`pad_mode_trig_resolves_to_live_trig_with_column`) · `render.rs:243`,
`:622-623`, `:705`, `:767`, `:829`, `:912`, `:980`, `:987` · `lib.rs:316`,
`:490`, `:768`, `:1659`, `:1666`
(`grid_rec_off_trig_key_emits_trig_now_command`) · `action.rs:119` (the
`Outcome` match arm).

**Tests** (pure): `rec_toggles_off_and_grid`,
`rec_held_plus_play_enters_live_rec` (kitty path),
`rec_from_live_returns_to_off`,
`grid_rec_survives_a_later_play_press` (the cycle's pass-through hazard,
pinned so it cannot come back),
`fallback_rec_while_running_arms_live` and
`fallback_rec_while_stopped_arms_grid` (no-kitty path),
`pad_mode_trig_resolves_to_live_trig_with_column`,
`grid_mode_trig_toggles_step`,
`armed_trk_still_wins_over_pads` (§0 A10 regression).
(injection): `pad_press_selects_track_and_trigs_that_track`,
`pad_beyond_track_count_is_silent`,
`new_pushes_clock_stop` — the injection suite builds apps via the
`test_app` struct literal (`lib.rs:1450-1487`), which bypasses
`TheotokosApp::new` (it runs `setup_keyboard_flags`, `lib.rs:1329`), so
this commit adds a `#[cfg(test)] fn startup_commands(clock_id) -> Vec<NodeCommand>`
that `new` calls and the test asserts on directly.
(render): `rec_indicator_shows_three_states` — a `TestBackend` test in
`render.rs`; `lib.rs` has no `TestBackend` seam.

### C2 — Key chips and the legend strip (D3/D4)

Ordered **after** C1 deliberately: D3's chip placement is keyed on
`RecMode`, and until C1 lands a `grid_rec == false` trig is a live trig on
the *selected* track, not a track pad — so chips on the track line would
violate D3's own invariant.

`input.rs`:

- New `const DEFAULT_BINDINGS: &[(KeyCode, PanelButton, bool)]` — the §2
  panel table as data, the `bool` marking the **preferred** key for a
  button that has several (`x` and `Space` both reach `Play`,
  `input.rs:404-408`; the chip must read `[x] PLAY`, so a lexicographic
  tie-break is wrong). `key_to_button`'s built-in fall-through
  (`input.rs:380-388`) reads the same table, so the two directions cannot
  drift.
- New `pub fn key_label(keymap: &Keymap, button: PanelButton) ->
  Option<String>`: a user binding for that button wins (lowest `key_name`
  among several); otherwise the preferred `DEFAULT_BINDINGS` key, **but
  only if no user binding has claimed that `KeyCode` for another button**
  (D3 shadow-awareness — `key_to_button` consults user bindings first, so
  a shadowed default key no longer reaches this button).

`render.rs`:

- `RenderData` gains `trig_key_labels: Vec<Option<String>>` (length 16)
  and `track_key_labels: Vec<Option<String>>` (length = track count),
  filled in `lib.rs` from `key_label` against the live `Keymap`.
- Chip placement per D3's table; dimmed chips use `Color::DarkGray`.
- `render_legend()` rewritten as `[key] NAME` chips from this literal
  per-screen priority list, truncating from the tail:

| Screen | Ordered chips |
|---|---|
| Grid (pads or grid-rec) | `[Tab] TRK`, `[p] PTN`, `[z] REC`, `[x] PLAY`, `[c] STOP`, `[1-6] PAGE`, `[-/=] WIN`, `[o] SONG`, `[0] TEMPO`, `[8] SET`, `[Enter] YES`, `[Esc] NO`, `[:] CMD`, `[?] HELP` |
| Param, ENC off | `[n] ENC`, `[m] LOCK`, `[1-6] PAGE`, `[Esc] BACK`, `[Tab] TRK`, `[z] REC`, `[x] PLAY`, `[:] CMD`, `[?] HELP` |
| Any screen, ENC on | `trigs ENCODER ±`, `[Ctrl] FINE`, `[FUNC] COARSE`, `[n] ENC off`, `[m] LOCK`, `[Esc] BACK`, `[:] CMD`, `[?] HELP` |
| Chain | `[Enter] PUSH`, `[Esc] CLEAR`, `[←/→] CURSOR`, `[o] SONG`, `[:] CMD`, `[?] HELP` |
| Tempo | `[Enter] TAP`, `[↑/↓] ±1`, `[FUNC+↑/↓] ±0.1`, `[Esc] BACK`, `[?] HELP` |
| Settings | `[Esc] BACK`, `[?] HELP` |

- **Declared literals, not `key_label` output:** `[:] CMD`, `[?] HELP`,
  `[^C] QUIT`, the range chip `[1-6] PAGE`, and the `trigs`/`[FUNC]`
  entries. None has a `PanelButton` (`input.rs:18-64`); they are
  hardcoded raw-key checks (`lib.rs:410-420`) and are not remappable. The
  legend table marks them so, and a test pins that they are not claimed to
  be dynamic.

**Update to stay green:** the five `RenderData` construction sites listed
in C0.

**Tests:** `key_label_prefers_user_binding`,
`key_label_falls_back_to_preferred_default`,
`key_label_skips_default_key_shadowed_by_user_binding`,
`default_bindings_match_key_to_button` (drift guard, both directions),
`play_chip_is_x_not_space`, `strip_cells_show_key_chips_in_grid_mode`,
`chips_move_to_track_line_in_pad_mode`,
`pad_column_without_a_track_has_no_chip`,
`legend_renders_labeled_chips`, `legend_literal_entries_are_not_derived`,
`chip_titlecases_named_keys_only` (D3 — `[Tab]`, not `[tab]`; `[q]`, not
`[Q]`; the keymap file format is untouched).

### C3 — Live record + the missing stop signal (D8) *(depends on R2)*

The phase's only engine work, and **P11 scope pulled forward**: ADR-039
decision 7 owns live record and names the surface-side `CMD_SET_STEP` path
as its rejected alternative. Implement that decision's slice only — no
kits, no temp save, no mute tiers, none of the reserved CMD 39–45.

**C3a — BUG-041, the stop signal.** `crates/paraclete-nodes/src/internal_clock.rs`:
`CMD_CLOCK_STOP` currently sets `playing = false` (`:146-148`) and
`process` then returns early (`:216`), emitting nothing — so
`Sequencer.playing`, which clears only on `flags.global_stop`
(`sequencer.rs:908`), never clears in the standalone app (the only
`global_stop` emitters in tree are the CLAP bridges,
`paraclete-clap/src/subgraph.rs:252`, `transport.rs:102`). Emit one
transport event carrying `global_stop` on the transition to stopped,
before the early return. Without this, C3b's gate is permanently open and
its unit test would still pass, because sequencer tests inject
`global_stop` directly (`sequencer.rs:2080`).

**C3b — `live_rec`.** `crates/paraclete-nodes/src/sequencer.rs`:

- New `live_rec` bank param, trig-gate shape, modelled on `mute`
  (`sequencer.rs:1262-1271`). It is a record-arm, not a sound param —
  exclude it from kit membership when ADR-039 amendment 1's opt-in flag
  lands (noted in the report so P11 inherits it).
- While `live_rec ≥ 0.5` and `self.playing`, the consumed
  `pending_live_trig` (`sequencer.rs:294`, taken at `:1323`) records
  itself into the active pattern. **Formula, stated so nobody invents
  one:** with `pos` = the sequencer's tick position within the pattern
  (`current_step`/`step_tick`, `:216-217`) and `period` = the
  speed-scaled ticks-per-step the existing step-advance path computes —
  `nearest = round(pos / period) mod pattern_length`;
  `delta_ticks = pos − nearest × period`;
  `micro = clamp(round(delta_ticks / (TICKS_PER_BEAT / 96)), −47, 47)`.
  `StepTiming::micro_offset` is in **1/96-beat units**, not ticks
  (`sequencer.rs:12-16`); with `TICKS_PER_BEAT = 960` one unit is 10
  ticks. Write step-active, note and velocity via the existing
  `set_step` path (`:572-580`) and `steps[idx].timing.micro_offset`
  (`:635`) — inline scalar fields, no allocation.
- Stopped transport records nothing; the trig still sounds (`is_muted()`
  already gates `emit_live_trig`, `:1155`).
- Known bound for the report: the pending trig is taken at the top of
  `process` (`:1323`), before this block's transport events are handled
  (`:1338`), so `pos` can lag by up to one block — and `CMD_TRIG_NOW` is
  drained at block start regardless (ADR-039 amendment 2's HAL
  timestamping is P11 scope).

`crates/paraclete-theotokos/src/lib.rs`: entering `RecMode::Live` sends
`CMD_SET_PARAM live_rec = 1.0` to every track sequencer; leaving it sends
`0.0`. No step computation anywhere on the surface.

`tools/test-driver`: scenario `sequencer_live_rec.yaml` using the existing
`set_param`/`trig_now` actions (timeline path: `scenario.rs:47`, `:136`;
`main.rs:1011`, `:1137`). Note the assertion vocabulary is numeric
state-bus + audio only (`scenario.rs:252-270`) and the step state
publishes as `Text`, so the scenario asserts the *audible* consequence —
`peak_gte` on the next loop pass, with an explicit window — not the step
bitfield. Crate tests carry the structural assertions.

**Tests** (clock): `clock_stop_emits_global_stop`,
`sequencer_playing_clears_on_clock_stop` (the app-shaped regression
BUG-041 describes).
(sequencer): `live_rec_records_trig_now_at_nearest_step`,
`live_rec_writes_micro_timing_in_96th_units`,
`live_rec_ignores_trig_now_while_stopped`,
`live_rec_off_leaves_pattern_untouched`,
`live_rec_trig_still_sounds_when_recording`.
(theotokos): `entering_live_arms_live_rec_on_every_track`,
`leaving_live_disarms_live_rec`.

### C4 — Descriptor-accurate encoder resolution (D10, closes BUG-040)

`model.rs::resolve_encoder_params` (`:343`): the composite branch stops
returning the `0.0, 1.0` placeholder (`:357`). Each `(node_id, param_id)`
is looked up in `Model::caps` — verified to contain every node in the
instrument file, not just generators (`main.rs:265`, `builder.rs:193`,
`main.rs:408`) — for the descriptor's real `min`, `max` and `stepped`.
Unresolvable params keep 0..1 and are flagged. The returned tuple grows
`stepped: bool` and `resolved: bool`.

`model.rs::Tuning::jog_step` gains a `stepped` short-circuit: stepped
params move exactly 1 per press, ignoring range, magnitude and ramp.
**§4.2's constants are unchanged** — Normal `range/128`, Fine
`range/1024`, Coarse `range/32`, dwell 150 ms, ×1.05 capped ×8
(`model.rs:896-909`) all stand; ADR-044 D10 withdrew the drafted new
divisors.

`render_encoder_cell` renders an unresolved cell dimmed.

**Update to stay green:** `resolve_encoder_params` call sites `lib.rs:261`
and `:827`; `jog_step` call sites `lib.rs:577`, `:624`, `:843` and the
eight `Tuning` tests (`model.rs:974-1015`) if the signature takes
`stepped` (prefer a separate `jog_step_stepped` helper to leave those
tests untouched — implementer's choice, stated either way in the report);
the two `EncoderCell` literals (`render.rs:1018`, `:1024`) if the dim flag
lands on that struct.

**Tests:** `encoder_uses_descriptor_range_on_composite_pages` (fixture
must add a composite view — `test_app` passes `composite: vec![]`,
`lib.rs:1465`), `stepped_param_jogs_by_exactly_one`,
`unresolvable_param_falls_back_and_renders_dim`,
`plock_clamp_uses_real_range` (the `lib.rs:856` truncation BUG-040 §1
describes).

### C5 — ENC mode + p-lock target (D9/D15)

**C5a — ENC mode.** `model.rs`: `Model.enc: bool`, default false.
`input.rs`: a new `PanelButton::Enc` (default key `n`) toggles it;
`PanelButton::Lock` (default key `m`, free since D12 retires the Mute
screen) drives C5b. Both join `BUTTON_NAMES` so they are remappable
(ADR-037). While `enc` is true **and no prefix is armed (§0 A10)**, a bare
trig resolves to `EncoderJog { col: col % 8, dir, mag }` — top row `Next`,
bottom `Prev` — on **any** screen, not only Param. While `enc` is false,
trigs are pads or steps per D5/D6, on any screen including Param.

Magnitudes: in ENC mode, bare = `Normal`, `Ctrl` = `Fine`, `FUNC` =
`Coarse` (the first producer of the already-existing `Mag::Coarse`,
`model.rs:33`, `:918`; never constructed today, `input.rs:612`). Outside
ENC mode `FUNC`+trig = `Normal` and `FUNC+Ctrl` = `Fine`, exactly as TK2
§1 D8 has it. The status line shows `ENC` when on.

The Param contextual window is **unchanged from TK2 C9** apart from C0's
region re-fit: `render_perf_window` (`render.rs:356-371`) already renders
page tabs with the §0 A11 sub-page indicator (`:427-452`), the encoder
bank, the live envelope gauge and the LFO phase track, and
`param_screen_animates_envelope_and_lfo` (`render.rs:1052`) covers the
live half. What changes is that reaching a knob no longer depends on which
screen is open.

**C5b — the lock target.** `model.rs`: `lock_target: Option<(usize,
usize)>` (track, step), published as `/script/theotokos/lock_step`
alongside the existing `/script/theotokos/selected` publish
(`lib.rs:955-963` — set `selected_changed`-style flags, never borrow the
bus inside the dispatch loop).

- **Latched:** `PanelButton::Lock` arms "the next trig sets the target";
  the following trig in `Grid` mode sets `(active_track, step)`. The same
  key again, `Esc`, or re-pressing that trig clears it. This is the path
  that works with no release reporting and the path that works in ENC
  mode — arm the step, toggle ENC, jog.
- **Momentary:** where the kitty probe is true, holding a trig in `Grid`
  mode sets the target for the duration of the hold (`HeldState`'s
  `on_kitty_press`/`on_kitty_release` already track physical state;
  extend `pressed` to carry trig buttons, which today it deliberately does
  not — `input.rs:509-539`).

Value routing: while `lock_target` is `Some`, Theotokos's own parameter
motion — ENC jog, numpad slots, `:set` — routes to
`CMD_SET_LOCK_TARGET`/`CMD_SET_STEP_LOCK` (33/34) on that track's
sequencer instead of the live bank. The code path already exists and is
currently unreachable (`lib.rs:848-868`, the `step_focus` branch); this
commit re-points it at `lock_target` and deletes the dead `step_focus`
field rather than keeping two notions of the same thing. `ClearAllLocks`
(`lib.rs:672`) and Backspace regain their meaning against the target.

**Out of scope, by ADR-044 R6:** capturing parameter writes that arrive
from *other* surfaces (a MIDI controller's encoders while the step is held
here) — that rewrites every surface's mutation path and needs its own ADR.
C5b is the half that makes the keyboard workflow whole and publishes the
state the cross-surface half will consume.

**Tests:** `enc_toggle_switches_trig_rows`,
`enc_mode_works_on_grid_screen_not_only_param`,
`param_screen_with_enc_off_still_pads` (D6 invariant),
`enc_bare_trig_does_not_jog_while_trk_armed` (§0 A10 regression),
`enc_func_is_coarse_ctrl_is_fine`, `off_enc_fine_is_func_ctrl`,
`lock_key_then_trig_sets_lock_target`,
`lock_target_clears_on_esc_and_on_retap`,
`jog_with_lock_target_emits_lock_pair_not_bump`,
`jog_without_lock_target_emits_bump`,
`kitty_trig_hold_sets_lock_target_for_the_hold`,
`lock_target_is_published_to_the_bus`.

### C6 — Sticky re-tap, Mute retirement, keymap degradation (D11/D12/D14)

`input.rs::HeldState`: `on_press` gains an injected `now` (following
`JogTracker::press/repeat(now, tick_ms)`, `model.rs:~940`) and a
`last_prefix_press` field. An already-armed same prefix disarms, unless
the press is within `repeat_guard_ms` (**400** *(tunable)*) of the previous
same-prefix press, which is treated as OS auto-repeat and ignored. Kitty
path untouched.

`PanelButton::Mute`, `Screen::Mute` and the `("Mute", …)` entry
(`input.rs:108`) are deleted, with the `m` binding, the Mute arm in
`button_to_action` (`:664`), the trig retarget (`:624`), and the help and
legend entries. Mute state already lives on the track indicator (C0);
`TRK`+`FUNC`+trig is unchanged.

D14: `Keymap::from_yaml` (`input.rs:297`) stops propagating unknown key
and button names with `?`. Unknown entries are skipped, the rest of the
file loads, and the skipped names are reported through `cmdline_status`.
Structurally invalid YAML still fails. Without this, one stale `m: Mute`
line would reject a user's entire keymap.

**Update to stay green:** `render.rs:213` (`screen_name`'s Mute arm),
`:692` (the `Tempo | Chain | Settings | Mute` status arm), C0's temporary
`Screen::Mute` dispatch arm · `input.rs:841-849`
(`mute_screen_trigs_toggle_mutes` — delete), `:886-900`
(`sticky_prefix_same_key_is_a_noop_per_a9` — delete; it asserts exactly
what D11 reverses), `:910` (`sticky_prefix_same_key_toggles_off` —
rewrite to assert the toggle rather than delegate), `:1080-1082`
(`keymap_roundtrips_yaml` — use `Song`), and the `on_press` call sites
`:876`, `:878`, `:892`, `:893`, `:916`, `:917`, `:925`, `:926` ·
`lib.rs:476` (`on_press` call site), `:2282`
(`esc_returns_to_grid_from_other_screens` — drop Mute from the iteration),
`:2656` and `:2831` (`:bind w mute` → `:bind w song`), `:2790`
(`PanelButton::Mute` → `Song`). Keep the existing
`trk_func_trig_toggles_mute` (`input.rs:823`) — do not duplicate it under
a new name.

**Tests:** `sticky_prefix_retap_disarms`,
`sticky_prefix_autorepeat_within_guard_does_not_disarm`,
`sticky_prefix_esc_still_disarms`,
`mute_button_name_is_rejected_by_bind`,
`keymap_with_retired_button_name_skips_only_that_binding` (D14),
`mute_state_visible_on_track_indicator`.

### C7 — Docs, smoke gate, BUG-038 disposition

No new features. Run the app on the default 4-track instrument, fix paper
cuts the suites cannot see (the TK2 C7 precedent), file engine issues per
the standing directive. Then:

- The doc sweep documents the p-lock gesture D15 introduces, and drops
  every reference to the retired `step_focus` model from `AGENTS.md`, the
  help overlay and the README.
- **BUG-038 must be resolved or formally descoped in this commit.** C4/C5
  rewrite the encoder path and make the trig rows the encoder bank, which
  raises the visibility of the cursor that never moves
  (`encoder_cursor` stuck at 0) and of the unwired numpad slot jog. Wire
  the arrow cursor, or descope it in `bugs.md` and drop the language from
  the spec — do not leave it dangling a second phase.
- `AGENTS.md` + `README.md` key tables → the TK2.1 grammar (REC toggle +
  REC+PLAY live rec, pads, `n` = ENC, `m` = LOCK, no Mute screen).
- Help overlay (`render.rs:488`) regenerated: rec modes, pads, encoder
  mode, live record, no Mute screen.
- design.md §5.1/§5.2 rewritten as DETERMINED against ADR-044; Stage 5
  records the resolution; the TK2 spec's §0 gains an appended note that
  A9 and A16 are superseded, A14 is half-stale, and A7's condition is
  discharged by D9 (append, never rewrite).
- ADR-044 `Status:` → ✅ accepted with an implementation note; ADR-039
  gains an appended note that its decision-7 slice shipped early (its
  REC+PLAY grammar is implemented as written, not superseded).
- `design/phases/tk2.1-report.md` + roadmap.

### C8 — Usability session #3 (user-paired, no code)

Produces `design/sessions/theotokos-3.md` + `tk2.1-report.md`, with an
explicit converged / revise / park verdict per hypothesis.

---

## §5. Hypotheses under test in session #3

| Hypothesis | Source |
|---|---|
| The fixed panel (one-track strip + separate contextual window + legend) reads as a hardware front panel, not an LED grid | ADR-044 D1/D2 |
| Key chips make the current meaning of the trig rows self-evident without the help overlay | D3 |
| REC-toggle + REC+PLAY reads as the reference box's own grammar on a keyboard, and the no-kitty fallback (REC-while-running arms `Live`) is tolerable where releases are unavailable | D5, R1 |
| Trig N = track N in pad modes gives real finger-drumming without a mode error | D6, R4 |
| Engine-side live record feels tight enough at 120–140 bpm with block-start command delivery | D8 |
| An explicit ENC mode (`n`) beats both held-FUNC and screen-as-mode, and leaves pads reachable on every screen | D9 |
| P-lock authoring works: latched target + ENC jog on the keyboard, and momentary hold where releases are reported | D15 |
| Descriptor-accurate jog (incl. stepped = 1) fixes "no variable step size" without changing §4.2's constants | D10 |
| Sticky re-tap with a 400 ms repeat guard behaves as the hand expects | D11 |
| FUNC+transport copy/clear/paste ergonomics, deferred here per R5 | session #2 |
| **Carried from session #2, re-judged under the new layout:** TRK/PTN physical feel; encoder-bank simultaneity; numpad cluster fate (OQ-T24, now unconstrained since D9 discharges §0 A7) | session #2 parked items |

## §6. Open questions

| # | Question | Status |
|---|---|---|
| OQ-T24 | Numpad slot cluster fate | OPEN — session #3; D9 discharges §0 A7's modifier-floor condition, so this is now a free choice |
| OQ-T23b | Tap tempo behind a screen is friction — global chord? | OPEN — session #3 |
| OQ-T25 | Live-record erase gesture (ADR-039 lists "hold NO?") | OPEN — P11 phase spec |
| OQ-T27 | ~~P-lock authoring has no gesture~~ | **Resolved by ADR-044 D15** — shared lock target, latched or momentary; C5b implements the Theotokos-local half |
| OQ-T28 | Cross-surface lock capture: a MIDI controller's encoders writing locks while the step is held on the keyboard or a Launchpad pad (ADR-044 R6) | OPEN — needs its own ADR; also gated on the unverified relative-CC assumption (`handoff.md`) |
| OQ-T4 | design.md §4.2's step-size scaler, still unimplemented | OPEN — unchanged by this phase |
| OQ-T21 | KEYBD chromatic grammar | OPEN — TK3 |
| OQ-T12 | WT convergence | OPEN — after session #3 (three sessions held) |

## §7. Review pass — 2026-07-27

Three independent fresh-context reviewers (code claims / design
consistency / implementability), per AGENTS.md learning 8. **15 B, 26 M,
27 m; 49+ code claims verified clean.** **Superseded in part the same day:** D5's REC cycle was withdrawn at the
user's direction in favour of the reference box's own gestures (REC
toggles grid rec; REC held + PLAY escalates), which removes the review's
two largest findings outright rather than answering them — see ADR-044's
revision note. Folded here: the commit sequence was re-cut (chips now follow `RecMode`; the old C4 split into C4/C5);
every rename and deletion gained an exhaustive "update to stay green"
list; the legend priority list, cell formats, minimum width and
quantization formula are now literal; `startup_emits_clock_stop` gained a
real seam; BUG-041 was found and folded into C3; §4.2's constants were
restored; §0 A10's precedence was restored over C5; BUG-038 got a deadline;
D13 recorded the dead step-focus/p-lock path rather than leaving §4 point 6
reading as live, and D14 was added so a retired button name cannot reject a
user's keymap.
