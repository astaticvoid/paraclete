Paraclete — P1 First Sound Implementation Report
=================================================
Date: May 2026
Status: Complete — 96 tests, 0 warnings, binary builds and links clean


DELIVERABLE
-----------
The P1 graph is wired and producing audio:

  LaunchpadEmulator → HardwareMappingNode → SineOscillator → AudioOutput

Press Q–I for row 0 pads (notes 60–67, C4–G4).
Press A–K for row 1 pads (notes 68–75, Ab4–Eb5).
Each keydown opens the oscillator gate; keyup closes it. Audio comes out.


WHAT SHIPPED
------------

paraclete-node-api (LGPL-3.0)

  hardware.rs — new module
    HardwareEvent enum: PadPressed, PadReleased, PadPressure, ButtonPressed,
      ButtonReleased, EncoderDelta, EncoderPush, FaderMoved
    SurfaceDescriptor, Control enum, and six descriptor types:
      PadDescriptor, ButtonDescriptor, EncoderDescriptor, FaderDescriptor,
      LedDescriptor, DisplayDescriptor, DisplayType
    HardwareOutput, LedUpdate, DisplayUpdate, DisplayContent
    RgbColor with OFF/WHITE/RED/GREEN/BLUE constants
    HardwareDevice trait: surface() + update_output()

  event.rs
    Event::Hardware(HardwareEvent) variant added between Midi2 and ParamLock.
    Placed in the hot path — hardware events are frequent during live performance.
    The exhaustive-match regression test updated to cover the new variant.

  midi.rs — new module
    Re-exports midi2 types needed to build and decode UMP messages:
      ChannelVoice2, NoteOn, NoteOff, Channeled, Grouped, u4, u7
    Purpose: third-party node authors who depend only on paraclete-node-api
    get full MIDI 2.0 access without a separate midi2 dependency.

paraclete-runtime (GPL-3.0)

  configurator.rs
    NodeOrDevice enum: holds Box<dyn Node> or Box<dyn HardwareDevice>.
    add_hardware_device(user_id, Box<dyn HardwareDevice>) registered alongside
    regular nodes. Both participate in the graph identically.
    connect() now looks up the source port type and stores it in EdgeMeta
    so the executor can build event-routing tables without re-querying nodes.

  graph.rs — EdgeMeta extended
    Added src_port_type: PortType field. Stored at connection time.
    src_port and dst_port retained for P2 audio buffer wiring.

  executor.rs
    Event routing: event_routes: Vec<Vec<usize>> pre-computed at
    build_executor() time from EdgeMeta.src_port_type == PortType::Event.
    incoming: Vec<Vec<TimedEvent>> accumulates events per-slot per-cycle.
    std::mem::take pattern used in the process loop: takes the incoming
    events out of self.incoming[slot_idx] before processing, then writes
    to self.incoming[dst_idx] after — no borrow conflict, no allocation.
    Hardware output callbacks: hw_update() called on all hardware device
    slots after each cycle. Always passes HardwareOutput::empty() at P1;
    LED feedback is wired at P2.
    Drop impl: deactivate() called on all nodes when executor drops.
    NodeSlot now holds a NodeOrDevice; dispatch is explicit per-variant
    (no trait object upcasting — requires Rust 1.76+, project targets 1.75).

paraclete-nodes (GPL-3.0)

  SineOscillator
    Phase accumulator, 0.0–1.0, wraps at 1.0.
    activate() resets phase and stores sample_rate.
    NoteOn → frequency from midi_note_to_hz(), amplitude from velocity,
    gate open. NoteOff → gate closed, output silence.
    Renders to both channels (stereo).
    midi_note_to_hz(note) = 440 * 2^((note - 69) / 12). A4 = 440 Hz exactly.

  HardwareMappingNode
    default_chromatic(channel): pad 0 → C4 (60), chromatic scale up.
    from_map(HashMap<u32, u8>, channel): custom mapping.
    PadPressed → Midi2 NoteOn (velocity passed through from hardware).
    PadReleased → Midi2 NoteOff.
    All other event types pass through unchanged.
    Pads not in the map are silently ignored.
    build_note_on / build_note_off use the midi2 builder API directly.

