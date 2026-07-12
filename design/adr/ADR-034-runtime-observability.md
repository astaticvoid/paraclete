# ADR-034 — Runtime Observability (live dropout / xrun / drop counters)

**Status:** 🚧 **WIP — DRAFT, NOT ACCEPTED.** Scaffolded 2026-07-12 in an
exhaustion-mode session; seeded from INFRA-003. A fresh operator should finish
the sections marked **TODO** and then move Status to `proposed`, then `accepted`
after review. Do not implement against this until it is at least `proposed`.

**Date:** 2026-07-12
**Relates to:** INFRA-003 (bugs.md), roadmap Design Triage **D3/D4**, ADR-033
(offline/interactive driver — this ADR is the *live* complement), audio-model
review ("xrun/CPU meter to `/engine/cpu`", "Executor cell `Mutex` → `arc-swap`").

## Context (established — INFRA-003)

The engine recovers from several conditions by producing silence or dropping
data, and records nothing, so the trigger-based backlog is *assumed* quiet, not
measured:

- `crates/paraclete-hal/src/audio.rs:159–167` / `188–196` — audio callback:
  `try_lock` miss on the executor-swap `Mutex` → `data.fill(0.0)` (audible
  dropout); inner "no executor installed" → `fill(0.0)`.
- `crates/paraclete-runtime/src/executor.rs:588` — `let _ = state_bus_producer.push(…)`
  (SPSC full → state update silently dropped).
- `crates/paraclete-runtime/src/executor.rs:400` — deferred-event carry
  (`saturating_sub`); no visibility into how often events don't fit a block.
- `crates/paraclete-runtime/src/configurator.rs:607` — LED delivery: logs the
  *first* drop per device, then silent.

cpal *device*-level xruns are surfaced via `err_fn` (`audio.rs:48,139`); our own
self-inflicted drops are not. This blocks confirming the whole Deferred-Bug
Backlog (D4).

## Decision (PROPOSED — refine)

Add a **lock-free runtime counter block** owned by `AudioEngine`, incremented on
the audio thread with `Relaxed` atomics (no allocation, no lock — safe in
`process()`), read by the main thread and surfaced on the Antiphon `/engine/cpu`
state path.

Proposed counters (all `AtomicU64`):
- `buffers_processed` — denominator for rates.
- `dropout_buffers` — incremented on every audio-callback `fill(0.0)` recovery
  (both the `try_lock` miss and the missing-executor paths; consider separate
  counters so they're distinguishable — **TODO: decide 1 vs 2 counters**).
- `state_bus_overflows` — incremented at `executor.rs:588` when `push` returns
  `Err`.
- **TODO:** deferred-event-carry counter? LED-drop counter unification?

Exposure: a new `ServerMsg`/state entry under `/engine/cpu` (protocol v0 is
pre-freeze, so adding a field is free — confirm with `protocol.rs`). Coalesce at
the existing state-mirror rate (≤ ~30 Hz); never per-cycle.

### Why not fold into ADR-033?

ADR-033 is the *offline/interactive* driver (render a WAV, REPL over a scenario).
This is *live, in-session, always-on* instrumentation with a different owner
(`AudioEngine`, not the test-driver) and a different consumer (Antiphon clients /
the CPU meter). Keeping them separate keeps ADR-033 focused. **TODO: confirm
this framing with the reviewer; the alternative is one "observability" ADR
covering both.**

## Open questions for the fresh operator (TODO)

1. **Counter granularity** — one `dropout_buffers` or split lock-contention vs
   missing-executor? (Different root causes: contention → arc-swap fix; missing
   executor → startup/teardown race.)
2. **Reset semantics** — monotonic since-boot, or windowed/rate? Recommend
   monotonic + let the client diff.
3. **CPU load measurement** — this ADR covers *drops*; a true CPU/xrun meter also
   wants per-buffer process-time (e.g. `Instant` deltas). Is that in-scope here
   or a follow-up? The audio-model review lists "xrun/CPU meter" as one item —
   decide whether D3 delivers drops-only first (cheap, unblocks D4) or the full
   meter.
4. **Structured-log channel (roadmap Agent Infra Gaps)** — is the debug-mode
   per-node event log part of this ADR or its own? Likely its own; note the
   boundary.
5. **`arc-swap` interaction** — if the executor-swap `Mutex` becomes `arc-swap`
   (backlog trigger), the `try_lock`-miss dropout path disappears. Sequence:
   does the counter land first (to *prove* the contention is real before the
   refactor), or does arc-swap land first? Recommend: **counter first** — it is
   the evidence that justifies the refactor.

## Consequences (TODO — fill after decisions)

- Positive: turns the Deferred-Bug Backlog from assumed-quiet to measured (D4).
  Lock-free, no audio-thread cost beyond a `Relaxed` increment.
- Negative / risk: **TODO** (counter-reset races on rebuild? `/engine/cpu`
  schema churn before protocol freeze?).

## Implementation sketch (for whoever picks this up)

1. `AudioEngine`: add the atomic block; expose `&AtomicU64` refs or a snapshot
   getter to the main thread.
2. Increment at the four sites above (start with the two audio-callback paths +
   state-bus overflow — highest signal).
3. Add the `/engine/cpu` state entry; wire it through Antiphon's state mirror.
4. Test: unit-drive a `try_lock` contention (hold the cell, run one callback) and
   assert `dropout_buffers` incremented; assert `state_bus_overflows` increments
   when the SPSC is filled. (No harness needed — same style as the
   `remove_node_on_drained_slot` unit test added 2026-07-12.)
5. Update bugs.md INFRA-003 → RESOLVED and roadmap D3/D4.
