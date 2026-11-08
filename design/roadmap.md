# Paraclete — Master Roadmap & Architectural Direction

> **Plan, not state.** This file defines the architectural north star, the
> two-layer structure of the project (universal signal-graph substrate vs.
> opinionated "no-DAW" instruments), and the ordered phase sequence from
> immediate hardware playability to full modular graph authoring and a
> self-contained native binary host.
>
> **Live state — open bugs, open questions, spec gaps, spikes, provisional
> implementations — lives in GitHub Issues**, not here. See `design/README.md`
> for lookup commands:
> `gh issue list --state open` / `gh api repos/:owner/:repo/milestones`.

---

## 1. Architectural North Star: Substrate vs. Instruments

Paraclete is intentionally structured across two distinct levels that must not
be conflated:

### Level A — The Universal Signal-Graph Underlay (Reaktor / Bitwig Grid Level)
- **What it is:** A real-time, lock-free audio, modulation, and event graph
  (`paraclete-node-api`, `paraclete-runtime`, `paraclete-graph-nodes`,
  `paraclete-nodes`) where *every* component—oscillators, filters, envelopes,
  LFOs, macro engines, step sequencers, effects, clocks, and hardware
  controllers—is a first-class `Node` with typed ports (`Audio`, `Modulation`,
  `Midi2`, `Surface`, `Transport`).
- **What it enables:** Modular compositional articulation of synthesizers, drum
  machines, sequencers, and effects. It can sit at the core of a standalone
  hardware-coupled instrument, an interactive modular patch editor, a `.clap`
  plugin (`paraclete-clap`), or eventually a full DAW.
- **Two-Tier Voice & FX Model:** Nodes can be fast, curated C/Rust DSP macros
  (Tier 1: `AnalogEngine`, `FmVoice`, `Sampler`) *or* openable subgraphs
  composed from primitive modular blocks inside an `InnerGraphNode` (Tier 2).

### Level B — Opinionated "No-DAW" Instruments & Surfaces Built on the Substrate
Rather than defaulting to a mouse-bound timeline, Paraclete expresses concrete
musical instruments on top of the graph:
1. **Roland x0x + Launch Control XL Drum Machine (`lcxl_drum` + `Screen::Mix`):**
   Bounded, immediate 8-track drum machine. Each column on the LCXL is one drum
   track (3 primary synthesis/filter knobs + 1 volume fader), while the bottom
   16 buttons operate as a classic 16-step x0x sequencer (enhanced with
   hold-step + knob turn parameter locking) and track mute/focus strip.
2. **ThinkPad + Elektron-Class Workstation (`paraclete-theotokos`):**
   Deep keyboard-first performance terminal with parameter pages (`TRIG`, `SRC`,
   `FLTR`, `AMP`, `FX`, `MOD`), conditional trigs, microtiming, parameter locks,
   8-pattern chains, kit recall (`P11`), and live sampling (`AN0–AN1`).
3. **Expressive Grid & Controller Surfaces (`Launchpad`, `Keystep`):**
   Tactile 8×8 pad matrix for pin-matrix modulation/send routing (EMS Synthi /
   Pulsar-23 style), polyrhythmic clip/step control, and punch-in performance FX.
4. **Visual Graph Patcher & Host Progression (`Antiphon/Theoria` → Native Binary):**
   - **Near-term Bridge (`paraclete-antiphon` + `web/`):** Make the existing web
     interface feature-complete as a companion surface *and* extend it with a
     visual node-graph patch editor (`W5`) so instruments and routing can be
     designed visually without an AI agent in the loop.
   - **Long-term Destination (`TKW` / Native Self-Contained Host):** A
     cross-platform, high-performance, self-contained native binary GUI runtime
     (no browser dependency required) rendering both the performance panels and
     the interactive modular Grid patcher.

---

## 2. Master Execution Sequence (Horizons 1 → 4)

Work proceeds in four ordered horizons so that immediate musical playability
hardens the exact plumbing required by the deeper modular substrate.

### Horizon 1 — Complete & Harden the LCXL x0x 8-Track Drum Machine (Immediate)
**Goal:** A zero-menu, out-of-the-box playable 8-track techno/electro drum
machine on the Launch Control XL + ThinkPad `MIX` screen, with a strict
Definition of Done.

