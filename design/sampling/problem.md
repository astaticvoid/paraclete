# Anamnesis (Sampling Layer) — Problem Statement

> **Stage 1 of a staged design** (the Theotokos track pattern). This file
> states the problem; the experience design grows aspect-by-aspect in
> `design/sampling/design.md`; the architecture is ADR-040 (proposed).
> Phase specs (`design/phases/anN-*.md`) are written when a phase is next
> to start.
>
> **Authored:** 2026-07-23. **Status:** drafted at user request
> ("modern like Digitakt, but not lose any perf features of Octatrack");
> awaiting acceptance with ADR-040 ratification.
>
> Third-party marks appear per house naming policy: design prose only,
> never identifiers or UI strings. **Anamnesis** (ἀνάμνησις,
> "remembrance") continues the liturgical register — the layer that
> remembers sound.

---

## The gap

The Elektron-convergence directive (ADR-038) made the missing half of
the instrument explicit: Theotokos's SAMPLING button is **gated on a
sampling capability that no part of the system declares**. What exists
is playback only:

- **`Sampler` node** — wav loaded at activate, Hermite pitch resampling,
  `start`/`end`/`loop`/`root_note` params, and a `slice` param backed by
  a slice table that holds exactly **one full-sample slice** (P3
  scaffolding, never populated further).
- **No audio input path at all.** The HAL opens output streams only;
  nothing in the codebase can hear the outside world — or the
  instrument itself (no resampling tap).
- **No sample pool.** Each sampler owns its own file; samples are not
  project-level assets; a sampler cannot change samples at runtime, so
  Digitakt-class **sample locks** (a different sample per step) are
  impossible.
- **No capture, no trim, no slicing UX, no timestretch, no recorder
  buffers, no scenes/crossfader, no live looping.**

## The problem

**Paraclete cannot sample.** More precisely: it lacks the entire
capture-to-performance loop that defines a modern sampling groovebox —
*hear → capture → shape → assign → perform* — and it lacks the
performance layer that makes sampled material an instrument rather than
a playback library. Two references frame the bar (design-prose analogy
only):

- **Digitakt (the modern baseline):** one-screen sampling (source, arm,
  threshold, monitor), a project sample pool with hot assignment,
  per-step sample locks, immediate trim/preview.
- **Octatrack (the performance ceiling):** nothing it can do live may be
  lost — always-on track recorders with retroactive capture ("sample
  the last two bars of the master, now"), one-gesture resampling
  transitions, slices with per-step slice locks, scene A/B morphing on
  a crossfader, pickup-machine live looping, BPM-locked loop playback.

The convergence goal is the union: **Digitakt's directness with
Octatrack's depth**, expressed through Paraclete's existing planes
(cap-docs/`Rule`, ADR-019 commands, state bus, kits/ADR-039) so every
surface — Theotokos, Theoria, hardware — inherits it.

## Requirements

1. **Capture is one gesture away, always.** External input, master, or
   any track tap; threshold-armed or manual; and *retroactive* — the
   recorder is always listening, so "capture what just played" is a
   chord, not a setup.
2. **Samples are project assets, not node property.** A pool with file
   slots and RAM (captured) slots; samplers reference pool slots by a
   stepped, p-lockable param — sample locks per step.
3. **Slices are first-class and lockable.** Grid slicing at minimum;
   the existing `slice` param becomes real; slice-per-step via the CMD
   33–35 lock family; sample chains fall out of this.
4. **Scenes and a crossfader.** Two kit-shaped partial snapshots morphed
   by a continuous control, instrument-wide, from any surface. The
   single most identity-defining Octatrack performance feature.
5. **Live looping** (pickup-style): record a loop synced to pattern
   length, overdub, play — without stopping.
6. **BPM-locked loop playback** is designed-in (play modes: repitch now,
   stretch staged later) — the param surface must not change when the
   stretch DSP lands.
7. **All planes, no side doors.** Params via ADR-019 + `Rule` pages
   (SRC page discipline); capture/pool/scene orchestration via the
   ADR-039 app-op drain; sample data delivery via a real-time-safe
   asset plane. Zero allocation/blocking in `process()`.
8. **Session-gated iteration** (ADR-036 protocol): no gesture grammar
   is frozen without hands-on evidence.

## Non-goals

- **Not a wave editor.** Trim/normalize/slice-grid yes; destructive DSP
  editing stays on glass (Theoria) or out of scope.
- **Not disk streaming.** All playback is RAM-resident (the modern
  assumption); streaming is revisited only if pool sizes demand it.
- **Not a looper pedalboard.** Pickup-style looping serves the
  groovebox workflow (pattern-synced), not open-ended looper stacking.
- **Not P12.** Retrig/slide grammars touch sampled material but belong
  to the sequencer phases; this track designs the material they act on.

## Success criteria

- Sample a phrase from the master while a pattern plays, trim nothing,
  and have it playing sliced on a track **within one pattern cycle** —
  the transition trick, end to end, from Theotokos alone.
- A different sample on step 13 than on step 1 of the same track
  (sample lock), and a different slice on every pad of a 16-slice break.
- A scene crossfade that morphs filter + pitch + volume across all
  tracks smoothly at any tempo, and snaps back on release.
- Loop a live overdub against a playing pattern without the transport
  hiccupping.
- The SAMPLING screen (ADR-038, key `9`) goes from capability-gated
  stub to the capture home without any Theotokos-specific engine path.
