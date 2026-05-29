# ADR-001: License

**Date:** May 2026  
**Status:** Accepted

## Decision

- **L2 Node API (`paraclete-node-api` crate):** LGPL3
- **Everything else** (L0, L1, L3, L4, first-party nodes, runtime, scripting): GPL3

## Context

The platform needs a license that maximises contributions and keeps the ecosystem open, while not discouraging hardware manufacturers and controller makers from integrating with the platform.

Four options were considered:

1. **MIT/Apache everywhere** — maximum permissiveness. Anyone can take the platform, improve it, and not contribute back. Commercial actors can close the platform.
1. **GPL3 everywhere** — maximum openness. Everything must stay open. Hardware firmware vendors may be deterred.
1. **GPL3 platform / LGPL3 Node API** — platform stays open, hardware and third-party node authors can keep implementation details closed as long as they only link against the published L2 API.
1. **AGPL3** — considered and rejected. Network service use case is not a primary concern.

## Rationale

Option 3 was chosen. The reasoning:

The Node API (L2) is the published integration boundary — the CLAP equivalent for internal nodes. Making it LGPL3 means:

- A hardware manufacturer shipping a device with Paraclete built in can keep their HAL driver closed-source
- A commercial instrument developer can build a closed-source synth node
- A controller company can keep their proprietary communication protocol closed

If they modify the runtime itself, fork the sequencer, or extend the scripting layer — that code is GPL3 and must be published.

This mirrors how Linux handles proprietary drivers: GPL kernel, defined ABI boundary, hardware vendors ship proprietary modules against that boundary.

The goal is maximum contribution to the platform itself (GPL3 enforces this) while not creating a license that deters hardware adoption (LGPL3 on the API boundary addresses this).

## Consequences

- HexoDSP code can now be studied as a GPL3-to-GPL3 reference (previously restricted when MIT/Apache was being considered). Implementations must still be independent — not copy-paste.
- All DSP algorithms must be sourced from MIT/Apache code (Mutable Instruments, fundsp) or written from scratch. GPL3 algorithm references (HexoDSP, SuperCollider) can be studied but not directly ported without independent reimplementation.
- The `paraclete-node-api` crate README must clearly state LGPL3 and explain what this means for implementors.
- All other crates must include a GPL3 header.