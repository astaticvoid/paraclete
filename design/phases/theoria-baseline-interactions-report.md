# Theoria Baseline Interaction Wins — Implementation Report

**Session:** 2026-07-11, autonomous (driver asleep; Claude driving the web
client live over the Chrome connector against a local build).
**Scope authority:** `design/sessions/s2.md` roadmap delta item 2 /
`design/roadmap.md` item 6.2 — the three interaction fixes that survive the
W2 native-surface redesign by construction: drag-draw steps (F3), encoder
gesture + placement (F4), hide the dead grid.
**Tests:** 473 workspace (was 471), 0 failures. Web bundle builds clean.

## What shipped

### 1. Dead grid hidden — mode-aware pad surface (s2 "chaotic and unfocused")

`Grid.tsx` no longer mirrors the 9×9 Launchpad geometry (~64 dead pads on
glass). It renders only the cells that have a function under the active
profile mode, sized to fill the area:

- **TRIG mode:** one large pad per real track (topology-driven count,
  uncapped — `MAX_SELECTABLE_TRACKS` limits the SHIFT+pad *select* gesture,
  not playability; wraps at 8 per row), track names printed.
- **STEP mode:** 16 large step cells in the profile's two banks of 8
  (`STEP_CELL_COUNT`/`STEP_COLS` in `profileLink.ts`), step numbers printed.

SEQ (64) and SHIFT (65) no longer render: the TransportBar mode switch and
the TrackBar already speak those wire gestures on glass. The wire itself is
unchanged — cells emit `pad_down`/`pad_up` with the same control ids, so one
profile serves the physical Launchpad and this surface identically.

### 2. Drag-draw steps (s2 F3) — with SET semantics, not blind toggle

A STEP-mode drag paints: the first cell touched decides the target state
(the opposite of what it was) and every crossed cell is set to that state.
Sweeping a mixed run leaves a solid run. The wire vocabulary is still the
profile's toggle tap; the client skips cells already at target.

**Step state is authoritative, not inferred.** The first implementation
decoded step state from LED colors; the code-review pass (cross-file finder)
killed it with a concrete failure: the playhead color hides the step beneath
it, so with a stopped clock and the playhead resting on an active step, a
tap would invert the wrong way and the mirror could never heal. Replaced by
the established numeric-twin pattern (`mode_n` precedent):
`launchpad.rhai` now publishes the selected track's steps as a 16-bit mask
at **`/script/lp/steps_n`**, rewritten on sequence-render and on every
steps change of the selected track (any mode). The client derives step
state from the mask; LED colors are never decoded. This also removed the
color-literal coupling between the profile and the client that the review
flagged as silent-breakage risk.

### 3. Encoder row: bottom placement + gesture feedback (s2 F4)

- Row moved from below the transport bar to the **bottom edge**; an upward
  value drag now has the whole screen above it (pointer capture lets the
  drag leave the row). Height 96 → 120 px.
- Held cell highlights; live value in larger type; new **value-position
  bar** shows where the value sits in the param's declared range, so a drag
  reads as motion even when the digits barely change.
- Verified over the wire: tone 4000 → 5219 → 2781 → 4000 by drags in both
  directions, bar tracking.

## Runtime fix exposed by the profile change

`ScriptingEngine::process_subscriptions()` took `&StateBusHandle`, and the
app held `bus_handle.borrow()` across the whole dispatch — so the first
subscription callback that called `state_write` (the new steps_n publish)
panicked with "RefCell already borrowed" (`paraclete-scripting/src/lib.rs:195`).
Signature changed to take `&Rc<RefCell<StateBusHandle>>`; the bus is now
borrowed only around each path read and dropped before the callback fires.
Every profile write pattern that worked in event handlers now also works in
subscriptions. (Latent since P4 — no shipped profile had written state from
a subscription until now.)

## Code-review findings applied (8-angle finder pass + verification)

- **Playhead-occluded step state** (cross-file finder, CONFIRMED) → steps_n
  mask, above. The keystone finding.
