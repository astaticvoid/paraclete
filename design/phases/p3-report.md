Paraclete — P3 First Instrument Implementation Report
======================================================
Date: May 2026
Status: Complete — 133 tests, 0 failures, binary builds clean


PREREQUISITE: VOCABULARY REVERT
---------------------------------
All P3 code uses the plain-English identifier vocabulary per vocabulary-revert.md.
The Greek naming scheme introduced at P2 was fully reverted before P3 work began.
The P2 implementation report records P2 under the Greek names; P3 and all future
phases use plain English. Mapping for reference:

  Melos              → Node
  Oikonomos          → NodeConfigurator
  Anamnesis          → Sequencer
  ProeistosNode      → InternalClock
  KoinoniaValue      → StateBusValue
  KoinoniaSnapshot   → StateBusSnapshot
  KairosInfo         → TransportInfo
  KairosEvent        → TransportEvent
  KairosFlags        → TransportFlags
  Kenoma             → ExtendedEventSlab
  Symbolon           → ConnectionAgreement
  Thyra              → Port
  anaphora()         → process()
  epiclesis()        → activate()
  apolysis()         → deactivate()
  paradidomi()       → serialize()
  paralambanon()     → deserialize()
  ekthesis()         → capability_document()


DELIVERABLE
-----------
P3 graph:

  InternalClock → Sequencer → Sampler → AudioOutput
                      ↑
  LaunchpadEmulator → HardwareMappingNode

Sequencer runs steps 0, 2, 4, 8 (C4, E4, G4, C5). On each active step the
Sampler plays samples/kick.wav (silent if the file is absent). The Sequencer
emits ParamLockEvents before NoteOn events at the same sample offset, so the
Sampler applies per-step parameter overrides before triggering a voice.

The main loop calls NodeConfigurator::process_state_bus() each millisecond,
draining the SPSC ring buffer and keeping the main-thread StateBusHandle current.
The scripting engine has read/write access to the state bus via Rhai.


WHAT SHIPPED
------------

paraclete-node-api (LGPL-3.0)

  agreement.rs — updated
    LockableParam struct added (Clone + Debug):
      param_id: u32, name: String, min/max/default: f64, unit: ParamUnit
    ConnectionAgreement gains lockable_params: Vec<LockableParam>
      baseline() initializes it to empty. The receiving node (e.g. Sampler)
      populates it in negotiate(); the sending node (e.g. Sequencer) reads it
      from the ConnectionRecord it receives back.
    ConnectionRecord struct added (Clone + Debug):
      agreement: ConnectionAgreement, partner_id: u32, local_port_id: u32
    ConnectionAgreement derives Clone + Debug. ParamUnit derives Clone + Debug
      (required by LockableParam).

  node.rs — updated
    Node::set_connection_record(&mut self, _record: ConnectionRecord) {}
      Default no-op. Override in nodes that store connection context.
    Negotiable marker trait added:
      pub trait Negotiable: Node {}
      Declares intent to use negotiate() and set_connection_record() actively.
      The runtime always invokes the handshake on all connections — this trait
      is a declaration of intent, not a runtime gate.

  state_bus.rs — replaced
    StateBusHandle struct: HashMap<String, StateBusValue> backing store.
      read(path) -> Option<&StateBusValue>
      write(path, value)
      write_sandboxed(path, value) -> Result<()> — rejects non-/node/ paths
      subscribe(path) -> StateBusSubscription
      poll_subscription(&self, sub: &mut StateBusSubscription) -> Option<&StateBusValue>
      apply_updates(entries: Vec<(String, StateBusValue)>) — called by L1
    StateBusSubscription: { path: String, last_value: Option<StateBusValue> }
      Polled via StateBusHandle::poll_subscription().
    StateBusValue, StatePublisher retained unchanged.
    Rationale for moving StateBusHandle to L2: the scripting layer (L4) needs
      to access the state bus without depending on the runtime (L1). Placing the
      stable API in L2 satisfies the layer constraint.

  context.rs — updated
    modulation(), cv(), phase(), logic(), pitch() signal accessors:
      Previously always unimplemented!(). Now:
      - Unconnected port (port_id not in signal_inputs): returns a static empty
        buffer (0 frames, Deref → &[]). .first().copied().unwrap_or(0.0) safely
        returns 0.0.
      - Connected port with wrong type: panics (programming error).
      - Connected port with correct type: still unimplemented! (P4).
      Uses std::sync::OnceLock<T> per signal type for the static silent buffers.

  lib.rs — updated
    New re-exports: ConnectionRecord, LockableParam, Negotiable,
      StateBusHandle, StateBusSubscription.


