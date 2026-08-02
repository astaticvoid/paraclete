---
name: commit-gate
description: 'Pre-commit checklist: tests green, clippy clean, code review done, issue referenced, design reviewed (if applicable), working tree clean. Run before every commit.'
---

# Commit Gate

Pre-commit checklist. The orchestrator runs this after every atomic change, before `git commit`. Each gate must pass or be explicitly waived with a reason in the commit message.

## Gates (run in order)

### Gate 1: Tests green
```bash
cargo test --workspace
```
Exit 0 required. If a test failure is pre-existing and unrelated, document it in the commit message body as `Pre-existing: <test name> — <reason>`.

### Gate 2: Test trifecta (functionality-changing commits)

Every functionality-changing commit meets all three legs (the standing
directive; "trifecta review" in session shorthand):

**Leg 1 — unit tests** for the new logic: the functions/edge cases added by
this commit have `#[test]` coverage. Logic without unit tests has not met
the gate.

**Leg 2 — harness live test** proving the behavior through the running graph —
a test-driver scenario with assertions under `tools/test-driver/tests/`, run
green before commit:
```bash
timeout 120 cargo run -p test-driver -- tools/test-driver/tests/<scenario>.yaml
```
- A scenario counts as coverage only if it has been **mutation-checked**:
  break the code it covers, confirm the scenario fails, restore. A live
  test that has never been seen to fail is not evidence.
- If the harness cannot reach the functionality, that is a **defect in the
  harness** — fix the harness in this commit. Harness gaps are not waivers.

**Leg 3 — autonomous real-app live session** (per feature milestone, once the
surface gesture exists): run the actual app and drive the actual surface with
`RUST_LOG=info` (the app's own `log::info!` lines are the evidence channel —
silent otherwise; see AGENTS.md Logging). Panel readouts come from the ENC
bank cells, not the envelope gauge. Driving recipes: AGENTS.md "Driving the
panel from an agent". A feature that was only harness-tested has not been
verified; a feature with no surface gesture yet records that gap explicitly
(#184's shape) instead of skipping silently.

Refactor/doc commits with no observable behavior change are exempt; say
"no live test — behavior unchanged" in the commit message body.

### Gate 3: Clippy clean on touched crates
```bash
# Capture baseline, touch changed files, capture again, diff
cargo clippy --workspace --all-targets > /tmp/clippy-before.txt 2>&1
touch <changed files>
cargo clippy --workspace --all-targets > /tmp/clippy-after.txt 2>&1
diff /tmp/clippy-before.txt /tmp/clippy-after.txt
```
Zero new warnings on the crates you touched. Pre-existing warnings in untouched crates are not yours.

### Gate 4: Code review complete
A subagent code review must have run on this change (or this batch of changes) and returned findings. The review agent must have been given the commit diff, the relevant spec/ADR sections, and the git-safety guard (`Do not run any git command that mutates the working tree`).

- If the review found **blockers**: fix them and re-review before committing.
- If the review found **nits only**: fix them or document the deferral in the commit message.
- If the review found **nothing**: you're clear.

### Gate 5: Issue referenced
The commit message must reference the issue it resolves (`Fixes #N`, `Closes #N`, or `Refs #N`). If there is no issue, create one or explain why in the message.

### Gate 6: Design changes reviewed (if applicable)
If this commit changes a design document (ADR body, phase spec, protocol definition):
- A `design-review` must have run on it.
- The review findings must be addressed or deferred.
- The design doc must be append-only (ADR bodies, phase reports). New content goes before existing content; existing content is never rewritten.

### Gate 7: Working tree accounted for
```bash
git status --short
```
Every changed file must be either staged for the commit or explicitly accounted for (scratch file, gitignore, intentional deferral). No silent dirt.

## Waiver

Any gate may be waived with an explicit reason in the commit message body. Format:
```
Gate N waived: <concise reason>
```
Use sparingly. Gate 1 (tests green) should almost never be waived.
