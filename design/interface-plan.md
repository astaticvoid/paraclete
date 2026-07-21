# Paraclete — Interface Plan: Antiphon & Theoria

> **Planning document.** Status: **accepted (July 2026).** Decision point 1
> becomes ADR-031 (authored with W0); decision point 2 becomes ADR-032
> (authored with W2). W0 gets `design/phases/w0-interfaces.md` before
> implementation. Sequencing per the roadmap: P10 C0 → W0 → P10 C1 → W1 →
> paired session #1.
>
> **Last updated:** July 2026
> **Depends on:** ADR-014 (capability document), ADR-018 (cellular
> architecture), ADR-019 (universal parameter control), ADR-026 (TUI /
> `publish_context()`), P10 (pattern engine — for W3).

---

## Names

- **Antiphon** — the interface server and its protocol. An antiphon is
  liturgical **call-and-response**: verses sung alternately between two sides.
  A bidirectional exchange, musical by definition — which is precisely what
  the protocol is. Crate: `paraclete-antiphon`.
- **Theoria** — the view layer and its clients. *Theoria* is the contemplative
  tradition's word for **beholding** — direct vision, as against *praxis*
  (action). The views are theoria; the controls are praxis; a surface is both.
  Clients: `theoria-web` (tablet/browser), `theoria-term` (terminal), plus a
  headless CI driver.

Assigned within Antiphon (July 2026):

- **kerygma** (*proclamation*) — the broadcast module in `paraclete-antiphon`
  that coalesces and proclaims LED batches and state diffs to all clients.
- **epiclesis** (*invocation*) — the headless protocol test driver (the CI
  harness that invokes the instrument through the real protocol; W4).

Reserved for later, same register: **Ordo** (workflow/layout profiles — the
*ordo* is the order of service) and **Triptych** (multi-panel layout system).
Don't spend these before the features exist.

Assigned outside Antiphon (2026-07-21): **Praxis** (*action* — the controls
half of "the views are theoria; the controls are praxis" above) — the
keyboard-first modal performance terminal. Track docs: `design/praxis/`;
decision: ADR-036.

**Rule:** evocative names go to *our own* components — crates, modules,
binaries, genuinely novel features. Wire messages and standard concepts keep
plain names (`hello`, `state`, `led`, *pages*, *grid*) — never rename
something standard for performance.

---

## What This Is

One interface architecture, three deliverables:

1. **Antiphon** — a presentation-agnostic interface server inside the
   Paraclete process. It owns the state mirror (StateBus subset), capability
   -document snapshots, the topology/context feed, and the two command planes
   (below). It speaks over **pluggable transports**: an in-process channel
   (terminal client, CI driver — no socket, no serialization cost) and
   WebSocket (remote clients). It does not know what a view is.
2. **`theoria-web`** — the tablet client. **Primary control and editing
   surface**, and a complete instrument interface on its own: a tablet-only
   user gets the full experience — grid, encoders, transport, editors — with
   no other hardware. When a Launchpad is present the two work in tandem as
   peers (an additional screen, not a replacement).
3. **`theoria-term`** — the terminal client. The existing `paraclete-tui`
   remains as-is through W0–W2; once the protocol stabilizes it is ported to
   an Antiphon client over the in-process transport, becoming a peer of the
   web client rather than a privileged in-process reader. The terminal is a
   first-class surface of this instrument permanently — parity target: param
   pages, grid view, and transport, rendered in the terminal idiom. Deferred,
   never dropped.

Real hardware already speaks this architecture's language — `SurfaceEvent`,
`SurfaceDescriptor`, `LedUpdate` — and Antiphon adopts that vocabulary rather
than inventing a parallel one. **One description language for every surface,
physical or drawn.**

**Explicitly out of scope:** audio in the browser. Surfaces control; sound
stays in the Paraclete process.

---

## Nodes All the Way Down (the honest version)

The cellular claim (ADR-018) holds as follows:

- **Every surface manifests in the graph as a device node.** A connected
  Theoria client is represented by a `Surface` node (dynamic
  `SurfaceDescriptor`), with its own `ScriptingGatewayNode`, exactly like
  `LaunchpadNode` or `DigitaktMidiNode`. Profiles cannot tell glass from
  plastic except by capability.
