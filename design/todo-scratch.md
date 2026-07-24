# Todo Scratch

> **Minimal, no stale items.** Transient session-to-session carryover.
> Completed items are removed. Permanent milestones live in `roadmap.md`.

## Carryover from 2026-07-23 session

### P11 performance state (ADR-039 ✅ accepted 2026-07-23, R1–R3)

- `p11-interfaces.md` phase spec is written **after TK2 + session #2**
  (R3, house front-load rule): freeze CMD 39–45, kit UX open questions.

### Anamnesis sampling track (drafted 2026-07-23)

- ADR-040 🟡 proposed + `design/sampling/{problem,design}.md` — await user
  ratification (HAL input, recorder rings, pool + asset plane, scenes).
- Scheduling decision at TK2 exit (AN0 independent; AN2 needs P11 kits).

### Next up: TK2 implementation

- Start at `design/phases/tk2-theotokos.md` C0 (page-order swap) and
  proceed commit-by-commit; session #2 = C10.

### Elektron convergence (ADR-038 ✅ accepted 2026-07-23, D1–D4)

- ~~Expand `design/phases/tk2-theotokos.md`~~ — **done 2026-07-23**: full
  spec, D5–D14 frozen, commits C0–C10 with named tests. Next: implement C0
  (page-order swap) onward.
- Session #2 (TK1 C8) HELD until TK2 S0 lands (D4); runs on ADR-038 grammar.
- `CANONICAL_PAGE_ORDER` reorder (TRIG SRC FLTR AMP FX MOD, D2) touches
  `paraclete-view-assembly` + Antiphon/web page order — call out at session.

## Carryover from 2026-07-22 session

### TK1 deferred (C7 partial)

- `instrument-8track.yaml` fixture (spec §7.5: 8 sequencers → 8 voices → chain
  nodes → mix; requires `gen-samples` output)
- 7/14 named C7 tests missing:
  - `render_shows_active_and_cued_pattern_markers` (TestBackend)
  - `paste_clamps_to_shorter_pattern`
  - `paste_includes_velocity_length_condition_timing_commands`
  - `cross_track_paste_remaps_generator_locks_drops_chain_locks`
  - `flash_colors_value_within_window` (render)
  - `eight_track_fixture_discovers_eight_tracks`
  - `nine_tracks_clamp_to_eight`
- Yank is lossy: bus doesn't expose per-step note/velocity/length/timing/condition.
  Only steps bitfield + locks grammar are yanked.

### TK1 remaining

- C8: usability session #2 (user-paired). Produce `design/sessions/theotokos-2.md`
  and `design/phases/tk1-report.md`.

### Upstream (requires user)

- 27 unpushed commits on `main`. Push when ready.
