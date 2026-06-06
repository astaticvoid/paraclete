# Paraclete — Known Bugs

Append-only. Add new bugs at the bottom. Mark resolved with **Fixed:** line and the commit that fixed it.

---

## Open

### BUG-001 — Step period is 241 ticks (should be 240)

**Severity:** Medium — affects timing precision at all tempos  
**Phase found:** P5  
**Description:** `InternalClock` emits `global_start` on its first tick, consuming one tick before the Sequencer starts. Net result: step period is 241 ticks instead of the intended 240 (at 24 PPQN). Manifests as a systematic ~0.4% BPM error.  
**Location:** `crates/paraclete-nodes/src/internal_clock.rs` — transport start model  
**Fix direction:** Emit `global_start` without consuming a tick, or adjust the step count to account for the extra tick.

---

### BUG-002 — ConnectionAgreement::baseline() hardcodes sample_rate=44100 and block_size=512

**Severity:** Medium — wrong values when engine runs at non-default config  
**Phase found:** P6  
**Description:** `ConnectionAgreement::baseline()` returns a fixed `sample_rate=44100.0` and `block_size=512` regardless of the actual engine configuration. Any node that relies on the baseline agreement for connection negotiation gets wrong values at other sample rates or block sizes.  
**Location:** `crates/paraclete-node-api/src/agreement.rs`  
**Fix direction:** Pass actual sample rate and block size into `baseline()` from the configurator at `connect()` time.

---

### BUG-003 — paraclete-hal depends on paraclete-runtime — L0→L1 layer violation

**Severity:** Low (functional, architecturally wrong)  
**Phase found:** P4  
**Description:** `paraclete-hal` (L0 HAL) imports `paraclete-runtime` (L1 Runtime) for `StateBusHandle`. The five-layer model requires L0 to only depend on L2 (node-api), not L1.  
**Location:** `crates/paraclete-hal/Cargo.toml`  
**Fix direction:** Move `StateBusHandle` to `paraclete-node-api` (L2) so L0 can depend on L2 directly. Blocked until `StateBusHandle` is suitable for LGPL3 publication.

---

### BUG-004 — Sequencer negative micro_offset behaves identically to zero

**Severity:** Medium — signed micro-timing feature silently non-functional  
**Phase found:** P5  
**Description:** `CMD_SET_STEP_TIMING` stores `micro_offset` as `i8` (±47), but negative offsets (pushing a step earlier) require emitting events across the step boundary — i.e. in the previous step's emission window. The current implementation only delays (positive offset); negative values produce no detectable difference from offset=0.  
**Location:** `crates/paraclete-nodes/src/sequencer.rs` — step emission logic  
**Fix direction:** At step N, if step N's `micro_offset < 0`, emit the step event during step N-1's window (wrap around for step 0).

---

### BUG-005 — Sequencer::serialize() does not save P5 fields

**Severity:** Medium — data loss on project save/load  
**Phase found:** P5  
**Description:** `TrigCondition` (probability, repeat, fill), `StepTiming` (micro_offset), and `swing` are all lost on project save/load. A project that programs conditional trigs or timing offsets silently loses that programming across sessions. P9 adds `cv_locks` serialization but P5 fields remain excluded.  
**Location:** `crates/paraclete-nodes/src/sequencer.rs` — `serialize()` / `deserialize()`  
**Fix direction:** Include P5 fields in the serialised `Step` representation; bump serializer version.

---

### BUG-006 — agg_state_buf loses capacity on every send via mem::take()

**Severity:** Low — one reallocation per audio callback after every state bus send  
**Phase found:** P7  
**Description:** The executor sends `agg_state_buf` to the main thread via `mem::take()`, which zeros the field, discarding the allocated capacity. The next cycle starts with a zero-capacity `Vec` and grows again when nodes push entries. The comment "capacity grows back within a few cycles" documents but accepts the repeated reallocation.  
**Location:** `crates/paraclete-runtime/src/executor.rs` — `process_cycle()` state bus send  
**Partial mitigation (P9 C1):** Initial capacity increased from 64 to 256 and per-slot from 8 to 16, reducing the growth cost.  
**Fix direction:** Use a return channel (e.g. a second SPSC) to give back the drained `Vec` to the audio thread after the main thread drains it.

---

### BUG-007 — publish_bank_state() allocates String keys on audio thread every cycle

**Severity:** Low (pre-existing pattern; Sequencer does the same)  
**Phase found:** P9 C1  
**Description:** `publish_bank_state` calls `format!("/node/{}/{}", node_id, name)` per parameter per cycle on the audio thread. `state_bufs.clear()` drops these `String`s each cycle; they are re-created from scratch on the next cycle. The `Vec` outer capacity is retained but String heap data is not.  
**Location:** `crates/paraclete-node-api/src/parameter.rs` — `publish_bank_state()`  
**Note:** The Sequencer's `published_state()` has used the same `format!` pattern since P5. This is an accepted pattern — `published_state()` is not held to the same no-alloc rule as `process()` — but is worth eliminating for large graphs.  
**Fix direction:** Cache formatted path strings in a `Vec<String>` built once at `activate()` time. `publish_bank_state` zips cached paths with `iter_values()` to push pairs with no allocation.

---

### BUG-008 — set_initial_params re-applied on every activate() — overwrites deserialized state on re-activate

**Severity:** Medium — data loss on project load after dynamic topology change  
**Phase found:** P9 C2  
**Description:** All six ParameterBank nodes that implement `set_initial_params()` store the params map in `pending_initial_params` and apply it inside `activate()`. `activate()` is called again whenever dynamic topology rebuilds the executor (P9 C4). After a project load, `deserialize()` sets the bank to saved values; if `activate()` fires a second time before the next save, `pending_initial_params` is non-empty and overwrites the deserialized values. The pending map is never cleared after first use.  
**Location:** `crates/paraclete-nodes/src/analog_engine.rs`, `fm_engine.rs`, `sampler.rs`, `reverb.rs`, `distortion.rs`, `filter.rs` — `activate()` in each  
**Fix direction:** Call `std::mem::take(&mut self.pending_initial_params)` at the end of `activate()` so the map is cleared after first application. Subsequent `activate()` calls (re-activate) apply no params, leaving deserialized values intact.

---

## Resolved

_(none yet)_
