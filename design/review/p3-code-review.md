# P3 Code Review

Reviewing commit c60d556 ("Paraclete — P3 complete") relative to P2 (595f3b2).
P3 introduced: `Negotiable` trait + connection handshake, `StateBus` SPSC upgrade,
`ParamLockEvent` priority ordering, `Sampler` node, and Rhai state bus bindings.

P2 issues carried forward (not re-flagged here unless P3 made them worse) include
C-2, C-3, C-4, C-5, S-3 (step-0 skip), S-4 (param_locks.clone), and C-6 (mem::take).
P3 did address P2's C-1 (RwLock) and S-1 (negotiate never called).

---

## Critical (must fix before building further)

### C-1: Sampler param lock filtering silently drops locks for zero-default params

**File:** `crates/paraclete-nodes/src/sampler.rs`, `process()`, line ~314

```rust
if self.base_for(param_id) != 0.0 || param_id == param_hash("pitch") {
    self.node_locks.insert(...);
}
```

`base_for()` returns the node's *current base value* for a param. Three params
default to 0.0: `pitch`, `pan`, and `start`. The condition checks whether the
base value is non-zero as a proxy for "is this a known param ID" — which is
semantically wrong. The explicit `|| param_id == param_hash("pitch")` rescues
pitch, but `pan` (default 0.0) and `start` (default 0.0) are silently dropped
whenever a sequencer tries to apply a param lock to them at their default base
value.

**Concrete failure:** A sequencer step with `pan` locked to `+1.0` (hard right)
is silently ignored when `base_pan == 0.0`. The lock that was supposed to move
the sound right does nothing — the note plays center. There is no error, warning,
or indication that the lock was discarded.

**Why it matters:** The param lock protocol is the central feature of P3. Dropping
locks silently for two of seven declared lockable parameters (pan and start) is a
correctness bug that will propagate into P4 hardware control and P5 live performance
features. The bug is not caught by any existing test.

**Fix direction:** Replace the `base_for() != 0.0` heuristic with a proper known-ID
check. The simplest correct implementation:

```rust
let is_known = self.capability_document().params.iter().any(|p| p.id == param_id);
if is_known { self.node_locks.insert(...); }
```

For zero-allocation on the audio thread, pre-compute a `[u32; 7]` array of known
param IDs at `activate()` time and check against it in `process()`.

**Status:** Not fixed in P4 (current working tree has identical code).

---

### C-2: `entries.clone()` allocates unconditionally on the audio thread every cycle

**File:** `crates/paraclete-runtime/src/executor.rs`, lines ~223-228

```rust
let entries: Vec<(String, StateBusValue)> = self.nodes.iter()
    .flat_map(|s| s.published_state())
    .collect();

if !entries.is_empty() {
    let _ = self.state_bus_producer.push(StateBusUpdate { entries: entries.clone() });
}

let hw_out = self.build_hardware_output(&entries);
```

Three allocations happen on the audio thread every cycle:

1. `published_state()` per node returns `Vec<(String, StateBusValue)>` — N heap
   allocations, one per node, plus one `format!()` allocation per string key.
2. `.collect()` allocates the consolidated `entries` Vec.
3. `entries.clone()` clones the entire Vec again for the SPSC push — this is new
   in P3 and not present in P2's approach. Both the collect and the clone happen
   even when the ring is full and the push will be discarded.

The P3 SPSC upgrade correctly eliminated the RwLock (P2 C-1), but introduced a
clone operation that was not in P2's implementation. P2 at least only did one
allocation+insert path. P3 does two allocations for the Vec plus N string format!()
calls — and then clones all of it.

The `published_state()` returning `Vec<(String, StateBusValue)>` was flagged as
P2 S-5 (calling from audio thread). P3 kept the call site and added a clone on
top of it.

**Why it matters:** The audio-thread allocation prohibition is a hard constraint
(CLAUDE.md). This is the same constraint that drove the P3 SPSC upgrade, but the
upgrade introduced a new allocation path instead of eliminating the old one. At
86 cycles/sec (512-frame block), this is thousands of allocations per second.

