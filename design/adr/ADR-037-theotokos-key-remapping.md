# ADR-037: Theotokos Key Remapping

| Field | Value |
|-------|-------|
| **Status** | Proposed (2026-07-22) |
| **Author** | Agent |
| **Ratification** | Pending user |
| **Scope** | `paraclete-theotokos` crate |
| **Related** | ADR-036 (Theotokos), ADR-019 (semantic plane), TK1/TK2 phase specs; `design/theotokos/design.md` §3, §4 |

---

## Decision

**Theotokos gains a runtime-remappable key binding system (`Keymap` struct)
in TK2 (performance layer), not TK3 as originally scoped.** A
`HashMap<(Option<Mode>, KeyBinding), Action>` backs user bindings; lookup
order is mode-specific → global → hardcoded defaults. Remapping is done
through the `:` command line (TK1 C6) with verbs `:bind`, `:unbind`,
`:reset-bindings`, `:save-bindings`, `:load-bindings`. Persistence uses
YAML (`serde_yml`) at `~/.config/paraclete/keymap.yaml` with local
`./keymap.yaml` override.

The detailed spec lives in `design/phases/tk2-theotokos.md` (owned by the
design agent).

---

## Context

Theotokos's keymap is entirely hardcoded: two static arrays (`TRACK_KEYS`,
`STEP_KEYS`) and three pure functions (`map_global`, `map_seq`, `map_perf`)
in `input.rs`. There is no mechanism to change bindings at runtime, and no
persistence of user preferences across sessions.

Three forces:

1. **Physical keyboard diversity.** A 60% keyboard has different reach than
   a full-size; an AZERTY layout would scramble the home-row step grid.
   Drummers, pianists, and programmers have different muscle memory.

2. **The original roadmap scoped "keymap config file" to TK3 (2026-07-21):**
   `design.md:432-433` listed `keymap config file (Ordo-adjacent, plain-data
   keymap)` under TK3's convergence phase. TK3 is the final Theotokos phase,
   gated on three usability sessions and a WT convergence decision — users
   would wait through TK1 *and* TK2 before they can remap anything.

3. **The mechanism is simple and self-contained.** A `HashMap` lookup, a
   YAML file, and 5 `:`-line verbs. No new crate dependencies, no
   audio-thread concerns, no cross-crate extraction. It needs only the `:`
   command line (TK1 C6) and file I/O (stdlib). The `map_key()` test seam
   (`handle_keys` key-injection) already exists.

The question is whether this belongs in TK2 (performance layer) or TK3
(breadth/convergence). The answer turned on one criterion: **is key
remapping a performance customization or a convergence/integration feature?**

---

## Alternatives considered

### A. Keep keymap config in TK3 as originally scoped (status quo)

**Rationale:** the original design deferred all config-file concerns to TK3;
the `:` line was a TK1 upgrade (from stub to full); mutes were pulled
forward to TK1 because they were a basic performance blocker (session #1).
Key remapping is not a blocker.

**Rejected.** Key remapping is a performance customization — the same class
as ALT-layer mutes/fills and temp save/reload (both TK2). The `:` line
already provides the entry point in TK1. Deferring to TK3 means users go
through TK1 + TK2 without being able to customize the keyboard to their
hands — they might build muscle memory on keys they later want to change.

### B. Ship key remapping in TK1 as a C9 commit

**Rationale:** the `:` line is built in TK1 C6; the mechanism is ~200 lines
of HashMap + serde_yml; it could ship alongside the `:` line with minimal
scope creep.

**Rejected.** TK1 is already an 8-commit spec with a new crate extraction
and 5 new sequencer commands. Adding remapping to the same phase would
overload the usability session (session #2 covers p-locks, mutes, `:`
verbs, patterns, paste, yellow-flash, and leader rebinding — adding "try
remapping your keys" to that session dilutes every other hypothesis). TK2
is the correct phase boundary: the user arrives with TK1 muscle memory
established, and TK2 gives them tools to customize it.

### C. Key remapping in TK2, Ordo profiles + WT editor in TK3 (chosen)

**Rationale:** the core mechanism (HashMap + `:` verbs + YAML file) is a
performance customization — it needs no new architecture, no cross-crate
work, and no convergence decisions. The Ordo-adjacent profile switching,
guided remap wizard, and WT convergence editor are properly TK3 scope
(gated on session evidence and the WT decision).

This mirrors the mutes precedent: mutes were pulled from TK2 into TK1
because session #1 identified them as a basic performance blocker. Key
remapping is the analogous pull from TK3 into TK2 — a performance
customization the mechanism already supports.

---

## Consequences

### Positive

- Users can remap keys at runtime by TK2 (two sessions after Theotokos
  ships, not three).
- The `:` command line gets a coherent verb family (`:bind`/`:unbind`/
  `:save`/`:load`/`:reset`) that demonstrates its extensibility.
- The Keymap struct is testable in isolation (key-injection seam +
  roundtrip YAML tests) — no audio-thread concerns, no crossterm coupling.
- The `map_key()` function signature change (`&Keymap` parameter) is small
  and mechanical — one call site (`lib.rs:handle_keys`).
- AZERTY/QWERTZ users, 60%-keyboard users, and left-handed users get
  customization two phases earlier than planned.

### Negative

- `map_key()` is no longer a pure function (it reads the `Keymap` HashMap).
  This is a one-line diff in the test seam (tests that inject `&[KeyEvent]`
  need an empty `Keymap`). The function remains deterministic and
  allocation-free on the hot path (HashMap lookup with no binding → direct
  fallthrough).
- One more file format to document in user-facing docs (the YAML schema).
  Mitigated by making `keymap.yaml` flat and self-describing; no new
  serialization infrastructure (we already depend on `serde_yml`).
- The `:` command line's fuzzy index grows by 5 entries. The index is
  generated — no maintenance burden.

### Risks

- **Binding the leader key (`\`) creates a chicken-and-egg problem.** If the
  user rebinds `\` (the TK1 C7 leader) and then needs `:` to fix it, both
  entry points are gone. The design agent must define a "safe reset" escape
  hatch (e.g., `Ctrl+\` always opens `:`, or `--reset-keymap` CLI flag).
  Tracked as OQ-T18 in the TK2 spec.
- **Parsing action-with-parameters from the `:` line is a new grammar
  concern.** `Jog { slot: B, dir: Next, mag: Normal }` needs a one-liner
  representation. Tracked as OQ-T15 in the TK2 spec.
- **Step/track key arrays vs Action-level bindings.** If a user rebinds
  `ToggleStep { col: 0 }` from `a` to `1`, the positional grid shifts — is
  that intuitive? The design agent should decide whether the keymap operates
  at the Action level (individual `ToggleStep` bindings) or at the array
  level (remap the `STEP_KEYS` positions). Tracked as OQ-T19.

---

## Implementation note (to be added when ratified)

```text
ADR-037 implemented as part of TK2 (YYYY-MM-DD).
See design/phases/tk2-theotokos.md for the full spec.
See [commit hash] for the implementation.
```
