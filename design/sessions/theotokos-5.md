# Theotokos — Usability Session #5

**Date:** 2026-08-01
**Phase:** MM §6.7 (exit criterion) — the machine-select and MOD-page gate for
ADR-041 + ADR-042
**Session type:** hands-on, default 4-track instrument (`paraclete-default`,
140 BPM, Kick/Snare/HiHat/Bass), `./target/release/paraclete`, run **in kitty**
with the agent observing and driving keys over `kitty @ --listen-on unix:@tk5`

> **Session status: held and concluded 2026-08-01.** Both halves of §6.7 were
> exercised: machine select (round 1) and LFO depth (round 2, appended below).
> **Round 2 supersedes the `NOT TESTED` / `NO VERDICT` rows for M6 and M7 in
> the round-1 hypothesis table** — M6 failed on presentation, M7 was accepted
> by the user. **M4 (§6.2, params surviving machine round-trips) was never
> tested** and is the one exit criterion this session did not reach.
>
> This banner is the only part of the file revised after the fact, the same
> exception AGENTS.md makes for an ADR's `Status:` line; everything below is
> as written at the time.

## Verdict so far: machine select is good. The round after it found the biggest defect of the phase — a p-lock only survives one audio block, so every lock on decay/open/tone has been inaudible on all three engines, and the one baseline that covers p-locks proves nothing.

Machine switching works and sounds right — the user, on a live sweep of node
20 through all three analog machines while a pattern ran: *"sounded great"*.
No artefact on the switch, no transport disruption, and #161's header fix
tracked the active machine correctly.

What the session found instead sits underneath the surface, exactly as
session #4's did. Asked to author an open-hat p-lock, the user reported the
locked step *"sound identical"* — and it is. `node_locks` is cleared at the
top of every `process()` cycle, so a lock lives ~11 ms, not a note. Params
latched into voice state at trigger time (`tune`, velocity) lock correctly;
params re-read every render span (`decay`, `open`, `tone`, the whole `lfo_*`
block) revert to the bank almost immediately. **#169.**

It survived MM because the only p-lock coverage in the ADR-035 baselines
authors a lock that cannot do anything — wrong param id *and* a step with no
trig. **#170.**

## Hypotheses

| # | Hypothesis | Verdict |
|---|---|---|
| M1 | All six machines runtime-selectable from Theotokos with no audible artefact (§6.1) | **PASS.** User, on a live sweep of node 20 through machines 0/1/2 with the pattern running: "sounded great". Agent-verified: SRC params moved `punch,drive` -> `snap,noise` -> `tone,open`; playback never faltered |
| M2 | The active machine's name reaches the performer (#161's panel half) | **PASS.** Header tracked `Kick — AnalogKick` -> `Kick — AnalogSnare` -> `Kick — AnalogHiHat` across the sweep |
| M3 | `machine` is refused as a p-lock target (ADR-041 §0 A4) | **PASS on the refusal, FAIL on the feedback.** Jogging encoder 1 with a lock target set left `machine` at 2.00 and wrote no lock. But the status line still advertised `J:machine→s12`, and the refusal was silent — **#171** |
| M4 | A track's params survive machine round-trips losslessly (§6.2) | **NOT TESTED.** Deferred with the LFO round |
| M5 | A p-lock authored from the panel moves audio | **FAIL — #169.** The authoring path is correct end to end (surface, sequencer, delivery); the engines drop the lock one cycle later |
| M6 | LFO depth is performable as a gesture (§6.3, ADR-042) | **NOT TESTED.** The session paused before this round |
| M7 | The page-index shift is acceptable (TRIG now at index 0, ADR-041 impl note) | **NO VERDICT.** Measured and presented; the user went to the hi-hat question and the session did not return to it |

## The p-lock defect (#169)

The user authored an `open` lock on step 13 of a HiHat-switched track and
heard no difference. Root-caused away from the panel, because the panel
cannot answer it — see #163, which is exactly about `RenderData` being
unverifiable.

`AnalogEngine::process()` clears the lock set every cycle
(`analog_engine.rs:864`), with the comment "cleared each cycle so locks from
one step do not bleed into steps that carry no lock". Step scoping is right;
a cycle is ~512 samples, not a note.

Measured with three `test-driver` renders (sequencer 12 -> node 22, notes on
steps 0 and 8, deterministic `--update-baseline` path). 50 ms RMS-envelope
windows from the step-8 onset:

