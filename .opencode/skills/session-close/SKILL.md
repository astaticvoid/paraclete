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

## Step 3: Runtime health checks

After killing paraclete (or if it was run this session):

### 3a. Verify the process is actually dead

```bash
pgrep paraclete || echo "clean"
```

### 3b. Verify HTTP/WSS ports are released

```bash
timeout 1 bash -c 'echo >/dev/tcp/127.0.0.1/7274' 2>/dev/null && echo "port 7274 still in use" || true
```

## Step 4: Working tree integrity

- [ ] `git status` — no unintended files staged
- [ ] `git status --short` — no untracked files that should be committed or `.gitignore`d
- [ ] If dirty: report what and why (explicit, never silent)
- [ ] Unpushed commits: `git log origin/main..HEAD --oneline` — report them
- [ ] `design/todo-scratch.md` — updated with any carryover items for next session

## Step 5: Commit quality

- [ ] `git diff --staged` — review actual changes
- [ ] Commit message matches repo style (present-tense, no trailing period)
- [ ] Subagent code review ran before commit (for non-trivial changes)

## Non-obvious gotchas

- `serde_yml` not `serde_yaml`. `serde_yaml` was removed in P9; do not add it back.
- `Hardware*` was renamed to `Surface*` in July 2026. Historical docs use old names — map accordingly; do not edit those documents.
- Design doc bodies (ADRs, bugs.md, phase reports) are **append-only** — never rewrite existing entries.
