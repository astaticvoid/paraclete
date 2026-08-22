# AN0 — Anamnesis Pool + Asset Plane — Report

> Companion to `an0-anamnesis.md` (spec). Written at phase close,
> 2026-08-21. Eight commits, `1d8f20f`..`4cd1626`.

---

## What shipped

| Commit | What |
|---|---|
| `1d8f20f` | Spec: execution-ready C0–C6, ten implementation decisions (§0) |
| `8e1f8f8` | **C0** — `SamplePool`/`PoolEntry`, project persistence (mirrors `KitStore`) |
| `3a36e6f` | Docs: C0 build note (load_wav duplication), C2 sequencing preview |
| `0d100dd` | **C1** — asset-delivery plane: `SwapSample` rtrb ring pair, `Node::swap_sample()` |
| `d9aefc6` | **C2** (+C3+C4 folded in) — `Sampler` moves to pool-indexed sample selection, v4 serialize, SRC/AMP page reshape |
| `fc6979a` | Docs: C2 fold recorded, #185 filed |
| `37b5661` | **C5** — `AppOp::PoolLoadFile` + `:pool-load`, runtime pool population |
| `4cd1626` | **Fix** — #185: `load_project_v3` rehydrates `pool_cache` on reload |

The phase closed at 8 commits against a 7-commit plan (C0–C6) because two
commits split further than planned (C0's `load_wav` duplication forced a
build note; the #185 gap surfaced in C2 review and needed its own fix
commit after C5) while C3 and C4 collapsed into C2 (removing
`sample_path` forces a simultaneous serialize change; declaring `sample`
forces a simultaneous page placement — see `an0-anamnesis.md`'s C0/C2
build notes for the mechanics). Net: the same ground the spec described,
reshuffled by what the codebase's own invariants actually allowed to land
as standalone, buildable commits.

## Review findings, by severity

Five review passes (one per commit or logical batch — C0, C1, C2, C5, the
#185 fix), all via the `code-reviewer` subagent, all findings fixed before
commit:

- **C1 (real hard-constraint bug):** `drain_asset_swaps()` dropped a
  `SwapSample`'s `Arc<SampleData>` in place — synchronously on the audio
  thread — whenever the target `node_id` didn't resolve to a live `Node`
  (unknown id, or a `Device` target with no `swap_sample` method). Fixed:
  both paths now funnel through the same return-or-retry handling as an
  actually-displaced buffer. The two tests that should have caught this
  couldn't — a bare "eventually drops to 1" `Arc::strong_count` check
  can't distinguish "dropped safely on the main thread" from "dropped in
  place inside `process()`"; strengthened to assert the count *immediately
  after* `process()`, before `process_main_thread()` runs.
- **C0 (real correctness bug):** `load_project_v3`'s
  `slots.resize(NOMINAL_POOL_SIZE, None)` would have silently truncated a
  pool grown past 128 slots back down on every load — `kits` can't hit
  this (`KitStore::set` caps at 64), the sample pool has no such ceiling.
  Fixed: resize only grows, never shrinks.
- **C5 (test-coverage gap):** the first version of the app-op handler
  test only checked pool metadata; deleting the entire broadcast loop
  still passed it. Replaced with an integration test driving a real
  two-Sampler graph through `NodeExecutor::process()`.
- **#185 fix (minor, addressed):** the decode-failure branch had no
  direct test; added one. Left one silent-no-op path as documented,
  currently-unreachable behavior rather than adding warning noise for a
  case the single call site can't produce.
- Everything else reviewers checked (per-voice pool-lookup correctness,
  slice-idx equivalence to pre-AN0 behavior, v2/v3/v4 serialize byte
  offsets, rate-scale consistency across every population path, layer
  boundaries, scope) came back clean on the first pass.

## Test coverage

| Layer | What | Catches |
|---|---|---|
| Unit (`sample_pool.rs`) | RON round-trip, `decode_file`, slot-growth | persistence format drift, decode regressions |
| Unit (asset plane) | swap delivery, D7 return-ring stress test (mutation-checked) | lost/leaked `Arc`, audio-thread alloc/dealloc |
| Unit (`Sampler`) | `swap_sample`, `is_known_param(P_SAMPLE)`, silence-on-uncached, sample-lock-plays-different-slot (mutation-checked) | lock plumbing regressions, stale-buffer bugs |
| Unit (`Sampler` serialize) | v4 round-trip, v2/v3 backward-read (silence, no crash) | format drift |
| Unit (`Sampler` `Rule`) | SRC/AMP page shapes | page-group regressions |
| Integration (`perform_state.rs`) | `PoolLoadFile` end-to-end through a real executor (mutation-checked) | broadcast-loop regressions |
| Integration (`project_tests.rs`) | project save/load round-trip incl. pool overflow, pre-AN0 backward-compat, `pool_cache` rehydration on reload (mutation-checked), decode-failure warning | format drift, silent playback loss on reload |
| Integration (Theotokos) | `:pool-load` parse + dispatch to `AppOp` | cmdline grammar regressions |
| Harness (test-driver) | `sampler_chain.yaml` + 6 other baselines, bit-exact throughout every commit | render-path regressions the builder→pool→playback rewrite could have introduced |

`cargo test --workspace` green after every commit; `cargo clippy
--workspace --all-targets` held at 49 pre-existing warnings, zero new,
after every commit.

## Exit criteria status

| # | Criterion | Status |
|---|-----------|--------|
| 1 | `cargo test --workspace` green after every commit | ✅ |
| 2 | `cargo clippy --workspace` clean on touched crates (diff against baseline) | ✅ 49→49 throughout |
| 3 | Each commit carries unit tests for the new logic | ✅ |
| 4 | C1's return-ring stress test (D7) passes | ✅ mutation-checked |
| 5 | C2's step-lock test confirms a per-step `sample` lock plays a different pool slot than the base | ✅ mutation-checked |
| 6 | Code review (Flash) on each commit or logical batch | ✅ five passes |
| 7 | No usability session required to close AN0 | ✅ — the sampling-session gate is AN1's (ADR-040 R2) |

All met. AN0 closes clean.

## Deferred items (unchanged from spec, still accurate)

See `an0-anamnesis.md` §6 — HAL input/recorders/SAMPLING screen (AN1),
slices/chains/scenes/crossfader (AN2), timestretch DSP (AN3), stereo
sample data (inherited limitation), default `instrument.yaml` gaining a
sample track (user call), per-node reference-tracking vs. broadcast-to-all
(no pressure yet), a pool browser command (AN1's screen is the real UI).
\#185 is now closed, not deferred — see §6's updated row.

## Gate

All exit criteria met, no open defects. AN0 closes on the strength of
this report. Per roadmap row 4b (AN0–AN1), the sampling-session gate sits
at **AN1's** exit, not here — AN1's spec is written next per the
front-load rule, when AN1 is next to start.
