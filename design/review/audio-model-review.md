# Audio Model & Instrument DSP Review (July 2026)

> Requested design-review cycle: validity of the realtime audio model and each
> instrument engine against current best practice and available open-source
> integrations. Verdicts grounded in code reading (`audio.rs`, `executor.rs`,
> `ladder.rs`, `reverb.rs`, `analog_engine.rs`, `fm_engine.rs`, `sampler.rs`,
> `engine_dsp.rs`). Follow-ups filed as BUG-012/013 and backlog triggers.

## Verdict summary

The **architecture** (configurator/executor split, SPSC-everything, typed
ports, message-driven RT thread) matches how serious audio software is built —
no structural rework needed. The **device boundary** and **event-to-audio
timing** have two real correctness gaps (BUG-012/013). The **DSP** is
honest v1 quality: right references, known-dated algorithms in two places
(ladder, per-voice sinc), one latent CPU hazard (denormals on x86). Nothing
invalidates the model; the gaps are scheduled, not existential.

---

## 1. Realtime model

**Sound practice already in place:** allocation-free `process()` discipline
with tracked exceptions; lock-free SPSC for config/commands/state; events
routed same-cycle in topo order; graph built off-thread and swapped;
`try_lock` (never blocking) on the RT path; Rust's saturating `as` cast
protects the i16 output path from wraparound.

**Gap A — device format is assumed, not negotiated (BUG-012, escalates
BUG-002).** The app hardcodes 44100/512; the cpal stream opens at the
*device's* default rate and buffer size; `NodeExecutor::process()` renders
exactly `self.block_size` frames with only a `debug_assert_eq!` on the real
callback length (`executor.rs:578`). On a 48 kHz interface everything is
~8.8% sharp/fast; on any buffer size ≠ 512 release builds index out of bounds
or emit stale tail samples. Fix direction: take rate from
`default_output_config()` before building the graph (constructor plumbing
exists — `NodeConfigurator::new(sr, block)`), and chunk the callback buffer
into internal fixed blocks through a small ring (standard practice; also
decouples internal block size from the OS). Subsumes BUG-002's baseline fix.

**Gap B — voice starts are block-quantized (BUG-013).** Sequencer emits
events with sample offsets (verified accurate by the s0 timing harness), but
every engine ignores `sample_offset` and starts voices at block boundaries:
±11.6 ms jitter at 512/44.1k. P5's micro-timing (±5 ms at 120 BPM) and P10
C3's signed micro-timing are therefore **inaudible in the audio output** —
the feature exists only in event timestamps. Best practice (JUCE synth
voices, CLAP sample-accurate events): split the render block at event
offsets. Cheapest correct form: engines render `[0..offset)` with old state,
apply the event, render the rest. **Schedule with P10 C3** — micro-timing
ships audible or not at all.

**Minor (trigger-backlog):**
- No FTZ/DAZ on the audio thread. Freeverb's damped combs are a classic
  denormal CPU bomb on x86; Apple Silicon is mostly immune, so it's latent
  until an x86 build. One-time FPCR/MXCSR setup at stream start (JUCE
  `ScopedNoDenormals` equivalent).
- Executor cell is `Arc<Mutex<Option<NodeExecutor>>>`; contention yields a
  silent buffer. Correct today (only the swap path contends); replace with an
  atomic pointer swap (`arc-swap`) when apply_patch frequency grows (W-track
  live patching).
- No master headroom strategy: 8 tracks + distortion sum raw; f32 path has no
  clamp at all. A master soft-clip/limiter is P15 material; until then,
  document a −6 dB/track gain convention in the instrument YAML defaults.
- No xrun/CPU observability. A callback-duration EWMA pushed to
  `/engine/cpu` via the existing StateBus would cost ~nothing and give the
  TUI/Theoria a load meter (natural W1-era add).
- `DigitaktMidiNode` input uses `Arc<Mutex<VecDeque>>` (try_lock, drops on
  contention) while `LaunchpadNode` uses `rtrb` — harmonize on `rtrb` when
  the node is next touched (BUG-009 work).

## 2. AnalogEngine (kick/snare/hihat)

**Design: valid.** Phase-reset oscillator + retriggered envelopes is exactly
the 808-style model (phase reset is part of the sound); envelopes retrigger
from current value (`engine_dsp.rs:20`) so rapid retriggers don't click —
good. Monophonic-with-retrigger matches the drum-machine idiom.

**Against best practice:** the reference implementations to measure against
are Mutable Instruments *peaks* (`BassDrum`/`SnareDrum`/`HighHat`) and
*plaits* analog drum models (both MIT — already the sanctioned source). Theirs
add: resonant-filter-based body (self-tuned SVF ping rather than raw sine +
pitch env), hat metallic core from 6 detuned squares through a bandpass, and
transient click circuits. Current engines are simpler but idiomatic; the
upgrade path is engine-internal and non-breaking (params are the ADR-019
contract, not the DSP). **Recommendation:** treat peaks/plaits as the P12+
quality pass; no action now. A Rust port exists (`mi-plaits-dsp` crate, MIT)
— evaluate depending on it for P13/P14 voices rather than re-porting.

