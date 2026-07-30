# Theotokos — Usability Session #3

**Date:** 2026-07-29
**Phase:** TK2.1 C8 (exit criterion) — judges the ADR-044 panel redesign built in C0–C7
**Session type:** hands-on, default 4-track instrument (`paraclete-default`, 140 BPM,
Kick/Snare/HiHat/Bass), `./target/debug/paraclete`, run **in kitty 0.48.1** with the
agent observing and driving single keys over `kitty @ --listen-on unix:@tk3`

## Verdict: the redesign is CONVERGED — the panel is right. Four bugs and two structural design collisions were found inside it. TK2.1 does not close on this session; a fix pass and a session #4 follow.

Session #2 reopened `design.md` §5.1/§5.2 because the panel read as a
Launchpad-style LED grid and the REC posture inverted the finger-drum
model. **Both complaints are discharged.** The fixed panel, the
pads-by-default mode model, the REC toggle + REC-hold+PLAY escalation, and
engine-side live record were all judged good under the hand on first
contact. What did *not* survive contact is narrower and deeper: the
**legend/chip layout model** (D3/D4) needs a second pass, and **ENC mode
(D9) and p-lock authoring (D15) collide over ownership of the bare trig**
in a way that no local fix reaches.

This session covered 7 of the 11 hypotheses in the phase spec's §5 —
every hypothesis that was load-bearing for the redesign, including all
three that had reopened `design.md`. The remaining four were stopped at
the user's direction ("we have plenty to consolidate and fix so far")
rather than run thin.

## Environment note (why this session ran in kitty)

The dev box's only terminal is **konsole**, which does not implement the
kitty keyboard protocol — so key *releases* are unavailable there and the
whole release-dependent half of the grammar (REC-hold+PLAY, momentary
p-lock, TRK/PTN hold feel) is unreachable. kitty was installed for this
session; SETTINGS confirmed `kitty keyboard protocol: yes`. The session
deliberately did **not** run through tmux: tmux's `extended-keys` is
CSI-u/modifyOtherKeys and does not proxy release events, which would have
silently forced the sticky fallback and invalidated H3/H7/H11 without
saying so. This environment fragility is what motivated the new **TKW**
roadmap track (a platform-agnostic host that owns the keyboard itself).

## Converged hypotheses

| Hypothesis | Verdict |
|---|---|
| **Fixed panel reads as a front panel, not an LED grid** (D1/D2) | **Converged.** "I like it. It's digital, it's hardcore, it feels like weird unix and hacker culture." **Record the nuance:** the endorsement is of a *terminal-native* identity, not of D1's literal goal ("reads as a hardware front panel"). What the user values is that it looks like a terminal, not that it mimics an Elektron. **Consequence: stop treating hardware skeuomorphism as the target.** This bears directly on TKW — a rasterised native window re-rendering the panel as faux-hardware moves *away* from what the user likes; the WASM/browser route preserves the text character. Session #2's §5.1/§5.2 reopening is discharged. |
| **REC toggle + REC-hold+PLAY reads as the reference box's grammar** (D5, R1) | **Converged.** "1, 2 works good. I like it." Both the bare-`z` Off↔Grid toggle and the hold-`z`+`x` escalation to `Live` read correctly under the hand. Worth noting this decision was withdrawn and rewritten at the user's direction *after* the design review, so it carried the least review of anything in the phase — and it is now confirmed by play. |
| **Trig N = track N in pad modes gives real finger-drumming** (D6/R4) | **Converged.** "It's great." No mode error: bare trig played and selected, nothing was written unexpectedly. This discharges session #2's H8 finding (REC-armed-by-default) and closes the `design.md` §3.A points 3–4 implementation gap. |
| **Engine-side live record feels tight at 120–140 bpm** (D8) | **Converged.** "It's pretty good," judged at 140 BPM. Confirms D8 and ADR-039 decision 7's early `live_rec` slice. **BUG-042 (double-trigger near a step boundary) did not surface** — absence of evidence, not disproof; it stays deferred on unchanged terms, with no session verdict either way. |
| **Silent launch** (D7) | **Converged**, verified by the agent on a pristine boot rather than by feel: transport `■`, `REC○`, `Step:1`, strip empty. Pads-by-default is the boot posture. |

