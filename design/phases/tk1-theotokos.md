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
> **Review:** pre-implementation subagent design review held 2026-07-21
> (same gate as TK0). No blockers; findings M1–M7 + m1–m12 folded inline
> (mute publish/persistence, paste completeness, clear-lane pairing, mode
> scoping, test-driver actions, fixture chains, display-name source, lock
> mirror precision, `&self` publish mechanics, wire deltas, capacity
> asymmetry, §3.3 amendment, `clap_plugin` discovery, mute scenario, fifth
> stale test). D1–D4 ratified by the user 2026-07-21.

---

## Ratified design anchors

| Anchor | Resolution (source) |
|---|---|
| Mutation plane | ADR-019 commands + sequencer CMD 16–32 + clock CMD 16/17 + **new sequencer CMD 33–37 (C1)**. Nothing else. (ADR-036.3) |
| Reads | Shared `StateBusHandle` + startup cap-doc cache + startup topology snapshot via the **existing** `NodeConfigurator::all_edges()` (configurator.rs:376). No JSON, no sockets, no new runtime accessor. (ADR-036.3) |
| Views | Composite `Rule` pages assembled by a **shared crate** both Antiphon and Theotokos consume (C2, OQ-T13). Engine-local Rules remain the fallback. (ADR-036.4 scope note) |
| Tracks | **Edge-derived** (C3): sequencer→generator from event edges, engine→chain from audio edges. ≤8. Never the positional zip (TK0 POC), never the stale AGENTS.md table. |
| Step keys | Invariant across modes (TK0 divergence, settled). Consequence: `,` is step 14 and **cannot** be the leader key — leader is `\` (D4). |
| Page order | Composite pages use one canonical order defined once in the shared crate (`CANONICAL_PAGE_ORDER`); both surfaces agree by construction. This resolves the ADR-032 §8 vs Antiphon divergence — the shared crate is the tiebreaker. |
| Dependencies | Theotokos: unchanged (`ratatui`, `crossterm`, `paraclete-node-api`) + the workspace-internal assembly crate. **No new external deps.** (ADR-036.2) |

---

## Decisions taken in this spec (**ratified by the user 2026-07-21**)

| # | Question | Decision | Alternatives rejected |
|---|---|---|---|
| **D1** | OQ-T13 — composite assembly home | **New GPL platform crate `paraclete-view-assembly`** (deps: `paraclete-node-api` only). Antiphon keeps a thin L2→protocol mapping; wire format unchanged. | L2 (`paraclete-node-api`) — assembly is an app-level concern (tracks/chains are not node concepts); freezing it into the LGPL boundary raises the cost of every later change. Reimplementing in Theotokos — drift risk (design.md). |
| **D2** | Mute mechanism | **Sequencer `mute` bank param** (trig-gate): fire paths skip note-on + lock emission while muted; already-sounding notes decay naturally; reverb tails ring. Click-free by construction (no audio-rate gain step); Antiphon clients get it free (CMD_SET_PARAM). Bus mirroring + v3 persistence are explicit C4 work (review M1 — the sequencer never calls `publish_bank_state` today). | MixNode per-input gain ride — `ParameterBank::set` applies instantly with no dezipper (audible click), all N params share the name `"input_gain"` (name-based resolution ambiguous), and it hard-cuts tails. Trig-gating is also the reference behavior (session #1 pulled mutes in for *performance*; gating trigs is the groovebox semantic). |
| **D3** | OQ-T8 — lock command shape | **Two-command pair + clear** (C1): `CMD_SET_LOCK_TARGET` (33) carries `(node_id, param_id)` at full width; `CMD_SET_STEP_LOCK` (34) carries `(step, value)` with full f64 precision; `CMD_CLEAR_STEP_LOCK` (35). No bit-truncation of node/param ids anywhere. | Single packed command — a lock is step+node+param+value ≈ 102 bits; any one-command packing truncates a u32 id (a frozen-format hardcoded-limit violation, guardrail 6). Value-quantization schemes lose precision or need bit-punning conventions the codebase doesn't have. |
| **D4** | OQ-T2 — leader key | **`\`** — `,` is step 14 on the now-invariant step grid; double-tap-Space conflicts with transport muscle memory. HYPOTHESIS, session #2. | `,` (grid collision), double-tap-Space. |

---

## Commit sequence

| Commit | Crate(s) | Deliverable |
|---|---|---|
| **C0** | `paraclete-theotokos` | **Pre-flight:** ~~BUG-034 page-window stride fix~~ (**DONE 2026-07-22** — `GRID_STEPS=16`; commits `fd8d4ac`); deprecate `[`/`]` (keep `-`/`=`); publish `/script/theotokos/selected` (OQ-T9 → yes). |
| **C1** | `paraclete-nodes`, `tools/test-driver` | **OQ-T8:** p-lock command family (CMD 33/34/35) + step-field commands (CMD 36/37, for paste) + dirty-gated lock state publish + overflow counter + new test-driver actions. |
| **C2** | `paraclete-view-assembly` (new), `paraclete-antiphon`, `paraclete-app` | **OQ-T13:** extract composite assembly to the shared crate; edge-derived chains replace placeholder wiring; Antiphon wire format unchanged. |
| **C3** | `paraclete-theotokos`, `paraclete-app` | **Composite track pages** in PERF mode + edge-derived track map. |
| **C4** | `paraclete-nodes`, `paraclete-theotokos` | **Mutes:** sequencer `mute` param + Shift+track-key toggle + dimmed render. |
| **C5** | `paraclete-theotokos` | **P-lock UI:** step focus, jog→lock routing, lock display, clear gestures. **DONE 2026-07-22.** |
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
`page_count = pattern_length.div_ceil(GRID_STEPS)`. Update the **five** tests
encoding the stale semantics: the four `lib.rs` tests (`bracket_*`,
`toggle_step_includes_page_window_offset`) **and** `action.rs:89-96`
(`toggle_step_offset_includes_page_window`, review m12).

### 0.2 Deprecate bracket page-nav (session #1 finding)

Remove `[`/`]`/`{`/`}` from `map_seq`; `-`/`=` are the only page-window keys.
Update `input.rs` tests **and the AGENTS.md keyboard table** (the
"`[] or -= = page window`" line → `-`/`=` only).

### 0.3 Publish `/script/theotokos/selected` (OQ-T9 → decided yes)

On `SelectTrack` (and once at startup): write
`/script/theotokos/selected` = `StateBusValue::Int(active track's sequencer
node id)` (`Int` is `i64`, state_bus.rs:6-11; direct in-process write
precedent: main.rs:455-462). Other surfaces may follow; no surface is
required to. **Mechanics (review m6):** `handle_keys` holds an immutable
bus borrow across its whole loop — collect bus writes during the loop and
apply them after `drop(bus_ref)`; the startup publish happens from
`main.rs` after `TheotokosApp::new` (the bus handle is in scope there), not
inside `new`.
**Non-goal:** the legacy `/script/selected_track` path (BUG-033) stays
parked — that is a tui/profile concern, not Theotokos's.

### 0.4 Tests (named)

- `page_window_stride_is_16_steps` (toggle on page 1 hits step 16+col)
- `page_count_for_16_step_pattern_is_one` (']' clamps at page 0)
- `page_count_for_32_step_pattern_is_two`
- `bracket_keys_are_noop`; `minus_equals_navigate_pages`
- `select_track_publishes_selected_sequencer_id`

---

## Commit 1 — p-lock + step-field command family (OQ-T8, D3)

The sequencer stores per-step locks (`StepParamLock { node_id, param_id,
value }`, sequencer.rs:130) and emits them as `ParamLockEvent` on all four
fire paths; engines consume them via the per-cycle `node_locks` pattern.
**The only missing piece is an authoring path** — locks are written today
only by the v1/v2/v3 deserializers and tests. This commit adds it, plus the
two step-field commands (velocity, length) that C7's paste needs (review M2).

### 1.1 Constants (`sequencer.rs`)

```rust
pub const CMD_SET_LOCK_TARGET:   u32 = 33;
pub const CMD_SET_STEP_LOCK:     u32 = 34;
pub const CMD_CLEAR_STEP_LOCK:   u32 = 35;
pub const CMD_SET_STEP_VELOCITY: u32 = 36;
pub const CMD_SET_STEP_LENGTH:   u32 = 37;
const LOCK_CAP_PER_STEP: usize = 8;
```

Antiphon's `node_cmd` gate admits type_id ≥ 16 (server.rs:144-149) — all
five pass it, so tablet-side authoring is possible later with no protocol
change.

### 1.2 Wire semantics

| Command | arg0 | arg1 | Effect |
|---|---|---|---|
| `CMD_SET_LOCK_TARGET` | `node_id` (i64 carrying full u32) | `param_id` as f64 (u32 exact, < 2⁵³) | Sets `lock_target: Option<(u32, u32)>`. Idempotent. Surfaces **always** send this paired immediately before 34 (and before a lane-targeted 35 — review M3). |
| `CMD_SET_STEP_LOCK` | step index (0–63) | value (f64, full precision) | Requires `lock_target`; else drop + bump `lock_dropped`. Step has a lock for the target → **update in place**; else push if `param_locks.len() < LOCK_CAP_PER_STEP`; else drop + bump `lock_dropped`. Sets `locks_dirty` on any change. |
| `CMD_CLEAR_STEP_LOCK` | step index | `param_id` as f64, **−1.0 = all** | arg1 = −1.0 → clear every lock on the step; otherwise clear only the `(lock_target.node_id, param_id)` lane. Sets `locks_dirty` only when something was removed. |
| `CMD_SET_STEP_VELOCITY` | step index | velocity (0.0–1.0) | Sets `Step.velocity` (CMD 17 does not carry it — sequencer.rs:520-531). |
| `CMD_SET_STEP_LENGTH` | step index | length (same unit/scale as the v3 step record's f32) | Sets `Step.length` (no authoring path exists today). |

Out-of-range step → drop at **`≥ STEP_CAPACITY` only** (review m8): locks
and fields on steps beyond the current `pattern.length` are inert but
preserved — matching `read_step_record`'s acceptance of all 64 capacity
steps and `pattern_is_used` — so a later `CMD_SET_LENGTH` up never loses
authored data.

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
entry := "s" step ":" node_id ":" param_id "=" value   // value: {:.6}
```

