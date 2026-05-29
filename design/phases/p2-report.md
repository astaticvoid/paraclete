Paraclete — P2 Sequencer v1 Implementation Report
==================================================
Date: May 2026
Status: Complete — 103 tests, 0 warnings, binary builds clean


PREREQUISITE: NAMING TRANSITION
---------------------------------
The full vocabulary rename from naming-transition.md was applied before any P2
code was written. All identifiers now match the specified vocabulary. P2 code
uses the new names throughout. Summary of renames applied:

  trait Node           → trait Melos
  NodeConfigurator     → Oikonomos
  RuntimeNodeId        → MelosId
  PortDescriptor       → ThyraDescriptor
  PortType             → ThyraType
  PortDirection        → ThyraDirection
  PortName             → ThyraName
  fn ports()           → fn thyrai()
  fn process()         → fn anaphora()
  fn activate()        → fn epiclesis()
  fn deactivate()      → fn apolysis()
  fn serialize()       → fn paradidomi()
  fn deserialize()     → fn paralambanon()
  fn capability_document() → fn ekthesis()
  fn capabilities_changed() → fn metanoei()
  CapabilityDocument   → Ekthesis
  ConnectionAgreement  → Symbolon
  TransportInfo        → KairosInfo
  TransportEvent       → KairosEvent
  TransportFlags       → KairosFlags
  ExtendedEventSlab    → Kenoma
  ExtendedEventRef     → Semeion
  fn new() (no-arg)    → fn protokletos() on all Melos implementations

Template traits renamed: SignalNodeTemplate → SignalMelos, etc.
Oikonomos.add_node() → add_melos(). NodeOrDevice → MelosOrDevice (internal).


DELIVERABLE
-----------
P2 graph:

  ProeistosNode → Anamnesis → SineOscillator → AudioOutput
                   ↑
  LaunchpadEmulator → HardwareMappingNode

Startup behavior: ProeistosNode emits the first KairosEvent with global_start,
Anamnesis begins playback at step 0. Pattern fires C-E-G-C across steps 0,2,4,8.
Launchpad keyboard input routes to Anamnesis events_in for future recording.
Current step pad lights green via LED feedback.


WHAT SHIPPED
------------

paraclete-node-api (LGPL-3.0)

  proeistos.rs — new
    trait Proeistos: Melos — declares domain_id() and priority()
    KairosPriority enum: SubDomain < Internal < ExternalHardware < AbletonLink
      < DawHost; derives PartialOrd for priority federation

  koinonia.rs — new
    KoinoniaValue enum: Float(f64), Int(i64), Bool(bool), Text(String)
    trait StatePublisher: Melos — semantic marker; implementation via
      Melos::published_state() default method

  transport.rs (KairosFlags) — updated
    Three new fields added: global_stop, global_start, sync_pulse
    All default false. Downstream melos respond to these for transport control.

  agreement.rs (Symbolon) — updated
    New field: initial_kairos: Option<KairosInfo>
    baseline() initializes it to None.
    A Proeistos node sets this when negotiating so downstream melos can
    initialize their internal position without waiting for the first KairosEvent.

  node.rs (Melos trait) — updated
    Two new default methods:
      fn set_melos_id(&mut self, _id: u32) {} — called by Oikonomos at
        registration so the melos knows its own ID for Koinonia path construction
      fn published_state(&self) -> Vec<(String, KoinoniaValue)> { vec![] } —
        called by the executor after each anaphora cycle

  lib.rs — updated
    Re-exports: KairosPriority, Proeistos, KoinoniaValue, StatePublisher


