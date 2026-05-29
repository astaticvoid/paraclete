P0 skeleton is bootstrapped and compiling clean. Here's what was built and what I found reading the ADRs.


## What shipped

Six-crate Rust workspace, zero warnings:

  Crate                   Layer       Status
  paraclete-node-api      L2 LGPL3    All P0 types from the interface spec
  paraclete-runtime       L1 GPL3     Configurator/executor split, SPSC ring buffer, petgraph topology
  paraclete-hal           L0 GPL3     cpal audio backend, LaunchpadEmulator stub
  paraclete-scripting     L4 GPL3     Sandboxed Rhai engine
  paraclete-nodes         L3 GPL3     SilentNode placeholder
  paraclete-app           binary      Boots skeleton, opens audio stream, parks main thread


## ADR conflicts found — need your input

### 1. ADR-013 vs p0-interfaces.md (Event enum) — decision needed

ADR-013 specifies:

    enum Event {
        NoteOn(NoteOnEvent),
        NoteOff(NoteOffEvent),
        ParamChange(ParamChangeEvent),
        ParamLock(ParamLockEvent),
        Transport(TransportEvent),
        Tempo(f64),
        Extended(ExtendedEventRef),
    }

p0-interfaces.md specifies:

    enum Event {
        Midi2(midi2::UmpMessage),   // collapsed into one variant
        ParamLock(ParamLockEvent),
        Transport(TransportEvent),
        Tempo(f64),
        Extended(ExtendedEventRef),
    }

I implemented Midi2(UmpPacket) following p0-interfaces.md, on the reasoning that: UMP already
encodes message type in words[0] >> 28, splitting it further duplicates the MIDI 2.0 spec in
Rust, and CLAP/hardware both deal in raw packets. If you want the note-specific variants, say so
and I'll add them — but ADR-013 should then be updated to explain why the higher-level
decomposition is preferred over UMP.


### 2. ADR-013 — #[cold] on an enum variant

The ADR documents:

    #[cold]
    Extended(ExtendedEventRef),

#[cold] is only valid on functions in Rust, not enum variants. The intent (tell the compiler this
branch is rarely taken) is correct but the mechanism isn't available. Extended is placed last in
the enum as the practical alternative — LLVM treats later exhaustive match arms as less likely.
If this matters, the right solution is a wrapper function marked #[cold] around the Extended
handling path. No action needed for P0; flagging it so the ADR can be corrected.


### 3. ADR-001 — crate name says mmp-node-api

The ADR references mmp-node-api. The code uses paraclete-node-api. If the project was renamed
from MMP to Paraclete, ADR-001 just needs its crate name updated. No code change needed.


## One missing dependency caught late

ADR-005 and ADR-011 both explicitly require petgraph from P0 ("P0 scheduler code must use
petgraph and must not assume DAG topology"). The initial implementation used a plain Vec —
caught and fixed. The runtime now has:

  - RuntimeGraph — petgraph::stable_graph::StableDiGraph
  - execution_order() — toposort with is_cyclic_directed + Tarjan SCC cycle detection
  - NodeConfigurator::connect() — rejects cyclic connections at the boundary until P9 loop-break nodes
  - build_executor() — passes nodes to the executor in toposort order


## To run

    export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
    cargo run -p paraclete-app

Produces silence on the default audio output. That's the point.
