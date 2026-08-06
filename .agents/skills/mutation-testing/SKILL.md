---
name: mutation-testing
description: Methodology for verifying tests actually catch defects. Use when writing new tests, trusting a test suite, or checking if a baseline scenario provides real coverage.
---

# Mutation Testing

Tests are routinely mutation-checked before being trusted: apply one deliberate defect, confirm a named test fails, revert.

## Three ways the harness silently lies

All three have happened in this project.

### 1. Restoring with `cp`/`mv` gives the backup's older mtime

Cargo compares mtimes, decides the crate is unchanged, and reruns the **mutant's** binary. A green run afterwards proves nothing. **`touch` after every write *and* every restore.**

### 2. A mutant can hang instead of failing

An iterator that stops advancing makes a test collect forever. Wrap every `cargo test` / `cargo run` in `timeout`; treat exit 124 as a *killed* mutant.

### 3. `cargo test` prints `error: test failed` on ordinary assertion failure

A `grep -E "^error"` compile-check misreads killed mutants as "did not compile". Match `^error\[|could not compile` instead.

## The converse trap

A baseline proves *stability*, not *behaviour*. It fingerprints whatever the scenario does, so a scenario that does nothing fingerprints cleanly and passes forever.

Before trusting a baseline as coverage for a behaviour, check the scenario actually exercises it: perturb the thing under test and confirm the fingerprint moves. A baseline you have never seen fail is not evidence.

## Two perturbations, not one

Removing the feature moved the fingerprint — that only proves the scenario does *something* the baseline notices. Reinstating the **defect** and confirming it also moves is what proves the baseline covers the bug.

The first check passes on a scenario that merely exercises a code path; the second is the one that catches the actual bug. Do both, and write down that you did — in the scenario header, where the next agent reads it.
