# Paraclete — TK2.2 Theotokos Specification (Session #3 Fix Pass)

> **EXECUTION-READY for C0–C5 — 2026-07-29.** C5's gate is **lifted**:
> ADR-046 was **accepted the same day** with R1–R4 settled as recommended
> (three-command split; `global_start`→`global_rewind`; rewind valid while
> running; clock state published here). Written so a fresh-context session
> can implement without further design decisions:
> every commit names its files, the current behaviour it changes, the sites
> it must update to stay green, and its tests. Where a value is a tuning
> knob the default is stated *(tunable)*.
>
> **Design authority:** `design/sessions/theotokos-3.md` (the evidence —
> read it first), ADR-044 (accepted; amended by this phase, see §0),
> ADR-046 (✅ accepted 2026-07-29 — C5), ADR-045 (🟡 proposed, premise
> confirmed but deliberately still parked — see §6 OQ-T27).
> **Baseline:** TK2.1 C0–C7 code-complete (`7d3d6c2`), session #3 held
> (`e364893`). `cargo test --workspace` green at baseline.
> **Exit:** `cargo test --workspace` green after every commit; a real
> terminal pass in kitty per the session #3 method note; **usability
> session #4 at C6**.
>
> **Read this first — how to treat the specs it cites.** A ratified ADR in
> this project is a *frontloaded hypothesis, not a contract* (user, session
> #3). Where this phase changes an ADR-044 decision, that is the expected
> output of a usability session, not a defect being excused. Record current
> behaviour + `file:line` + what to build next; do not sort findings into
> "spec's fault" vs "code's fault".
>
> Third-party marks appear per house naming policy: design prose only,
> never identifiers or UI strings.

---

## §0. What session #3 settled about ADR-044

**Confirmed by play, do not revisit:** D1/D2 (fixed panel, one-track
strip), D5 (REC toggles `Off↔Grid`; REC-hold+PLAY escalates to `Live`),
D6 (trig N = track N in pad modes), D7 (no transport at launch — *intent*
confirmed; its mechanism changes under ADR-046 T3), D8 (engine-side live
record, tight at 140 BPM), D10/D12/D14 (untested but unchallenged).

**Confirmed with a redirection that outranks the original wording:** the
panel converged, but on **terminal-native** grounds — "digital, hardcore,
weird unix and hacker culture" — *not* on D1's literal goal of reading as
a hardware front panel. **Consequence, applies to every layout choice in
this phase: do not add skeuomorphism.** Prefer the plainest text form that
carries the information; when in doubt, look more like a terminal, not
more like a box.

**Amended by this phase:**

- **D3/D4 (chips + legend)** — the mechanism works; the *placement model*
  is replaced. See E1/E2/E3.
- **D9 (ENC mode)** — direction confirmed ("much improved over chord"),
  implementation unusable in `Grid`. See E4.
- **D15 (p-lock authoring)** — the **momentary half is retired**; latched
  `[m]` becomes the only p-lock gesture. See E4 and §6 OQ-T27.

---

## §1. Scope

**In:** the four bugs session #3 filed (BUG-043/044/045/046), the legend
and chip placement revision (E1–E3), bare-trig ownership between ENC and
p-lock (E4), and jog/lock feedback (E5).

**Out, unchanged:** the two-tier input pipeline (`key_to_button` →
`HeldState` → `button_to_action`), TRK/PTN hold grammar and TK2 §0 A10's
precedence, the `:` command line and `Keymap` persistence, Tempo/Chain/
Settings screens, `RecMode` and the pad model, engine-side live record,
design.md §4.2's jog constants.

**Out, deferred with a citation:**

- **Quantization control** (OQ-T29 = roadmap OQ-12) — a real user
  requirement from session #3, but P11-scoped and larger than a fix pass.
  BUG-045's fix must not preempt it: see C2's note.
- **Cross-surface lock capture** (ADR-045, OQ-T28) — session #3 *confirmed
  its premise empirically*, which strengthens the case for unparking it,
  but it is not this phase.
- **FUNC+transport ergonomics** (H10) — deferred by R5, by session #2, and
  again by session #3 (not reached). Still not silently dropped.
- **The 16-track width question** (F1) — untestable until an instrument
  with 16 named tracks exists.

---

## §2. Decisions this phase adds

- **E1 — Residual legend.** The legend strip carries **only affordances
  with no on-screen referent**. A key whose effect is visible as a specific
  panel element gets its chip **inline on that element** instead.
- **E2 — No column reflow.** A chip that is hidden still reserves its
  width. Region *columns* are as fixed as region heights (extending D1,
  which only ever promised heights).
- **E3 — Hidden, not dimmed.** Where a key has no action in the current
  mode, its chip is **absent** (blanked), not rendered in gray.
- **E4 — The bare trig has exactly one owner per mode.** No press ever
  means two things at once.
- **E5 — A jog says what it is writing.** The panel shows which parameter
  a jog is moving, and whether it is writing a **lock** or the **live
  value**.

### E1 in full (the assignment a fresh session should implement)

| Chip | Where it goes | Why |
|---|---|---|
| `[z]` REC | inline on the transport line's rec glyph → `[z]REC○` | the glyph is the referent |
| `[x]` PLAY | inline on the transport line's play/stop glyph | same |
| `[c]` STOP | inline on the transport line — **only once ADR-046 lands (C5)**; until then the chip must be absent everywhere | do not advertise a key that does nothing (BUG-043) |
| `[0]` TEMPO | inline on the BPM readout → `[0]140.0 BPM` | the readout is the referent |
| `[p]` PTN | inline on the track line's pattern field → `[p]PTN P1` | the field is the referent |
| `[Tab]` TRK | inline at the track line's head | the track line *is* the track selector |
| `[n]` ENC, `[m]` LOCK | **legend strip, on every screen including `Grid`** | no on-screen referent — and this is exactly F8's bug: `[m]` is a `Grid`-only gesture that today is advertised only *off* `Grid` |
| `[1-6]` PAGE, `[-/=]` WIN, `[o]` SONG, `[8]` SET, `[Enter]` YES, `[Esc]` NO, `[:]` CMD, `[?]` HELP, `[^C]` QUIT | legend strip | open or act on things not currently shown |

The legend keeps `pack_two_lines`' declared-priority truncation (D4) —
only the *membership* changes, not the packing rule.

### E4 in full

| Mode | Bare trig means | P-lock target set by |
|---|---|---|
| `Off` / `Live` (pad modes) | play + select that track | *(p-lock is a `Grid` concept; unchanged)* |
| `Grid`, ENC **off** | write / clear that step | `[m]`-hold + trig (latched) |
| `Grid`, ENC **on** | jog that encoder | `[m]`-hold + trig (latched) |

Momentary p-lock (holding a bare trig to arm a target) is **removed
entirely** — not gated, removed. Rationale, from session #3's F11 and
predicted by ADR-045: to p-lock encoder *N* on step *N* the hand must hold
trig *N* and jog encoder *N*, which is the same physical key. The gesture
has a **hole in its domain** that no timing or gating fix reaches, and
latched has no such hole (`m`+`q`, release, then `Shift`+`q` lands a lock
on step 1 — the diagonal works). Momentary was borrowed from hardware for
authenticity, but hardware has two hands on two distinct control surfaces;
the keyboard collapses steps and encoders onto one set of keys.

---

## §3. Commit sequence

Every commit: `cargo test --workspace` green, `cargo clippy` clean on
touched crates, no new deps, no audio-thread allocation. Per the house
process (`AGENTS.md`), each staged commit gets a fresh-context hostile
review before it lands, and code and doc changes go in **separate**
commits.

### C0 — BUG-044: a live pad trig must use the track's `default_note`

**Change:** `crates/paraclete-nodes/src/sequencer.rs:809` — `CMD_TRIG_NOW`
resolves `arg0 <= 0` to a hardcoded `60`; it must resolve to
`self.default_note`.

**Why it matters:** the default instrument configures `default_note: 36`
on all four sequencers, so every live pad hit sounds **two octaves** above
the same track's sequenced steps. This is the primary performance gesture.

**The test is the real work here.**
`trig_now_uses_default_note_and_velocity_when_zero` (`:4870`) builds a bare
`Sequencer::new()`, whose `default_note` *is* 60 (`:490`), so the hardcoded
constant passes and the suite is structurally blind. Its assertion message
also claims it checks "`Step::empty()`'s defaults", which route through
`self.default_note` (`:412`) and would be 36 on a configured track — the
message and the assertion disagree once the two differ.

**Tests:** rewrite that test to build a sequencer with
`with_default_note(36)` and assert the live trig's note **equals the
track's sequenced-step note**, not a literal. Add a second case at a third
value (e.g. 48) so a future hardcode cannot pass.

### C1 — BUG-046 + E4: one owner for the bare trig

**Changes:**
1. `crates/paraclete-theotokos/src/lib.rs:681` — `KeyEventKind::Repeat` is
   consumed for `PanelButton::Rec` **only**, so OS auto-repeat re-fires
   `ToggleStep` once per pulse and holding a trig rapid-toggles the step.
   Consume `Repeat` for trig buttons too (a step toggle is
   once-per-physical-press).
2. `crates/paraclete-theotokos/src/lib.rs:616-650` — delete the momentary
   lock-arm block (E4). Remove `TheotokosApp::momentary_lock` (`:76-81`,
   `:159`) and the release-side clearing it drives.
3. Confirm the latched path is untouched: `[m]`-hold + trig, cleared by the
   same trig, `m` again, or `Esc`.

**Ordering note:** these two land together deliberately. Fixing repeat
without removing momentary would make the *broken* gesture more reachable;
removing momentary without fixing repeat leaves holding a trig destroying
pattern data.

**Update to stay green:** `grep -c momentary
crates/paraclete-theotokos/src/lib.rs` returns **21** at baseline, so this
is wider than the one block — field, initialiser, press/release arms,
doc-comments, and tests. Delete the tests asserting momentary arm/release
rather than adapting them; they assert a retired gesture, and that includes
the C5b hostile-review regressions about *press-time capture* (they exist
only to protect momentary's release path). Grep to zero, and check
`Model::lock_target`'s doc comment (`model.rs:108`) still describes reality
once momentary is gone.

**Tests:** a held trig in `Grid` (Press then N×Repeat then Release) toggles
the step exactly **once**; a bare trig in `Grid`+ENC jogs and leaves
`lock_target` `None`; `[m]`+trig still latches; `Esc` still clears.

### C2 — BUG-045: a hand-written step is on the grid

**Change:** `crates/paraclete-nodes/src/sequencer.rs` — all three manual
write paths ignore `timing.micro_offset`, so a live-recorded offset
survives erase-and-rewrite: `CMD_TOGGLE_STEP` (`:601`) flips `active`
only; `CMD_SET_STEP` (`:608`) writes `note` + `active` only; `CMD_CLEAR`
(`:620`) sets `active = false` across the lane and leaves every offset in
place. Activating a step by hand must zero `micro_offset`.

**Decision this commit makes (and must record in its message):** does
`CMD_CLEAR` also reset micro-timing? TK2 §0 A8 establishes that
`CMD_CLEAR` deliberately *preserves* per-step data (locks survive a
clear). **Ruling for this phase: yes, `CMD_CLEAR` resets micro-timing** —
micro-timing is the step's own placement, not an attached lock, and "clear
the lane" that leaves the grid crooked cannot be explained to a user.
Locks still survive, unchanged.

**Do not preempt OQ-T29.** The user's rule ("step rec should be fully
quantized") is implemented here as an invariant. If quantization control
lands later it may become a *mode*; leave the reset in one named helper so
that change is local, and say so in a comment.

**Tests:** live-record a step with a non-zero offset, toggle it off, toggle
it on → offset is 0; `CMD_SET_STEP` on a previously-offset step → 0;
`CMD_CLEAR` → all offsets 0 **and** locks still present (pin A8 explicitly,
so the ruling above is visible to the next reader).

### C3 — E1/E2/E3: the legend and chip placement revision

The largest commit, and all render-layer.

**Changes in `crates/paraclete-theotokos/src/render.rs`:**
- `legend_chips_for_screen` (`:272`) — rebuild membership per E1. `Grid`
  gains `Enc` + `Lock`; `Rec`/`Play`/`Stop`/`Tempo`/`Ptn`/`Trk` leave every
  list. Keep the `enc == true` override's *shape* but apply the same rule.
- `render_transport` (`:433`) — inline `[z]`/`[x]` (+`[0]` on the BPM
  readout). `[c]` only at C5.
- `render_track_indicator` (`:469`) — inline `[Tab]` at the head and `[p]`
  on the pattern field; and **E2/E3**: at `:487-495` the chip becomes
  `String::new()` in `Grid`, shifting every track name 3 columns left (12
  cumulative at 4 tracks) and dragging `PTN P1` with it. Emit
  width-matched blanks instead.
- **E2 caution:** the reserved width must equal what the chip *would* have
  been. `chip_key_display` (`:20`) can yield multi-char labels (e.g.
  `Tab`) under ADR-037 remapping, so a hardcoded 3 spaces is wrong.
  Compute the width from the same label the pad mode would render.
- `render_trig_row` (`:599`) and `render_encoder_cell` (`:749`) — audit for
  the same reflow class; the §3 cell-format table in
  `tk2.1-theotokos.md:93-99` fixes trig-cell width at 5, so those are
  likely already stable. **Verify, don't assume.**

**Update to stay green:** the render tests asserting current legend
membership and track-line text — search `legend` and
`chips_move_to_track_line_in_pad_mode` (`:1685`) in the `render.rs` test
module. These encode the *old* placement model and must be rewritten, not
patched to pass.

**Tests:** a track-line snapshot in `Off` and in `Grid` occupies
**identical columns** (this is E2's regression test and the one that would
have caught F3); `Grid`'s legend contains `[n]` and `[m]`; no legend
contains `[z]`/`[x]`/`[p]`/`[Tab]`/`[0]`; a remapped multi-char track key
still does not reflow.

### C4 — E5: a jog says what it is writing

**Change:** session #3's F7 left the user unable to tell what a jog was
doing — "not obvious what's being modified either". Routing is at
`lib.rs:1106`: with a lock target set, a jog emits `CMD_SET_LOCK_TARGET` +
`CMD_SET_STEP_LOCK`; otherwise `CMD_BUMP_PARAM`. Both are silent on the
panel.

Show, in the contextual window and/or status line: **which** parameter the
last jog moved, and **whether** it wrote a lock (naming the step) or the
live value. The armed lock target already renders on the trig strip; what
is missing is the *destination of the value*.

Keep it terminal-plain per §0's redirection — text, not a graphical
affordance.

**Tests:** with a target armed, the panel names the locked step; with none,
it shows the live parameter; the indicator clears with the target.

### C5 — BUG-043: pause and stop *(ADR-046 accepted; gate lifted)*

Implements ADR-046 T1–T5, with R1–R4 settled: add `CMD_CLOCK_REWIND`;
`CMD_CLOCK_START` stops implying a rewind; rename
`TransportFlags::global_start` → `global_rewind` (R2); rewind is valid
while running (R3); `InternalClock` boots `playing: false` (closing BUG-039
and retiring ADR-044 D7's startup `CMD_CLOCK_STOP` at
`theotokos/lib.rs:98` — remove it, do not leave two mechanisms for one
invariant); publish clock-level `playing`/position (R4/T4);
`Action::Stop` (new variant) = `STOP` + `REWIND` on `c`; `[c]` STOP rejoins
the transport line's inline chips (E1).

**Read this before renaming anything — R2's hazard.**
`sequencer.rs:925`'s `global_start` branch does **four** things, and only
one belongs to a rewind: sets `self.playing = true`; resets position
(`current_step = wstart`, `step_tick = 0`); calls `reset_period()`; and
**fires the entry step** (the BUG-001 fix, emitting that step's note-on). A
mechanical rename would make `CMD_CLOCK_REWIND` start playback and emit a
note — and since R3 permits rewind while running, a mid-play rewind would
double-fire against the ordinary boundary path. Decompose:

- `playing` derives from `flags.playing`, not from the rewind flag
  (`global_stop` already clears it at `:908-909`; keep the two sides
  symmetric).
- Position reset + `reset_period()` happen on `global_rewind`.
- The entry-step fire happens on the **transition into playing**, not on
  rewind.

**Update to stay green — read each before changing it.** These tests are
statements about semantics, so "make it green" is the wrong instinct:
`sequencer.rs:2210-2245`, `theotokos/lib.rs:1878`, `:2171`.

**Tests:** pause mid-pattern then resume → position continues (not step 0),
and micro-timing/page-window survive; `c` → halted **and** at the window
start; a fresh `InternalClock` is stopped with no surface command; **rewind
while stopped moves position and emits nothing**; **rewind while running
relocates without double-firing**; a normal start still fires its entry step
**exactly once** (the BUG-001 regression must survive this commit — it is
the reason the entry-step fire lives in that branch at all).

### C6 — Usability session #4 (user-paired, no code)

Produces `design/sessions/theotokos-4.md` + `tk2.2-report.md`. Owes a
verdict on this phase's fixes **plus** the five items session #3 did not
reach — see §5.

---

## §4. Method for the paired session

Use session #3's setup (recorded in `theotokos-3.md`'s method note): run
the app in **kitty** with `--listen-on unix:@tk3 -o
allow_remote_control=yes`, so the agent can read the panel
(`kitty @ get-text`) and drive single keys (`send-text`) without spending
user time. Hard constraints learned there:

- **Never run it through tmux** — tmux's `extended-keys` is
  CSI-u/modifyOtherKeys and does not proxy key *releases*, which silently
  forces the sticky fallback and invalidates every hold/chord hypothesis.
- **`send-text` cannot produce modifier chords or holds** (uppercase `'A'`
  arrives as a bare `a`), so chord/hold items genuinely need the user's
  hands. Do not claim to have tested them.
- Agent and user keystrokes **interleave**; announce hands-off before
  measuring.
- **Verify layout offsets against a full numbered capture** before
  reporting a render bug — two phantom findings in session #3 came from
  reading the wrong screen lines.
- Ask **one question at a time**, set the panel up for the user, and give
  exact key sequences.

---

## §5. Hypotheses for session #4

| Hypothesis | Source |
|---|---|
| The residual legend + inline chips make the panel readable without `?`, and nothing reflows on a mode change | E1/E2/E3 (F2/F3/F8) |
| With one owner per bare trig, ENC mode is usable in `Grid`, and latched `[m]` is a sufficient p-lock gesture on its own | E4 (F7/F11) |
| A jog's destination (live vs. locked step) is obvious without counting steps | E5 |
| Pause resumes where it paused; `c` stops and rewinds; both read as the reference box | ADR-046 / BUG-043 |
| A live pad and its sequenced step sound like the same instrument | BUG-044 |
| **Carried, unjudged in session #3:** descriptor-accurate jog proportionality (D10) — note its **stepped** half is untestable until some engine declares `stepped: true`; today none do, so this needs an instrument that exposes one | H8 |
| **Carried:** sticky re-tap + the 400 ms repeat guard (D11) — judge *after* C1, against intended behaviour | H9 |
| **Carried:** TRK/PTN physical feel; encoder-bank simultaneity | H11 |
| **Carried:** FUNC+transport copy/clear/paste ergonomics | H10, R5 |

---

## §6. Open questions

| # | Question | Status |
|---|---|---|
| OQ-T27 | P-lock authoring gesture | **REOPENED by session #3.** This phase's E4 answers the *near-term* half (latched only). The **real** answer may be **ADR-045** (cross-surface capture: hold the step here, dial the value on Theoria) — session #3 empirically confirmed ADR-045's own stated premise that "in ENC mode the trig rows *are* the encoders, so the holding hand has nowhere to be". **Recommend unparking ADR-045 on that evidence**, separately from this phase |
| OQ-T29 / OQ-12 | Quantization control for live record | Open. Concrete demand from session #3; P11-scoped. C2 must not preempt it |
| OQ-T30 / OQ-16 | Multi-surface bidirectional state agreement | Open. ADR-046 T4 publishes the state that makes it tractable; reconciliation policy stays here |
| OQ-T24 | Numpad slot cluster fate | Open — **not testable as built** (input side unwired; the `Action::Jog`/`Slot` routing behind it exists and is tested). A decision, not an experiment |
| OQ-T23b | Tap tempo behind a screen — global chord? | Open — not reached by session #3 |
| OQ-T4 | design.md §4.2's step-size scaler | Open — unchanged |
| OQ-T28 | Cross-surface lock capture | ADR-045 (🟡) — see OQ-T27 above |
| OQ-T21 | KEYBD chromatic grammar | Open — TK3 |
| OQ-T12 | WT convergence | Open — interacts with **TKW** route (b), a near-identical animal |
| F1 | Does the panel survive 16 named tracks? | Open — untestable until such an instrument exists |
