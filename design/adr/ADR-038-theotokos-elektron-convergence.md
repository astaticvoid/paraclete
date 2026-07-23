# ADR-038: Theotokos Elektron Convergence — Virtual Front Panel

| Field | Value |
|-------|-------|
| **Status** | ✅ Accepted (2026-07-23) |
| **Author** | Agent (user-directed redesign session) |
| **Ratification** | Ratified by user 2026-07-23 (decisions D1–D4 below) |
| **Scope** | `paraclete-theotokos`, `paraclete-view-assembly` (page order), sequencer command vocabulary (live-trig gap) |
| **Related** | ADR-036 (Theotokos), ADR-037 (key remapping), ADR-019 (semantic plane), ADR-030 (pattern engine); `design/theotokos/design.md` Stage 3 |

> Third-party names (Elektron, Digitakt, Digitone, Syntakt) are trademarks
> of their respective owners, used here nominatively for design comparison
> only. Paraclete is unaffiliated with and not endorsed by Elektron. Per
> house naming policy (ADR-036), such marks never appear in identifiers,
> UI strings, or product naming.

---

## Decision

**Theotokos's interaction model is rebuilt around a *virtual front panel*: a
device-neutral vocabulary of Elektron-class panel buttons, mapped to keyboard
keys, replacing the SEQ/PERF major-mode split, the dedicated `qweruiop`
track row, and the number-row pattern select.** The goal (user directive,
2026-07-23) is **the best Elektron-style groovebox workflow achievable on
a keyboard** — the Digitakt panel is the design reference;
Digitone/Syntakt workflow compatibility falls out of keeping the panel
vocabulary positional and `Rule`-driven.

Five structural changes:

1. **Input becomes two-tier: physical key → `PanelButton` → `Action`.**
   The key→button tier is the remappable layer (ADR-037's `Keymap`
   re-based here); the button→action grammar is fixed per screen/held-state.

2. **Track select is a hold-chord, not a row.** Hold **TRK** + trig key =
   select track 1–16. The `qweruiop` track row is retired, freeing the top
   letter row for trigs.

3. **One continuous trig grid is the primary (and only built-in) layout:**
   steps 1–8 = `q w e r t y u i`, steps 9–16 = `a s d f g h j k` — two
   continuous rows of 8, no split. The touch-type split layout
   (`asdf jkl;` / `zxcv m,./`) is **dropped entirely** (ratification D3);
   anyone who wants it can hand-write it as an ADR-037 keymap, with the
   known caveat that `Shift+;`/`Shift+/` collide with `:`/`?` on the
   FUNC-encoder plane. (User directive 2026-07-23: optional at most,
   dropped if it costs anything — it cost a file, a doc note, and session
   time, so it is dropped.)

4. **Major modes are replaced by Elektron state: a grid-recording toggle
   (REC), screens, and hold layers.** Trigs are *always trigs* on every
   screen: REC on → trigs toggle steps (grid recording / step-seq mode);
   REC off → trigs fire the track live (trigger mode). `Tab`-cycled
   SEQ/PERF modes are gone.

5. **The 8-encoder bank is emulated on the FUNC layer.** Each param page
   exposes up to 8 params (encoders A–H). **FUNC (Shift) + trig key** =
   encoder jog: the column (1–8) picks the encoder, the row picks the
   direction (top row = up, bottom row = down). Hold to ramp (§4 mechanics
   of `design.md` unchanged: `CMD_BUMP_PARAM` relative deltas, per-param
   step, acceleration). Two-encoder simultaneity is preserved (two keys
   under one held Shift).

## The panel vocabulary and default key homes

Buttons are semantic; keys are the default homes (remappable). All are
HYPOTHESIS-grade until session evidence, per the ADR-036 convergence rule —
the *model* (two-tier input, hold-chords, grid-rec toggle, encoder bank) is
what this ADR freezes.

| Panel button | Default key | Behavior |
|---|---|---|
| **TRIG 1–16** | `q…i` / `a…k` | REC on: toggle step. REC off: live-trig track (OQ-T20). Chord target for TRK/PTN holds. |
| **FUNC** | `Shift` (held) | The secondary plane: encoder jog on trig keys; FUNC+REC/PLAY/STOP = copy/clear/paste; FUNC+PG = setup/overflow pages. |
| **TRK** | `Tab` (held) | +trig = select track *n*. Sticky one-shot fallback without kitty release events. |
| **PTN** | `p` (held) | +trig = select pattern *n* (cue while playing, per TK1 semantics). Replaces number-row pattern select. |
| **REC** | `z` | Toggle grid recording (the "trigger mode vs step-seq mode" toggle). Prominent ● indicator in the transport bar. |
| **PLAY** | `x` (alias `Space`) | Play/pause. `Space` retained — session #1 muscle memory. |
| **STOP** | `c` | Stop (double-tap = kill voices, later). FUNC+z/x/c = copy/clear/paste — the Elektron chord verbatim. |
| **PG1–PG6** | `1`–`6` | Param pages in canonical order **TRIG, SRC, FLTR, AMP, FX, MOD** (Digitakt order; see Consequences). Selecting a page shows the encoder bank; labels from the track's composite `Rule`. FUNC+PG = overflow pages 7–12 / setup. |
| **KIT** | `7` | Screen (reserved; kit model TBD). |
| **SETTINGS** | `8` | Screen (global/instrument settings). |
| **SAMPLING** | `9` | Screen — **capability-gated**: appears only when the instrument declares sampling (Digitakt-alike); hidden for Digitone/Syntakt-alikes. |
| **TEMPO** | `0` | Tempo screen; YES taps tempo there. Retires the global `t` tap (now trig 5). |
| **YES / NO** | `Enter` / `Esc` | Confirm / back-cancel. Absorbs step-focus confirm and dialog grammar. |
| **UP/DOWN/LEFT/RIGHT** | arrows | Screen navigation; encoder-cursor movement on param screens. |
| **PAGE** | `-` / `=` | Step-page window prev/next (keyboard advantage over the single hardware button: a direct pair). |
| **SONG** | `o` | Reserved — chain/song screen (CHAIN scope, TK2/TK3). |
| **KEYBD** | `v` | Reserved — chromatic trig mode for melodic engines (OQ-T21). |
| **MUTE** | `m` | Mute screen: trigs toggle mutes. Quick chord: TRK held + FUNC+trig (OQ-T22). |
| **ENC 1–8 ±** | FUNC + trig keys | Column = encoder, top row = +, bottom row = −. Fine = FUNC+Ctrl (hypothesis). |

