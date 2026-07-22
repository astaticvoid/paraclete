# Paraclete — TK1 Theotokos Specification (Editing Depth)

> **Implementation blueprint.** Contracts only — deviations stop, get recorded
> in the phase report, and go to the user.
>
> **TK1 deliverable:** editing depth on the POC foundation — composite track
> `Rule` pages (OQ-T13), p-lock authoring end-to-end (OQ-T8), mutes (pulled
> from TK2 by session #1), a full `:` command line, pattern select + cue,
> slot rebinding, yank/copy, and the edge-derived track map. Ends in
> **usability session #2**.
> **Last updated:** 2026-07-21
> **Baseline:** TK0 complete with usable sign-off (`tk0-report.md`,
> `design/sessions/theotokos-1.md`); TK1 scope re-cut by session #1 (mutes
> pulled from TK2, `:` upgraded stub → full); W2 C0–C6 shipped.
> **Design authority:** `design/theotokos/problem.md` +
> `design/theotokos/design.md` (as amended 2026-07-21) + ADR-036 +
> `design/phases/tk0-report.md` §implementation-notes.
> **Model routing:** C0 Sonnet-capable; C1–C7 Opus (RT command handling,
> cross-crate extraction, modal state machine); C8 is the user session.
> Chord grammar remains HYPOTHESIS-grade per ADR-036 — do not extend beyond
> this spec without a session.

---

## Ratified design anchors

