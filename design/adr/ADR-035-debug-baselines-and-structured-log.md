# ADR-035 — Regression Baselines & Structured Debug Log

**Status:** ✅ **Accepted — Part A implemented (2026-07-13, `b74b853`); Part B implemented (2026-07-13)**

**Date:** 2026-07-13

**Date:** 2026-07-13
**Relates to:** roadmap Rank 2 (debug-posture push; the two remaining
harness-built items), ADR-033 (headless driver — capture ring, `analysis.rs`,
interactive REPL), ADR-034 (runtime observability — this ADR is its per-*event*
complement; ADR-034 §60 explicitly deferred the structured-log channel and §57
the CPU meter to "a separate ADR / follow-up"), `design/review/audio-model-review.md`.

## Context

The interactive debug harness shipped (ADR-033, `92b8795`). Two Rank 2 gaps
remain that **build on it**, plus one adjacent observability follow-up:

1. **Audio diff/snapshot baselines.** We can render a scenario to WAV and assert
   on peak/artifact metrics, but there is no stored *baseline* to diff a later
   render against. A refactor that halves a decay, drops a voice to silence, or
   introduces DC has no automated tripwire — the roadmap "Audio diff/snapshot"
   gap ("no peak/RMS baselines for refactoring").

2. **Structured per-node debug log.** The state bus is a per-cycle *snapshot*
   (last-value-wins per path): you can `read /node/10/state/current_step` and
   see `3`, but you cannot see *"step 3 fired at sample-offset N with note 36."*
   Discrete events — step fires, voice triggers, param changes — have no channel.
   The roadmap "Structured log channel" gap; ADR-034 §60 punted it here.

3. **CPU-% meter** (adjacent). ADR-034 delivered drop counters but explicitly
   scoped out a process-time meter (§57). Small; see **§CPU meter** — kept as an
   ADR-034-lineage follow-up, not a Part of this ADR's core.

## Decision — overview

- **Part A:** add a **baseline fingerprint** (scalar summary + windowed-RMS
  envelope, *not* raw samples) written beside each scenario and diffed on a
  `--check-baseline` run, tolerance-gated. Reuses ADR-033's capture buffer and
  `analysis.rs`.
- **Part B:** add a **fixed-size, allocation-free debug-event record** emitted by
  nodes through an *additive* `ProcessOutput` sink (no `Node`-trait method-signature
  change), drained by the executor exactly like `published_state()`, forwarded to
  the main thread over a dedicated SPSC, and polled by the test-driver.

---

## Part A — Audio regression baselines

### A.1 Fingerprint, not raw samples

Raw-sample equality is the wrong contract: any intended DSP change (a filter
coefficient, an oversampling tweak) rewrites every sample and would fail the
check, training us to `--update` blindly. The baseline is a **derived
fingerprint** robust to intended change but sensitive to regressions:

| Field | Source (`analysis.rs` / capture) | Catches |
|-------|----------------------------------|---------|
| `sample_count` | capture length | truncated/over-long render |
| `peak` | existing peak scan | clip / silence / level shift |
| `rms` | new: √mean(x²) over whole capture | overall energy drift |
| `dc_offset` | `analysis::dc_offset` | DC creep |
| `non_finite` | `analysis::non_finite` | NaN/Inf regressions (must stay 0) |
| `rms_envelope[]` | new: RMS per 50 ms window | *where* it changed — decay, timing, a voice dropping out mid-render |

The envelope is the key add: a scalar RMS hides a kick whose decay doubled but
whose total energy is similar; a 50 ms envelope shows the shape.

### A.2 Storage & workflow

- Baseline file: `<scenario>.baseline.json` beside the scenario YAML, committed.
- `test-driver <scenario> --update-baseline` renders and (over)writes the baseline.
- `test-driver <scenario> --check-baseline` renders, compares within tolerance,
  prints the worst-diverging metric/window, exits **1** on drift (same code as an
  assertion failure → CI-usable). No baseline present under `--check-baseline`
  is an error, not a silent pass.
- Absent either flag, batch mode is unchanged.

### A.3 Tolerances

Per-metric **relative** epsilon stored in the baseline (overridable in the
scenario): default `peak`/`rms` within 1 %, envelope windows within 1.5 %,
`dc_offset` absolute `< 1e-3`, `non_finite` must equal 0, `sample_count` within
±1 block. Tolerances live in the file so a deliberately-loosened scenario is
reviewable in the diff.

### A.4 Determinism caveat

Debug-build capture runs ~25 % slow (AGENTS.md); the *fingerprint* is over
captured samples, not wall-clock, so it is timing-stable **as long as the
scenario is deterministic**. Randomised conditions (probability locks) must be
seeded or excluded from baselined scenarios — noted as a scenario-authoring rule,
enforced later if it bites.

---

## Part B — Structured debug-log channel

