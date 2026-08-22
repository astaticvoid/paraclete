# Paraclete — AN0 Anamnesis Specification

> **EXECUTION-READY for C0–C6 — 2026-08-21.** Written so a fresh-context
> session can implement without further design decisions: every commit
> names its files, the current behaviour it changes, and its tests. Where a
> value is a tuning knob the default is stated *(tunable)*.
>
> **C0 build note (2026-08-21, found in review):** §3's C0 file list says
> `sampler.rs` — remove `load_wav` (moved). In practice `load_wav` is
> **duplicated**, not moved, in C0: `Sampler::activate()` still calls its
> own copy until C2 rewires it to pull from `pool_cache` instead, and
> deleting the original in C0 without also changing `activate()` would
> break the build (C0 must land as a standalone, buildable commit).
> `sample_pool.rs:decode_file` is a faithful copy; `sampler.rs`'s
> `load_wav` is deleted in C2 once nothing calls it. Not a defect — the
> literal C0 file list wasn't achievable as its own commit; this is what
> a coherent intermediate state looks like.
>
> **Design authority:** ADR-040 (accepted 2026-07-23; the Anamnesis
> architecture — seven decisions, R1–R3 ratification, post-ratification
> review notes 1–5). Experience design: `design/sampling/design.md` §3
> (pool model), §7 (SRC page shape). Problem statement:
> `design/sampling/problem.md`.
> **Baseline:** TK3 closed (`tk3-report.md`, milestone #18, session #7
> verdict). `cargo test --workspace` green at baseline. No Sampler is wired
> into the default `instrument.yaml` today — the only exerciser is
> `tools/test-driver/tests/sampler_chain.yaml`.
> **Exit:** `cargo test --workspace` green after every commit;
> `cargo clippy --workspace` clean on touched crates; code review on each
> commit. **No user session gates AN0** — the roadmap's sampling-session
> gate sits at AN1 exit (the one-gesture transition trick, ADR-040 R2),
> not here. AN0 is app-plumbing and param-plane work; nothing in it is a
> performance gesture to evaluate by ear.
>
> **Read this first — what this phase is and is not.** AN0 is the pool,
> the asset-delivery plane, and the `Sampler` node's move from
> file-owned to pool-indexed. **No audio input, no recorders, no capture,
> no SAMPLING screen** — that is all AN1 (ADR-040 phase table). No slices
> UI, no chains, no scenes/crossfader — that is AN2. AN0's only
> user-visible surface is: a project can carry a named sample pool, a
> `Sampler` node's `sample` param selects a pool slot (steppable,
> p-lockable — a different sample per step), and the SRC page shows the
> canonical 8-param shape design.md §7 specifies. Everything else about
> "recording" is out of scope by construction — AN0 has nothing to
> record with yet.

---

## §0. Decisions made in this spec (no further design input needed)

These resolve gaps ADR-040 and `design.md` leave open at the
implementation level. Cited here so commits can reference a decision
without re-deriving it.

