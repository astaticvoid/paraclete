# Paraclete — TK2 Theotokos Specification (Elektron Convergence)

> **FULL SPEC — 2026-07-23.** Supersedes the 2026-07-22 stub and its
> 2026-07-23 re-cut. Written for implementation **without further design
> decisions**: every commit names its files, contracts, and tests. Where a
> value is a tuning knob, the default is stated and marked *(tunable)*.
>
> **Design authority:** ADR-038 (✅ accepted 2026-07-23, D1–D4),
> `design/theotokos/design.md` Stage 3 + §4 mechanics, ADR-036, ADR-037
> (key remapping — re-based by ADR-038; ratifies with C8 landing).
> **Baseline:** TK1 code complete (C0–C7; 60 crate tests green).
> **Exit:** `cargo test --workspace` green after every commit; agent
> smoke-run after C7; **usability session #2 after C8 (D4)** — notes in
> `design/sessions/theotokos-2.md`, report in
> `design/phases/tk2-report.md`.
>
> Third-party marks appear per house naming policy: design prose only,
> never identifiers or UI strings.

---

## §0. Hostile-review amendments (2026-07-23) — NORMATIVE

> Post-ratification hostile review (three subagents, findings graded
> B/M/m; reports summarized in the roadmap). **Where this section
> conflicts with any D/C text below, this section wins.** BUG-035/036
> filed under the standing defect directive.

- **A1 (B) Key normalization.** crossterm never delivers `Shift+letter`
  as lowercase+SHIFT: legacy input sends the uppercase char (+SHIFT);
  kitty alternate-keys sends uppercase with SHIFT *cleared*; shifted
  punctuation/digits arrive as the shifted symbol with **no** SHIFT
  flag. `key_to_button` MUST case-fold `Char(c)` and infer FUNC from
  case for letters. **FUNC+digit chords are dropped entirely**
  (layout-dependent symbols): sub-page overflow is now *press the
  active PG key again* to toggle sub-page 2 *(HYPOTHESIS — session)*.
  C2 tests must feed `Char('Q')+SHIFT` and `Char('Q')`-no-SHIFT
  variants, never synthetic lowercase+SHIFT (the BUG-035 false-pass
  class).
- **A2 (B) Kitty flags.** `REPORT_EVENT_TYPES` alone yields **no
  release events for text keys** (Tab, `p`, all trigs). C3 must push
  `DISAMBIGUATE_ESCAPE_CODES | REPORT_EVENT_TYPES |
  REPORT_ALL_KEYS_AS_ESCAPE_CODES`. D6's "already enabled" is wrong.
- **A3 (B) Live-trig gate (D5).** Gate close is transport-tick-driven
  today; a stopped clock emits no ticks — the specced note would ring
  forever. The live variant uses a **sample-counted gate** decremented
  in `process()` by buffer length, independent of transport; and emits
  `emit_note_off` first when `gate_open` (a live trig over a sounding
  step must not orphan its note-off).
- **A4 (B) C2 restructure.** C2 is **additive only** (new types beside
  old; no deletions) — deleting `Mode`/`map_seq`/actions breaks
  `lib.rs`/`render.rs` compilation. All deletions move to C3 with the
  wiring flip.
- **A5 (B) Numpad slots (D13).** Numpad digits are indistinguishable
  from the number row without `DISAMBIGUATE_ESCAPE_CODES` +
  `KeyEventState::KEYPAD`. Slot jog is **gated on the kitty probe +
  KEYPAD flag**; on terminals without it, slots are unavailable (the
  encoder bank is the jog path) — never ambiguous with KIT/SETTINGS/
  SAMPLING/PG keys.
- **A6 (B) The `:` binding (D14).** Legacy terminals deliver typing `:`
  as `Char(':')` with no SHIFT — the TK1 arm (`Char(';')+SHIFT`) may be
  dead on real terminals (BUG-036). Bind the `:` line on **`Char(':')`
  any-modifiers** (keep the legacy `';'+SHIFT` arm as an alias); the
  D14 unbindable entry is `Char(':')`.
