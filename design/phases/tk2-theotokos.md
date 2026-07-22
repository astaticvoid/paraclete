# Paraclete — TK2 Theotokos Specification (Performance Layer)

> **STUB — created 2026-07-22.** This is a placeholder. The full spec is
> owned by the **design agent** (see `design/handoff.md` model-tier routing).
> This stub captures the scope envelope and anchors; the design agent expands
> it into a full TK1-style implementation blueprint.
>
> **Design authority:** `design/theotokos/design.md` §4 (TK2 scope),
> ADR-036, ADR-037 (key remapping, proposed).
> **Baseline:** TK1 ship (composite pages, p-locks, mutes, `:` line,
> patterns, session #2 complete).
> **Exit:** `cargo test --workspace` green; agent smoke on 4+8 track fixtures;
> **usability session #3**.

---

## Scope envelope

| # | Item | Source | Notes |
|---|------|--------|-------|
| S1 | **Key remapping mechanism** | ADR-037 (proposed 2026-07-22) | Pulled forward from TK3. See §Key remapping below. |
| S2 | Numpad-`0` momentary ALT layer | design.md §4 | Mutes, fills. Dependency: P11 mute system (flagged — crude mute via MixNode gain as fallback). |
| S3 | CHAIN mode | design.md §4 | Pattern bank, cue-blink, chain push/clear, page-loop UI. |
| S4 | Tap tempo | design.md §4 | Rides the §2.6.1 clock extension; `CMD_BUMP_PARAM` on clock `bpm`. |
| S5 | Temp save/reload | design.md §4 | Dependency: P11 scope (flagged — may defer). |
| S6 | Live visualization (env/LFO phase, scope tap) | design.md §5.3(b)+(c) | EnvelopeNode/LfoNode state publish; optional master scope via rtrb SPSC ring. |
| S7 | Numpad-less fallback layer (OQ-T3) | design.md §4 | `j`/`k`+`u`/`i` or `,`/`.`+`<`/`>` candidates. |
| S8 | Ramp/acceleration retune | design.md §4 | From session #2 telemetry feedback. |

---

## Key remapping (S1)

### Design anchors

| Anchor | Resolution |
|--------|-----------|
| Data model | `Keymap` struct: `HashMap<(Option<Mode>, KeyBinding), Action>`. `None` mode = global (applies in all modes). Fall-through to hardcoded defaults when no binding matches. |
| `map_key()` integration | `input::map_key()` gains a `&Keymap` parameter. Check user bindings first; fall back to existing hardcoded match arms. Zero overhead for unbound keys (empty HashMap → straight to fallback). |
| Runtime mutation | Via the `:` command line (TK1 C6 infrastructure). Verbs: `:bind <mode> <key> <action>`, `:bind <key> <action>` (global), `:unbind <mode> <key>`, `:reset-bindings`, `:save-bindings`, `:load-bindings`. |
| Key name grammar | Human-readable strings: `a`, `;`, `Space`, `Tab`, `Enter`, `Backspace`, `Esc`, `Up`/`Down`/`Left`/`Right`, `Ctrl+k`, `Shift+Up`, `F1`–`F12`. Matching is case-insensitive for modifiers, literal for char keys. |
| Persistence format | YAML via `serde_yml`. `~/.config/paraclete/keymap.yaml` (global) + `./keymap.yaml` (local override, loaded after global — last write wins per binding). |
| Action names | Matches `Action` enum variant names exactly: `Quit`, `CycleMode`, `PlayToggle`, `SelectTrack`, `ToggleStep`, `PageWindow`, `SelectParamPage`, `Jog`, `Noop`. Some actions carry parameters (e.g. `Jog { slot: A, dir: Next, mag: Normal }`) — these need a sub-grammar in the `:` line parser. |
| File load timing | Startup (after `TheotokosApp::new` but before first tick). Reload via `:load-bindings` without restart. |
| File save timing | `:save-bindings` writes current bindings. Also auto-save on clean exit (quit). |
| Error handling | Invalid bindings in file → warn on load, skip the entry, continue. `:bind` with unknown action/key → echo error to mode line, no mutation. |

### Integration points

1. **`input.rs`** — `map_key(&Keymap, mode, ev) -> Action`. User bindings checked before hardcoded match arms.
2. **`lib.rs` / `TheotokosApp`** — owns `Keymap`; loads at startup; exposes `:bind`/`:unbind`/`:save`/`:load` through the `:` command dispatcher.
3. **`TheotokosConfig`** — optional `keymap_path: Option<PathBuf>`.
4. **`:` command line** (TK1 C6) — new verb family dispatched from the fuzzy index.

### Non-goals

- No GUI keymap editor (TK3, WT convergence).
- No Ordo layout profile integration (TK3).
- No key-*sequence* bindings (chords beyond single-modified-key); the chord grammar is HYPOTHESIS-grade per ADR-036.
- No display of current bindings in the mode line (`:list-bindings` exists but renders to echo area only).
- No per-track or per-instrument bindings — the keymap is Theotokos-global.

### Test plan (minimal)

- `keymap_resolves_user_binding_over_default`
- `keymap_falls_through_to_default_when_unbound`
- `keymap_global_binding_applied_in_both_modes`
- `keymap_mode_specific_overrides_global`
- `bind_without_mode_is_global`
- `unbound_key_returns_noop_not_crash`
- `keymap_roundtrips_yaml`
- `keymap_load_merge_local_overrides_global`
- `bind_invalid_action_echoes_error`
- `unknown_key_in_file_warned_and_skipped`

---

## Commit sequence (to be designed by design agent)

Placeholder. Expected structure: one commit per scope item (S1–S8), ordered by dependency chain. Key remapping (S1) depends on TK1 C6 (`:` line) and has no TK2-internal dependencies — can ship first or in parallel with S2/S3.

---

## Open questions for the design agent

| # | Question | Context |
|---|----------|---------|
| OQ-T15 | Action-with-parameters grammar for `:bind` | How to express `Jog { slot: B, dir: Next, mag: Normal }` as a one-liner? Options: positional args (`:bind Down Jog A Next Normal`), sub-flags (`:bind Down Jog --slot=A --dir=Next`), or by-example (`:bind Down Jog B Next Normal`). |
| OQ-T16 | Should `reset-bindings` clear ALL or keep a whitelist? | Emacs `global-unset-key` vs `(setq ... nil)`. |
| OQ-T17 | Auto-save on quit: always, prompt, or opt-in? | Risk: user experiments, quits, loses work vs surprises on next launch. |
| OQ-T18 | Key name for `\` (backslash) | Currently the leader key (TK1 C7). If remappable, the leader key itself can be rebound — but the `:` line is the only way to rebind it (chicken-and-egg if leader is needed to open `:`). Design agent resolves. |
| OQ-T19 | Should step/track key arrays (`TRACK_KEYS`, `STEP_KEYS`) also be remappable? | Currently fixed-positional. If a user rebinds step 0 from `a` to `1`, does the step grid compensate (shifting all positions) or does the keymap operate at the Action level (individual `ToggleStep { col: N }` bindings)? |

---

## Explicit non-goals (TK2)

Undo (session #3 may re-cut); step-note editing (`CMD_SET_STEP` note entry — later); WT convergence (TK3); slot C bindings (TK2 already scoped for this — confirm); CHAIN mode spec is the design agent's responsibility to define against the P10 engine semantics.

## Hypotheses under test in session #3

| Hypothesis | Source |
|------------|--------|
| Runtime key remapping via `:` line is discoverable and usable | S1 |
| ALT-layer numpad-0 mutes/fills are ergonomic for performance | S2 |
| Pattern-chain push/clear in CHAIN mode matches session expectations | S3 |
| Tap tempo on clock BPM is sufficiently responsive | S4 |
| Live env/LFO phase visualization aids performance editing | S6 |
| Numpad-less fallback layer is viable on compact keyboards | S7 |