| # | Decision | Reasoning |
|---|---|---|
| D1 | `SamplePool` is a plain app-owned struct (`Vec<Option<PoolEntry>>`), structurally identical to `KitStore` (`crates/paraclete-app/src/kit.rs`) — own file `crates/paraclete-app/src/sample_pool.rs`, plain `serde` derive, no hand-rolled RON. | ADR-040 decision 3 says the pool follows the same app-ownership reasoning as kits (ADR-039: "nodes cannot command nodes"). `KitStore` is the only existing sibling; mirroring it exactly means no new persistence idiom to invent. |
| D2 | Default pool capacity: **128 slots** *(tunable)*. `Vec`-backed, not a fixed array — a slot beyond current length grows the vec with `None` padding rather than being rejected. | `KitStore` uses a fixed 64-slot `Vec` filled by `Default`; 128 mirrors that shape at Digitakt-class scale. Vec-backed (not array-backed) keeps the roadmap's "no hardcoded counts in frozen formats" tiebreaker honest — the *nominal* size is a UI/param-range convenience, not a wire-format ceiling. |
| D3 | `PoolEntry.source` is an enum with **both** variants declared now — `File(String)` and `Ram { frames: usize }` (opaque marker; AN1 fills in real capture metadata) — even though AN0 only ever constructs `File`. | Matches ADR-040 decision 7's "surface frozen now, DSP staged later" precedent (`play_mode`). Declaring the shape now means AN1 adds a *variant body*, not a breaking enum change to a format already saved in projects. |
| D4 | The `Sampler` node caches **whichever pool slots it has been sent**, keyed by slot index (`HashMap<u32, Arc<SampleData>>`), not the whole pool unconditionally and not a single active buffer. `SwapSample` is unicast per `(node_id, pool_slot)`; the app decides which slots a given node needs and sends exactly those. | ADR-040's message shape is `SwapSample { node_id, pool_slot, data }` — already node-targeted, not a pool-wide broadcast type. A `HashMap` cache lets a node hold as many or as few slots as it's actually been told about, without inventing a "does this node need this slot" oracle. AN0's own driver (C5) sends a node every slot it currently references (its `base_sample` plus every step-lock value found on its driving sequencer) at load time and on each explicit pool-load command; nothing stops AN1/AN2 from sending more later. |
| D5 | `SampleData` is **mono** (`Vec<f32>` + `rate: f32` + `frames: usize`), matching `load_wav`'s current mono-only output. Stereo capture/playback is explicitly deferred (ADR-040 review note 5 flags this as open; AN0 does not close it — it inherits the existing mono limitation, it does not introduce a new one). | `load_wav` (`sampler.rs:900+`) already downmixes to mono; `Sampler::sample_data: Vec<f32>` is already single-channel. AN0 changing *where* the buffer comes from doesn't obligate it to also fix channel count — that is a pre-existing constraint, not new scope. |
| D6 | The asset-delivery plane is a **new dedicated rtrb channel pair**, not a `ConfigMessage` variant. | `ConfigMessage::SetParam` (`crates/paraclete-runtime/src/message.rs:8-21`) is received and silently discarded at `executor.rs:398` — the exact "cautionary precedent" ADR-040 review note 4 warns against. The real working precedent is `state_buf_return` (`configurator.rs:175-176`, `executor.rs:143`), a second SPSC ring used purely to hand an allocation back across the audio-thread boundary without audio-thread alloc/dealloc. `SwapSample` needs the same shape in both directions (payload in, displaced `Arc` out), so it gets its own ring pair rather than overloading `ConfigMessage`. |
| D7 | Displaced `Arc<SampleData>` return uses a **bounded retry, never a drop, on the audio thread.** If the return ring is full, the executor holds the displaced `Arc`s in a pre-allocated `Vec` (`with_capacity` at construction, never grown at runtime) and retries the push next cycle. | Hard constraint 1: `process()` must never allocate, block, or take a lock — and dropping a multi-MB `Vec<f32>` on the audio thread is a `free()` syscall, not RT-safe. A retry-with-preallocated-backlog is the only shape that guarantees zero audio-thread dealloc without blocking. |
| D8 | AN0 ships one concrete producer of `SwapSample` traffic: a `AppOp::PoolLoadFile { slot: u32, path: String }` app-op (ADR-039 drain idiom), driven by a `:pool-load <slot> <path>` command. This is what "no capture" still needs to exercise the asset plane end-to-end. | ADR-040 review note 4 scopes `SwapSample`'s delivery mechanism to AN0 explicitly, but AN0 has no capture UI to trigger it from. A file-load app-op is the minimal legitimate AN0-scope trigger — loading a WAV into a pool slot at runtime is normal pool population, not capture. |
| D9 | The `Sampler` node's own `serialize()`/`deserialize()` bumps from version 2/3 to **version 4**: the length-prefixed `sample_path` string is replaced by a `pool_slot: u32`. Loading a v2/v3 project (a `sample_path` string, no pool) leaves the Sampler with **no sample assigned** — silence, not a crash, not a lookup-by-path fallback. | Pre-1.0, no back-compat guarantee is owed (CLAUDE.md: no feature flags/back-compat shims). A path→pool auto-migration would be exactly the kind of speculative shim the project's conventions reject; the honest behaviour is "old save, new format, no data to carry over" — documented here so it's a deliberate breaking change, not a silent one (guardrail 5). |
| D10 | `instrument.yaml`'s `sample:` node-def key **keeps its current meaning** (a file path) but now means "load this file into the pool at construction and bind this node's `base_sample` to the resulting slot" rather than "load this file directly into the node." `builder.rs` gains the pool-registration step; `Sampler::with_path` is removed. | Keeps `sampler_chain.yaml` (the only fixture exercising Sampler today) working with a one-line semantic change instead of a fixture rewrite, and gives every YAML-defined Sampler a real pool entry from the start — no separate "legacy direct-load" code path to maintain alongside the pool. |

