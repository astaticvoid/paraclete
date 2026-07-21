# Praxis — Design (staged, append-only)

> **How this document grows.** The problem is fixed (`problem.md`). The
> design below is appended **aspect by aspect**, each marked:
>
> - **DETERMINED** — decided this session (pending ADR-036 ratification);
>   implementable.
> - **HYPOTHESIS** — a concrete proposal to be battle-tested in a usability
>   session; expected to change. Convergence is by evidence, not taste.
> - **OPEN** — unresolved; has an entry in the Open Questions table.
>
> A new session picks up by reading `problem.md` → this file top to bottom →
> the Open Questions table → the current PRX phase spec (when written).
> Stages are dated and never rewritten; corrections are appended with a new
> dated note. Per naming policy, third-party marks appear here only as
> design-prose analogy; never in identifiers.

---

## Stage 2 — 2026-07-21 — full first-pass design

Aspects 1–7 determined below in one pass (this session had a complete
scaffolding inventory to work from). Keymaps and chord grammar are
deliberately HYPOTHESIS-grade; architecture is DETERMINED-grade.

---

## §1. Positioning and name — DETERMINED

**Praxis.** `interface-plan.md` already fixed the house vocabulary: "The
views are theoria; the controls are praxis; a surface is both." Theoria is
the contemplative/editing surface family; **Praxis is the action/performance
surface**: the keyboard-first modal terminal. Crate: `paraclete-praxis`
(GPL3 platform crate, same standing as `paraclete-tui`).

Praxis targets **parity-plus with the web GUI for performance**: anything
you can perform from the tablet you can perform from the keyboard, faster.
It is *not* an editor replacement — deep editing stays on glass and on the
composition floor.

---

## §2. Architecture & hook-in — DETERMINED

### 2.1 The composability keystone: views are data + state + renderer

ADR-032 made a node's view a **serializable data contract** (`Rule`: page
groups, param pages, macros, affordances, envelope groups) living on the
cap-doc. The web client is its first renderer. **Praxis is the second** —
the proof that ADR-032 is the *universal node-view contract* rather than a
web feature:

```
        CapabilityDocument (+ Rule)        StateBusHandle
        ─────────────────────────  ×  ─────────────────────  =  a view
        "what this node is and            "what it is doing
         how it should look"               right now"
                │                                   │
        ┌───────┴───────────────┬───────────────────┴──────────┐
        ▼                       ▼                              ▼
  theoria-web renderer    praxis renderer (ratatui)      future: WT / hw screens
```

A new engine ships a `ViewPlugin` impl and gets a terminal view **for
free**. No terminal-only side doors: every mutation goes through ADR-019
(`CMD_SET_PARAM`/`CMD_BUMP_PARAM`) or the declared sequencer command
vocabulary (16–32) — the same semantic plane Antiphon exposes over JSON.
Praxis speaks it in-process, without JSON.

### 2.2 Process topology

Praxis is **environment, not a node** — the same ADR-018 exemption shape as
`paraclete-tui`, the scripting engine, and the Antiphon server (transport
plumbing ticked on the main thread). It is **not** a `Surface` node and
does **not** route through Rhai:

- **Why not a Surface/gateway/profile:** surface events exist so *hardware*
  semantics can live in profiles. Praxis's modal state machine is native,
  latency-critical, and must be unit-testable; Rhai adds indirection and
  buys nothing here. Profiles remain the hardware-surface mechanism,
  untouched.
- **Reads:** the shared `Rc<RefCell<StateBusHandle>>` + a cap-doc cache
  (the app already collects cap-docs at startup, main.rs step 7).
- **Writes:** `Vec<NodeCommand>` drained by the app each main-loop
  iteration → `conf.send_command()` — byte-for-byte the
  `scripting.take_pending_commands()` / `antiphon.drain_commands()`
  pattern. Key→command latency ≈ one main-loop tick (~1 ms).
- **Cross-surface awareness:** Praxis may read `/context/*` and `/script/*`
  and will publish its own `/script/praxis/*` scratch (e.g.
  `/script/praxis/selected` so other surfaces can follow its track
  selection) via direct `StateBusHandle` writes — in-process, not
  sandboxed. Reconciliation with Theoria is the platform-wide
  last-writer-wins model; no new rules.

