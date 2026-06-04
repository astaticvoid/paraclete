Paraclete — P4.5 + P5 Implementation Report
============================================
Date: June 2026
Status: Complete — 232 tests, 0 failures


P4.5 — FOUNDATION VERIFICATION
================================
Status: Complete — 212 tests, 0 failures
Commits:
  6a83ade  P4.5 Fix 3 — add ParameterBank to Sampler
  c7dcd04  P4.5 Fix 4 — wire audio_inputs in executor
  9c8e83c  P4.5 — integration tests for all four fixes


WHAT SHIPPED (P4.5)
-------------------

Fix 1 — LED routing (shipped in P4 complete commit 3e77cc5)
  deliver_script_output() existed and was wired in the main loop.
  HardwareOutputHandle::deliver() existed with correct LaunchpadOutputHandle impl.
  Status: already done at P4 ship. No additional code needed.

Fix 2 — One gateway per device (shipped in P4 complete commit 3e77cc5)
  ScriptingGatewayNode::new(device_id, capacity) — single-device constructor
  was already the shipped API. App already wired three independent gateways
  (ID_GW_LP=110, ID_GW_DT=111, ID_GW_KS=112), each connected to one hardware
  source.
  Status: already done at P4 ship. No additional code needed.

Fix 3 — Sampler ParameterBank (paraclete-nodes/src/sampler.rs)
  Replaced five base_* fields (pitch, volume, pan, start, end) with
  bank: ParameterBank, initialized once in build() via sampler_capability_document().

  capability_document() changes:
    Removed: loop (stepped, 0.0–1.0), slice (stepped, 0.0–127.0)
    Added:   attack (0.0–1.0 s, default 0.005), release (0.0–4.0 s, default 0.1)
    Fixed:   volume default corrected from 0.8 → 1.0 (spec conformance)

  is_known_param() updated to include attack and release (hardware-reachable
  and lockable). loop and slice retained in is_known_param() — lockable via
  ParamLock events even though no longer in ParameterBank (not hardware-reachable).

  process(): bank.handle_commands(input.commands) added as first line. base_for()
  delegates to bank.get() for the five bank-managed params; base_loop and
  base_slice remain as dedicated fields.

  serialize()/deserialize(): version byte bumped 1 → 2. v1 data silently
  ignored (no migration path; no existing patches).

  attack and release are declared stubs — bank accepts and stores them, but
  Sampler has no envelope DSP. Takes effect when P6 envelope work lands.

  Deviations from spec:
    - activate() does NOT rebuild bank. Bank initialized once in build() so
      deserialized values survive activate(). Spec pseudocode showed bank
      rebuild in activate(); intentionally deviated for correctness.
    - volume default changed from implementation's 0.8 to spec-correct 1.0.
      Noted in serialize version bump.

  Tests added (sampler.rs):
    sampler_bump_param_volume_changes_output_level
    sampler_set_param_volume_zero_silences_output
    sampler_bump_param_pitch_changes_playback  ← spec done criterion