paraclete-nodes (GPL-3.0)

  proeistos.rs — ProeistosNode
    Implements Melos + Proeistos. Internal clock at 120 BPM.
    Sub-sample tick accumulator: ticks_per_sample = (bpm/60) * TICKS_PER_BEAT
      / sample_rate. For 120 BPM at 44100 Hz: ~0.0435 ticks/sample.
    Emits one KairosEvent per tick at correct sample_offset.
    First tick has global_start: true — this is the trigger that starts
      Anamnesis playback. Without this, Anamnesis stays silent (see Bug below).
    Emits sync_pulse: true every bar (beat==0 && tick==0).
    THYRA_BPM_MOD: Modulation input for BPM offset. Deferred to P9 when
      signal ports are wired. Currently unused.
    published_state(): publishes /koinonia/transport/bpm, bar, beat, tick,
      playing to the Koinonia.

  anamnesis.rs — Anamnesis
    Implements Melos + StatePublisher. 16-step sequencer.
    Step struct: active, note (u8), velocity (u16 MIDI 2.0), length (f32
      fraction of ticks_per_step), param_locks: Vec<StepParamLock>
    StepParamLock: melos_id, param_id, value — per-step parameter override.
    Responds to Event::Transport(KairosEvent):
      - global_start: sets playing=true, resets to step 0
      - global_stop: sets playing=false, closes gate (emits NoteOff if needed)
      - sync_pulse: snaps position from bar/beat/tick fields
      - playing tick: checks gate-off, checks step boundary, advances
    Step boundary logic: when step_tick >= ticks_per_step, advance to next step.
      Active steps emit NoteOn + any param locks. Gate closes after
      step.length * ticks_per_step ticks.
    ticks_per_step: TICKS_PER_BEAT / 4 = 240 (16th note at 960 ppq)
    paradidomi() / paralambanon(): binary encoding, format version 1.
      Persists pattern_length, ticks_per_step, and all step data including
      param locks. Unknown format versions are ignored gracefully.
    published_state(): /koinonia/melos/{id}/state/current_step,
      pattern_length, playing.
    melos_id: set by Oikonomos at registration via set_melos_id().


paraclete-runtime (GPL-3.0)

  koinonia.rs — new
    KoinoniaSnapshot = Arc<RwLock<HashMap<String, KoinoniaValue>>>
    KoinoniaSubscription: polls a path for changes since last check.
      changed(&snap) returns Some(&value) if changed, None otherwise.

  configurator.rs (Oikonomos) — updated
    koinonia_snapshot: KoinoniaSnapshot field (shared with executor).
    proeistos_domains: Vec<(u32, u32)> — (user_id, domain_id) pairs.
    next_domain_id: u32 — monotonic counter.
    register() calls set_melos_id() on each melos before epiclesis().
    add_proeistos(user_id, Box<dyn Melos>) -> (MelosId, u32): registers
      the melos as both a graph node and a clock domain provider.
    koinonia_read(path) -> Option<KoinoniaValue>: reads from snapshot.
    koinonia_subscribe(path) -> KoinoniaSubscription: subscription handle.
    koinonia_snapshot_ref(): exposes Arc for tests.
    build_executor(): passes koinonia_snapshot Arc clone to executor.
    Event routing: ThyraType::Clock edges now route like ThyraType::Event
      (both carry Event::Transport in the event system).

  executor.rs (NodeExecutor) — updated
    koinonia_snapshot: KoinoniaSnapshot field.
    After all anaphora() calls: collect published_state() from all melos
      into a local HashMap, write to RwLock snapshot (brief write lock),
      then build HardwareOutput from local map.
    LED feedback: scans local koinonia for /state/current_step keys.
      Current step → RgbColor::GREEN, all others → RgbColor::OFF.
      Calls update_output() on all hardware device slots with the result.
    MelosSlot.published_state() dispatches to Node or Device variant.


paraclete-app

  main.rs — P2 graph wiring
    ProeistosNode → Anamnesis (clock_out → clock_in)
    LaunchpadEmulator → HardwareMappingNode → Anamnesis (events_in)
    Anamnesis → SineOscillator (events_out → events_in)
    Pre-loaded pattern: steps 0,2,4,8 with notes C4, E4, G4, C5 (60,64,67,72)
    No state bus or terminal UI loop yet — main thread parks after audio starts.


TEST COVERAGE
-------------
103 tests total, 0 failures.

paraclete-node-api (51 unit + 5 integration = 56)
  proeistos::tests::kairos_priority_ordering_daw_host_is_highest
  koinonia::tests::koinonia_value_is_clone
  koinonia::tests::koinonia_value_variants_are_distinct
  + all prior P0/P1 tests retained

paraclete-nodes (21 tests)
  proeistos: protokletos_has_bpm_120, emits_kairos_events_each_cycle,
    emits_sync_pulse_at_bar_start, domain_id_and_priority,
    published_state_includes_bpm
  anamnesis: protokletos_has_16_empty_steps, does_not_emit_when_not_playing,
    advances_step_and_emits_note_on_at_boundary, responds_to_global_stop,
    paradidomi_paralambanon_round_trip, published_state_has_current_step