### 2.3 Crate shape and dependencies

`crates/paraclete-praxis` — deps: `ratatui 0.26`, `crossterm 0.27` (both
already workspace-pinned), `paraclete-node-api` only. **No new
dependencies.** It never sees `NodeConfigurator` (layer-clean: app drains
its command queue, exactly like scripting).

Internal module split (this is where composability is enforced):

| Module | Role | Purity |
|---|---|---|
| `input` | `map_key(&ModeState, KeyEvent) -> Vec<Action>` | **pure** — no I/O, unit-tested without a terminal |
| `action` | `Action` enum (semantic intents) + `resolve(&[Action], &CapDocCache, &Model) -> Vec<NodeCommand>` | **pure** — mirrors Antiphon's `resolve_semantic` |
| `model` | view-model: subscribes state bus, holds mode/track/page/focus state, builds per-mode view state | main-thread, I/O at edges only |
| `render` | ratatui widgets per mode (canvas + braille) | `TestBackend`-testable |
| `lib` | `PraxisApp { model, pending: Vec<NodeCommand> }`, `tick(&mut Terminal, &StateBusHandle, now_ms)`, `take_pending_commands()` | same tick pattern as `TuiApp` |

### 2.4 Terminal ownership — main thread, exclusive

Praxis owns raw mode, the alternate screen, and keyboard input **on the
main thread**. The Launchpad emulator currently polls crossterm *inside
`process()` on the audio thread* — a known defect Praxis must not copy.
Keyboard enhancement flags (kitty protocol: press/release/repeat
distinction) are enabled when the terminal supports them, with a
repeat-event fallback when not (OPEN — OQ-P6).

Startup flags: `--praxis` enables; it implies `--no-emulator` and skips the
old TUI (one owner of the screen). `paraclete-tui` is left in place during
the track; retirement is a later user decision (ADR-036 §Alternatives).

### 2.5 What is reused (no wheel reinvention)

| Existing piece | Reuse |
|---|---|
| ADR-019 commands + canonical param names + `id_for_name` | all param mutation |
| Sequencer CMD 16–32 vocabulary | all step/pattern ops |
| cap-docs + `Rule` (ADR-032) | param pages, envelope groups, affordance hints, macros |
| `StateBusHandle` subscribe/poll | all live values |
| `/transport/*`, `/node/*/state/*`, `/engine/*` paths | transport, playheads, CPU meter |
| `Antiphon server.rs::resolve_semantic` | *concept* — name→id resolution + clamp against cap-doc; Praxis implements its own pure `resolve` on the same rules |
| `tools/test-driver` command tables | reference vocabulary for `Action`; headless verification of command effects |
| `TuiApp` tick/shutdown/`TestBackend` test pattern | main-loop integration + render tests |
| Main-loop drain pattern (scripting, Antiphon) | command egress |

### 2.6 Gaps that require extension (only these)

1. **Live visualization data** — envelope level, LFO phase, output scope
   are not published anywhere today. §5.3; PRX2.
2. **P-lock authoring path** — the sequencer stores per-step locks (P3),
   but no current surface authors them; the exact command/event surface
   needs confirming in the PRX1 spec. OPEN — OQ-P8.

---

## §3. Interaction model — modes and keymaps

Two physical postures drive everything:

- **Typing posture** — both hands on the home position. Step entry, track
  selection, navigation.
- **Performance posture** — right hand on the numpad, left hand on
  mode/track/page keys. Sweeps, mutes, fills.

Modes switch posture. Mode keys are global; the meaning of the letter rows
changes per mode (that is the modal bargain: maximum density, zero
modifiers for the hot path).

### 3.1 Global keys — DETERMINED (modulo session findings)

