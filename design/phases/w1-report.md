# Paraclete — W1 Implementation Report (Theoria MVP)

> **Append-only.** One section per commit, written as each ships.
> Spec: `design/phases/w1-interfaces.md`. Ends in paired session #1 (C5).

---

## Commit 0 — `CMD_TRIGGER` (universal) + velocity plumbing — shipped `53caf77`

### What shipped

- **`CMD_TRIGGER = 19`** added to `paraclete-node-api` (`command.rs`) and
  re-exported. Universal instrument command: `arg0` = note (`< 0` → default/
  last note), `arg1` = velocity `0.0–1.0` (`<= 0.0` → `0.79`). Same retrigger
  path as a `NoteOn`. Implemented in `AnalogEngine`, `FmEngine`, `Sampler`;
  other nodes ignore it (unknown-command silence).
- **Sampler** already had a local `CMD_TRIGGER = 19` (hardcoded
  `trigger_voice(60, u16::MAX/2, 0)`); it is now an alias of the shared const
  and parses `arg0`/`arg1` for real, defaulting the note to `root_note`.
- **Velocity plumbing (universality sweep):** `retrigger(note)` →
  `retrigger(note, velocity)`. A `NoteOn`'s 16-bit velocity maps `vel/65535`
  → `0..1`; that becomes a linear output-level multiplier (`velocity_level`),
  applied allocation-free at the output copy (mono engines) / per-voice
  (Sampler). Full velocity = unity gain = pre-W1 level. `Sampler::Voice`
  gained a `velocity_level` field; the two mono engines gained
  `velocity_level` + `last_note`. Velocity-mod routing is P12+ — only the
  plumbing landed.

### Behavior change to flag for the paired session (design tiebreaker)

Velocity now scales output for **all** `NoteOn`-driven playback, including
sequencer steps — which previously ignored velocity entirely. Consequence:

- The default 8-track preset (`apply_preset`, velocity `40_000` ≈ 0.61) now
  plays ~4 dB below its pre-W1 level.
- `Step::empty()`'s default velocity is `32768` (≈ 0.50); any step programmed
  without an explicit velocity plays at half level.
- A `CMD_TRIGGER` with default velocity uses `0.79`, so a live-triggered pad
  is louder than a default sequencer step — an asymmetry worth a decision.

This is **spec-mandated** (§Commit 0: `vel/65535`, `CMD_TRIGGER` default
`0.79`), so it shipped as specified rather than being redesigned inline
(guardrail 1). The open question for session #1: should the default step
velocity rise (e.g. to ~52000 ≈ 0.79 for parity with `CMD_TRIGGER`), or is
per-step velocity control the intended way to reach full level? Recorded here
so it is decided with the instrument in hand, not guessed.

### Found while here (filed, not fixed)

- **BUG-015** — `FmEngine` routes `ParamLock` events through
  `bank.handle_commands()` (converts each into a synthetic `CMD_SET_PARAM`),
  the exact anti-pattern CLAUDE.md's ADR-019 ParamLock section forbids: the
  locked value permanently mutates the bank and bleeds into later steps.
  `AnalogEngine` uses the correct `node_locks` pattern. Pre-existing; left
  untouched to keep C0 scoped. Fix direction in the tracker.

### Gate results

- `cargo test --workspace`: **450 passed, 0 failures** (441 baseline + 9 new).
- New tests (per engine): `cmd_trigger_produces_audio`,
  `cmd_trigger_negative_note_uses_default`, `velocity_scales_output_level`.
- `cargo clippy -p paraclete-nodes -p paraclete-node-api --all-targets`: no
  new warnings vs baseline (verified by A/B stash; the one `single_match` hint
  near `trigger_voice` is the pre-existing `NoteOn` arm, line-shifted).
- Scope: `paraclete-node-api` (const + re-export) and the three engine files
  only.

### Review

Direct orchestrator read of the full ~250-line diff plus independent
test/clippy verification (proportionate to a small, well-understood mechanical
commit; the 8-angle fan-out was reserved for the gated P10 C1 data-model
change). Verified: consistent velocity math and clamping, allocation-free
output scaling, sensible defaults, state reset in `activate()`, valid tests.
No correctness issues found; BUG-015 filed as a pre-existing neighbor.

---

## Commit 1 — state-path unification + BUG-007 path caching — shipped `c9468b9`

### What shipped

