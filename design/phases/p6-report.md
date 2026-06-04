Paraclete — P6 Implementation Report
=====================================
Date: June 2026
Status: Complete — 285 tests, 0 failures

Commits:
  0d1b8cb  Paraclete — P6 complete (all nine spec commits in one ship commit)
  Post-ship: code review (7 finder angles × 7 verifiers) → 6 confirmed/plausible
             findings fixed in analog_engine.rs, fm_engine.rs, sampler.rs.


WHAT SHIPPED (P6)
-----------------

Commit 1 — L2 API extension (paraclete-node-api/src/context.rs)

  `ProcessInput` changes:
    logic(port_id) — was returning `&LogicBuffer` (unimplemented stub). Now
      returns `&[f32]`, scanning signal_inputs for the matching port and kind;
      falls back to static SILENCE[..block_size] when unconnected.
    modulation(port_id) — same pattern, was returning `&ModBuffer` stub.
    Removed: modulation_mut(), logic_mut(), cv_mut(), pitch_mut(), phase_mut()
      (all were unimplemented!() panics; no callers existed).

  `ProcessOutput` additions:
    mod_output_mut(port_id) — returns &mut [f32] from signal_outputs for
      Modulation kind. Panics if port not found (intentional: signals that the
      node's port declaration and the executor's slot allocation are out of sync).
    logic_output_mut(port_id) — same for Logic kind.

  Both use the private find_output() helper that panics unconditionally (not
  debug-only) on missing port.

  Tests added (context.rs):
    logic_input_unconnected_returns_zeros
    mod_output_mut_returns_block_size_slice


Commit 2 — OscillatorNode (paraclete-nodes/src/oscillator.rs, appended)

  Five waveforms: 0=sine, 1=triangle, 2=sawtooth+polyBLEP, 3=square+polyBLEP,
  4=xorshift32 LFSR noise.

  poly_blep(phase, phase_inc) helper — first-order correction on leading/trailing
  edge; applied to sawtooth (one edge) and square (two edges).

  Gate input: rising edge resets phase to 0.0 (hard sync to gate).
  Pitch CV and FM modulation inputs (Modulation port type).
  Audio output: writes to output.audio_outputs.first_mut()?.channel_mut(0).

  Parameters: waveform (stepped 0–4), pitch (-60..60 semitones),
    tune (-100..100 cents), level (0–2, default 1).

  Tests added (oscillator_node_tests module, 7):
    oscillator_portability_check, oscillator_sine_output_is_periodic,
    oscillator_sawtooth_output_is_nonzero, oscillator_square_output_is_nonzero,
    oscillator_noise_output_is_nonzero, oscillator_pitch_cv_shifts_frequency,
    oscillator_gate_rising_edge_resets_phase


Commit 3 — EnvelopeNode (paraclete-nodes/src/envelope.rs, new file)

  ADSR, AD (auto-reset), and Looping AD modes. Retrigger from current envelope
  value (no snap to zero) to avoid click, consistent with AdState::trigger().

  EnvPhase enum: Idle, Attack, Decay, Sustain, Release (private).
  Gate rising edge → Attack regardless of current phase (retrigger).
  Gate falling edge in ADSR mode → Release. AD and Looping modes ignore gate low.
  Looping AD: resets to Idle (→ next gate re-triggers) after Decay completes.
  Output range 0.0–1.0 written to mod_output_mut(PORT_MOD_OUT).

  Parameters: env_mode (stepped 0–2), attack (0.001–4s), decay (0.001–8s),
    sustain (0–1), release (0.001–8s).

  Tests added (8):
    envelope_adsr_attack_reaches_one, envelope_adsr_decay_falls_to_sustain,
    envelope_adsr_gate_low_triggers_release, envelope_adsr_release_reaches_zero,
    envelope_ad_completes_without_gate, envelope_ad_ignores_gate_low,
    envelope_looping_ad_restarts_after_decay, envelope_retrigger_from_nonzero_no_click


Commit 4 — LfoNode (paraclete-nodes/src/lfo.rs, new file)

  Five shapes: 0=sine, 1=square, 2=ramp-up, 3=ramp-down, 4=S&H.
  S&H: xorshift32 LFSR updates held_value each time phase wraps (p < phase_inc).
  Sync input (Logic): rising edge resets phase to lfo_phase offset parameter.
  Output range: -depth..+depth written to mod_output_mut(PORT_MOD_OUT).

  Parameters: lfo_waveform (stepped 0–4), lfo_rate (0.01–20 Hz),
    lfo_depth (0–1), lfo_phase (0–1 initial phase offset).

  Tests added (7):
    lfo_sine_output_oscillates_between_minus_one_and_one,
    lfo_square_output_is_only_plus_minus_depth, lfo_ramp_up_rises_monotonically,
    lfo_ramp_down_falls_monotonically, lfo_random_holds_value_between_cycles,
    lfo_sync_rising_edge_resets_phase, lfo_depth_zero_produces_silence


