# Paraclete — TK3 Theotokos Specification

> **EXECUTION-READY for C0–C7 — 2026-08-05.** Written so a fresh-context
> session can implement without further design decisions: every commit names
> its files, the current behaviour it changes, and its tests. Where a value
> is a tuning knob the default is stated *(tunable)*.
>
> **Review amendments (2026-08-05):** §2.1 rewritten — the sequencer's
> virtual params bypass the composite assembly (the sequencer is not in
> `TrackChain`); `EncoderTarget` enum distinguishes real vs virtual at
> the dispatch. §2.6 rewritten — `FUNC+Space` tap tempo is a companion
> intercept to ADR-044 A12, not a `button_to_action` change. §2.5 adds
> Ctrl+FUNC modifier interaction and tier×acceleration semantics. B3
> resolved: condition encoder cycles `FillCondition` (7 states) with
> read-modify-write on the packed `TrigCondition`. §1 Out section
> accounts for chain view, macro support, and WT convergence dropped
> from the original TK3 vision.
>
> **Post-review fixes (2026-08-05):** B1 — step-detail bus tuple expanded
> from `cond_u8` (fill-only) to `prob_u8,repeat_n_u8,repeat_m_u8,fill_u8`
> so the condition read-modify-write can reconstruct the full
> `TrigCondition`. B2 — §2.7 rewritten: FUNC/SHIFT is not a
> `PanelButton`; `HeldState` gains `func_held`/`ctrl_held` persistent
> fields (C6 file list updated to include `input.rs`). M1 —
> `EncoderTarget` integration with `EncoderParam` made explicit (new
> field, sentinel `node_id`/`param_id` for `VirtualStep`). M2 — key
> inventory corrected (1–9 and 0, Sampling noted). M3 — sequencer→bus→model
> round-trip test required (not optional) in C0 test plan and exit criteria.
>
> **Design authority:** ADR-044 (accepted; the panel model), ADR-037
> (accepted; key remapping mechanism), ADR-038 (accepted; Elektron
> convergence). 14 open questions pre-resolved 2026-08-03 (roadmap step 7).
> **Baseline:** P11 closed (`db3d028`), MM closed (code-complete, session
> #5 concluded), TK2.2 closed (`tk2.2-report.md`). `cargo test --workspace`
> green at baseline.
> **Exit:** `cargo test --workspace` green after every commit; a real
> terminal pass in kitty; **usability session #6** (`sessions/theotokos-6.md`).
>
> **Read this first — what this phase is and is not.** TK3 is breadth and
> polish, not architecture. No new screens beyond MIX. No new modes. No
> WT convergence code (resolved by decision, not implementation). No
> chromatic grammar code (needs a melodic engine that does not exist). No
> Ordo profile wizard (mechanism complete; switching deferred to session
> evidence). The phase closes visible gaps (#180, #163, #184), adds the
> one missing screen (MIX), and implements three resolved OQs (OQ-T4
> step-size scaling, OQ-T23b dual-path tap tempo, OQ-T31 modifier
> highlight).

---

## §0. Decisions already made (pre-session resolution, 2026-08-03)

These are not open questions. They are inputs to the spec, settled by the
user before spec writing began. Cited here so each commit can reference
the decision without re-deriving it.

| Issue | Decision | Commits |
|---|---|---|
| OQ-T4 (#126) | ENC + jog step-size scaling (8/16/32+) | C4 |
| OQ-T23b (#125) | Dual-path tap tempo: global chord from any screen + ENC jog for continuous BPM | C5 |
| OQ-T31 (#147) | Live modifier readout on panel — held modifiers highlighted | C6 |
| OQ-T27 (#121) | Hold-step focus for p-lock authoring | (already shipped in TK2.1 C5b — `model.rs:435` `lock_step_for_active_track()`) |
| OQ-T5 (#132) | Jog-first, type-in via standard digit keys | (no new code — current model is correct) |
| OQ-T10 (#135) | Global tempo primary; per-track speed multiplier stays in engine | (no new code — current model is correct) |
| OQ-T21 (#128) | All three chromatic modes available | **Deferred** — needs a melodic engine (P13/P14) |
| OQ-T24 (#124) | Numpad slots repurpose TBD | **Deferred** — no verdict, no pressure |
| OQ-T12 (#129) | Unified source of truth; Theoria may carry extra views | (no new code — architecture already correct) |
| OQ-T28 (#127) | Cross-surface lock capture (ADR-045) | **Stays parked** per session #4 judgment |
| OQ-T1/T8/T30 | Superseded / internal / resolved | (no action) |
| OQ-T7 (#133) | Esc/Tmux — deferred, tmux not used | (no action) |
| OQ-T2 (#131) | `\` leader key | (already shipped in TK1) |

---

## §1. Scope

**In:**

- **#180** — TRIG page has 7 empty slots and 4 sequencer per-step params
  with no encoder home. Give them one.
- **#163** — `RenderData` assembled inline in `render_if_needed`, so
  nothing about the panel's data can be unit-tested. Extract into a pure
  function.
- **#184** — Surface `AppOp` production (Theotokos/Antiphon) is not
  headless-testable. Expose `pending_app_ops()`.
- **MIX screen** — per-track level view, encoder jogs gain. One screen,
  no sends (the graph has none).
- **OQ-T4** — ENC + jog step-size scaling (8/16/32+).
- **OQ-T23b** — Dual-path tap tempo (global chord + ENC jog).
- **OQ-T31** — Modifier highlight on panel.
- **#138/#139** — TK1 carryover (missing tests, missing fixture), if
  quick. Folded into C1 or C3 if not.

**Out, with a citation:**

- **OQ-T21 (chromatic grammar)** — resolved in principle; implementation
  deferred to the phase that ships the first melodic engine (P13 or P14).
  No melodic engine exists today; the gesture has nothing to play.
- **Ordo layout profiles** — ADR-037's mechanism (runtime-remappable
  `Keymap`, `:bind`/`:save-bindings`/`:load-bindings`, YAML persistence)
  shipped in TK2. Profile switching and a guided remap wizard are
  deferred to session evidence — no performer has asked to switch layouts
  in five sessions.
- **ADR-045 (cross-surface lock capture)** — stays parked per session #4
  judgment (OQ-T28).
- **#140 (yank lossy)** — sequencer-side: the state bus does not expose
  per-step note/velocity/length/timing/condition. C0 adds a
  `step_detail` bus path that covers velocity/length/timing/condition
  but **not note** (the step's trigger target). #140's most visible
  half — yank cannot copy the note — remains. Not a Theotokos fix.
- **Per-track scope tap** — OQ-T10 resolved as "global tempo primary";
  per-track taps are a session ask, not a spec assumption.
- **Chain view** — was in the original TK3 vision; deferred. No session
  has produced a "I need to see the chain on the panel" moment; the
  Chain screen (text list) covers the use case.
- **Macro support from `Rule`** — was in the original TK3 vision;
  deferred. The `Rule` already carries `macros` and the assembly merges
  them (`CompositePage::macros`), but no shipped node declares any.
  Implementation is moot until a node produces macros.
- **WT convergence decision** — was in the original TK3 vision;
  resolved by decision in §0 (OQ-T12: architecture already correct).
  No implementation needed.

---

## §2. Architecture notes

### 2.1 The TRIG page problem (#180)

The TRIG page comes from the **engine's** `Rule`, not the sequencer's.
The sequencer does not implement `ViewPlugin` — it has no `Rule`, no
`page_groups`, no `param_pages`. The only param on TRIG today is
`machine` (slot 0), declared by both `AnalogEngine` and `FmEngine` in
their `machine_page_refs()`.

The per-step params (velocity, length, timing, condition) are not bank
params — they are step-internal data accessed via dedicated sequencer
commands (`CMD_SET_STEP_VELOCITY` = 36, `CMD_SET_STEP_LENGTH` = 37,
`CMD_SET_STEP_TIMING` = 25, `CMD_SET_STEP_CONDITION` = 26). They have no
`ParamDescriptor`, no page placement, no encoder reach.

**The fix is not to route through the composite assembly.** The
assembly's `assemble_for()` builds its contributor list from
`chain_rules` — `engine_node_id` + `chain_ids` (`view-assembly/src/lib.rs:260-263`).
`chain_ids` are audio-graph downstream nodes discovered by BFS over
audio edges in `main.rs:1051-1085`. The sequencer is not in the
`TrackChain` — it is the track's *source*, upstream of the engine. No
sequencer rule exists in the `rules` HashMap that `assemble` receives.
Adding the sequencer as a contributor would require changing
`TrackChain` or `assemble_for`, and the per-step (not per-param) data
these params carry is a fundamentally different kind of thing from what
the assembly merges.

**The fix is a model-level bypass.** The sequencer gets a `ViewPlugin`
impl (for cap-doc completeness and discovery — it declares
`page_groups: ["TRIG"]` and four synthetic `ParamDescriptor`s at slots
1–4). But the TRIG page's virtual params are **not** resolved through
the composite view. Instead, `Model::resolve_encoder_params()` detects
when the active page is TRIG and **appends** four virtual entries after
the engine's composite params. These entries are a new enum added to `EncoderParam`:

```rust
/// How the encoder dispatch resolves a jog on this column.
/// `Real` is the existing path (CMD_BUMP_PARAM to a node/param pair).
/// `VirtualStep` is a per-step sequencer param (CMD_SET_STEP_*).
pub enum EncoderTarget {
    Real,
    VirtualStep { seq_id: u32, kind: StepParamKind },
}

pub enum StepParamKind { Velocity, Length, Timing, Condition }
```

`EncoderParam` gains a `target: EncoderTarget` field (default `Real`).
For `Real` targets, the existing `node_id`/`param_id` fields are used
as today. For `VirtualStep`, `node_id` and `param_id` are set to
sentinel 0 — the dispatch matches on `target` before reading them, so
the sentinels are never used for command emission. The `seq_id` and
`kind` carry the actual routing.

The dispatch in `lib.rs` matches on `param.target`: `Real` emits
`CMD_BUMP_PARAM` as today (unchanged); `VirtualStep` emits the
dedicated `CMD_SET_STEP_*` command targeting the focused step. The
render path matches — `EncoderCell` in `RenderData` carries the
`EncoderTarget` so the renderer can draw virtual params with a distinct
glyph (e.g., `▸` prefix) to signal "this edits a step, not a live
param".

**What "focused step" means here:** the existing `lock_target_step` (the
step the p-lock target is armed on). When no lock target is armed, the
encoder reads/writes the **current playing step** (`/node/{seq}/state/current_step`).
When a lock target is armed, it reads/writes that step. This is the same
step the existing p-lock jog routes through — no new focus model.

**The four virtual params:**

| Name | Range | Stepped | Command | Read path |
|---|---|---|---|---|
| `velocity` | 0.0–1.0 | no | `CMD_SET_STEP_VELOCITY` (36) | `step.velocity / 65535.0` |
| `length` | 0.0–1.0 | no | `CMD_SET_STEP_LENGTH` (37) | `step.length` |
| `timing` | −1.0–1.0 | no | `CMD_SET_STEP_TIMING` (25) | `step.timing.micro_offset / 47.0` |
| `condition` | 0.0–6.0 | yes | `CMD_SET_STEP_CONDITION` (26) | condition discriminant |

**Condition as a stepped selector:** `TrigCondition` has one variant —
`Simple { repeat: RepeatCondition, fill: FillCondition, probability: u8
}`. The encoder cycles the **`FillCondition`** sub-field only (seven
states: Ignore, FillA, FillB, FillAny, NoFill, NotFillA, NotFillB).
Repeat and probability are not surfaced here.

**The dispatch must read-modify-write.** `CMD_SET_STEP_CONDITION`
(type_id 26) packs all three sub-fields into `arg1`:
`probability | (repeat_n << 8) | (repeat_m << 16) | (fill_disc << 24)`.
The encoder jog cannot just write a fill value — it must read the step's
current `TrigCondition`, replace the `fill` field with the new value,
and send the packed result. The step-detail bus path (below) carries
**all three sub-fields** (`prob_u8`, `repeat_n_u8`, `repeat_m_u8`,
`fill_u8`) so the dispatch can reconstruct the full `TrigCondition`,
swap the fill, and repack. Carrying only the fill discriminant would
make the repeat and probability unrecoverable — they are not cached
elsewhere, and a cache would go stale on live-record, Antiphon, or
project-load paths. This matches the Elektron model (the condition
encoder is a fill selector, not a full condition editor).

**Velocity/length reads need sequencer cooperation.** The sequencer's
`published_state()` currently publishes a bitfield (`steps_bitfield`) but
not per-step velocity/length/timing/condition. Two options:

1. **Add per-step detail to `published_state()`.** A new bus path
   `/node/{id}/state/step_detail` carrying a packed text blob (same shape
   as `steps_bitfield` but with velocity/length/timing/condition per
   step). The Theotokos model parses it.
2. **Read from the model's existing step cache.** Theotokos already reads
   `step_state` and `step_states` from the bus bitfield. Extend the
   bitfield or add a parallel path.

Option 1 is cleaner. The sequencer already serializes per-step data for
v3 project saves (`write_step_record`); the bus path is a subset of that.
The format: a semicolon-delimited list of
`vel_u16,len_f32,timing_i8,prob_u8,repeat_n_u8,repeat_m_u8,fill_u8`
tuples, one per step, in step order. Theotokos parses the focused step's
tuple. The four condition fields mirror `write_step_record`'s encoding
(sequencer.rs:2335–2344): `prob_u8` is the raw probability, `repeat_n`
and `repeat_m` are the `nm_from_repeat()` output (zero = Always), and
`fill_u8` is the `fill_discriminant()` output (0=Ignore … 6=NotFillB).
The dispatch reads all four, replaces only `fill_u8` with the new
value, and repacks into `CMD_SET_STEP_CONDITION`'s `arg1`.

> **Precedent note:** this is the only state bus path that packs
> structured per-step data into a text blob. Every other path carries a
> single `StateBusValue`. Future bus paths should prefer separate paths
> per field; this one is justified by the volume (N steps × 7 fields
> would be 7N bus paths) and by the fact that the consumer always reads
> a specific step's tuple, not the whole set. Documented so future
> agents do not pattern-match against it.

**Partial #140 coverage.** The bus now exposes per-step
velocity/length/timing/condition. #140 also lists **note** (the step's
trigger target), which is not in the step-detail path — it would require
a separate `step_notes` path. #140 remains open; the data for four of
its five fields is now available for a future phase to consume.

### 2.2 MIX screen

One new screen: `Screen::Mix`. Opened by **`FUNC+8`** (Settings is on
`8`; FUNC+8 = MIX). All number keys 1–9 and 0 are already assigned
(param pages, KIT, Sampling, Tempo), and there is no free `PanelButton`.
(`9` → Sampling is bound but currently `Action::Noop`; stealing it would
reserve a key for an unscheduled feature.) A chord
is the only option that does not steal a key from an existing screen.
*(Tunable — session #6 may reassign.)*

**Layout:** the contextual window shows the model's track list
(`model.tracks.len()`, not MixNode's input count — the model knows how
many tracks the instrument has; MixNode's input count is a graph
concern that may differ). Each row: track display name, a block-element
level bar (read from the state bus — MixNode publishes per-input gains),
and the gain value. The encoder bank's first N columns map to tracks
1–N; jogging adjusts `input_gain_{track}` on MixNode (node 2). Column 8
maps to `master_gain`.

**State bus reads:** MixNode already publishes its bank via
`publish_bank_state()` — paths `/node/2/param/input_gain_0` through
`/node/2/param/input_gain_7` and `/node/2/param/master_gain`. Theotokos
reads these directly. No new bus paths needed.

**No sends, no pan.** The graph has one reverb (hard-wired) and no pan
node. Building send/pan architecture now would be speculative depth.

### 2.3 RenderData extraction (#163)

The `RenderData` struct is assembled inline in `lib.rs::tick()` (around
line 440). Extract into a free function:

```rust
pub fn build_render_data(
    model: &mut Model,
    bus: &StateBusState,
    held: &HeldState,
    keymap: &Keymap,
    now: Instant,
    tuning: &Tuning,
) -> RenderData
```

`tick()` calls this, then passes the result to `render::render()`. The
function takes `&mut Model` because `encoder_flash` update is a side
effect on `Model`. It has no I/O — no `Terminal`, no bus writes — so it
is **testable** (unit tests construct a `Model` + synthetic
`StateBusState` and assert on the `RenderData` fields), but it is not
pure in the functional sense. The `&mut` is documented in the signature
so callers know the model may change.

### 2.4 AppOp testability (#184)

Theotokos produces `AppOp`s (perform-mode toggle, temp save/reload, kit
operations) that the app main loop drains via
`Model::take_pending_app_ops()` (`model.rs:1951`, called from
`main.rs:591,599`). The destructive drain **is** a test seam — a test
can call `take_pending_app_ops()` and assert on the returned vec — but
it consumes the ops, so a test cannot inspect them and then let the
normal drain path run. What is missing is a **non-destructive observer**
so a test can check ops without consuming them, and a way to drive the
key-to-op path without constructing a full `TheotokosApp`.

Fix: add `pub fn pending_app_ops(&self) -> &[AppOp]` on `Model` for
non-destructive inspection. Unit tests construct a `Model`, drive key
sequences through `handle_keys`, and assert on `pending_app_ops()`.

### 2.5 Step-size scaling (OQ-T4)

The `Tuning` struct already has `jog_step()` with `Mag::Fine` and
`Mag::Coarse`. Add a `step_size_tier` field to `Model` (values: 0=normal,
1=×2, 2=×4, 3=×8, etc.). ENC mode + a dedicated gesture cycles the tier.
The jog dispatch multiplies the base step by `2^tier`.

**Gesture:** in ENC mode, `Ctrl+FUNC` + encoder jog up/down changes the
tier. The status line shows the current tier (`×1`, `×2`, `×4`, `×8`).

**Modifier interaction:** currently Ctrl = Fine (jog divisor),
FUNC/Shift = Coarse (jog multiplier). `Ctrl+FUNC` together is currently
undefined — no code path handles it. In ENC mode, `Ctrl+FUNC` becomes a
**mode switch** (tier change), not a jog magnitude. The Fine and Coarse
meanings are suppressed while both are held simultaneously. Outside ENC
mode, `Ctrl+FUNC` has no effect (the gesture is ENC-mode-only).

**Tier × acceleration:** the tier multiplier applies to the **base**
step (`max(0.001, range/128)`), before the time-based acceleration ramp
(`held_ms` in `Tuning::jog_step`). The acceleration curve shape is
unchanged; the tier scales its output. At tier 4 (×16) with a ramped
hold, the jog can produce large jumps — this is intentional (the
performer asked for big steps).

### 2.6 Dual-path tap tempo (OQ-T23b)

**Path 1 (existing):** `TapTempo` action, currently reachable only from
the Tempo screen (`PanelButton::Yes` when `screen == Screen::Tempo`).
Extend: make it reachable from **any** screen via a global chord.

**Chord: `FUNC+Space`.** ADR-044 A12 (normative, `lib.rs:801-811`)
currently overrides FUNC+Space to `Action::Noop` to prevent it from
collapsing onto `ClearLane` (since Space and `x` both map to
`PanelButton::Play`, and `button_to_action` cannot tell them apart).
The A12 guard fires only when the action is `ClearLane`:

```rust
let action = if matches!(action, Action::ClearLane) && ev.code == KeyCode::Char(' ') {
    Action::Noop
} else { action };
```

The fix is a **companion intercept** in `handle_keys`, *before* the
action is dispatched: if the raw key is `Space` and FUNC is held, emit
`Action::TapTempo` directly. This is analogous to the existing A12
override — it uses the raw key to distinguish Space from `x`. The A12
guard's purpose (prevent accidental ClearLane from Space) is preserved:
ClearLane still requires the literal `x` key. The new intercept fires
first and produces `TapTempo` (not `ClearLane`), so the A12 guard's
condition never matches.

**Path 2 (new):** ENC jog on BPM. When the Tempo screen is open, encoder
column 1 jogs BPM continuously (the existing `NudgeBpm` action, but via
encoder instead of arrow keys). When the Tempo screen is *not* open, the
encoder bank is showing the active page — BPM jog is Tempo-screen-only.

### 2.7 Modifier highlight (OQ-T31)

A dedicated panel element showing held modifiers: `SHIFT` (FUNC), `TRK`,
`PTN`, `LOCK`, `REC`. Stays lit while held, goes dark on release.
Placement: the status line already shows the armed prefix (`TRK…`,
`PTN…`, `LOCK…`). Extend it to show **all** held modifiers, not just the
armed one. `SHIFT` is new — currently FUNC is invisible on the panel
until it participates in a chord.

Implementation: FUNC/SHIFT is **not** a `PanelButton` — it is tracked
per-event via `Mods { func: bool, ctrl: bool }` (input.rs:720),
constructed from `KeyEvent::modifiers` at dispatch time and discarded.
`HeldState::pressed` tracks `PanelButton` presses (Trk, Ptn, Rec, Lock,
trigs, etc.) — none of which correspond to the Shift key. The `Hold`
enum (input.rs:542) has `Trk, Ptn, Rec, Lock` — no `Func` variant.

**The gap:** to render a `SHIFT` chip on the status line, the model
needs to know whether Shift is physically held *right now*. In kitty
mode (with releases), this requires persistent tracking.

**Fix:** add `func_held: bool` and `ctrl_held: bool` fields to
`HeldState`. Set `func_held = true` on any key event where
`func_held(ev)` is true and the key is a modifier-key press; clear it
when a release event drops the Shift modifier. Same pattern for
`ctrl_held`. The `button_to_action` path already receives `Mods` — the
press/release tracking happens in the lower `handle_keys` loop where
press and release events are distinguished (input.rs:589+). The status
line reads `held.func_held`, `held.ctrl_held`, and `held.armed` to
render the chip set. In sticky-fallback mode (no releases), the armed
prefix is the only signal — same as today.

---

## §3. Commit plan

### C0 — #180: TRIG page gets the four sequencer per-step params

**Files:**
- `crates/paraclete-nodes/src/sequencer.rs` — add `ViewPlugin` impl (for
  cap-doc completeness), four synthetic `ParamDescriptor`s,
  `published_state()` step-detail path
- `crates/paraclete-view-assembly/src/lib.rs` — no changes (the
  sequencer's virtual params bypass the assembly; see §2.1)
- `crates/paraclete-theotokos/src/model.rs` — add `EncoderTarget` enum
  (`Real` / `VirtualStep`), `StepParamKind` enum, parse step-detail bus
  path, append virtual entries in `resolve_encoder_params()` when active
  page is TRIG
- `crates/paraclete-theotokos/src/lib.rs` — encoder jog dispatch matches
  on `EncoderTarget`: `Real` → `CMD_BUMP_PARAM` (unchanged);
  `VirtualStep` → `CMD_SET_STEP_*` targeting the focused step.
  Condition dispatch does read-modify-write (preserve repeat/probability).
- `crates/paraclete-theotokos/src/render.rs` — `EncoderCell` carries the
  target variant; virtual params render with a `▸` prefix glyph

**Current behaviour:** TRIG page shows `machine` in slot 0, slots 1–7
empty. Per-step velocity/length/timing/condition have no encoder reach.

**New behaviour:** TRIG page shows `machine` (slot 0), `▸velocity` (1),
`▸length` (2), `▸timing` (3), `▸condition` (4), slots 5–7 empty.
Encoder jog on slots 1–4 emits the dedicated sequencer commands. Value
reads come from the new `/node/{seq}/state/step_detail` bus path.

**Tests:**
- Sequencer: `ViewPlugin` impl produces a Rule with `page_groups: ["TRIG"]`
  and four `param_pages` entries at slots 1–4
- Sequencer: `published_state()` includes `step_detail` path with correct
  format
- Theotokos model: `resolve_encoder_params()` on TRIG page returns
  `EncoderTarget::VirtualStep` entries at positions 1–4
- Theotokos model: `resolve_encoder_params()` on non-TRIG page returns
  only `EncoderTarget::Real` entries (no virtual leakage)
- Theotokos dispatch: encoder jog on virtual `velocity` emits
  `CMD_SET_STEP_VELOCITY` (not `CMD_BUMP_PARAM`)
- Theotokos dispatch: encoder jog on virtual `condition` does
  read-modify-write — repeat and probability are preserved, only fill
  changes
- **Round-trip:** construct a `Sequencer`, set per-step velocity,
  length, timing, and a non-default condition (e.g. probability=75,
  repeat=NthOfM{1,2}, fill=FillA), call `published_state()`, extract
  the `step_detail` text value, feed it to `model::parse_step_detail()`,
  and assert all seven tuple fields match. This is the integration seam
  where format mismatches surface — it is unit-testable without the
  harness and is required, not optional.
- Mutation: remove the virtual-param dispatch, confirm the encoder jog
  test fails (emits `CMD_BUMP_PARAM` instead of `CMD_SET_STEP_VELOCITY`)
- Mutation: remove the step-detail bus path, confirm the parse test fails
- Mutation: drop `prob_u8`/`repeat_n`/`repeat_m` from the bus tuple,
  confirm the condition read-modify-write test fails (repeat/probability
  are clobbered to zero)

**Step-detail bus format:** semicolon-delimited list of
`vel_u16,len_f32,timing_i8,prob_u8,repeat_n_u8,repeat_m_u8,fill_u8`
tuples, one per step, step order. Parsed by `model::parse_step_detail()`.
The four condition fields mirror `write_step_record`'s encoding:
`prob_u8` is the raw probability, `repeat_n_u8`/`repeat_m_u8` are the
`nm_from_repeat()` output (0,0 = Always), `fill_u8` is the
`fill_discriminant()` output (0=Ignore … 6=NotFillB). The dispatch
reads all four, replaces only `fill_u8`, and repacks into
`CMD_SET_STEP_CONDITION`'s `arg1`.

### C1 — #163: Extract RenderData assembly into a pure function

**Files:**
- `crates/paraclete-theotokos/src/lib.rs` — extract `build_render_data()`
- `crates/paraclete-theotokos/src/render.rs` — no changes (already takes
  `&RenderData`)

**Current behaviour:** `RenderData` assembled inline in `tick()`, ~120
lines of bus reads and model queries mixed with the render call.

**New behaviour:** `pub fn build_render_data(model: &mut Model, bus:
&StateBusState, ...) -> RenderData` is a free function. `tick()` calls
it, then passes the result to `render::render()`. Takes `&mut Model`
for the `encoder_flash` side effect; no I/O.

**Tests:**
- Construct a `Model` + synthetic state, call `build_render_data()`,
  assert on `screen`, `bpm`, `encoder_cells`, `mute_states`, etc.
- At least 5 tests covering: default state, param page with encoder
  cells, muted tracks, armed prefix, kit screen state

**Also fold in #138/#139 if trivial** (missing TK1 tests, missing
fixture). If not trivial, note in the report and move on.

### C2 — MIX screen

**Files:**
- `crates/paraclete-theotokos/src/model.rs` — add `Screen::Mix`
- `crates/paraclete-theotokos/src/input.rs` — bind `FUNC+8` to
  `OpenScreen(Screen::Mix)` (intercepted in `handle_keys` before the
  bare-8 → Settings dispatch)
- `crates/paraclete-theotokos/src/render.rs` — `render_mix_screen()`
- `crates/paraclete-theotokos/src/lib.rs` — MixNode gain reads in
  `build_render_data()`, encoder jog dispatch for MixNode params
- `crates/paraclete-theotokos/src/action.rs` — no changes (uses existing
  `OpenScreen`)

**Current behaviour:** No MIX screen. MixNode params reachable only via
the encoder bank on a param page (if MixNode is in the chain's composite
view) or via `:set`.

**New behaviour:** MIX screen shows `model.tracks.len()` rows (the
model's track count, not MixNode's input count). Each row: track name,
level bar, gain value. Encoder columns 1–N jog track gains; column 8
jogs master gain.

**Tests:**
- Render: `TestBackend` assertion on MIX screen layout (track names, bar
  widths)
- Model: MixNode gain reads resolve to correct tracks
- Dispatch: encoder jog on MIX screen emits `CMD_BUMP_PARAM` to MixNode
  with correct `input_gain_{n}` param id

### C3 — #184: Expose pending AppOps for headless testing

**Files:**
- `crates/paraclete-theotokos/src/model.rs` — add `pending_app_ops()`
  non-destructive accessor alongside the existing
  `take_pending_app_ops()` drain

**Current behaviour:** `AppOp`s accumulate in `Model::pending_app_ops`
and are drained by the app main loop via `take_pending_app_ops()`
(`model.rs:1951`, `main.rs:591,599`). The destructive drain is a test
seam but consumes the ops. No non-destructive observer exists.

**New behaviour:** `pub fn pending_app_ops(&self) -> &[AppOp]` on
`Model` for non-destructive inspection. Unit tests drive key sequences
through `handle_keys` and assert on the returned slice without
consuming the ops.

**Tests:**
- FUNC+KIT produces `AppOp::SetPerformMode(true)` then
  `AppOp::SetPerformMode(false)`
- FUNC+Enter produces `AppOp::TempSave`
- FUNC+Esc produces `AppOp::TempReload`

### C4 — OQ-T4: ENC + jog step-size scaling

**Files:**
- `crates/paraclete-theotokos/src/model.rs` — add `step_size_tier: u8`
  to `Model`
- `crates/paraclete-theotokos/src/input.rs` — bind `Ctrl+FUNC` + encoder
  jog to step-size tier change (ENC-mode-only; outside ENC mode
  `Ctrl+FUNC` has no effect)
- `crates/paraclete-theotokos/src/action.rs` — add
  `Action::SetStepSizeTier(i8)` (delta: +1 or −1)
- `crates/paraclete-theotokos/src/lib.rs` — dispatch: clamp tier to
  0..4, multiply **base** jog step by `2^tier` (before acceleration
  ramp). When both Ctrl and FUNC are held in ENC mode, suppress
  Fine/Coarse jog meanings and treat as tier change instead.
- `crates/paraclete-theotokos/src/render.rs` — status line shows tier
  (`×1`, `×2`, `×4`, `×8`, `×16`)

**Current behaviour:** Jog step is `max(0.001, range/128)` coarse
default; fine = coarse/8; coarse = coarse×4 (from `Tuning`). No
performer-controllable scaling. `Ctrl+FUNC` together is undefined.

**New behaviour:** ENC mode + `Ctrl+FUNC` + up/down encoder jog changes
the step-size tier. Tier 0 = normal, tier 1 = ×2, tier 2 = ×4, tier 3 =
×8, tier 4 = ×16. Status line shows current tier. The multiplier applies
to the base step, before `Tuning::jog_step`'s acceleration ramp. Fine
and Coarse meanings are suppressed while both Ctrl and FUNC are held
simultaneously in ENC mode. Outside ENC mode, `Ctrl+FUNC` has no
effect. The tier persists until changed (no auto-reset).

**Tests:**
- Tier 0: jog step is base
- Tier 2: jog step is base × 4
- Tier clamps at 0 and 4
- Tier resets to 0 on... (nothing — it persists until changed. Document
  this.)

### C5 — OQ-T23b: Dual-path tap tempo

**Files:**
- `crates/paraclete-theotokos/src/lib.rs` — in `handle_keys`, before the
  A12 guard: intercept raw `Space` + FUNC → `Action::TapTempo` (see
  §2.6). Encoder column 1 on Tempo screen jogs BPM.
- `crates/paraclete-theotokos/src/action.rs` — no changes (reuses
  existing `TapTempo` and `NudgeBpm`)

**Current behaviour:** Tap tempo reachable only from Tempo screen
(`PanelButton::Yes`). BPM nudge via arrow keys on Tempo screen.
FUNC+Space is a normative ADR-044 A12 no-op.

**New behaviour:**
- `FUNC+Space` = tap tempo from any screen (the "global chord" path).
  Companion intercept to A12 — uses raw key to distinguish Space from
  `x`. A12's ClearLane protection is preserved (ClearLane still
  requires literal `x`).
- Tempo screen + encoder column 1 = continuous BPM jog (the "ENC jog"
  path). Same `NudgeBpm` semantics but driven by encoder rotation.

**Tests:**
- `FUNC+Space` on Grid screen produces a tap-tempo event
- `FUNC+Space` on Param screen produces a tap-tempo event
- `FUNC+Space` does NOT produce `ClearLane` (A12 preserved)
- Bare `Space` still toggles transport (no regression)
- Encoder jog on Tempo screen emits BPM nudge command

### C6 — OQ-T31: Modifier highlight on panel

**Files:**
- `crates/paraclete-theotokos/src/input.rs` — add `func_held: bool` and
  `ctrl_held: bool` to `HeldState`; track on press/release events in
  the `handle_keys` loop (set on press with matching modifier, clear on
  release)
- `crates/paraclete-theotokos/src/render.rs` — status line renders held
  modifier chips (reads `held.func_held`, `held.ctrl_held`,
  `held.armed`)
- `crates/paraclete-theotokos/src/lib.rs` — pass `&HeldState` modifier
  flags through to `RenderData`
- `crates/paraclete-theotokos/src/model.rs` — add `held_modifiers:
  Vec<String>` to `RenderData`

**Current behaviour:** Status line shows armed prefix (`TRK…`, `PTN…`,
`LOCK…`) but not FUNC/SHIFT. A performer holding SHIFT to access the
encoder bank has no visual confirmation that FUNC is active.

**New behaviour:** Status line shows chips for every held modifier:
`SHIFT`, `TRK`, `PTN`, `LOCK`, `REC`. Chips are bright while held, dark
when released. In sticky-fallback mode (no kitty releases), only the
armed prefix shows (same as today — the fallback's limitation is
inherent).

**Tests:**
- Render: status line with SHIFT held shows `SHIFT` chip
- Render: status line with TRK armed shows `TRK…` chip
- Render: status line with no modifiers shows no chips (existing
  behaviour preserved)

### C7 — Review + session gate

Code review (Flash subagent) on the full diff. Then usability session #6
(`design/sessions/theotokos-6.md`).

---

## §4. Test plan

| Layer | What | Catches |
|---|---|---|
| Unit (sequencer) | `ViewPlugin` impl, step-detail bus format | wrong page placement, malformed bus data |
| Unit (Theotokos model) | step-detail parsing (all 7 fields), `EncoderTarget::VirtualStep` resolution, TRIG-only gating, sequencer→bus→model round-trip | parse errors, wrong command mapping, virtual leakage to non-TRIG pages, format drift |
| Unit (Theotokos dispatch) | encoder jog on virtual params, condition read-modify-write | wrong command, wrong target, repeat/probability clobber |
| Unit (Theotokos render) | MIX screen layout, modifier chips, virtual param glyph | layout breakage, missing chips, missing `▸` prefix |
| Unit (RenderData) | `build_render_data()` with synthetic inputs | data assembly regressions |
| Harness (test-driver) | step-detail bus round-trip (optional; unit test covers the contract) | sequencer→bus→model data loss in the full app loop |
| Live (session #6) | MIX screen feel, modifier visibility, step-size scaling, tap tempo from any screen | everything that matters most |

**Mutation checks:**
- C0: remove the virtual-param dispatch, confirm the encoder jog test
  fails (emits `CMD_BUMP_PARAM` instead of `CMD_SET_STEP_VELOCITY`)
- C0: remove the step-detail bus path, confirm the parse test fails
- C0: remove the condition read-modify-write, confirm repeat/probability
  are clobbered when condition changes
- C0: drop `prob_u8`/`repeat_n`/`repeat_m` from the bus tuple (revert to
  fill-only), confirm the round-trip test fails (repeat/probability read
  back as zero)
- C4: set tier to 0 when it should be 2, confirm the jog-step test fails
- C5: remove the `FUNC+Space` intercept, confirm the tap-tempo test fails
- C5: confirm `FUNC+Space` does NOT produce `ClearLane` (A12 preserved)

---

## §5. Exit criteria

1. `cargo test --workspace` green after every commit.
2. `cargo clippy --workspace` clean on touched crates (diff against
   baseline).
3. Each commit carries unit tests for the new logic (see §4).
4. C0's step-detail bus path is verified by a **unit test** that
   constructs the sequencer, sets per-step params, publishes state,
   extracts the bus text, and parses it through `parse_step_detail()` —
   asserting all seven tuple fields round-trip. A test-driver scenario
   is welcome but not required; the unit test covers the format
   contract.
5. Code review (Flash) on each commit or logical batch.
6. **Usability session #6** — the phase gate. The user plays the
   instrument with the MIX screen, modifier highlight, step-size
   scaling, and global tap tempo. Findings recorded in
   `design/sessions/theotokos-6.md`.

---

## §6. Deferred items (with citations)

| Item | Why deferred | Trigger for re-evaluation |
|---|---|---|
| OQ-T21 (chromatic grammar) | No melodic engine exists | P13 or P14 starts |
| Ordo profile switching/wizard | No session has produced a "I wish I could switch layouts" moment | A session asks for it |
| ADR-045 (cross-surface lock capture) | Parked per session #4 judgment | User decides to unpark |
| #140 (yank lossy) | C0 adds velocity/length/timing/condition to the bus but **not note**; yank still cannot copy the note | A phase needs deep step inspection including note |
| Chain view | Was in original TK3 vision; no session has asked for it | A session asks for it |
| Macro support from `Rule` | Assembly merges macros but no shipped node declares any | A node produces macros |
| WT convergence decision | Resolved by decision in §0 (OQ-T12); no implementation needed | — |
| #172 (cross-track choke) | Engine-level; not a surface concern | A phase owns drum-machine breadth (#173) |
| #173 (toms/claps) | No phase owns machine breadth | User schedules it |
| Per-track scope tap | OQ-T10 resolved as "global primary" | A session asks for it |
| MIX screen key binding | `FUNC+8` is the default; may change after session #6 | Session #6 verdict |

---

## §7. Cross-references

- `design/adr/ADR-044-theotokos-fixed-panel.md` — the panel model
- `design/adr/ADR-037-theotokos-key-remapping.md` — key remapping mechanism
- `design/adr/ADR-038-theotokos-elektron-convergence.md` — Elektron convergence
- `design/theotokos/design.md` — the full design doc (TK3 stub at §7)
- `design/phases/tk2.2-report.md` — the baseline this phase builds on
- `design/sessions/theotokos-5.md` — the most recent session (MM §6.7)
- Issues: #180, #163, #184, #138, #139