Retired bindings: `qweruiop` track row, `Tab` mode cycle, number-row
pattern select, global `t` (tap) and `y`/`Y` (yank/paste → FUNC+REC /
FUNC+STOP), `\` leader (families fold into the FUNC plane and the `:`
line; `\` frees up). `:` command line, `?` help, `Backspace` lock-clears,
`Ctrl-C` quit are unchanged.

## Multi-device compatibility (Digitone/Syntakt)

The panel is device-neutral by construction:

- **Pages are positional** (PG1–PG6), labeled from the composite `Rule` —
  a Digitone-alike's SYN1/SYN2/LFO pages land in the same six slots via
  `CANONICAL_PAGE_ORDER`; devices with more pages overflow onto FUNC+PG.
- **Track count is discovered** (TK1 edge-derived discovery): TRK+trig
  addresses up to 16 tracks; 4-track (Digitone-class), 8-track,
  12-track (Syntakt-class) instruments all clamp naturally.
- **Machine-specific screens are capability-gated** (SAMPLING only when
  declared); no Digitakt-only assumptions in the grammar.

## Hold-chord infrastructure and degradation

TRK/PTN/MUTE-chord holds require key-release events (kitty keyboard
protocol, already the ADR-036 posture). Degradation policy without kitty:
**held buttons become sticky one-shot prefixes** (press TRK, next trig
selects, auto-releases) — the vim-prefix shape already familiar from the
leader. FUNC = Shift works on every terminal (modifier flags arrive
per-event). The input layer gains a pressed-set/held-state model either
way; it is pure and unit-testable as before.

## Alternatives considered

- **Keep SEQ/PERF modes, add Elektron chords on top** — rejected: the mode
  split is itself the usability defect (same keys mean different things per
  mode; trigs stop being trigs in PERF). Elektron has no major modes.
- **Track row retained alongside TRK-hold** — rejected: costs the top trig
  row, caps tracks at 8, and splits muscle memory across two grammars.
- **Encoder bank on the number row** (1–8 select + arrows jog) — rejected:
  single-encoder-at-a-time violates the frozen two-param simultaneity
  floor; number row keeps its page/menu role instead.
- **Touch-type split as co-equal toggle** — rejected per user directive;
  the split layout's `Shift+;`/`Shift+/` collide with `:` and `?` on the
  FUNC-encoder plane. Initially proposed as a shipped remap preset;
  dropped entirely at ratification (D3).

## Consequences

- **`input.rs` restructures** from per-mode match to key→`PanelButton`→
  `Action` with held-state; `Mode::{Seq,Perf}` is replaced by
  `Screen` enum + `grid_rec: bool` + held set. The ADR-037 `Keymap`
  re-bases onto the key→button tier (resolves OQ-T19 by construction —
  remapping moves buttons, the grammar stays fixed).
- **`CANONICAL_PAGE_ORDER` in `paraclete-view-assembly` reorders** to
  TRIG, SRC, FLTR, AMP, FX, MOD (from SRC, AMP, FLTR, FX, TRIG, MOD).
  Wire format unchanged; Antiphon/web page *order* changes — call out at
  the next session.
- **Live-trig engine gap (OQ-T20):** REC-off trigs need an audition/
  trig-now command (CMD 38+ candidate) — nothing on the semantic plane can
  fire a voice immediately today. Until it lands, REC-off trigs are inert
  (staged like the TK0 transport gap, filed under the standing directive).
- **Numpad focus-slot cluster (A/B/C) is demoted to an optional
  performance accelerator** — slots become captures of encoders/params for
  sweep performance; the encoder bank is the editing workhorse. Whether
  slots survive at all is session evidence (OQ-T24).
- **TK1-shipped hypotheses superseded before session #2:** `\` leader,
  number-row patterns, `y`/`Y`. Session #2 tests the *new* grammar; the
  TK2 stub is re-cut accordingly (Elektron convergence is TK2's headline).
- Help overlay, mode line, and `:` index regenerate from the panel model —
  the legibility contract (bindings always visible) is unchanged.

## Ratification decisions — 2026-07-23

- **D1** — core model and default key homes accepted as written; chord
  homes remain session-revisable per the ADR-036 convergence rule.
- **D2** — both engine-side extensions pre-approved in direction: (a) the
  live-trig/audition command (exact shape frozen in the TK2 spec, ADR-036
  decision-7 style); (b) `CANONICAL_PAGE_ORDER` reorder to TRIG, SRC,
  FLTR, AMP, FX, MOD (Antiphon/web page order changes with it).
- **D3** — touch-type split grid dropped entirely (no shipped preset;
  hand-writable via ADR-037 remapping).
- **D4** — usability session #2 (TK1 C8) is held until the TK2 S0 panel
  lands; it tests the ADR-038 grammar, not the superseded TK1 chords.

## Implementation note (to be added when implemented)

```text
ADR-038 implemented across TK2 (YYYY-MM-DD).
See design/phases/tk2-theotokos.md for the commit plan.
```