### B.1 Event record (POD, `Copy`, no heap)

```rust
#[derive(Clone, Copy)]
pub struct DebugEvent {
    pub sample_offset: u32,  // within the current block — sub-cycle timing
    pub node_id: u32,
    pub kind: DebugEventKind, // repr(u16) enum
    pub arg0: i64,           // note / step / param_id
    pub arg1: f64,           // velocity / value
}
```

No `String` on the audio thread (mirrors the `NodeCommand` shape). Path/name
resolution happens on the main thread at drain time.

### B.2 Emission path — additive `ProcessOutput` sink (the key decision)

The `Node` trait already hands each node `&mut ProcessOutput` in
`process(&mut self, input, output)` (`node.rs:29`). Add an **optional debug sink
to `ProcessOutput`** — a `&mut Vec<DebugEvent>` (or `None` when logging is off):

```rust
// node side — no trait method-signature change:
output.emit_debug(DebugEventKind::VoiceTrigger, note, velocity);
// compiles to: if let Some(buf) = &mut self.debug { buf.push(...) }  // one branch
```

Why this over the alternatives:
- **vs. changing `process()`'s signature** — that edits the L2 method contract
  (the LGPL3 third-party boundary); rejected. An *additive field* on the
  `ProcessOutput` struct leaves the method signature and every existing node
  untouched — non-emitting nodes never call `emit_debug` and pay nothing.
- **vs. a thread-local sink** — works with zero L2 change but hides control flow;
  the explicit `output.emit_debug` reads like `published_state` and is greppable.
- **vs. the state bus** — last-value-wins per path can't represent two step-fires
  in one cycle or sub-cycle offsets. Wrong data model.