paraclete-runtime (16 tests)
  koinonia_values_published_by_melos_appear_in_snapshot
  koinonia_subscription_detects_first_change
  add_proeistos_returns_domain_id_and_node_participates_in_graph
  + all prior P0/P1 runtime tests retained


DECISIONS MADE DURING IMPLEMENTATION
--------------------------------------

1. global_start required for Anamnesis to begin playback (Bug found and fixed)

  Anamnesis starts with playing: false. It only begins on a global_start event.
  ProeistosNode was initially emitting KairosEvents with playing: true in the
  flags but never global_start: true. Result: the sequencer silently ignored
  all ticks and never fired a note.

  Resolution: ProeistosNode has a first_tick: bool field (true at construction).
  The very first KairosEvent emitted has global_start: true. This is cleared
  immediately — all subsequent ticks have global_start: false. The Anamnesis
  receives this on the first audio cycle and starts playing.

  This is architecturally correct: the Proeistos node is the one who presides.
  It signals when the transport starts.

2. ThyraType::Clock routed identically to ThyraType::Event

  KairosEvents are carried as Event::Transport in the event stream. The Clock
  thyra type signals the semantic (timing authority) not the transport mechanism
  (which is the same event routing). In configurator.rs:

    matches!(src_port_type, ThyraType::Event | ThyraType::Clock)

  This routes Clock-typed edges through the same SPSC-style incoming Vec as
  Event edges. No separate routing mechanism needed. Both carry TimedEvent.

3. Koinonia shared via Arc<RwLock<HashMap>> — explicitly provisional

  The spec says "Koinonia is never touched on the audio thread." The chosen
  implementation holds a brief write lock at the END of each executor cycle,
  after all anaphora() calls complete. The lock is held only during a HashMap
  bulk insert — measured in microseconds. The main thread reads hold the read
  lock only for the duration of a HashMap lookup.

  This is acceptable for P2. The path to the correct architecture (lock-free
  SPSC from audio to main) is clear and non-breaking: replace the RwLock write
  with a ring buffer send, add a drain step on the main thread. Target: P4.

4. LED feedback builds HardwareOutput from local koinonia, not shared snapshot

  After published_state() is collected into a local HashMap, LED feedback is
  built from that local map (no second lock acquisition). The shared snapshot
  write and the LED construction use the same data but the snapshot write
  is separate. This avoids holding the write lock during LED construction.

5. Koinonia path scanning approach for LED feedback

  The executor scans koinonia keys ending in /state/current_step to find any
  Anamnesis publishing its step. This is deliberately pattern-based rather than
  requiring the executor to know specific melos IDs. Multiple Anamnesis
  instances would each light a different set of pads. Correct and extensible.

6. StatePublisher as a marker trait, published_state() on Melos

  Implementing published_state() as a default method on Melos (not on a
  separate trait object) avoids the Rust trait object upcasting limitation
  (stable only in 1.76+, project targets 1.75). All melos can be called
  polymorphically as &dyn Melos while still dispatching to the correct
  published_state() via dynamic dispatch. StatePublisher: Melos exists as a
  semantic marker — a node implementing it commits to overriding the method.


WHAT P2 DELIBERATELY DEFERRED
-------------------------------
  Real-time recording — Anamnesis receives Midi2 events but discards them — P5
  Conditional trigs — P5
  Micro-timing — P5
  Pattern length > 16 — P5
  BPM modulation via signal port — THYRA_BPM_MOD defined, poll skipped — P9
  Clock federation priority enforcement — add_proeistos() assigns domain IDs,
    priority comparison not yet enforced between multiple Proeistos nodes — P7
  State bus → Rhai scripting access — P4
  Terminal UI loop on main thread (run_terminal_ui) — main thread parks — P4
  Koinonia → lock-free SPSC channel — P4


NEXT: P3 — First Instrument (Sampler)
--------------------------------------
  Sampler node in paraclete-nodes
  Slice mode: chop sample addressable by MIDI note
  Full parameter lock protocol: Anamnesis param locks reach Sampler
  State bus scripting access: Rhai can read/write Koinonia paths
  Negotiable trait: L3 melos participate in connection handshake
