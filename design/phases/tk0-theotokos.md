# Paraclete — TK0 Theotokos Specification (POC Vertical Slice)

> **Implementation blueprint.** Contracts only — deviations stop, get recorded
> in the phase report, and go to the user.
>
> **TK0 deliverable:** a keyboard-playable proof of the Theotokos posture —
> home-row steps and top-row tracks on the real (4-track) default instrument,
> dual numpad focus-slot jogs on engine-local `Rule` pages, working
> play/stop/tempo, mode line with live bindings. Ends in **usability
> session #1**.
> **Last updated:** 2026-07-21
> **Baseline:** ADR-036 accepted 2026-07-21; W2 C0–C6 shipped; BUG-032/033
> filed.
> **Design authority:** `design/theotokos/problem.md` +
> `design/theotokos/design.md` (as amended 2026-07-21) + ADR-036.
> **Model routing:** C0 (clock commands) Sonnet-capable; C1–C2 Opus (new
> crate, modal state machine); chord grammar is HYPOTHESIS-grade per ADR-036 —
> do not extend beyond this spec without a session.

---

## Ratified design anchors

| Anchor | Resolution (source) |
|---|---|
| Mutation plane | ADR-019 commands + sequencer CMD 16–32 + new clock commands (C0). Nothing else. (ADR-036.3) |
| Reads | Shared `StateBusHandle` + startup cap-doc cache. No JSON, no sockets. (ADR-036.3) |
| Views | Engine-local `Rule`s only in TK0 (SRC/AMP exist on engines). Composite assembly is TK1 (OQ-T13). (ADR-036.4 scope note) |
| Tracks | **4 in the default `instrument.yaml`** (seqs 10–13 → voices 20,21,22,27). Discovery-driven, ≤8; never the stale AGENTS.md table. (review M1) |
| Terminal ownership | Main thread only; `--theotokos` implies `--no-emulator` and skips `paraclete-tui`. (ADR-036.6) |
| Dependencies | `ratatui 0.26`, `crossterm 0.27`, `paraclete-node-api`. **No new deps.** (ADR-036.2) |
| Transport | C0 adds clock command handling (BUG-032); `Space` depends on it. (ADR-036.7a) |

---

## Commit sequence

| Commit | Crate(s) | Deliverable |
|---|---|---|
| **C0** | `paraclete-nodes` | **BUG-032 fix:** `InternalClock` command handling — start/stop + applied `bpm`. |
| **C1** | `paraclete-theotokos` (new), `paraclete-app` | **Crate + wiring + modal shell + SEQ mode:** pure keymap, Action/resolve, terminal ownership, `--theotokos` flag, transport bar + step grid + page windows. |
| **C2** | `paraclete-theotokos` | **PERF mode:** focus slots A/B, engine-local `Rule` page select, jog/ramp, mode line, static envelope render. |
| **C3** | — | **Usability session #1** (no code): 30 min hands-on, findings in `design/sessions/theotokos-1.md`, verdicts per hypothesis; report `design/phases/tk0-report.md`. |

---

## Commit 0 — `InternalClock` command handling (BUG-032)

The clock currently implements **no** `handle_commands`; its declared `bpm`
is dead and `playing` flips only on `TransportEvent` flags nothing in the
standalone app emits. This commit puts transport on the declared ADR-019
plane (OQ-T14 resolved: **node commands**, not an executor/`SetPlaying`
change — every present and future surface gets it for free; Antiphon's
`node_cmd` gate admits type_id ≥ 16, so the new commands pass it too).

### 0.1 Constants (`internal_clock.rs`)

```rust
pub const CMD_CLOCK_START: u32 = 16;  // node-specific range
pub const CMD_CLOCK_STOP:  u32 = 17;
```

### 0.2 Command semantics (override `handle_commands`)

| Command | Effect |
|---|---|
| `CMD_CLOCK_START` | If not playing: `playing = true; first_tick = true` — identical semantics to the `global_start` event path. Idempotent (already playing → no-op; `first_tick` NOT reset). |
| `CMD_CLOCK_STOP` | If playing: `playing = false`. Idempotent. |
| `CMD_SET_PARAM` (id `"bpm"`) | `self.bpm = value.clamp(20.0, 300.0)` |
| `CMD_BUMP_PARAM` (id `"bpm"`) | `self.bpm = (self.bpm + delta).clamp(20.0, 300.0)` |

Manual scan, no `ParameterBank`: the clock has exactly one param and
publishes `/transport/*` by hand — a bank would change nothing observable
but adds machinery. Published paths are unchanged; `/transport/bpm` and
`/transport/playing` reflect command effects on the next publish cycle.