| Unit | Scope | Tracking |
|---|---|---|
| **H1.1 — LCXL Framework & Matched TUI Strip** | `CMD_SET_PARAM_NORM`/`CMD_BUMP_PARAM_NORM`, `LcxlNode` (105), `LcxlDrumController`, 8-track `Screen::Mix` strip (`Shift+8`), runtime `:w`/`:e` save/load, lossless yank/paste | **Shipped** (`a4b7e5b`, `c8f0503`) |
| **H1.2 — Drum Voice Breadth & Velocity** | Add **Clap** and **Tom / Rim** synthesis machines to `AnalogEngine`; wire per-step velocity into `AnalogEngine`, `FmEngine`, and `Sampler` triggers; add closed-to-open hi-hat choke | **#173**, **#116**, **#172** |
| **H1.3 — Hold-Step P-Lock & Aux FX Routing on LCXL** | Hold any of the 16 step buttons on LCXL in `Seq` mode + turn any track knob to write a per-step parameter lock (`CMD_SET_STEP_LOCK`); wire per-track Delay + Reverb aux sends in `instrument-8track.yaml` | **#190**, **#127** |
| **H1.4 — Paired Hardware Dialing & AN1/P14 Playability Gate** | Live paired session with physical LCXL (`#190`), `FmVoice` playability sign-off (`P14`), and `SAMPLING` screen source/arm/bar controls (`#187`) | ⛔ Paired session (`#190`, `#187`) |

---

### Horizon 2 — Tactile 8×8 Matrix Routing & Groove Expressiveness
**Goal:** Liberate the Launchpad 8×8 grid (now that LCXL owns the 8-track channel
strips) into an expressive routing/performance surface, and add generative
rhythm tools.

| Unit | Scope | Tracking |
|---|---|---|
| **H2.1 — Smooth Audio/CV Matrix Node & Launchpad Pin-Matrix** | First-class `MatrixNode` (pre-allocated $N \times M$ audio/modulation crosspoints with 5 ms one-pole smoothing and zero audio-thread pauses) driven by the Launchpad 8×8 grid (pad tap = latch crosspoint, polyphonic pad pressure = live modulation/send depth, RGB = signal × weight) | **#75**, **#123** |
| **H2.2 — P12 Groove & Generation** | Per-step ratchets/retrig, Euclidean pattern generator, controlled step mutation/randomness, and momentary punch-in fill/roll gestures | **P12** |
| **H2.3 — P15 Effects Palette** | Dub/BBD & tape delay enhancements, master bus compressor/limiter + sidechain ducking, chorus/phaser/flanger, distortion flavors | **P15** |

---

### Horizon 3 — The Modular Grid Substrate & Visual Patch Bridge
**Goal:** Expose the Reaktor / Bitwig Grid level so instruments, voice
subgraphs, and effects chains can be visually patched, inspected, and saved
without editing YAML by hand or prompting an agent.

| Unit | Scope | Tracking |
|---|---|---|
| **H3.1 — Modular Utility Primitives & P13 Analog Voice** | Complete the Tier-2 modular building blocks in `paraclete-nodes`: CV-controlled VCA/attenuverter, CV/audio mixer, slew limiter, sample-and-hold, pitch quantizer, clock divider/multiplier; build `P13` subtractive/paraphonic voice and prove Tier-1 ↔ Tier-2 equivalence | **P13**, **#80** |
| **H3.2 — `InnerGraphNode` Persistence & Live Subgraph Patching** | Implement `InnerGraphNode::serialize()` / `deserialize()` and runtime `PatchOp` forwarding into nested executors; decouple surfaces from hard-coded node IDs (`10..17`) via dynamic track/role discovery | **#119**, **#113**, **#165** |
| **H3.3 — W2–W5 Feature-Complete Web Bridge & Visual Graph Patcher** | Close remaining `Theoria` parity items (`W2 C7`, `W3`) and add the **W5 Visual Patch Editor** over `Antiphon` (`ServerMsg::Topology` with full ports/edges + `ClientMsg::Patch` for drag-and-drop node creation, cable routing, macro assignment `P16`, and YAML export) | **W2**, **W3**, **W5**, **P16** |

---

### Horizon 4 — Self-Contained Cross-Platform Native Binary Host
**Goal:** Replace the browser dependency with a fast, self-contained,
single-binary native UI runtime (`TKW` + Native Grid Canvas) on Linux, macOS,
and Windows.

| Unit | Scope | Tracking |
|---|---|---|
| **H4.1 — Self-Contained Native UI Host (`TKW`)** | Embed both the keyboard-first performance workstation (`RenderData` / `paraclete-view-assembly`) and the visual node-graph patch editor directly into a GPU-accelerated native window binary (no external browser or npm runtime required), with full raw key-press/release semantics and zero-copy state bus readouts | **TKW** |