## 3. FmEngine (kick/bell/bass)

**Design: valid.** 2-op phase modulation with modulator self-feedback,
`sin()` per sample (cheap on modern CPUs; a LUT is a micro-opt, not a need),
1-sample feedback path. One known FM wart: raw 1-sample feedback gets noisy
("growl") as feedback → max; the DX7-lineage fix is averaging the last two
modulator outputs. Half-line change, audible at `feedback > ~0.7` on the
bell. **Recommendation:** apply the 2-sample average with P14 (melodic FM),
where feedback ranges get exercised; drum presets sit in the safe zone.

## 4. Sampler

**One wrong tool confirmed: per-voice `SincFixedOut` (rubato) for pitch.**
Rubato is a *stream* resampler: per-voice sinc state is heavy, its chunked
output shape fights voice rendering, and sinc group delay (~taps/2) both
delays voice onset by 1.5–3 ms and smears drum transients — the opposite of
what a drum sampler wants, and it compounds BUG-013's block quantization.
Best practice for one-shots: **load-time** high-quality resample to engine
rate (already done — `SincFixedIn` at load, correct use) + **playback-time
Hermite/cubic interpolation** for pitch (zero latency, transient-exact,
~30 lines, from-scratch per DSP policy). **Recommendation:** replace
`VoiceResampler` with Hermite playback interpolation; keep rubato for load
only. Schedule alongside BUG-013 (same files, same test rig). Serializer v3
`root_note` semantics are unaffected.

## 5. LadderFilterNode

**Dated core, fine for now, wrong for P13.** The cascade is the classic
naive one-pole Euler ladder with a single input `tanh` (`ladder.rs:107`) and
`fc = sin(π·f/sr)` tuning. Known deviations: cutoff detunes toward Nyquist,
resonance behavior drifts with cutoff, self-oscillation pitch is unreliable.
The modern standard is a zero-delay-feedback (TPT) ladder with per-stage
saturation and modest oversampling — and **P13 explicitly requires a
self-oscillating filter**, which this topology cannot deliver in tune.
**Recommendation:** no change now (effects-chain duty is fine); write the
ZDF ladder as the first P13 commit (from scratch per policy, Zavalishin's
TPT derivation; `stmlib`'s ZDF SVF as MIT cross-reference). OQ-7
(oversampling) activates there too.

## 6. ReverbNode (Freeverb)

Faithful Schroeder/Freeverb; fine as the v1 master verb. Two notes: the
damped combs are the denormal hazard named above (fix at the thread level,
not per-node), and the P15 palette (plate/spring) should be *additional*
nodes — Freeverb's character is its own thing. **Airwindows (MIT!) is the
single best-aligned external source for P15**: dozens of characterful
saturation/tape/reverb/chorus effects, permissively licensed, portable
per-algorithm — exactly the "70s/80s character" scope. Adopt as a named
reference in the P15 planning docs.

## Open-source integration map (policy: MIT/Apache source or from-scratch)

| Source | License | Use | When |
|---|---|---|---|
| Mutable *peaks*/*plaits*/*stmlib* | MIT | drum-model quality pass; ZDF SVF reference | P12+/P13 |
| `mi-plaits-dsp` (Rust port) | MIT | evaluate as dependency for P13/P14 voices | P13 spike |
| Airwindows | MIT | effects palette source (saturation/tape/verb) | P15 |
| `rubato` | MIT | keep: load-time resampling only | now |
| from-scratch Hermite | — | replace per-voice sinc | with BUG-013 |
| `arc-swap` | MIT/Apache | executor cell swap | W-track live patching |
| `no_denormals` or inline FPCR/MXCSR | MIT | FTZ/DAZ on audio thread | with BUG-012 |
| Surge XT / Vital / Odin2 | GPL3 | **study-only** (policy reaffirmed — keeps node portability options open) | — |
| fundsp / dasp | MIT/Apache | algorithm cross-reference only; no dependency need identified | — |

## Scheduling deltas proposed

1. **BUG-012** (device rate/buffer negotiation) — correctness; schedule as a
   small standalone commit before W1 C5's paired session (a session on a 48k
   interface would be mistuned).
2. **BUG-013** (sub-block voice starts) + Sampler Hermite — schedule with
   P10 C3 (micro-timing), which is inaudible without it.
3. Denormals/FTZ — fold into the BUG-012 commit (same file).
4. Everything else — trigger-based backlog entries; no phase slots.