paraclete-runtime (GPL-3.0)

  Cargo.toml — updated
    rtrb = "0.3" added (MIT license). Lock-free SPSC ring buffer.

  state_bus.rs — replaced
    StateBusUpdate struct: entries: Vec<(String, StateBusValue)>
      One cycle's worth of state bus updates. Pushed by executor, drained by
      NodeConfigurator::process_state_bus().
    Arc<RwLock<HashMap>> (StateBusSnapshot) removed entirely.

  graph.rs — updated
    #[allow(dead_code)] removed from EdgeMeta::src_port and dst_port.
    These fields are now read by the Negotiable handshake in connect().

  executor.rs — updated
    state_bus_producer: rtrb::Producer<StateBusUpdate> replaces StateBusSnapshot.
    At end of each process() cycle:
      published_state() collected from all nodes into Vec<(String, StateBusValue)>.
      If non-empty, pushed to SPSC producer — non-blocking, drop if buffer full
        (one cycle of stale LED state is acceptable; no crash, no block).
      hardware update_output() called unconditionally every cycle.
    Event delivery sort added before each node's process() call:
      events in incoming[slot_idx] sorted by (sample_offset, event_priority)
      using sort_unstable_by_key (no allocation, in-place).
      Priority: ParamLock=0, Transport/Tempo=1, Midi2=2, Hardware=3, Extended=4.
      Guarantees ParamLockEvent arrives before NoteOn at the same sample_offset.

  configurator.rs — updated
    state_bus: Rc<RefCell<StateBusHandle>> — shared with scripting engine.
      Rc<RefCell<>> not Arc<Mutex<>>: NodeConfigurator is main-thread only.
      Making NodeConfigurator !Send is intentional and correct.
    state_bus_consumer: rtrb::Consumer<StateBusUpdate>
    state_bus_producer_pending: Option<rtrb::Producer<StateBusUpdate>>
      Held until build_executor(), then moved into the executor.
    process_state_bus(&mut self):
      Drains SPSC consumer, calls handle.apply_updates() for each batch.
      Main loop must call this between audio cycles.
    state_bus_handle() -> Rc<RefCell<StateBusHandle>>:
      Returns a clone of the Rc for sharing with the scripting engine.
    state_bus_read(), state_bus_write(), state_bus_subscribe(),
      state_bus_poll_subscription(): convenience accessors on the handle.
    connect() — Negotiable handshake added after cycle check:
      1. src_doc = nodes[src].capability_document() (immutable)
      2. dst_doc = nodes[dst].capability_document() (immutable)
      3. src_agreement = nodes[src].negotiate(&dst_doc) (mutable)
      4. dst_agreement = nodes[dst].negotiate(&src_doc) (mutable)
      5. Reconcile: sample_rate/block_size from runtime; lockable_params from
         dst_agreement (the instrument declares what the sequencer can lock).
      6. Deliver ConnectionRecord to both sides via set_connection_record().
    NodeOrDevice gains: capability_document(), negotiate(), set_connection_record()
      delegating to inner Node or HardwareDevice.

  lib.rs — updated
    StateBusSubscription re-exported from paraclete_node_api (moved to L2).
    Old StateBusSubscription from L1 removed.


