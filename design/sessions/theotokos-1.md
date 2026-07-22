# Theotokos — Usability Session #1

**Date:** 2026-07-21
**Phase:** TK0 C3 (exit criterion)
**Session type:** hands-on, default instrument, `cargo run`

## Verdict: usable. TK0 exit criteria met.

The core posture (home-row + lower-row steps, track select, modal
switching, PERF jog) works end-to-end on the default 4-track instrument.

## Converged hypotheses

| Hypothesis | Verdict |
|---|---|
| 16-step grid (home+lower row) beats 8-step page windows | **Converged.** Two-row invariant grid is the settled model. |
| Arrow keys for PERF jog | **Converged.** Intuitive, no key conflicts. |
| `-`/`=` as page nav fallback | **Converged.** Handles extended patterns without `[`/`]` terminal issues. |
| Square step cells (4 wide × 2 tall) | **Converged.** Visual distinctness worth the screen space. |

## Revision findings (TK1 scope)

| Finding | Action |
|---|---|
| No `:` command line yet — all long-tail ops unreachable | Move from TK1 "stub" to full implementation with fuzzy command index |
| No track mute/unmute — basic performance blocker | Pull mute overlay from TK2 into TK1 |
| No visual feedback for parameter changes in PERF mode line flash | Add yellow-flash-on-change (matching old TUI encoder pattern) |
| `[`/`]` unreliable across terminals | `-`/`=` stay as primary; deprecate brackets |
| 4-track instrument works; 8-track untested | TK1 requires 8-track instrument YAML or discovery stress test |
| PipeWire stranding on exit | Mitigated (auto-recovery); root cause (direct ALSA) not fixed |

## Parked (no change)

| Item | Reason |
|---|---|
| P-lock authoring (OQ-T8) | Needs TK1 command design pass first |
| Numpad mapping | crossterm can't distinguish main row from numpad; arrow keys settled |
| Leader key choice (OQ-T2) | No leader grammar implemented yet — punt to TK1 |
| Composite Rule pages (OQ-T13) | Engine-local working; composite needs assembly extraction |

## Roadmap deltas

- TK1 now pulls mutes from TK2
- TK1 `:` command line upgraded from "stub" to full
- End-of-session verdict: TK1 scope re-cut; begin design phase immediately