Commit 5 — LadderFilterNode (paraclete-nodes/src/ladder.rs, new file)

  4-pole Moog-style ladder LP. Self-oscillates at resonance = 1.0.
  Algorithm: x = tanh(in * drive_gain - 4 * r * stage[3]); four cascaded
  one-pole LP sections accumulating into stage[0..3].
  drive_gain = 1 + drive * 4. Soft input saturation before the feedback path.

  Ports: audio_in (0, Mono), cutoff_mod (1, Mod), resonance_mod (2, Mod),
    audio_out (3, Mono). Cutoff and resonance accept per-sample modulation.
  cutoff clamped to [20, sample_rate * 0.49]. fc computed as sin(π * fc / sr).

  Parameters: ladder_cutoff (20–18000 Hz, default 4000), ladder_resonance (0–1),
    ladder_drive (0–1). Extension: "paraclete.effect".

  Tests added (5):
    ladder_attenuates_above_cutoff, ladder_resonance_peak_at_cutoff,
    ladder_does_not_blow_up_at_max_resonance_and_drive,
    ladder_cutoff_mod_shifts_frequency, ladder_portability_check


Commit 6 — Sampler audio quality (paraclete-nodes/src/sampler.rs)

  WAV loader rewritten: hound replaced by symphonia (WAV/FLAC/AIFF/OGG/MP3).
  Probe by magic bytes (not extension). Deinterleaves N-channel to mono by
  averaging channels.

  Load-time resampling via rubato 0.15 SincFixedIn when native_rate ≠
  target_rate. Parameters: sinc_len=64, f_cutoff=0.95, Linear interpolation,
  oversampling_factor=64, BlackmanHarris2 window. Per-voice real-time pitch
  resampling remains linear interpolation (per-voice rubato deferred to P7).

  Envelope DSP: attack/release parameters (declared as stubs in P4.5) activated.
    EnvPhaseSimple { Attack, Release, Done } added to Voice struct.
    trigger_voice() initializes env_value=0.0, env_phase=Attack.
    Render loop: attack_inc = 1/(attack_s * sr).max(1); Attack ramps env_value
    to 1.0 then transitions to Release. Release applies exponential decay
    (release_coeff = 0.001^(1/(release_s*sr).max(1))). Done deactivates voice.

  Workspace: hound removed from paraclete-nodes; symphonia + rubato added.
  hound retained in gen-samples tool.

  Tests added (5):
    sampler_attack_nonzero_delays_onset, sampler_release_nonzero_extends_tail,
    sampler_default_attack_0005_is_audibly_instantaneous,
    sampler_rubato_pitch_up_12_semitones_produces_audio,
    sampler_symphonia_wav_loads_and_plays


Commit 7 — AnalogEngine (paraclete-nodes/src/analog_engine.rs, new file)

  Three machine variants: Kick, Snare, HiHat. All share one struct; machine
  selection routes process() to the appropriate DSP function.

  engine_dsp.rs (new file, private module): shared helpers used by both
  AnalogEngine and FmEngine.
    AdPhase { Idle, Attack, Decay } + AdState { trigger(), tick(), is_idle() }
      — trigger() preserves current value for anti-click retrigger.
    xorshift(state: &mut u32) — xorshift32 LFSR, returns f32 in [-1, 1].
    note_to_hz(note, tune_semitones) — MIDI note to Hz.
    soft_clip(x) — tanh saturation.
    svf_lp_sample(input, f, q, &mut low, &mut band) — Chamberlin SVF LP,
      returns low output.

  KickMachine: sine osc + pitch-drop envelope (AdState, 2ms attack / 80ms decay)
    driving frequency; amp envelope controls overall level; SVF LP smooths click.
    Parameters: tune, punch (pitch-drop depth), decay, drive (post-SVF saturation),
    tone (SVF cutoff).

  SnareMachine: sine body + bandpass-filtered xorshift noise. Two independent
    AdState envelopes (body_env for snap, noise_env for tail). SVF run in
    bandpass mode (uses .svf_band output). Parameters: tune, snap, noise, decay,
    tone.

  HiHatMachine: xorshift noise through SVF, high-pass derived by subtracting
    SVF low from raw noise. open parameter multiplies effective decay (×8 at
    max). Parameters: tone, decay, open.

  Event handling: ParamLock before NoteOn (executor ordering). Port layout:
    events_in (0, Event), audio_out_l (1, Audio), audio_out_r (2, Audio).
  Both channels carry the same mono signal (stereo wiring deferred to P7).

  Tests added (10):
    analog_kick_produces_audio_on_note_on, analog_kick_is_silent_before_note_on,
    analog_kick_punch_zero_has_no_pitch_drop, analog_snare_body_and_noise_both_present,
    analog_snare_noise_zero_silences_noise_path, analog_hihat_open_extends_decay,
    analog_hihat_closed_short_decay, analog_bump_param_decay_changes_output_length,
    analog_param_lock_drive_overrides_base_drive, analog_portability_check