| render | onset | +50ms | +100ms | +150ms | +200ms |
|---|---|---|---|---|---|
| control (no lock) | 0.3376 | 0.0203 | 0.0132 | 0.0078 | 0.0052 |
| **`open`=1.0 p-locked on step 8** | 0.3630 | 0.0219 | 0.0142 | 0.0084 | 0.0056 |
| `open`=1.0 set live | 0.7212 | 0.4804 | 0.2896 | 0.1686 | 0.0983 |

The locked hit is the control's decay *shape* at ~7% higher amplitude — the
~11 ms of slow decay before the lock is dropped. On the unlocked step the
locked render is byte-identical to control, so step scoping itself is
correct. Same `node_locks.clear()`-per-cycle pattern in `fm_engine.rs:781`
and `sampler.rs:658`.

## What the user asked that the design had not answered

**Open and closed hats** (*"Double up as two tracks and then have long and
short decay?"*). Half right: `open` is one continuous knob on one machine,
`effective_decay = decay * (1 + open * 7)`. The two routes differ in choke —
one track with a p-locked `open` chokes for free, because one node is one
voice and `retrigger()` resets `amp_env`; two tracks never cut each other,
and no choke concept exists in the workspace at all. **#172.**

**Toms and claps.** In `instrument-vision.md` as machine breadth, not new node
types — and now buildable, since MM shipped the selector. But P13 is the
analog *synth* voice and P14 is FM; no phase owns drum-machine breadth.
**#173.**

Both questions came from playing the instrument for a few minutes, and
neither was in the spec's field of view. That is the argument for the session
gate, independent of what it found.

## Filed

| Issue | |
|---|---|
| #169 | BUG-063 — a p-lock only survives one audio block (all three engines) |
| #170 | INFRA-022 — `plock_authoring.yaml` authors an inert lock; its baseline proves nothing |
| #171 | BUG-064 — machine-select refuses silently three ways (+ page keys 5/6 clamp, in comments) |
| #172 | OQ-M6 — no cross-track choke exists |
| #173 | Toms/claps: machine breadth buildable, unscheduled |
| #174 | BUG-065 — HiHat leaves SRC slot 0 empty; are slots comparable across machines? |

`d75a149` guards the p-lock jog's accumulation round trip — a gap found while
bisecting #169, not a fix for it.

## Method notes

**The user shares the keyboard.** The transport stopped twice with no key
sent by the agent, and three probes went into bisecting it before the user
explained: *"I stopped when we were between rounds, because I didn't want to
hear snare noise polution before i was ready."* In a paired kitty session the
two hands are indistinguishable from the agent's side. Ask before
investigating; do not file. Both stops were correctly left unfiled, but the
probing was wasted.

**Releasing a latched hold needs a raw release, not a re-press.** AGENTS.md
documents `send-key` (press+release) and `send-text` (press only). Neither
releases a latched prefix: `send-key m` after `send-text 'm'` reads as a
deliberate second press, and for Lock that *clears the target you just set* —
which looks exactly like a broken gesture. The kitty keyboard protocol
release escape works:

```bash
kitty @ --to unix:@tk5 send-text $'\x1b[109;1:3u'   # key 109 ('m'), event type 3 = release
```

With that, `L:s12` survived as designed. Folded into AGENTS.md.

**The panel could not answer the question it raised.** `B:open=0.000L` shows
the *live* param with a lock-exists flag — it never displays the locked
value, so "is the lock 0, or is the display lying?" is unanswerable from the
panel. `test-driver` renders settled it. #163 is the structural version of
this complaint.

---

## Round 2 — the LFO (§6.3) and the page verdict, same sitting

Appended after the session resumed. The section above is unchanged.

## Verdict: the LFO is correctly wired and almost unusable. Nothing is broken in the DSP; everything that made it unusable is in what the surface offers and how it is labelled. The page shift is accepted.

Four rounds went into trying to hear the LFO do something, and each failure
had a different cause. That pattern *is* the finding — the LFO works, and a
performer still cannot get a musical result from it without knowing the
source.

## Hypotheses (continued)

| # | Hypothesis | Verdict |
|---|---|---|
| M6 | LFO depth is performable as a gesture (§6.3, ADR-042) | **FAIL, on presentation not on function.** The DSP is correct and exhaustively verified. Getting an audible, intended result required knowing which dests the active machine reads (#179), that the labels are missing (#176), that depth is scaled by the union range (#178), and that `tune` is per-note only (#175). None of that is discoverable from the panel |
| M7 | The page-index shift is acceptable | **PASS.** User: *"it's fine, elektron puts some stuff there. Maybe we find something more for trg in future."* Follow-up filed as #180 |
| M8 | `lfo_fade`'s 4-second maximum suits a percussive voice | **DEFERRED, deliberately.** With #178 and #179 open, a fade judgement would confound three variables. Revisit after those land |

## The four failures, in order

Each was diagnosed and each had a distinct cause. Recorded in order because
the sequence is the argument for building the audit earlier.

1. **`tune`, `Free`, 1 Hz** — "high high, low low". Correct behaviour: a
   free-running LFO sampled once per note against a fixed trig grid, phase
   advancing 3/7 of a cycle per hit, so the pattern repeats every 7 hits.
2. **`tune`, `Trig`** — "four on four identical kicks". Also correct: `Trig`
   resets phase per note, so every hit samples the same value. Led to #175 —
   `tune` is read only in `retrigger()` (`analog_engine.rs:501`) and the
   render uses the frozen `current_hz`, so it can never sweep *within* a note.
3. **`tone`, depth 1.0 then 0.08** — "nope, all same... softer/more muted".
   `tone` is a lowpass cutoff at 4 kHz over a ~65 Hz sine: inaudible at
   moderate depth, and at full depth the union-range scaling (#178) clamps it
   to 200 Hz, which reads as "the kick got muted".
4. **`drive`, full depth, `Trig`** — "sounds different, but each hit is the
   same". Correct again: `Trig` by definition.

Then, on the user's request for a slow modulation across 16 steps: `tune`,
`Free`, 0.583 Hz (one cycle per bar, hand-computed because the LFO receives
no tempo), 16 trigs — *"a low thrum from a helicopter"*. Sixteen kicks a bar
is a drone whatever the LFO does; a fourth badly-chosen demo.

**The user's call at that point was the right one:** *"That should be
carefully audited through a one by one ab test performed autonomously before
you waste anymore user time."*

## The audit (`3f729a0`)

Two tests, both mutation-checked, replacing four rounds of guessing:

- `every_dest_index_modulates_exactly_the_param_it_names` — walks all eight
  one-based dest indices against every observable param (the eight dests, the
  seven LFO controls, `machine`). Exactly one must move, and it must be the
  one the table names. **Passes** — the wiring is correct. An off-by-one in
  `lfo_dest_id` is killed by five tests.
- `some_dest_indices_are_inert_on_each_machine` — pins the finding that
  explains the session.

A throwaway render-and-analyse harness (`lfo_audit.py`, scratchpad) measured
across-note vs within-note movement per destination and corroborated it: on a
Kick, dests 6/7/8 are bit-identical to a depth-0 control. Two artifacts in
that harness are **not** findings and are recorded so nobody re-reads them as
such: `drive` at full depth saturates enough to merge onset detection into a
single hit, and the within-note pitch column is confounded by the kick's own
pitch envelope spanning more Hz at a higher base pitch.

## What the audit found

`LFO_DESTS` is machine-invariant; `machine_params` is not. So per machine:

| machine | inert dests |
|---|---|
| Kick | 6 `snap`, 7 `noise`, 8 `open` |
| Snare | 4 `punch`, 5 `drive`, 8 `open` |
| HiHat | 1 `tune`, 4 `punch`, 5 `drive`, 6 `snap`, 7 `noise` — **5 of 8** |

Worse across a machine switch, which is MM's entire premise: the dest index
survives the switch while its meaning flips from live to inert, silently.
**#179.**

## Filed in round 2

| Issue | |
|---|---|
| #175 | BUG-066 — LFO on `tune` is a per-note sample-and-hold (severity corrected in a comment after measurement) |
| #176 | BUG-067 — stepped selectors show raw numbers; Theotokos never reads `ParamDescriptor.display`, and `machine` declares none |
| #177 | BUG-068 — `:set` clamps every param to 0..1 regardless of declared range |
| #178 | BUG-069 — LFO depth scales by the bank's union range, miscalibrated on every machine but the widest |
| #179 | BUG-070 — 3–5 of 8 dest indices are silently inert per machine |
| #180 | TRIG page has 7 empty slots; the sequencer already has 4 per-step params with no encoder home |

## Method notes (round 2)

**Build the oracle before spending the ears.** Four user-facing rounds
produced one usable fact between them; two unit tests produced the answer in
minutes and left permanent guards. The tell that it was time to stop was
present early — the second "no" on a destination I had chosen by table order
rather than by measurement.

**A wrong theory stated confidently is worse than no theory.** #175 was filed
with a table asserting the other seven destinations sweep correctly, derived
from a code read alone. The user's "nope, all same" refuted it. Corrected in
a comment rather than by a silent edit, so the record shows the error.

**`:set` was found by trying to use the instrument, not by testing it.** The
0..1 clamp (#177) surfaced only because the encoder ramp overshot and the
command line was the obvious alternative. Its three existing tests all use
`"set dec 0.8"` — the one param range that cannot expose the bug.
