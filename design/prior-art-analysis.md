# Prior Art & Failure Analysis

> **Reference document.** Append new findings as research continues.
> Does not need replacing — add sections at the bottom for new projects studied.

-----

## Purpose

This document records the open-source projects studied before and during design, with particular focus on failure modes. Every failure mode has a named mitigation in the architecture. If a new failure mode is discovered that has no mitigation, the architecture document must be updated before continuing.

-----

## Summary Scorecard

|Project          |Lang|Architecture|Scope      |Team          |Primary Failure Mode                                  |
|-----------------|----|------------|-----------|--------------|------------------------------------------------------|
|Meadowlark DAW   |Rust|Weak        |Too large  |Solo          |No architecture doc; code preceded design; GUI blocker|
|OTTO Groovebox   |C++ |Mixed       |Too large  |2 devs        |Legacy foundation rot; hardware coupling; feature gap |
|HexoSynth/HexoDSP|Rust|Strong      |Focused    |Solo          |Graph/DSP coupling; GPL contamination; solo bandwidth |
|VCV Rack         |C++ |Strong      |Incremental|Solo→community|Closed ecosystem initially; no DAW integration path   |

-----

## Meadowlark DAW

**Repo:** github.com/MeadowlarkDAW/Meadowlark  
**Engine:** github.com/MeadowlarkDAW/dropseed  
**Blog:** billydm.github.io/blog  
**Status:** Hiatus (April 2023)  
**Language:** Rust  
**License:** MIT

### What it is

A Rust-based open-source DAW for Linux, macOS, Windows. The developer had a design document, public Discord, community enthusiasm, and split the codebase into reusable crates (dropseed, rainout, creek, pcm-loader, clack fork, audio-graph fork). Went on hiatus after 2+ years.

### Failure Mode 1: Code preceded architecture

> *“My original plan for Meadowlark was to just wing it and develop nearly everything myself for the MVP release for Meadowlark, and then go from there. This however didn’t turn out as well as I hoped. The code began to get really messy and hard to follow with a lot of interconnected parts.”*

> *“If I do decide to have a team of developers, I need to spend time creating an actual design document this time around. Not just explaining the goals of the project, but actual technical plans on how things will work and fit together.”*

The design document was a goals document, not a technical architecture document. Interfaces were not defined before implementation. Messy code became impossible to coordinate around.

**Paraclete mitigation:** Architecture document written before any implementation. Interface contracts defined per phase before phase begins.

### Failure Mode 2: Repository sprawl without architecture

> *“Because of how easy Rust makes it to make modular code, wherever it made sense I split the code into reusable crates… But now I find that managing so many repositories is a headache.”*

12+ repositories and forks created without a governing architecture. Each became a coordination burden. Issues and PRs created anxiety rather than progress.

**Paraclete mitigation:** Single workspace until Phase 5. Crate boundaries defined by the layer model before any code is written. See `architecture-core.md` crate structure section.

### Failure Mode 3: GUI underestimated, not decoupled

> *“DAWs might just have one of the most complicated GUIs out of any piece of software out there.”*

The developer published a 6,000-word analysis of Rust GUI libraries and found none suitable. Because the engine and GUI were not cleanly separated, GUI problems blocked everything. Resolution required partnering with another developer for a Flutter frontend and pivoting the engine to a standalone API (Dropseed).

**Paraclete mitigation:** GUI is a hardware controller supplement, not an editing environment. Deferred to P4. Isolated behind the state bus. See `architecture-evolving.md` GUI strategy section.

### Failure Mode 4: Volunteer dependency without onboarding infrastructure

> *“You can’t rely on volunteers to build key components of a large open source project… you must be prepared that you will be working on it alone for a long time.”*

> *“I’ve gotten a lot of people joining my Discord server excited about the project. A lot of them are even eager to help contribute code. While this sounds like a great thing, in reality I’ve been really struggling to figure out what these volunteer developers can actually do to help.”*

Community enthusiasm created anxiety rather than velocity. No onboarding infrastructure, messy architecture meant no clear contribution paths.

**Paraclete mitigation:** Roadmap designed for solo execution through P5. Community contribution only after architecture is proven and interfaces are stable. Node extension is the natural contribution path once L2 API is stable.