- **The Antiphon server is transport plumbing, not a platform object** — the
  same standing as `midir`'s callback threads or the TUI render loop. Its
  in-graph presence *is* the device/gateway nodes. This is the same exemption
  shape the scripting engine already has: environment, not node.
- **The semantic plane carries only what nodes already declare** (cap-doc
  params via ADR-019 commands, declared node commands). No web-only or
  terminal-only side doors into the engine.

So: surfaces are nodes, hardware is nodes, views bind to cap-docs, and the one
non-node piece is a message pipe. That is the architecture's existing shape,
extended — not an exception carved into it.

---

## Decision Point 1 (→ ADR-031): Antiphon architecture & transport

**WebSocket + in-process channel; NOT Web MIDI.** Bare MIDI cannot carry
cap-docs, state mirrors, or context — the bidirectional editor is impossible
over it. (An optional Web MIDI *input adapter* may come later at W4.)

```
theoria-web (tablet)      theoria-term / CI driver
      │ WebSocket                │ in-process channel
      ▼                          ▼
┌─────────────────── Antiphon server ───────────────────┐
│ session mgmt · state mirror (30 Hz coalesced diffs)   │
│ cap-doc snapshots · context feed · command validation │
└──────┬─────────────────────────────────────▲──────────┘
       │ rtrb SPSC (per client-surface)      │ tick()/subscriptions (main thread)
       ▼                                     │
  device node + ScriptingGatewayNode  ◄── profiles (launchpad.rhai serves
  (one per surface, ids 106+/113+)         glass and plastic alike)
```

- **New crate `paraclete-antiphon`** (GPL3, platform crate like
  `paraclete-tui`; the device-node side links L2 only per ADR-022).
- **Threading:** no tokio. Blocking `tungstenite` accept/read threads + `rtrb`
  SPSCs (the `midir` pattern); `tiny_http` serves the embedded `theoria-web`
  bundle so the instrument stays one binary. In-process transport is the same
  message types over an internal channel — zero cost for the terminal.