- **A7 (M) §4.4 floor reconciliation** (user-approved 2026-07-23): one
  shared held modifier is permitted for the encoder bank; the
  modifier-free two-slot floor is carried by the numpad slots while
  they exist. OQ-T24 (numpad retirement) is **conditional on session
  evidence that held-FUNC sweeps are acceptable**. Recorded in
  `design/theotokos/design.md` amendment log.
- **A8 (M) C4 clear.** There is no "TK1 clear-lane pair": engine
  `CMD_CLEAR` clears steps only, locks survive. FUNC+PLAY emits
  `CMD_CLEAR` **plus `CMD_CLEAR_STEP_LOCK` per step** of the pattern
  length.
- **A9 (M) Sticky-prefix flap.** OS auto-repeat streams Press events;
  same-prefix re-press while armed is a **no-op** (not a toggle).
  Disarm only via Esc, a completed chord, or a non-trig key.
- **A10 (M) Chord precedence.** While TRK is held/armed: plain trig =
  select track; FUNC+trig = mute toggle. Encoder jog resolves **only
  with no armed prefix**. Sticky TRK explicitly supports the FUNC+trig
  mute chord before disarming.
- **A11 (M) Pages over 8 params.** The composite merge can exceed 8
  params per page (multi-node FX pages do today). The view layer splits
  pages at 8 into sub-pages reached via the A1 gesture; C5's render
  must show a sub-page indicator.
- **A12 (M) `Space` alias is transport-only.** FUNC+Space is a no-op;
  destructive clear requires the `x` home (and Shift+Space typically
  arrives as plain `Char(' ')` anyway — A1).
- **A13 (m)** `t` was never a shipped binding (design-only) — dropped
  from the removed-bindings lists; the C2 dead-bindings test is
  respecced as "old TK1 *actions* are unmapped; the keys resolve to
  their new buttons".
- **A14 (m)** MUTE is not a hold — strike it from the D6/ADR-038 hold
  lists (it is a screen + the TRK-held chord).
- **A15 (m)** FUNC is the fixed Shift modifier, **not** a `PanelButton`
  variant, and is unbindable (D11/D14 clarified).
- **A16 (m)** "Trigs are always trigs" is qualified: on Grid and Param
  screens; holds and the Mute screen retarget them deliberately.
- **A17 (C0 field report)** C0's "web needs no change" was wrong —
  `PageNav.tsx` carried its own hardcoded page order (fixed in
  `040a45f`). Lesson recorded: grep for duplicated constants before
  asserting "no change needed".

---

## §1. Decisions frozen by this spec (D5–D14)

ADR-038 ratification fixed D1–D4. This spec fixes the remaining
implementation-level decisions. None are open; sessions may revise them
*after* TK2 with evidence.

- **D5 — Live-trig command** *(amended — §0 A3)*.** `Sequencer::CMD_TRIG_NOW = 38`.
  `arg0` = MIDI note (`0` → default `60`, the `Step::empty()` note);
  `arg1` = velocity `0.0..=1.0` (`0.0` → default `0.5`, matching
  `Step::empty()`'s `32768/65535`). Handling: `handle_commands` stores a
  pending live trig (note, velocity); at the start of the next `process`
  window (sample offset 0) the sequencer emits a note-on via a live
  variant of `emit_note_on_at` — same Midi2 build, gate length
  `0.75 × step_period` (the `Step::empty()` length), increments
  `trig_count`, emits `DebugEventKind::StepFired` with step index `-1`
  (live marker). Pattern state is untouched. Respects `is_muted()`.
  Works with transport stopped (process ticks regardless). One pending
  trig per track per window; a second command in the same window
  replaces the first.
- **D6 — Hold semantics** *(amended — §0 A2/A9/A10)*.** With kitty release events (already enabled via
  `REPORT_EVENT_TYPES`, `lib.rs::setup_keyboard_flags`): TRK/PTN/MUTE-
  chord holds arm on press, disarm on release; every trig pressed while
  held chords. Without kitty (`supports_keyboard_enhancement()` probe at
  startup): **sticky one-shot** — press arms the prefix (mode line shows
  `TRK…`), next trig chords and disarms; pressing the same prefix key
  again disarms (toggle); `Esc` disarms; any non-trig key disarms and is
  then processed normally. **No timeout.** FUNC = Shift works identically
  everywhere (modifier flags are per-event).
- **D7 — Copy/clear/paste scope.** FUNC+REC = copy the **active track's
  active pattern lane** (TK1 yank semantics, including the known
  lossy-via-bus caveat); FUNC+PLAY = clear the active track's pattern
  (TK1 clear-lane pair: `CMD_CLEAR` + lock clears); FUNC+STOP = paste
  (TK1 paste semantics: steps, velocity, length, condition, timing,
  lock remap). Multi-track / whole-pattern operations stay on the `:`
  line. While TRK or PTN is held, FUNC+transport is a no-op (reserved).
- **D8 — Encoder bank.** The active param page's params in `Rule` order,
  first 8, are encoders 1–8. FUNC+top-row key *n* = encoder *n* up;
  FUNC+bottom-row key *n* = down. Column > param count → no-op + echo
  `no encoder N on this page`. Magnitudes: FUNC = normal,
  FUNC+Ctrl = fine (Mag::Fine). Jog step, ramp, and acceleration
  constants are design.md §4 unchanged *(tunable)*. While a step is
  focused, encoder jog routes to that step's p-lock (existing TK1
  step-focus path, CMD 33/34).