Entries ordered by step, then insertion. **`{:.6}` not `{:.3}`** (review m1):
fine jog steps are `range/128/8` ≈ 0.000125 on [0,1] params; 3-decimal
rounding would swallow sub-0.0005 adjustments and accumulate drift, since
C5 re-parses the mirror as each jog's base. **Mechanics (review m2):**
`published_state` takes `&self` — `locks_dirty` is a `Cell<bool>`; build a
fresh `String` on dirty cycles only (one bounded alloc per edit gesture,
strictly cheaper than the existing per-cycle `steps_bitfield()` alloc —
sequencer.rs:1057). No reused scratch buffer needed.

### 1.5 Tests (named)

- `set_lock_target_then_step_lock_stores_lock`
- `step_lock_without_target_drops_and_counts`
- `step_lock_updates_in_place_when_same_target`
- `step_lock_overflow_at_cap_drops_and_counts`
- `clear_step_lock_all_lanes_with_minus_one` / `clear_step_lock_target_lane_only`
- `step_lock_beyond_length_preserved_inert` (drop at capacity only, m8)
- `set_step_velocity_and_length_commands`
- `authored_lock_fires_param_lock_event_at_step` (process-level; mirrors the
  existing deserializer-lock fire test)
- `authored_lock_roundtrips_v3_serializer`
- `locks_publish_only_when_dirty`
- existing condition/timing/serializer/fire tests stay green