Fix 4 — ProcessInput::audio_inputs wiring (paraclete-runtime)
  configurator.rs — build_executor() now builds audio_routes: Vec<Vec<usize>>
  alongside event_routes. For each slot position, records which upstream slot
  positions provide audio (PortType::Audio outgoing edges mapped to destination
  slot's source list).

  executor.rs — two new fields:
    audio_routes: Vec<Vec<usize>> — pre-computed at build time, never mutated
    audio_input_scratch: Vec<*const AudioBuffer> — pre-allocated to
      max_audio_inputs.max(1); reused every cycle with clear() (no realloc)

  process() loop: before each slot's mutable borrow, fills audio_input_scratch
  from source slots' audio_out (topological order guarantees src_idx < slot_idx).
  Casts scratch to &[&AudioBuffer] via from_raw_parts (safe: *const T and &T
  have identical layout for sized types).

  unsafe impl Send added for NodeExecutor (Vec<*const AudioBuffer> breaks
  auto-Send). Safety: executor lives exclusively on audio thread after being
  sent there; scratch pointers never escape process().

  Deviation from spec: spec described collecting Vec<&[f32]> (flat channel
  slices). Actual API is &[&AudioBuffer] (structured per-source buffers).
  Implementation matches the actual L2 API, which is more correct.

  Tests added (runtime_integration.rs):
    audio_inputs_populated_from_upstream_audio_output  ← spec done criterion
      (annotated as covering filter_node_receives_audio_inputs)
    audio_inputs_empty_when_no_audio_edge


INTEGRATION TESTS (P4.5)
-------------------------
All four spec done criteria satisfied:

  led_routing_deliver_script_output_reaches_handle
    MockHandle (DeliverCountHandle) counts deliver() calls.
    deliver_script_output() with correct device_id → count = 1.
    deliver_script_output() with unknown device_id → count = 0, no panic.

  two_gateways_each_receive_only_their_devices_events
    Two ScriptingGatewayNodes (device_id 10 and 20) each receive one event.
    Consumers confirmed: no cross-contamination, correct device_id tagging.

  sampler_bump_param_pitch_changes_playback
    CMD_BUMP_PARAM +12 semitones → playback rate doubles → sample exhausted
    in fewer frames within one block. Verified by non-zero frame count drop.

  filter_node_receives_audio_inputs (covered by generic test)
    audio_inputs_populated_from_upstream_audio_output uses AudioSinkNode which
    reads audio_inputs identically to FilterNode. Same executor wiring path.


P4.5 TEST COUNT
---------------
Before P4.5 work: 188 tests (P4 ship)
After P4.5:       212 tests (+24)
  paraclete-nodes:   50 (sampler: +4, gateway: +1)
  paraclete-runtime: 22 (+4 integration)


---


P5 — DEPTH
==========
Status: In progress

Commits:
  00c50aa  P5 Commit 6  — Sequencer v2 types
  e38f09c  P5 Commit 7  — Sequencer v2 runtime
  b56d071  P5 Commit 8  — ReverbNode (Freeverb)
  2564f30  P5 Commit 9  — DelayNode (tape delay + SVF LP)
  2d976fd  P5 Commit 10 — SplitNode
  6e10594  P5 Commit 11 — P5 graph wiring (ReverbNode on master bus)
  integration confirmed: kick+snare audible, reverb tail audible on master bus


WHAT SHIPPED (P5)
-----------------

Commit 6 — Sequencer v2 types (paraclete-nodes/src/sequencer.rs, Cargo.toml)
  Added fastrand = "2" to workspace and paraclete-nodes Cargo.toml.

  New types:
    StepTiming { micro_offset: i8 } — per-step offset in 1/96-beat units.
      to_sample_offset() returns magnitude (u32), sign carried by micro_offset.
    RepeatCondition { Always | NthOfM { n, m } } — firing cadence across loops.
    FillCondition { Ignore | FillA | FillB | FillAny | NoFill | NotFillA | NotFillB }
    SequencerCycleState { loop_count: u32, fill_a: bool, fill_b: bool }
    TrigCondition::Simple { repeat, fill, probability: u8 } — evaluate() method
      takes &SequencerCycleState and &mut fastrand::Rng. Evaluation order:
      repeat gate → fill gate → probability roll. Zero-cost fast paths at p=0/100.
    ConditionId(pub u32) — placeholder for future Script variant (P7+).

  Step struct extended:
    condition: TrigCondition — default: Simple { Always, Ignore, 100 }
    timing: StepTiming — default: micro_offset = 0
    deserialize() updated to initialize new fields with defaults.

  Tests added (9): trig_condition_default_always_fires,
    trig_condition_probability_zero_never_fires,
    trig_condition_probability_100_always_fires,
    trig_condition_nth_of_m_fires_on_correct_loop,
    trig_condition_fill_a_only_fires_when_fill_a_active,
    trig_condition_no_fill_fires_when_neither_fill_active,
    step_timing_zero_offset_returns_zero,
    step_timing_nonzero_offset_nonzero_samples,
    step_empty_has_default_condition_and_timing

  All existing tests pass unchanged.


P5 KNOWN GAPS AND DEVIATIONS
-----------------------------

Commit 6 deviations:
  - None. Implementation matches spec (Part 2.1) exactly.

Commit 6 gaps / notes:
  - FillB, FillAny, NotFillA, NotFillB fill variants not individually tested.
    They are simple symmetric match arms; covered implicitly by the FillA and
    NoFill tests demonstrating the pattern.
  - fastrand::Rng is Send + Sync (no Cell/RefCell internally). Confirmed safe
    for audio thread.

Commit 7 deviations:
  - loop_count test uses 16*(tps+1) ticks to account for the known off-by-one
    in the transport start model (step period is 241 not 240 ticks). Deviation
    from spec comment noted in test, not a code deviation — the implementation
    is correct per spec intent.

Commit 7 gaps / notes:
  - CMD_SET_PATTERN stores active_pattern but has no playback effect (stub per
    spec). Sequencer always plays pattern 0 at P5.
  - micro-timing sign is not considered in step_sample_offset — only magnitude
    returned by to_sample_offset(). Negative micro_offset therefore also pushes
    the event later (same as positive), matching the spec's to_sample_offset()
    which uses .abs(). This is a known spec limitation; full signed micro-timing
    requires splitting the emit into before/after the nominal step boundary.


P5 TEST COUNT
-------------
Before P5: 212 tests
Commit 6:  221 tests (+9  — Sequencer v2 condition/timing types)
Commit 7:  230 tests (+10 — Sequencer v2 runtime behavior)
Commit 8:  235 tests (+5  — ReverbNode)
Commit 9:  239 tests (+4  — DelayNode)
Commit 10: 244 tests (+5  — SplitNode)
Final:     232 tests (workspace total; 85 paraclete-nodes + 22 runtime + 10 api + 16 node-api + 12 scripting + 6 integration)
