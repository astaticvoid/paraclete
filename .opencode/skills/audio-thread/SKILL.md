---
name: audio-thread
description: Use when editing Node::process(), NodeExecutor, Configurator/Executor split, ring buffers, or any code touching the real-time audio path. The audio thread must never allocate, block, or take a lock.
---

# Audio Thread Constraints

## The two-thread model

```
Main thread (NodeConfigurator)          Audio thread (NodeExecutor)
─────────────────────────────           ────────────────────────────
Owns petgraph DAG                       Owns node instances
Manages lifecycle                       Runs process() in topological order
Sends ConfigMessage via rtrb            Receives ConfigMessage via rtrb
Can allocate, block, lock               MUST NOT allocate, block, or lock
```

## Communication: lock-free ring buffer

`Configurator` → `rtrb::RingBuffer<ConfigMessage>` → `Executor`

ConfigMessages are incremental: the executor applies them at the top of each
callback, never during `process()`.

## process() rules

1. **No allocation:** No `Vec::push`, no `format!`, no `Box::new`, no `String::from`
2. **No blocking:** No `Mutex::lock`, no `Condvar::wait`, no I/O
3. **No lock:** No `Mutex`, no `RwLock`. Use atomics or rtrb for cross-thread data
4. **JSON never touches the audio thread** — all serde happens on the main thread

## Key patterns

### publish_bank_state() OnceLock caching (BUG-007)

```rust
// Cache state bus paths so format! only runs once
static PATH_CACHE: OnceLock<String> = OnceLock::new();
let path = PATH_CACHE.get_or_init(|| format!("/node/{}/param/cutoff", self.node_id));
```

### published_state() push-down

```rust
// Push into caller's buffer — never allocate your own Vec
fn published_state(&self, out: &mut Vec<(String, StateBusValue)>) {
    out.push(("key".into(), StateBusValue::F64(value)));
}
```

### ParamLock per-cycle clearing

```rust
fn process(&mut self, ctx: &mut ProcessContext) -> ProcessResult {
    // Clear at top of every cycle — do NOT route through bank.handle_commands()
    let node_locks: Vec<(u32, f64)> = ctx.drain_param_locks(self.node_id);

    for (step, _) in &mut self.steps {
        // Check locks before falling back to bank values
        let cutoff = node_locks.iter()
            .find(|(id, _)| *id == CUTOFF_ID)
            .map(|(_, v)| *v)
            .unwrap_or_else(|| self.bank.get(CUTOFF_ID));
    }
}
```

### Event delivery ordering (same sample_offset)

1. `ParamLockEvent` — routed by node_id match (not graph edges)
2. `TransportEvent` — position updates
3. `Midi2` — notes and controllers
4. `Surface` — pad events
5. `Extended` — custom

## Configurator/Executor split

- `NodeConfigurator` — main thread, owns graph topology, manages lifecycle
- `NodeExecutor` — audio thread, receives ConfigMessage, executes nodes

## Verification

```bash
# Check for Mutex usage in audio path crates
rg "Mutex|RwLock" crates/paraclete-runtime/src/executor.rs

# Check for allocations in process()
rg "\.push\(|format!|Box::new|String::from" crates/paraclete-nodes/src/ --include '*.rs' -l | xargs rg "fn process"
```