---

## §1. Scope

**In:**

- `SamplePool` app struct + `PoolEntry`, project persistence
  (`crates/paraclete-app/src/sample_pool.rs`, new `Project` field).
- Asset-delivery plane: new rtrb channel pair, `Node` trait hook, executor
  + configurator wiring, RT-safety proven by test.
- `Sampler` node: `P_SAMPLE` param (stepped, pool-indexed, lockable via
  the existing CMD 33–35 family — no new lock code), `P_PLAY_MODE` param
  (stepped: repitch/stretch·loop/oneshot/reverse — surface only, repitch
  is the only DSP until AN3), pool-slot cache, `swap_sample()` override.
- `Sampler` serialize/deserialize v4 (pool-slot index, not path).
- SRC page reshape: `sample · slice · pitch · play_mode · start · end ·
  volume · pan` (8 slots, one page). AMP page keeps `attack`/`release`.
- `builder.rs` wiring: YAML `sample:` registers into the pool at graph
  construction (main thread, pre-audio-thread-start — no asset-plane
  traffic needed for initial load).
- `AppOp::PoolLoadFile` + `:pool-load` command — the one live-mutation
  path AN0 ships, exercising `SwapSample` end-to-end without capture.

**Out, with a citation:**

- **HAL audio input, `AudioInputNode`, `RecorderNode`, capture app-ops,
  the SAMPLING screen, the `FUNC+9` transition-trick chord** — all AN1
  (ADR-040 phase table). AN0 has no recorder to capture from.
- **Slices UI, sample chains** — AN2 (`design.md` §2 feature map row
  "Slices + slice locks"). AN0's `PoolEntry.slice_table` field exists
  (D3-adjacent — see §2.1) but nothing populates or reads it beyond the
  trivial single full-sample slice, same as `Sampler`'s current
  `slices: vec![(0, sample_frames)]` default.
- **Scenes, crossfader, `/context/xfade`** — AN2, additionally gated on
  P11's KitStore (already shipped) plus AN2's own spec.
- **Timestretch DSP** — AN3 (ADR-040 decision 7, OQ-A8). `play_mode`'s
  `stretch` variant exists on the param in AN0 but behaves identically
  to `repitch` until AN3 lands an algorithm.
- **Stereo sample data / stereo capture** — inherited limitation, not
  newly introduced (D5). Revisit when AN1's input path forces the
  question (an external input is very likely stereo).
- **Wiring a Sampler into the default `instrument.yaml`'s 4 tracks** —
  a scope decision about what the default kit sounds like, not an AN0
  architecture question. AN0 proves the mechanism works via
  `sampler_chain.yaml` and unit tests; whether the shipped default
  instrument gains a sample track is a separate, later call.
- **A `:` command to browse/list the pool, or a Theotokos/Theoria pool
  UI** — AN1's SAMPLING screen is the real UI surface for this. AN0's
  `:pool-load` is a minimal loader, not a browser.

---

## §2. Architecture notes

### 2.1 `SamplePool` (mirrors `KitStore`)

`crates/paraclete-app/src/kit.rs` is the template. Its shape:
`Kit { name: String, entries: Vec<KitEntry> }`, `KitEntry { node_id,
param_id, value: f64 }`, `KitStore { kits: Vec<Option<Kit>> }` — plain
`serde` derives, no hand-written RON in the file itself, round-tripped
via `ron::ser::to_string`/`ron::de::from_str` (`kit.rs:96-97`) and via
`Project`'s own derive when embedded.