- **Two planes, JSON first** (binary only if profiling demands):
  - **Surface plane** — device-shaped, profile-routed:
    `{"ev":"pad_down","id":13,"vel":96}` → `SurfaceEvent::PadPressed` →
    gateway → Rhai. Grid touches use the P9.5 control-id map (0–63 grid,
    64–71 scene, 72–79 control row); touch encoders use ids 90–97 and emit
    **relative deltas only** — a drag *is* a delta, which is the
    `CMD_BUMP_PARAM` contract the instrument mandates.
  - **Semantic plane** — ADR-019-shaped, for editors:
    `{"cmd":"bump_param","node":20,"param":"cutoff","delta":0.01}` →
    `NodeCommand`, name-resolved and cap-doc-validated. Sequencer ops use the
    existing command vocabulary (`CMD_TOGGLE_STEP`, conditions, P10's 28–32).
- **Downstream:** `hello` (device map, live node list with `type_tag` +
  cap-doc snapshot), `led` batches (the same RGB feed the Launchpad gets),
  `state` diffs, `context`, `topology` (after `apply_patch` — clients bind to
  nodes **by discovery, never by hard-coded id**).
- **Multi-client:** tablet + terminal + phone concurrently; shared state
  mirror; input is last-writer-wins (two hands on two boxes). Session token in
  the URL; LAN-bind only. A stage/studio tool, not an internet service.

---

## Decision Point 2 (→ ADR-032): Theoria — views as an extension layer

Three strata, mirroring the platform's own L2 boundary:

1. **Core runtime** (`web/packages/core`, transport-agnostic TS) — connection,
   state store, command dispatch, gesture library (pad, XY, relative encoder
   with fine/accel modifiers, hold-to-lock). Versioned with the protocol.
   Structured for extraction and permissive licensing from day one (settled in
   ADR-032) so third-party clients are invited, echoing the LGPL3 L2 boundary.
2. **Generic views** — generated, never hand-built per node:
   - **Parameter pages** — a node's cap-doc params grouped into pages of 8
     (grouping from a `paraclete.ui.pages` extension hint when present, else
     automatic). Because ADR-019 names are canonical, one page view serves the
     sampler, FM, analog model, every effect, **and every future engine
     (wavetable) with zero client changes** — a new engine ships and its pages
     just appear. This page-per-aspect editing workflow will be familiar to
     anyone who has played a modern groovebox; we name the feature *pages*,
     nothing else (see naming policy).
   - **Grid view** — 8×8 + scene + control surface.
   - **Chain view** — effect nodes discovered via `"paraclete.effect"`;
     wet/dry/bypass; chain order (read-only until inner-graph patching).
3. **View plugins (the extension layer)** — modules registered against
   `type_name` or a cap-doc extension id, augmenting or replacing the generic
   view: waveform/slice view for `Sampler`, operator view for `FmEngine`,
   pattern overview for `Sequencer`. Plugins get the core-runtime API only —
   they cannot invent protocol. First-party plugins use the same loading
   mechanism third parties would.

`theoria-term` consumes the same strata: core runtime (Rust port or shared
protocol types), generic views rendered as ratatui widgets, and a terminal
plugin registry. Views are *specifications bound to cap-docs*; the renderer is
the client's business. That's what makes both clients thin.

**Usability floor (W1 onward, non-negotiable):** latched vs momentary visible
on every control; ≥ 44 px touch targets; a fine-adjust gesture on every
encoder; no gesture that requires looking away from the playing surface; a
dead socket must *look* dead (staleness indicator), never silently absorb
input.

**Client stack:** TypeScript + Vite, Preact (or Solid) for chrome, **canvas**
for grid/encoders (60 fps sweeps, multi-touch pointer events). PWA manifest
for full-screen. Bundle embedded in the Paraclete binary; no CDN, no install.

---

## Naming & Trademark Policy (public repo)

Third-party marks — Elektron, Digitakt, Digitone, Ableton, Push, Novation,
Launchpad, Arturia, Keystep — appear in this repository only as:

1. **Factual device support** (`LaunchpadNode`, `DigitaktMidiNode`, protocol
   docs for hardware we integrate with) — standard interoperability naming.
2. **Sparing analogies in design prose** ("the workflow popularized by…"),
   as `architecture-core.md` already does.

Never: in our feature names, view names, UI strings, crate names, commit
headlines, or anything that could read as claiming affiliation or cloning
trade dress. House vocabulary instead — *pages*, *plates*, *grid*, *chain*,
*Antiphon*, *Theoria*. Existing docs already comply (analogy usage); new docs
and **all code identifiers** hold this line. What we build is a page-based
parameter workflow on our own architecture — describe it as ours.

---

## New project or in-repo?

**In-repo.** `crates/paraclete-antiphon` + `web/` (TS workspace:
`packages/core`, `packages/app`). The protocol churns violently through W0–W2;
a separate repo adds version skew for zero benefit at one developer and one
consumer. Extraction of `packages/core` (and its permissive license) is a
folder move later because it's structured for it now.

---

## Phases

### W0 — POC: the grid on glass *(next after P10 C0; ~1 commit scale)*

Prove the transport and the peer-device claim, nothing else.

- `paraclete-antiphon` crate: ws thread, static server, surface device node
  (P9.5 80-control descriptor), gateway id 113, app wiring. The server core is
  minimal but **shaped for the in-process transport from day one** (message
  types defined transport-free) — that's the cheap insurance that keeps the
  terminal-client promise.
- `theoria-web`: single canvas page — grid + scene + control, touch →
  surface-plane events, LED batches → pad colors, transport indicator.
- **Exit criteria:** on a tablet, toggle steps on all 8 tracks of the running
  instrument while the Launchpad (or terminal emulator) is connected; both
  surfaces show the same LEDs; the unmodified `launchpad.rhai` serves both;
  touch→LED round-trip measured and logged (< 30 ms target on LAN).

### W1 — MVP: a playable surface → paired session #1

- **Touch encoder row** (ids 90–97): drag = relative delta → `CMD_BUMP_PARAM`;
  fine gesture; name/value/unit rendered in the control from `/context/*` +
  `/node/{id}/{param}`.
- Transport + track header (play/stop, BPM, track select mirrored both ways).
- State mirror v1 (subscribe/coalesce/30 Hz) + semantic plane with cap-doc
  validation.
- Reconnect, staleness UI, session token, PWA manifest.
- **Tablet-only parity check:** program, tweak, save, reload a beat with *no
  physical controller attached* — this is now an explicit exit criterion.
- **Exit criteria:** the contextual flow (select kick → encoders are kick
  params) works end-to-end on glass; values live-update under sequencer
  p-locks; usable both tablet-only and alongside hardware.

### W2 — The editor: mapped views for every engine → paired session #2

- Cap-doc-driven **parameter pages** per track/node (page tabs, 8 params per
  page, bidirectional), **chain view**, and the **view-plugin registry** with
  two showcase plugins: Sampler (waveform, root note, start/end) and FmEngine
  (operator/macro view). The ADR-032 API freezes here.
- **Exit criteria:** every current engine and effect fully editable from the
  tablet with no per-node client code beyond the two plugins; a node added to
  the instrument YAML appears with working pages, zero client changes.

### WT — Theoria/term: the terminal comes home *(after W2; parallel to W3)*

- Port `paraclete-tui` to an Antiphon client over the in-process transport:
  same state mirror, same command planes, no socket.
- Terminal renditions of the generic views: parameter pages, grid view,
  transport — the TUI stops being display-only (delivers the P16 "TUI as
  editing surface" goal early, through shared machinery instead of bespoke
  code).
- `paraclete-tui`'s direct-StateBus path retires only when parity is proven.
- **Exit criteria:** the W1 tablet-only parity check passes terminal-only.

### W3 — Sequencer deep views *(hard dependency: P10 C2–C5; parts on P11)*

The full-depth sequencing surface — deliberately after P10 because patterns,
pages, cueing, chains, and per-track length don't exist in the engine until
then.

- 64-step pattern view with page tabs + page-loop selection; pattern bank
  (select / cue-blink / chain); per-track length & speed.
- **Hold-step editor:** p-lock overlay (bump params while held), trig
  condition + probability + micro-timing editors.
- Mute view when P11 lands.
- **Exit criteria:** program the vision's "A Session" walkthrough entirely
  from the tablet; everything survives save/load (P10 serializer v3).

### W4 — Extension maturity *(ongoing)*

Ordo (layout/workflow profiles UI), multi-client polish, theming, macro views
(pairs with P16), wavetable view when that engine exists, optional Web MIDI
input adapter, protocol v1 freeze + `packages/core` extraction/licensing call,
headless protocol driver for CI (the W-track's end-to-end test harness).

---

## Risks & open questions

| # | Risk / question | Mitigation / where decided |
|---|---|---|
| WQ-1 | Wi-Fi jitter on stage (tail latency under congestion) | Wired USB-C-Ethernet / dedicated-AP setups documented; measured in W0; finger-drumming latency is explicitly not a design target |
| WQ-2 | No pressure/velocity on most tablets | Velocity from strike position or per-view setting; `PadPressure` absent from the web surface descriptor — profiles already branch per capability |
| WQ-3 | Two sources of truth during rapid bumps (bank vs mirror) | Optimistic deltas reconciled against the 30 Hz mirror; relative-only means drift self-corrects |
| WQ-4 | StateBus fan-out with a full editor subscribed | Mirror subscribes to the union of client-visible paths only; coalesced diffs; BUG-006/007 triggers fire if churn shows up in W1 profiling |
| WQ-5 | Hard-coded node IDs vs dynamic topology | `hello`/`topology` carry the live node list; Theoria binds by discovery, never by constant |
| WQ-6 | Rhai dispatch throughput under a chatty touch surface | Client coalesces encoder deltas per frame; surface events are Copy-cheap through the SPSC |
| WQ-7 | `packages/core` license (invite third-party clients?) | ADR-032 |
| WQ-8 | Semantic plane vs profiles (does the editor bypass Rhai too much?) | Position: ADR-019 generic param commands are a platform contract; profiles own *hardware* semantics. Revisit at ADR-031 review |
| WQ-9 | Terminal parity scope creep (WT ballooning into a second full client) | WT scope = generic views only via shared machinery; terminal-specific plugins are W4+; parity check is the W1 checklist, not the W3 one |

---

## Dependency & new-crate summary

- **New crate:** `paraclete-antiphon` (GPL3, platform crate; device-node side
  links `paraclete-node-api` only).
- **New dir:** `web/` — TS workspace (`packages/core`, `packages/app`); not a
  cargo member.
- **New Rust deps:** `tungstenite` (blocking WS), `tiny_http` (static
  serving), `serde_json` (protocol) — MIT/Apache; **no tokio**.
- **Client deps:** vite, typescript, preact (or solid) — dev-time only;
  runtime bundle self-contained, embedded in the binary.
- **New device ids:** 106+ per connected Theoria surface, 113+ gateways;
  injected constant `THEORIA_DEVICE_ID` for the first/primary surface.
