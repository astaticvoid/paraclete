Paraclete — P7 Implementation Report
=====================================
Date: June 2026
Status: In progress — 295 tests, 0 failures (after Commit 2)

Commits:
  de1db04  Paraclete P7 Commit 1 — type_name() + published_state() push-down (OQ-9)
  (staged) Paraclete P7 Commit 2 — per-voice rubato pitch resampling in Sampler


WHAT SHIPPED (P7)
-----------------

Commit 1 — Node API evolution (paraclete-node-api, paraclete-nodes, paraclete-runtime)
Tests after: 291 (+2 from baseline 289)

  Node::type_name() (paraclete-node-api/src/node.rs)

    New optional method on the Node trait. Default returns
    `std::any::type_name::<Self>()` (fully-qualified Rust path). Nodes that
    want a stable, refactor-proof label override it to return a string literal:
    `fn type_name(&self) -> &'static str { "AnalogEngine" }`. Required by
    ADR-025: NodeSnapshot.type_name stores this label so project files remain
    readable after module-path changes.

    No existing node tests broke; the existing nodes use the default and their
    tests do not assert the exact string.

  Node::published_state() push-down (OQ-9 resolved)

    Signature changed from returning `Vec<(String, StateBusValue)>` to
    accepting `&mut Vec<(String, StateBusValue)>` (push-down pattern). Callers
    clear the buffer before calling; nodes push into it; Vec::clear() retains
    capacity so per-slot buffers never reallocate after the first audio cycle.

    The old signature caused a Vec heap allocation per node per audio cycle
    (OQ-9 in the P6 spec). The push-down eliminates these allocations.

  NodeExecutor state buffer pre-allocation (paraclete-runtime/src/executor.rs)

    Two new pre-allocated fields on NodeExecutor:
      state_bufs: Vec<Vec<(String, StateBusValue)>>  — one entry per slot,
        pre-seeded with capacity 8. Cleared (not dropped) before each
        published_state() call; capacity is retained across cycles.
      agg_state_buf: Vec<(String, StateBusValue)>  — pre-seeded with capacity
        64. Filled by extend_from_slice from each state_bufs entry. Transferred
        to StateBusUpdate via mem::take() so no collect() allocation occurs.
        Grows back to stable capacity within a few cycles.

    agg_state_buf itself is mem::take()'d each cycle (one allocation per cycle
    for the StateBusUpdate ownership transfer); a return channel would eliminate
    this last allocation but is deferred.

  HardwareDevice migration

    Removed dead _entries parameter from build_hardware_output() (was always
    ignored; no behavior change).

  Migrated nodes

    InternalClock, Sequencer, Sampler: published_state() converted to push-down
    signature. No behavioral change; same keys/values published.

  Tests added (+2)

    node_type_name_default_is_nonempty (internal_clock.rs) — asserts
      type_name() returns a non-empty string for InternalClock.
    state_buf_does_not_reallocate_after_first_cycle (runtime_integration.rs) —
      runs two audio cycles; asserts state_buf_capacity(idx) == capacity after
      the first cycle, confirming no realloc on the second cycle.


