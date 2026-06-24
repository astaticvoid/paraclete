# Paraclete — Roadmap

> **Living document.** Replace this file when a phase completes or significant
> planning changes occur. Keep it short — current state only.
>
> **Last updated:** June 2026
> **Current phase:** P10 in design (Pattern Engine); P9.5 (Device Emulation &
> Test Harness) is an enabling prerequisite for testing P10's surface features

---

## Roadmap

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0** | Skeleton | Empty runtime, one node, one clock tick | **Complete** |
| **P1** | First Sound | Sine oscillator triggered by Launchpad emulator | **Complete** |
| **P2** | Sequencer v1 | 16-step sequencer, clock domain, LED feedback | **Complete** |
| **P3** | First Instrument | Sampler, parameter locks, StateBus SPSC, Rhai bindings | **Complete** |
| **P4** | Hardware Integration | 3-device setup live. Launchpad = 8-track drum machine. XL = contextual params. Keystep = melodic. DistortionNode + FilterNode. | **Complete** |
| **P5** | Depth | Sequencer v2 (conditional trigs, micro-timing, swing, fill). ReverbNode, DelayNode, SplitNode, MixNode. | **Complete** |
| **P6** | Synthesis | FM synth, analog-style synth. High-quality resampling. Broader audio format support. | **Complete** |
| **P6.5** | Cleanup | EnvelopeNode looping AD fix. LadderFilter param rename. Signal port executor allocation. | **Complete** |
| **P7** | DAW Integration | CLAP plugin mode. DAW transport sync. Project save/recall. paraclete-node-api → crates.io. Per-voice rubato. | **Complete** |
| **P8** | Instrument Layer | Instrument definition file (YAML). Terminal UI. CLAP Host. | **Complete** |
| **P9** | Modular Graph | Patchable primitive node graph. Single-sample feedback. Sequencer as CV source. GraphNode / nested executor. | **Complete** |
| **P9.5** | **Device Emulation & Test Harness** | **Full Launchpad emulator (all 64 pads + scene + control row, mode switching, RGB) — the priority. Encoder testing via the real Digitakt (endless encoders) as stopgap. Computer-keyboard-as-piano (Keystep role). Scripted/headless device-input injection for integration tests. Enables P10/P11 grid features to be tested without a physical Launchpad.** | **In design** |
| **P10** | **Pattern Engine** | **Multi-pattern + seamless switching + chaining. Multi-page (64-step) patterns + page-loop selection. Per-track length & speed (polyrhythm). Serialization of all sequencer state. Fixes BUG-005/008/001/004 (sequencer + timing).** | **In design** |
| **P10.5** | Cleanup | Infrastructure bugs not in P10's blast radius: BUG-002 (agreement baseline), BUG-003 (hal→runtime layer violation), BUG-006 (agg_state_buf realloc), BUG-007 (publish_bank_state alloc). Mirrors the P6.5 precedent. | — |
| **P11** | Live Performance | Mute system (pattern + global + prepared). Temporary save / reload. Perform Kit mode. Live record from Keystep. | — |
| **P12** | Groove & Generation | Retrig. Euclidean mode. Controlled randomness polish. Generative Rhai fills. | — |
| **P13** | Analog Voice | Full subtractive mono synth voice (Pro-One-style): dual oscillators + sync + noise, self-oscillating 4-pole filter, two envelopes, LFO, modulation matrix, glide, arpeggiator. Paraphonic voice allocation. | — |
| **P14** | FM Voice | Ergonomic four-operator melodic FM synth — macro-first, fixed algorithm set. Distinct from the FM drum engine. | — |
| **P15** | Effects Palette | Distortion / saturation variety (tube, diode, fold, crush). Chorus / phaser / flanger. BBD + tape delay. Spring + plate reverb. 70s/80s and dub-techno character. | — |
| **P16** | Macro & Terminal Control | First-class macro system (one control → many scaled destinations, saved per kit). TUI as an input / editing surface, not just a display. | — |

---

## Why P10 Now — The Playability Gap

P6–P9 invested in **platform depth**: synthesis engines, CLAP plugin/host,
dynamic topology, the modular CV graph, and nested executors. That work is
done and the platform is sound.