**Fix direction:** Two steps:
1. Remove `entries.clone()` by pushing the Vec directly (move, not clone) into
   `StateBusUpdate`, then separately query `self.nodes` for `build_hardware_output`
   using a pre-allocated buffer. Or restructure so `build_hardware_output` reads
   from the `StateBusHandle` after the main thread drains the SPSC.
2. Long-term: replace `published_state() -> Vec<...>` with a push model:
   `fn push_state(&self, sink: &mut StateBusSink)` where `StateBusSink` wraps a
   pre-allocated ring-buffer slot. This eliminates all per-cycle allocations for
   state publishing.

**Status:** Not fixed in P4 (current working tree has identical code).

---

### C-3: `Negotiable` trait is a structural dead-end — behavior belongs on the trait, not on `Node`

**File:** `crates/paraclete-node-api/src/node.rs`, last 15 lines;
`crates/paraclete-runtime/src/configurator.rs`, `connect()`, lines ~185-195

The P3 spec placed `negotiate()` and `set_connection_record()` on the `Negotiable`
trait. The implementation moved both to `Node` as default methods, leaving `Negotiable`
as an empty marker:

```rust
pub trait Negotiable: Node {}  // empty
```

The runtime calls `negotiate()` via `NodeOrDevice::negotiate()` on every node in
every `connect()` call — not just on nodes that implement `Negotiable`. The marker
trait has zero behavioral effect. A node author who intentionally overrides
`negotiate()` but forgets to `impl Negotiable` will still be called in the handshake;
one who properly marks `impl Negotiable` but doesn't override `negotiate()` gets
only the baseline default — same as any other node.

