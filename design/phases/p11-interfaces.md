# P11 — Live Performance: Implementation Blueprint

> **Stage 2 of a staged design.** This is the implementation plan. The
> problem statement is `p11-problem.md`; the architecture is ADR-039
> (accepted 2026-07-23, with hostile-review amendments).
>
> **Authored:** 2026-08-02. **Status:** spec draft — reviewed 2026-08-02
> (5 blockers, 5 majors resolved). Awaiting ratification.
> **Milestone:** `P11 — Live Performance` (#17).

---

## §0 — Authority and amendments

This spec is bound by ADR-039 and its post-ratification amendments (§0
An1–An4 in that ADR). Where this spec disagrees with the ADR body, the
amendment wins; where the ADR is silent, this spec's choice is noted.

**Amendments that change the ADR body** *(the ADR is not edited; these
are recorded here as the normative reading order)*:

- **Amd 1** (review B1): kit membership is opt-in via a
  `ParamDescriptor` flag. Sound params only; structural params
  (`mute`, `swing`, `pattern_length`, `ticks_per_step`, `bpm`,
  `live_rec`) are excluded.
- **Amd 2** (review B2): Keystep events arrive at sample offset 0
  today. Intra-block-offset timestamping in the HAL MIDI path is P11
  scope — without it, requirement 4's "sample-accurate" is aspirational.
- **Amd 3** (review M3): temp-save shadow uses `copy_into`, not
  derived `Clone` (which allocates nested `Vec<Step>`). Shadow must
  also reserve `CV_LOCK_CAP` per step — `Step::empty()` leaves
  `cv_locks` at capacity 0, and deserialized patterns can carry CV
  locks that would allocate on copy.
- **Amd 4** (review M4): `send_command` results SHALL NOT be discarded;
  the kit-apply path retries on `Err`. Prepared mutes get dedicated
  commands, not a defer flag in `CMD_SET_PARAM`'s arg0.

**ADR-039 note (2026-07-29):** decision 7's `live_rec` param and
`CMD_TRIG_NOW`→live-record path shipped early as TK2.1 C3b. The Midi2
note-on consumption on `events_in` did NOT ship — that is the one
live-record piece this phase must add. (Survey confirmed: `process()`
only matches `Event::Transport` at `sequencer.rs:1489` — no Midi2 arm.)

**Implementation note (2026-08-02):** C0–C3's §4 test plan is now
realized as debug-harness live tests, per the standing live-test gate:
`p11_kit_capture_apply.yaml` (C0 in_kit opt-in + C2 capture/apply),
`p11_temp_save_reload.yaml` (C3 app-level both halves), and
`p11_bind_kit_apply.yaml` (C2 pattern-switch apply + perform-mode skip)
under `tools/test-driver/tests/`, plus the C1/C2 unit tests (KitStore
RON round-trip, ring-full chunking, pattern-switch diff, app-op drain).
All are mutation-checked. Harness verbs for P11 (kit/temp ops through
`PerformState`) landed in test-driver first, as the gate requires.

**Live session (2026-08-02, autonomous):** the pattern-switch kit-apply was
additionally verified against the real running app — Theotokos in kitty,
project v3 `--load` (kit 0 "LiveKick" = decay 0.9 on node 20, binding slot 0
→ kit 0), real PTN chords switched the kick sequencer P0→P1→P0; the app's
log recorded `[kit] pattern switch seq=10 pattern=0 → kit 0` + `[kit] loading
kit 0: LiveKick`, and the ENC bank read `decay ... 0.90` afterwards. The
P0→P1 leg (unbound pattern) produced no kit lines — the negative control.
This is the first application of the trifecta leg 3 (autonomous real-app live
session). Temp-save/reload chords, the KIT screen and the perform-mode toggle
have no surface yet (C6) — #184 tracks surface-side AppOp production.

**Implementation note (2026-08-03) — C4–C7 shipped, milestone #17 closed:**
C4 (mute tiers), C5 (live record + live_quantize), C6 (Theotokos surfaces)
and C7 (Antiphon/Theoria verbs) all landed; see `design/phases/p11-report.md`.
OQ-12 (#76) resolved by the `live_quantize` hard-quantize param (C5);
OQ-T25 (#137) resolved by the hold-NO Elektron-style live erase (C6);
OQ-T11 (#136) moot (C3 shipped the full engine+app scope); OQ-T29 (#122)
closed as the #76 duplicate. The live session from C2 was extended
(2026-08-03, autonomous): KIT screen save/load (kitty `7`/`h`/`g`, `[kit]
saved kit Kit 1 in slot 0` / `[kit] loading kit 0: Kit 1`), FUNC+KIT →
`⚡` perform indicator on the status line, FUNC+Enter/Esc → `[temp_save]` /
`[temp_reload]` log lines, TRK+FUNC+trig → `◌` pattern-mute marker (both
ways), and hold-Esc (NO) live erase wiping the authored 4-on-the-floor.
The C7 verbs were additionally probed over the raw WebSocket against the
running graph (kit_save/temp_save executed in PerformState; `kit_list`
replied `0:ProbeKit`).

**Design decisions this spec makes where ADR-039 is silent:**

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Kit count | 64 named kits | Elektron Analog Rytm has 128; 64 is generous for a first pass and fits in one u8 index |
| Kit naming | `String`, max 16 chars, user-editable; `KitSaveAs` auto-names "Kit N" | Short names fit Theotokos's 18-char cell; project RON is human-readable; auto-name ships faster than a text-entry mode |
| Pattern mute publication | State path `/node/{id}/state/pattern_muted` via `published_state()` | Not a bank param — it lives on the Pattern, not the bank; state path is the existing mechanism |
| Temp-save triggered by | `FUNC + YES` tentative, `FUNC + NO` for reload | ADR-039:133 proposed; session-testable; spec-committed to let the C6 Theotokos commit pick the exact chord |
| Prepared-mute mechanism | **Engine-side defer, not app-polled.** The sequencer applies pending mutes at its own pattern wrap inside `handle_transport`, per ADR decision 6. No `CMD_APPLY_PREPARED`, no bus polling. | ADR-039:77-81: "the sequencer holds the change and applies it exactly at the next pattern wrap. Per-node and sample-deterministic." The alternatives section (ADR-039:115-117) explicitly rejected surface-side boundary polling as race-prone. This spec implements what the ADR decided. |
| Prepared-mute on stop | Pending mutes are **cleared** when transport stops. | A stale mute applied at the first wrap after restart is an unintended side effect the performer never asked for. Clear-on-stop is the boring, defensible option. |
| Kit apply chunking | 16 params per tick, retry on ring-full | The command ring is 512 slots; a full-instrument kit is ~80 params across 8 tracks × ~10 params each; 16/tick = 5 ticks worst case. The ring drains per 512-frame audio block (~11.6 ms at 44.1 kHz); 5 main-loop ticks fits comfortably. |
| Pattern-switch kit trigger | App polls `/node/{id}/state/active_pattern` per sequencer each tick, diffs against a cached array, and applies the bound kit when it changes. "Active slot" for KitCommit/KitReload = the Theotokos-selected track's sequencer pattern index. | The app already polls the state bus each tick; `active_pattern` is published at `sequencer.rs:1526`. The Theotokos-selected track is the natural focus — the performer is looking at it. |
| Perform mode default | `false` (kit-apply mode) | "Default with a binding present becomes kit-apply" (ADR-039:39); unbound slots behave as today |
| Theotokos KIT screen | List: kit name + 8-slot encoder bank for the selected kit's first 8 params | The KIT button is reserved (ADR-038); a screen that lists kits and lets the user load/save/commit/reload is the minimal usable surface. The encoder bank shows the selected kit's params read-only — editing a kit is a later refinement |
| `live_rec` excluded from kits | Via the `in_kit` flag, default `false` on `live_rec` | ADR-039 amendment 1; the param already carries a comment at `sequencer.rs:1392-1395` |
| Keystep→sequencer routing | App-side: `main.rs` connects `KeystepNode` output to the Theotokos-selected sequencer's `events_in`. Track-select changes rebind the edge. | The Keystep node exists; `events_in` exists; no routing connects them today. Amd 2 delegated this decision to the phase spec. No YAML or profile changes needed — P11 ships with one track live-recordable at a time, which is the Elektron model. Per-track routing is a later refinement. |
| Project RON v4 | New `kits` and `kit_binding` sections with `#[serde(default)]`; `load_project` gains a v3/v4 branch. The serde `version` field is the project-format version (3 for kits, 4 when the sequencer blob also bumps). | v1/v2 loaders are already locked out by the version gate (`project.rs:156-158`); adding `default` lets old binaries ignore the new sections while new binaries tolerate old files. The sequencer's per-node blob version stays 3 for now (see §5). |

---

## §1 — Commit plan

Each commit is independently compilable and testable. Commits build
green at every step (per the house rule). CLAP-host param exposure for
kit membership is **deferred** to the first machine-crate that ships
(§C8) — MixNode and the built-in engines cover the initial surface.

### C0 — ParamDescriptor gains `in_kit` flag

**Scope:** `paraclete-node-api` (L2), all param-declaration sites.

Every `ParamDescriptor` literal in every node gains `in_kit: true` or
`in_kit: false`. The field defaults to `true` for sound params and
`false` for structural params.

**What changes:**

1. `capability.rs` — add `pub in_kit: bool` to `ParamDescriptor` (after
   `stepped`, before `unit`). All existing constructors must add the
   field — this is a compile-driven sweep.

2. **Per-node assignments** (derived from actual capability docs, not
   inferred):

   | Node | `in_kit: false` params | `in_kit: true` params |
   |------|----------------------|----------------------|
   | `Sequencer` | `pattern_length`, `ticks_per_step`, `swing`, `mute`, `live_rec` | *(none — sequencer has no sound params)* |
   | `InternalClock` | `bpm` | *(none)* |
   | `MixNode` | *(none)* | `input_gain_{0..N}`, `master_gain` |
   | `AnalogEngine` | `machine` (identity), all `lfo_*` (`lfo_dest`, `lfo_depth`, `lfo_shape`, `lfo_speed`, `lfo_mode`, `lfo_start_phase`, `lfo_fade`) | `tune`, `tone`, `decay`, `punch`, `drive`, `snap`, `noise`, `open` |
   | `FmEngine` | `machine`, all `lfo_*` | `tune`, `tone`, `decay`, `mod_index`, `feedback`, `ratio`, `drive` |
   | `Sampler` | all `lfo_*` | `pitch`, `volume`, `pan`, `start`, `end`, `attack`, `release`, `root_note`, `loop`, `slice` |
   | `FilterNode` | all `lfo_*` | `cutoff`, `resonance`, `drive`, `wet`, `dry` |
   | `DistortionNode` | *(none — no LFO)* | `drive`, `wet`, `dry` |
   | `ReverbNode` | *(none — no LFO)* | `wet`, `dry`, `decay`, `size`, `damping` |

   **Notes on the sweep:**
   - `fill_a`/`fill_b`/`speed` are NOT `ParamDescriptor`s (they are
     `cycle_state` flags, `speed_mult` field, and `bpm` on the clock) —
     the compile-driven sweep cannot touch them because they were never
     capturable.
   - The `lfo_*` params come from the shared `lfo_params()` builder
     (`engine_dsp.rs:378-441`), called by FilterNode, Sampler,
     AnalogEngine, and FmEngine. The sweep touches each of those four
     nodes, not a non-existent "LfoHost".
   - `Sampler` has no `sample` param — it has `root_note` etc. as
     above (`sampler.rs:173-195`).

   **Rationale for `lfo_*` exclusion:** capturing modulation routing in
   a kit is a later refinement (per-track kit extraction, ADR-039:141).
   For whole-instrument kits, replaying an LFO dest that points at a
   param the kit also overwrites is at best redundant and at worst
   races.

3. **Guard test:** exhaustive per-node test that every param has an
   explicit `in_kit: true/false` — no `..Default::default()` gaps —
   so adding a param later forces the author to decide.

**Files:** `capability.rs`, `sequencer.rs`, `mix.rs`,
`analog_engine.rs`, `fm_engine.rs`, `sampler.rs`, `filter.rs`,
`distortion.rs`, `reverb.rs`, `internal_clock.rs`, `engine_dsp.rs`.

---

### C1 — AppOp vocabulary and drain

**Scope:** `paraclete-app` (new module), surface-side plumbing.

A new `app_ops` module in `paraclete-app` with:

```rust
#[derive(Debug, Clone)]
pub enum AppOp {
    TempSave,
    TempReload,
    KitLoad(KitId),
    KitSaveAs(String),
    KitCommit,
    KitReload,
    BindKit { slot: usize, kit: Option<KitId> },
    SetPerformMode(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KitId(pub u8);  // 0..63; values ≥ 64 rejected by KitStore
```

A trait `AppOps` with `take_pending_app_ops(&mut self) -> Vec<AppOp>`,
implemented by the Theotokos handle and the Antiphon session handle
(two implementors; scripting does not produce app-ops in P11). The app
main loop drains app-ops after the existing Antiphon command drain,
before Theotokos commands:

```
// After Antiphon drain, before Theotokos tick:
for op in [&mut antiphon_handle, &mut tk_handle]
    .flat_map(|h| h.take_pending_app_ops())
{
    perform_state.execute(op, &mut conf, &bus);
}
```

**No CMD wire yet** — the `execute` methods in C1 are stubs that log
the op and return. This commit is the structural plumbing only;
C2–C5 fill in each op.

**Files:** new `crates/paraclete-app/src/app_ops.rs`,
`main.rs` (drain site), `paraclete-theotokos/src/lib.rs` (trait impl),
`paraclete-antiphon/src/lib.rs` (trait impl).

---

### C2 — Kit store and capture/apply

**Scope:** app-level `KitStore`, `PerformState`, kit execution.

#### C2a — KitStore

```rust
pub struct Kit {
    pub name: String,           // max 16 chars, enforced on save
    pub entries: Vec<KitEntry>, // sorted by (node_id, param_id)
}

pub struct KitEntry {
    pub node_id: u32,
    pub param_id: u32,
    pub value: f64,
}

pub struct KitStore {
    kits: Vec<Option<Kit>>,  // 64 slots; None = empty
}
```

Kit capture enumerates node ids and capability docs from
`conf.get_node_cap_doc()` (the existing method at `main.rs:814`);
iterates each capability doc's params; for every `in_kit: true` param,
reads the value from the state bus at `/node/{id}/param/{name}` (the
`publish_bank_state` path). The `StateBusHandle` store is private
(`state_bus.rs:31-51`) with no iteration API — capture goes through
the capability doc as the authoritative param list, reading one value
per param from the bus.

Kit apply replays `CMD_SET_PARAM` for each entry, chunked at 16 per
tick with retry-on-`Err` (Amd 4 — `.ok()` is forbidden). The main loop
tracks an `apply_pending: Vec<KitEntry>` so a ring-full tick resumes
where it left off.

Kit save/load to project RON: `kits` top-level section, an array of
`Option<Kit>` serialized as `(name, [(node_id, param_id, value), ...])`.
RON format: `Some(("Kit Name", [(20, 3541427549, 0.5), ...]))`, `None`.
The `kits` and `kit_binding` fields on `Project` carry `#[serde(default)]`
so old files (no fields) deserialize as empty; new files carry
`version: 3`.

#### C2b — PerformState

```rust
pub struct PerformState {
    pub kit_store: KitStore,
    pub kit_binding: [Option<KitId>; 8],  // slot-indexed
    pub perform_mode: bool,
    pub temp_param_snapshot: Option<Vec<KitEntry>>,  // volatile, RAM-only
    /// Cached active_pattern per sequencer (node_id → last seen pattern index).
    /// The app diffs this each tick to detect pattern switches for kit-apply.
    cached_active_patterns: HashMap<u32, usize>,
}
```

Published to state bus: `/context/perform` = `Float(perform_mode as u8 as f64)`.

#### C2c — Kit execution

- `KitLoad(id)`: validate `id < 64`; read kit from store, apply entries
  via chunked `conf.send_command(cmd)`. If ring-full, retry next tick.
- `KitSaveAs(name)`: truncate name to 16 chars; capture current param
  state via cap-doc/bus read; store as new kit.
- `KitCommit`: capture into the kit bound to the Theotokos-selected
  track's pattern slot.
- `KitReload`: re-apply the kit bound to the Theotokos-selected track's
  pattern slot.
- `BindKit(slot, kit)`: set `kit_binding[slot]`, persist in project.
- `SetPerformMode(bool)`: set flag, publish to `/context/perform`.

**Pattern-switch kit-apply trigger:** each tick the app reads
`/node/{id}/state/active_pattern` for every sequencer (published at
`sequencer.rs:1526`), diffs against `cached_active_patterns`, and when
a sequencer's active pattern changes AND `!perform_mode` AND
`kit_binding[new_index].is_some()`, applies the bound kit. The
`cached_active_patterns` map is updated to the new value.

"Theotokos-selected track" = the track whose sequencer node_id is
stored in the Theotokos model's `selected_track` (ADR-036). It is
the one the performer is looking at; KitCommit/KitReload target its
active pattern's bound kit.

**Test:** `KitStore` unit tests (capture via cap-doc, round-trip RON,
apply chunking under ring-full, name truncation, KitId bound check),
`PerformState` tests (pattern-switch applies kit when perform is off,
skips when on, active-pattern diff logic).

**Files:** new `crates/paraclete-app/src/kit.rs`,
`crates/paraclete-app/src/perform_state.rs`,
`crates/paraclete-app/src/app_ops.rs` (fill in execute methods),
`project.rs` (kits + kit_binding sections, version 3, `#[serde(default)]`).

---

### C3 — Temp save/reload

**Scope:** sequencer shadow pattern (engine), temp param snapshot (app),
orchestration (app-op).

#### C3a — Engine: sequencer shadow

`Sequencer` gains:
```rust
shadow_pattern: Pattern,       // pre-allocated at build, same capacity as patterns[0]
shadow_has_data: bool,
```

`Pattern` gains `copy_into(&self, dest: &mut Pattern)` (Amd 3):
- `dest.steps` pre-allocated to `STEP_CAPACITY`; each step's
  `param_locks` pre-reserved to `LOCK_CAP_PER_STEP` (8) and `cv_locks`
  pre-reserved to a new `pub(crate) const CV_LOCK_CAP: usize = 4`.
- `copy_into` clears and copies step-by-step — no `Vec` allocation,
  just element copies. CV locks exceeding `CV_LOCK_CAP` are clamped
  (truncated, same policy as `CMD_SET_STEP_LOCK` at `sequencer.rs:786`).
- `debug_assert!` on every step's lock capacities at build and before
  each copy.

New commands (ADR-039 reserves 39–45):
- `CMD_TEMP_SAVE = 39`: clone active pattern into shadow; set
  `shadow_has_data = true`.
- `CMD_TEMP_RELOAD = 40`: restore shadow into active pattern (if
  `shadow_has_data`), then clear `shadow_has_data`.

The shadow is RAM-only; never serialized.

#### C3b — App: temp param snapshot

`PerformState::temp_param_snapshot: Option<Vec<KitEntry>>` — captured
via the same cap-doc/bus-read as kit capture. One `AppOp::TempSave`
broadcasts `CMD_TEMP_SAVE` to every sequencer AND captures params in
the same main-loop tick. `TempReload` replays the snapshot as params
(chunked at 16/tick, same retry pattern as kit apply) AND broadcasts
`CMD_TEMP_RELOAD` to every sequencer.

The temp-save batch (~8 sequencer commands + param capture) fits
comfortably in one tick. Temp-reload fires ~8 sequencer commands +
begins the 16/tick param replay — the same chunk budget as kit apply.

#### C3c — Sequencing constraint (Amd 4)

The command ring is 512 slots. A temp-save batch is ~8 sequencer
commands; a kit-apply is ~80 param commands chunked at 16/tick.
If any `send_command` returns `Err`, the remainder retries next tick.
The batch-fence commit marker (ADR-039:185) is NOT implemented in this
phase — the retry-on-Err loop is sufficient.

**Test:**
- `copy_into` capacity assertions and step-by-step equality (including
  lock and CV-lock copies, CV-over-cap clamping).
- `CMD_TEMP_SAVE` / `CMD_TEMP_RELOAD` round-trip: mutate pattern, save,
  mutate again, reload, assert restored.
- App-op `TempSave` / `TempReload` integration: save while playing,
  reload, assert both pattern and params restored.

**Files:** `sequencer.rs` (shadow + commands + copy_into),
`app_ops.rs` (TempSave/TempReload execution),
`perform_state.rs` (temp snapshot capture/apply).

---

### C4 — Mute tiers

**Scope:** sequencer pattern-mute, prepared-mute commands (engine-side).

#### C4a — Pattern mute

`Pattern` gains `muted: bool` — appended as a **trailing byte** at the
end of each pattern record (inside the u32 record-length envelope, after
the last step record). This is the position the existing v3
skip-tolerance already covers: `deserialize_v3` reads up to the
declared record length and skips trailing bytes (`sequencer.rs:1840-1843`);
the test `deserialize_v3_skips_unknown_trailing_pattern_fields`
(`sequencer.rs:3485`) proves this. The sequencer's per-node blob
**stays version 3** — a trailing byte inside a length-prefixed record
is backward-compatible by construction.

New command:
- `CMD_SET_PATTERN_MUTE = 41`: toggles or sets `pattern.muted` via
  `arg0` (0 = off, 1 = on, 2 = toggle).

`is_muted()` becomes:
```rust
fn is_muted(&self) -> bool {
    self.bank.get(id_for_name("mute")) >= 0.5
        || self.patterns[self.active_index()].muted
}
```

Pattern mute published via `published_state()`:
`/node/{id}/state/pattern_muted` = `Bool(pattern.muted)`.

**Global mute + pattern mute are independent tiers** (ADR-039 decision
6). The outer wrapper's trailing `u8 mute` (`sequencer.rs:1679`) is
the bank global mute and persists as today. The per-pattern `muted`
byte is a separate field — effective mute is `global OR pattern`. On
load, the v3 outer mute byte is restored to the bank as today; the
per-pattern `muted` byte is read from the trailing field inside the
pattern record (or defaults to `false` if absent — v3 files, or v4
files from an older save that didn't have it). Neither shadows the
other.

#### C4b — Prepared mutes (engine-side defer)

Per ADR-039 decision 6: "the sequencer holds the change and applies it
**exactly at the next pattern wrap. Per-node and sample-deterministic.**"

Two dedicated commands (Amd 4: "deferred mutes get dedicated commands
from the 39–45 family"):

- `CMD_PREPARE_MUTE = 42`: deferred global mute toggle. `arg0`:
  0 = off, 1 = on. The sequencer stores `pending_global_mute: Option<bool>`.
- `CMD_PREPARE_PATTERN_MUTE = 43`: deferred pattern mute toggle for the
  active pattern. `arg0`: as above. Stored as
  `pending_pattern_mute: Option<bool>`.

**No `CMD_APPLY_PREPARED`.** The sequencer applies pending mutes at
its own pattern wrap inside `handle_transport()` (the same place it
advances the step and handles chain/cues — `sequencer.rs:1093-1125`).
At the wrap (step index wraps to 0): if `pending_global_mute` is
`Some(v)`, set the bank mute param to `v as u8 as f64` and clear the
pending; if `pending_pattern_mute` is `Some(v)`, set
`pattern.muted = v` and clear. This is sample-deterministic within
the audio block and needs no app polling or bus path.

On `global_stop` (`sequencer.rs:1281`): clear both
`pending_global_mute` and `pending_pattern_mute` — stale mutes must
not survive a stop and land on the first wrap after restart.

**Test:**
- Pattern mute round-trips through v3 blob with trailing byte.
- `is_muted()` returns true when either global OR pattern mute is set.
- Prepared mute holds until pattern wrap; applied at wrap; cleared
  after application.
- Prepared mute cleared on `global_stop` — does not survive a stop.
- v3 loader tolerates pattern records with extra trailing bytes
  (existing test covers this; add a specific muted-byte case).
- Global mute and pattern mute are independent: setting one does not
  affect the other.

**Files:** `sequencer.rs` (Pattern.muted, commands 42-43, wrap
  application, stop-clear, is_muted, serialize/deserialize trailing
  field), `app_ops.rs` (surface→command mapping for pattern mute).

---

### C5 — Live record: Midi2 note-on consumption + Keystep routing

**Scope:** sequencer `events_in` path, Keystep wiring, HAL timestamps.

#### C5a — Keystep routing (Amd 2)

Today the Keystep node exists and connects to
`ScriptingGatewayNode` → `ID_GW_KS` (`main.rs:177`). The gateway
forwards only `Event::Surface` to scripts (`gateway.rs:77-78`);
`Event::Midi2` note-ons from the Keystep are dropped.

**P11 routing decision:** `main.rs` connects `KeystepNode` output
directly to the Theotokos-selected sequencer's `events_in` port.
When the performer changes track (TRK + trig), the app tears down
the old edge and creates a new one to the new sequencer — one track
live-recordable at a time, matching the Elektron model. This is an
app-side routing change; no YAML or profile changes. Per-track
simultaneous routing is a later refinement.

The existing `main.rs` wiring code already handles dynamic edges
(`connect`/`disconnect` on the configurator).

#### C5b — HAL arrival timestamping

`paraclete-hal`'s midir callback (`MidiInput::connect`, `hal/src/midi/
mod.rs:60-83`) fires on a separate OS thread with a `u64` microsecond
timestamp. The HAL cannot compute audio-frame offsets from this
callback (CPAL is in `hal/src/audio.rs`, not the MIDI path; no
`block_frame_count` is available).

**Best-effort approach:** the Keystep queue (`incoming:
Arc<Mutex<VecDeque<TimedEvent>>>`) already stores events with the
midir µs timestamp (`keystep/mod.rs:73`). `KeystepNode::process()`
(audio thread, `keystep/mod.rs:153`) drains the queue at block start.
Map the queued timestamp against the current block's start time to a
best-effort intra-block offset: `min((ts - block_start_ts) * sr /
1_000_000, block_size - 1)`. The result is ≤ 1 block of jitter — a
block is ~11.6 ms at 44.1 kHz, which is perceptible as micro-timing
drift but not as quantization noise. True sample-accurate timestamps
require a JACK backend.

The `TimedEvent` or the HAL's WrappedMidiEvent gains an `offset: u32`
field; the already-plumbed event carries this to the sequencer's
`record_live_trig` which writes it as the step's `micro_offset`.

#### C5c — Sequencer Midi2 consumption

New match arm in `Sequencer::process()` event loop (after the
`Event::Transport` arm at `sequencer.rs:1489`):

```rust
Event::Midi2(ref midi) => {
    if self.playing && self.is_live_recording() {
        if let Some((note, velocity)) = midi_note_on(midi) {
            self.record_live_trig(note, velocity);
        }
    }
}
```

`midi_note_on` is a new free function in `sequencer.rs`: extracts note
number and velocity (as `u8, f32`) from a `Midi2` message's status and
data words. The existing `build_note_on` (used for output-side UMP
construction elsewhere in the crate) demonstrates the format.

The `record_live_trig` path already exists (TK2.1 C3b,
`sequencer.rs:1191`) — it quantizes to the nearest step and writes
micro-timing from the event's sample offset. This commit gives it a
second input source.

**Test:**
- Keystep edge rebinds on track-select change.
- HAL: submit a TimedEvent with a known µs delta → `process()` computes
  a non-zero offset ≤ `block_size - 1`.
- Sequencer: send `Event::Midi2(note_on)` while `live_rec` is on and
  transport playing; assert a step is activated with the correct note,
  velocity, and micro-timing.
- `midi_note_on` rejects non-note-on messages (CC, aftertouch, etc.).
- Midi2 note-ons are ignored when `live_rec` is off or transport is
  stopped.

**Files:** `main.rs` (Keystep→sequencer edge routing),
`paraclete-hal/src/keystep/mod.rs` (arrival-offset computation),
`sequencer.rs` (Midi2 match arm + `midi_note_on` helper).

---

### C6 — Theotokos surfaces

**Scope:** KIT screen, perform-mode indicator, temp-save/reload chords,
REC+PLAY live-record arming, pattern-mute track-select chord.

#### C6a — KIT screen

New `Screen::Kit` variant. The KIT button (7) opens it; ESC returns to
Grid.

Screen layout:
```
┌── KIT ────────────────────────┐
│ 1 Kick Basic       ◄ loaded  │
│ 2 Snare Tight                 │
│ 3 Hat Crisp                   │
│ 4 (empty)                     │
│ …                             │
│ [LOAD] [SAVE] [COMMIT] [RELD] │
└───────────────────────────────┘
```

- List: 16 visible kit slots, scrollable. The loaded/active kit is
  marked. Encoder 8 scrolls.
- Bottom row buttons (trigs 13–16): LOAD = KitLoad, SAVE = KitSaveAs
  (auto-names "Kit N"), COMMIT = KitCommit, RELD = KitReload.
- Encoder bank (row 2): shows the selected kit's first 8 params
  read-only (param name + value). Not editable — kit editing is a later
  refinement.

#### C6b — Perform mode indicator

`/context/perform` published by C2b. Theotokos reads it and shows a
`⚡` (or `P`) indicator in the status bar when perform mode is on.
FUNC + KIT toggles `SetPerformMode` — implement as an app-op from the
Theotokos `take_pending_app_ops()` drain.

#### C6c — Temp save/reload chords

`FUNC + YES` = `AppOp::TempSave`, `FUNC + NO` = `AppOp::TempReload`.
These are tentative per ADR-039:133; session-testable. If the session
prefers different chords, that's a one-line change in the input mapper.

#### C6d — REC+PLAY live record

Already partially built (TK2.1 C3b): `set_live_rec_for_all_tracks` arms
`live_rec` on every sequencer. REC+PLAY → the existing REC+PLAY path
enters live-record mode. The new Midi2 consumption (C5c) means Keystep
notes now land in the pattern when REC+PLAY is engaged. `CMD_TRIG_NOW`
(the live trig path) already records itself when `live_rec` is on.

#### C6e — Pattern-mute surface

No dedicated mute screen in P11 (ADR-038's mute screen is unscoped).
The Theotokos panel grammar has TRK (Tab hold) as the track-select
prefix. Pattern mute: `TRK + FUNC(Shift) + trig` on the active track
row toggles `pattern_mute` for that track, sent as
`CMD_SET_PATTERN_MUTE` with `arg0 = 2` (toggle). Prepared mute:
`TRK + FUNC + SHIFT(Ctrl) + trig` queues a deferred mute via
`CMD_PREPARE_PATTERN_MUTE`.

The track indicator (ADR-044 D1, the always-visible trig strip) already
shows per-track state; `pattern_muted` state from the state bus
`/node/{id}/state/pattern_muted` dims or marks the indicator.

**Files:** `paraclete-theotokos/src/model.rs` (Screen::Kit, kit list
  state), `render.rs` (KIT screen rendering), `input.rs` (KIT button →
  screen, FUNC+KIT → perform toggle, FUNC+YES/NO → temp save/reload,
  TRK+FUNC+trig → pattern mute / prepared mute), `lib.rs` (handle_keys
  KIT screen dispatch), `action.rs` (new actions).

---

### C7 — Antiphon/Theoria protocol verbs

**Scope:** W-track AppOp verbs over the WebSocket protocol.

New Antiphon protocol verbs (W-track scope — the existing `session`
envelope):

| Verb | AppOp | Payload |
|------|-------|---------|
| `kit_load` | `KitLoad` | `{ kit_id: u8 }` |
| `kit_save` | `KitSaveAs` | `{ name: String }` |
| `kit_commit` | `KitCommit` | `{}` |
| `kit_reload` | `KitReload` | `{}` |
| `temp_save` | `TempSave` | `{}` |
| `temp_reload` | `TempReload` | `{}` |
| `set_perform_mode` | `SetPerformMode` | `{ on: bool }` |
| `bind_kit` | `BindKit` | `{ slot: usize, kit_id: u8\|null }` |
| `list_kits` | *(query, not an op)* | `{}` → response with kit list |

Theoria: add KIT tab/page showing the kit list, with load/save/commit/
reload buttons. Pattern-bind dropdown per track slot. Perform-mode
toggle. REUSE the existing `ViewPlugin`/cap-doc mechanism — no new
Theoria architecture.

**Files:** `antiphon/src/protocol.rs` (verb dispatch),
`antiphon/src/session.rs` (app-op drain impl),
`web/packages/app/src/` (KIT tab, kit list component).

---

### C8 — Deferred: CLAP host param exposure

**Not in P11 scope; filed as a follow-up.** When the first
machine-crate ships, the CLAP host must expose hosted-plugin params
to the state bus so kits can capture them. This is its own commit in
a later phase. Until then, only built-in nodes participate in kits,
which covers the shipped instrument graphs.

---

## §2 — Commit ordering dependencies

```
C0 (in_kit flag) ─────────────────────────────────────────┐
C1 (AppOp drain) ─────────────────────────────────────┐   │
C2 (Kit store + capture/apply) ─── depends on C0, C1   │   │
C3 (Temp save/reload) ─────────── depends on C2        │   │
C4 (Mute tiers) ───────────────── depends on C1        │   │
C5 (Live record + Keystep) ────── independent ─────────│───│┐
C6 (Theotokos) ────────────────── depends on C1..C5   ◄┘   ││
C7 (Antiphon/Theoria) ─────────── depends on C1..C5        ││
                                                           ││
C5 touches only HAL + sequencer (no AppOp machinery).      ││
C6 and C7 are parallelizable (different crates, no shared state).
```

---

## §3 — Open questions carried forward

These are filed as GitHub issues and tagged to the P11 milestone;
answers are not required before implementation starts but must be
resolved before the phase closes.

| Issue | Question | Blocks |
|-------|----------|--------|
| #137 | OQ-T25 — Live-record erase gesture (hold NO?) | C6 (Theotokos REC grammar) |
| #136 | OQ-T11 — Temp save/reload engine scope | C3 (scope confirmation) |
| #76 / #122 | OQ-12 — Live-record quantisation model | C5 (quantization resolution) |
| #134 | OQ-T8 — P-lock packed lock-set command | P11 adjacency (kit includes locks?) |
| #172 | OQ-M6 — Cross-track choke | Not P11; stays open |

---

## §4 — Test coverage plan

| Commit | Key tests |
|--------|-----------|
| C0 | Exhaustive `in_kit` audit: every param has the flag; structural params are `false` |
| C1 | App-op drain integration: ops flow from surfaces to app loop |
| C2 | Kit capture/apply round-trip via cap-doc; RON round-trip; chunked apply under ring-full; pattern-switch kit-apply (active-pattern diff); perform-mode skip |
| C3 | `copy_into` capacity/equality/CV-clamp; temp-save/reload pattern round-trip; temp-save/reload param+pattern together |
| C4 | `is_muted` global+pattern independent; prepared-mute defer+wrap-apply+clear; prepared-mute cleared on stop; v3 blob tolerance for trailing per-pattern `muted` byte; global mute persists independently |
| C5 | Keystep edge rebind on track-select; HAL arrival-offset computation; Midi2 note-on→step; `midi_note_on` rejects non-note messages; Midi2 ignored when live_rec off or stopped |
| C6 | KIT screen rendering (kit list, encoder bank, button dispatch); perform indicator; temp-save chord; pattern-mute TRK+FUNC+trig chord |
| C7 | Protocol verb→AppOp mapping; Theoria kit list rendering |

**Regression baselines:** all seven ADR-035 baselines must stay green.
Run `cargo run -p test-driver -- <scenario>.yaml --check-baseline`
on all seven before and after the phase.

---

## §5 — Serializer formats

### Project RON (project-level)

The project RON gains `version: 3` and two new top-level sections after
`profiles`:

```ron
Project(
    version: 3,
    metadata: ...,
    graph: [...],
    profiles: [...],
    kits: [
        Some(("Kick Basic", [(20, 3541427549, 0.5), (20, 1234567890, 0.3)])),
        None,
        Some(("Snare Tight", [(21, 3541427549, 0.7), ...])),
        // ... 64 slots
    ],
    kit_binding: [
        Some(0),   // slot 0 → kit 0
        None,      // slot 1 → unbound
        Some(2),   // slot 2 → kit 2
        None, None, None, None, None,
    ],
)
```

- `kits` and `kit_binding` carry `#[serde(default)]` — old files
  without them deserialize as empty; new files carry the fields.
- `load_project` gains a v3 branch (v1/v2 already rejected at
  `project.rs:156-158`).
- Old binaries loading v3 projects fail the version gate upfront (the
  serde tolerance is for same-binary forward compat, not cross-version).

### Sequencer per-node blob

The sequencer's per-node `serialize()` **stays version 3** — no blob
version bump. The `Pattern::muted` byte is appended as a **trailing
byte inside each pattern's u32 record-length envelope** (after the last
step record, before the next pattern record). This is the position the
existing v3 skip-tolerance covers: `deserialize_v3` reads up to the
declared record length (`sequencer.rs:1840-1843`) and the test
`deserialize_v3_skips_unknown_trailing_pattern_fields` (`sequencer.rs:
3485`) already proves tolerance for extra bytes at this position.

Write: after all step records, write `muted as u8`. Read: if bytes
remain in the pattern record after step parsing, read one `u8` as
`muted`; if no bytes remain, `muted = false`.

The outer wrapper's trailing `u8 mute` (`sequencer.rs:1679`) is
unchanged — it is the bank global mute, independent of per-pattern
`muted`.

### Version summary

| Layer | Version | What changed |
|-------|---------|-------------|
| Project RON | **3** | New `kits`, `kit_binding` sections |
| Sequencer blob | **3** (unchanged) | Pattern records gain trailing `muted` byte |