| Anchor | Resolution (source) |
|---|---|
| Mutation plane | ADR-019 commands + sequencer CMD 16–32 + clock CMD 16/17 + **new lock CMD 33–35 (C1)**. Nothing else. (ADR-036.3) |
| Reads | Shared `StateBusHandle` + startup cap-doc cache + startup topology snapshot via the **existing** `NodeConfigurator::all_edges()` (configurator.rs:376). No JSON, no sockets, no new runtime accessor. (ADR-036.3) |
| Views | Composite `Rule` pages assembled by a **shared crate** both Antiphon and Theotokos consume (C2, OQ-T13). Engine-local Rules remain the fallback. (ADR-036.4 scope note) |
| Tracks | **Edge-derived** (C3): sequencer→generator from event edges, engine→chain from audio edges. ≤8. Never the positional zip (TK0 POC), never the stale AGENTS.md table. |
| Step keys | Invariant across modes (TK0 divergence, settled). Consequence: `,` is step 14 and **cannot** be the leader key — leader is `\` (D4). |
| Page order | Composite pages use one canonical order defined once in the shared crate (`CANONICAL_PAGE_ORDER`); both surfaces agree by construction. This resolves the ADR-032 §8 vs Antiphon divergence — the shared crate is the tiebreaker. |
| Dependencies | Theotokos: unchanged (`ratatui`, `crossterm`, `paraclete-node-api`) + the workspace-internal assembly crate. **No new external deps.** (ADR-036.2) |

---

## Decisions taken in this spec (user ratifies before C1 lands)

| # | Question | Decision | Alternatives rejected |
|---|---|---|---|
| **D1** | OQ-T13 — composite assembly home | **New GPL platform crate `paraclete-view-assembly`** (deps: `paraclete-node-api` only). Antiphon keeps a thin L2→protocol mapping; wire format unchanged. | L2 (`paraclete-node-api`) — assembly is an app-level concern (tracks/chains are not node concepts); freezing it into the LGPL boundary raises the cost of every later change. Reimplementing in Theotokos — drift risk (design.md). |
| **D2** | Mute mechanism | **Sequencer `mute` bank param** (trig-gate): fire paths skip note-on + lock emission while muted; already-sounding notes decay naturally; reverb tails ring. Click-free by construction (no audio-rate gain step); serializes with the project bank; Antiphon clients get it free (CMD_SET_PARAM). | MixNode per-input gain ride — `ParameterBank::set` applies instantly with no dezipper (audible click), all N params share the name `"input_gain"` (name-based resolution ambiguous), and it hard-cuts tails. Trig-gating is also the reference behavior (session #1 pulled mutes in for *performance*; gating trigs is the groovebox semantic). |
| **D3** | OQ-T8 — lock command shape | **Two-command pair + clear** (C1): `CMD_SET_LOCK_TARGET` (33) carries `(node_id, param_id)` at full width; `CMD_SET_STEP_LOCK` (34) carries `(step, value)` with full f64 precision; `CMD_CLEAR_STEP_LOCK` (35). No bit-truncation of node/param ids anywhere. | Single packed command — a lock is step+node+param+value ≈ 102 bits; any one-command packing truncates a u32 id (a frozen-format hardcoded-limit violation, guardrail 6). Value-quantization schemes lose precision or need bit-punning conventions the codebase doesn't have. |
| **D4** | OQ-T2 — leader key | **`\`** — `,` is step 14 on the now-invariant step grid; double-tap-Space conflicts with transport muscle memory. HYPOTHESIS, session #2. | `,` (grid collision), double-tap-Space. |

---

## Commit sequence

| Commit | Crate(s) | Deliverable |
|---|---|---|
| **C0** | `paraclete-theotokos` | **Pre-flight:** BUG-034 page-window stride fix; deprecate `[`/`]` (keep `-`/`=`); publish `/script/theotokos/selected` (OQ-T9 → yes). |
| **C1** | `paraclete-nodes` | **OQ-T8:** p-lock command family (CMD 33/34/35) + dirty-gated lock state publish + overflow counter. |
| **C2** | `paraclete-view-assembly` (new), `paraclete-antiphon`, `paraclete-app` | **OQ-T13:** extract composite assembly to the shared crate; edge-derived chains replace placeholder wiring; Antiphon wire format unchanged. |
| **C3** | `paraclete-theotokos`, `paraclete-app` | **Composite track pages** in PERF mode + edge-derived track map. |
| **C4** | `paraclete-nodes`, `paraclete-theotokos` | **Mutes:** sequencer `mute` param + Shift+track-key toggle + dimmed render. |
| **C5** | `paraclete-theotokos` | **P-lock UI:** step focus, jog→lock routing, lock display, clear gestures. |
| **C6** | `paraclete-theotokos` | **`:` command line:** echo area, generated fuzzy index, v1 verbs. |
| **C7** | `paraclete-theotokos`, app fixture | **Pattern select + cue, yank/paste, slot-rebinding leader, yellow-flash, 8-track stress fixture.** |
| **C8** | — | **Usability session #2** (no code): findings in `design/sessions/theotokos-2.md`, per-hypothesis verdicts, `tk1-report.md`, roadmap + design.md marks updated in the same commit. |

---

## Commit 0 — pre-flight (BUG-034, brackets, selected-publish)

### 0.1 BUG-034 — page-window stride inconsistency

Three stride values disagree in the TK0 code (filed in `bugs.md` under the
standing defect-filing directive):

- `render.rs:101` window base = `page_window * PAGE_SIZE * 2` (16-step grid) ✓
- `action.rs:7,60` toggle offset = `page_window * PAGE_SIZE (8) + col` ✗
- `model.rs:262` `page_count = pattern_length.div_ceil(8)` ✗ (a 16-step
  pattern reports 2 pages, so page 1 is reachable but empty; on a 32-step
  pattern, page-1 toggles hit steps 8–15 which page 0 already shows)

**Fix:** one `GRID_STEPS: usize = 16` constant shared by `action` and `model`
(render already strides 16). Toggle offset = `page_window * GRID_STEPS + col`;
`page_count = pattern_length.div_ceil(GRID_STEPS)`. Update the four `lib.rs`
tests encoding the stale semantics (`bracket_*`, `toggle_step_includes_page_window_offset`).

### 0.2 Deprecate bracket page-nav (session #1 finding)

Remove `[`/`]`/`{`/`}` from `map_seq`; `-`/`=` are the only page-window keys.
Update `input.rs` tests.

### 0.3 Publish `/script/theotokos/selected` (OQ-T9 → decided yes)

On `SelectTrack` (and once at startup): write
`/script/theotokos/selected` = `StateBusValue::Int(active track's sequencer
node id)` via the shared `StateBusHandle` (in-process, not sandboxed —
design.md §2.2). Other surfaces may follow; no surface is required to.
**Non-goal:** the legacy `/script/selected_track` path (BUG-033) stays
parked — that is a tui/profile concern, not Theotokos's.

### 0.4 Tests (named)

- `page_window_stride_is_16_steps` (toggle on page 1 hits step 16+col)
- `page_count_for_16_step_pattern_is_one` (']' clamps at page 0)
- `page_count_for_32_step_pattern_is_two`
- `bracket_keys_are_noop`; `minus_equals_navigate_pages`
- `select_track_publishes_selected_sequencer_id`

---

## Commit 1 — p-lock command family (OQ-T8, D3)

The sequencer stores per-step locks (`StepParamLock { node_id, param_id,
value }`, sequencer.rs:130) and emits them as `ParamLockEvent` on all four
fire paths; engines consume them via the per-cycle `node_locks` pattern.
**The only missing piece is an authoring path** — locks are written today
only by the v3 deserializer and tests. This commit adds it.

### 1.1 Constants (`sequencer.rs`)

```rust
pub const CMD_SET_LOCK_TARGET: u32 = 33;
pub const CMD_SET_STEP_LOCK:   u32 = 34;
pub const CMD_CLEAR_STEP_LOCK: u32 = 35;
const LOCK_CAP_PER_STEP: usize = 8;
```

Antiphon's `node_cmd` gate admits type_id ≥ 16 — all three pass it, so
tablet-side lock authoring is possible later with no protocol change.

### 1.2 Wire semantics

| Command | arg0 | arg1 | Effect |
|---|---|---|---|
| `CMD_SET_LOCK_TARGET` | `node_id` (i64 carrying full u32) | `param_id` as f64 (u32 exact, < 2⁵³) | Sets `lock_target: Option<(u32, u32)>`. Idempotent. Surfaces **always** send this paired immediately before 34. |
| `CMD_SET_STEP_LOCK` | step index (0–63) | value (f64, full precision) | Requires `lock_target`; else drop + bump `lock_dropped`. Step has a lock for the target → **update in place**; else push if `param_locks.len() < LOCK_CAP_PER_STEP`; else drop + bump `lock_dropped`. Sets `locks_dirty` on any change. |
| `CMD_CLEAR_STEP_LOCK` | step index | `param_id` as f64, **−1.0 = all** | arg1 = −1.0 → clear every lock on the step; otherwise clear only the `(lock_target.node_id, param_id)` lane. Sets `locks_dirty` only when something was removed. |

Out-of-range step (`≥ STEP_CAPACITY` or `≥ pattern.length`) → drop silently
(executor drop-safe idiom).

### 1.3 Real-time rules

- `Pattern` construction reserves `param_locks` capacity `LOCK_CAP_PER_STEP`
  per step; `deserialize()` re-reserves to `max(LOCK_CAP_PER_STEP, len)` after
  loading (both run on the main thread — the CHAIN_CAP pre-reserve
  precedent, sequencer.rs:405). `Vec::push` within reserved capacity never
  allocates on the audio thread.
- The command scan follows the established inherent-scan idiom at the top of
  `process()` — no `Node::handle_commands` addition (L2 boundary).
- `lock_dropped: u64` published as `/node/{id}/state/lock_dropped` (Int).

### 1.4 Lock state publish (dirty-gated)

`/node/{id}/state/locks` — `Text`, published **only when `locks_dirty`** (set
by CMD 34/35 and `deserialize`; cleared after publish). Grammar:

```
locks := (entry (";" entry)*)?
entry := "s" step ":" node_id ":" param_id "=" value   // value: {:.3}
```

Entries ordered by step, then insertion. Built with `core::fmt::Write` into
a reused `String` scratch (no `format!` chain); cloned into the bus value
only on dirty cycles — zero per-cycle cost when idle, strictly cheaper than
the existing per-cycle `/state/steps` publish.

### 1.5 Tests (named)

- `set_lock_target_then_step_lock_stores_lock`
- `step_lock_without_target_drops_and_counts`
- `step_lock_updates_in_place_when_same_target`
- `step_lock_overflow_at_cap_drops_and_counts`
- `clear_step_lock_all_lanes_with_minus_one` / `clear_step_lock_target_lane_only`
- `authored_lock_fires_param_lock_event_at_step` (process-level; mirrors the
  existing deserializer-lock fire test)
- `authored_lock_roundtrips_v3_serializer`
- `locks_publish_only_when_dirty`
- existing condition/timing/serializer/fire tests stay green

### 1.6 Test-driver scenario

`tools/test-driver/tests/plock_authoring.yaml`: 4-on-the-floor kick; author
a much-shorter `decay` lock on step 2 via the 33+34 pair; assert windowed
RMS around step 2 is below the step-0 window (`peak_lt`-family assertion).
This is the headless proof that the same command vocabulary Theotokos emits
moves audio. Baseline-fingerprint it (ADR-035).

---

## Commit 2 — composite `Rule` assembly extraction (OQ-T13, D1)

### 2.1 New crate

`crates/paraclete-view-assembly` (GPL-3.0 platform crate, same standing as
`paraclete-antiphon`; deps: `paraclete-node-api` **only**). Root
`Cargo.toml` `members` is explicit — add it; add the dep to
`paraclete-antiphon` (this commit) and `paraclete-app` (C3).

### 2.2 Types (plain Rust, no serde)

```rust
pub struct TrackChain { pub engine_node_id: u32, pub chain_ids: Vec<u32> }

