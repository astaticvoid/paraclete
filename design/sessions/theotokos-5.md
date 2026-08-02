# Theotokos — Usability Session #5

**Date:** 2026-08-01
**Phase:** MM §6.7 (exit criterion) — the machine-select and MOD-page gate for
ADR-041 + ADR-042
**Session type:** hands-on, default 4-track instrument (`paraclete-default`,
140 BPM, Kick/Snare/HiHat/Bass), `./target/release/paraclete`, run **in kitty**
with the agent observing and driving keys over `kitty @ --listen-on unix:@tk5`

> **Session status: paused, not closed.** Machine select was exercised and
> passed. The p-lock round that followed found #169 and the session stopped
> there. **LFO depth — the other half of §6.7 — was never tested**, and the
> user has not given a verdict on the page-index shift. MM's milestone stays
> open. Continue by appending to this file.

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
