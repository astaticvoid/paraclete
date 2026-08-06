---
name: test-driver
description: Headless audio rendering and assertion harness (ADR-033). Use when testing DSP changes, running regression baselines, writing test scenarios, or verifying audio output without hardware.
---

# Test Driver (ADR-033)

`tools/test-driver` renders the graph without hardware and asserts on the result.

## Quick mode

```bash
cargo run -p test-driver -- --trigger analog_engine:kick --at 1.0 -d 3
```

## Scenario mode

```bash
cargo run -p test-driver -- tools/test-driver/tests/kick_reverb_clean.yaml
```

Scenarios live under `tools/test-driver/tests/`. Each is a YAML file with timed commands and assertions.

## Regression baselines (ADR-035)

```bash
cargo run -p test-driver -- <scenario>.yaml --update-baseline   # write baseline
cargo run -p test-driver -- <scenario>.yaml --check-baseline    # diff; exit 1 on drift
```

Baselines use a deterministic single-threaded render (bit-stable). Tolerances live in the `.baseline.json`.

**Baselines can lie.** A baseline you have never seen fail is not evidence. Two perturbations minimum:
1. Removing the feature moves the fingerprint
2. Reinstating the defect also moves it

Always check with `2>&1` — exit code is the reliable signal (0 pass, 1 drift). `2>/dev/null` with grep hides failures.

## Seven regression baselines

Run all before and after any DSP-touching change:

| Scenario | Coverage |
|----------|----------|
| `kick_reverb_clean` | Node 20 through mix+reverb (analog Kick) |
| `plock_authoring` | Node 10→20, authored p-locks (analog Kick) — locks `decay` by NAME on step 4 |
| `analog_machines` | Nodes 21, 22, both at once (analog Snare, HiHat) |
| `fm_machines` | Node 27 through all three (FM Kick, Bell, Bass) |
| `fx_chain` | Engine → filter → distortion (uses instrument-fx.yaml) |
| `sampler_chain` | Node 23 sweeping pitch/start/end/loop (uses instrument-fx.yaml) |
| `lfo_sweep` | Nodes 20 + 27, lfo_dest tune and tone — the LFO's ONLY coverage |

`fx_chain` catches filter-coefficient changes but MISSES filter-state re-sequencing. `lfo_sweep` is the only coverage for the MOD block — all others run at lfo_dest 0 (off).

## Interactive mode

```bash
cargo run -p test-driver -- --interactive --instrument instrument.yaml
```

JSON-lines REPL. Commands: `trigger`, `set_param`, `read`, `peak`, `dump`, `render`, `quit`.

## Naming targets

Accepts: node id, full type tag (`analog_engine:kick`), display name (`Kick`), short tag (`kick`), or qualified `type_tag/display_name` (`sequencer/Kick`). Names are case-insensitive.

**Ambiguous names are a hard error** listing candidates. A name claimed by more than one node is NOT resolved silently (INFRA-012).

**Numeric ids are NOT validated** — a typo'd id resolves and silently no-ops (INFRA-014).

## Naming lock lanes

`set_lock_target` and `clear_step_lock` take `param: <name>` (preferred) or raw `param_id`. **Prefer the name** — a raw id is an FNV-1a hash (`decay` = 3541427549) that no reader can check. A wrong id is stored, emitted, and silently never matched.

## Assertions

- State-bus: `eq`, `between`
- Live: `peak_gte`, `peak_lt`
- Post-capture: `discontinuity_lt`, `dc_offset_lt`, `dropout_lt_ms` (windowed by `from`/`until` seconds; NaN/Inf fail outright)

Exit 0 pass, 1 assertion failure, 2 fatal.

**Caveat:** timeline actions dispatch on wall clock but capture time runs ~25% slower in debug builds — leave margin in artifact windows around action times.

## P11 live scenarios

| Scenario | Coverage |
|----------|----------|
| `p11_kit_capture_apply` | C0+C2: capture is opt-in via `in_kit` |
| `p11_temp_save_reload` | C3: AppTempSave/AppTempReload |
| `p11_bind_kit_apply` | C2: pattern-switch kit-apply outside perform mode |
| `p11_mute_tiers` | C4: immediate, prepared, wrap mutes |
| `p11_live_rec_midi2` | C5: midi2_note_on injection + live_quantize |
| `p11_live_erase` | C6: live_erase verb |

All are mutation-checked and part of the Live-test gate for P11-touching commits.