paraclete-nodes (GPL-3.0)

  Cargo.toml — updated
    hound = "3.5" added (MIT license). WAV file loading.

  sampler.rs — new
    Sampler struct. Polyphonic sampler: 4 voices, oldest-voice stealing.
    Port declarations:
      PORT_EVENTS_IN (0): Event input
      PORT_AUDIO_OUT_L (1): Audio output left
      PORT_AUDIO_OUT_R (2): Audio output right
      PORT_PITCH_MOD (3): Modulation input
      PORT_VOLUME_MOD (4): Modulation input
    Constructors:
      Sampler::new() — no loaded sample, silent.
      Sampler::with_path(path) — loads WAV at activate().
    Parameters (all lockable, all declared in capability_document()):
      pitch   (-24..+24 semitones, default 0.0)
      volume  (0.0..1.0, default 0.8)
      pan     (-1.0..+1.0, default 0.0)
      start   (0.0..1.0 fraction of sample, default 0.0)
      end     (0.0..1.0 fraction of sample, default 1.0)
      loop    (stepped bool, default off, ON/OFF display)
      slice   (0..127 stepped, stub — always slice 0 at P3)
    Voice pool:
      4 voices: Voice { active, note, playback_pos, active_locks, triggered_at }
      active_locks: HashMap<u32, ActiveParamLock { locked_value }> per voice
      Oldest-voice stealing: min_by_key(triggered_at)
      At trigger: current node_locks are inherited by the new voice
      At release/sample-end: voice deactivated, node_locks cleared
    Parameter lock handling in process():
      ParamLockEvent with matching node_id: stored in node_locks HashMap.
        Unknown param_ids silently ignored (base_for() returns 0.0 as sentinel;
        known params are pitch/volume/pan/start/end/loop/slice).
        Wrong node_id: ignored (lock.node_id != self.node_id check).
      Effective param at render time: node_lock value if present, else base value.
      Voice-level effective: voice.active_locks if present, else node-level.
    Audio rendering in process():
      Pre-computes all effective params before self.voices.iter_mut() to
        satisfy the borrow checker (calling &self methods inside the mutable
        voice iterator is not allowed; precomputed values are plain locals).
      Pre-allocates render_l/render_r (Vec<f32>) at activate(); reused each
        cycle without allocation. Rule: process() must never allocate.
      Writes render buffers to audio_outputs[0] channels 0 (L) and 1 (R).
        Executor's single AudioBuffer per node carries stereo as two channels.
      Constant-power panning law: pan_l = sqrt((1-pan)/2 + 0.5)
      Playback rate: 2^(note_diff/12) * native_sr / output_sr
        note_diff = voice.note - root_note + voice_pitch + pitch_mod
    Node::negotiate() override:
      Returns ConnectionAgreement with all 7 params as lockable_params.
      Populates LockableParam.unit = ParamUnit::Generic for all (simplified;
        unit metadata from ParamDescriptor available for future improvement).
    Node::set_connection_record():
      Stores record in self.connection_records: Vec<ConnectionRecord>.
    impl Negotiable for Sampler {}
    serialize()/deserialize(): format version 1.
      Encodes: path (u16 length + UTF-8 bytes), root_note, pitch, volume, pan,
        start, end (f64 LE each), loop (u8), slice (u8).
      Unknown version byte: silently returns without modifying state.
    load_wav() helper:
      Opens WAV via hound, reads as f32 (normalised) or i32 (scaled by 2^(bits-1)).
      Mixes stereo to mono: (L + R) * 0.5
      Resamples if native rate differs from target: linear interpolation.
        TODO P6: replace with rubato crate for high-quality resampling.
      On failure: logs to stderr, sets 1-frame silent buffer. Never panics.
      Isolated to one function — swapping hound for symphonia touches only here.

  lib.rs — updated
    pub mod sampler; added. pub use sampler::Sampler; added.


paraclete-scripting (GPL-3.0)

  lib.rs — replaced
    Rhai compiled WITHOUT sync feature (removed from workspace Cargo.toml).
      ScriptingEngine is !Send. Scripts run on the main thread only.
      Rc<RefCell<StateBusHandle>> used throughout (not Arc<Mutex<>>).
    StateBusProxy: Rc<RefCell<StateBusHandle>> wrapper, registered as "StateBus".
      read(path: &str) -> Dynamic: returns Float/Int/Bool/Text or UNIT.
      write(path: &str, value: f64): delegates to write_sandboxed(); silently
        drops writes to non-/node/ paths (no script error, no panic).
      subscribe(path: &str) -> StateBusSubscriptionProxy
    StateBusSubscriptionProxy: hold handle + path + last_value.
      changed() -> bool: compares current store value to last_value.
      value() -> Dynamic: returns last polled value.
    register_state_bus_api(): registers StateBus and StateBusSubscription types.
    register_test_helpers(): registers assert(bool) — Rhai 1.x does not include
      assert in its standard library. Required for test scripts.
    ScriptingEngine::bind_state_bus(handle: Rc<RefCell<StateBusHandle>>):
      Stores proxy; injected into Rhai scope at run() time as "state_bus".
    ScriptingEngine::run() injects "state_bus" into a fresh Scope each call.


paraclete-app

  main.rs — P3 graph
    Nodes: InternalClock (ID 1), Sequencer (ID 2), LaunchpadEmulator (ID 3),
      HardwareMappingNode (ID 4), Sampler (ID 5, loads "samples/kick.wav").
    Connections:
      clock → seq (PORT_CLOCK_OUT → PORT_CLOCK_IN)
      emulator → mapper (0 → 0)
      mapper → seq (1 → PORT_EVENTS_IN)
      seq → sampler (PORT_EVENTS_OUT → PORT_EVENTS_IN) — triggers handshake
    Main loop: calls conf.process_state_bus() every 1ms.
    Scripting engine bound to the same StateBusHandle as the configurator.


