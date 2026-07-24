# ADR-040: Anamnesis — The Sampling Layer

| Field | Value |
|-------|-------|
| **Status** | 🟡 Proposed (2026-07-23) |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Pending user |
| **Scope** | New AN track (AN0–AN3): `paraclete-hal` (input), new nodes (`AudioInputNode`, `RecorderNode`), `Sampler`, app layer (pool, scenes), project format |
| **Related** | ADR-038 (SAMPLING/KIT screens, capability gate), ADR-039 (KitStore, app-op drain — scenes extend it), ADR-029 (recorder taps are graph edges), ADR-025 (project format), ADR-019/ADR-032 (param plane, `Rule`); `design/sampling/{problem,design}.md` |

> Third-party names (Elektron, Digitakt, Octatrack) are trademarks of
> their respective owners, used nominatively for design comparison only;
> Paraclete is unaffiliated. Marks never appear in identifiers or UI.

---

## Decision

**A new design track — Anamnesis (AN0–AN3) — gives Paraclete the full
capture-to-performance sampling loop**: Digitakt-class directness
(one-screen sampling, pool, sample locks) with the Octatrack performance
ceiling preserved (always-on recorders, retroactive capture, slices,
scenes/crossfader, pickup looping, BPM-locked loops). Experience design
lives in `design/sampling/design.md`; this ADR fixes the architecture.

Seven decisions:

1. **Audio input enters through the HAL and a node.** `paraclete-hal`
   gains an input stream (same device layer as output); samples cross to
   the audio graph via SPSC ring into a new **`AudioInputNode`** (L3),
   which is an ordinary signal source in the graph. Monitoring is graph
   wiring (input → mix), not a special path.

2. **`RecorderNode` — always-on ring recorders.** A recorder is a node
   with one signal input and no output obligation; wired by edges to
   whatever it should hear (external input, mix bus, any track tap —
   ADR-029 topology, the resampling path for free). It writes a
   **pre-allocated ring** continuously (real-time-safe, no alloc/lock in
   `process()`). Capture is **retroactive by construction**: a capture
   command marks a region (last N bars, threshold-with-pre-roll, or
   armed span); the **app** drains the marked region from the ring on
   the main thread and assembles the sample. Ring length is fixed at
   build (OQ-A1).

3. **The sample pool is app-owned; samplers reference it.**
   `SamplePool`: ordered slots `{file | ram-capture}` with metadata
   (name, frames, rate, `bars`, gain, slice table), persisted in the
   project RON (new sections; ADR-025 format bump). The `Sampler`'s
   per-file loading is replaced by a stepped **`sample` param indexing
   the pool** — p-lockable, so **per-step sample locks ride the
   existing CMD 33–35 lock family with zero new lock machinery.**

4. **Sample data reaches nodes over an asset plane, not the command
   plane.** `NodeCommand` stays fixed-width; sample payloads travel as
   `Arc<SampleData>` in a new config-path message
   (`SwapSample { node_id, pool_slot, data }`), swapped in at block
   boundary; the displaced `Arc` is returned to the main thread for
   drop (no audio-thread dealloc). Decode/resample happens on the main
   thread at pool-load time (the existing `load_wav` path, hoisted).

5. **Slices are pool metadata, first-class.** Slice tables live on the
   pool entry (grid slicing at minimum; per-sample, shared by all
   referencing samplers). The `Sampler`'s existing `slice` param
   becomes real; sample chains are just sliced long samples.

6. **Scenes are a KitStore extension; the crossfader is an app-side
   morph.** A scene = a *partial* kit (only assigned params). The
   crossfader is one continuous value (`/context/xfade`, any surface);
   the app interpolates scene A→B each main-loop tick and replays
   `CMD_SET_PARAM` (nodes' own smoothing declicks). Instrument-wide and
   engine-agnostic — the same reasoning that made kits app-owned
   (ADR-039: nodes cannot command nodes). Scene ops are app-ops.

7. **Timestretch is staged surface-first.** The SRC page ships
   `play_mode` (repitch · stretch · loop · oneshot · reverse) and
   `bars` metadata **now**; repitch (existing Hermite) is the only DSP
   until AN3 lands a stretch algorithm (OQ-A8). The performer-visible
   contract (BPM-locked loops) is frozen before the DSP exists so no
   surface changes when it arrives.

**Phases (session-gated, ADR-036 protocol):**

| Phase | Ships | Depends on |
|---|---|---|
| **AN0** | Pool + asset plane + `sample` param/locks + SRC page reshape (no capture) | — |
| **AN1** | HAL input, `AudioInputNode`, `RecorderNode`, capture app-ops, SAMPLING screen (manual/threshold/retro), transition-trick chord | AN0 |
| **AN2** | Slices + chains + scenes/crossfader | AN0; **P11 KitStore shipped** (scenes) |
| **AN3** | Pickup-style looping, stretch DSP, transition polish | AN1, AN2 |

The ADR-038 SAMPLING capability gate is satisfied by the app declaring
the sampling capability when an input/recorder is configured (cap-doc
level, so Digitone/Syntakt-alike instruments without recorders simply
never show the screen).

## Alternatives considered

- **Keep samples node-owned (status quo) and add a recording node
  writing files** — rejected: no sample locks, no hot assignment, no
  shared slices; the pool is what makes sampled material *performable*.
- **Capture assembled on the audio thread into the pool** — rejected:
  allocation and large copies in `process()`; the ring-drain split puts
  assembly on the main thread with ~ms latency, inaudible for capture.
- **Scenes as an engine-side morph node** — rejected: a node cannot
  write other nodes' params (ADR-018); and app-side morph at ~1 kHz
  through declared smoothing is already transparent.
- **Disk streaming (Octatrack Static machines)** — explicit non-goal;
  all-RAM is the modern baseline. Revisit only on real pool pressure.
- **A dedicated sampling protocol/surface** — rejected: SAMPLING screen
  (Theotokos), Theoria sample manager, and profiles all consume the
  same pool/app-ops/`Rule` planes; no side doors.

## Consequences

- **New:** AN track docs (`design/sampling/`), `AudioInputNode`,
  `RecorderNode`, `SamplePool` + project sections, `SwapSample` asset
  message, `/context/xfade`, scene entries in the KitStore, sampling
  capability declaration; phase specs `design/phases/anN-*.md`.
- **Changed:** `Sampler` (pool-indexed `sample` param, slice table from
  pool, `play_mode`), HAL (input streams), SRC-page `Rule` for the
  sampler (8-param encoder-bank shape, ADR-038).
- **Preserved:** every Octatrack performance capability has a named home
  (`design.md` §2 checklist) — scenes, retro-capture, slices+locks,
  pickup loops, BPM-locked loops; parts and neighbor-chains were
  already covered (ADR-039 kits; graph topology).
- **Ordering:** design accepted now; implementation slots after TK2 per
  the R3 precedent — AN0 has no TK2/P11 dependency, AN2 needs P11's
  KitStore. Exact scheduling is a user call at TK2 exit.
- **Open:** OQ-A1–A8 in `design.md` (ring budget, RAM-capture
  persistence, pre-roll, crossfader feel, transient slicing, LFO
  designer, cue outs, stretch algorithm).

## Implementation note (to be added when ratified)

```text
ADR-040 implemented across AN0–AN3 (YYYY-MM-DD …).
See design/phases/anN-*.md for the commit plans.
```
