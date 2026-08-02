---
name: implement
description: Delegate implementation to a Flash subagent via task(). Inline skill — read this, then use task(model='flash', ...). Never edit source files directly.
runAs: inline
---

# Skill: implement

**Never edit source files directly.** When you need to implement a code
change, delegate it to a Flash subagent via `task()` with `model='flash'`
and explicit `write_paths`. The subagent gets Reasonix's full default
prompt — you only need to provide the project-specific context.

## How to delegate

```text
task(
  model='flash',
  write_paths=['crates/foo/src/bar.rs', ...],
  prompt='''
<your task description — be specific about files, spec sections, expected change>

## Project rules
1. Audio thread: process() must never allocate, block, or take a lock.
   JSON never touches the audio thread.
2. Layer boundaries (L0→L5): no layer may reach across another.
   L0 hal → L2 node-api only (not L1 runtime).
3. No tokio. Blocking tungstenite + rtrb only.
4. Logging: use log::info!/warn!/error! — no bare eprintln!/println! in library code.
5. serde_yml not serde_yaml.

## Build/test
- cargo check --workspace (after every edit)
- cargo test --workspace (before reporting done)
- cargo test -p <crate> <test_name> (single test)
- cargo run -p test-driver -- tools/test-driver/tests/<scenario>.yaml --check-baseline

## Workflow
1. Read the relevant files
2. Make the code changes (prefer edit_file over write_file)
3. cargo check --workspace — fix any errors
4. cargo test --workspace — fix any failures
5. Report what you changed (files + summary)
  '''
)
```

## Exceptions (orchestrator may edit directly)

- Config files (.reasonix.toml, .mcp.json, opencode.json, Cargo.toml dev-deps)
- Doc/skill files (AGENTS.md, design/**, .reasonix/skills/**)
- Single-line fixes: typo in a string, one-character clippy suppression, constant rename
- Git operations (add, commit — never stash/reset/checkout)