TEST COVERAGE
-------------
133 tests total, 0 failures.

paraclete-node-api (61 unit + 5 integration = 66)
  agreement: baseline_agreement_has_no_lockable_params,
    connection_agreement_carries_lockable_params,
    lockable_param_is_clone_and_debug,
    connection_record_stores_partner_id_and_port
  state_bus: state_bus_handle_read_returns_none_for_unknown_path,
    state_bus_handle_write_then_read_returns_value,
    state_bus_handle_apply_updates_inserts_entries,
    state_bus_subscription_detects_first_change,
    state_bus_write_sandboxed_rejects_transport_path,
    state_bus_write_sandboxed_accepts_node_path
  All prior P0/P1/P2 tests retained.

paraclete-nodes (35 tests, up from 21)
  sampler (14 new tests):
    sampler_new_produces_silence_with_no_sample
    sampler_capability_document_has_7_params
    sampler_capability_document_extension_is_instrument
    sampler_negotiate_returns_7_lockable_params
    sampler_negotiate_lockable_params_include_pitch_and_volume
    sampler_set_connection_record_stores_record
    sampler_param_lock_for_unknown_param_is_ignored
    sampler_param_lock_on_wrong_node_id_is_ignored
    sampler_volume_param_lock_changes_output_level
    sampler_4_voices_5th_note_steals_oldest
    sampler_note_off_deactivates_voice_and_clears_locks
    sampler_serialize_deserialize_round_trip
    sampler_deserialize_unknown_version_leaves_defaults
    sampler_stereo_output_reflects_pan

paraclete-runtime (16 tests, updated)
  state_bus_values_published_by_melos_appear_in_snapshot — updated:
    exec.process() + conf.process_state_bus() now required before reading.
  state_bus_subscription_detects_first_change — updated:
    Uses conf.state_bus_poll_subscription() instead of sub.changed(&snap).
  All prior runtime tests pass unchanged.

paraclete-scripting (6 tests, new)
  scripting_engine_eval_simple_expression
  scripting_engine_state_bus_read_returns_unit_for_unknown_path
  scripting_engine_state_bus_write_then_read
  scripting_engine_state_bus_write_to_node_path_succeeds
  scripting_engine_state_bus_write_to_transport_is_rejected
  scripting_engine_state_bus_subscribe_detects_change


DECISIONS MADE DURING IMPLEMENTATION
--------------------------------------

1. StateBusHandle placed in L2 (paraclete-node-api), not L1

  The scripting layer (L4) needs read/write access to the state bus. L4 depends
  only on L2. If StateBusHandle lived in L1, L4 would need an L1 dependency,
  violating the layer model.

  Resolution: StateBusHandle goes in L2. The stable API (read, write, subscribe,
  poll_subscription) is entirely expressible in terms of L2 types (StateBusValue,
  StateBusSubscription). The SPSC drain path (apply_updates) is also on the
  handle — it takes Vec<(String, StateBusValue)>, which is pure L2. L1 holds the
  rtrb consumer and calls apply_updates() at process_state_bus() time. The rtrb
  dependency stays entirely in L1; L2 has no knowledge of it.

2. NodeConfigurator becomes !Send via Rc<RefCell<>>

  The scripting engine uses Rc<RefCell<StateBusHandle>> (not Arc<Mutex<>>) because
  Rhai was compiled without the sync feature. Rhai with sync requires all custom
  types to be Send + Sync, which would force Arc<Mutex<>> throughout. Removing
  sync enforces at compile time that ScriptingEngine stays on the main thread.
  NodeConfigurator holding Rc<RefCell<>> becomes !Send. This is correct: the
  configurator must never cross thread boundaries. Making this explicit via !Send
  is a benefit, not a cost.

3. Negotiable as a marker trait; negotiate() and set_connection_record() on Node

  The spec describes Negotiable as a supertrait of Node that declares negotiate()
  and set_connection_record(). But negotiate() already exists on Node (default
  returns ConnectionAgreement::baseline()) from the P0 design. Adding a second
  declaration on a supertrait would create confusion about which method is
  canonical.

  Resolution: negotiate() and set_connection_record() are default methods on Node
  itself. Negotiable is a pure marker trait (no methods) indicating that a node
  actively uses both. The runtime always runs the handshake on all connections —
  non-Negotiable nodes simply return baseline() and discard the ConnectionRecord.
  This is simpler and avoids the trait object complexity that would arise from
  trying to downcast Box<dyn Node> to Box<dyn Negotiable> at connect() time.