But "fun to play" is defined by `instrument-vision.md`'s **"A Session"**
walkthrough, and most of its gestures depend on a **live-performance / pattern
layer that has not been built.** The `Sequencer` is still a single 16-step,
single-pattern engine with the P5 feature set (conditions, swing, fill,
micro-timing) bolted on. Concretely, the vision needs and the engine lacks:

- **Pages** — patterns longer than 16 steps (up to 64), scene-button page
  select, and page-loop selection
- **Patterns** — more than one pattern per track, seamless end-of-loop
  switching, cueing, and chaining (`CMD_SET_PATTERN` is a stub that always
  plays pattern 0)
- **Polyrhythm** — per-track length and speed multiplier (the hat at 2× while
  the kick runs 16ths)
- **Durable state** — trig conditions, micro-timing, and swing are silently
  **dropped on save** (BUG-005). This is data loss today.

P10 closes this gap. P11 layers the live-performance affordances (mute, temp
save, perform kit, live record) on top of the pattern model P10 establishes.

The platform-vs-instrument tiebreaker (`instrument-vision.md`): through the
playable instrument, the instrument wins. P10–P12 are the instrument finally
becoming playable the way the vision describes.

---

## The Arc Beyond P12 — Completing the Full Instrument

P10–P12 complete the **sequencing and performance** pillar. They make Paraclete
a world-class groovebox — but a groovebox is not yet the *full instrument* the
vision describes (`instrument-vision.md`, four pillars). Two pillars remain
largely unbuilt, and the roadmap now schedules them:

- **Synthesis (P13–P14)** — the melodic core. A full subtractive analog voice
  (the lead / bassline engine) and an ergonomic FM voice. The drum modules
  already exist; the *melodic* voices do not. **P13 is the keystone of the whole
  arc** — without it the instrument is percussion-and-bassline only.
- **Effects (P15)** — the sonic identity. Characterful distortion, modulation,
  delay, and reverb. The current set is one distortion, one delay, one
  algorithmic reverb; the 70s/80s and dub-techno character lives in the breadth
  and the analog-modeled flavors this phase adds.
- **Macro & terminal control (P16)** — the performance glue and the "fully
  managed by terminal" goal: a first-class macro layer and a TUI that *edits*,
  not just displays.

These are **scope, not new architecture** — the node model and the synthesis
primitives (oscillator, envelope, LFO, ladder filter) already support all of it.
Sequenced first because the sequencer is what makes the rest playable; synthesis
and effects follow because that is where the sound actually lives. Only after
this arc does "a single tool that replaces the DAW's role as a full instrument"
become true rather than aspirational.

---

## P9.5 Scope (enabling prerequisite)

**Deliverable:** Every feature of every supported device is exercisable from the
terminal — no physical hardware required to develop or test.

**Why:** Audit (June 2026) found device emulation is largely absent. Only the
Launchpad has a software emulator, and it reaches just **24 of 64 pads** (rows
0–2), with **no scene buttons, no control row, fixed velocity, and no RGB** in
the keyboard mapping. The Launch Control XL has **no device node at all**.
Keystep and Digitakt are `midir`-only (exist solely when plugged in). The result:
step toggles on 3 of 8 tracks are the entire keyboard-testable surface. P10's
pages/pattern-switching (scene buttons, all 8 tracks) and P11's mutes/fills are
bound to exactly the controls the emulator cannot reach — so without this work
they are hardware-only to verify.

- **Full Launchpad emulator** *(priority)* — all 64 pads + 8 scene buttons + the
  top control row reachable from the keyboard (track-select cursor scheme, since
  controls exceed keys), mode switching, and distinct rendering per LED
  state/color. The Launchpad is the device that *has* an emulator but an
  incomplete one; this is the minimum P10 Commit 5 depends on.
- **Encoder testing via the Digitakt (stopgap)** — the Digitakt has endless
  encoders and is already a supported `midir` node (`DigitaktMidiNode`,
  `digitakt.rhai`). Use the **real hardware** to exercise contextual parameter
  control rather than building an encoder emulator; verify/complete the Digitakt
  encoder → `CMD_SET_PARAM`/`CMD_BUMP_PARAM` relative-encoder path. This matches
  the eventual endless-encoder controller (Launch Control XL **Mk3**); the Mk2's
  absolute-position pots are explicitly *not* a target — limit-bound pots are a
  poor fit for this instrument, so no XL Mk2 node is built.