### 0.3 Tests (named)

- `clock_start_via_command_plays_and_resets_first_tick`
- `clock_stop_via_command_halts` (+ start/stop idempotence)
- `clock_bpm_set_param_applies_and_clamps` (300.0 ceiling, 20.0 floor)
- `clock_bpm_bump_param_applies`
- existing `step_period_is_240_ticks` and app-level tempo tests stay green
  (start semantics must match the old auto-start path)

---

## Commit 1 — crate, wiring, modal shell, SEQ mode

### 1.1 Crate skeleton

`crates/paraclete-theotokos` (GPL-3.0; workspace member):

```toml
[dependencies]
ratatui = { workspace = true }
crossterm = { workspace = true }
paraclete-node-api = { path = "../paraclete-node-api" }
```

Modules and purity contracts (ADR-036 §2.3 — this is the composability
seam, do not collapse it):

| Module | Contract |
|---|---|
| `input` | `pub fn map_key(mode: &ModeState, ev: KeyEvent) -> Vec<Action>` — **pure**, no I/O |
| `action` | `pub enum Action { … }` (§1.3) + `pub fn resolve(actions, caps: &CapDocCache, model: &Model) -> Vec<NodeCommand>` — **pure** |
| `model` | track map, mode, page windows, slot bindings; polls `StateBusHandle` |
| `render` | ratatui widgets; `TestBackend`-testable |
| `lib` | `TheotokosApp { model, pending: Vec<NodeCommand>, … }`; `tick(&mut Terminal, &StateBusHandle, now_ms)`; `take_pending_commands()`; `should_quit()` |

### 1.2 Terminal ownership & app wiring

- Raw mode + alternate screen on the **main thread**; kitty
  `REPORT_EVENT_TYPES` pushed on entry, ignored on failure (emulator
  precedent); **Drop-guard restore** so a panic never strands the terminal.
- `Ctrl-C` is a raw-mode key event (no SIGINT): `map_key` emits
  `Action::Quit` → `should_quit()` → the main loop exits through the
  normal shutdown path.
- `--theotokos` flag: implies `--no-emulator`, skips old-TUI construction
  (one screen owner), logs one line. Tick placed after
  `antiphon.drain_commands()` in the main loop; egress:
  `for cmd in theotokos.take_pending_commands() { conf.send_command(cmd) }`.
- Cap-doc cache built at startup from the app's existing step-7
  collection; track map from `InstrumentIds` (sequencers zipped with
  generators, sorted by id, ≤8 entries → keys `q w e r u i o p`).

### 1.3 `Action` enum (initial contract — exhaustive for TK0)

```rust
pub enum Action {
    Quit,
    CycleMode(Dir),                    // Tab / Shift-Tab
    PlayToggle,                        // Space → START/STOP by /transport/playing
    SelectTrack(usize),                // q w e r u i o p → track map index
    ToggleStep { step: usize },        // a s d f j k l ; → absolute step (window-aware)
    PageWindow(Dir),                   // [ ]
    SelectParamPage(usize),            // PERF: 1..=6 (C2)
    Jog { slot: Slot, dir: Dir, mag: Mag },  // PERF: numpad (C2); Mag::{Coarse,Fine}
    RampStart { slot: Slot, dir: Dir },// key held (kitty) (C2)
    RampStop { slot: Slot },           // key released (kitty) (C2)
    Noop,                              // unmapped keys in this mode
}
```

`resolve` drops commands to unknown nodes silently (executor already
drop-safes), clamps nothing itself (the bank clamps), and never allocates
per keystroke beyond the small output `Vec`.

### 1.4 Keymap (SEQ + global; HYPOTHESIS marks per design.md §3)

Global (both modes): `Space` PlayToggle · `Tab`/`Shift-Tab` CycleMode ·
`Esc` Noop for now (reserved) · `Ctrl-C` Quit · `q w e r u i o p`
SelectTrack(0..7) — invariant across modes · `t`/`y` reserved (Noop).

SEQ: `a s d f j k l ;` → ToggleStep(col) → `CMD_TOGGLE_STEP` on the active
track's sequencer, `arg0 = window_base + col`; `[`/`]` → PageWindow
(window = 8 steps, clamped to `pattern_length` from
`/node/{seq}/state/pattern_length`).

### 1.5 SEQ render

- Transport bar: `♫ bpm` · `▶/■` (from `/transport/{bpm,playing}`) ·
  active track name/id · window range.
- Step grid: one row per discovered track (≤4 default), 8 cells per row;
  cell state from `/node/{seq}/state/steps` (text bitfield); playhead from
  `/node/{seq}/state/current_step`; active row highlighted.
