---
name: covers-this
description: Use before editing shared or real-time-path code (Node::process, NodeExecutor, ring buffers, frequently-called utilities). Queries the narsil MCP call-graph to identify every caller of the target symbol, so edits don't miss call sites or violate audio-thread constraints.
---

# Call-Graph Coverage ("Covers This?")

Before editing shared code (especially in the audio path), determine everything
that calls the target. Missing a caller in a real-time context can introduce
allocations, locks, or blocking that crash the executor.

## When to invoke

- Editing any `fn process()` implementation
- Changing a function signature used by multiple nodes
- Touching `NodeExecutor`, `ProcessContext`, ring buffer internals
- Modifying a utility function in `paraclete-runtime` or `paraclete-node-api`
- Auditing after a `use` statement removal

## Querying narsil

The `narsil-mcp` server is configured in `opencode.json` with `--call-graph`.
Use the MCP tools it provides to ask:
- "Who calls `AnalogEngine::process`?"
- "What are all callers of `ProcessContext::drain_param_locks`?"
- "Show me the incoming edges to `RingBuffer::push`"

The MCP exposes these automatically — you can ask in natural language.

## Manual fallback (no MCP / quick check)

```bash
# Find all call sites for a function name (text search, no type resolution)
rg "function_name\b" crates/ --include '*.rs' -n

# Find all implementors of a trait method (e.g. process)
rg "fn process\b" crates/paraclete-nodes/src/ --include '*.rs' -n

# Find all uses of a struct field
rg "\.node_id\b" crates/ --include '*.rs' -n

# Find where a type is constructed
rg "::new\b" crates/paraclete-nodes/src/ --include '*.rs' -n
```

## Audio-thread caller checklist

If the target is called from `process()` or any audio-thread path, verify every
caller against these rules:

1. **No allocation** — no `Vec::push`, `format!`, `Box::new`, `String::from`
2. **No blocking** — no `Mutex::lock`, `Condvar::wait`, I/O
3. **No lock** — no `Mutex`, `RwLock`; use atomics or rtrb
4. **No JSON** — serde never touches the audio thread

```bash
# Audit a specific file for audio-thread violations after an edit
rg "Mutex|RwLock|format!|Box::new|String::from|serde" <file> --include '*.rs'
```

## Example workflow

1. Identify the symbol to change: `ProcessContext::push_event`
2. Query narsil: "Show all callers of `push_event`"
3. For each caller:
   - Is it in `process()`? → apply audio-thread rules
   - Is it in `NodeConfigurator`? → main thread, fewer restrictions
   - Is it in `builder.rs`? → construction-time only, full freedom
4. Confirm the change is safe for each call site
5. Build each affected crate: `cargo check -p <crate>`

## Paraclete-specific hotspots

These symbols have exceptionally many callers — manually verify extra carefully:

| Symbol | Location | Callee count (approx) |
|--------|----------|-----------------------|
| `ParamDescriptor::id_for_name` | `paraclete-node-api` | Every node module |
| `ProcessContext::push_event` | `paraclete-runtime` | Every node `process()` |
| `ParameterBank::get` / `set` | `paraclete-node-api` | Every ParameterBank node |
| `Node::process` trait method | `paraclete-node-api` | Every node (25+) |
| `SurfaceOutputHandle` trait | `paraclete-node-api` | Every surface node |
