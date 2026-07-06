# Paraclete — W1 Implementation Report (Theoria MVP)

> **Append-only.** One section per commit, written as each ships.
> Spec: `design/phases/w1-interfaces.md`. Ends in paired session #1 (C5).

---

## Commit 0 — `CMD_TRIGGER` (universal) + velocity plumbing — shipped `53caf77`

### What shipped

- **`CMD_TRIGGER = 19`** added to `paraclete-node-api` (`command.rs`) and
  re-exported. Universal instrument command: `arg0` = note (`< 0` → default/
  last note), `arg1` = velocity `0.0–1.0` (`<= 0.0` → `0.79`). Same retrigger
  path as a `NoteOn`. Implemented in `AnalogEngine`, `FmEngine`, `Sampler`;
  other nodes ignore it (unknown-command silence).
- **Sampler** already had a local `CMD_TRIGGER = 19` (hardcoded
  `trigger_voice(60, u16::MAX/2, 0)`); it is now an alias of the shared const
  and parses `arg0`/`arg1` for real, defaulting the note to `root_note`.
- **Velocity plumbing (universality sweep):** `retrigger(note)` →
  `retrigger(note, velocity)`. A `NoteOn`'s 16-bit velocity maps `vel/65535`
  → `0..1`; that becomes a linear output-level multiplier (`velocity_level`),
  applied allocation-free at the output copy (mono engines) / per-voice
  (Sampler). Full velocity = unity gain = pre-W1 level. `Sampler::Voice`
  gained a `velocity_level` field; the two mono engines gained
  `velocity_level` + `last_note`. Velocity-mod routing is P12+ — only the
  plumbing landed.

### Behavior change to flag for the paired session (design tiebreaker)

Velocity now scales output for **all** `NoteOn`-driven playback, including
sequencer steps — which previously ignored velocity entirely. Consequence:

- The default 8-track preset (`apply_preset`, velocity `40_000` ≈ 0.61) now
  plays ~4 dB below its pre-W1 level.
- `Step::empty()`'s default velocity is `32768` (≈ 0.50); any step programmed
  without an explicit velocity plays at half level.
- A `CMD_TRIGGER` with default velocity uses `0.79`, so a live-triggered pad
  is louder than a default sequencer step — an asymmetry worth a decision.

This is **spec-mandated** (§Commit 0: `vel/65535`, `CMD_TRIGGER` default
`0.79`), so it shipped as specified rather than being redesigned inline
(guardrail 1). The open question for session #1: should the default step
velocity rise (e.g. to ~52000 ≈ 0.79 for parity with `CMD_TRIGGER`), or is
per-step velocity control the intended way to reach full level? Recorded here
so it is decided with the instrument in hand, not guessed.

### Found while here (filed, not fixed)

- **BUG-015** — `FmEngine` routes `ParamLock` events through
  `bank.handle_commands()` (converts each into a synthetic `CMD_SET_PARAM`),
  the exact anti-pattern CLAUDE.md's ADR-019 ParamLock section forbids: the
  locked value permanently mutates the bank and bleeds into later steps.
  `AnalogEngine` uses the correct `node_locks` pattern. Pre-existing; left
  untouched to keep C0 scoped. Fix direction in the tracker.

### Gate results

- `cargo test --workspace`: **450 passed, 0 failures** (441 baseline + 9 new).
- New tests (per engine): `cmd_trigger_produces_audio`,
  `cmd_trigger_negative_note_uses_default`, `velocity_scales_output_level`.
- `cargo clippy -p paraclete-nodes -p paraclete-node-api --all-targets`: no
  new warnings vs baseline (verified by A/B stash; the one `single_match` hint
  near `trigger_voice` is the pre-existing `NoteOn` arm, line-shifted).
- Scope: `paraclete-node-api` (const + re-export) and the three engine files
  only.

### Review

Direct orchestrator read of the full ~250-line diff plus independent
test/clippy verification (proportionate to a small, well-understood mechanical
commit; the 8-angle fan-out was reserved for the gated P10 C1 data-model
change). Verified: consistent velocity math and clamping, allocation-free
output scaling, sensible defaults, state reset in `activate()`, valid tests.
No correctness issues found; BUG-015 filed as a pre-existing neighbor.