Commit 8 — FmEngine (paraclete-nodes/src/fm_engine.rs, new file)

  Two-operator FM (phase modulation). Three machine variants. Modulator output
  scales carrier phase increment per sample.

  FmKickMachine: pitch-drop envelope + FM feedback on modulator. Modulator feeds
    into carrier phase. index = 2 + punch * 4. Feedback term: prev_mod_out * fb.
    Parameters: tune, punch, decay, feedback, drive.

  FmBellMachine: long decay, non-integer ratio (default 3.5), FM feedback. mod_env
    and amp_env share same attack/decay envelope curves. Parameters: tune, ratio,
    index, decay, feedback.

  FmBassMachine: configurable attack, shorter mod_env decay (decay * 0.3) to
    shape carrier vs modulator independently. Parameters: tune, ratio, index,
    attack, decay, drive.

  Same port layout as AnalogEngine. Extension: "paraclete.instrument".

  Tests added (8):
    fm_kick_produces_audio_on_note_on, fm_kick_feedback_zero_vs_nonzero_timbre,
    fm_bell_decays_longer_than_kick, fm_bell_noninteger_ratio_differs_from_integer,
    fm_bass_attack_param_changes_onset_slope, fm_bass_drive_increases_rms_output,
    fm_param_lock_ratio_overrides_base_ratio, fm_portability_check


Commit 9 — P6 graph wiring (paraclete-app/src/main.rs)

  Track assignments changed from all-Sampler to:
    Track 0 (node 20) = AnalogEngine::kick()
    Track 1 (node 21) = AnalogEngine::snare()
    Track 2 (node 22) = AnalogEngine::hihat()
    Track 7 (node 27) = FmEngine::bass()
    Tracks 3–6 (nodes 23–26) = Sampler::with_path(...)

  events_in_port and audio_out_port resolved per match arm so existing
  connect() calls require no structural change.

  lib.rs: new public re-exports added:
    AnalogEngine, AnalogMachine, EnvelopeNode, FmEngine, FmMachine,
    LadderFilterNode, LfoNode, OscillatorNode

  eprintln startup message updated to indicate P6.


POST-SHIP CODE REVIEW AND FIXES
---------------------------------

A structured code review (7 finder angles, 7 independent verifiers) ran after
the P6 ship commit. Six findings confirmed or plausible:

Finding 1 — CONFIRMED: AnalogEngine ParamLock bleeds across cycles
  File: analog_engine.rs
  Root cause: ParamLock events called bank.handle_commands(&[CMD_SET_PARAM, ...]),
  permanently mutating the ParameterBank. Subsequent cycles without a lock
  continued using the locked value.
  Fix: Added node_locks: Vec<(u32, f64)> field, cleared at the start of each
  process() cycle. ParamLock events push to node_locks rather than the bank.
  Added get_param() helper that checks node_locks first, then bank. All param
  reads in retrigger() and process_kick/snare/hihat() switched to get_param().
  Test added: analog_param_lock_does_not_bleed_to_next_cycle — verifies bank
  drive stays 0.0 after a drive=1.0 locked cycle, and cycle-2 output differs
  from a locked-drive reference.

Finding 2 — CONFIRMED: Sampler attack min=0.0 causes immediate silence
  File: sampler.rs, sampler_capability_document()
  Root cause: attack min=0.0 allowed attack_inc=1.0 via
  1.0/(0.0*44100.0).max(1.0). envelope transitions to Release in one sample.
  Fix: attack min changed from 0.0 to 0.001, matching EnvelopeNode's constraint.

