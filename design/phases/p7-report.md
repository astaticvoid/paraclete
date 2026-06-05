Paraclete — P7 Implementation Report
=====================================
Date: June 2026
Status: In progress — 310 tests, 0 failures (after Commit 5)

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


Commit 3 — Project save/recall (paraclete-runtime, paraclete-app)
Tests after: 299 (+4 from Commit 2)

  ron crate + serde (workspace deps)

    Added `ron = "0.8"` and `serde = { version = "1", features = ["derive"] }` to
    workspace Cargo.toml. Added to paraclete-app Cargo.toml as direct deps.
    License: MIT/Apache-2.0.

  NodeConfigurator additions (paraclete-runtime/src/configurator.rs)

    EdgeView — public struct {src_node, src_port, dst_node, dst_port}. Not serde;
      used only for validation in save_project(). Deriving Clone+Debug.

    NodeOrDevice::as_node_ref() / as_node_mut() — match-dispatch helpers that
      coerce Box<dyn HardwareDevice> to &dyn Node via trait upcasting (Rust 1.86+).
      Used internally by all_nodes() and node_mut().

    all_nodes() — iterates id_to_index in sorted user_id order; returns
      impl Iterator<Item = (u32, &dyn Node)>. Only valid before build_executor()
      (nodes are drained at build time). Does NOT add a runtime guard — callers
      are responsible for calling before build.

    all_edges() — uses petgraph::visit::IntoEdgeReferences (needed in import) to
      call self.graph.edge_references(). Maps each edge to EdgeView.

    node_mut() — looks up id_to_index → nodes HashMap → returns &mut dyn Node
      via as_node_mut(). Returns None if node not registered or already drained.

  paraclete-app: lib.rs + project.rs

    Added [lib] section to Cargo.toml so integration tests can import the module.
    paraclete-app/src/lib.rs exposes `pub mod project;`.
    paraclete-app/src/project.rs:
      Project, ProjectMetadata, GraphSnapshot, NodeSnapshot, EdgeRecord,
      ProfileBinding — serde Serialize/Deserialize structs.
      ProjectError — Io(std::io::Error) | Parse(ron::error::SpannedError) |
        UnknownVersion(u32). Implements Display and From conversions.
      save_project(path, conf, metadata, profiles) — collects all_nodes() +
        all_edges(), serialises to RON via to_string_pretty + PrettyConfig::default().
      load_project(path, conf) — reads RON, validates version, calls
        node_mut(id).deserialize(state) for each NodeSnapshot. Returns
        Ok(Vec<String>) with warnings for unknown node IDs and edge count mismatches.

  main.rs keyboard wiring

    Added --load=<path> and --save=<path> CLI flags (separate paths, no conflict).
    Load executes before build_executor(); save executes after load, also before
    build_executor(). Both paths silently skip if the arg is absent.

  Node::deserialize() doc fix (paraclete-node-api/src/node.rs)

    Corrected doc comment: was "Called on the main thread before activate()" —
    WRONG for nodes that use ParameterBank (activate() resets bank to defaults;
    deserialize() must run after to restore values). Now says "Called after
    activate()" with an explanation of the correct invariant for bank nodes.

  Code review findings and fixes

    Finding 1 (FIXED): deserialize-after-activate ordering — doc comment said
      "before activate()" but the actual sequence (and the correct sequence for
      ParameterBank nodes) is after. Corrected the doc comment.
    Finding 2 (FIXED): --save/--load path collision in main.rs — single
      project_path variable was computed from either arg, causing load to read
      from the wrong path when both flags are provided. Fixed by using separate
      save_path and load_path Option<PathBuf> variables.
    Finding 3 (FIXED): weak roundtrip test — was comparing serialised byte
      lengths (always equal for same struct layout) or byte content vs. empty
      sequencer (could miss field-mapping bugs). Fixed: test now compares the
      loaded node's serialised bytes against a reference sequencer with exactly
      the same step configured, byte-for-byte.
    Finding 4 (NOT FIXED, noted): Sequencer P5 fields (TrigCondition, StepTiming)
      not serialised — pre-existing limitation, not introduced by this commit.
      Deferred to P7.5+ with a note in the known-issues section.
    Finding 5 (ACCEPTED): silent empty node list post-build_executor — documented
      in all_nodes() doc comment; no runtime guard added (cost-benefit not worth
      the extra state field at P7).
    Finding 6 (ACCEPTED): no HardwareDevice upcast test — coverage gap is
      acceptable for P7; the upcast path is exercised in runtime integration tests
      via the existing hardware node tests.

  Tests added (+4, all in crates/paraclete-app/tests/project_tests.rs)

    project_save_creates_valid_ron_file — saves a 1-node graph, re-parses the
      file with ron::de::from_str, asserts version==1 and node count==1.
    project_save_then_load_restores_state — InternalClock + Sequencer with step 3
      set; roundtrip via save/load; loaded node serialised bytes compared
      byte-for-byte against a reference sequencer with the same step.
    project_load_unknown_node_id_skips_with_warning — injects id=999 via string
      replace on the RON; asserts Ok(warnings) with non-empty warning list.
    project_load_unknown_version_returns_error — writes version:99 RON manually;
      asserts Err(ProjectError::UnknownVersion(99)).