## Revision findings

| # | Finding | Source | Action |
|---|---|---|---|
| **F2** | **Key hints must sit adjacent to their referent; the legend strip should carry only what is *not* otherwise on screen.** "I feel like all key hints should be close to their referent. So z close to rec at bottom etc. The extra row should be for stuff that isn't directly on screen." | H2 (D3/D4) | **Substantial revision of D4's legend model.** Today the legend is a flat two-line list of every binding, including keys whose referent is already drawn elsewhere (`[z] REC` while `REC○` sits in the transport line; `[p] PTN` while `PTN P1` is on the track line; `[0] TEMPO`). Under F2 the legend becomes a **residual** list — off-screen affordances only — and each on-screen element carries its own key inline (e.g. the transport's REC glyph renders as `[z]REC○`). This generalises to the whole panel the principle D3's chips already apply to trigs/tracks/encoders. |
| **F3** | **Pad chips should vanish in `Grid` like the trigs do, but must not reflow.** "the pads should not have gray keys in trig mode, they should disappear like the trigs do. But they should not have gaps disappear. When I press z, the trig sounds all move left because the [x] filler disappears." | H2 (D3) | Measured: `REC○` → `▸[q]1 Kick   [w]2 Snare   [e]3 HiHat   [r]4 Bass  PTN P1`; `Grid` → `▸1 Kick   2 Snare   3 HiHat   4 Bass  PTN P1`. `[k]` is dropped outright, shifting each name 3 columns left (12 cumulative at 4 tracks) and dragging `PTN P1` with it. Two distinct sub-parts: **(a)** hide the glyph rather than dimming it to gray, **(b)** reserve its columns so nothing moves. D1 guaranteed fixed region *heights* and never column stability — this session extends the guarantee to columns. Implementation caution: the reserved width must match what the chip *would* have been, and `chip_key_display` can yield multi-char labels (e.g. `Tab`), so a hardcoded 3 spaces is wrong for remapped keys. Current behaviour: `render.rs:487-495`. |
| **F8** | **`[n] ENC` and `[m] LOCK` are absent from the Grid legend.** "I don't see [n] in legend btw." | H6 (D9) | `render.rs::legend_chips_for_screen` (`:272`): the `Screen::Grid` list has neither chip; both appear only in the `Screen::Param(_)` list and in the `enc == true` override — so `[n]` becomes visible only once you are *already* in ENC mode. **`[m] LOCK` is the worse case: p-lock arming is a `Grid`-only gesture, advertised only on screens where it does not apply.** Contradicts D3/D4's premise that the panel reads without `?`. Folds into F2 — under a residual-legend model these two belong in the strip precisely *because* they have no on-screen referent. |
| **F1** | **Chips will be squashed at 16 assigned trig sounds.** | H2 | User prediction, not an observation — today's instrument is 4 named tracks and fits easily. D2's `‹`/`›` windowing exists, but *names* are the width pressure, not count. **Cannot be closed until a 16-track instrument exists to test against**; no such instrument exists today. |
| **F6** | **Quantization control is missing.** "We will definitely want quantization control though." | H5 (D8) | New requirement → **OQ-T29**. Live record is currently *always* record-as-played: `record_live_trig` snaps to the nearest step **and** writes micro-timing in 96th units (`sequencer.rs:1388`; `live_rec_writes_micro_timing_in_96th_units`, `:4987`). There is no hard-quantize path, so a clean on-grid take is unobtainable. Wanted: at minimum record-as-played vs. snap-to-step, plausibly a strength or grid division. **Weigh non-destructive seriously** — the micro offsets are already persisted per step, so quantize-as-a-playback-setting is cheaper than it sounds. Interacts with BUG-045 and OQ-T25. |