pub struct CompositeView {
    pub engine_node_id: u32,
    pub engine_name: String,
    pub display_name: String,
    pub pages: Vec<CompositePage>,
    pub chain: Vec<u32>,            // engine + chain_ids, signal order
    pub routes: Vec<CompositeRoute>,
}
pub struct CompositePage {
    pub id: String,                 // page-group name
    pub label: String,              // "Source"/"Amp"/… (same mapping as today)
    pub params: Vec<CompositeParam>,
    pub envelopes: Vec<CompositeEnvelope>,
    pub macros: Vec<CompositeMacro>,
}
pub struct CompositeParam {
    pub node_id: u32,               // the owning node (engine or chain node)
    pub param_id: u32,
    pub name: String,
    pub label: String,
    pub affordance: AffordanceHint, // the value, not a JSON string
    pub env_group: Option<u32>,
    pub slot: u8,
    pub routing: Option<String>,
}
pub struct CompositeEnvelope { pub id: u32, pub env_type: String, pub label: String, pub params: Vec<(u32, String)> } // (param_id, name)
pub struct CompositeMacro { pub name: String, pub targets: Vec<(u32, String)>, pub page: Option<String> }
pub struct CompositeRoute { pub source: u32, pub dest: String, pub param_id: u32, pub param_name: String }