---

## 3. Phase Reference Table (Historical & Planned)

Per-phase specs and reports live under `design/phases/` and `design/sessions/`.

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0–P9** | Skeleton → Modular Graph | Core 5-layer engine, DAG, CLAP export/host, Rhai | **Complete** (`architecture-evolving.md`) |
| **P9.5** | Device Emulation & Test Harness | Launchpad emulator (C1), headless `test-driver` (ADR-033) | **Complete** |
| **W0–W1** | Antiphon & Theoria MVP | `paraclete-antiphon` WS bridge, 8×8 mirror, touch encoders, state mirror v1 | **Shipped** (`w0-report.md`, `w1-report.md`) |
| **P10** | Pattern Engine Foundation & Depth | Multi-page (64-step) patterns, cued switching, chains, per-track length/speed, v3 save | **Shipped** (`p10-report.md`) |
| **MM** | Machine Select & Modulation | ADR-041 `machine` identity param, per-variant overlays, per-machine LFO destinations | **Shipped** (`mm-machine-and-mod.md`) |
| **P11** | Live Performance & Kits | Mute tiers, temp save/reload, `KitStore`, Perform mode, live record/quantize/erase | **Shipped** (`p11-report.md`) |
| **TK0–TK3** | Theotokos Terminal Workstation | 7-region keyboard panel, hold-chords, ENC mode, p-locks, TRIG/MIX screens | **Shipped** (`tk3-report.md`) |
| **AN0–AN1** | Anamnesis Live Sampling | `SamplePool`, `AudioInputNode`, `RecorderNode`, RAM capture freeze, `SAMPLING` screen | **Code complete** (`a4b7e5b` closes runtime wiring; awaiting paired session + `#187`) |
| **P14** | FM Voice | 4-operator melodic FM (`FmVoice`, node 28) with 8 algorithms + 7 machine modes | **Code complete** (`p14-fm-voice.md`; awaiting playability session) |
| **LCXL-1** | LCXL 8-Track Drum Machine | `LcxlNode` (105), normalized param commands, `LcxlDrumController`, 8-track `MIX` strip | **Framework shipped** (`c8f0503`; Horizon 1 completion in `#190`, `#173`, `#116`, `#172`) |
| **P12** | Groove & Generation | Retrig/ratchets, Euclidean generator, controlled randomness, fill modes | Horizon 2 |
| **P15** | Effects Palette | BBD/tape delay, chorus/phaser/flanger, master compressor/limiter | Horizon 2 |
| **P13** | Analog Voice & Modular Primitives | Subtractive/paraphonic voice + Tier-2 modular utility nodes (VCA, slew, S&H, quantizer) | Horizon 3 (`#80`) |
| **P16** | Macro Control | Instrument-wide macro knobs + controller sync policies (SPIKE-006) | Horizon 3 |
| **W2–W5** | Feature-Complete Web Bridge & Patch Editor | Complete web surface parity + interactive drag-and-drop graph patcher over Antiphon | Horizon 3 |
| **TKW** | Native Self-Contained Binary Host | Cross-platform native GUI window hosting both performance views and the Grid patcher | Horizon 4 |

---

## 4. Standing Architectural Guardrails

1. **Audio-thread constraints are absolute:** `process()` never allocates,
   blocks, or takes a lock.
2. **One substrate, multiple instrument archetypes:** Never hard-code a single
   surface's assumptions into L1/L2/L3. Hardware controllers and UIs are modular
   consumers/producers on the graph and StateBus.
3. **Two-tier engines:** Fast monolithic C/Rust DSP engines (Tier 1) and
   subgraph-composed `InnerGraphNode` patches (Tier 2) share the exact same
   `Node` contract (`ADR-023`, `ADR-032`).
4. **Controller synchronization (`SPIKE-006`):** Soft-takeover / pickup is
   disqualified. Every physical control binding declares either
   `HardwareAuthoritative` (pots/faders are immediate truth via
   `CMD_SET_PARAM_NORM`) or `PseudoRelative` (first CC latches reference,
   subsequent CCs emit deltas via `CMD_BUMP_PARAM_NORM`).
5. **No Digitakt-as-encoder-controller:** Digitakt transmits absolute CCs only
   and remains a workflow reference, not a parameter-knob surface
   (`design/sessions/s0-hardware-checks.md`).
