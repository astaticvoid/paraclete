---
name: commit-workflow
description: Pre-commit checklist and review process. Use before committing changes, when running code review, or when updating design documents.
---

# Commit Workflow

## Pre-commit gates

1. `cargo test --workspace` exit 0
2. `cargo clippy --workspace` — zero new warnings on touched crates (diff against baseline)
3. Issue referenced in commit message (`Fixes #N` / `Refs #N`)
4. Code review on the diff before commit
5. Design review if ADR/spec/protocol changed
6. Working tree clean or explicitly accounted for

## Test trifecta (functionality-changing commits)

Every functionality-changing commit meets all three legs:

**Leg 1 — unit tests** for the new logic. Logic without unit tests has not met the gate.

**Leg 2 — harness live test** — a test-driver scenario with assertions under `tools/test-driver/tests/`, run green before commit. A scenario counts as coverage only if it has been **mutation-checked** (see the mutation-testing skill). If the harness cannot reach the functionality, that is a **defect in the harness** — fix it in this commit.

**Leg 3 — autonomous real-app live session** (per feature milestone, once the surface gesture exists): run the actual app with `RUST_LOG=info`, drive the actual surface. Panel readouts come from the ENC bank cells, not the envelope gauge. A feature that was only harness-tested has not been verified.

Refactor/doc commits with no observable behavior change are exempt; say so in the commit message.

## Code review

Review the diff before committing. Look for:
- Bugs and logic errors
- Architectural or maintenance risks
- Weak test coverage
- Unclear code or unnecessary complexity
- Spec/ADR violations

For blind review, spawn a fresh session (or a prompt template that runs `pi --print`) with the diff and relevant context. A new session has no attachment to the work.

## Design review

Any change to an ADR body, phase spec, protocol definition, or architectural decision triggers a design review. Check contradictions, unverified claims, missing edge cases, guardrail violations, spec gaps, implementation order, and breaking changes.

## Clippy baseline comparison

```bash
cargo clippy --workspace --all-targets > /tmp/clippy-before.txt 2>&1
touch <changed files>
cargo clippy --workspace --all-targets > /tmp/clippy-after.txt 2>&1
diff /tmp/clippy-before.txt /tmp/clippy-after.txt
```

## Commit message style

- Present tense, no trailing period
- `Fixes #N` or `Refs #N` for issue reference
- Type prefix: `feat`, `fix`, `docs`, `tool`, `style`, `refactor`, `test`

## Session close

Before closing a session:
- `git status` clean or explicitly accounted for
- Any untracked files reported
- Stranded notes from `.scratch/SESSION_NOTES.md` filed as issues
- All applicable trackers updated (see AGENTS.md "Keep-current set")
