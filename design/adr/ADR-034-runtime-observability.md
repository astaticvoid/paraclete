# ADR-034 — Runtime Observability (live dropout / xrun / drop counters)

**Status:** 🟡 **Proposed** — awaiting review. Implementation may proceed against the
decisions below; stop-and-ask if any decision proves wrong during implementation.

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

## Decision

Add a **lock-free runtime counter block** (`RuntimeCounters`) shared between the
audio callback and the executor, incremented on the audio thread with `Relaxed`
atomics (no allocation, no lock — safe in `process()`). Counter values are
published by the executor into the state bus as `/engine/*` paths each cycle and
surfaced to Antiphon clients through the existing state mirror at ≤ ~30 Hz.

### Counter fields (all `AtomicU64`)

| Counter | Path | Incremented when |
|---------|------|------------------|
| `buffers_processed` | `/engine/buffers_processed` | Every executor `process()` call |
| `dropout_lock_miss` | `/engine/dropout_lock_miss` | Audio callback `try_lock()` returns `Err` — contention with executor swap |
| `dropout_no_executor` | `/engine/dropout_no_executor` | Audio callback `guard.as_mut()` is `None` — no executor installed |
| `state_bus_overflows` | `/engine/state_bus_overflows` | `state_bus_producer.push()` returns `Err` — SPSC full |

### Resolved open questions

1. **Counter granularity — split into two dropout counters.** `dropout_lock_miss`
   indicates mutex contention (→ arc-swap trigger); `dropout_no_executor`
   indicates a startup/teardown race. Different root causes with different
   fixes; separate counters let us distinguish them.
2. **Reset semantics — monotonic since-boot.** The executor publishes absolute
   values each cycle. Clients diff. No windowing or rate computation on the
   engine side — leave that to the consumer.
3. **CPU load measurement — out of scope for this ADR.** A true CPU/xrun meter
   (per-buffer process-time deltas) is a follow-up. This ADR delivers *drops
   only* — the cheapest, highest-signal first step that unblocks D4.
4. **Structured-log channel — out of scope.** Per-node debug events are a
   separate concern; spin a new ADR/infra item when needed.
5. **`arc-swap` sequence — counter first.** Land the counters before any
   `Mutex` → `arc-swap` refactor. If the counter is zero after weeks of use,
   the refactor is de-prioritised; if non-zero, the counter justifies the work.
6. **Deferred-event-carry counter — deferred.** Low priority. The
   `saturating_sub` path is a recovery mechanism, not a silent drop (events are
   preserved); a counter would be informative but doesn't block D4. Add when
   the structured-log channel lands.
7. **LED-drop counter — deferred.** On the main thread (configurator), not the
   audio thread. First-drop logging already exists. Unify when the structured-log
   channel lands.

### Why not fold into ADR-033?

ADR-033 is the *offline/interactive* driver (render a WAV, REPL over a scenario).
This is *live, in-session, always-on* instrumentation with a different owner
(`AudioEngine` + `NodeExecutor`, not the test-driver) and a different consumer
(Antiphon clients / the CPU meter). Keeping them separate keeps ADR-033 focused
on offline testing.

### Ownership model

`RuntimeCounters` is an `Arc<RuntimeCounters>` shared between:

- **Audio callback closures** (for `dropout_lock_miss` / `dropout_no_executor`
  increments) — the callback captures `Arc::clone(&counters)`.
- **`NodeExecutor`** (for `buffers_processed` / `state_bus_overflows` increments
  and state-bus publishing) — stores the `Arc` as a field.
- **`AudioEngine`** (source of truth across executor rebuilds) — creates the
  `Arc` in `start()` and injects it into each new executor via
  `resume_with_executor()`. For the simple `AudioBackend::start()` path (no
  mutex, no swap), the executor creates its own default `Arc` internally.

Counter values flow to Antiphon clients through the existing state-bus pipeline:
executor `process()` writes `/engine/*` entries into `agg_state_buf` → SPSC →
`StateBusHandle` → `AntiphonHandle::pump()` → mirrored to clients.
The Antiphon mirror filter gains `|| path.starts_with("/engine/")`.

## Consequences

- **Positive:** turns the Deferred-Bug Backlog from assumed-quiet to measured
  (D4). Lock-free — no audio-thread cost beyond a `Relaxed` increment per
  `process()` call. Four `AtomicU64` fields (32 bytes total). No new allocation,
  no new channel, no protocol format change (paths are plain strings).
- **Negative / risk:** counter resets on `rebuild_executor()` if the
  `AudioEngine` injects a fresh `Arc`. This is acceptable — the "since-boot"
  contract is scoped to the current graph, and a rebuild is a deliberate
  user action. Monotonic counters survive within a single executor's lifetime.
  Schema churn: `/engine/*` paths are new but protocol v0 is pre-freeze.
- **Testability:** `dropout_lock_miss` is trivially unit-testable by holding the
  `Mutex` lock and calling `process()` on a buffer. `state_bus_overflows` is
  testable by filling the SPSC ring buffer and pushing one more entry. Neither
  needs the ADR-033 harness.

## Implementation sketch

1. **`RuntimeCounters` struct** — `paraclete-runtime/src/runtime_counters.rs`:
   `AtomicU64` × 4, `Default`, `Arc`-wrapped.
2. **`NodeExecutor`** — add `counters: Arc<RuntimeCounters>` field; accept in
   `build_executor()`; add `set_counters()` for `AudioEngine` injection.
3. **`AudioEngine`** — create `Arc<RuntimeCounters>` in `start()`; store as
   field; pass clone to callback closures in `AudioBackend::start_with_callback()`.
4. **Callback increments** — `audio.rs:159,165,190,193`: increment
   `dropout_lock_miss` on `try_lock` `Err`, increment `dropout_no_executor`
   on `else { fill(0.0) }`.
5. **Executor increments** — `executor.rs`: `buffers_processed` at top of
   `process()`; `state_bus_overflows` when `push` returns `Err`.
6. **State-bus publishing** — executor writes all four counter values into
   `agg_state_buf` as `/engine/*` entries each cycle.
7. **Antiphon mirror** — add `path.starts_with("/engine/")` to
   `is_mirrored_state_path` filter in `paraclete-antiphon/src/lib.rs`.
8. **Tests** — unit test holding `Mutex` lock + calling callback → assert
   `dropout_lock_miss > 0`; unit test filling SPSC → assert
   `state_bus_overflows > 0`.
9. **Docs** — update `bugs.md` INFRA-003 → RESOLVED; update `roadmap.md`
   D3/D4 → done.

(End of file)