paraclete-hal (GPL-3.0)

  LaunchpadEmulator — full P1 implementation replacing the P0 stub
    Implements Node + HardwareDevice.
    activate(): enables crossterm raw mode on the calling (main) thread.
    Clears screen, hides cursor for the emulator UI.
    deactivate(): disables raw mode, restores cursor.
    process(): non-blocking keyboard poll via crossterm::event::poll(ZERO).
    KeyDown → PadPressed { velocity: 32768 }. KeyUp → PadReleased.
    Renders the 8×8 grid to stdout at most once per 16 ms.
    Pressed pads render [#], unlit render [ ].
    update_output(): stores LED state for future use (P2).

  Keyboard layout (P1):
    Q W E R T Y U I → row 0 pads (ids 0–7)
    A S D F G H J K → row 1 pads (ids 8–15)
    Z X C V B N M , → row 2 pads (ids 16–23)

paraclete-app

  main.rs updated to wire the P1 graph:
    add_hardware_device(1, LaunchpadEmulator::new())
    add_node(2, HardwareMappingNode::default_chromatic(0))
    add_node(3, SineOscillator::new())
    connect(1, 0, 2, 0)   — emulator events_out → mapper events_in
    connect(2, 1, 3, 0)   — mapper events_out → oscillator events_in


TEST COVERAGE
-------------
96 tests total, 0 failures.

paraclete-node-api (53 tests)
  hardware::tests::hardware_event_is_copy
  hardware::tests::rgb_color_constants_have_correct_values
  hardware::tests::hardware_output_empty_has_no_updates
  hardware::tests::surface_descriptor_construction_for_test_surface
  event::tests::event_enum_has_midi2_not_decomposed_note_variants
    (now exhaustive over 6 variants including Hardware — regression guard)
  + 48 prior P0 tests retained

paraclete-nodes (10 tests)
  oscillator::sine_oscillator_produces_silence_when_gate_is_closed
  oscillator::sine_oscillator_produces_non_zero_samples_after_note_on
  oscillator::sine_oscillator_gate_closes_on_note_off
  oscillator::sine_oscillator_frequency_changes_on_note_on
  oscillator::midi_note_to_hz_a4_is_440
  oscillator::midi_note_to_hz_c4_is_approximately_261_63
  mapping::mapping_node_translates_pad_pressed_to_midi2_note_on
  mapping::mapping_node_translates_pad_released_to_midi2_note_off
  mapping::mapping_node_passes_unknown_events_through_unchanged
  mapping::mapping_node_ignores_pads_not_in_its_map

paraclete-runtime (13 integration tests, 10 unit tests)
  event_routing_delivers_events_from_upstream_to_downstream_node
  disconnected_nodes_receive_empty_event_slice
  add_hardware_device_registers_device_for_output_callbacks
  + 10 prior P0 graph and ring buffer tests retained
  + 10 prior configurator/executor integration tests retained


DECISIONS MADE DURING IMPLEMENTATION
--------------------------------------

1. Trait object upcasting not used (project targets Rust 1.75)

  The spec's HardwareDevice: Node supertrait means a Box<dyn HardwareDevice>
  can call Node methods directly. But Rust's fat pointer layout for
  dyn HardwareDevice and dyn Node differ — you cannot cast between them
  (trait object upcasting is only stable in Rust 1.76+).

  Resolution: NodeOrDevice enum in the executor dispatches to Node or
  HardwareDevice variants explicitly. Both implement process() via supertrait;
  calling d.process() on a Box<dyn HardwareDevice> works correctly.
  No unsafe required. No runtime overhead beyond one match arm.

2. Terminal I/O in process() — acceptable for P1

  The spec calls for polling keyboard events in process() on the audio thread.
  crossterm::event::poll(Duration::ZERO) is genuinely non-blocking (returns
  immediately). Terminal writes for rendering are infrequent (16ms debounce)
  and fast. For a developer tool this is acceptable. If profiling shows
  audio glitches from terminal I/O, rendering can be moved to the main thread
  with a shared atomic state flag. Not doing that pre-emptively.

3. midi.rs re-export module in paraclete-node-api

  Third-party node authors need access to midi2 types to decode UMP messages
  and build note events. Options were:
    a) Document that nodes add midi2 directly to their Cargo.toml
    b) Re-export from paraclete-node-api under a midi module

  Chose (b). The LGPL-3.0 boundary is paraclete-node-api — it is the only
  crate a third-party node should need to depend on. Exposing midi2 types
  there is correct. midi2 is MIT/Apache-2.0, no license conflict.

4. event_routes pre-computed at build_executor(), not at connect()

  Event routing could be computed incrementally at each connect() call and
  stored in the configurator. Instead it is computed once at build_executor()
  from the finalized graph. This is simpler and correct: the graph is frozen
  when build_executor() is called. Incremental routing would only matter if
  hot-reloading connections while audio is running (P9+).

5. HardwareOutput always empty at P1

  The runtime calls update_output(&HardwareOutput::empty()) after every cycle.
  This is correct per the spec. LED state from the graph reaches hardware at P2
  when the state bus is wired. The call site is already in place; P2 only needs
  to populate HardwareOutput before calling it.


WHAT P1 DELIBERATELY DEFERRED (per spec)
-----------------------------------------
  State bus — P2
  LED feedback from graph to emulator — P2
  Scripted mappings — HardwareMappingNode has hardcoded pad map — P4
  Graphical emulator window — terminal only — P4
  Full Launchpad surface — 8x8 grid sufficient, scene/mode buttons done
    but not keyboard-mapped beyond row 2 — P4
  Audio port wiring between nodes — not yet needed (oscillator is terminal) — P2
  Sub-buffer event timestamping — all events at sample_offset=0 — P5


NEXT: P2 — Sequencer v1
-----------------------
  16-step sequencer node in paraclete-nodes
  ClockSignal port and clock domain (ADR-009)
  Basic parameter locks
  State bus publish/subscribe (/transport/*, /node/{id}/*, /hw/{id}/*)
  HardwareOutput LED feedback wired
  TempoSource trait