Finding 3 — CONFIRMED: AnalogEngine SVF filter reset on retrigger causes click
  File: analog_engine.rs, retrigger()
  Root cause: retrigger() unconditionally set svf_low=0.0, svf_band=0.0. At
  rapid retrigger, the accumulated filter state snapped to zero, producing an
  audible transient. This contradicts AdState::trigger()'s anti-click design
  (which preserves value on retrigger).
  Fix: Removed the two SVF reset lines. Filter state now carries over naturally;
  the amp envelope (which starts from its current value, not zero) drives the
  filter input to the correct region without a discontinuity.

Finding 4 — CONFIRMED (latent): FmEngine pitch_env triggered but never ticked
             in Bell/Bass
  File: fm_engine.rs, retrigger()
  Root cause: retrigger() called pitch_env.trigger() for all machine types, but
  process_bell() and process_bass() never call pitch_env.tick(). On each NoteOn,
  pitch_env enters Attack phase and is never advanced, accumulating stuck state.
  No current audio artifact (Bell/Bass don't read pitch_env), but a maintenance
  hazard for P7 pitch-envelope work where Bell might gain pitch-chirp.
  Fix: retrigger() now calls pitch_env.trigger() only when machine == FmMachine::Kick.

Finding 5 — PLAUSIBLE: symphonia sample_rate unwrap_or(44100) silent pitch error
  File: sampler.rs, load_wav()
  Root cause: codec_params.sample_rate.unwrap_or(44100) silently assumed 44.1 kHz
  when sample rate was absent from codec metadata (possible with certain encoders),
  causing playback at the wrong pitch with no error signal.
  Fix: Changed to ok_or_else(|| "audio file has no sample rate...") — load fails
  loudly rather than silently playing at the wrong pitch.

Finding 6 — CONFIRMED (efficiency): resample_sinc wasteful on empty input
  File: sampler.rs, resample_sinc()
  Root cause: called with empty &[] slice (zero-duration audio file), the function
  constructed a rubato SincFixedIn resampler, padded 512 zeros, ran a full sinc
  convolution, and truncated the output to 0. Correct result (empty Vec) but
  wasted one full resampler construction and convolution.
  Fix: Early return `if samples.is_empty() { return Ok(vec![]); }` before
  rubato construction.

Finding refuted: mod_output_mut panic in executor
  EnvelopeNode and LfoNode are not registered in the executor graph at P6 —
  they are standalone primitives for manual graph wiring. Their tests construct
  SignalOutputSlot manually. The "panic on missing port" path in find_output()
  is only reachable if a registered node declares a port but the executor fails
  to allocate the slot, which cannot happen given the current executor design.


P6 KNOWN DEFERRED ITEMS
------------------------

- Primitive nodes (EnvelopeNode, LfoNode, OscillatorNode) not wired into the
  executor graph — signal port pre-allocation (SignalOutputSlot) deferred to P7.
  Tests construct SignalOutputSlot manually. Current use case is standalone
  primitives for inner GraphNode graphs (P9+).
- Per-voice real-time pitch resampling in Sampler uses linear interpolation.
  Rubato is only applied at load time. Per-voice rubato deferred to P7.
- AnalogEngine/FmEngine produce mono audio packed into stereo buffer. True
  stereo widening deferred to P7.
- AnalogEngine and FmEngine are monophonic with retrigger. Polyphony deferred to P7.
- OscillatorNode unison, FM input, and hard-sync between oscillators: P7+.
- LadderFilter HP/BP modes: P7.
- LFO tempo sync (requires TransportInfo in process()):  P7.
- 4-operator FM and more complex FM topologies: P8+.


P6 TEST COUNT
-------------

Before P6: 232 tests (P5 ship)

Workspace total after P6 + post-ship fixes: 285 tests, 0 failures

  paraclete-node-api:     16  (+2  — context signal I/O tests)
  paraclete-runtime:      22  (unchanged)
  paraclete-nodes lib:   136  (+51 — all new nodes + Sampler envelope/symphonia)
  paraclete-nodes int:     6  (unchanged — eight_track_pattern)
  paraclete-scripting:    12  (unchanged)
  paraclete-hal:           5  (unchanged)
  Other (node-api, etc):  88  (unchanged)

New tests in paraclete-nodes by module:
  engine_dsp / shared helpers:  (covered by AnalogEngine + FmEngine tests)
  oscillator (OscillatorNode):   7
  envelope (EnvelopeNode):       8
  lfo (LfoNode):                 7
  ladder (LadderFilterNode):     5
  analog_engine (AnalogEngine): 11  (10 original + 1 no-bleed fix)
  fm_engine (FmEngine):          8
  sampler (P6 additions):        5
  Total new:                    51
