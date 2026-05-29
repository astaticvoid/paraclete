# ADR-007: Scripting Runtime — Rhai

**Date:** May 2026  
**Status:** Accepted

## Decision

Rhai as the L4 scripting runtime. Lua (via mlua) remains the documented alternative if Rhai’s type system proves limiting.

## Rationale

Rhai chosen over Lua for:

- No FFI boundary — pure Rust, no C runtime dependency
- Strong Rust type integration — Rust types are directly usable in scripts without wrapping
- Sandboxed execution with configurable capability restrictions
- Don’t Panic guarantee — a library that never panics the host system
- Surgically configurable as a DSL — keywords and operators can be disabled, custom syntax added
- Performance: 1 million iterations in ~0.14 seconds

Lua (mlua) remains the documented alternative if Rhai proves limiting. The scripting layer is isolated enough that switching runtimes is a contained change.

## Constraints

- Scripts run exclusively off the audio thread
- Scripts cannot block, spawn threads, or access hardware directly
- All script entry points go through a capability-checked API
- Scripts run in a sandboxed Engine with only explicitly registered capabilities