## Structural design collisions (the session's most important output)

These two are one root cause seen twice, and neither is reachable by a
local fix. **D9 makes trigs *be* the encoders; D15 uses trigs as *step
selectors*.** They cannot share the bare trig.

**F7 — ENC mode and momentary p-lock collide; ENC mode is unusable in `Grid`.**
User: "you try it, it's broken. if I tap n and hold q it bumps param only for
step q. If I tap a it only bumps param for step a. both seem to change pitch, up
for q and down for a, again only for step 1 and 9. so i guess plock is implied.
not obvious what's being modified either." — and, on the concept itself, "the
idea is much improved over chord though."
`lib.rs:626` gates the momentary-lock arm on `self.held.kitty &&
self.held.armed.is_none() && self.model.rec == RecMode::Grid` — **not** on ENC
being off. So in `Grid`+ENC one trig press simultaneously arms a momentary
p-lock target (D15) *and* jogs that encoder (D9); the jog then routes to the
lock rather than the live value, confirmed at `lib.rs:1106` (`if let Some(step) =
lock_step_for_active_track()` → `CMD_SET_STEP_LOCK`, `else` → `CMD_BUMP_PARAM`).
The observations line up exactly: `a` = col 8 = step 9, bottom row = downward,
encoder 1 = `tune`, hence "changes pitch, only steps 1 and 9".
Three casualties — the third was not user-reported but follows structurally and
should be verified before fixing: (1) ENC mode cannot edit a live parameter at
all while in `Grid`; (2) nothing on the panel says a lock rather than the live
value is being written, nor which parameter — "not obvious what's being modified
either"; (3) bare trig in `Grid`+ENC no longer writes steps, so ENC silently
disables `Grid`'s primary gesture.
**Verdict: D9's direction is converged** ("much improved over chord") and should
stand; the implementation needs a decision about which control owns the bare trig
per mode. Adding `&& !enc` to `lib.rs:626` removes the symptom but leaves p-lock
authoring with no gesture whenever ENC is on. Options to weigh: p-lock requires
the explicit `[m] LOCK` arm whenever ENC is on; ENC and `Grid` become mutually
exclusive; or the lock target moves to a different control entirely.

**F11 — momentary p-lock cannot express the diagonal case; recommend retiring it.**
The user's own diagnosis, and it is decisive: "how would holding q for plock? how
would you jog q on step q?"
- To p-lock encoder *N* on step *N*, the hand must hold trig *N* (to address the
  step) and jog encoder *N* — which is `Shift`+trig *N* with ENC off, or bare trig
  *N* with ENC on. **The same physical key.** Unrepresentable. This is a hole in
  the gesture's domain, not a tuning problem; no gate or timing change reaches it.
- Independently, a bare trig press in `Grid` fires `ToggleStep`, so holding a step
  in order to lock it also writes-or-erases it — and the toggle cannot be
  suppressed at press time, because whether a hold "means" a lock arm is only
  knowable if a jog follows.
- **Latched has neither problem**, because the hold is released before the jog:
  `m`+`q` arms step 1 → release → `Shift`+`q` jogs encoder 1 → the lock lands on
  step 1. The diagonal works. Latched is therefore *strictly more expressive* than
  momentary on this surface.
- **Recommendation: retire D15's momentary path**; latched `[m]` LOCK becomes the
  single p-lock gesture. The irony worth recording: momentary was the gesture
  borrowed *from hardware* for authenticity, but hardware has two hands on two
  distinct physical control surfaces — the keyboard collapses steps and encoders
  onto one set of keys, so the borrowed gesture cannot survive the translation.

**Consequence: OQ-T27 (p-lock authoring gesture) is reopened**, having been
recorded as resolved by ADR-044 D15 / C5b.

## Bugs filed

