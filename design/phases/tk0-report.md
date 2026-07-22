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

| Spec | Implementation | Rationale |
|---|---|---|
| Numpad 7/1 for jog | Arrow keys (↑↓ A, ←→ B) | crossterm can't distinguish numpad from main row; session converged on arrows |
| `j`/`k` for jog | N/A (removed) | Invariant step keys (Digitakt model) took priority |
| 8-step page windows | 16-step two-row grid + page windows for extended | Session converged on full grid |
| Envelope braille canvas | Gauge bar | POC simplification; reasonable for session |

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