### 1.6 Test-driver actions + scenario

`tools/test-driver` gains three scenario actions (its `dispatch_action`
vocabulary is closed today): `set_lock_target`, `set_step_lock`,
`clear_step_lock` (same argument shape as the commands). C1's crate list
includes `tools/test-driver` (review M5).

Scenario `tools/test-driver/tests/plock_authoring.yaml`: 4-on-the-floor
kick; author a much-shorter `decay` lock on step 2 via the 33+34 pair;
assert windowed RMS around step 2 is below the step-0 window
(`peak_lt`-family assertion). This is the headless proof that the same
command vocabulary Theotokos emits moves audio. Baseline-fingerprint it
(ADR-035).

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

/// Thin per-node view input both consumers can build (review M7+m3):
/// the app builds it from instrument labels + cap-docs; Antiphon already
/// holds the equivalent in `NodeSummary`/`ParamSummary`.
pub struct NodeInfo {
    pub display_name: Option<String>,   // instrument-file label, if any
    pub params: Vec<(u32, String)>,     // (param_id, name)
}

pub struct CompositeView {
    pub engine_node_id: u32,
    pub engine_name: String,
    pub display_name: String,
    pub pages: Vec<CompositePage>,
    pub chain: Vec<u32>,            // rule-bearing nodes only, engine first (today's wire)
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
    nodes: &HashMap<u32, NodeInfo>,
) -> Option<CompositeView>;
```

Semantics are exactly today's `ViewRegistry::assemble` + `merge_page`
(antiphon/src/view.rs), with four deliberate changes:

1. Param-name + display-name lookup goes through `NodeInfo` (review M7+m3):
   `NodeSummary` is protocol-shaped and Antiphon never sees cap-docs, while
   `NodeSummary.name` carries the instrument-file label the cap-doc lacks.
   `NodeInfo` is the intersection both sides already have. Wire content is
   preserved exactly, including `display_name`.
2. `affordance` is the `AffordanceHint` value; JSON-string conversion moves
   to Antiphon's mapping layer.
3. Routes carry no `value` field (today hardcoded `0.0`); Antiphon adds its
   placeholder at mapping time. Live route values remain a W-track concern.
4. `chain` lists **rule-bearing chain nodes only, engine first** — exactly
   today's `chain_meta.nodes` (review m4a); viewless chain nodes stay
   invisible, matching the current wire.

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
  change; valid after `build_executor()` — the graph is retained).
  `EdgeView` carries no port type; derive audio vs event edges from the
  cap-doc port lists of both endpoints (`PortType`).
- **Chains:** for each engine (type_tag discovery as today **plus
  `clap_plugin`** — review m10: CLAP-based tracks currently get no
  composite view), BFS downstream along audio edges in signal order,
  collecting nodes whose cap-doc has `view.is_some()`, stopping before
  `mix`/`audio_output` type_tags.
- **Sequencer→engine pairs** (also feeds C3's track map): follow event
  edges from each sequencer to the first node with an audio output.
- **Known behavior change (review m4b):** edge-derived chains stop at the
  mix, so the **shared post-mix reverb leaves the per-track composite** on
  the stock instrument (today's placeholder slaps the first-found reverb
  onto every track). This is the more correct model (reverb is a master
  FX, not per-track), but it is user-visible on the web FX page — call it
  out in the phase report and at session #2.

Snapshot semantics are unchanged from today (built once at startup; no
rebuild on `apply_patch` — the standalone TK-track graph is static
post-load; documented limitation).

### 2.5 Tests (named)

- `assemble_merges_engine_and_chain_pages_in_canonical_order`
- `assemble_custom_pages_alphabetical_after_canonical`
- `assemble_envelope_group_indices_offset_across_nodes`
- `assemble_missing_engine_rule_returns_none`
- `assemble_param_carries_owning_node_id`
- `assemble_display_name_prefers_instrument_label` (M7)
- `chain_lists_rule_bearing_nodes_only` (m4a)
- `edge_derived_chain_follows_signal_order` (app-side fixture graph)
- `clap_plugin_engine_gets_composite_view` (m10)
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
positional zip is deleted). Like the Antiphon registry (C2), the
`composite` views and cap-doc cache are a **startup snapshot** — valid
after `build_executor()` (`all_edges()`/`get_node_cap_doc` remain
readable), never rebuilt on `apply_patch` (same documented limitation).

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
  `Sequencer`. The bank handles `CMD_SET_PARAM`/`CMD_BUMP_PARAM` for free.
  **Two things are NOT free (review M1) and are part of this commit:**
  1. **Bus mirroring:** `Sequencer::published_state` never calls
     `publish_bank_state` today (contrast analog_engine.rs:333) — add it,
     so `/node/{id}/param/mute` exists for the UI toggle read. The
     OnceLock path cache rebuilds per `activate()`, so the new path is
     cached from first publish (no per-cycle `format!`).
  2. **Persistence:** the v3 blob has no bank-param generality. Append
     `mute` as a trailing track-level field (the format tolerates
     trailing bytes, sequencer.rs:1116-1118) and re-apply it in
     `deserialize_v3` via `bank.set` after `activate()` (the established
     activate-then-deserialize ordering; unlike swing there is no
     per-pattern mirror, so no conduit machinery).
- All four fire paths skip **note-on and `ParamLockEvent` emission** while
  `mute >= 0.5`. Note-offs for already-fired notes are unaffected (they
  emit from the gate countdown and `global_stop`, independent of the
  note-on paths — verified in review), so voices decay naturally and
  shared reverb tails ring: the documented trig-gate semantic.
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
- `mute_publishes_bank_state_path` (M1)
- `mute_roundtrips_v3_serializer` (M1)
- `keymap_shift_track_toggles_mute` (input, both modes)
- `mute_toggle_reads_bus_and_flips_value`
- `render_muted_track_dims_label_with_m_marker` (TestBackend)

### 4.4 Test-driver scenario (review m11)

`tools/test-driver/tests/mute_gates_track.yaml`: 4-on-the-floor kick;
`set_param` `mute` 1.0 on the kick sequencer (the driver's existing
set_param action needs no extension); assert `peak_lt` over the muted
window; `set_param` `mute` 0.0; assert `peak_gte` after. Backs exit
criterion 2's "audibly gates".

---

## Commit 5 — p-lock UI

### 5.1 Step focus (OQ-T1 focus model; hold-step stays a session probe)

- Model tracks `last_step: Option<usize>` per track (updated by every
  `ToggleStep`).
- `Enter` toggles **step focus** on the active track's `last_step`;
  focused step renders `Reversed`. `Enter` again or `Esc` releases
  (`Esc` gains its first real semantics: cancel/release — global).
  Focus is per-track; switching tracks keeps each track's focus.
- **Mode scoping (review M4):** `Enter`/focus is meaningful in **both**
  modes (SEQ: mark a step; PERF: edit it — the natural SEQ→PERF flow),
  but jog→lock routing exists only in **PERF**, the only mode with jog
  keys. SEQ never routes jogs anywhere (it has none).

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
- `Shift+Backspace` with focus → the **pair** (review M3):
  `CMD_SET_LOCK_TARGET` with slot A's `(node_id, param_id)` immediately
  followed by `CMD_CLEAR_STEP_LOCK` (arg1 = slot A's param_id). The pair
  is required because slots routinely live on different nodes after C3 —
  lane identity is `(lock_target.node_id, param_id)`, and param ids are
  global name-hashes that collide across nodes.

### 5.5 Tests (named)

- `enter_focuses_last_toggled_step`; `esc_releases_focus`
- `enter_focuses_in_seq_jog_edits_in_perf` (M4 mode scoping)
- `jog_while_focused_emits_target_then_lock_pair`
- `jog_lock_value_starts_from_existing_lock`
- `jog_lock_value_starts_from_live_when_no_lock`
- `jog_without_focus_still_bumps_param` (no regression)
- `backspace_clears_all_lanes`; `shift_backspace_emits_target_then_clear_pair` (M3)
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
  note, **velocity, length**, condition, timing, **and locks** — into a
  Model-side copy buffer).
- `Y` (Shift+y — `P` is taken by Shift+track-8 mute) pastes over the active
  track's current pattern. Per pasted step, emit in order
  (review M2 — CMD 17 alone is lossy: it sets note+active only, preserves
  condition/timing/locks, and never touches velocity):
  1. `CMD_CLEAR_STEP_LOCK` arg1 = −1.0 (clear stale destination lanes);
  2. `CMD_SET_STEP` (17) — note + active;
  3. `CMD_SET_STEP_VELOCITY` (36) + `CMD_SET_STEP_LENGTH` (37);
  4. `CMD_SET_STEP_TIMING` (25) + `CMD_SET_STEP_CONDITION` (26);
  5. the lock pairs (33+34) for each yanked lock.
  One drained batch, ≤ ~90 commands for 16 steps. Clamps to
  `min(src_len, dst_len)`; does not touch pattern length/speed.
- **Cross-track lock remap:** locks carry the *source* engine's `node_id`,
  and the destination engine drops foreign locks at its
  `pl.node_id == self.node_id` gate. On cross-track paste, remap
  generator-targeted locks source-generator → destination-generator (the
  edge-derived pair from C3 gives both ids); locks targeting chain nodes
  are **dropped** with an echo note (chains differ across tracks; remapping
  them is guesswork). Same-track paste keeps all locks verbatim.
- **Audio-thread batch note (review m5):** the executor's per-node
  `pending_cmds` Vec starts at capacity 16 — a ~90-command paste would
  grow it on the audio thread (one-time, first paste). Bump the initial
  capacity to 128 in `executor.rs` (startup allocation, one line).
- Paste is destructive-by-design (overwrites); **no confirm, no undo** —
  HYPOTHESIS, session #2 judges.

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
  app's existing instrument-path mechanism) realizing the AGENTS.md
  aspirational graph: sequencers 10–17 → voices 20,21,22,23,24,25,26,27
  (23–26 = Samplers; `gen-samples` output required, as for the default
  instrument), **and chain nodes** (review M6): each voice → its
  `DistortionNode` (30–37) → its `FilterNode` (40–47) → mix input.
  Without chain nodes, C3's composite-page smoke ("PERF FLTR page binds
  the chain node's params") has nothing to bind.
- Discovery must yield 8 tracks with keys `q`–`p` mapped 0–7; a synthetic
  9-track config clamps to 8.

### 7.6 Tests (named)

- `seq_number_row_sends_set_pattern`
- `render_shows_active_and_cued_pattern_markers` (TestBackend)
- `yank_then_paste_emits_full_step_command_batch`
- `paste_clamps_to_shorter_pattern`
- `paste_clears_stale_lock_lanes_before_writing`
- `paste_includes_velocity_length_condition_timing_commands`
- `cross_track_paste_remaps_generator_locks_drops_chain_locks`
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
- **`x`-as-clear from design.md §3.2 is superseded** — `x` is step 10 on
  the invariant grid (1-based; `,` is step 14); clear moves to the `:` line
  (`clear`) and locks to `Backspace`.