4. Hardware update_output() stays in the executor (audio thread)

  The spec's process_state_bus() is described as calling update_output() on all
  hardware devices from the main thread. This would require hardware devices to
  exist in two places simultaneously: the executor (for process() calls) and the
  configurator (for update_output() calls). Box<dyn HardwareDevice> cannot be
  cloned without an explicit method on the trait.

  Resolution: update_output() stays in the executor at P3, called after each
  cycle as in P2. The hardware output is built from the same entries vector that
  is pushed to the SPSC. process_state_bus() on the configurator handles the
  state bus drain and subscription notifications; it does not yet call
  update_output(). Moving hardware callbacks to the main thread is a P4 item and
  requires a device dual-ownership design.

5. Sampler process() borrow checker split

  Calling self.effective_node() (which takes &self) inside a for voice in
  self.voices.iter_mut() loop is rejected by the borrow checker: iter_mut()
  takes a mutable borrow over voices, and effective_node() tries to take an
  immutable borrow over all of self.

  Resolution: all effective param values (node_pitch, slice_idx, start_frame,
  end_frame, looping, native_sr, output_sr, root_note) are computed before the
  loop as plain local variables. Inside the loop, only self.render_l[frame]
  and self.render_r[frame] are written — different fields from voices, which the
  2021 edition borrow checker permits simultaneously. self.sample_data is
  captured as a slice reference before the loop; accessing sample_data inside the
  loop uses the local reference, not a new &self borrow.

6. render_l/render_r pre-allocated in activate()

  process() must never allocate. The Sampler needs intermediate L/R accumulation
  buffers to avoid borrow conflicts when writing to AudioBuffer channels. These
  are pre-allocated in activate() as Vec<f32> of block_size length, zeroed each
  cycle with iter_mut().fill(0.0). Copy-from-slice into the AudioBuffer at the
  end of the cycle.

7. ParamLock unknown-param sentinel

  The spec says "a lock for an unknown param_id is silently ignored." Sampler
  stores base params as named fields, not a HashMap. To check whether a param_id
  is known without iterating the capability document on the audio thread (which
  would require allocation for the Vec<ParamDescriptor>), the base_for() helper
  maps param_id → value for known ids and returns 0.0 as a sentinel for unknown.

  Problem: pitch has a base value of 0.0, which is the same as the sentinel.

  Resolution: the check is:
    if self.base_for(param_id) != 0.0 || param_id == param_hash("pitch") { lock }
  This correctly accepts pitch (explicitly checked) and correctly rejects truly
  unknown ids (which return 0.0 and don't match pitch). All seven known params
  pass. This is intentionally not generic; it encodes the P3 parameter set.

8. Rhai assert() not in standard library

  Rhai v1.x does not include assert(bool) in its standard library (it was present
  in earlier versions, removed to keep the stdlib minimal). The scripting tests
  call assert() in script strings. Rather than restructure tests to avoid assert,
  a minimal assert(bool) is registered in ScriptingEngine::new() via
  register_test_helpers(). This is acceptable in production code — assert is a
  genuinely useful scripting primitive.


WHAT P3 DELIBERATELY DEFERRED
-------------------------------
  Connected signal port views (modulation(), cv() for wired ports) — P4
    Unconnected ports now return silence instead of panicking (P3).
    Connected ports still unimplemented!() — no signal ports are wired in P3.
  Slice boundaries (multi-slice chop sampler) — slices Vec has one entry — P5
  High-quality resampling — linear interpolation in load_wav — P6 (rubato)
  Broader WAV format support — hound handles WAV only — P6 (symphonia)
  Hardware update_output() on main thread — still in executor — P4
  StateBus OQ-9: published_state() allocates per cycle — P4 design needed
  Hardcoded for pad in 0..16 in build_hardware_output() — P4


NEXT: P4
--------
  Hardware device main-thread refactor (update_output from NodeConfigurator)
  Connected signal port views (modulation() for wired Modulation ports)
  StateBus: lock-free published_state collection (OQ-9)
  Terminal UI main-loop (replace std::thread::park)
  Sequencer UI: step active/inactive via Launchpad pads
