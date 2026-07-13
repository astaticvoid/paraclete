---
name: dsp-node
description: Use ONLY when adding a new DSP node (engine, effect, sampler, sequencer) to paraclete-nodes. Covers the multi-file registration checklist, parameter conventions, set_initial_params pattern, published_state push-down, ParamLock per-cycle clearing, and activate()/deserialize() ordering.
---

# DSP Node Registration Checklist

Adding a new node to `paraclete-nodes` requires changes in 6 locations.
Missing any step produces a silently non-functional node.

## 1. Create the module file

`crates/paraclete-nodes/src/my_node.rs`

```rust
use paraclete_node_api::*;
use paraclete_node_api::parameter::ParamDescriptor;

// Short helper for param IDs — each module invents its own shorthand
fn mp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }
```

## 2. Register in `lib.rs`

Two edits needed in `crates/paraclete-nodes/src/lib.rs`:

```rust
// ~line 4-23: add to module declarations
pub mod my_node;

// ~line 25-43: add to pub use re-exports
pub use my_node::MyNode;
```

## 3. Register in `builder.rs`

Two edits needed in `crates/paraclete-nodes/src/builder.rs`:

- `construct_node()` match (~line 136): add arm for your type_tag
- `classify_node()` (~line 188): add arm returning the type_tag string

## 4. Implement `Node` trait

Minimum trait surface (`crates/paraclete-node-api/src/node.rs`):

- `process(&mut self, ctx: &mut ProcessContext) -> ProcessResult`
- `capability(&self) -> CapabilityDocument`
- `activate(&mut self, sample_rate: f64, max_block: usize)`
- `set_initial_params(&mut self, params: &HashMap<String, f64>)` — **NOT** part of the trait; it's a duck-typed convention. `builder.rs` calls it after construction.
- `deserialize(&mut self, data: &[u8])` — called AFTER `activate()` for ParameterBank nodes
- `serialize(&self) -> Vec<u8>`

## 5. Parameter conventions

Use canonical names via `ParamDescriptor::id_for_name()`:

```rust
const CUTOFF_ID: u32 = ParamDescriptor::id_for_name("cutoff");
```

Canonical names: `"cutoff"`, `"resonance"`, `"drive"`, `"wet"`, `"dry"`,
`"decay"`, `"attack"`, `"release"`, `"tune"`, `"swing"`.

**Exception:** `distortion.rs` uses sequential IDs — do not mix with `id_for_name()`.

## 6. Audio thread rules

`process()` must never allocate, block, or take a lock. JSON never touches the
audio thread.

### `published_state()` push-down pattern

Use the push-down signature (do not allocate a Vec per cycle):

```rust
fn published_state(&self, out: &mut Vec<(String, StateBusValue)>) {
    out.push(("cutoff".into(), StateBusValue::F64(self.cutoff)));
}
```

### ParamLock per-cycle clearing

ParamLock must NOT go through `bank.handle_commands()`. Route to a per-cycle
`node_locks: Vec<(u32, f64)>` cleared at the top of each `process()`. Check
your param getter against it before falling back to the bank.

### `publish_bank_state()` with OnceLock

Cache state bus paths in `OnceLock` — no `format!` on the audio thread after
the first cycle (BUG-007 pattern).

## 7. Activation ordering

`activate()` resets the bank to defaults. `deserialize()` re-applies saved
values on top. Call order: `activate()` → `deserialize()`, never the reverse.

## 8. Event delivery ordering (within same sample_offset)

1. `ParamLockEvent` — node_id match, not graph edges
2. `TransportEvent`
3. `Midi2`
4. `Surface`
5. `Extended`