pub const CANONICAL_PAGE_ORDER: [&str; 6] = ["SRC", "AMP", "FLTR", "FX", "TRIG", "MOD"];

pub fn assemble(
    rules: &HashMap<u32, Rule>,
    chains: &[TrackChain],
    track_id: u32,
    caps: &HashMap<u32, CapabilityDocument>,
) -> Option<CompositeView>;
```

Semantics are exactly today's `ViewRegistry::assemble` + `merge_page`
(antiphon/src/view.rs), with three deliberate changes:

1. Param-name lookup uses cap-docs (`HashMap<u32, CapabilityDocument>`)
   instead of protocol `NodeSummary` — both consumers have cap-docs.
2. `affordance` is the `AffordanceHint` value; JSON-string conversion moves
   to Antiphon's mapping layer.
3. Routes carry no `value` field (today hardcoded `0.0`); Antiphon adds its
   placeholder at mapping time. Live route values remain a W-track concern.

### 2.3 Antiphon becomes a thin mapper

`view.rs` shrinks to `CompositeView → ServerMsg::ViewMeta` conversion
(one `impl`+helpers). **Wire format unchanged** — the web client is
untouched; guard with existing protocol tests plus
`viewmeta_wire_unchanged_after_extraction` (fixture CompositeView → assert
the exact JSON shape the web client parses today).

### 2.4 Edge-derived chains (`main.rs::build_view_registry`)

Replace the placeholder ("first filter/distortion/reverb found in the node
list", main.rs:796-829) with topology-derived chains:

- Use the **existing** `conf.all_edges()` (configurator.rs:376 — no runtime
  change). `EdgeView` carries no port type; derive audio vs event edges from
  the cap-doc port lists of both endpoints (`PortType`).
- **Chains:** for each engine (type_tag discovery as today), BFS downstream
  along audio edges in signal order, collecting nodes whose cap-doc has
  `view.is_some()`, stopping before `mix`/`audio_output` type_tags.
- **Sequencer→engine pairs** (also feeds C3's track map): follow event
  edges from each sequencer to the first node with an audio output.

Snapshot semantics are unchanged from today (built once at startup; no
rebuild on `apply_patch` — the standalone TK-track graph is static
post-load; documented limitation).

### 2.5 Tests (named)

- `assemble_merges_engine_and_chain_pages_in_canonical_order`
- `assemble_custom_pages_alphabetical_after_canonical`
- `assemble_envelope_group_indices_offset_across_nodes`
- `assemble_missing_engine_rule_returns_none`
- `assemble_param_carries_owning_node_id`
- `edge_derived_chain_follows_signal_order` (app-side fixture graph)
- `viewmeta_wire_unchanged_after_extraction`

---

## Commit 3 — composite track pages + edge-derived track map

### 3.1 App wiring

`build_view_registry`'s derivation (C2) is factored so `main.rs` builds
**one** `Vec<TrackChain>` + rules map used for both the Antiphon registry
and `TheotokosConfig`. `TheotokosConfig` gains:

```rust
pub composite: Vec<CompositeView>,   // one per track, same order as tracks
```

and `seq_ids`/`gen_ids` are filled from the edge-derived pairs (the
positional zip is deleted).

### 3.2 PERF mode consumes composite pages

- `1`–`6` selects into `composite.pages[track]` (canonical order, C2);
  page tabs render composite labels.
- Slot binding uses `CompositeParam.node_id` — a FLTR page binds A/B to the
  **filter node's** cutoff/resonance, not the generator. Fallback chain
  preserved: composite → engine-local `Rule` → first two cap-doc params.
- Envelope gauge reads through composite envelope refs (node-aware).
- Slot C stays unbound (TK2 per design.md §3.3; FX pages bind A/B only).

### 3.3 Tests (named)

- `perf_page_selects_composite_page_and_binds_chain_node`
- `slot_binding_uses_param_node_id_not_generator`
- `composite_fallback_to_engine_local_when_absent`
- `page_tabs_render_composite_labels` (TestBackend)
- `track_map_is_edge_derived_not_positional` (app-side: reordered YAML ids
  still pair seq↔engine by edges)

---

## Commit 4 — mutes (D2)

### 4.1 Sequencer `mute` param (`paraclete-nodes`)

- New declared bank param `"mute"` (stepped, min 0, max 1, default 0) on
  `Sequencer`. The bank handles `CMD_SET_PARAM`/`CMD_BUMP_PARAM` for free;
  `publish_bank_state` mirrors `/node/{id}/param/mute` for free; project
  bank serialization persists it for free.
- All four fire paths skip **note-on and `ParamLockEvent` emission** while
  `mute >= 0.5`. Note-offs for already-fired notes are unaffected (natural
  decay; shared reverb tails ring — the documented trig-gate semantic).
- Playhead/step publish continues while muted (the track keeps moving —
  the performer sees the pattern it will rejoin).

### 4.2 Theotokos toggle + render

- **Shift+track-key** (`Q W E R U I O P` — `KeyCode::Char` with SHIFT, free
  today) toggles `mute` on that track, **invariant across modes** (same
  muscle-memory rule as track select). Emits `CMD_SET_PARAM` on the track's
  sequencer with the flipped value (read current from
  `/node/{id}/param/mute`).
- Muted tracks render with the label in `DarkGray` + an `M` marker after the
  track number; step cells keep their state colors (the pattern is still
  there, just gated).

### 4.3 Tests (named)

- `muted_sequencer_skips_note_and_lock_emission`
- `muted_sequencer_still_publishes_playhead`
- `unmute_resumes_at_next_step`
- `keymap_shift_track_toggles_mute` (input, both modes)
- `mute_toggle_reads_bus_and_flips_value`
- `render_muted_track_dims_label_with_m_marker` (TestBackend)

---

## Commit 5 — p-lock UI

### 5.1 Step focus (OQ-T1 focus model; hold-step stays a session probe)

- Model tracks `last_step: Option<usize>` per track (updated by every
  `ToggleStep`).
- `Enter` toggles **step focus** on the active track's `last_step`;
  focused step renders `Reversed`. `Enter` again or `Esc` releases
  (`Esc` gains its first real semantics: cancel/release — global).
- Focus is per-track; switching tracks keeps each track's focus.

### 5.2 Jog→lock routing

While a step is focused on the active track, A/B jogs emit the **pair**
instead of `CMD_BUMP_PARAM`:

```
CMD_SET_LOCK_TARGET { arg0: slot.node_id, arg1: slot.param_id as f64 }
CMD_SET_STEP_LOCK   { arg0: focused_step, arg1: new_value }
```

`new_value` = existing lock value for (step, slot param) parsed from the
`/state/locks` mirror, else the live bus value; ± the usual jog delta
(same `Tuning` math, same fine/coarse modifiers). Lock target = the
most-recently-jogged slot's `(node_id, param_id)`; default slot A.

### 5.3 Display

- Mode line: `F:s{step}` + per-slot lock values with an `L` marker when the
  displayed value comes from a lock (`A:decay=0.300L`).
- Grid: steps carrying any lock render distinctly — active+locked → `Green`;
  locked-inactive → `White`. Focused step → `Reversed`. (Glyphs tunable;
  the render test asserts only that the three states are distinguishable.)

### 5.4 Clear gestures

- `Backspace` with focus → `CMD_CLEAR_STEP_LOCK` arg1 = −1.0 (all lanes).
- `Shift+Backspace` with focus → arg1 = slot A's param (target lane only).

### 5.5 Tests (named)

- `enter_focuses_last_toggled_step`; `esc_releases_focus`
- `jog_while_focused_emits_target_then_lock_pair`
- `jog_lock_value_starts_from_existing_lock`
- `jog_lock_value_starts_from_live_when_no_lock`
- `jog_without_focus_still_bumps_param` (no regression)
- `backspace_clears_all_lanes`; `shift_backspace_clears_target_lane`
- `render_lock_states_distinguishable` (TestBackend)

---

## Commit 6 — `:` command line (full)

### 6.1 Layout & modal capture

- Echo area realized from design.md §5.1: vertical layout becomes
  `[transport 2, window min, echo 1, mode 1]`.
- `:` opens the line (global, both modes); `cmdline: Option<String>` on the
  Model. While open, **all** keys route to the line editor (modal capture);
  `Enter` executes, `Esc` cancels, `Backspace` edits.

### 6.2 Generated fuzzy index

Built once at startup from cap-docs + composite views + static verbs.
Matching: subsequence; rank exact-prefix > subsequence, shorter name wins
ties. Echo area shows the top candidate + up to 4 alternates live as you
type.

### 6.3 v1 verbs (execute through the same Action/NodeCommand egress)

| Verb | Effect |
|---|---|
| `set <param> <value>` | absolute `CMD_SET_PARAM` on the active track's composite param (fuzzy param match) |
| `bpm <n>` | `CMD_SET_PARAM` `bpm` on the clock (20–300 clamp, as C0/TK0) |
| `track <n>` | select track n |
| `pattern <n>` | `CMD_SET_PATTERN` n−1 on the active track (cued semantics, C7 render lands in C7) |
| `mute <n>` / `unmute <n>` | explicit `CMD_SET_PARAM` `mute` 1.0/0.0 on track n |
| `clear` | `CMD_CLEAR` on the active track (echo "cleared T{n}") |
| `lock-clear` | clear all locks on the focused step |
| `mode seq\|perf` | mode switch |

Unknown input → echo `?{input}` in red; never crashes, never partially
executes.

### 6.4 Tests (named)

- `colon_opens_and_esc_cancels`; `cmdline_captures_all_keys_while_open`
- `enter_executes_set_with_fuzzy_param_match`
- `fuzzy_ranking_prefers_prefix_over_subsequence`
- `bpm_command_sends_set_param_to_clock`
- `mute_command_sends_explicit_value`
- `unknown_command_echoes_error_no_crash`
- `render_echo_area_shows_candidates` (TestBackend)

---

## Commit 7 — patterns, yank/paste, rebinding, flash, 8-track stress

### 7.1 Pattern select + cue (SEQ mode)

- `1`–`8` in SEQ → `CMD_SET_PATTERN` arg0 = n−1 on the active track
  (existing engine semantics: immediate when stopped, cued when playing).
  PERF keeps the number row for pages — mode-scoped meanings, the modal
  bargain.
- Transport bar: `PT{active+1}` + `→{cued+1}` cue marker, read from
  `/node/{id}/state/active_pattern` and `/state/cued_pattern` (−1 = none).

### 7.2 Yank / paste

- `y` yanks the active track's current pattern (all `Step` fields — active,
  note, velocity, condition, timing, **and locks** — into a Model-side
  copy buffer).
- `Y` (Shift+y — `P` is taken by Shift+track-8 mute) pastes over the active
  track's current pattern: per step, emit `CMD_SET_STEP` (17) +
  `CMD_SET_STEP_TIMING` (25) + `CMD_SET_STEP_CONDITION` (26) + the lock
  pairs (33+34) — one drained batch, ≤ ~70 commands for 16 steps. Clamps to
  `min(src_len, dst_len)`; does not touch length/speed.
- Cross-track paste works (yank T1, select T3, paste). Paste is
  destructive-by-design (overwrites); **no confirm, no undo** — HYPOTHESIS,
  session #2 judges.

### 7.3 Slot-rebinding leader (`\`, D4)

- `\` enters leader state; `a`/`b` picks the slot; digit `1`–`9` binds that
  slot to the nth param of the current page (composite order). `Esc`
  cancels mid-chord; anything else is Noop + exits leader.
- Example: `\b3` = bind slot B to the 3rd param of the current page.
- Mode line shows a `\_` leader pending indicator.

### 7.4 Yellow-flash on param change (session #1 finding)

- Model records the `Instant` of the last bus-value change per slot
  (detected by frame-read delta). Render colors that slot's mode-line value
  `Yellow` while age < `flash_ms` (a `Tuning` const, default 400) — the old
  TUI encoder-flash idiom.

### 7.5 Eight-track stress

- New dev fixture `instrument-8track.yaml` (repo root, loadable via the
  app's existing instrument-path mechanism): sequencers 10–17 → voices
  20,21,22,23,24,25,26,27 per the AGENTS.md aspirational graph (23–26 =
  Samplers; `gen-samples` output required, as for the default instrument).
- Discovery must yield 8 tracks with keys `q`–`p` mapped 0–7; a synthetic
  9-track config clamps to 8.

### 7.6 Tests (named)

- `seq_number_row_sends_set_pattern`
- `render_shows_active_and_cued_pattern_markers` (TestBackend)
- `yank_then_paste_emits_full_step_command_batch`
- `paste_clamps_to_shorter_pattern`
- `paste_includes_condition_timing_and_lock_commands`
- `leader_rebind_b3_binds_third_page_param`; `leader_esc_cancels_chord`
- `flash_colors_value_within_window` (render)
- `eight_track_fixture_discovers_eight_tracks`
- `nine_tracks_clamp_to_eight`

---

## Exit criteria

1. `cargo test --workspace` green; clippy clean on touched crates.
2. Agent smoke (default 4-track **and** the 8-track fixture): Shift+track
   mute audibly gates a track; a focused-step p-lock audibly reshapes one
   step (the C1 test-driver scenario passes); PERF FLTR page binds the
   chain node's params; `:` runs `bpm 90`, `mute 2`, `set cutoff 0.4`.
3. **Usability session #2** held; findings append-only in
   `design/sessions/theotokos-2.md`; per-hypothesis verdicts
   (converged/revise/park); `tk1-report.md` written; roadmap + design.md
   marks updated in the same commit.

## Explicit non-goals (TK1)

Slot C bindings (TK2); momentary ALT/numpad-`0` layer (TK2); CHAIN mode and
chain push/clear UI (TK2); MIX mode (TK3); live visualization — env/LFO
phase, scope tap (TK2, design.md §5.3); note/velocity step editing
(`CMD_SET_STEP` note entry — later); numpad type-in entry (OQ-T5, TK2);
keymap config (TK3); `LfoNode`/`EnvelopeNode` `Rule` declarations (TK2);
fill keys (TK2 performance layer); legacy `/script/selected_track`
(BUG-033, parked); undo (session #2 may re-cut); WT convergence (OQ-T12,
TK3).

## Hypotheses under test in session #2

| Hypothesis | Where introduced |
|---|---|
| `\` leader for rebinding/paste family (OQ-T2) | C7 |
| Shift+track-key mute chord (invariant across modes) | C4 |
| Step-focus p-lock flow vs hold-step (OQ-T1 probe — hold needs kitty release events; focus is the numpad-less-safe model) | C5 |
| `:` fuzzy discoverability + v1 verb sufficiency | C6 |
| Paste without confirm/undo | C7 |
| Number-row pattern select in SEQ vs pages in PERF (same keys, mode-scoped) | C7 |
| Yellow-flash duration/legibility | C7 |
| Composite page order legibility (canonical merge order on both surfaces) | C3 |

## Implementation notes for the TK1 builder

- **`handle_keys(&bus, &[KeyEvent])` is the test seam** — add key-injection
  tests there; never through `tick()` (needs crossterm). (tk0 report)
- **Drop `bus_ref` before `terminal.draw()`**; rendering reads all state
  from the bus each frame, no local cache to invalidate. (tk0 report)
- **`NodeConfigurator::all_edges()` already exists** (configurator.rs:376) —
  C2 needs **no** runtime-crate change. `EdgeView` has no port type; derive
  audio/event from cap-doc port lists.
- **Crossterm cannot distinguish numpad from main row** (0.27) — unchanged
  constraint; arrows remain the jog cluster; numpad mapping needs Kitty
  extended keys (TK2+).
- **The default instrument is 4-track** (seqs 10–13, voices 20–22/27);
  the AGENTS.md 8-track table is aspirational — C7's fixture realizes it.
- **MixNode today:** all input params share the name `"input_gain"`, no
  smoothing, no `published_state`, `view: None` on the cap-doc despite an
  existing `ViewPlugin` impl. D2 routes around all of this; a proper
  MixNode pass is P11 scope.
- **`x`-as-clear from design.md §3.2 is superseded** — `x` is step 11 on
  the invariant grid; clear moves to the `:` line (`clear`) and locks to
  `Backspace`.