- Dirty-flag redraw, ≤30 fps.

### 1.6 Tests (named)

- `keymap_seq_home_row_emits_toggle_step_with_window_offset`
- `keymap_track_select_invariant_in_both_modes`
- `keymap_tab_cycles_modes_and_back`
- `keymap_ctrl_c_quits`; `keymap_unmapped_is_noop`
- `resolve_toggle_step_targets_track_sequencer`
- `resolve_play_toggle_reads_playing_state` (start vs stop selection)
- `resolve_drops_unknown_track_silently`
- `render_seq_grid_shows_steps_and_playhead` (TestBackend)
- `render_transport_shows_bpm_and_play_state` (TestBackend)

---

## Commit 2 — PERF mode

### 2.1 Focus slots (ADR-036.5)

Two live slots `A`, `B` (`Slot` enum carries a `C` variant, unbound in
TK0). Each slot binds `(node_id, param_id)`. Bindings and values are
always visible in the mode line.

### 2.2 Page select → slot binding

Number row `1`–`6` selects the active track's **engine-local** `Rule`
page group by index into the Rule's own `page_groups` order (no canonical
order exists — follow the declaration). Binding rule: params of that
group (`param_pages` filtered by page, ordered by `PageRef.slot`) fill A,
then B. Fallbacks: page has 1 param → A only; engine has no `Rule` →
first two cap-doc params. Out-of-range page key → Noop.

### 2.3 Jog & ramp mechanics (constants in one `tuning` module — session-tunable)

| Gesture | Effect |
|---|---|
| numpad `7`/`1` (A ↑/↓), `8`/`2` (B ↑/↓) tap | one `CMD_BUMP_PARAM`, step = `max(range/128, 0.001)` |
| Shift+tap | fine step (÷8) |
| Ctrl+tap | coarse step (×4) |
| hold ≥150 ms (kitty repeat/release) | ramp at 60 Hz, accel ×1.05ⁿ capped ×8; release → stop immediately |
| no kitty support | OS key-repeat drives repeats; accel disabled; same bindings |
| numpad `9`/`3`, `4 5 6`, `0 . ⏎ + - * /` | Noop in TK0 (reserved per design.md §3.3) |

### 2.4 Mode line & envelope render

- Mode line: `MODE · T{n} name · PAGE · A:param=value · B:param=value ·
  step` — bindings **always** on screen (s1/s2 legibility contract).
- Envelope: if the bound page's engine `Rule` declares an
  `EnvelopeGroup`, draw the piecewise curve from live param values on a
  braille canvas; redraw on value change. No engine changes (ADR-036.7
  baseline). `LfoShape`/live phase are TK2 (gap §2.6.5).

### 2.5 Tests (named)

- `page_select_binds_first_two_params_of_group`
- `page_select_fallback_without_rule_uses_cap_doc_order`
- `page_select_out_of_range_is_noop`
- `jog_step_math_coarse_fine`
- `ramp_state_machine_dwell_accel_release`
- `render_mode_line_shows_bindings_and_values` (TestBackend)
- `render_envelope_from_live_params` (TestBackend)

---

## Exit criteria

1. `cargo test --workspace` green; clippy clean on touched crates.
2. Agent smoke: `cargo run -- --theotokos` on the default instrument —
   grid renders, `asdf` toggles kick steps audibly (test-driver scenario
   optional), `Space` stops/starts the clock (C0), `1`+jog sweeps an
   engine param with the mode line tracking values.
3. **Usability session #1** held; findings append-only in
   `design/sessions/theotokos-1.md`; per-hypothesis verdicts
   (converged/revise/park) recorded; `tk0-report.md` written; roadmap +
   `design.md` marks updated in the same commit.

## Explicit non-goals (TK0)

P-locks (TK1; OQ-T8), composite `Rule` pages (TK1; OQ-T13), explicit slot
rebinding leader grammar (TK1), mutes/fills/CHAIN (TK2), live
visualization (TK2), numpad-less fallback (TK2; OQ-T3), keymap config
(TK3), `:` command line beyond a stub (TK1), `Esc` semantics beyond
reserved (OQ-T7), slot C bindings (TK2).

## Hypotheses under test in session #1 (from design.md)

Hold-vs-focus p-lock gesture (deferred to TK1 but probe opinions), leader
key choice (OQ-T2), `+`/`-` semantics (OQ-T4), kitty-less ramp
degradation (OQ-T6), Esc/tmux ergonomics (OQ-T7), jog step/ramp constants
(§2.3 tuning), page-order discoverability (number row vs page names).
