# TK0 — POC Vertical Slice: Report

**Status:** ✅ COMPLETE (2026-07-21)
**Session:** `design/sessions/theotokos-1.md` — usable sign-off

## Commits

| C0 | `903536a`–`1905bbf` | BUG-032: InternalClock command handling (start/stop/bpm) |
| C1 | `0acfab6`–`46075b9` | `paraclete-theotokos` crate, modal shell, SEQ mode |
| C2 | `2f0d9cd` | PERF mode: focus slots, Rule pages, jog/ramp, envelope |
| C3 | `theotokos-1.md` | Usability session #1 — converged, scope re-cut |

## Exit criteria

| Criterion | Status |
|---|---|
| `cargo test --workspace` green | ✅ 48 suites, 0 failures |
| Agent smoke run | ✅ `cargo run` renders, keys drive 4-track instrument, play/stop works |
| 16-step grid (home + lower row) | ✅ Converged |
| PERF: Rule page select + slot A/B jog | ✅ Converged |
| Arrow keys for jog (↑↓ A, ←→ B) | ✅ Converged |
| Transport via clock commands | ✅ `Space` toggles, `bpm` live |
| PipeWire auto-recovery on exit | ✅ |

## Spec divergences

Every divergence from `design/phases/tk0-theotokos.md` that a TK1 planner must know:

| Spec | Implementation | Why |
|---|---|---|
| Numpad 7/1/8/2 for jog (A↑↓ B↑↓) | Arrow keys (↑↓ A, ←→ B) | crossterm can't distinguish numpad from main row; session converged |
| `j`/`k` as jog fallback | Removed | Step keys (asdfjkl; + zxcvm,./) made invariant across modes (Digitakt model); jog needed separate keys |
| 8-step page windows, 1 row per track | 16-step two-row grid, page windows for 32+ | Session converged: full pattern visible on one screen |
| Envelope braille canvas | Gauge bar | POC simplification; TK1 upgrade path clear |
| `[`/`]` page nav | `-`/`=` primary, `[`/`]`/`{`/`}` also accepted | Terminal ESC-sequence interception; `-`/`=` always reliable |
| `--theotokos` flag | Theotokos is default; `--emulator` for legacy | User decision at session; launchpad sim no longer starts automatically |
| Step cells: 2-char ` ▓` | 4-char ` ████ `, 2 lines tall (square) | Visual iteration during session |
| `log::info!/warn!` (ADEMP-AGENTS.md logging convention) | Audio/rtkit/scripting now use `log` crate exclusively | Was `eprintln!` — debug spam suppressed at default RUST_LOG |
| Per-track state (RenderData::step_state) | `RenderData::step_states: Vec<StepState>` | Initial implementation showed all tracks as active track's state |
| PipeWire stranding | Pre-audio recovery + post-exit auto-restart | Not in spec; found during session testing; `--recover` flag added |
| `debug_event` on mode line | Last key→action shown on every non-Noop press | Testability; no spec coverage |
| Emulator handles `keyboard on audio thread` (INFRA-008) | No change; Theotokos main-thread keyboard proves the pattern | Gated on Theotokos adoption; fix deferred |

## Open questions resolved

| OQ | Resolution |
|---|---|
| OQ-T14 (transport shape) | Node commands (CMD_CLOCK_START/STOP 16/17) |
| OQ-T3 (numpad-less) | Arrows settled; TK2 deferred |
| OQ-T6 (kitty-less) | OK in practice; `-`/`=` fallback for page nav |

## TK1 scope (re-cut by session #1)

- Pull mutes from TK2
- `:` command line: full (was stub)
- Composite `Rule` pages (OQ-T13)
- P-lock command design (OQ-T8)
- Slot rebinding grammar
- Pattern select + cue
- Yellow-flash on param change in PERF mode line

## Implementation notes for the TK1 planner

These are things a fresh session will need to know that are not in the
TK0 spec or obvious from reading the code alone.

- **`handle_keys(&bus, &[KeyEvent])`** is the test seam — pure key→action
  processing with no terminal dependency.  Four end-to-end key-injection
  tests exist in `lib.rs` (bracket nav, page-window offset, step toggle).
  Add new tests here; do not test through `tick()` (requires crossterm).
- **`render_if_needed()`** handles frame-rate cap + dirty flag.  Rendering
  reads all track state from the bus each frame; no local cache to
  invalidate.
- **The `BusHandle` type** is `Rc<RefCell<StateBusHandle>>`.  Methods on
  `Model` take `&StateBusHandle` — deref the `Ref` with `&*bus_ref`.
  Always `drop(bus_ref)` before `terminal.draw()` (borrow conflict).
- **Page groups follow each Rule's own `page_groups` order** — there is no
  platform-wide canonical page order.  ADR-032 §8 and the Antiphon
  implementation already differ; Theotokos is the tiebreaker (spec §3.3).
- **Crossterm `KeyCode::Char` cannot distinguish numpad from main row** on
  this terminal version.  Future numpad mapping needs Kitty extended
  key protocol (`KeyEventKind` + location field, not in crossterm 0.27).
- **The default instrument is 4-track** (seqs 10–13, voices 20–22/27).
  The AGENTS.md 8-track ID table is aspirational.  Discovery is positional
  (zip sequencers × generators); edge-derived mapping is TK1.
- **Composite `Rule` assembly** lives in `paraclete-antiphon` and is
  protocol-shaped, built only when Antiphon is enabled.  TK1 track pages
  need this hoisted (OQ-T13).
- **P-locks have no command authoring path today** — locks are written only
  by the v3 deserializer and tests.  CMD 33+ is free; packing precedent
  exists in `CMD_SET_STEP_CONDITION` (sequencer.rs:1318).