| Key | Action |
|---|---|
| `Space` | play / stop |
| `Tab` / `Shift-Tab` | cycle modes forward / back |
| `:` | command line (fuzzy; the M-x escape hatch — every long-tail command lives here) |
| `Esc` | cancel / close / unfocus (the `C-g` reflex) |
| `Ctrl-C` | quit |
| `q w e r u i o p` | **select track 1–8 — invariant in every mode** (muscle memory rule #1) |
| `t` | tap tempo (global; this is why `t` stays out of the track row) |
| `y` | reserved (yank/copy family; assigned in PRX1) |

### 3.2 SEQ mode — DETERMINED core, HYPOTHESIS details

The default programming posture. One track selected; its 8-step page
window under the home row.

| Key | Action |
|---|---|
| `a s d f j k l ;` | toggle steps 1–8 of the current page on the active track (`CMD_TOGGLE_STEP`) |
| `[` / `]` | previous / next 8-step page window (up to 64 steps) |
| hold step key + jog | **p-lock** (HYPOTHESIS — needs kitty release events; fallback: leader-focus, §OQ-P1) |
| `Enter` | step-focus toggle: focus step under cursor / last-touched step for p-lock editing; `Esc` releases |
| `x` | clear active track pattern (confirm in echo area) |
| `,` … | leader family: copy/paste steps, set length, set speed, conditions, micro-timing (grammar fixed in PRX1) |

### 3.3 PERF mode — the performance cluster — model DETERMINED, chords HYPOTHESIS

**The model (frozen):** three **focus slots** A, B, C — virtual data
encoders, each bound to one (node, param). Slots are always live; moving
two slots simultaneously is the two-parameter requirement (filter sweep =
A on cutoff, B on resonance, one finger each). **Page retargeting is one
keystroke:** pressing a param-page key rebinds the slots to that page's
defaults instantly. Defaults come from the track's `Rule` (first slots of
the page); explicit rebinding of any slot to any param on the page is a
leader sequence.

**The numpad cluster (phase 1 assumes present):**

```
  7   8   9        A↑  B↑  C↑
  4   5   6        (free — PRX1 assigns: 4/5/6 candidate for direct page select)
  1   2   3        A↓  B↓  C↓
  0       .   ⏎    ALT-layer (hold)   reset   value-entry
  +   -   *   /    coarse jog pair / step-size ×2 ÷2
```

- **Tap = one jog step; hold = ramp** with acceleration (our own ramp
  timing, not OS repeat, when kitty repeat events exist).
- **Shift+jog = fine, Ctrl+jog = coarse** — HYPOTHESIS; alternative is
  `*`/`/` step-size scaling only. Sessions decide.
- **`0` held = momentary ALT layer** (thumb): while held, `qweruiop` are
  momentary mutes, home row fires fills/trigs. The one-hand performance
  money key. PRX2.
- **`.`** reset slot A to default; `,`+`.` resets B. HYPOTHESIS.
- **`+`/`-`** semantics OPEN — OQ-P4 (coarse jog vs step-size).

**Param pages (left hand):** number row `1`–`6` selects the selected
track's param page in `Rule` order (typically SRC/FLTR/AMP/FX/MOD/TRIG);
page select rebinds A/B (and C on FX pages) to page defaults. Slots and
bindings shown in the mode line at all times — **you never wonder what a
key will do** (the legibility lesson from sessions s1/s2).

### 3.4 Further modes — scoped, detailed in their phase specs

- **MIX** (PRX2): per-track level/sends; rows as faders on jogs.
- **CHAIN** (PRX2): pattern bank, cue-blink, chain push/clear, page-loop
  windows — the P10 command set given a performance layout.
- **MODE** count stays small (Emacs lesson: few major modes, rich leader
  families). New modes are a design-doc decision, not an implementation
  detail.

---

## §4. Parameter performance model — DETERMINED mechanics

1. **All motion is `CMD_BUMP_PARAM` relative deltas** — the platform's
   relative-only contract; drift-free against the 30 Hz-equivalent state
   mirror and against p-locks fighting the hand.
2. **Jog step is per-param**: `max(0.001, range/128)` coarse default; fine
   = coarse/8; step-size scaler multiplies/divides by 2 within
   [range/1024, range/8]. (Exact constants are tuning knobs for sessions.)
3. **Ramp**: key held → jog repeats at 60 Hz with acceleration
   (step × 1.05^n capped at ×8) after a 150 ms dwell; release stops
   immediately. Without kitty events, OS key-repeat drives the ramp and
   acceleration is disabled (graceful degradation, same bindings).
4. **Two-slot simultaneity is the floor, not the ceiling**: the input
   layer tracks pressed keys as a set; any combination of slot keys ramps
   concurrently. Three slots exist because the numpad offers three columns;
   two-handed two-slot sweeps must never require a modifier.