Commit 4 — paraclete-clap infrastructure (paraclete-clap new crate, paraclete-runtime)
Tests after: 307 (+8 from Commit 3)

  New crate: paraclete-clap

    Workspace member crates/paraclete-clap. Dependencies: paraclete-node-api,
    paraclete-runtime, paraclete-nodes. No clap-sys yet — added in Commits 5/6
    when actual plugin binaries land. Crate modules: bridge, transport (public);
    plugin, process_input, single_node, subgraph (stubs for later commits).

  ClapParamBridge (crates/paraclete-clap/src/bridge.rs)

    Maps CLAP's sequential parameter IDs (0,1,2...) to Paraclete
    content-addressed IDs (ParamDescriptor::id_for_name() hash). Built from a
    CapabilityDocument; iterates doc.params in declaration order, assigning CLAP
    IDs 0-based.

      from_capability_document(&doc) — builds Vec<ClapParamEntry>
      paraclete_id_for(clap_id) — returns Option<u32>
      make_set_param_command(clap_id, value, target_id) — returns
        Option<NodeCommand { type_id: CMD_SET_PARAM, target_id, arg0: paraclete_id as i64, arg1: value }>

    arg0 is i64 (NodeCommand field type), not f64 — caught during test writing;
    the spec pseudocode showed f64 but the actual NodeCommand struct uses i64 for
    arg0 to carry param IDs.

  translate_transport (crates/paraclete-clap/src/transport.rs)

    Signature: translate_transport(flags: u32, tempo: f64, song_pos_beats: i64, prev_playing: bool)
      -> (TransportInfo, Option<TransportEvent>)

    Constants (corrected in code review):
      CLAP_TRANSPORT_HAS_BEATS_TIMELINE=1<<1 (validity flag)
      CLAP_TRANSPORT_IS_PLAYING=1<<4 (NOT 1<<0 — bits 0-3 are HAS_* flags in spec)
      CLAP_TRANSPORT_IS_RECORDING=1<<5, CLAP_TRANSPORT_IS_LOOP_ACTIVE=1<<6
      CLAP_BEATTIME_FACTOR: i64=1<<31

    Takes raw scalar fields (not a clap-sys FFI struct) so it is testable in
    pure Rust without a C ABI dependency. When clap-sys is added in Commit 5,
    the CLAP process() callback extracts these scalars from the clap_event_transport_t
    and calls this function.

    Returns Some(TransportEvent) only on state transitions: global_start when
    !prev_playing && playing, global_stop when prev_playing && !playing, None
    otherwise. This prevents Sequencer step-counter resets (seq.current_step=0)
    on every audio buffer while playing. The CLAP adapter in Commit 5 maintains
    prev_playing state across calls.

    song_pos_beats is only read when CLAP_TRANSPORT_HAS_BEATS_TIMELINE is set
    (bit 1); defaults to bar=1,beat=0,tick=0 otherwise. Negative pre-roll
    positions are clamped to 0 to avoid Rust's saturating f64→u64 cast.

  NodeExecutor::set_transport_override (crates/paraclete-runtime/src/executor.rs)

    Field: transport_override: Option<(TransportInfo, Option<TransportEvent>)>
    Method: set_transport_override(&mut self, info: TransportInfo, event: Option<TransportEvent>)

    Applied at the start of process(), AFTER apply_messages() and AFTER the
    incoming.clear() loop. The after-clear placement is critical: if the event
    were injected before incoming.clear(), it would be wiped before any node
    sees it. The after-apply_messages() placement ensures the CLAP transport
    override wins over any ring-buffer messages that may arrive in the same cycle.
    Consumed via .take() so it applies exactly once per override.

  incoming vec pre-allocation (executor.rs)

    Changed `incoming: vec![vec![]; n]` to `(0..n).map(|_| Vec::with_capacity(16)).collect()`.
    Without this, the first set_transport_override push on a zero-capacity Vec
    triggers a heap allocation on the audio thread.

  Code review findings and fixes

    Finding 1 (FIXED): transport override events cleared before delivery.
      Moved override block to AFTER incoming.clear(). Regression test:
      transport_override_event_delivered_to_nodes.
    Finding 2 (FIXED): wrong CLAP flag bits — IS_PLAYING was 1<<0, correct
      value is 1<<4 (bits 0-3 are HAS_* validity flags in spec). Wrong bits
      caused playing=false every cycle regardless of DAW state.
    Finding 3 (FIXED): global_start emitted every cycle while playing would
      reset Sequencer step counter to 0 each buffer. Fixed by adding prev_playing
      parameter and only emitting on state transitions.
    Finding 4 (FIXED): incoming[i].push() could allocate on audio thread when
      vecs had capacity 0. Fixed by pre-allocating with_capacity(16) at build time.
    Finding 5 (FIXED): song_pos_beats used unconditionally; added HAS_BEATS_TIMELINE
      validity check and pre-roll clamp.

  Tests added (+8)

    paraclete-clap/tests/infrastructure_tests.rs (+7):
      bridge_from_cap_doc_assigns_sequential_ids
      bridge_unknown_clap_id_returns_none
      bridge_make_set_param_command_correct
      translate_transport_start_transition_emits_global_start
      translate_transport_no_event_when_already_playing
      translate_transport_stop_transition_emits_global_stop
      translate_transport_no_event_when_already_stopped

    paraclete-runtime/tests/runtime_integration.rs (+1):
      transport_override_event_delivered_to_nodes


