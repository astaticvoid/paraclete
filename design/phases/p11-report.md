# P11 — Live Performance: Implementation Report

> **Phase:** P11 — Live Performance (milestone #17). **Spec:**
> `design/phases/p11-interfaces.md`. **Authority:** ADR-039 (with §0
> amendments). **Status:** complete — all commits landed, milestone
> closed 2026-08-03. **Companion docs:** this report is append-only.

## What shipped (per commit)

| Commit | Piece |
|--------|-------|
| `cc579ec` | C0 — `ParamDescriptor.in_kit` flag + exhaustive audit |
| `fc40f3d` | C1 — AppOp vocabulary + main-loop drain |
| `89010b6` | C2 — KitStore, PerformState, capture/apply, pattern-switch kit-apply, project v3 |
| `7e05d98` | C3 — temp save/reload (sequencer shadow + param snapshot) |
| `e49d486` / `afc5383` | C1–C3 unit + harness tests (mutation-checked) |
| `1580114` + `d7162e3` + `9cf3805` | C4 — mute tiers (pattern mute, prepared mutes, v3 trailing byte) |
| `f8eee54` + `4f32bbd` + `02cece2` + `f6c3611` | C5 — Midi2 live record, `live_quantize`, Keystep routing, HAL arrival timestamps |
| `7383be7` + `d16b14f` | C6 engine — CMD_LIVE_ERASE=44, `/context/kits` + `/context/kit_binding` |
| `da91c50` + `8b67549` + `38bb7a3` + `85d66d7` | C6 surfaces — KIT screen, ⚡ perform, temp chords, pattern-mute chords, hold-NO live erase + harness |
| `6ad7e0f` + `1afaae7` | C7 — Antiphon verbs, Theoria KIT tab, KitCommit/KitReload |
| `8b67549` etc. | harness verbs: `set_pattern_mute`, `prepare_mute`, `prepare_pattern_mute`, `clock_rewind`, `midi2_note_on`, `live_erase`; Bool `eq` assertions |

## Open-question resolutions (all four closed)

- **OQ-12 / #76 (live-record quantization)** — user decision 2026-08-02:
  **hard-quantize control**. `live_quantize` stepped param on the
  sequencer (off / 1/4 / 1/8 / 1/16 / 1/32); off = record-as-played with
  micro-timing (prior behavior), a note-value = the recorded step snaps
  to that grid with zero micro-offset. Grid formula
  `TICKS_PER_BEAT*4/denom` — the naive `/denom` form was caught by a unit
  test during authoring.
- **OQ-T25 / #137 (live-record erase gesture)** — user decision
  2026-08-02: **Elektron-style live erasing** — hold NO while the
  transport plays erases each step as the playhead passes it (the erased
  step does not sound). Engine `CMD_LIVE_ERASE=44`; Theotokos Grid-gated
  hold-NO arm/disarm (kitty release; sticky fallback relies on the engine
  stop-disarm — documented).
- **OQ-T11 / #136 (temp save/reload scope)** — moot: C3 shipped the full
  engine+app scope (pattern shadow + param snapshot in the same tick);
  the C6 FUNC+YES/NO chords followed.
- **OQ-T29 / #122** — duplicate of OQ-12; closed with it.

## Test trifecta status

Every functionality commit carried unit tests; harness live tests
(`p11_mute_tiers.yaml`, `p11_live_rec_midi2.yaml`, `p11_live_erase.yaml`
plus the C1–C3 trio) were mutation-checked in both directions before
being trusted; the autonomous real-app live sessions covered the surface
gestures (C2's pattern-switch kit-apply, 2026-08-02; C6's KIT
screen/perform/temp/pattern-mute/live-erase, 2026-08-03 in kitty; C7's
verbs probed over the raw WebSocket against the running graph). All seven
ADR-035 baselines stayed green after every engine change.

## Deviations and notes (boring choices, recorded)

1. **C4 app_ops.rs** — the spec's "surface→command mapping for pattern
   mute" lives directly in the Theotokos command path (NodeCommands via
   `take_pending_commands`), not via AppOps — the mute is a per-track
   sequencer command, not an app-level op.
2. **C5b arrival timestamps** — best-effort as specced (≤ 1 block of
   jitter); the sequencer's own tick-based micro-timing supersedes the
   event offset for the recorded step's micro value (the offset refines
   `pos`, and the ADR's "micro from the event's sample offset" is
   honored); true sample accuracy needs a JACK backend (ADR-039 Amd 2).
3. **C6 KIT-screen encoder-bank preview** — the bus publishes kit *names*
   only in P11, so the preview shows the selected kit's name in each cell
   (or `(empty)`), documented as a preview limitation; values would need
   per-entry publication.
4. **Live-erase sticky fallback** — arms on press only; no release event
   exists in non-kitty terminals; the engine disarms at transport stop.
   Grid-screen-only so the Chain screen's NO=clear-chain is never
   co-armed (review finding).
5. **`live_quantize` finer than the step grid** (e.g. 1/32 with 16th
   steps) degenerates to the step grid with micro 0 — the step is the
   finest writable unit (documented in `record_live_trig`).
6. **`deserialize_v3` positional trailing byte** — the first extra byte
   inside a pattern record is `muted`; with the blob version pinned at 3
   the field order is now a permanent ABI for v3 — a future per-pattern
   field must either append after `muted` or bump to v4 (review note;
   latent, no action taken).
7. **Keystep live-record routing** (C5a) needs the hardware (Keystep) to
   exercise end-to-end; the routing decision + rebind are unit-tested and
   the Midi2 consumption is harness-verified.

## Still open (filed, not P11 blockers)

- #184 — INFRA: surface AppOp production (Theotokos/Antiphon) is not
  headless-testable (the harness drives `PerformState` directly; the
  Theotokos app-op drain is covered by the autonomous live session).
- #180 / #173 / #165 / #163 / #158 / #147 / #140 / #139 / #138 and the
  remaining `OQ-T*` register — out of P11 scope (TK3 etc.).

## Milestone

Milestone #17 (P11 — Live Performance) closed 2026-08-03; all four
milestone issues (#76, #122, #136, #137) closed as resolved/moot/
duplicate in the commits that implemented them.