Commit 2 — Per-voice rubato pitch resampling in Sampler (paraclete-nodes)
Tests after: 295 (+4 from Commit 1)

  VoiceResampler struct

    New struct wrapping SincFixedOut<f32> with pre-allocated input and output
    Vec<Vec<f32>> buffers:

      struct VoiceResampler {
          rs: SincFixedOut<f32>,   // rubato resampler (1 channel, fixed output)
          input: Vec<Vec<f32>>,    // [1][up to chunk_size * max_ratio] pre-alloc
          output: Vec<Vec<f32>>,   // [1][chunk_size] pre-alloc
          ratio: f64,              // current ratio (output_frames / input_frames)
      }

    RUBATO_MAX_RATIO = 16.0 covers ±4 octaves (2^4 = 16). Ratios outside
    [1/16, 16] are clamped, not rejected. Rubato ratio semantics: ratio =
    output_frames/input_frames; ratio=2.0 means half-speed (pitch down 1 oct).
    Correct pitch conversion: ratio = output_sr / (native_sr * pitch_factor)
    where pitch_factor = 2^(note_diff/12).

  set_ratio() ordering: reset() must precede set_resample_ratio()

    Discovery during implementation: reset() restores the resampler to its
    original ratio (the value passed to SincFixedOut::new(), which is 1.0).
    Calling reset() AFTER set_resample_ratio() undoes the ratio change, leaving
    the resampler at ratio=1.0 with a warmup-inflated input_frames_next()
    (~192 frames at 64-frame block size). This caused immediate voice
    deactivation on the first block (new_pos ≥ sample length for short samples).
    Fix: call reset() before set_resample_ratio().

    This ordering rule is only needed at voice trigger (full resampler reset).
    Mid-voice ratio updates (for live CMD_BUMP_PARAM pitch changes) call
    set_resample_ratio() without reset() to avoid discontinuity.

  Sinc filter warmup

    SincFixedOut with sinc_len=256 requires sinc_len/2 = 128 frames of warmup
    on first call after reset(). First-call input_frames_next() ≈
    chunk_size/ratio + sinc_len/2. At ratio=2.0 (one octave down), chunk=64:
    input_frames_next() ≈ 32 + 128 = 160. Samples shorter than this length
    triggered past_end on the first block. Tests changed from 128-frame to
    512-frame dummy samples.

  root_note as ParameterBank entry

    Removed struct field `root_note: u8` from Sampler. The root_note is now the
    8th entry in the ParameterBank (min=0, max=127, default=60.0, stepped=true).
    Accessible via bank.get(param_hash("root_note")); participates in negotiate()
    lockable params list. CapabilityDocument version bumped to (0,5,0).

    Benefits: root_note is now lockable per-step (Sequencer can lock it), and
    it participates in the universal CMD_SET_PARAM/CMD_BUMP_PARAM dispatch path.

  Live ratio updates

    Ratio is recomputed at the start of every audio block (not just at trigger)
    so CMD_BUMP_PARAM pitch changes take effect immediately rather than waiting
    for the next note trigger. If the computed ratio differs from the current
    vrs.ratio by more than 1e-6, set_resample_ratio() is called without reset()
    for smooth mid-voice pitch change.

  advance_envelope() helper

    Extracted from duplicated ~30-line state machine that appeared in both the
    rubato path and the bypass (passthrough) path. Returns Option<f32>: Some
    contains the current envelope gain (Attack or Release phase), None signals
    voice deactivation (Done phase).

  Borrow checker strategy

    rubato's process_into_buffer() needs &mut rs, &input[..], &mut output[..].
    Using UFCS — Resampler::process_into_buffer(&mut vrs.rs, vrs.input.as_slice(),
    vrs.output.as_mut_slice(), None) — borrows three disjoint fields of
    VoiceResampler simultaneously; compiles correctly under Rust NLL field
    borrowing. The vrs borrow is block-scoped so it releases before updating
    voice.playback_pos and reading vrs.output to accumulate samples.

  Serialize version 3

    serialize() now emits version byte 3. Saves root_note from bank as f64.
    deserialize() handles: v2 (root_note as raw u8; clamped to [0,127] on read
    to guard against out-of-range byte values); v3 (root_note as f64 directly
    into bank). Pre-v2 format is unchanged.

  Tests added (+4)

    sampler_rubato_nonunity_pitch_produces_audio — trigger at note 72 (root 60,
      one octave up); confirms audio output after one block via rubato path.
    sampler_rubato_pitch_down_12_semitones_voice_survives_more_blocks — trigger
      at note 48 (one octave down, ratio=2.0); runs 5 blocks; confirms voice
      remains active and produces audio throughout (validates sinc warmup
      and reset/set_ratio ordering).
    sampler_root_note_param_affects_playback_ratio — two Sampler instances with
      different root_note values; same note trigger produces different output,
      confirming root_note routes through bank correctly.
    sampler_serialize_v3_root_note_round_trip — serialize v3 then deserialize;
      confirms root_note survives the round-trip via bank.get().


DEVIATIONS FROM SPEC
--------------------

Commit 1: None. Implemented exactly as specified in p7-interfaces.md §1.1 and §1.2.

Commit 2: None. Implemented exactly as specified in p7-interfaces.md §2.


PENDING
-------

Commit 3 — Project save/recall (paraclete-runtime, paraclete-app)
  Target: ~299 tests

  (to be filled when Commit 3 lands)

Commit 4 — paraclete-clap infrastructure (paraclete-clap new crate, paraclete-runtime)
  Target: ~304 tests

  (to be filled when Commit 4 lands)

Commit 5 — SingleNodePlugin (paraclete-clap)
  Target: ~307 tests

  (to be filled when Commit 5 lands)

Commit 6 — SubgraphPlugin + machine bank binaries (paraclete-clap)
  Target: ~313 tests

  (to be filled when Commit 6 lands)

Commit 7 — crates.io publication prep (paraclete-node-api v0.1.0)
  Target: ≥310 tests (310 was the original spec target; Commit 6 may overshoot)

  (to be filled when Commit 7 lands)
