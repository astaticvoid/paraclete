---
name: session-close
description: Mandatory at the end of every implementation session. Verifies that cargo test/clippy passes workspace-wide, and that all stale design documents are synced. Missing doc updates make a session incomplete — this is the checklist to prevent that.
---

# Session Close Checklist

After code changes land, run this before considering work done.

## Step 1: Test + Clippy (workspace-wide)

```bash
cargo test --workspace && cargo clippy --workspace
```

Every commit must be green on both. Run from workspace root — `cargo test`
without `--workspace` only hits `paraclete-app`.

## Step 2: Design document sync

Check every row in the table below. If *related to any change this session*,
the doc MUST be updated before the session ends. Skip none.

| Doc | Update when… | Action |
|-----|--------------|--------|
| `design/roadmap.md` | A phase/rank ships, is reprioritized, or status changes | Add/update status line |
| `design/bugs.md` | A bug/INFRA item is found, resolved, or a gating assumption changes | **Append only**. Also refresh the top Status block |
| `design/adr/*` | An ADR decision is **implemented** | Update `Status:` line + add implementation note. Body is append-only |
| `design/phases/*` | A phase commit lands | **Append only** |
| `AGENTS.md` | A workflow, command, tool mode, node ID, or convention changes | Edit the relevant section |
| `design/handoff.md` | Task routing or model-tier guardrail changes | Edit the relevant section |

### Workflow

1. Read each doc's current state.
2. For each doc that's stale: make the edit now, in this session.
3. If nothing changed that touches docs: state so explicitly (that's valid).
4. Commit doc changes together with or immediately after the code changes.

## Step 3: Commit quality

- `git status` — no unintended files staged
- `git diff --staged` — review actual changes
- Commit message matches repo style (present-tense, no trailing period)

## Non-obvious gotchas

- `serde_yml` not `serde_yaml`. `serde_yaml` was removed in P9; do not add it back.
- `Hardware*` was renamed to `Surface*` in July 2026. Historical docs use old names — map accordingly; do not edit those documents.
- Design doc bodies (ADRs, bugs.md, phase reports) are **append-only** — never rewrite existing entries.
