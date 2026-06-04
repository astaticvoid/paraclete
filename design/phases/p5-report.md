Paraclete — P4.5 + P5 Implementation Report
============================================
Date: June 2026
Status: In progress — P4.5 complete, P5 in progress


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

Commits so far: none


WHAT SHIPPED (P5)
-----------------
(updated as each commit lands)


P5 KNOWN GAPS AND DEVIATIONS
-----------------------------
(updated as each commit lands)


P5 TEST COUNT
-------------
Before P5: 212 tests
After P5:  (target: 212 + ≥20 new)
