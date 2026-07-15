---
name: refactor
description: Use when making cross-cutting changes (rename, move, signature change) that touch multiple crates. Enforces layer boundary checks, crate dependency audits, and grep-based coverage before each edit.
---

# Refactor Protocol

Cross-crate changes (rename param, move trait, change a public API) can silently
break layer boundaries or miss call sites. This protocol catches those before
the edit.

## Pre-flight: layer boundary audit

Before touching anything, verify the change doesn't cross layers.

```bash
# Which crates does the target module appear in?
rg -l "use paraclete_<crate>" crates/*/src/ --include '*.rs'

# Does the target crate have a dependency on the caller's layer?
# Check Cargo.toml for the relevant dependency line.
rg "paraclete-" <crate>/Cargo.toml
```

### Layer dependency table (from layer-boundary skill)

| Layer | Crate | License | May depend on |
|-------|-------|---------|---------------|
| L0 HAL | `paraclete-hal` | GPL3 | — (special: owns audio callback) |
| L1 Runtime | `paraclete-runtime` | GPL3 | Nothing above L0 |
| L2 Node API | `paraclete-node-api` | **LGPL3** | Nothing internal |
| L3 Nodes | `paraclete-nodes` | GPL3 | L2 only |
| L4 Scripting | `paraclete-scripting` | GPL3 | L2, L1 (via StateBusHandle) |

**Key check:** `paraclete-graph-nodes` is the ONLY crate allowed to depend on
both `paraclete-nodes` and `paraclete-runtime`. If your change creates or
removes such a dependency, stop and verify.

## Step-by-step

### 1. Find all call sites

```bash
# For a symbol rename or signature change
rg "old_name_or_pattern" crates/ --include '*.rs' -l

# For a trait method change
rg "fn method_name" crates/ --include '*.rs' -l

# For a struct field change
rg "\.field_name" crates/ --include '*.rs' -l
```

### 2. Identify affected crates

Group the results by crate. Each crate listed = an edit target.

### 3. Check crate dependencies

For each affected crate, verify it's allowed to depend on the changed crate:

```bash
rg "paraclete-<target>" <affected_crate>/Cargo.toml
```

If the dependency doesn't exist but the code uses types from that crate, the
build will fail — this is a layer violation.

### 4. Execute edits

- Start with the definition (the thing being renamed/moved/changed)
- Then update call sites crate by crate
- Build after each crate edit to catch errors early:

```bash
cargo check -p <crate-i-just-edited> 2>&1 | head -30
```

### 5. Full workspace verification

```bash
cargo build --workspace && cargo test --workspace
```

## Special cases

### Parameter rename

Parameter names are canonical across all nodes. Renaming a param touches:
- `ParamDescriptor::id_for_name("old_name")` in every node module
- State bus paths (`/node/{id}/param/old_name`) in scripts, tests, docs
- `set_initial_params` calls in `builder.rs`
- Any test fixtures or test-driver scenarios

```bash
rg "id_for_name\(\"old_name\"\)" crates/
```

### Trait in `paraclete-node-api` (L2)

Changing `paraclete-node-api` is the highest-risk refactor — it's the LGPL3
boundary. Every node crate depends on it. Test EVERYTHING:

```bash
rg "paraclete-node-api" crates/*/Cargo.toml | wc -l  # number of dependents
cargo test --workspace  # non-negotiable
```

### Running tests selectively

```bash
# Test only the crate you're touching
cargo test -p paraclete-nodes

# Test a specific test function
cargo test -p paraclete-runtime configurator_connect_rejects_two_node_cycle
```
