# Theotokos — Usability Session #4

**Date:** 2026-07-30
**Phase:** TK2.2 C6 (exit criterion) — judges the session #3 fix pass built in C0–C5
**Session type:** hands-on, default 4-track instrument (`paraclete-default`, 140 BPM,
Kick/Snare/HiHat/Bass), `./target/release/paraclete`, run **in kitty** with the agent
observing and driving keys over `kitty @ --listen-on unix:@tk4`

## Verdict: TK2.2's fixes all hold. The panel is done arguing with itself. Four defects were found beside the fixes, one of them a transport regression the user heard immediately; ADR-045 stays parked on the user's judgment.

Every hypothesis TK2.2 set out to prove passed. E1/E2/E3's legend and chip
revision reads without `?`; E4's one-owner bare trig makes latched `[m]`
sufficient on its own; E5's jog readout names what it writes; BUG-044's live
pad and its sequenced step are the same instrument. The panel-grammar
argument that dominated sessions #2 and #3 did not resurface at all.

What the session found instead sits *underneath* the panel: **`CMD_CLOCK_REWIND`
never resets the clock's own position** (#142), so `x → c → x` restarts at
the top but re-phases the loop to the old boundary — the user caught this by
ear within seconds of being asked. Three further defects came out of agent
measurement around it (#143, #144, #145), plus one in the test harness itself
(#146).

The user also gave a standing directive that outgrew the question that
prompted it: **every command should acknowledge itself, and a held key should
highlight** (#147). It is filed as an open question wanting an ADR, not folded
in silently.

## Hypotheses

| # | Hypothesis | Verdict |
|---|---|---|
| E1/E2/E3 | Residual legend + inline chips make the panel readable without `?`; nothing reflows on a mode change | **PASS.** User: "looks good". Agent-verified: track line and both trig rows hold identical screen rows across all 6 param pages, GRID, TEMPO, SETTINGS, CHAIN and both ENC states. The legend's 2-row region keeps its height; only its tail wraps between rows, which the user judged invisible in play |
| E4 | One owner per bare trig; ENC usable in `Grid`; latched `[m]` sufficient as a p-lock gesture | **PASS, and it settles OQ-T27's near-term half.** User: "latched m is enough, keep ADR-045 parked" |
| E5 | A jog's destination is obvious without counting steps | **PASS.** Agent-verified: jogging encoder 1 puts `J:tune→live` in the status line, encoder 2 `J:tone→live` |
| ADR-046 / BUG-043 | Pause resumes where it paused; `c` stops and rewinds; both read as the reference box | **PARTIAL — pause/stop correct, rewind is not.** Pause holds position (halted at step 11, resumed from 11) and `c` returns the playhead to 1. But `x → c → x` re-phases the loop to where the previous loop would have ended — **#142**, root-caused below |
| BUG-044 | A live pad and its sequenced step sound like the same instrument | **PASS.** User, on direct comparison: "yes it sounds the same, and it was fine before" |
| H8 | Descriptor-accurate jog proportionality (D10) | **PASS (continuous half).** Acceleration measured on successive taps: `tune` +0.375, +0.798, +2.745. User: good. The **stepped** half remains untestable — no engine declares `stepped: true` |
| H9 | Sticky re-tap + the 400 ms repeat guard (D11) | **UNTESTABLE in this setup — carried again.** See the method note |
| H10 / R5 | FUNC+transport copy/clear/paste ergonomics | **Mechanically PASS, ergonomically REJECTED.** `shift+z/x/c` all work; copy's total silence is what prompted #147 |
| H11 | TRK/PTN physical feel; encoder-bank simultaneity | **PASS.** User: "all the rest is good" |

## The transport regression (#142)

The user's report: `x` → `c` → `x` "starts from begining but loop repeats
where prev loop would have ended."

`InternalClock::handle_commands` (`internal_clock.rs:172`) implements
`CMD_CLOCK_REWIND` as `saw_rewind_command = true;` and nothing else.
`self.tick`, `self.bar`, `self.beat` and `self.tick_accumulator` are never
reset, and `emit_rewind_event` (`internal_clock.rs:219-240`) then ships those
un-rewound values downstream on the event carrying `global_rewind`.

The sequencer honours the rewind correctly (`sequencer.rs:940-945`), which is
why the playhead restarts at 1 and the restart *looks* right. The clock keeps
running on its old phase, and the sequencer's bar-sync snap
(`sequencer.rs:889-902`, gated on `!in_sync && new_tick == 0`) drags it back
to that phase at the next bar boundary.

ADR-046 T1 defines the command as "**set position to the window start**".
The clock relays the intent without honouring it for its own domain. The ADR
is sound; the implementation is one command arm short of it.

## Defects found beside the hypotheses

| Issue | Finding |
|---|---|
| **#142** | `CMD_CLOCK_REWIND` never resets the clock's own position (ADR-046 T1 conformance) |
| **#143** | The app boots with the transport running — `profiles/launchpad.rhai:29` sends `CMD_CLOCK_START` in `on_load`, and profiles load unconditionally regardless of `--no-emulator` or whether a Launchpad exists. `InternalClock` itself boots correctly per T3; `profiles: []` boots `■` frozen at Step 1 |
| **#144** | A latched hold-prefix is unrecoverable in the kitty path. `armed` is cleared only by the matching physical release; `HeldState::on_esc()` — which D6 specifies as "Esc disarms unconditionally" — has no production caller |
| **#145** | Antiphon's initial `TransportSummary` hard-codes `playing: true` with a comment whose premise ADR-046 T3 retired |
| **#146** | test-driver name resolution silently misroutes: short type-tag names overwrite display names, so `target: Kick` hits the AnalogEngine (20), not the sequencer (10). `audit_sequencer.yaml` has four dead `toggle_step`s today and still passes |
| **#147** | *(open question, user directive)* Every command should acknowledge itself; a held key should highlight. Prompted by copy's silence and by FUNC being undiscoverable outside `?` |

#143, #145 and the launchpad.rhai comment are all one blast radius: ADR-046
T3's migration inventory checked *callers of the clock commands* and missed
*claims about clock state* — a profile's `on_load`, a hard-coded hello
snapshot, and a comment asserting what another surface does.

## Method note (for whoever runs session #5)

**Session #3's "chords and holds genuinely need the user's hands" is only
half true.** `kitty @ send-key` delivers a press *and* a release, and
composing it with `send-text` (which delivers a press with no release)
synthesizes a hold-chord:

```bash
kitty @ --to unix:@tk4 send-text $'\t'   # Tab press, no release -> TRK armed
kitty @ --to unix:@tk4 send-key q        # trig press+release while armed
kitty @ --to unix:@tk4 send-key tab      # press+release -> clears the arm
```

That selected track 1 cleanly. `send-key shift+z` also carries FUNC intact,
so the whole copy/clear/paste family is agent-testable. This upgrade is what
let session #4 hand the user only judgment calls. `send-text` alone remains
unable to produce a release, which is a hazard rather than a limitation:
sending `z` that way latches REC as a held prefix, after which **every trig
becomes `Action::Noop` by design** (`input.rs:749`) — this looked exactly
like "step entry is broken" until the arm was found. Use `send-key` for
anything that should be a tap.

**H9 cannot be judged in kitty at all.** Kitty delivers releases, so the
sticky-fallback path (`on_press`, where the re-tap disarm and the 400 ms
guard live) never runs. Judging D11 needs a terminal that omits key releases
and is not tmux — the method note forbids tmux because its `extended-keys` is
CSI-u/modifyOtherKeys and silently drops releases, which is the same
condition being tested but with no way to tell a real result from an
artifact. Plain xterm or alacritty without the kitty protocol is the likely
vehicle. Until then H9 stays carried.

**Do not trust `parecord` off the default sink monitor as an audio oracle for
this app.** A 3 s capture read peak `0.0000` while the sequencer was playing
a pattern the user could hear, and `1.0000` for live pad taps in the same
instance — which led the agent to report a sequencer-silence regression that
does not exist. The user's ears refuted it: "it was fine before. Something is
missing from your framework. Let's not chase it." The finding was withdrawn
in full. Where a measurement and the user disagree about sound, the user
wins; use `test-driver` renders, which write a file the assertions read
directly, rather than sampling the system mixer.

**Verify against a full numbered capture, still.** Session #3's warning held
up: the `decay ▶ 0%` line was misread as a stale jog readout before
`render.rs:863` identified it as the live envelope meter — the actual jog
indicator (`render.rs:1051-1056`) renders only after a jog, exactly as
`jog_indicator_absent_before_any_jog` specifies.

## Carried to session #5

- **H9** — sticky re-tap + 400 ms guard (D11), needs a release-less terminal
  that is not tmux
- **H8 stepped half** — needs an instrument whose engine declares
  `stepped: true`; none does today
- **F1** — does the panel survive 16 named tracks? Untestable until such an
  instrument exists
- **#147** — wants an ADR and the user's sign-off before implementation
