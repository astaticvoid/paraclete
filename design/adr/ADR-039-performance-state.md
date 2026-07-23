# ADR-039: Performance State — Kits, Temp Save, Perform Mode, Mute Tiers, Live Record

| Field | Value |
|-------|-------|
| **Status** | 🟡 Proposed (2026-07-23) |
| **Author** | Agent (drafted at user request) |
| **Ratification** | Pending user |
| **Scope** | P11: app layer (new), `paraclete-nodes/sequencer.rs`, project file format (ADR-025 revision), serializer v4 |
| **Related** | ADR-010 (state model), ADR-018 (environment exemption), ADR-019 (semantic plane), ADR-025 (project format), ADR-030 (pattern engine — declared P11 layers on it), ADR-038 (KIT button, REC grammar); `design/phases/p11-problem.md` |

---

## Decision

**Performance state is modeled in two tiers with two owners:** state that
lives *inside one node* (a pattern's shadow copy, mute application, live
record) is engine-side via new sequencer commands; state that *spans
nodes* (kits, perform mode, temp-save orchestration) is **app-owned
environment state** mutated through a new app-op drain, with all node
mutation fanning out through the existing ADR-019 command plane.

Seven concrete decisions:

1. **A kit is a named set of `(node_id, param_id, value)` entries over
   the instrument's declared bank params** — captured by reading the
   state bus (`publish_bank_state` already mirrors every declared param),
   applied by replaying `CMD_SET_PARAM`. **Zero engine changes for
   capture or apply.** Kit scope is **whole-instrument** (all tracks —
   the Elektron kit shape); per-track extraction is a later refinement.
   Kits live in an app-owned `KitStore`, persisted as a new top-level
   `kits` section in the project RON (ADR-025 format version bump;
   unknown-section-tolerant loaders unaffected).

2. **Patterns bind to kits by global pattern slot.** The app holds
   `kit_binding: [Option<KitId>; 8]` (slot-indexed, matching
   `PATTERN_BANK_SIZE`), persisted with the project. On pattern
   activation the app applies the bound kit — **unless Perform mode is
   on**. Today's engine behavior (switch changes steps, params carry
   over) is exactly Perform-mode-on; the default with a binding present
   becomes kit-apply. Unbound slots behave as today.

3. **Perform mode is an app flag**, published to `/context/perform` for
   surface display. On: pattern switches skip kit apply (sound evolves
   across switches). Gestures: **reload** re-applies the bound kit of
   the active slot; **commit** captures current param state into that
   kit. Both are app-ops.