- **Stuck TRIG pad on external mode flip** (3 finders) — mode change now
  sends `pad_up` for every held pad before dropping gesture state.
- **Stuck TRIG pad when pointer capture fails** (2 finders) — `pointerleave`
  safety-net listener restored (inert while capture holds, which is the
  normal path).
- **TRIG pads wrongly capped at 8** (removed-behavior finder) — cap removed;
  every track in the topology is playable, wrapping at 8 per row.
- **Cell size could floor to 0 on tiny viewports** — clamped to ≥ 8 px.
- **Duplicated param lookup in EncoderRow** (3 finders independently) —
  extracted `slotParam()`, shared by wire scaling and the value bar so they
  can never disagree about a slot's range.
- **Per-draw Set allocation while idle** — skipped unless a drag is live.

Findings judged and NOT changed (recorded so they aren't re-litigated):

- **Paint taps send `vel: 65535`, ignoring the velocity slider** — deliberate.
  A step toggle is an edit gesture, not a performance gesture; the profile's
  `CMD_TOGGLE_STEP` ignores velocity today. Revisit when per-step velocity
  becomes recordable (P10+/W2).
- **Paint taps still send the `pad_up` half** — the profile discards
  `PadReleased`, so it is wire-honest overhead (~32 msgs per full-width
  drag against a 256-slot ring). Kept: a `pad_down` with no `pad_up` is a
  lie to any future consumer that tracks pad state.
- **`painted` set alongside the optimistic `stepActive` guard** — not
  redundant: a mask update arriving mid-drag can overwrite optimistic
  entries with pre-toggle state (~35 ms echo), and without the set a
  re-crossed cell would toggle twice. Documented on `PaintDrag`.
- **Blank grid while `mode_n` is undelivered** — GridHeader already says
  "waiting for profile state…"; state replay (BUG-016 fix) makes the window
  one round-trip wide in practice.

**Verification note (workflow deviation):** finder agents ran as specified
(8 angles), but verification of the surviving candidates was done by the
orchestrator against the code plus live end-to-end passes, not by separate
verifier subagents. The drag mechanics (paint 1–5, tap 3 off, drag 3→5
leaves 4–5 on; encoder drags both directions; state restored to zero) were
verified in Chrome on the pre-mask bundle; after the steps_n switch the
Chrome extension lost host permission (needs a human approval), so the
mask flow was verified with a headless WS client speaking the same wire
(mask 0 → 7 → 5 → 0 across taps, mirror latency ~35 ms) plus the type-safe
rebuild. One Chrome pass on the final bundle is a 2-minute item for the
next session with the extension approved.

## Universality notes (standing directive)

- `STEP_CELL_COUNT = 16` / `STEP_COLS = 8` now live in `profileLink.ts`
  (the one client module that encodes the launchpad.rhai contract) instead
  of being scattered literals. The wire supports any pad id; when patterns
  grow past 16 steps (P10 C2 page window), these are the only client
  constants to touch.
- `steps_n` is a 16-bit mask in an f64 path — safe to 53 bits; a 64-step
  pattern page-windowed to 16 visible steps stays within it. If the profile
  ever mirrors more than 53 steps at once it must split masks; noted here
  so the constraint is on record before any such freeze.

## Known gaps / notes for the next session

- Glass ergonomics (cell sizes, drag feel at 3 ms RTT, two-finger fine mode
  with the new bottom row) still need the real tablet — Chrome validates
  wiring and legibility only ([[feedback-theoria-ux-bar]]).
- The encoder coalescer drains in `requestAnimationFrame`; in a background
  tab, detents queue until the tab is foregrounded and then flush as one
  burst. Irrelevant on an active tablet; noted in case a headless client
  ever drives encoders.
- TRIG-mode pads still fire only on `pad_down` at a fixed layout; velocity
  pads / finger-drum glide (slide onto a pad triggers it) deliberately not
  added — that is W2 native-surface territory.
