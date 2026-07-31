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

## Step 2: Tracker and design document sync

Check every row in the table below. If *related to any change this session*,
it MUST be updated before the session ends. Skip none.

Live state is **GitHub Issues**, not `design/` (migrated 2026-07-30 — see
`design/README.md`).

| Where | Update when… | Action |
|-----|--------------|--------|
| **GitHub Issues** | A bug is found or fixed; an open question is answered; a spike concludes | Open/close the issue. Close it in the resolving commit (`Fixes #N`), not in a later sweep |
| **GitHub Milestones** | A phase completes | Close its milestone, open the next. The open milestone is the "what next" pointer |
| `design/adr/*` | An ADR decision is **implemented** | Update `Status:` line + add implementation note. Body is append-only |
| `design/phases/*` | A phase commit lands | **Append only** |
| `design/roadmap.md` | The phase *sequence* or a design *gate* changes | Edit the relevant row. **Not** for status — status is the milestone |
| `AGENTS.md` | A workflow, command, tool mode, node ID, or convention changes | Edit the relevant section |

### Workflow

1. Check open issues touched by this session: `gh issue list --state open`.
2. For anything stale: make the edit or close the issue now, in this session.
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
- [ ] Carryover for the next session filed as issues (label `carryover`)

## Step 5: Commit quality

- [ ] `git diff --staged` — review actual changes
- [ ] Commit message matches repo style (present-tense, no trailing period)
- [ ] Subagent code review ran before commit (for non-trivial changes)

## Non-obvious gotchas

- `serde_yml` not `serde_yaml`. `serde_yaml` was removed in P9; do not add it back.
- `Hardware*` was renamed to `Surface*` in July 2026. Historical docs use old names — map accordingly; do not edit those documents.
- Design doc bodies (ADRs, phase reports, session notes) are **append-only** — never rewrite existing entries.
