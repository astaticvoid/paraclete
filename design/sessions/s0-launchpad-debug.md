# Session 0b — Launchpad Live-Rig Verification Runbook

> **Executor:** any model tier (written to be Haiku-executable). Follow the
> steps literally; capture outputs; append findings to the bottom; do NOT
> redesign anything. If a step's outcome doesn't match any listed branch,
> record exactly what happened and stop that check.
>
> **Context (July 2026, written by the session that found the bugs):** the
> Launchpad appeared dead because three silent-error swallows hid a profile
> crash (empty `TRACK_SAMP_IDS` on the 4-track default instrument). Fixed:
> handler/on_load errors now print as `[rhai] ... error`, the profile is
> guarded for <8 tracks, `TRACK_GEN_IDS` is injected. LED bytes were confirmed
> flowing (`[lpx-deliver]`, `[lpx] note=..`) but **no human has yet confirmed
> the pads actually light** — that's what this runbook settles, plus the
> subshell-vs-user-terminal question.

## Check A — does the self-driving rig light up? (5 min)

1. Build once: `cargo build`. Then run **in the user's own terminal** (ask the
   user to type it themselves — see Check B for why):
   `cargo run -- --no-tui`
2. Ask the user to hold SHIFT (second side-button from top, right edge) and
   press the top-left pad, then press top-row pads 1 and 5 and second-row
   pads 1 and 5. (Enters sequence mode on track 1 and programs a 4/4 kick.)
3. Prompt the user, verbatim: *"Three yes/no answers please: (1) do you hear a
   kick? (2) are the programmed step pads lit blue? (3) is a green light
   sweeping across the row?"*
4. Branches:
   - **Yes/yes/yes** → rig fully working. Record and go to Check B.
   - **Sound yes, LEDs no** → LED bytes reach the OS but not the device or the
     device ignores them. Capture: does stderr show `[lpx] note=.. vel=..`
     lines while the sweep should be moving? If yes, the bytes are sent —
     suspect Programmer-mode state: ask the user to power-cycle the Launchpad
     (unplug/replug USB) with the app running, wait 5 s, re-ask question 2–3.
     Record both results.
   - **LEDs yes, sound no** → audio-device problem, not a surface problem.
     Capture `grep 'audio' stderr` output; ask the user whether the Mac's
     output device changed (headphones vs speakers). Record.
   - **No/no/no with `[rhai] ... error` lines in stderr** → new profile crash;
     copy the exact error line into findings; stop.

## Check B — subshell vs user-terminal (10 min)

Question raised by the user: does the rig behave differently when the app is
launched by the agent (background subshell, no TTY) vs by the user in their
own terminal? Differences could come from: TTY-dependent code paths, macOS
per-app permissions (CoreMIDI generally needs none, but audio in/out can
prompt), or environment.

1. With the SAME build, run it both ways for ~60 s each, pressing the same 3
   pads each time:
   - (a) user types `cargo run -- --no-tui` in Terminal/iTerm.
   - (b) agent runs `cargo run -- --no-tui` as a background task.
2. For each run capture: the three yes/no answers from Check A step 3, plus
   `grep -c 'lpx-deliver' <stderr>` and `grep -c 'rhai\] pad' <stderr>`.
3. Branches:
   - **Identical behavior** → subshell launching is fine; record and close the
     question.
   - **Different** → record the exact difference + both greps; do not attempt
     a fix; flag for an Opus/Fable session.

## Check C — full-surface LED sanity via lpx-debug (5 min, optional)

Only if Check A had LED failures: quit the app, run `cargo run -p lpx-debug`
(dedicated hardware test; logs to `/tmp/lpx-debug.log`). Ask the user what
they see on the grid. Attach the log tail (last 30 lines) to findings.

---

## Findings

_(append, dated)_