- **Canonical path scheme frozen** (documented in CLAUDE.md → StateBus):
  param values move `/node/{id}/{name}` → `/node/{id}/param/{name}`;
  `/node/{id}/state/{key}`, `/transport/*`, `/context/*`, `/script/*`
  unchanged; `/hw/*` → `/surface/{id}/*`. This matches the sandbox rule that
  was already correct (`write_sandboxed` permitted `/node/{id}/param/*`) — the
  publisher, not the rule, was wrong.
- **BUG-007 fixed** (resolved in tracker): `publish_bank_state` caches its
  formatted paths in a per-`ParameterBank` `std::sync::OnceLock<Vec<String>>`,
  built lazily on first call and cloned thereafter — no `format!` on the audio
  thread after the first cycle. Signature unchanged, so all 11 caller nodes
  are untouched. `Sequencer::published_state()` got the same treatment
  (`OnceLock<[String; 9]>` for its fixed `/state/*` keys). The residual
  per-entry `String::clone` is retained by design (the buffer ships owned
  strings to the main thread; full elimination needs BUG-006's return channel).
- `/hw/`→`/surface/` in the sandbox doc + test; TUI encoder row + affected
  tests read the new `/param/` path.

### Scope came in smaller than the spec's file list

I verified before implementing (recorded so the smaller diff isn't mistaken
for incompleteness):

- **Profiles need no change** — `profiles/*.rhai` read only
  `/node/{id}/state/*` and `/script/*`, never param paths. The spec's "update
  all `profiles/*.rhai`" over-stated it.
- **No `/hw/*` writers or readers exist** anywhere except one doc comment and
  one sandbox test, so that rename was doc + test only.
- The only reader of the old param scheme was the TUI encoder row (one line).

### Deviations / boring-option choices

- Sequencer's conditional `track_name` `/state/` entry is **not** cached
  (still inline `format!`): caching it cleanly would need a second cache keyed
  on "present at first call," fragile if `track_name` is set post-construction
  but pre-first-publish. It is one optional entry against nine cached ones —
  documented in-code.
- `paraclete-antiphon/src/protocol.rs` has a JSON round-trip **test fixture**
  using the old `/node/20/cutoff` string. It is fixture data, not a scheme
  assertion, so C1 left it. **Carried to C2:** when the live state mirror is
  wired, the emitted paths must be the new `/param/` scheme; the fixture will
  be aligned there (C2 is in that crate anyway).

### Gate results

- `cargo test --workspace`: **452 passed, 0 failures** (450 baseline + 2 new:
  `publish_bank_state_uses_param_prefix`,
  `publish_bank_state_allocates_no_paths_after_first_call` — pointer-stability
  proof that `format!` does not re-run).
- Clippy A/B (`-p paraclete-node-api -p paraclete-nodes -p paraclete-tui`,
  forced full recompile): identical warning locations; zero new.
- 3 existing param-path test assertions updated (parameter.rs, tui_tests.rs,
  distortion.rs); `/state/`-path assertions left untouched.

### Review

Orchestrator-designed (the `OnceLock` caching mechanism was specified before
implementation to resolve the BUG-007 buffer-ownership subtlety), then direct
read of both substantive diffs + independent gate re-run. Verified: correct
`OnceLock` lazy init keyed on `node_id`, faithful index→value mapping in the
Sequencer cache, no new audio-thread allocation beyond the accepted clone.

---

## Commit 2 — kerygma state mirror v1 — shipped `3672d1a`

### What shipped

- **`StateBusHandle::iter()`** (L2, additive): full-scan access so the mirror
  can diff the whole bus each pump.
- **`AntiphonHandle::pump(bus, now_ms)`** (main-thread, called at main-loop
  step 1.5): diffs the bus against a per-path `state_shadow`, coalesces
  changes into a `pending` HashMap (repeated writes to one path collapse to
  the latest), and flushes a `ServerMsg::State` batch at a 33 ms cadence.
  Batch cap 256 with immediate overflow flush mid-scan (no drops). Numeric
  coercion `Int`/`Bool`/`Float` → f64; **`Text` skipped** (steps bitfield /
  track_name — the grid renders from LED, names from `NodeSummary`).
- **`context`**: full 8-slot `/context/*` snapshot emitted whenever any
  context path changes.
- **`publish_topology(nodes)`**: `ServerMsg::Topology` hook. No live runtime
  `apply_patch` call site exists at W1 (dynamic topology is test-only), so the
  method + test ship as the ready hook for when apply_patch is wired to a
  trigger — no trigger was invented.
- App: monotonic `Instant` clock; `pump` at step 1.5 after
  `process_main_thread`. protocol fixture aligned to the `/param/` scheme.

### Subagent failure + recovery (process note)

The implementer subagent died mid-task on an API connection drop. It had the
code compiling but had **not** run its own gate and had written only 1 of the
6 spec'd tests. On recovery I reviewed the full diff and found a **real
correctness bug**: `context_dirty` was a per-`pump()` local, so a `/context/*`
change during a non-due pump updated the shadow (preventing re-detection next
pump) but was never flushed — the update was silently lost. Fixed by making it
a persisted field cleared only on flush; added
`context_change_during_non_due_pump_is_not_lost` as the regression guard. Also
completed the 6-test suite and resolved an `is_none_or`-vs-MSRV-1.75 clippy
conflict (the crate's MSRV is 1.75; `is_none_or` is 1.82) with an explicit
`match`.

### Flags for the paired session (design decisions to validate)

1. **Context enc-id mapping is an assumption.** `publish_context` keys are
   strings (`/context/encoder_3/...`) but `ContextSlot.enc` is a `u32`. The
   mirror derives `enc` as the trailing integer of the key (`encoder_3` → 3).
   This assumes one profile-encoder-key per slot; when the web encoder row
   (C4) binds ids 90–97, this may need a defined key→id map instead. Marked
   in-code and here.
2. **No per-client initial state sync.** The mirror broadcasts *diffs*; a
   client connecting mid-session gets `welcome` (which carries `NodeSummary`
   params at their **default** values) and then only live changes — so current
   non-default param values are not shown until they next change. Fine for the
   single-tablet W1 MVP; a per-client welcome-time state snapshot is a
   post-W1 / C4 refinement. Flag if a second tablet joining mid-jam feels
   wrong in session #1.

### Gate results

- `cargo test --workspace`: **459 passed, 0 failures** (452 baseline + 7 new:
  `StateBusHandle::iter` + 6 pump tests: diff-only, coalescing window,
  overflow-without-drops, skips-text, context-snapshot-shape, context-dirty
  regression).
- Clippy clean on `paraclete-antiphon` (0 real warnings).

### Review

Orchestrator-scoped (value coercion, iteration API, and the context/enc
decisions were specified before implementation). After the subagent's partial
delivery I did a direct full-diff review — which caught the `context_dirty`
bug the incomplete self-gate would have missed — plus the test completion and
independent gate re-run.

---

## Commit 3 — semantic plane (set_param / bump_param / node_cmd) — shipped `004bf77`

### What shipped

- **`FrameAction::Command(NodeCommand)`** + `route_frame(msg, &SessionInfo)`.
  `resolve_semantic()` validates each semantic-plane message against the
  cap-doc snapshot (`SessionInfo.nodes`):
  - `set_param` — resolve param name → id, clamp `v` to `[min, max]` →
    `CMD_SET_PARAM`.
  - `bump_param` — resolve id, clamp `delta` to ±0.5 per message (the node's
    bank clamps the *result* to range) → `CMD_BUMP_PARAM`.
  - `node_cmd` — reject `cmd < 16` (universal range must use the typed
    messages), require a known node, else pass-through.
  - Unknown node/param → log + `Drop`, never disconnect (ADR-019 discipline).
- **Command channel**: `std::sync::mpsc` from client threads → main loop.
  Each client thread holds a `Sender` clone; `AntiphonHandle` owns the
  `Receiver` and exposes `drain_commands()`. Many producers → one consumer;
  off the audio thread, so mpsc allocation is fine (rtrb is SPSC and wouldn't
  fit the multi-client shape).
- **App**: drains antiphon commands into `conf.send_command()` at main-loop
  step 5, beside the script-command flush. `Enc`/`EncPush` untouched — those
  are the C4 surface-plane path, not the semantic plane.

### Notes

- `resolve_semantic` is a private helper with a controlled caller (only the
  three semantic variants reach it); its final `unreachable!` is a
  programming-error guard, not a runtime path.
- One pre-existing test was renamed for accuracy (its old name claimed the
  semantic variants always drop at W1; they now drop only when unresolvable —
  asserted via an empty-node session). No behavior change.

### Gate results

- `cargo test --workspace`: **466 passed, 0 failures** (459 baseline + 7 new:
  name resolution, value clamp, delta clamp, unknown-target drop, `cmd<16`
  rejection, unknown-node drop, and a channel FIFO-drain test).
- Clippy clean on `paraclete-antiphon` (MSRV 1.75 respected — no `is_none_or`).

### Review

Orchestrator-designed (the `FrameAction::Command` variant, the
`route_frame(&SessionInfo)` signature, and the mpsc-not-rtrb channel choice
were specified before implementation). After delivery I independently
recompiled (the reported "466 pass" was contradicted by a stale mid-edit
diagnostic — verified it was stale), re-ran the gate, and read the
`resolve_semantic` logic + channel threading directly. Correct.

---

## Commit 4 — theoria-web (TS workspace + encoder row + embedded bundle) — shipped `11608c8`

### What shipped (builds green; interactive acceptance is C5/paired session)

- **`web/` npm workspace** (Node 26 / npm 11):
  - `@paraclete/core` — transport-agnostic, no DOM: TS protocol types
    mirroring `paraclete-antiphon/src/protocol.rs`, a WS connection wrapper
    (token, hello/welcome, reconnect/backoff, staleness), a path-keyed state
    store + a context store fed by `state`/`context` messages, and a
    gesture→detent helper.
  - `@paraclete/app` — Preact + vite + canvas: **Grid** (ported from W0 —
    pads, LED colors, pad_down/up), **EncoderRow** (ids 90–97; vertical drag →
    detents → `{t:"enc",id,delta}`, coalesced per animation frame; cells show
    param name + live value from the C2 mirror + the C3 context mapping;
    p-lock flash), **TransportBar** (play/BPM read-only from `/transport/*`,
    track-select). W0 `web/theoria/` deleted.
- **Relative-only encoders** upheld — the wire carries detent *deltas*, never
  absolute values.
- **`embed-ui` cargo feature** (opt-in, not default): `include_dir!`s
  `web/packages/app/dist` at compile time. `antiphon/http.rs` gains a
  `StaticSource { Disk | Embedded }` enum sharing one traversal-safe path
  resolver (`resolve_rel_path` rejects `..`/empty/`\` segments — W0's guard
  preserved); `--theoria-dir` still overrides for dev. Plain `cargo build` /
  `cargo test --workspace` never require `dist/` or pull `include_dir`.

### Build step to note (release with embedded UI)

`dist/` is a build artifact (gitignored). Building with `--features embed-ui`
requires the bundle to exist first: `cd web && npm install && npm run build`,
then `cargo build -p paraclete-app --release --features embed-ui`. The feature
is opt-in precisely so a plain build never depends on the frontend toolchain.

### Gate results

- `npm run build`: green (core + app; 25 KB app bundle; `tsc --noEmit` clean).
- `cargo build -p paraclete-app` (default) and `--features embed-ui`: both
  compile; embedded asset filenames verified present in the feature binary.
- `cargo test --workspace`: **466 passed, 0 failures**.
- No new clippy warnings on the Rust changes (all 9 workspace warnings are
  pre-existing in unrelated crates).

### Needs paired-session (C5) validation

Touch/pointer feel on canvas, encoder coarse/fine gestures, p-lock flash
timing, reconnect on a real Wi-Fi drop, RTT overlay, PWA/install (not built —
W0 approach carried). **Known gap:** `TransportBar` track-select read-back is
optimistic/local-only — the C2 mirror allowlist excludes `/script/*`, so
`/script/lp/selected` is not live-mirrored. Either extend the mirror to a
`/script/lp/*` subset in a later commit or accept local-only for W1; decide
at the session.

### Review

Bounded to a green build (interactive UI is inherently paired-session work).
After delivery I independently verified both cargo configs compile (the
reported success was contradicted by stale mid-edit E0425/E0308/`unexpected_cfgs`
diagnostics — confirmed all stale), re-ran the full gate, confirmed the
`embed-ui` feature wiring in both Cargo.tomls and that the default build has
zero cfg warnings, and read the static-serving path-safety refactor.

---

## W1 status at end of autonomous run

Commits 0–4 shipped (`53caf77`, `c9468b9`, `3672d1a`, `004bf77`, `11608c8`,
each with a paired design-docs commit). The **runtime side of W1 is complete
and test-verified** (466 passing): pads live-trigger with velocity, the state
path scheme is frozen and cached, the tablet receives a coalesced live state
mirror, and it can drive parameters/commands through the validated semantic
plane. The **web client builds** as a proper workspace with the new encoder/
transport views and an embeddable bundle.

**Remaining before the milestone: C5 — paired session #1** (user, not
autonomous). That session exercises the tablet end-to-end (the exit-criteria
checklist in `w1-interfaces.md` §Commit 5), captures findings in
`design/sessions/s1.md`, and produces the roadmap delta that re-validates the
P10 C2–C5 ordering. Carry-in items for the session: the default step-velocity
loudness question (C0), the context enc-id mapping assumption (C2), the
`/script/*` mirror gap for track-select read-back (C4), and BUG-015
(FmEngine p-lock bleed) if an FM track is used.