Commit 5 — SingleNodePlugin (paraclete-clap)
Tests after: 310 (+3 from Commit 4)

  process_input.rs — make_process_input() helper

    Constructs a ProcessInput for a MIDI-effect node (no audio inputs).
    Takes transport, events, commands, slab, sample_rate, block_size.
    Used by SingleNodePlugin::process_block() and by the CLAP FFI callbacks
    that will be added in Commit 6.

  single_node.rs — SingleNodePlugin

    Struct: node: Box<dyn Node>, bridge: ClapParamBridge, sample_rate,
    block_size, events_out: EventOutputBuffer(256).

    Safe API:
      new(node)                    — wraps any Node; bridge starts empty
      activate(sr, bs)             — activates node + rebuilds param bridge
      deactivate()                 — delegates to node.deactivate()
      process_block(transport, events, commands) → Vec<TimedEvent>
                                   — runs one block; returns output events
      state_save() → Vec<u8>       — delegates to node.serialize()
      state_load(&[u8])            — delegates to node.deserialize()
      bridge() → &ClapParamBridge  — exposes bridge for params extension

    audio_outputs is an empty slice (`&mut []`) for MIDI-effect use.
    The CLAP FFI callbacks and binary entry point are added in Commit 6
    when clap-sys is introduced.

    state_load() before activate(): CLAP hosts call load() between
    deactivate() and activate() during session restore. For Sequencer this
    is safe: deserialize() sets self.steps directly; activate() only resets
    self.bank and self.rng (not self.steps), so the loaded steps survive.

  bridge.rs — ClapParamBridge::empty()

    Added empty() constructor returning a zero-entry bridge. Used in
    SingleNodePlugin::new() before the capability document is available.

  src/bin/sequencer.rs — stub binary

    Placeholder binary target establishing the module structure.
    The CLAP FFI entry point and cdylib build target are added in Commit 6.

  tests/clap_validator.rs — ignored CI test

    Tagged #[ignore] so it does not block cargo test runs. Validates the
    Sequencer .clap binary with clap-validator when run explicitly:
      cargo test --test clap_validator -- --ignored

  Code review findings

    Finding (DEFERRED): Sequencer::serialize() does not save the swing
    bank parameter. A round-trip through state_save()/state_load()/activate()
    silently loses any non-zero swing value (activate() resets the bank to
    defaults). Pre-existing issue (same class as TrigCondition/StepTiming
    noted in p6-report.md). Fixing requires bumping the serialize version
    and an activate() change to re-apply saved bank params; deferred to P7.5.

  Tests added (+3, all in paraclete-clap/tests/single_node_tests.rs)

    single_node_plugin_init_activate_deactivate_no_panic
    single_node_plugin_process_passes_transport_to_node
    single_node_plugin_state_roundtrip


PENDING
-------

Commit 6 — SubgraphPlugin + machine bank binaries (paraclete-clap)
  Target: ~313 tests

  (to be filled when Commit 6 lands)

Commit 7 — crates.io publication prep (paraclete-node-api v0.1.0)
  Target: ≥310 tests (310 was the original spec target; Commit 6 may overshoot)

  (to be filled when Commit 7 lands)