- **D9 — Chord clamps.** PTN+trig col ≥ `PATTERN_BANK_SIZE` (8) → no-op
  + echo `no pattern N`. TRK+trig col ≥ discovered track count → no-op +
  echo `no track N`. (Grammar addresses 16 for future banks/tracks;
  engine bank stays 8 in TK2.)
- **D10 — No inert state ships.** The engine live-trig command (C1)
  lands *before* the REC toggle (C6), so REC-off trigs sound from the
  first build that has them.
- **D11 — Keymap re-base (resolves ADR-037 OQ-T15/T19).** User bindings
  are `HashMap<KeyBinding, PanelButton>` — **global, flat; no per-screen
  bindings**. Screens consume buttons, never raw keys. `:` verbs:
  `:bind <key> <button>`, `:unbind <key>`, `:list-bindings`,
  `:reset-bindings`, `:save-bindings`, `:load-bindings`. Button names
  are the `PanelButton` variant names (`Trig1`…`Trig16`, `Trk`, `Ptn`,
  `Rec`, `Play`, `Stop`, `Pg1`…`Pg6`, `Kit`, `Settings`, `Sampling`,
  `Tempo`, `Yes`, `No`, `Up`, `Down`, `Left`, `Right`, `PagePrev`,
  `PageNext`, `Song`, `Keybd`, `Mute`), case-insensitive.
- **D12 — Screen model.** `enum Screen { Grid, Param(usize), Tempo,
  Chain, Settings, Mute }` replaces `Mode`. Plus `grid_rec: bool`
  (default **true** — TK1 behavior preserved on day one) and a
  `HeldState` (armed prefix + pressed set). KIT and SAMPLING have **no
  screens in TK2**: the buttons echo `reserved (kit)` / SAMPLING is
  hidden entirely unless the cap-doc set declares a sampling capability
  (none does today). SONG opens the Chain screen. Settings shows bpm,
  kitty status, track/pattern counts, version — read-only in TK2.