5. **Absolute entry** via the command line (`:set cutoff 0.7`) from day
   one; numpad type-in entry is OPEN — OQ-P5.
6. **P-locks**: a jog while a step is focused/held routes to the step's
   lock, not the live bank (per-cycle `node_locks` semantics already in the
   engines; authoring path OQ-P8). The focused step is always visible.

---

## §5. Rendering & real-time visualization

### 5.1 Layout — DETERMINED skeleton

```
┌ transport: BPM · ▶/■ · pattern · position · CPU ──────────────┐
│ contextual window (per mode)                                   │
│   SEQ:  8-track × 8-step page grid, playheads, trig colors     │
│   PERF: param page — envelope/LFO/filter graphics + value bars │
│   CHAIN: pattern bank + chain lane                             │
├ mode line: MODE · track · page · A/B/C bindings+values · step ┤
└ echo area: messages, confirms, `:` command line ───────────────┘
```

The mode line is the legibility contract (s1/s2 lesson): current bindings
and values are always on screen.

### 5.2 Graphics from data we already have — DETERMINED for POC

- **Envelope curves**: drawn from `Rule::EnvelopeGroup` param values
  (attack/decay/sustain/release → piecewise curve on a braille canvas),
  recomputed when the params move. This is exactly what the ADR-032
  affordance metadata exists for; zero engine changes.
- **LFO/filter shapes**: from `AffordanceHint::{LfoShape,FilterShape}` +
  rate/depth/cutoff/resonance values. LFO phase is *static* until §5.3(b).
- **Step grid**: trig state from `/node/{seq}/state/steps` text bitfield +
  `current_step` playheads; per-track colors.
- **Value bars/meters**: block-element bars (the existing `paraclete-tui`
  idiom), yellow-flash on recent change.
- **Frame policy**: dirty-flag redraw, ≤60 fps, canvases ≤30 fps.

### 5.3 Live data (the one real gap) — PRX2, approved direction

1. **(a) Static-from-params** is the POC baseline — always works, for
   every node, forever.
2. **(b) Publish phase/level**: `EnvelopeNode` and `LfoNode` add
   `/node/{id}/state/env_level` and `/node/{id}/state/lfo_phase` to their
   existing `published_state()` push-down — no trait changes, no new
   channels, coalesced to render rate. Then envelopes/LFOs *move*.
3. **(c) Scope tap**: an SPSC ring tapped at the mix output (the
   test-driver `CaptureRing` pattern: `try_push`, drop-on-full — the same
   real-time-safe shape as the state-bus and debug-event SPSCs). Master
   scope first; per-track taps only if sessions ask. Details frozen in the
   PRX2 spec; audio-thread rules unchanged.

---

## §6. Testability & iteration protocol — DETERMINED

Testing is layered so that *feel* is the only thing a human must judge:

| Layer | Tool | Catches |
|---|---|---|
| Keymap | pure `map_key` unit tests (crossterm `KeyEvent`s are constructible) | wrong action for chord, mode leaks, dead keys |
| Resolution | pure `resolve` tests against fixture cap-docs | wrong node/param, clamp errors |
| Render | ratatui `TestBackend` buffer assertions (existing `tui_tests.rs` pattern; **no insta snapshots** — SPIKE-005 verdict stands) | layout breakage, missing bindings in mode line |
| Engine effect | `tools/test-driver` scenarios asserting the same commands Praxis emits | regressions in the command contract itself |
| Feel | **paired usability session after every phase** | everything that matters most |

Session protocol (adopts the W-track instrument): notes append-only in
`design/sessions/praxis-N.md`; each session ends with an explicit
**converged / revise / park** verdict per hypothesis tested; the roadmap
and this file's HYPOTHESIS marks are updated in the same commit as the
session notes.

**Convergence rule:** no chord grammar graduates from HYPOTHESIS to
DETERMINED without one session of hands-on use; nothing DETERMINED is
reopened without new session evidence.

---

## §7. Roadmap — PRX phases with testing cycles

Phase specs are written per-phase in `design/phases/` (`prx0-praxis.md`…)
only when that phase is next to start (the house front-load rule). Reports
append to `design/phases/prxN-report.md`.

### PRX0 — POC: the vertical slice *(next after ADR-036 ratification; 1–2 commits)*

Prove posture and feel, nothing else.