`sample_pool.rs` mirrors this exactly:

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum SampleSource {
    File(String),
    Ram { frames: usize },   // AN1 fills in real capture metadata
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PoolEntry {
    pub name: String,
    pub source: SampleSource,
    pub frames: usize,
    pub rate: f32,
    pub bars: Option<f32>,
    pub gain: f64,
    pub slice_table: Vec<(usize, usize)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SamplePool {
    pub slots: Vec<Option<PoolEntry>>,
}
```

`SamplePool::default()` fills 128 `None` slots (D2). `load_file(&mut
self, slot: usize, path: &str) -> Result<(&PoolEntry, Arc<SampleData>),
String>` decodes via the hoisted `load_wav` (§2.4), grows `slots` with
`None` padding if `slot >= slots.len()`, stores the `PoolEntry`, and
returns the entry plus the freshly-decoded `Arc<SampleData>` for the
caller to distribute via `SwapSample`.

**Project persistence** (`crates/paraclete-app/src/project.rs`): add
`sample_pool: SamplePool` to `Project` (line 21-33 today) with
`#[serde(default)]`, following the exact precedent of `kits: Vec<Option<Kit>>`
(line 28) and `kit_binding` (line 31) — additive field, no version-number
bump on `Project` itself (the file's own version history at lines 4-6
treats "v3" as "the version that added kits"; this addition folds into
the same tolerance, old files without the field simply default to an
empty pool).

### 2.2 Asset-delivery plane

Two new rtrb ring pairs, owned symmetrically with `state_buf_return`
(`configurator.rs:175-176`, `executor.rs:143`):

```rust
pub struct SwapSample {
    pub node_id: u32,
    pub pool_slot: u32,
    pub data: Arc<SampleData>,
}
```

- **Forward ring** (`sample_swap_producer: rtrb::Producer<SwapSample>`
  on `NodeConfigurator`, `sample_swap_consumer: rtrb::Consumer<SwapSample>`
  on `NodeExecutor`), capacity **32** *(tunable — pool loads are not a
  per-block event; this is generous)*.
- **Return ring** (`sample_return_producer: rtrb::Producer<Arc<SampleData>>`
  on `NodeExecutor`, `sample_return_consumer: rtrb::Consumer<Arc<SampleData>>`
  on `NodeConfigurator`), same capacity.

**`Node` trait gains one default-no-op hook**
(`crates/paraclete-node-api/src/node.rs`, alongside `activate()`,
`deserialize()`, `set_initial_params()`):

```rust
/// Receive (or replace) a cached sample buffer for `pool_slot`.
/// Returns the displaced buffer, if any, for the caller to return
/// via the asset-return ring — the callee must never drop it itself.
fn swap_sample(&mut self, _pool_slot: u32, _data: Arc<SampleData>) -> Option<Arc<SampleData>> {
    None
}
```

**Executor per cycle** (alongside `drain_commands()`, `executor.rs:403`):
drain `sample_swap_consumer` fully; for each `SwapSample`, look up the
target node by `node_id` in the same registry `drain_commands` already
uses for per-node command routing, call `node.swap_sample(pool_slot,
data)`. If it returns `Some(old)`, attempt `sample_return_producer.push(old)`;
on `Err` (ring full), push `old` into a `pending_sample_drops: Vec<Arc<SampleData>>`
field on the executor, **pre-allocated with `Vec::with_capacity(32)` at
construction and never grown** (D7) — each cycle, before draining new
swaps, first drain `pending_sample_drops` into the return ring (oldest
first) so a transient backlog clears without ever allocating or
dropping on the audio thread.

**Configurator main-thread side**
(`process_main_thread`, `configurator.rs:664-670`, same tick that drains
published state and returns the state buffer): drain
`sample_return_consumer` fully and let each `Arc<SampleData>` drop
normally (main thread — safe).

**Test proving D7:** fill the swap ring with more `SwapSample`s than the
return ring can hold in one cycle (mock a node whose `swap_sample`
always returns `Some`), assert no panic/alloc-in-audio-thread (via the
existing allocator-tracking test harness if one exists, else assert the
`pending_sample_drops` vec's capacity never changes across cycles), and
assert every displaced `Arc` eventually reaches the return consumer
across N cycles.

### 2.3 `Sampler` changes

Current state (`crates/paraclete-nodes/src/sampler.rs:210`): struct
holds `sample_data: Vec<f32>`, `sample_path: Option<String>`,
`slices: Vec<(usize,usize)>` directly; `activate()` (line 654) calls
`load_wav` (line 672-696) itself when `sample_path` is `Some`.

**New fields:** `pool_cache: HashMap<u32, Arc<SampleData>>` (D4),
`base_sample: usize` (mirrors `base_slice`'s existing pattern exactly —
a non-bank, node-owned field read through the lock-aware `base_for()`,
`sampler.rs:308`). `sample_path` and the node's own `load_wav()` call in
`activate()` are **removed** — a Sampler no longer decodes anything
itself.

**`swap_sample()` override:** inserts `(pool_slot, data)` into
`pool_cache`, returns the previous value at that key if any (`HashMap::insert`'s
return is exactly the "displaced" value D7 needs — no extra bookkeeping).

**Playback:** wherever `process()` currently reads `self.sample_data`
directly (the render/voice path), it instead resolves the *effective*
`sample` param for the currently-triggered step (base value, or the
step's lock value if one is armed — same `base_for()`/`node_locks`
resolution every other lockable param already goes through, e.g.
`P_SLICE`) as a pool-slot index, and looks it up in `pool_cache`. If the
slot is not cached (not yet delivered, or never referenced), the voice
plays silence for that trigger rather than falling back to a stale
buffer — this is the honest failure mode; C5's driver (§2.5) is
responsible for making sure referenced slots are cached before they can
be triggered.

**`is_known_param()`** (`sampler.rs:302`) gains `P_SAMPLE` to the
lockable whitelist — this is the entire "sample lock" mechanism per
ADR-040 decision 3; CMD 33–35 (`sequencer.rs:460-464`, handler
935-971) are param-id-generic and need no changes. `P_PLAY_MODE` is
**not** added to the lockable set — no design text calls for per-step
play-mode locks, and adding it speculatively would be scope beyond what
§7 of `design.md` specifies.

**`ParamDescriptor` additions** (following the exact
`const P_X: u32 = ParamDescriptor::id_for_name("x")` idiom,
`sampler.rs:34-48`):

```rust
const P_SAMPLE: u32 = ParamDescriptor::id_for_name("sample");
const P_PLAY_MODE: u32 = ParamDescriptor::id_for_name("play_mode");
```

`sample`: stepped, `min: 0.0, max: 127.0, default: 0.0` *(D2 — mirrors
pool nominal size)*, same `ParamUnit` choice as the existing `slice`
param (both are stepped indices into a variable-length collection).
`play_mode`: stepped, `min: 0.0, max: 4.0, default: 0.0` (repitch=0,
stretch=1, loop=2, oneshot=3, reverse=4 — *order tunable*, DSP behavior
identical for repitch/stretch until AN3).

### 2.4 `load_wav` hoist

Current: `fn load_wav(path: &str, target_rate: f32) -> Result<Vec<f32>, String>`
(`sampler.rs:900+`), called only from `Sampler::activate()`. Unchanged
in signature and behaviour — it moves (not rewrites) to
`sample_pool.rs` as a free function `SamplePool::decode_file(path,
target_rate) -> Result<(Vec<f32>, usize, f32), String>` (frames + rate
alongside the samples, since `PoolEntry` needs them), called from
`SamplePool::load_file()` (§2.1) instead of from `Sampler`. Still
main-thread-only, still uses `symphonia` + `rubato`, still mono-only
(D5).

### 2.5 Builder wiring (D10) and pool population

**Graph construction** (`crates/paraclete-app/src/builder.rs:146-149`):
today, `Sampler::with_path(path)` is called directly from
`node_def.sample: Option<String>`. New: the builder holds a
`&mut SamplePool` during construction; for each Sampler node-def with a
`sample:` path, it calls `pool.load_file(next_free_slot, path)`,
constructs `Sampler::new().with_base_sample(slot)`, and — because graph
construction happens entirely on the main thread before the executor
starts — calls `sampler.swap_sample(slot, data)` directly (no
asset-plane traffic needed; the ring only matters once the audio thread
is live). `Sampler::with_path` is deleted; the one call site
(`builder.rs:146-149`) is the only caller.

**`AppOp::PoolLoadFile`** (D8, ADR-039 drain idiom — mirrors the
existing `KitLoad`-style app-ops): `{ slot: u32, path: String }`.
Handled on the main thread: `SamplePool::load_file()`, then for every
live Sampler node whose `pool_cache` already references that slot (or,
simplest correct first cut, every live Sampler node period — D4 doesn't
forbid a node caching a slot it doesn't strictly need, it just doesn't
*require* whole-pool mirroring) push a `SwapSample` onto the forward
ring. **AN0's driver sends it to every registered Sampler node** — this
is the minimal thing that's correct without needing per-node
reference-tracking (which nothing in AN0 needs yet; AN2's slice/chain
work is a more natural place to add "only send what's referenced" if it
ever matters for pool sizes that make broadcast wasteful).

`:pool-load <slot> <path>` — a new debug/console command (wherever the
existing `:set` command is dispatched from) that constructs and submits
the `AppOp`.

---

## §3. Commit plan

### C0 — `SamplePool` struct + project persistence

**Files:**
- `crates/paraclete-app/src/sample_pool.rs` (new) — `SampleSource`,
  `PoolEntry`, `SamplePool`, `SamplePool::decode_file()` (hoisted
  `load_wav`, §2.4), `SamplePool::load_file()`.
- `crates/paraclete-app/src/project.rs` — add `sample_pool: SamplePool`
  field with `#[serde(default)]`.
- `crates/paraclete-nodes/src/sampler.rs` — remove `load_wav` (moved).

**Current behaviour:** No pool exists; each Sampler owns one file.

**New behaviour:** `SamplePool` round-trips through RON (own test,
mirroring `kit.rs:96-97`); `Project` carries an (initially empty) pool;
old v1–v3 project files load fine with an empty pool (`#[serde(default)]`).

**Tests:**
- `SamplePool` RON round-trip (empty, and with 2+ entries incl. a `File`
  source).
- `decode_file()` on a real fixture WAV returns correct frame count/rate
  (reuse whatever fixture `sampler.rs`'s existing `load_wav` tests use).
- `Project` deserializes a v3 fixture file (no `sample_pool` key) into an
  empty-pool `Project` without error.
- `SamplePool::load_file()` grows `slots` when given an out-of-range
  index, padding with `None`.

### C1 — Asset-delivery plane

**Files:**
- `crates/paraclete-runtime/src/message.rs` (or new `asset.rs`) —
  `SwapSample`, `SampleData` (`{ samples: Vec<f32>, rate: f32, frames:
  usize }`).
- `crates/paraclete-node-api/src/node.rs` — `Node::swap_sample()` default
  hook.
- `crates/paraclete-runtime/src/configurator.rs` — `sample_swap_producer`,
  `sample_return_consumer` fields + construction; drain
  `sample_return_consumer` in `process_main_thread` (line 664-670).
- `crates/paraclete-runtime/src/executor.rs` — `sample_swap_consumer`,
  `sample_return_producer`, `pending_sample_drops: Vec<Arc<SampleData>>`
  (pre-allocated capacity 32); drain logic each cycle (§2.2), alongside
  `drain_commands()` (line 403).

**Current behaviour:** No mechanism delivers arbitrary payloads into a
running node. `ConfigMessage::SetParam` is received and discarded
(`executor.rs:398`).

**New behaviour:** A `SwapSample` pushed on the configurator side
reaches the target node's `swap_sample()` within one audio-thread cycle;
any displaced `Arc` reaches the configurator's return consumer within a
few cycles, never dropped on the audio thread even under a full return
ring.

**Tests:**
- End-to-end: push `SwapSample` for a node with a no-op-default
  `swap_sample`, confirm no panic, no return traffic.
- End-to-end: push `SwapSample` twice for the same `(node_id, slot)` on a
  test node whose `swap_sample` returns the old value; confirm the first
  `Arc` appears on the return consumer.
- D7 stress test (§2.2): overflow the return ring, assert
  `pending_sample_drops`'s capacity is unchanged after N cycles (no
  audio-thread allocation) and every displaced `Arc` is eventually
  returned.
- Mutation: remove the `pending_sample_drops` retry path, confirm the
  stress test fails (a displaced `Arc` is lost / return count mismatch).

### C2 — `Sampler`: pool-indexed sample selection

**Files:**
- `crates/paraclete-nodes/src/sampler.rs` — `pool_cache: HashMap<u32,
  Arc<SampleData>>`, `base_sample: usize`, `P_SAMPLE` const, `swap_sample()`
  override, `is_known_param()` gains `P_SAMPLE`, playback path resolves
  the sample via `base_for(P_SAMPLE)` → `pool_cache` lookup (§2.3),
  `sample_path` field and its `activate()` load path removed.

**Current behaviour:** One sample per node, loaded at `activate()` from
`sample_path`. `is_known_param()` has no `P_SAMPLE` — sample selection
cannot be locked or bumped.

**New behaviour:** `sample` is a real, lockable, steppable param. A
Sampler with an empty `pool_cache` plays silence, not a crash. A
step-locked `sample` value plays a different pool slot than the base on
that step (subject to the slot being cached — see C5 for how it gets
cached).

**Tests:**
- `swap_sample()` inserts into `pool_cache`, returns the previous value
  on a second call for the same slot, returns `None` on the first.
- `is_known_param(P_SAMPLE)` is `true`.
- Playback with an uncached slot produces silence (no panic, no stale
  buffer).
- Playback with a cached slot at the base value plays that slot's data.
- With a step lock on `P_SAMPLE`, the locked step plays the locked
  slot's data, not the base slot's — mirrors the existing `P_SLICE`
  lock test pattern 1:1.
- Mutation: remove `P_SAMPLE` from `is_known_param`, confirm the lock
  test fails (lock silently ignored, base slot plays on every step).

### C3 — `Sampler` serialize/deserialize v4

**Files:**
- `crates/paraclete-nodes/src/sampler.rs` — bump node-serialize version
  to 4: write `base_sample: u32` instead of the length-prefixed
  `sample_path` string; `deserialize()` on version 2/3 leaves
  `base_sample` at its default (no sample assigned) per D9.

**Current behaviour:** v2/v3 write/read a path string.

**New behaviour:** v4 write/reads a pool-slot index. Loading a v2/v3
project's Sampler node produces a silent (no-sample) node — documented,
not a crash, not a lookup.

**Tests:**
- v4 round-trip: construct, set `base_sample`, serialize, deserialize
  into a fresh node, assert `base_sample` matches.
- v3 fixture (old format, `sample_path` present) deserializes without
  error into a Sampler with no sample assigned (silence on trigger, per
  C2's silence test).

### C4 — SRC page reshape

**Files:**
- `crates/paraclete-nodes/src/sampler.rs` — `to_rule()` (line 593-629):
  SRC page grows to 8 slots (`sample, slice, pitch, play_mode, start,
  end, volume, pan`); AMP page shrinks to `attack, release` (2 slots).

**Current behaviour:** SRC has 6 slots (`pitch, start, end, root_note,
loop, slice`); AMP has 4 (`volume, pan, attack, release`).

**New behaviour:** SRC matches `design.md` §7's canonical 8-param shape
exactly. `root_note` and `loop` move off SRC — **note:** `design.md` §7
does not list `root_note` or `loop` in the 8-param SRC shape at all.
Per ADR-038 discipline (one param page, no overflow), they relocate to
AMP, which now holds `attack, release, root_note, loop` (4 slots,
matching the page's prior width so no new page is introduced).

**Tests:**
- `to_rule()` produces SRC with exactly the 8 named params in order.
- `to_rule()` produces AMP with `attack, release, root_note, loop`.
- Cap-doc round-trip test (if one exists for other nodes' `Rule`
  changes — follow that pattern) confirming no page-group name
  collisions.

### C5 — `AppOp::PoolLoadFile` + `:pool-load`

**Files:**
- `crates/paraclete-node-api/src/app_op.rs` (or wherever `AppOp` lives) —
  `PoolLoadFile { slot: u32, path: String }` variant.
- Main-thread app-op handler (wherever `KitLoad`-style ops are handled)
  — `SamplePool::load_file()`, then broadcast `SwapSample` to every
  registered Sampler node (§2.5).
- Console/`:` command dispatch — `:pool-load <slot> <path>` constructs
  and submits the app-op.

**Current behaviour:** No runtime path to populate the pool other than
project load.

**New behaviour:** `:pool-load 3 samples/kick.wav` decodes the file,
stores it at pool slot 3, and every live Sampler node's `pool_cache`
gains an entry at key 3 within a few audio-thread cycles — end-to-end
proof the asset plane works without any capture UI.

**Tests:**
- App-op drain: `PoolLoadFile` produces a `SamplePool` entry and at
  least one `SwapSample` per registered Sampler node.
- Integration: construct a small graph with 2 Sampler nodes, submit
  `PoolLoadFile`, run several executor cycles, assert both nodes'
  `pool_cache` (via a test-only accessor) contain the new slot.

### C6 — Review + exit pass

Code review (Flash subagent) on the full diff. `cargo test --workspace`
and `cargo clippy --workspace` (diff against baseline) on every touched
crate. No usability session — AN0 has no performance gesture to
evaluate (see header).

---

## §4. Test plan

| Layer | What | Catches |
|---|---|---|
| Unit (`sample_pool.rs`) | RON round-trip, `decode_file`, slot-growth | persistence format drift, decode regressions |
| Unit (asset plane) | swap delivery, return-ring D7 stress test | lost/leaked `Arc`, audio-thread alloc/dealloc |
| Unit (`Sampler`) | `swap_sample`, `is_known_param(P_SAMPLE)`, silence-on-uncached, base vs. locked playback | lock plumbing regressions, stale-buffer bugs |
| Unit (`Sampler` serialize) | v4 round-trip, v3 backward-read (silence) | format drift, crash-on-old-project |
| Unit (`Sampler` `Rule`) | SRC/AMP page shapes | page-group regressions, param loss |
| Integration (app-op) | `PoolLoadFile` → pool entry → `SwapSample` → node cache, across a multi-node graph | end-to-end plumbing break |

**Mutation checks:**
- C1: remove the `pending_sample_drops` retry, confirm the stress test
  fails.
- C2: remove `P_SAMPLE` from `is_known_param`, confirm the step-lock
  test fails (locked step plays the base slot instead).
- C3: skip the v4 version bump (always read as v3), confirm the
  round-trip test fails (base_sample lost, path bytes misread).

---

## §5. Exit criteria

1. `cargo test --workspace` green after every commit.
2. `cargo clippy --workspace` clean on touched crates (diff against
   baseline).
3. Each commit carries unit tests for the new logic (§4).
4. C1's return-ring stress test (D7) passes — no audio-thread
   allocation or dealloc under a full return ring, verified by capacity
   inspection across N cycles.
5. C2's step-lock test confirms a per-step `sample` lock plays a
   different pool slot than the base — the one line ADR-040 decision 3
   promises ("zero new lock machinery").
6. Code review (Flash) on each commit or logical batch.
7. **No usability session required to close AN0** — the sampling
   session gate is AN1's (ADR-040 R2, roadmap row 4b).

---

## §6. Deferred items (with citations)

| Item | Why deferred | Trigger for re-evaluation |
|---|---|---|
| HAL input / `AudioInputNode` / `RecorderNode` | AN1 scope (ADR-040 phase table) | AN1 spec |
| SAMPLING screen, `FUNC+9` transition trick | AN1 scope; needs recorders to capture from | AN1 spec |
| Slices UI, sample chains | AN2 scope (`design.md` §2) | AN2 spec |
| Scenes, crossfader | AN2 scope, also gated on P11 KitStore (shipped) | AN2 spec |
| Timestretch DSP | AN3 scope (OQ-A8) | AN3 spec |
| Stereo sample data | Inherited limitation (D5), not newly introduced | AN1's input path (likely stereo) forces the question |
| Default `instrument.yaml` gains a sample track | Scope/musical-content decision, not architecture | User schedules it |
| Per-node "only cache referenced slots" (vs. AN0's broadcast-to-all) | Nothing in AN0 needs it; broadcast is simple and correct at current pool sizes | Pool sizes or Sampler-node counts grow enough that broadcast cost is measured, not assumed |
| Pool browser / list command | AN1's SAMPLING screen is the real UI | AN1 spec, or a session asks for it sooner |

---

## §7. Cross-references

- `design/adr/ADR-040-anamnesis-sampling-layer.md` — architecture authority
- `design/adr/ADR-039-performance-state.md` — KitStore / app-op drain precedent
- `design/adr/ADR-038-theotokos-elektron-convergence.md` — SRC page discipline, capability gate
- `design/sampling/design.md` — experience design (§3 pool, §7 SRC page)
- `design/sampling/problem.md` — problem statement, success criteria
- `design/phases/tk3-report.md` — the baseline this phase builds on
- Code: `crates/paraclete-app/src/kit.rs` (KitStore, the structural
  template), `crates/paraclete-nodes/src/sampler.rs`,
  `crates/paraclete-runtime/src/{configurator,executor,message}.rs`,
  `crates/paraclete-app/src/builder.rs:146-149`