| Bug | Summary | Severity |
|---|---|---|
| **BUG-043** | Transport has neither pause nor stop: `c` is inert (`input.rs:857`, "Bare STOP has no meaning yet"), and `x` always rewinds because `CMD_CLOCK_START` sets `first_tick` → `global_start` → sequencer position reset. STOP is advertised in the legend, `?` overlay, and README while doing nothing. **Needs an engine-level decision, not a patch** — there is no "resume" vocabulary, and any fix changes transport semantics for every surface. | High |
| **BUG-044** | A live pad trig sounds two octaves above the same track's sequenced steps — `CMD_TRIG_NOW` resolves `arg0 <= 0` to a hardcoded `60` (`sequencer.rs:809`) instead of the track's `default_note` (36 here). Missed by the suites because the test builds a bare `Sequencer::new()`, whose default *is* 60. | High |
| **BUG-045** | A hand-written step inherits stale micro-timing from an erased live-recorded step; all three manual write paths ignore `timing.micro_offset` (`sequencer.rs:601`, `:608`, `:620`), so even a lane clear leaves offsets behind. | Medium |
| **BUG-046** | Holding a trig in `Grid` rapid-toggles the step — `lib.rs:681` suppresses OS auto-repeat for `PanelButton::Rec` only. Prerequisite for any coherent hold-a-trig gesture, so it should land with the OQ-T27 decision. | Med-High |

Also noted, not filed: `--no-tui` still paints the legacy Launchpad emulator
despite `main.rs:78`'s comment calling it the headless path. Cosmetic.

## Not judged this session (no verdict — carried to session #4)

| Item | Why |
|---|---|
| **H8** — descriptor-accurate jog fixes "no variable step size" (D10) | Set up but not judged; session stopped first. **Its stepped half is untestable on this instrument regardless:** no machine engine declares `stepped: true` — stepped params exist only on filter/envelope/distortion nodes, which the Theotokos param pages do not reach. Needs an instrument that exposes one. Related asymmetry found while setting up: with ENC **off** there is no coarse jog at all (only `Shift` = normal, `Ctrl+Shift` = fine, `input.rs:801-811`), whereas ENC **on** offers bare/`Shift`/`Ctrl` = normal/coarse/fine. |
| **H9** — sticky re-tap + 400 ms auto-repeat guard (D11) | Not reached. Note BUG-046 shows the repeat guard is absent for trigs, so H9's judgement should be made *after* that fix, against the intended behaviour. |
| **H10** — FUNC+transport copy/clear/paste ergonomics | Not reached. Already explicitly deferred by R5 and by session #2, so re-deferring costs nothing. |
| **H11** — TRK/PTN physical feel; encoder-bank simultaneity | Not reached. |
| **OQ-T23b** — tap tempo behind a screen; global chord? | Not reached. |
| **OQ-T24** — numpad slot cluster fate | Not reached, and **not testable as built** — the numpad input side is unwired (needs `KeyEventState::KEYPAD` detection and a `PanelButton` per slot), while the `Action::Jog`/`Slot` routing behind it exists and is tested. This is a decision to be made, not a thing to try. |

## Open questions

| # | Question | Status |
|---|---|---|
| **OQ-T29** | Quantization control for live record: record-as-played vs. snap-to-step; destructive at record time or non-destructive over already-persisted micro offsets; where it lives in the panel grammar. **Not a new question — this is the TK-local instance of the roadmap's existing OQ-12 ("live-record quantisation model", P11-scoped), which this session supplies concrete demand and evidence for.** Recorded under both names; do not design them separately | **OPEN — from F6, = OQ-12** |
| **OQ-T27** | P-lock authoring gesture | **REOPENED** by F7/F11 — was recorded as resolved by D15/C5b. Latched works; momentary is recommended for retirement; the bare-trig ownership question in ENC mode is undecided |
| **OQ-T30** | **Multi-surface transport/state agreement.** User, musing: "konsole works mostly, we should consider what happens with multiple interfaces ie terminals and web etc. There needs to be listeners to engine bi directional." Lands on the same seam BUG-043 exposes — transport state is authored by whichever surface pressed a key, and the engine's vocabulary is too thin for surfaces to stay agreed. Spans antiphon's existing bidirectional bridge (W0/W1), **W4** multi-client polish, **OQ-T28**/ADR-045 cross-surface capture, and **TKW** route (b), which would make the panel itself one of several concurrent clients. Tracked in the roadmap's global series as **OQ-16**. Any BUG-043 fix should be designed against this, not just against the terminal | **OPEN — new** |
| OQ-T24 | Numpad slot cluster fate | Open — unchanged, not testable as built (see above) |
| OQ-T23b | Tap tempo behind a screen | Open — not reached |
| OQ-T4 | `design.md` §4.2 step-size scaler | Open — unchanged |
| OQ-T21 | KEYBD chromatic grammar | Open — TK3 |
| OQ-T12 | WT convergence | Open — three sessions now held; interacts with TKW route (b), which is a near-identical animal |
| OQ-T28 | Cross-surface lock capture | Deferred — ADR-045, not TK2.1 scope |