- **Computer-keyboard-as-piano** — note/arp input standing in for the Keystep
  when its hardware isn't connected.
- **Scripted/headless input injection** — drive device-event sequences from a
  file or virtual-MIDI port so P10/P11 integration tests run in CI, not only by
  hand.

A full interface spec (`design/phases/p9.5-interfaces.md`) is TBD; the full
Launchpad emulator is the priority and gates P10's surface verification.

---

## P10 Scope (current)

**Deliverable:** The sequencer becomes a real pattern engine. A track holds
multiple patterns; a pattern spans multiple pages; tracks run at independent
lengths and speeds; and every bit of sequencer state survives a save/load.

See `design/phases/p10-interfaces.md` for the implementation blueprint and
`design/adr/ADR-030-pattern-engine.md` for the data-model decision.

- **Pattern data model** — `Pattern` struct owns the `Vec<Step>`, length, and
  page-loop window. `Sequencer` owns a `Vec<Pattern>` plus an `active` /
  `cued` pattern index. (ADR-030)
- **Multi-page patterns** — up to 64 steps; page-loop window selects which
  contiguous run of pages plays.
- **Seamless pattern switching** — `CMD_SET_PATTERN` cues a pattern; the switch
  lands at the end of the current pattern's cycle (or page-loop window). Pattern
  chaining: a volatile queue of up to 8 patterns.
- **Per-track length & speed** — independent `pattern_length` and a speed
  multiplier (1/8×–2×) applied to `ticks_per_step`. Polyrhythm falls out.
- **Full serialization** — serializer v3: conditions, timing, swing, page-loop,
  and all patterns are persisted. Resolves **BUG-005**.
- **State bus + TUI + Launchpad surface** — publish active/cued pattern, page,
  and per-track length so the TUI and Launchpad LED feedback reflect the new
  state. Scene buttons select pages; cued patterns blink.

**Out of scope for P10 (moves to P11):** mute system, temporary save/reload,
Perform Kit mode, live record from the Keystep. P10 builds the model these sit on.

---

## P11 Scope (next)

**Deliverable:** The instrument becomes performable. Mutes, safety net, and
continuous-evolution modes from the vision's "A Session."

- **Mute system** — pattern mute (saved with pattern) + global mute (saved with
  project) + prepared mute (stage while holding a modifier, execute atomically).
- **Temporary save / reload** — volatile in-RAM pattern snapshot; the live
  "quick-save" safety net. Not persistent across power cycles.
- **Perform Kit mode** — parameter tweaks stay live and unsaved; current state
  is retained across pattern switches rather than reset to the pattern's preset.
- **Live record** — Keystep notes captured into the active pattern in real time
  (step-record and live-quantised-record).

---

## Known Provisional Implementations

| Item | Status | Resolution | Target |
|---|---|---|---|
| `CMD_SET_PATTERN` is a stub (always plays pattern 0) | Active | Multi-pattern model | **P10** |
| Single-pattern, 16-step only (no pages, no polyrhythm) | Active | Pattern engine | **P10** |
| `Sequencer::serialize()` drops P5 fields + swing (BUG-005) | Active | Serializer v3 | **P10 C1** |
| `set_initial_params` re-applied on re-activate (BUG-008) | Active | `mem::take` in `activate()` | **P10 C0** |
| Step period 241 ticks, not 240 (BUG-001) | Active | Transport start-model fix | **P10 C0** |
| Negative micro-timing == zero (BUG-004) | Active | Emit in prev step's window | **P10 C3** |
| `ConnectionAgreement::baseline()` hardcodes 44100/512 (BUG-002) | Active | Thread real config through | **P10.5** |
| `paraclete-hal` depends on `paraclete-runtime` (BUG-003) | Active | Move `StateBusHandle` to L2 | **P10.5** |
| `agg_state_buf` reallocs per cycle (BUG-006) | Active | Return-channel SPSC | **P10.5** |
| `publish_bank_state()` String alloc per cycle (BUG-007) | Active | Cache path strings | **P10.5** |
| Launchpad emulator reaches only pads 0–23, no scene/control row, no RGB | Active | Full emulator | **P9.5** |
| No keyboard path to test encoders | Active | Use real Digitakt (endless encoders) as stopgap | **P9.5** |
| Keystep is `midir`-only (no emulator / input injection) | Active | Keyboard-piano + scripted injection | **P9.5** |
| Launch Control XL Mk2 (absolute pots) not a fit; no XL node | By design | Target Mk3 endless encoders later; Digitakt covers encoders now | — |
| AnalogEngine/FmEngine monophonic (retrigger only) | Active | Voice allocator | P11+ |
| Inner GraphNode runtime patching (outer graph only at P9) | Active | Inner `apply_patch()` | P11+ |
| `InnerGraphNode::serialize()` returns empty | Active | Inner-graph persistence | P11+ |
| CLAP plugin nodes not in `NodeRegistry` | Active | Registry + PluginLibrary arg | P11+ |