### What Meadowlark got right

- **Dropseed** (audio graph engine with CLAP hosting via clack) — well-written, potentially reusable under MIT
- **creek** (audio file streaming from disk) — solid, potentially reusable for sampler node
- **GUI analysis** — the most thorough survey of Rust GUI options for audio software, still current
- **Architecture pivot** to engine-first / GUI-separate validates Paraclete’s layer model

### Technical finding: Three-state synchronisation problem

Meadowlark’s frontend blog identifies a specific problem: save file state, GUI state, and audio engine state must be kept in sync but have different units, update frequencies, and threading constraints.

**Paraclete solution:** State bus as the synchronisation mechanism. Audio engine publishes to the bus. GUI subscribes. Save file serialises bus state plus node private blobs. No direct state sharing between audio thread and GUI thread.

-----

## OTTO Groovebox

**Repo:** github.com/bitfieldaudio/OTTO  
**Status:** Suspended (May 2022)  
**Language:** C++  
**License:** CC BY-NC-SA 4.0

### What it is

An open-source hardware groovebox — complete hardware and software instrument with synths, sampler, sequencer, and effects. OP-1 inspired. Custom PCB hardware running on Raspberry Pi. Worked on for 5 years by 2 developers.

### Failure Mode 1: Legacy foundation rot

> *“On both the software-side and more broadly in terms of features and design, OTTO still builds on a foundation that was laid 5 years ago, when we didn’t know what we were doing, and the past year has been spent fighting these fundamental issues.”*

Original architecture made reasonable sense at the start but accumulated assumptions that became increasingly expensive to work around. Cost of rearchitecting outweighed continuing. Classic debt spiral.

**Paraclete mitigation:** Architecture document defines interfaces before code. Stub Deep / Ship Shallow principle ensures interfaces are designed for full depth. Layer model creates upgrade paths — L1 can be rearchitected without breaking L2 and above.

### Failure Mode 2: Hardware/software tight coupling

> *“From a hardware-perspective, the global chip shortage hit the project hard — forcing us to rework hardware parts several times because of out-of-stock components.”*

Hardware communication was baked into the application layer. PCB revisions required software rewrites.

**Paraclete mitigation:** HardwareDevice is a capability-declaring graph node. The platform never assumes a specific controller layout. Hardware revisions change the HAL node’s capability document, not the platform.

### Failure Mode 3: Feature differentiation crisis

> *“Since we started, the groovebox space has been expanding, and in 2022 the OTTO lacks the defining feature that would make it worth the wait.”*

Product competing in a product market. By 2022 the market had expanded and OTTO had no clear differentiating feature.

**Paraclete positioning:** Paraclete is a platform, not a product. Its differentiating feature is the composability model and hardware-first open extensibility — not a specific sound or workflow.

### What OTTO got right

- Service-oriented architecture (audio, graphics, controller, logic services) — influenced Paraclete layer design
- **Hardware emulator** — software simulation of physical controller for development without hardware. Paraclete adopts this pattern at P1.
- Board configuration system cleanly separated platform-specific implementations
- OttoFM and Nuke engine implementations are C++ algorithm references

-----

## HexoSynth / HexoDSP

**Repo:** github.com/WeirdConstructor/HexoSynth  
**Crate:** github.com/WeirdConstructor/HexoDSP  
**JIT:** github.com/WeirdConstructor/synfx-dsp-jit  
**Status:** Active (solo developer, limited bandwidth)  
**Language:** Rust  
**License:** GPL3

### What it is

A FLOSS modular synthesizer plugin in Rust. Runtime-changeable DSP graph, JIT compilation for custom DSP, full monitoring/feedback system, 25+ DSP modules. Most directly relevant Rust prior art for Paraclete.

### What it gets right

**Runtime-changeable graph:** NodeConfigurator (main thread) and NodeExecutor (audio thread) separated by a lock-free ring buffer. Graph changes communicated via pre-allocated messages. Paraclete adopts this pattern directly (independently implemented).

**JIT compilation for custom DSP:** synfx-dsp-jit compiles custom DSP code to native speed inside a graph node. Directly relevant to Paraclete’s Phase 9 modular graph meta-node concept.