**Audio-safety:** the `Vec<DebugEvent>` is executor-owned, pre-allocated, and
`clear()`ed (capacity retained) before each `process()` — the *exact* pattern
`published_state()` already uses (`executor.rs:75` "capacity is retained across
cycles", drained at `:592`). No allocation, no lock in `process()`.

### B.3 Drain path

Post-`process()`, the executor drains the per-node event buffer (beside the
existing `published_state` drain at `executor.rs:592`) and pushes records onto a
dedicated **`debug_event` SPSC** (parallel to `state_bus_producer`). The main
thread drains it in `configurator.process_main_thread()`. SPSC-full increments a
counter and drops (reuse ADR-034's overflow-counter discipline — never block the
audio thread).

### B.4 Gating

Logging is **opt-in** via an executor `AtomicBool debug_log_enabled`. Default app
runs: the executor skips passing the sink into `ProcessOutput`, so `emit_debug`
sees `None` — one branch, no work. The test-driver sets it on. This keeps the
shipping binary at zero cost and satisfies the "JSON never touches the audio
thread" constraint (records are POD; JSON formatting is main-thread, at drain).

### B.5 Event kinds (v1)

`StepFired { step, note }`, `VoiceTrigger { note, velocity }`,
`ParamChange { param_id, value }`. Enum is `#[non_exhaustive]`-friendly; add kinds
without breaking consumers.

### B.6 Interactive surface (ADR-033 stays poll-based)

New REPL command, consistent with ADR-033's poll-not-push model:

```jsonc
{"cmd":"log"}
// → {"events":[{"t":12034,"node":20,"kind":"voice_trigger","arg0":36,"arg1":1.0}, ...]}
```

Returns events buffered since the last `log` poll (main-thread ring, drained on
read). Batch mode may also dump the event log to stderr at end-of-run for
scenario debugging.

---

## CPU meter (scoped out — ADR-034 follow-up)

Not part of this ADR's core; recorded here so the third Rank 2 item has a home.
Per `audio-model-review.md:64` and ADR-034 §57: an **EWMA of audio-callback
process time** pushed to `/engine/cpu` via the existing state-bus `/engine/*`
mechanism — cheap, main-thread-published, no new machinery. It belongs to the
ADR-034 counter family, not the harness-built features here. Implement as a small
ADR-034 addendum when picked up.

---

## Resolved decisions

1. **Fingerprint over raw samples** (A.1) — refactor-tolerant, regression-sensitive.
2. **Envelope included** (A.1) — locates *where* audio changed, not just *that* it did.
3. **Additive `ProcessOutput` sink** (B.2) — no `Node`-trait method-signature change.
4. **Reuse the `published_state` drain pattern** (B.3) — proven audio-safe, no new
   allocation discipline to invent.
5. **Opt-in, zero-cost-when-off** (B.4) — shipping binary unaffected.
6. **Poll, not push** (B.6) — consistent with ADR-033's resolved threading model.
7. **CPU meter stays ADR-034 lineage** — keeps each ADR cohesive.

## Open questions for review

1. **L2 touch on `ProcessOutput`.** Adding `debug: Option<&mut Vec<DebugEvent>>`
   (or a sink handle) to the L2 `ProcessOutput` struct is additive and
   method-signature-preserving, but it *is* the LGPL3 third-party boundary and a
   freeze-sensitive surface (universality directive). Sign-off needed: is an
   additive `ProcessOutput` field acceptable pre-freeze, or should v1 route debug
   events a non-L2 way (executor-side interception of the events it already
   delivers — step/trigger — covering the common cases without any node opt-in)?
   *Recommendation:* accept the additive field; it is the honest, extensible path
   and third-party nodes are unaffected.
2. **Baseline scope for v1.** Ship Part A first (self-contained, no L2 question),
   land Part B after the L2 sign-off? *Recommendation:* yes — A is the faster win
   and unblocks refactor confidence immediately.
3. **Baseline file format** — JSON (chosen for the envelope array + tolerance
   readability) vs RON (project convention). *Recommendation:* JSON; it is a
   test-tool artifact, not a project file, and diffs cleanly.

## Alternatives considered

- **`insta` for audio** — insta is text/serialization snapshots; audio needs
  tolerance-gated numeric comparison and a domain fingerprint. (Separately,
  SPIKE-005 weighs `insta` for the *TUI*.)
- **Full-spectrum/FFT fingerprint** — higher fidelity, but an FFT dependency and
  bin-tolerance tuning for marginal gain over a windowed-RMS envelope at this
  stage. Revisit if the envelope misses a real regression.
- **Logging via the state bus** — rejected (B.2): wrong data model for discrete,
  sub-cycle, possibly-repeated events.

## Implementation note (2026-07-13)

Part A shipped in `b74b853`. The baseline workflow is:

- `test-driver <scenario> --update-baseline` renders and writes `<scenario>.baseline.json`.
- `test-driver <scenario> --check-baseline` renders, compares within per-metric
  tolerances (peak/rms 1%, envelope windows 1.5%, dc_offset < 1e-3 absolute,
  non_finite = 0, sample_count ±1 block), exits 1 on drift.
- Absent either flag, batch mode is unchanged.

Key implementation finding: the threaded batch render jitters by a block
run-to-run (trigger lands in different blocks), which makes the per-window
envelope unstable. Baseline runs therefore use a new deterministic
single-threaded render (`render_deterministic`): step the executor block-by-block,
applying each action at its sample-accurate block boundary — bit-identical every
run. `dispatch_action` now takes `&mut NodeConfigurator` so both paths share it.

The first real baseline (`kick_reverb_clean.baseline.json`) is committed as a
regression fixture. Verified: 3 identical passing checks confirm determinism;
a velocity 1.0→0.3 regression trips peak/rms/dc + envelope windows 1-3.

## Implementation note — Part B (2026-07-13)

Structured debug-log channel shipped. The design matched the ADR with two minor
deviations:

1. **`emit_debug` includes `sample_offset`.** The node provides it (only the node
   knows sub-cycle timing). The executor fills `node_id` post-`process()` via a
   drain loop that copies events from per-node buffers into an aggregate SPSC
   buffer using `DebugEvent { node_id, ..ev }` struct update syntax.

2. **`ProcessOutput::new()` constructor.** A `ProcessOutput::new(audio_outputs,
   signal_outputs, events_out)` convenience constructor with `debug: None` was
   added so all existing construction sites (26) use one function. Only the
   executor uses the struct literal (to pass `debug: Some(buf)`).

Infrastructure:
- `DebugEvent` / `DebugEventKind` in `paraclete-node-api/src/debug.rs` (L2)
- `ProcessOutput.debug: Option<&mut Vec<DebugEvent>>` + `emit_debug()` in L2
- Per-node pre-allocated `debug_bufs` (capacity 64), `agg_debug_buf` (256),
  `debug_event_producer: SPSC<DebugEvent>` (cap 1024), `debug_log_enabled:
  AtomicBool` in `NodeExecutor` (L1)
- `debug_event_consumer`, drain in `process_main_thread()`, `debug_events()`
  accessor in `NodeConfigurator` (L1)
- `log` REPL command + batch-mode dump in test-driver
- Debug logging enabled on executor at test-driver `build_executor()` time;
  disabled by default in the shipping app (zero-cost: the `debug_log_enabled`
  path skips buffer clear/pass/drain when false)

Three node types emit debug events:
- `Sequencer::emit_note_on_at` — `StepFired { step_idx, note }`
- `AnalogEngine::process` — `VoiceTrigger { note, velocity }` (both CMD_TRIGGER and NoteOn paths)
- `FmEngine::process` — same as AnalogEngine

Verified via `test-driver --trigger kick` — a kick trigger produces a
`VoiceTrigger { note=36, velocity=0.79 }` event. Sequencer mode verified via
a 3-second scenario with a toggled step: `StepFired { step=0, note=36 }`
emitted at sample 355, followed by the downstream `VoiceTrigger`.