## Roadmap deltas

- **TK2.1 does not close on this session.** The redesign's core is signed off,
  but four bugs and two structural collisions were found inside it. Sequence
  from here: **fix pass** (BUG-043/044/045/046 + F2/F3/F8 layout revision) →
  **decision on OQ-T27's bare-trig ownership** → **session #4** to judge the fix
  pass and run the five unjudged items above.
- **ADR-044 status:** D1/D2, D5, D6, D7, D8 confirmed by play. D3/D4 need a
  second pass (F2/F3/F8). D9 converged in direction, unusable as built (F7).
  **D15's momentary half is recommended for retirement** (F11); its latched half
  stands.
- **`design.md` §5.1/§5.2 stay DETERMINED** — session #2's reopening is
  discharged, on a terminal-native reading of the goal rather than a
  hardware-mimicry one.
- **TKW** (added this session, before the run) is reinforced by it: the session
  could not have tested half its hypotheses without installing a different
  terminal. H1's verdict argues for the **WASM/browser** route over a native
  rasterised window, since the text aesthetic is what the user actually values.
- New open questions **OQ-T29** and **OQ-T30**; **OQ-T27 reopened**.
- Process principle recorded by the user, and it governs how the above should be
  read: *"I don't care about spec as canon. I purposefully ratified, knowing
  everything was up for revision once we built. This is not waterfall, it's
  agile, we just have a bit more frontload of design/impl to test because we're
  trying to efficiently use agentic resources... now that I see it I want to
  continue the design."* A ratified ADR is a **frontloaded hypothesis, not a
  contract**; divergence between spec and build is not a defect class and must
  not be recorded as one. Findings get current behaviour + `file:line` + what to
  build next — citations exist to make the next change cheap, not to assign fault.

## Method note (for whoever runs session #4)

Running the app in kitty with `--listen-on unix:@tk3` and
`-o allow_remote_control=yes` let the agent read the panel (`kitty @ get-text`)
and drive single keys (`send-text`) independently. That carried the protocol
check, D7's silent-launch verification, BUG-043's timing measurement, and F3's
reflow measurement **without spending user time**. Two hard limits, both hit:

1. **`send-text` cannot produce modifier chords or holds.** An uppercase `'A'`
   arrives under the kitty protocol as a bare `a` with no SHIFT flag — it hit
   `ToggleStep` on step 9 instead of jogging encoder 1. Every chord/hold
   hypothesis genuinely needs the user's hands.
2. **Agent and user keystrokes interleave** on the same window. An early false
   alarm (agent read a running transport and a cycling track name as a D7
   violation) was just the user playing. Announce hands-off before measuring.

Two further phantoms were caught before reaching this document, both agent
measurement error rather than defects: a "blank param page" (the param cells are
on screen line 5; the `sed` window was reading blank lines 3–4) and "sub-page
cycling is inert" (`page_sub_page_count()` correctly returns 1 for a 4-param
page, and `[Source]   Amp` is the *page* bar with brackets marking selection, not
a sub-page tab list). Verify layout offsets against a full numbered capture
before reporting a rendering bug.