This inverts the intent: in the L2 LGPL API, third-party nodes now have two methods
on `Node` that they are not expected to implement unless they are "smart nodes," with
no trait boundary separating the optional behavior. Discovery ("does this node
participate in negotiation?") requires checking for the `Negotiable` marker, but the
marker cannot enforce the behavior.

**Why it matters:** This is an API contract shipped at L2 (LGPL). Once third-party
nodes exist, moving `negotiate()` and `set_connection_record()` from `Node` to
`Negotiable` (or back to the spec's design) is a breaking change. Fixing it now
costs one refactor; fixing it after external nodes exist costs a major version bump.

**Fix direction:** Restore the spec design: move `negotiate()` and
`set_connection_record()` back to the `Negotiable` trait with concrete method bodies.
Update `configurator.connect()` to downcast or use trait objects to call these only
on nodes that implement `Negotiable`. Since `Box<dyn Node>` cannot be downcast to
`Box<dyn Negotiable>` without `Any`, an alternative is a separate registration path
(`add_negotiable_node`) or a method on `Node` that returns `Option<&mut dyn Negotiable>`.

**Status:** Not addressed in P4 (current working tree has the same empty marker).

---

## Significant (fix before P5)

### S-1: `node_locks` bleed into subsequent steps when steps are legato

**File:** `crates/paraclete-nodes/src/sampler.rs`, `release_voice()`

`node_locks` is cleared in `release_voice()` (triggered by NoteOff). When a step's
`length` parameter is `>= 1.0` (legato), the Sequencer does not emit a NoteOff before
the next NoteOn. The gate stays open. When step N+1 fires, `node_locks` still contains
step N's param overrides. The next `trigger_voice()` copies `node_locks` into the new
voice's `active_locks`, causing step N's locks to apply to step N+1's note — a phantom
lock on a step that has no param override.

**Concrete failure:** With `volume` locked to 0.5 on step N and no lock on step N+1,
step N+1 in a legato sequence will play at volume 0.5 instead of the base volume.

**Fix direction:** In `trigger_voice()`, clear `node_locks` before copying into
`voice.active_locks`. Step N's voice already has its locks captured; new voices
should not inherit lingering node-level state from previous steps. Alternatively,
clear `node_locks` at step boundaries (beginning of handle_transport when gate
is still open and a new step fires).

**Status:** Not fixed in P4 (node_locks cleared only in release_voice).

---

### S-2: `initial_transport` is always `None` — nodes cannot position themselves at connect time

**File:** `crates/paraclete-runtime/src/configurator.rs`, `connect()`, line ~219

```rust
let reconciled = ConnectionAgreement {
    ...
    initial_transport: None,  // never populated
    ...
};
```

The `ConnectionAgreement::initial_transport` field was designed so a node connecting
mid-session (e.g. a sequencer added while the clock is running) can receive the
current transport position and start in sync. The `NodeConfigurator` holds the clock
state (via the `StateBus` from the previous cycle) but never populates
`initial_transport` at `connect()` time.

**Why it matters:** Any node (in P4 or beyond) that reads `initial_transport` in
`set_connection_record()` for sync purposes will see `None` and either start at
position 0 (a jump) or have to implement its own workaround. The field was
advertised in the CLAUDE.md architecture docs and the P3 spec as the mechanism
for mid-session sync. It ships non-functional.

**Fix direction:** In `connect()`, read the current transport state from the StateBus
(e.g. `/transport/bpm`, `/transport/playing`) and construct a `TransportInfo` to
populate `initial_transport`. Alternatively, have `NodeConfigurator` store the last
transport state (as it does `sample_rate` and `block_size`) and use it here.

---

### S-3: `lockable_params` reconciliation is dst-only — src side's params are silently discarded

**File:** `crates/paraclete-runtime/src/configurator.rs`, `connect()`, lines ~212-220

```rust
let reconciled = ConnectionAgreement {
    ...
    lockable_params: dst_agreement.lockable_params,  // src side discarded
};
```

Both `src_agreement` and `dst_agreement` are computed from `negotiate()`, but only
`dst_agreement.lockable_params` ends up in the reconciled record. Both nodes receive
the same reconciled record, meaning:

- The source node (e.g. Sequencer) receives `lockable_params` from the destination
  (Sampler) — this is the intended use case for discovering what the downstream
  instrument exposes.
- If the source node also declared lockable params in its own `negotiate()`, those
  are silently dropped and neither node sees them.
- The reconciliation is asymmetric and undocumented as such.

**Why it matters:** For the current Seq→Sampler topology this happens to be correct
(the Sequencer doesn't declare lockable params), but it's a latent error for any
future "smart" source node. The runtime's behavior deviates from the stated protocol
without documenting why.

**Fix direction:** Either document that `lockable_params` is always the dst's
declaration (rename it `dst_lockable_params` to be explicit), or implement proper
merging and give each side a record with its own partner's params. At minimum, add
an inline comment explaining why only `dst_agreement.lockable_params` is used.

---

### S-4: `LockableParam` loses the `ParamUnit` from `ParamDescriptor`

**File:** `crates/paraclete-nodes/src/sampler.rs`, `lockable_params_list()`, line ~223

```rust
.map(|p| LockableParam {
    param_id: p.id,
    name: p.name.as_str().to_string(),
    min: p.min,
    max: p.max,
    default: p.default,
    unit: ParamUnit::Generic,  // hardcoded — ignores p.unit
})
```

The `ParamDescriptor` already has a `unit` field (`Semitones` for pitch, `Percent`
for start/end, etc.). `lockable_params_list()` silently downgrades every parameter
to `ParamUnit::Generic`. Any consumer of `ConnectionRecord.agreement.lockable_params`
(a GUI, a hardware controller, a script) receives incorrect unit information.

**Why it matters:** A step sequencer using `lockable_params` to display a pitch lock
value would show "0.0" instead of "+0 st" because it has no unit context. This is
an information loss in the L2 public API that propagates to all consumers.

**Fix direction:** Replace `unit: ParamUnit::Generic` with `unit: p.unit.clone()`.
Requires `ParamUnit` to implement `Clone`, which it currently does.

**Status:** Not fixed in P4.

---

### S-5: `Sampler.connection_records` is `pub(crate)` — an internal detail exposed to the crate

**File:** `crates/paraclete-nodes/src/sampler.rs`, struct definition

```rust
pub(crate) connection_records: Vec<ConnectionRecord>,
```

`connection_records` is declared `pub(crate)`, making it accessible to any code in
`paraclete-nodes`. The field is only used in tests (to assert `partner_id`). The
`ConnectionRecord` struct holds the full `ConnectionAgreement` including all
`lockable_params` — this is moderately sensitive state that should not be readable
from outside the type. The test access should use a dedicated accessor or a
`cfg(test)` inner module.

**Fix direction:** Make the field private. Add a `#[cfg(test)]` accessor in the test
module, or make the test use the public `Node::set_connection_record()` indirectly.

---

### S-6: `write_sandboxed` allows `/node/*/state/*` — scripts can overwrite node-published state

**File:** `crates/paraclete-node-api/src/state_bus.rs`, `write_sandboxed()`

```rust
if !path.starts_with("/node/") {
    return Err("scripts may only write to /node/ paths");
}
```

The sandbox check allows any `/node/` path, including `/node/{id}/state/*`. Paths
under `state/` are published by nodes from the audio thread via `published_state()`.
Scripts can overwrite these, causing the GUI or LED feedback to display incorrect
values that conflict with what the node actually reports next cycle. The CLAUDE.md
spec says scripts may write to `/node/{id}/param/*` — not `state/`.

**Fix direction:** Tighten the check to `/node/*/param/`:

```rust
if !path.contains("/param/") || !path.starts_with("/node/") {
    return Err("scripts may only write to /node/{id}/param/ paths");
}
```

Or use a regex/split to enforce the exact `"/node/{int}/param/"` prefix structure.

---

### S-7: No runtime test verifies that `connect()` actually invokes the negotiation handshake

**Files:** `crates/paraclete-runtime/tests/runtime_integration.rs`;
`crates/paraclete-node-api/tests/node_implementations.rs`

The P3 spec requires a test: "Negotiable handshake invoked at `connect()` — both
sides receive `ConnectionRecord`." The only test for negotiation
(`test_level3_node_negotiate_is_called_at_connection_time`) calls `node.negotiate()`
directly — it does not call `conf.connect()`. There is no test that:

1. Creates two nodes, at least one implementing `Negotiable`.
2. Calls `conf.connect()`.
3. Verifies that both nodes received a `ConnectionRecord` with correct content.
4. Verifies that the `lockable_params` from the Sampler's `negotiate()` appear in the
   record delivered to the Sequencer.

The runtime integration test file has zero references to `negotiate`, `Negotiable`,
`set_connection_record`, or `lockable_params`. P3's central protocol ships without
end-to-end test coverage.

**Fix direction:** Add a runtime integration test:

```rust
#[test]
fn connect_invokes_negotiation_and_delivers_connection_record() {
    let received_record: Arc<Mutex<Option<ConnectionRecord>>> = Arc::new(Mutex::new(None));
    // ... set up NegotiatingNode that stores record in set_connection_record()
    let mut conf = NodeConfigurator::new(44100.0, 512);
    conf.add_node(1, Box::new(negotiating_src));
    conf.add_node(2, Box::new(negotiating_dst));
    conf.connect(1, 0, 2, 0).unwrap();
    // Assert both nodes received ConnectionRecord with correct partner_id
    // Assert lockable_params from dst appear in src's record
}
```

---

### S-8: No runtime test for `ParamLockEvent` delivery ordering

**File:** `crates/paraclete-runtime/tests/runtime_integration.rs`

The P3 spec requires: "ParamLockEvent delivered before Midi2 NoteOn at same
`sample_offset`." No runtime test verifies this. The sort in the executor is
correct but untested. If a future refactor removes or reorders the sort, the
parameter lock protocol silently breaks (locks arrive after the note they were
supposed to modify, and the note plays with base values).

**Fix direction:** Add an integration test with a node that records the order of
received events. Emit a `ParamLock` and a `NoteOn` at the same `sample_offset`
from an upstream node. Verify that `ParamLock` arrives first in the downstream
node's event slice.

---

### S-9: `update_output()` still called from the audio thread — P2 C-7 not fixed by P3

**File:** `crates/paraclete-runtime/src/executor.rs`, lines ~217-220

```rust
let hw_out = self.build_hardware_output(&entries);
for slot in &mut self.nodes {
    slot.hw_update(&hw_out);
}
```

`build_hardware_output()` allocates two `Vec`s (for `led_updates` and
`display_updates`) on the audio thread every cycle, and `hw_update()` calls
`HardwareDevice::update_output()` from inside `executor.process()`. For
`LaunchpadEmulator` this means terminal I/O on the audio thread. P2 flagged this
as C-7 and M-9. P3 added a `// TODO P4` comment but did not move the call site.

This is noted not as a P3 regression (it existed in P2) but as a P3 opportunity
missed: the `StateBusUpdate` mechanism introduced in P3 provides the natural hook
for moving LED output to the main thread, but `process_state_bus()` does not call
`update_output()` on hardware devices. The main-thread path is half-built.

**Fix direction:** In `process_state_bus()`, after draining the SPSC, build
`HardwareOutput` from the updated `StateBusHandle` and call `update_output()` on
all registered hardware devices (keep them in a `Vec` in `NodeConfigurator`
alongside `nodes`). Remove the call from `executor.process()`. This is what the
P4 `HardwareOutputHandle` spec formalizes.

---

## Minor (worth noting but low urgency)

### M-1: Rhai `StateBusProxy::write()` accepts only `f64` — no way to write `Bool` or `Text` values

**File:** `crates/paraclete-scripting/src/lib.rs`, `StateBusProxy::write()`

```rust
pub fn write(&mut self, path: &str, value: f64) { ... }
```

`StateBusValue` has four variants: `Float`, `Int`, `Bool`, `Text`. Scripts can only
write floats. A script cannot write a boolean flag (e.g. `node/5/param/loop` which is
0.0/1.0 — acceptable as float) or text labels. This is not critical at P3 but limits
future scripting use cases. Worth documenting as a known limitation.

---

### M-2: `sort_unstable_by_key` does not provide stable ordering for same-priority events

**File:** `crates/paraclete-runtime/src/executor.rs`, `process()` sort block

Events with identical `(sample_offset, priority)` — e.g. two `NoteOn` messages at the
same sample position — are reordered non-deterministically by `sort_unstable`. For
NoteOn events this is benign (order of simultaneous note triggers is not defined). For
`Transport` events, two `Transport` events at the same offset with different fields
could arrive in either order. Not a practical concern at P3's single-sequencer topology
but worth noting for multi-track P5+.

---

### M-3: `Sampler::Voice` uses `HashMap` for `active_locks` — initialized empty, may allocate on trigger

**File:** `crates/paraclete-nodes/src/sampler.rs`, `Voice::new()` and `trigger_voice()`

Each `Voice` holds `active_locks: HashMap<u32, ActiveParamLock>`, initialized with
`HashMap::new()` (capacity 0). When `trigger_voice()` copies `node_locks` into
`voice.active_locks` via `insert()`, if the map's capacity is exceeded the HashMap
reallocates on the audio thread. The maximum number of param locks is 7 (all Sampler
params), which triggers at least one reallocation from capacity 0.

Similarly, `node_locks` in `Sampler` itself is a `HashMap<u32, ActiveParamLock>` and
shares the same growth pattern.

**Fix direction:** Pre-allocate both maps with a known upper bound:
`HashMap::with_capacity(MAX_PARAMS)` where `MAX_PARAMS = 7` matches the Sampler's
param count. This eliminates reallocation inside the audio callback.

---

### M-4: `lockable_params_list()` calls `capability_document()` on every invocation, allocating a Vec of `ParamDescriptor`s

**File:** `crates/paraclete-nodes/src/sampler.rs`, `lockable_params_list()`

```rust
fn lockable_params_list(&self) -> Vec<LockableParam> {
    self.capability_document().params.iter()...
}
```

`capability_document()` constructs a `CapabilityDocument` with a freshly allocated
`Vec<ParamDescriptor>`. `lockable_params_list()` is called from `negotiate()`, which
is called at connection time (main thread, not audio thread) — so allocation is fine.
But the indirection is wasteful when the param list could be iterated directly.

This is low priority (main-thread allocation at connect time) but the pattern of
building the full `CapabilityDocument` just to iterate params creates unnecessary
coupling between the two methods.

---

### M-5: `StateBusSubscription` does not notify on the first `subscribe()` call consistently

**File:** `crates/paraclete-node-api/src/state_bus.rs`, `poll_subscription()`

When `subscribe()` is called, `last_value` is `None`. The first `poll_subscription()`
call compares `current` (which may already have a value) against `None`, reports
`changed = true`, and returns the current value. This means subscribing to a path
that already has a value in the bus will fire immediately on the first poll —
which may be surprising to callers expecting only *future* changes.

This behavior is tested (`state_bus_subscription_detects_first_change` passes), but
the behavior ("immediately fires if value exists") is not documented. Downstream
subscribers (Rhai scripts, future GUI) may accidentally react to stale state on startup.

---

### M-6: `Sampler::serialize()` uses `base_slice as u8` — silently truncates slice indices > 255

**File:** `crates/paraclete-nodes/src/sampler.rs`, `serialize()`

```rust
buf.push(self.base_slice as u8);
```

`base_slice` is declared as `usize`. If more than 255 slices are defined (unlikely at
P3's stub implementation but planned for P5 sample chopping), the serialized value
wraps silently. `deserialize()` reads it back as `u8` and widens to `usize`, so it
round-trips correctly only for values 0-255.

**Fix direction:** Use `u16` for the slice index in the serialization format and bump
the format version if needed.

---

## Confirmed correct (things that look suspicious but are intentional)

**SPSC ring capacity of 256 StateBusUpdates:** At ~86 cycles/sec (512 frames at 44100
Hz), the main thread needs to drain within ~3 seconds before updates are lost. One
missed update means one cycle of stale LED state, not a crash. The capacity is more
than adequate, and the silent drop-on-full behavior is correct for non-critical display
state.

**`process_state_bus()` uses `RefCell::borrow_mut()`, not a mutex:** The
`StateBusHandle` is `Rc<RefCell<...>>` shared between `NodeConfigurator` and the
scripting engine. Both live on the main thread. The `RefCell` enforces single-active-
borrow at runtime. In the P3 app, `process_state_bus()` and script evaluation are
never interleaved (scripting is only used for startup validation), so there is no
runtime borrow conflict. When P5 adds live scripting in the main loop, the caller
will need to ensure scripts do not hold a borrow across `process_state_bus()`.

**Event sort correctly places `ParamLock` before `NoteOn` at the receiver:** The
Sequencer emits `NoteOn` first, then `ParamLock`s into its `events_out`. These
propagate to the Sampler's `incoming[]` buffer. The executor sorts the Sampler's
`incoming[]` before calling `Sampler::process()`, using `(sample_offset, priority)`
where `ParamLock = 0` and `Midi2 = 2`. So `ParamLock` is correctly delivered before
`NoteOn` at the same sample offset, even though the Sequencer emitted them in reverse
order. The mechanism works.

**`mem::take` pattern is unchanged from P2:** P3 did not fix P2's C-6
(audio-thread allocation via `mem::take` on the incoming event vecs). The sort
added in P3 operates on the already-taken Vec. The C-6 finding from the P2 review
remains open.

**Rhai sandbox correctly rejects `/transport/` writes:** The `write_sandboxed()` check
blocks `/transport/bpm` and `/hw/` writes. The test `scripting_engine_state_bus_write_to_transport_is_rejected`
verifies this. The sandbox correctly silently ignores rejected writes (returns `Ok(())`)
to avoid script panics — this is consistent with Elektron-style "ignore bad input"
design.

**`LockableParam.param_id` matches `ParamDescriptor.id`:** Both use `ParamDescriptor::id_for_name()`
(FNV-1a hash). A `ParamLockEvent` addressed to `param_hash("pitch")` correctly routes to
the `pitch` parameter inside the Sampler. The hash is computed consistently and the
matching is correct.

**`ConnectionAgreement` hardcodes `channels: 2` in reconciliation:** The runtime
currently only supports stereo output. Hardcoding channels=2 in the reconciled
agreement is a known limitation noted in the P3 spec ("single stereo buffer").
Not flagged as a P3-introduced flaw.