4. **Temp save/reload is a paired volatile snapshot:** *(a)* engine-side,
   per sequencer — `CMD_TEMP_SAVE` clones the active `Pattern` into a
   pre-allocated shadow slot, `CMD_TEMP_RELOAD` restores it (ADR-030
   anticipated exactly this: "a snapshot is a clone of the active
   `Pattern`"); *(b)* app-side — a volatile kit-shaped param snapshot
   taken at the same moment. One app-op (`TempSave` / `TempReload`)
   broadcasts the command to every sequencer and snapshots params
   together. RAM-only; never serialized; the project serializer and its
   dormant race (bugs.md #2) stay untouched. The shadow `Pattern` is
   allocated at node build time — the command-time clone is a copy into
   existing capacity, no audio-thread allocation.

5. **App-op channel.** Surfaces gain a second drain alongside
   `take_pending_commands()`: `take_pending_app_ops() -> Vec<AppOp>`,
   `AppOp = { TempSave, TempReload, KitLoad(KitId), KitSaveAs(name),
   KitCommit, KitReload, BindKit(slot, KitId), SetPerformMode(bool) }`.
   The app main loop executes them (fan-out to `NodeCommand`s + bus
   reads/writes) exactly where it already drains scripting/Antiphon
   queues. This is the ADR-018 environment exemption doing what it
   exists for — project load already orchestrates multi-node state from
   the app; app-ops are its runtime-shaped sibling. Antiphon exposes the
   same verbs over the protocol in W-track scope (not this ADR).

6. **Three mute tiers, engine-applied:**
   - **Global mute** — the existing sequencer `mute` bank param,
     unchanged (persists across patterns, saved with the bank).
   - **Pattern mute** — new `muted: bool` on `Pattern` (serializer v4
     trailing field), toggled by a new sequencer command; effective mute
     is `global OR pattern`, evaluated in the existing `is_muted()`.
   - **Prepared mutes** — the mute-setting commands gain a defer flag in
     `arg0`: the sequencer holds the change and applies it exactly at
     the next pattern wrap. Per-node and sample-deterministic; all
     tracks drained in one main-loop batch share the same boundary.
     (Surface-side boundary polling was rejected — race-prone.)

7. **Live record is engine-side in the sequencer.** A new `live_rec`
   bank param (trig-gate like `mute`); while ≥ 0.5 and transport
   playing, incoming **Midi2 note-ons on `events_in`** (today consumed
   for `Transport` only — additive match arm) are quantized to the
   nearest step of the active pattern: step activated, note + velocity
   written, and the signed distance to the grid captured as the step's
   micro-timing from the event's sample offset (sample-accurate,
   requirement 4). A pending `CMD_TRIG_NOW` (TK2 C1) records itself the
   same way when `live_rec` is on — Theotokos REC+PLAY needs no extra
   path. Keystep→sequencer is instrument-YAML wiring into the existing
   `events_in` port, no new ports.

**Command numbering:** TK2 took 38 (`CMD_TRIG_NOW`). **This ADR reserves
39–45 for the P11 sequencer family** (temp save/reload, pattern mute,
prepared-mute variants, live-record adjuncts); exact assignments freeze
in `p11-interfaces.md`.

## Alternatives considered

- **Kits as full `NodeSnapshot` blobs (serialize/deserialize)** —
  rejected: blobs capture *all* private state (pattern data, phase),
  are opaque and version-coupled; a kit must be sound-only, diffable,
  and portable across pattern edits. Params-only via the declared bank
  is precisely the "structured state vs observable values" split
  ADR-010 already draws.
- **A KitNode owning kits inside the graph** — rejected: nodes cannot
  command other nodes (ADR-018's cellular rule); a kit's whole job is
  writing other nodes' params. This is environment work, same standing
  as project load, scripting drain, and Antiphon.
- **Temp save via the project serializer to a temp file** — rejected:
  heavyweight, hits the dormant ADR-025 live-save race (bugs.md #2),
  and persists what the vision explicitly wants volatile.
- **Surface-side prepared mutes (send at boundary, polled from the
  bus)** — rejected: the ~1 ms UI tick cannot guarantee the downbeat at
  high BPM ÷ short patterns; per-node defer flags are deterministic.
- **Live record surface-side (compute step from bus, send
  `CMD_SET_STEP`)** — rejected: micro-timing from a UI tick is
  quantization noise; the engine sees the true sample offset.

## Consequences

- **New:** app `PerformState` (KitStore, bindings, perform flag, temp
  param snapshot); app-op drain trait/pattern for surfaces; `kits` +
  `kit_binding` project sections; `/context/perform` bus path;
  sequencer serializer **v4** (pattern `muted` trailing field).
- **Engine:** sequencer gains temp-save shadow (pre-allocated), pattern
  mute, defer-at-boundary mute application, `live_rec` param + Midi2
  note-on consumption. All additive; CMD 39–45 reserved.
- **Surfaces:** Theotokos KIT screen (ADR-038 reserved button) becomes
  the kit UI; REC+PLAY = live record arms `live_rec`; FUNC+YES/NO are
  natural temp save/reload homes *(hypothesis — session-tested)*.
  Antiphon/Theoria and profile verbs follow in W-track scope.
- **Unblocks:** TK2 leftovers (ALT-layer mutes/fills against real mute
  tiers, temp save/reload), TK3, W3's mute view, the vision's §Session
  narrative end-to-end.
- **Open questions for `p11-interfaces.md`:** kit count/naming UX; kit
  diff display; whether pattern mute participates in `publish_bank_state`
  or a state path; live-record erase gesture (hold NO?); count-in/
  metronome (likely P12); per-track kit extraction.

## Implementation note (to be added when ratified)

```text
ADR-039 implemented across P11 (YYYY-MM-DD).
See design/phases/p11-interfaces.md for the commit plan.
```