**Comprehensive monitoring:** Multiple levels of signal monitoring — per-node input/output monitoring, current output values of all nodes, signal oscilloscope. Relevant to Paraclete state bus design for developer tooling.

### Failure Mode 1: Graph and DSP coupling

> *“It seems that the graph interface and the DSP algorithms are melted together, which obfuscates things.”* — community code review

Adding a new DSP node requires understanding graph infrastructure. Modifying graph infrastructure risks breaking DSP assumptions. No clean boundary.

**Paraclete mitigation:** L2 Node API is the only interface a node sees. Nodes receive typed ports and an event queue. They cannot reach the graph, scheduler, or other nodes. Enforced structurally.

### Failure Mode 2: GPL contamination

> *“This project is licensed under the GNU General Public License Version 3 or later. The obvious reason is that this project copied and translated code from many other free software / open source synthesis projects.”*

HexoDSP is GPL3 because it ported GPL DSP code. synfx-dsp-jit is also GPL3.

**Paraclete note:** Since Paraclete is GPL3, HexoDSP code can now be studied as a GPL3-to-GPL3 reference. Implementations must still be independent (not copy-paste) but the architecture and algorithms can be closely referenced. This constraint was written when Paraclete was considering MIT/Apache — it is relaxed under GPL3.

**Paraclete DSP policy:** All DSP implementations must be either written from scratch or ported from MIT/Apache sources. Mutable Instruments firmware (MIT) is the primary reference. HexoDSP is available as a GPL3 algorithm reference.

### Failure Mode 3: Solo developer bandwidth

> *“I currently have a quite precise vision of what I want to achieve and my goal is to make music with this project eventually. The project is still young, and I currently don’t have that much time to devote for project coordination.”*

HexoSynth works and ships. The bandwidth constraint is an honest acknowledgment of solo open source development. Not a design failure.

**Paraclete note:** Design phases sized for one developer. Deliverables usable at each phase. Architecture enables contribution without coordination overhead once L2 API is stable.

-----

## VCV Rack

**Repo:** github.com/VCVRack/Rack  
**Status:** Active  
**Language:** C++  
**License:** GPL3 (Rack itself), modules vary

### What it is

The most successful open-source modular synthesis platform. 100,000+ users, 1,200+ modules, 150+ plugin developers. Free platform with a commercial module ecosystem.

### What it gets right

- **Single-sample buffer processing** was a founding decision enabling feedback loops. Cannot be retrofitted.
- **Stable, documented plugin SDK** from early on. Third-party developers built with confidence.
- **Focused scope:** started as a virtual Eurorack case with 25 modules. Did not try to be a DAW.
- **Free platform + commercial modules:** viable ecosystem business model.

### Failure Mode 1: No DAW integration path initially

Standalone-only architecture meant no DAW workflow integration. Addressed years later in Rack v2 with a VST bridge, but never fully first-class.

**Paraclete mitigation:** CLAP plugin mode is a Phase 7 design requirement, not an afterthought. Clock federation model explicitly handles the standalone→hosted transition.

### Failure Mode 2: C++ SDK as high contributor barrier

Module development requires C++ and DSP knowledge. Quality floor for community modules is uneven.

**Paraclete opportunity:** Rhai scripting layer provides a lower-barrier extension point for hardware mappings, macro definitions, and behavioural nodes that don’t require DSP expertise.

### Technical finding: Cycle detection for feedback loops

VCV Rack uses explicit loop-break nodes (single-sample delay nodes) to allow cycles while preserving deterministic execution order. Pure topological sort fails with cycles.

**Paraclete adoption:** Explicit loop-break node approach confirmed for Phase 9. L1 scheduler topology model must support cycles from P0 even though single-sample execution is not implemented until P9. See ADR-005.

-----

## Cross-Project Patterns

### The architecture debt spiral

In every stalled project: early code → local decisions → decisions become load-bearing → later features require different decisions → rework is expensive → debt accumulates → project stalls.

**Shape:** Write code without architecture → code works but is messy → adding features requires touching messy code → each change risks breaking existing features → developer spends more time maintaining than building → contributors cannot understand codebase → project stalls.

### The unresolved hard dependency

Every stalled project had at least one hard dependency that was unresolved and blocking:

- Meadowlark: GUI library
- OTTO: hardware supply chain
- HexoSynth: GUI toolkit (HexoTK rewrite took a year)
- VCV Rack: DAW integration (addressed years later)

**Pattern:** Hard dependencies must be identified and explicitly resolved (or explicitly deferred with a plan) before they block. Paraclete Section 9 (Open Questions) exists for this purpose.

### The solo developer scope problem

All four projects started with one or two developers. All had vision scopes that eventually exceeded execution capacity. Survivors (HexoSynth, VCV Rack) constrained scope and shipped incrementally.

-----

## Crate Reference

Crates studied and their integration status:

|Crate           |License   |Status            |Notes                                                                            |
|----------------|----------|------------------|---------------------------------------------------------------------------------|
|nih-plug        |MIT/GPL3  |**Use**           |CLAP + VST3 wrapper                                                              |
|clack           |MIT       |**Use**           |CLAP host bindings                                                               |
|cpal            |Apache-2  |**Use**           |Audio I/O                                                                        |
|midir           |MIT       |**Use**           |MIDI I/O                                                                         |
|midi2           |MIT       |**Use**           |MIDI 2.0 UMP types                                                               |
|midi20          |MIT       |**Use**           |MIDI-CI capability inquiry                                                       |
|dasp_graph      |MIT       |**Reference**     |Graph scheduler pattern                                                          |
|fundsp          |MIT/Apache|**Use**           |DSP combinators for node internals                                               |
|rhai            |MIT/Apache|**Use**           |Scripting runtime                                                                |
|petgraph        |MIT/Apache|**Use**           |Graph data structure                                                             |
|hexodsp         |GPL3      |**Reference only**|Architecture and algorithm reference. GPL3→GPL3 clean. Do not copy code directly.|
|synfx-dsp-jit   |GPL3      |**Reference only**|JIT compiler reference for Phase 9.                                              |
|mi-plaits-dsp-rs|MIT       |**Use**           |Mutable Instruments Plaits port                                                  |
|dropseed        |MIT       |**Study**         |Meadowlark audio graph engine. Configurator/Executor pattern reference.          |
|basedrop        |MIT       |**Consider**      |Lock-free audio thread memory management                                         |

-----

## DSP Sources

|Source                      |License   |Quality    |Notes                                                                       |
|----------------------------|----------|-----------|----------------------------------------------------------------------------|
|Mutable Instruments firmware|MIT       |Exceptional|Primary DSP reference. All modules. Braids, Clouds, Rings, Plaits, Elements.|
|mi-plaits-dsp-rs            |MIT       |High       |Rust port of Plaits already exists                                          |
|fundsp                      |MIT/Apache|High       |Oscillators, filters, envelopes in composable Rust                          |
|HexoDSP                     |GPL3      |High       |Algorithm reference under GPL3→GPL3. Implement independently.               |
|SuperCollider UGens         |GPL3      |High       |Algorithm reference only. Do not port directly.                             |
|Vital synth                 |GPL3      |High       |Reverb algorithm ported by Meadowlark developer as a blog post reference    |

-----

## Further Reading

|Resource                |URL                                                         |Notes                               |
|------------------------|------------------------------------------------------------|------------------------------------|
|Meadowlark hiatus       |billydm.github.io/blog/why-im-taking-a-break-from-meadowlark|Primary post-mortem                 |
|Meadowlark GUI analysis |billydm.github.io/blog/daw-frontend-development-struggles   |GUI library survey                  |
|Meadowlark clarification|billydm.github.io/blog/clarifying-some-things               |Pivot to Dropseed                   |
|OTTO suspension         |matrixsynth.com/2022/05/otto-synth-project-suspended        |Official announcement               |
|HexoDSP docs            |docs.rs/hexodsp                                             |Node implementation guide           |
|VCV Rack CCRMA talk     |ccrma.stanford.edu (July 2019)                              |Architecture overview               |
|Rust DSP survey         |pbat.ch/brain/rust_dsp                                      |Community codebase analysis         |
|Rust Audio links        |WeirdConstructor gist                                       |Comprehensive crate inventory       |
|Awesome-Audio-DSP       |github.com/BillyDM/Awesome-Audio-DSP                        |Meadowlark developer’s resource list|