- Crate + modal shell (SEQ/PERF only), global keys, track select, home-row
  steps for the default instrument's 8 tracks (hardcoded ids acceptable
  *inside the POC only*), `[`/`]` pages, play/stop.
- PERF: slots A/B on the numpad bound to the selected track's first two
  engine params; tap/hold jog with ramp; `1`–`6` page select via `Rule`;
  mode line with live bindings/values; static envelope render (§5.2).
- Tests: keymap + resolve unit suites; TestBackend smoke for both modes.
- **Exit:** all tests green; agent smoke-run; **usability session #1**
  (user plays it for 30 minutes on the default instrument; findings
  recorded). Explicit go/no-go on PRX1 scope.

### PRX1 — Editing depth *(scope re-cut by session #1)*

- Discovery-driven binding (no hardcoded ids; `InstrumentIds`/cap-docs).
- Full `Rule` param pages incl. sub-pages; explicit slot rebinding leader
  grammar; `y` yank/copy family; step-focus p-locks (OQ-P1, OQ-P8);
  pattern select + cue; `:` command line real (fuzzy over a generated
  command index).
- **Session #2** → verdicts on chord grammar.

### PRX2 — Performance layer

- Numpad-`0` momentary ALT layer: mutes, fills; CHAIN mode; tap tempo;
  temp save/reload *(dependency: P11 scope — flagged, may defer)*;
  §5.3(b)+(c) live visualization; numpad-less fallback layer (OQ-P3);
  ramp/acceleration retune from session telemetry.
- **Session #3** → verdicts on performance ergonomics.

### PRX3 — Breadth & convergence

- MIX mode; chain view; macro support from `Rule`; keymap config file
  (Ordo-adjacent, plain-data keymap); **WT convergence decision** (does WT
  proceed as specced, fold into Praxis, or stay deferred — user decision,
  informed by three sessions of evidence).
- **Session #4.**

Explicitly **not** one-shot: every phase gate is a session; every session
can re-cut the next phase.

---

## Open Questions

| # | Question | Status | Where decided |
|---|---|---|---|
| OQ-P1 | P-lock gesture: hold-step (kitty) vs leader-focus vs both | OPEN — POC tests hold; focus is the fallback | session #1 |
| OQ-P2 | Leader key: `,` vs `\` vs double-tap-Space | OPEN — `,` is the working choice | session #1 |
| OQ-P3 | Numpad-less fallback jog map (candidates: `j`/`k`+`u`/`i`; `,`/`.`+`<`/`>`) | OPEN — post-phase-1 by brief | session #2 |
| OQ-P4 | Numpad `+`/`-`: coarse jog pair vs step-size scaling | OPEN | session #1 |
| OQ-P5 | Numpad type-in value entry vs command-line-only | OPEN | session #2 |
| OQ-P6 | Kitty-less terminals: accept OS-repeat ramp degradation, or disable hold-ramp entirely | OPEN | session #1 (test in tmux + stock terminals) |
| OQ-P7 | Esc/Tmux prefix delays and `Esc`-as-cancel ergonomics | OPEN | session #1 |
| OQ-P8 | P-lock authoring path: confirm the sequencer lock-set command/event surface (no current surface authors locks) | OPEN — resolve in PRX1 spec | PRX1 spec |
| OQ-P9 | Should Praxis publish `/script/praxis/selected` so Theoria/Launchpad follow its track selection | OPEN — default yes | PRX1 |
| OQ-P10 | Scope tap placement: master only vs per-track | OPEN — default master | PRX2 spec |
| OQ-P11 | Temp save/reload depends on P11 engine scope — ship UI-only or defer | OPEN | PRX2 spec |
| OQ-P12 | WT convergence: proceed / fold / defer | OPEN — user decision with session evidence | PRX3 |

---

## Cross-references

- `design/praxis/problem.md` — the fixed problem statement
- `design/adr/ADR-036-praxis-performance-terminal.md` — the architectural
  decision (proposed)
- ADR-018 (environment-vs-node), ADR-019 (semantic plane), ADR-026
  (terminal as instrument display), ADR-032 (universal node-view contract)
- `design/interface-plan.md` — W-track; WT relationship (§2.1, OQ-P12)
- `design/instrument-vision.md` — performance-first tiebreaker