- **D13 — Numpad and arrows** *(amended — §0 A5)*.** Arrows become panel UP/DOWN/LEFT/RIGHT
  (navigation: encoder cursor on Param, chain lane on Chain, list on
  Settings). Slot jog moves **exclusively to the numpad**: `7/1` = slot
  A, `8/2` = B, `9/3` = C, live on Grid and Param screens; slots
  auto-bind to the active page's first params on page select (existing
  TK1 behavior, extended to slot C). No numpad → the encoder bank is the
  jog path. Numpad-`0` ALT layer and fills stay **out of TK2**
  (OQ-T24 — session #2 decides the cluster's fate first).
- **D14 — Remap guardrails** *(amended — §0 A6/A15; resolves ADR-037 OQ-T16/T17/T18)*.**
  `Ctrl-C` (quit) and `Shift+;` (`:` line) are **unbindable** — `:bind`
  on them errors. `:reset-bindings` clears *all* user bindings (full
  fall-through to defaults). **No auto-save**; only explicit
  `:save-bindings` writes `~/.config/paraclete/keymap.yaml` (local
  `./keymap.yaml` still overrides on load). With the leader key retired
  there is no other chicken-and-egg entry point.

Dropped from the former stub: touch-type split preset (ADR-038 D3), temp
save/reload (P11 dependency — deferred to TK3 with P11), numpad-less
fallback layer (the encoder bank *is* keyboard-native), ALT layer (D13).

---

## §2. Target key table (continuous grid — the only built-in)

| Keys | PanelButton | Notes |
|---|---|---|
| `q w e r t y u i` | Trig1–8 | top trig row |
| `a s d f g h j k` | Trig9–16 | bottom trig row |
| `Shift` (modifier) | FUNC | encoder plane + secondary chords |
| `Tab` (hold/sticky) | TRK | +trig = select track |
| `p` (hold/sticky) | PTN | +trig = select pattern |
| `z` / `x` / `c` | REC / PLAY / STOP | `Space` = PLAY alias; FUNC+z/x/c = copy/clear/paste (D7) |
| `1`–`6` | Pg1–Pg6 | pages in canonical order TRIG SRC FLTR AMP FX MOD |
| `7` / `8` / `9` / `0` | KIT / SETTINGS / SAMPLING / TEMPO | KIT+SAMPLING reserved/gated (D12) |
| `Enter` / `Esc` | YES / NO | YES taps on Tempo screen |
| arrows | UP/DOWN/LEFT/RIGHT | navigation (D13) |
| `-` / `=` | PagePrev / PageNext | step-page window |
| `o` | SONG | opens Chain screen |
| `m` | MUTE | Mute screen; quick chord TRK-held + FUNC+trig |
| `v` | KEYBD | echoes `reserved (chromatic)` — OQ-T21, TK3 |
| numpad `7/1 8/2 9/3` | slot A/B/C jog | D13; `Ctrl` = fine |
| `Shift+;` | `:` line | unbindable (D14) |
| `?`, `Backspace`, `Ctrl-C` | help, lock-clears, quit | unchanged from TK1 |

**Removed TK1 bindings** (delete, don't alias): `qweruiop` track row,
`Tab` mode cycle, number-row pattern select, `t` tap, `y`/`Y`
yank/paste, `\` leader + `LeaderState`, Shift+track mute, arrow-key
slot jog.

---

## §3. Commit sequence

Every commit: `cargo test --workspace` green, no new deps, no
audio-thread I/O. File references are current as of TK1 HEAD.

### C0 — Canonical page order (D2b)
`paraclete-view-assembly/src/lib.rs:22`: `CANONICAL_PAGE_ORDER` →
`["TRIG", "SRC", "FLTR", "AMP", "FX", "MOD"]`. Fix every test that
asserts the old order (view-assembly, antiphon, theotokos). Web client
needs no change (order arrives via protocol).
**Tests:** `canonical_order_is_trig_first` (new);
existing composite-page tests updated.

### C1 — Live-trig engine command (D5)
`paraclete-nodes/src/sequencer.rs`: `CMD_TRIG_NOW = 38`, pending-trig
field, live emit at window start. `tools/test-driver`: new `trig_now`
action mirroring the command.
**Tests (sequencer):** `trig_now_emits_note_on_next_window`,
`trig_now_uses_default_note_and_velocity_when_zero`,
`trig_now_respects_mute`, `trig_now_works_while_stopped`,
`trig_now_second_command_replaces_pending`,
`trig_now_does_not_modify_pattern`.

### C2 — Panel model (pure types + mapping) *(amended — §0 A1/A4/A13: additive only, deletions move to C3)*
`paraclete-theotokos/src/input.rs` rewritten two-tier:
`enum PanelButton` (§2 list); `fn key_to_button(&Keymap, KeyEvent) ->
Option<PanelButton>` (user bindings then the §2 table);
`struct HeldState { kitty: bool, armed: Option<Hold>, pressed: … }`
with `Hold = Trk | Ptn` and D6 semantics;
`fn button_to_action(&HeldState, &ScreenState, PanelButton, mods) ->
Action`. `model.rs`: `Screen` enum + `grid_rec` replace `Mode` (D12);
`Action` gains `SelectPattern(usize)`, `LiveTrig { col }`,
`EncoderJog { col, dir, mag }`, `ToggleGridRec`, `OpenScreen(Screen)`,
`TapTempo`; loses `CycleMode`, `Yank`/`Paste` (renamed
`CopyLane`/`PasteLane`), `Leader`. Old `map_seq`/`map_perf`/
`TRACK_KEYS` deleted. **This commit does not touch `lib.rs` wiring** —
both layers coexist behind the old entry point until C3.
**Tests (pure, no terminal):**
`continuous_grid_maps_sixteen_trigs`,
`trk_hold_plus_trig_selects_track`,
`ptn_hold_plus_trig_selects_pattern`,
`sticky_prefix_one_shot_then_disarms`,
`sticky_prefix_same_key_toggles_off`,
`sticky_prefix_esc_disarms`,
`nontrig_key_disarms_and_processes`,
`func_top_row_is_encoder_up_bottom_row_down`,
`rec_toggles_grid_recording`,
`trig_with_grid_rec_off_is_live_trig`,
`removed_tk1_bindings_are_dead` (t/y/`\`/Tab-cycle/number-patterns).

### C3 — Wiring + render migration
`lib.rs::handle_keys` consumes the C2 pipeline; release events feed
`HeldState` (extend `is_press_or_repeat` filtering); kitty probe at
startup stored on the model. `render.rs`: mode line → **status line**
(screen name, track, REC ●/○, armed prefix, encoder/slot bindings);
help overlay regenerated from the §2 table; transport bar gains REC
indicator. `:` fuzzy index entries for retired verbs updated
(`mode` verb removed; `pattern`/`track` verbs kept).
**Tests:** TestBackend: `status_line_shows_rec_state_and_armed_prefix`,
`help_overlay_lists_panel_buttons`; injection:
`select_track_via_trk_chord_publishes_selected`,
`grid_rec_off_trig_key_emits_trig_now_command` (uses C1 constant),
`pattern_chord_clamps_with_echo` (D9).

### C4 — FUNC transport chords + mute chord *(amended — §0 A8/A10/A12)*
Copy/clear/paste rewired per D7 onto FUNC+REC/PLAY/STOP (reusing TK1
`yank_active_pattern`/`paste_pattern`/clear-lane); TRK-held +
FUNC+trig = mute toggle (existing sequencer `mute` param path); Mute
screen (`m`): trigs toggle mutes, track states rendered.
**Tests:** `func_rec_copies_active_lane`, `func_stop_pastes`,
`func_play_clears_lane_and_locks`,
`func_transport_noop_while_trk_held`,
`trk_func_trig_toggles_mute`, `mute_screen_trigs_toggle_mutes`.

### C5 — Encoder bank
D8 in full: encoder resolution against the active page's composite
`Rule`, ramp/acceleration (reuse the TK1 jog ramp machinery), fine via
Ctrl, p-lock routing under step focus, flash generalized from 2 slots
to 8 encoders + 3 slots, Param screen renders 8 encoder cells (2×4,
name + bar + value) with cursor (arrows, D13).
**Tests:** `encoder_col_maps_to_page_param_in_rule_order`,
`encoder_beyond_param_count_echoes_noop`,
`encoder_jog_emits_bump_param`, `encoder_fine_scales_step`,
`encoder_jog_routes_to_lock_when_step_focused`,
`page_select_rebinds_slots_a_b_c`, render: `param_screen_shows_eight_encoders`.

### C6 — Screens: Tempo, Settings, Chain
Tempo (`0`): bpm display, YES = tap (ring of 4 taps → `CMD_SET_PARAM`
on clock `bpm`; 2+ taps required, 4-tap window average *(tunable)*),
UP/DOWN = ±1 bpm, FUNC+UP/DOWN = ±0.1. Settings (`8`): read-only info
(D12). Chain (`o`): pattern bank row with active/cued markers (TK1
render reused), chain lane, YES = `CMD_CHAIN_PUSH` on cursor pattern,
NO/Backspace = `CMD_CHAIN_CLEAR`, page-loop display. KIT echo;
SAMPLING hidden (D12).
**Tests:** `tempo_screen_yes_taps_set_bpm`,
`tempo_arrows_nudge_bpm`, `chain_screen_yes_pushes_cursor_pattern`,
`chain_screen_clear_sends_chain_clear`, `kit_button_echoes_reserved`,
`sampling_hidden_without_capability`.

### C7 — Agent smoke + polish gate
No new features. Run the app on the 4-track default; fix paper cuts
found by the smoke script; update `design/bugs.md` per the standing
defect-filing directive if engine issues surface. Update
`AGENTS.md`/`README` key documentation to the panel table.

### C8 — Key remapping (ADR-037, re-based per D11/D14)
`Keymap` struct (flat key→button map), YAML persistence
(`serde_yml`, already a workspace dep), `:` verbs, unbindable-key
guard, load order global→local, startup load + `:load-bindings`.
ADR-037 status flips to accepted with an implementation note.
**Tests:** `keymap_resolves_user_binding_over_default`,
`keymap_falls_through_when_unbound`, `keymap_roundtrips_yaml`,
`local_file_overrides_global`, `bind_unbindable_key_errors`,
`reset_clears_all_user_bindings`, `unknown_button_name_echoes_error`,
`bindings_do_not_autosave_on_quit`.

### C9 — Live visualization (independent; may land any time after C3)
design.md §5.3(b): `EnvelopeNode`/`LfoNode` publish
`/node/{id}/state/{env_level,lfo_phase}` via `published_state()`;
Param screen animates envelope/LFO glyphs at ≤30 fps. The §5.3(c)
scope tap is **deferred to TK3** unless session #2 asks.
**Tests:** `envelope_node_publishes_level`,
`lfo_node_publishes_phase`, render smoke for the animated canvas.

### C10 — Usability session #2 (D4; user-paired, no code)
Held until here per D4 (C1–C8 landed; C9 optional). Hypotheses below.
Produces `design/sessions/theotokos-2.md` +
`design/phases/tk2-report.md`; OQ verdicts and any grammar re-cuts
recorded there. This session also carries the outstanding TK1 C8
obligations (that session was never run).

---

## §4. Hypotheses under test in session #2

| Hypothesis | Source |
|---|---|
| The panel grammar (TRK/PTN holds, REC toggle, FUNC encoder bank) reads as an Elektron-style groovebox workflow on the keyboard | D1 |
| Sticky-prefix fallback is tolerable on kitty-less terminals | D6 |
| FUNC+transport copy/clear/paste is discoverable and sufficient (vs. missing `y`/`Y`) | D7 |
| Encoder bank two-key simultaneity actually replaces slot sweeps; numpad cluster fate | D13/OQ-T24 |
| Tempo screen + YES-tap beats the retired `t` tap | D12/OQ-T23 |
| Mute quick-chord (TRK+FUNC+trig) vs. Mute screen — which survives | OQ-T22 |
| Runtime remapping via `:` line is discoverable | D11 |
| grid_rec defaulting to ON is right for a programming-first session | D12 |

## §5. Open questions

| # | Question | Status |
|---|---|---|
| OQ-T15/T16/T17/T18/T19 | remap grammar, reset scope, autosave, leader escape, positional arrays | **All resolved** — D11/D14 (leader retired; `:` unbindable) |
| OQ-T20 | live-trig command shape | **Resolved** — D5 |
| OQ-T21 | KEYBD chromatic grammar | OPEN — TK3 |
| OQ-T22 | mute chord vs. screen | session #2 |
| OQ-T23 | tempo/tap grammar | session #2 |
| OQ-T24 | numpad slot cluster fate | session #2 |
| OQ-T10/T11/T12 | scope tap placement, temp save/reload (P11), WT convergence | TK3 |