**Every open bug (BUG-001 … BUG-008) is now scheduled:** BUG-005/008/001/004 in
P10, BUG-002/003/006/007 in P10.5. Nothing is unscheduled.

---

## Open Questions

| # | Question | Blocking | Notes |
|---|---|---|---|
| OQ-3 | MIDI 2.0 CI depth | — | Negotiable trait shipped P3 with no CI. Deferred; not blocking. |
| OQ-4 | Network / distributed nodes | — | Shapes EventStream serialisation format. Not blocking. |
| OQ-6 | Micro-timing clock representation | — | Signed micro-timing implemented in P10 C3 (BUG-004 fix). Close out in p10-report. |
| OQ-7 | Oversampling strategy | — | No oversampling until CvSignal audio-rate modulation needs it. |
| OQ-10 | XL Mk3 query protocol integration | Mk3 | CC ch 7/8 protocol for bidirectional sync. Implement when Mk3 arrives. |
| OQ-11 | Pattern/page representation on the Launchpad grid | **P10** | Scene buttons = page select; how cued/chained patterns are shown. ADR-030 + profile script. |
| OQ-12 | Live-record quantisation model | P11 | Step-record vs. live-quantised-record; interaction with micro-timing. |

---

## Resolved Open Questions

| Question | Resolution | ADR / Phase |
|---|---|---|
| Language | Rust | ADR-002 |
| License | GPL3 platform / LGPL3 Node API | ADR-001 |
| Plugin format | CLAP primary, VST3 secondary | ADR-003 |
| Signal model | Mixed-rate typed ports | ADR-004 |
| Scheduler cycles | petgraph DAG; loop-break node at P9 | ADR-005, ADR-028 |
| Hardware emulator | Required at P1 | ADR-006 |
| Scripting runtime | Rhai | ADR-007 |
| Node API level | Single trait, three implicit levels | ADR-008 |
| Clock ownership | Federated domains | ADR-009 |
| Multi-track sequencer | Multiple node instances | ADR-016 |
| Effect node architecture | Plain nodes, AudioEffect marker only | ADR-017 |
| Cellular architecture | No non-node platform components | ADR-018 |
| Universal parameter control | CMD_SET_PARAM / ParameterBank | ADR-019 |
| Node portability | Nodes link L2 only | ADR-022 |
| Instrument encapsulation | GraphNode primitive | ADR-023 |
| CLAP plugin wrapper approach | Hand-rolled adapter; nih-plug removed | ADR-024 |
| State bus persistence boundary (OQ-2) | serialize() per node; RON project file | ADR-025 |
| Operator feedback surface (OQ-1) | Terminal UI primary; profiles publish context | ADR-026 |
| CLAP host approach | clap-sys + libloading; PluginLibrary/PluginNode | ADR-027 |
| Single-sample feedback loop break (OQ-5) | Explicit LoopBreakNode | ADR-028 |
| Dynamic topology | Pause-rebuild-resume; NodeRegistry | ADR-029 |
| Pattern engine data model | Pattern owns steps; Sequencer owns patterns | **ADR-030 (P10)** |
| WAV-only format support | symphonia (WAV/FLAC/AIFF/OGG/MP3) | P6 |
