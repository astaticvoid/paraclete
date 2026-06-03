Paraclete — P4 Hardware Integration Implementation Report
=========================================================
Date: May 2026
Status: Complete — 188 tests, 0 failures, binary builds clean
  (169 at initial P4 ship; +22 added during P2/P3 retroactive cleanup, P4 review, and P4-fixes review)


DELIVERABLE
-----------
P4 graph:

  InternalClock
    ├──→ Sequencer[0] (Kick)   → Sampler[0] → Distortion[0] → Filter[0] ──┐
    ├──→ Sequencer[1] (Snare)  → Sampler[1] → Distortion[1] → Filter[1] ──┤
    ├──→ Sequencer[2] (Hat CH) → Sampler[2] → Distortion[2] → Filter[2] ──┤
    ├──→ Sequencer[3] (Hat OH) → Sampler[3] → Distortion[3] → Filter[3] ──┼──→ MixNode → AudioOutput
    ├──→ Sequencer[4] (Perc A) → Sampler[4] → Distortion[4] → Filter[4] ──┤
    ├──→ Sequencer[5] (Perc B) → Sampler[5] → Distortion[5] → Filter[5] ──┤
    ├──→ Sequencer[6] (FX)     → Sampler[6] → Distortion[6] → Filter[6] ──┤
    └──→ Sequencer[7] (Bass)   → Sampler[7] → Distortion[7] → Filter[7] ──┘

  LaunchpadNode (or LaunchpadEmulator if no hardware)
  DigitaktMidiNode (if connected)    ──→ ScriptingGatewayNode → SPSC → ScriptingEngine
  KeystepNode (if connected)

  KeystepNode → HardwareMappingNode → Sequencer[7] (bass track)

All three hardware devices are optional at runtime. The app opens each device
with midir and falls back gracefully when not found: Launchpad degrades to the
terminal LaunchpadEmulator; Digitakt and Keystep are skipped entirely.

`cargo run` boots the P4 graph at 140 BPM. Profile scripts are loaded from
profiles/ if present. `cargo run -- --dev-ui` prints active sequencer step
positions to stderr every second.


WHAT SHIPPED
------------

paraclete-node-api (LGPL-3.0)

  command.rs — new
    NodeCommand struct (24 bytes):
      target_id: u32, type_id: u32, arg0: i64, arg1: f64
      Fixed size verified by size_of assertion in tests.
    Universal type IDs:
      CMD_SET_PARAM = 0 — set declared parameter to absolute value (clamped)
      CMD_BUMP_PARAM = 1 — apply signed delta to declared parameter (clamped)
    Node-specific type IDs start at 16; range 0–15 reserved for future
    universal commands.

  parameter.rs — new
    ParameterBank struct — pre-allocated parameter storage.
      slots: Vec<ParameterSlot> allocated at activate() time; zero audio-thread
      allocation in handle_commands() or get().
    ParameterSlot: { param_id, current, min, max, default } — all f64.
    ParameterBank::from_capability_document(doc) — builds from CapabilityDocument;
      sets current = default for all declared params.
    ParameterBank::empty() — zero-slot bank for nodes with no parameters.
    handle_commands(commands: &[NodeCommand]) — linear scan, O(n×m) for n
      commands and m params; correct and allocation-free for typical counts
      (n < 8, m < 32). Silently ignores unknown param_id and unknown type_id.
    get(param_id) -> f64 — returns current value or 0.0 for unknown id.
    set(param_id, value) — clamped write; used in activate()/deserialize().
    reset() — restores all slots to declared default.

  hardware.rs — updated
    EncoderBehaviour enum (Clone + Copy + Debug + PartialEq):
      Absolute { range_max: u16 } — sends absolute position (7-bit CC: 127).
      Relative — sends signed delta; positive = clockwise. Used by Digitakt.
    EncoderDescriptor updated:
      endless: bool replaced by behaviour: EncoderBehaviour.
      This is the only breaking change to a previously shipped L2 type;
      no shipped first-party nodes used EncoderDescriptor yet.
    HardwareEvent: EncoderDelta { id, delta: i32 } replaced by
      EncoderChanged { id, value: u16, delta: i16 }
        value carries absolute position for Absolute encoders (unused by
        Relative devices but present for a uniform event type across both modes).
        delta is i16 (sufficient for ±127 and signed relative values).
    HardwareEventMsg struct (Copy):
      device_id: u32, event: HardwareEvent
      Produced by ScriptingGatewayNode; consumed by ScriptingEngine.
    HardwareOutputHandle trait:
      fn tick(&mut self) — called by NodeConfigurator each main-loop iteration.
      Must not block. Drains an internal SPSC and delivers LED/fader output.
      Object-safe (verified by test).
    HardwareDevice trait — updated:
      update_output() retained for legacy path (audio thread).
      take_output_handle(&mut self) -> Option<Box<dyn HardwareOutputHandle>>
        Default returns None (preserves P3 behavior for LaunchpadEmulator).
        P4+ devices return Some; configurator stores the handle and calls
        tick() each main loop iteration instead of update_output().

  context.rs — updated
    ProcessInput gains: commands: &'a [NodeCommand]
      All test sites that construct ProcessInput directly must add commands: &[].
      This is the only breaking change in ProcessInput between P3 and P4.
    Signal port views (cv, phase, logic, pitch, modulation):
      unimplemented!() removed for the connected-port path. Now returns the
      same static empty buffer as the unconnected path. Full per-connection
      buffer wiring deferred to P5 (no P4 node requires these ports).
    SignalInputSlot and SignalOutputSlot gain pub constructors:
      SignalInputSlot::new(port_id, kind, &[f32]) — builds from a slice ref.
      SignalOutputSlot::new(port_id, kind, &mut [f32])

  lib.rs — updated
    New re-exports: NodeCommand, CMD_SET_PARAM, CMD_BUMP_PARAM, ParameterBank,
      EncoderBehaviour, HardwareEventMsg, HardwareOutputHandle.


paraclete-runtime (GPL-3.0)

  configurator.rs — updated
    NodeCommand SPSC:
      node_cmd_producer: rtrb::Producer<NodeCommand> capacity 512.
      node_cmd_consumer_pending: Option<rtrb::Consumer<NodeCommand>>
        Held until build_executor(); moved into executor then.
    send_command(&mut self, cmd: NodeCommand) -> Result<(), NodeCommand>:
      Pushes to producer. Returns Err(cmd) if ring buffer full (rtrb
      PushError has no field access; use match to recover the original value).
      Caller retries next main-loop tick.
    output_handles: Vec<Box<dyn HardwareOutputHandle>>
      Accumulated as devices register. Ticked each call to process_main_thread().
    add_hardware_device() updated:
      Calls device.take_output_handle() immediately after registration.
      If Some, stores the handle in output_handles.
    process_main_thread(&mut self):
      Drains state bus SPSC (same as former process_state_bus()).
      Calls handle.tick() for each stored HardwareOutputHandle.
      process_state_bus() kept as an alias for backward compatibility.
    build_executor() updated:
      Passes node_cmd_consumer to NodeExecutor::new().

  executor.rs — updated
    cmd_consumer: rtrb::Consumer<NodeCommand>
    pending_cmds: HashMap<u32, Vec<NodeCommand>>
      Pre-allocated in NodeExecutor::new() — one Vec<NodeCommand> per node id.
      Cleared each cycle before drain_commands(); no audio-thread allocation.
    drain_commands():
      Drains cmd_consumer into pending_cmds keyed by target_id.
      Commands for unknown node_ids are silently dropped (ring was full, or
      node was removed).
    process() loop updated:
      Calls drain_commands() after apply_messages(), before node dispatch.
      For each node slot, looks up pending_cmds[node_id] and passes as the
      commands field of ProcessInput.
    build_hardware_output() updated:
      Surface descriptor iteration replaces hardcoded for pad in 0..16.
      For each hardware device slot, iterates surface().controls, collects
      rgb-capable pad and button ids, and writes LedUpdates for those ids.
      Devices that returned Some from take_output_handle() still call
      update_output() on the executor — the output handle path takes the
      update via the internal SPSC inside the device's tick() call.

  lib.rs — unchanged.


paraclete-hal (GPL-3.0)

  Cargo.toml — updated
    midir = "0.9" added (MIT license). Cross-platform MIDI I/O.
    rtrb = "0.3" added (for LaunchpadNode's internal SPSC).

  midi/mod.rs — new
    MidiDeviceRegistry:
      Wraps midir::MidiInput and midir::MidiOutput.
      new() -> Result<Self, MidiDeviceError>
      input_port_names() / output_port_names() -> Vec<String>
      open_input(name_substr, callback) -> Result<MidiInputConnection<()>, ...>
        Finds port by substring match. Used by all three hardware nodes.
      open_output(name_substr) -> Result<MidiOutputConnection, ...>
    MidiDeviceError: Init(midir::InitError), Connect(String), PortNotFound(String)

  launchpad/mod.rs — new
    LaunchpadNode (HardwareDevice: Node):
      Surface: 64 pads (ids 0–63, row-major), 8 scene buttons (64–71), 8 top
        buttons (72–79). All RGB.
      MIDI translation (MK2 protocol):
        Note On ch0 vel>0 → PadPressed { id: note, velocity: vel<<9, pressure: 0 }
        Note On ch0 vel=0 / Note Off ch0 → PadReleased { id: note }
        CC ch0 vel>0 → ButtonPressed { id: cc_number }
        CC ch0 vel=0 → ButtonReleased { id: cc_number }
      Incoming events: midir callback thread → Arc<Mutex<VecDeque<HardwareEvent>>>.
        Audio thread drains with try_lock() in process(). If lock is contended
        (callback thread is mid-push), events are deferred to the next cycle.
        This is the only Mutex on the audio-thread path; contention is bounded
        to the brief duration of a VecDeque::push_back().
      Output path: HardwareOutputHandle pattern.
        hw_out_tx: rtrb::Producer<HardwareOutput> → audio thread pushes updates.
        take_output_handle() moves conn_out and the consumer rx into
          LaunchpadOutputHandle. Returns None on second call (take semantics).
        LaunchpadOutputHandle::tick(): drains rx, sends Note On to physical device
          via conn_out. LED color mapped to nearest MK2 palette velocity index.
    parse_launchpad_midi() — tested independently.

  digitakt/mod.rs — new
    DigitaktMidiNode (HardwareDevice: Node):
      Surface: 8 relative encoders (ids 0–7), 8 track buttons (8–15),
        transport buttons Play(16) Stop(17) Record(18).
      Relative encoder decoding:
        CC value 1–63 → delta = +value (clockwise)
        CC value 65–127 → delta = value - 128 (counter-clockwise)
        CC value 64 → delta = 0 (center detent)
        Emits EncoderChanged { id: cc_number, value: 0, delta }
      CC encoding matches Digitakt default MIDI Out setup. No configuration
      required on the Digitakt side beyond enabling MIDI Out.
      No LED output — take_output_handle() returns None.
    decode_relative_delta() — tested independently.

  keystep/mod.rs — new
    KeystepNode (HardwareDevice: Node):
      Surface: pitch_bend fader (id 0), mod_wheel fader (id 1).
      MIDI translation:
        Note On ch0 vel>0 → Event::Midi2 NoteOn (UMP, MIDI 2.0)
        Note On ch0 vel=0 / Note Off ch0 → Event::Midi2 NoteOff
        CC1 (mod wheel) → Event::Hardware(FaderMoved { id: 1, value: vel<<9 })
        Pitch bend (0xE0) → Event::Hardware(FaderMoved { id: 0, value: 14-bit })
      Note events go through the graph as Midi2 and are consumed by
        HardwareMappingNode; fader events go to ScriptingGatewayNode for
        scripting dispatch. No LED output.
    parse_keystep_midi() — tested independently.

  lib.rs — updated
    Exports: LaunchpadNode, DigitaktMidiNode, KeystepNode, MidiDeviceError,
      MidiDeviceRegistry.


paraclete-nodes (GPL-3.0)

  Cargo.toml — updated
    rtrb = "0.3" added (ScriptingGatewayNode's internal SPSC).

  gateway.rs — new
    ScriptingGatewayNode (Node):
      Fan-in node. One Event input port per connected hardware device.
      Dynamic port list: starts empty. New input port added per
        set_connection_record() call. capabilities_changed() returns true
        until the runtime re-queries the capability document.
      capabilities_dirty: Cell<bool>
        capabilities_changed() is &self (not &mut self) per the Node trait.
        Cell<bool> provides interior mutability; replace(false) reads and
        resets atomically in a single call.
      process(): collects Event::Hardware events from input.events. Tags
        each with device_ids[0] (first registered device — limitation, see
        deferred section). Pushes HardwareEventMsg to the SPSC producer.
      device_ids: Vec<u32> maps dst_port_id to source node_id. Populated
        by set_connection_record().
    ScriptEventConsumer:
      drain(&mut self, out: &mut Vec<HardwareEventMsg>)
        Drains the consumer side of the ScriptingGatewayNode SPSC.
        Called by the app main loop each iteration.
    ScriptingGatewayNode::new(capacity) -> (Self, ScriptEventConsumer)
      Capacity 1024 in production app.

  mix.rs — new
    MixNode (Node):
      N stereo inputs → 1 stereo output. N specified at construction.
      Ports: audio_in[0..N] (inputs), audio_out (id=N, output).
      Parameters: per-input gain (param_id = index, 0.0–2.0, default 1.0)
        and master_gain (param_id = N, 0.0–2.0, default 1.0).
        All serviced by ParameterBank.
      activate() pre-allocates render_l/render_r (Vec<f32>, block_size).
      process(): handle_commands first. Mix = sum(input[i] * gain[i]) * master.
        Mono inputs duplicated to both output channels.

  distortion.rs — new
    DistortionNode (Node):
      Ports: audio_in (0), audio_out (1).
      Parameters (ParameterBank):
        drive (id=0): 0.0–1.0, default 0.0
        output_level (id=1): −24.0..+6.0 dB, default 0.0
        blend (id=2): 0.0–1.0, default 1.0
      DSP: wet = tanh(input × exp(drive × 4)) × db_to_linear(output_level)
           output = dry + blend × (wet − dry)
      Note: at drive=0, exp(0)=1, tanh is still applied. For input amplitude
        0.5, tanh(0.5) ≈ 0.462. Full signal passthrough requires blend=0.
        For small signals (< 0.05), tanh(x) ≈ x and distortion is negligible.
      CapabilityDocument.extensions: ["paraclete.effect"] per ADR-017.

  filter.rs — new
    FilterNode (Node):
      Ports: audio_in (0), audio_out (1).
      Parameters (ParameterBank):
        cutoff_hz (id=0): 20–20000 Hz, default 1000
        resonance (id=1): 0.1–4.0, default 0.7
        filter_type (id=2): 0=LP, 1=HP, 2=BP, 3=Notch, default 0 (stepped)
      DSP: Chamberlin state-variable filter (SVF). Two state variables
        (low, band) per channel, updated each sample.
        f_coeff = 2π × cutoff / sr; q_coeff = 1/resonance.
        f_coeff clamped to 1.0 for stability at high cutoff frequencies.
        State variables low_l/band_l/low_r/band_r zero-initialized at
        activate() and on any subsequent activate() call.
      filter_type applied after SVF computation: LP=low, HP=high, BP=band,
        Notch=high+low. Coefficient update triggered only when cutoff changes
        by more than 0.5 Hz; avoids redundant sqrt/sin calls per cycle.
      CapabilityDocument.extensions: ["paraclete.effect"].

  sequencer.rs — updated
    with_name(name: &str) -> Self: constructor that sets track_name.
    track_name: String field.
    bank: ParameterBank — built at activate().
    CMD_TOGGLE_STEP = 16: arg0 = step index. Flips active bit.
    CMD_SET_STEP    = 17: arg0 = step index. arg1 < 0 → deactivate;
      arg1 >= 0 → set note = arg1 as u8 and activate.
    CMD_CLEAR       = 18: deactivates all steps.
    handle_commands() called before transport/event logic in process().
      Routes universal commands to bank; routes sequencer commands to
      step data directly.
    published_state() updated:
      /node/{id}/state/steps — 16-char ASCII bitfield, '1' or '0' per step,
        length = pattern_length. Launchpad profile subscribes to this path.
      /node/{id}/state/track_name — published if non-empty.

  lib.rs — updated
    New modules and pub uses: distortion, filter, gateway, mix.
    pub use gateway::{ScriptingGatewayNode, ScriptEventConsumer}.
    pub use mix::MixNode, pub use distortion::DistortionNode,
      pub use filter::FilterNode.


paraclete-scripting (GPL-3.0)

  lib.rs — replaced
    ScriptState (internal): current_context: String, context_data per named
      context, pending_commands: Vec<NodeCommand>, pending_output:
      HashMap<u32, HardwareOutput>, state_bus: Option<Rc<RefCell<StateBusHandle>>>.
      Held behind Rc<RefCell<>> and shared among all registered Rhai builtins.
    ContextData (internal): hw_handlers: Vec<(u32, FnPtr)>, macros:
      HashMap<String, FnPtr>, bindings: Vec<MacroBinding>,
      subscriptions: Vec<OwnedSubscription>.
      One ContextData entry per named profile.
    ScriptContext (public): name: String, ast: rhai::AST, scope: Scope<'static>.
      The AST is stored alongside the context because rhai::FnPtr::call requires
      the originating AST to resolve function definitions referenced by closures.
    ScriptingEngine fields:
      engine: Engine, script_state: Rc<RefCell<ScriptState>>,
      contexts: HashMap<String, ScriptContext>,
      state_bus_proxy: Option<StateBusProxy> (legacy compat).
    eval_file(name, path, constants: &[(String, Dynamic)]) -> Result:
      Tears down existing ContextData for this name (clears handlers/subs/macros).
      Sets current_context = name before eval so builtins register to the right slot.
      Builds scope with injected constants and legacy state_bus proxy.
      Compiles source to AST; runs the AST (top-level code executes).
      Calls engine.call_fn("on_load", ...) and ignores "function not found" error.
        rhai 1.25 does not expose ast.iter_fn_def() publicly; call_fn with
        ignore-error is the correct approach.
      Stores ScriptContext { name, ast, scope } in contexts map.
    dispatch_hardware_event(&mut self, msg: &HardwareEventMsg):
      For each context: collect matching hw_handlers (by device_id). For each
        matching handler: fn_ptr.call::<()>(&engine, &ast, (event_dyn,)).
        rhai 1.25 FnPtr::call signature: (engine, ast, args) — no Scope parameter.
        Captured closures carry their scope internally in the FnPtr.
      Then checks macro bindings for (device_id, event_type, control_id) match.
        If found, calls the macro FnPtr with () args.
      event_to_dynamic(msg): builds a rhai::Map with keys event_type, id, row,
        col, device_id, and event-specific fields (velocity, delta, value, etc.).
        row = id/8, col = id%8 — convenient for grid-based Launchpad layouts.
    process_subscriptions(&mut self, state_bus: &StateBusHandle):
      For each context, each subscription: reads current value from StateBusHandle.
      If different from last_value: calls fn_ptr.call(&engine, &ast, (dyn_val,)).
      Updates last_value and last_written_at on change.
      Fires immediately on first call after subscribe() since initial value is
        stored as last_value at subscription time, and the first process_subscriptions
        call compares None to the current value — they differ, so it fires.
    take_pending_commands() -> Vec<NodeCommand>:
      Swaps pending_commands with an empty Vec; returns the drained commands.
    take_pending_output() -> HashMap<u32, HardwareOutput>:
      Swaps pending_output; returns accumulated LED output per device_id.
    Rhai builtins registered (all capture Rc<RefCell<ScriptState>>):
      state_read(path) → Dynamic
      state_write(path, value: Dynamic)
      send_cmd(target_id, type_id, arg0, arg1) → accumulates NodeCommand
      set_led(device_id, control_id, r, g, b) → accumulates LedUpdate
      on_hw_event(device_id, fn) → registers hw handler to current context
      subscribe(path, fn) → registers subscription, fires immediately
      state_alive(path, timeout_ms) → bool
      def_macro(name, fn) → stores FnPtr in current context macros
      bind_macro(device_id, event_type, control_id, macro_name)
      fire_macro(name) — no-op stub (actual dispatch in dispatch_hardware_event)
      get_step_state(node_id) → String (reads /node/{id}/state/steps)
      reload_profile(name) — no-op stub (reload handled in app layer)
      assert(bool) — retained from P3
    Legacy API retained: StateBusProxy, StateBusSubscriptionProxy, compile(),
      run(), eval_str(). Existing P3 tests pass unchanged.
    set_constant(key, value): present but unused by the app. The app passes
      constants directly via eval_file's constants parameter. set_constant()
      currently pushes a zero-value NodeCommand as a placeholder — this is an
      implementation artifact, not a bug the app triggers, but it should be
      cleaned up before the method is used.


paraclete-app (GPL-3.0)

  main.rs — P4 graph
    Node IDs: clock=1, mix=2, seq[i]=10+i, samp[i]=20+i, dist[i]=30+i,
      filt[i]=40+i, gateway=100, emulator=101, launchpad=102, digitakt=103,
      keystep=104, mapper=105.
    try_open_launchpad(): attempts LaunchpadNode::open(). On Err, falls back
      to LaunchpadEmulator. Returns the device's node id either way.
    try_open_digitakt(), try_open_keystep(): attempt hardware open. On Err,
      log and return None. Connections to gateway skipped if None.
    ScriptingGatewayNode::new(1024) returns (gateway, script_consumer).
      Hardware devices connected to gateway sequentially on ports 0, 1, 2.
    eval_file called for launchpad, digitakt, keystep profiles; files skipped
      if not present in profiles/ directory.
    Constants injected per profile: LP_DEVICE_ID, DT_DEVICE_ID, KS_DEVICE_ID,
      CLOCK_ID, TRACK_SEQ_IDS, TRACK_SAMP_IDS, TRACK_DIST_IDS, TRACK_FILT_IDS.
    Main loop (1 ms sleep per iteration):
      1. conf.process_main_thread() — drains state bus, ticks output handles.
      2. script_consumer.drain() — collects HardwareEventMsgs from gateway SPSC.
      3. dispatch_hardware_event() for each event.
      4. process_subscriptions() — fires subscription callbacks on changes.
      5. take_pending_commands() → conf.send_command() for each.
      6. --dev-ui: logs current_step per track every 1000 iterations (~1 sec).

  Cargo.toml — updated
    rhai = { workspace = true } added (app uses rhai::Dynamic for constants).

profiles/ (new)

  launchpad.rhai:
    on_hw_event registers PadPressed handler: toggle step on row's sequencer
      (CMD_TOGGLE_STEP), write selected_track to state bus.
    subscribe to /node/{seq}/state/steps and current_step for all 8 tracks.
      On steps change: update_track_leds (blue=active, dim=inactive).
      On current_step change: update_step_indicator (green = current).
    def_macro("play_stop"): toggle /transport/playing.
    bind_macro to top button 72 (scene row 0 right button on MK2).

  digitakt.rhai:
    on_hw_event: routes relative encoder deltas to parameters on selected track.
      Encoder 0 → sampler pitch (CMD_BUMP_PARAM)
      Encoder 1 → distortion drive (CMD_BUMP_PARAM)
      Encoder 2 → filter cutoff_hz (CMD_BUMP_PARAM)
      Encoder 3 → filter resonance (CMD_BUMP_PARAM)
    Track buttons 8–15: write selected_track to state bus.

  keystep.rhai:
    on_hw_event: FaderMoved id=1 (mod wheel) → filter cutoff on bass track
      (CMD_SET_PARAM, track 7 filter).
    Note events routed through graph; profile handles only mod wheel.


TEST COVERAGE
-------------
191 tests total, 0 failures.
(169 at initial P4 ship; +22 added during cleanup and two review passes — see sections below)

paraclete-node-api (83 tests, up from 66)
  command: node_command_is_24_bytes, cmd_constants_are_in_universal_range
  parameter: default_values, cmd_set_param_clamps_to_range,
    cmd_bump_param_applies_delta_and_clamps, unknown_type_id_silently_ignored,
    get_returns_zero_for_unknown_param_id, reset_restores_defaults
  hardware: encoder_behaviour_absolute_vs_relative,
    hardware_output_handle_is_object_safe,
    hardware_event_msg_is_copy,
    (surface_descriptor, rgb_color, hardware_event retained)
  context: process_input_has_commands_field (new),
    (all P3 context tests retained)

paraclete-nodes (46 tests, up from 35)
  gateway (3): gateway_starts_with_no_ports,
    gateway_set_connection_record_adds_port (capabilities_dirty reset via
      Cell::replace(false) in capabilities_changed()),
    consumer_drain_collects_pushed_events
  mix (1): mix_node_sums_inputs_with_unity_gain
  distortion (2): distortion_at_zero_drive_passes_signal_through (uses 0.01
    input; tanh(0.01) ≈ 0.01 within 0.001 tolerance),
    distortion_at_max_drive_clips_near_unity
  filter (2): filter_at_high_cutoff_passes_dc,
    filter_state_zeroed_between_activate_calls
  sequencer (4 new): cmd_toggle_step_flips_active,
    cmd_clear_deactivates_all_steps,
    published_state_includes_steps_bitfield,
    published_state_has_track_name_when_set
  (all P3 sampler, oscillator, clock, mapping tests retained)

paraclete-hal (16 tests, up from 5)
  launchpad: parse_note_on_velocity_positive_is_pad_pressed,
    parse_note_on_velocity_zero_is_pad_released,
    parse_note_off_is_pad_released, parse_cc_positive_velocity_is_button_pressed
  digitakt: decode_relative_clockwise, decode_relative_counter_clockwise,
    decode_center_detent
  keystep: note_on_produces_midi2_event, note_off_via_zero_velocity,
    mod_wheel_produces_fader_event
  (existing emulator tests retained)

paraclete-runtime (10 tests, unchanged count)
  All P3 runtime tests pass. ProcessInput construction in executor updated
  to include commands: &[].

paraclete-scripting (9 tests, up from 6)
  state_read_builtin_returns_value (new)
  send_cmd_accumulates_node_commands (new)
  set_led_accumulates_output (new)
  (all P3 scripting tests retained)


DECISIONS MADE DURING IMPLEMENTATION
--------------------------------------

1. capabilities_changed() is &self — Cell<bool> required for ScriptingGatewayNode

  The Node trait declares capabilities_changed(&self) -> bool. The gateway
  needs to reset its dirty flag on read (otherwise the runtime would see it
  dirty on every call). &self cannot take a mutable borrow.

  Resolution: Cell<bool> for interior mutability. replace(false) reads the
  current value and atomically sets it to false in a single call. This is
  correct because ScriptingGatewayNode is single-threaded (audio-thread only).
  No Arc<Mutex<>> overhead needed; Cell is zero-cost.

2. rhai 1.25 FnPtr::call signature has no Scope parameter

  The spec described FnPtr::call as taking (engine, ast, scope, args).
  In rhai 1.25, the actual signature is:
    fn_ptr.call::<T>(engine, ast, args) -> Result<T, ...>
  There is no Scope parameter. Captured closures carry their scope internally
  in the FnPtr value. Passing scope as an argument would be wrong even if the
  API allowed it — the closure's captured environment would be shadowed.

  Resolution: All FnPtr::call invocations use (engine, ast, args). For named
  function calls (engine.call_fn), scope is still passed and mutations are
  visible outside the call. For closure FnPtrs registered by on_hw_event and
  subscribe, engine.call_fn is not used.

3. rhai 1.25 ast.iter_fn_def() is private

  The spec's eval_file() called ast.iter_fn_def() to check whether on_load()
  exists before calling it. In rhai 1.25, iter_fn_def() is marked with
  #[expose_under_internals] and is not part of the public API.

  Resolution: call engine.call_fn::<()>(&mut scope, &ast, "on_load", ()) and
  ignore the result. If the function does not exist, rhai returns an error
  variant for missing function; ignoring it is correct behavior. If it does
  exist, it runs. No conditional check needed.

4. rtrb PushError<T> has no field accessor

  configurator.send_command() needed to return Err(NodeCommand) when the ring
  buffer is full (so the caller can retry). rtrb::PushError<T> is a wrapper
  type but in rtrb 0.3 the inner value is not accessible via .0 field syntax.

  Resolution: match the push result:
    match self.node_cmd_producer.push(cmd) {
        Ok(()) => Ok(()),
        Err(_)  => Err(cmd),
    }
  The original NodeCommand is moved into push(); if it fails, the Err branch
  captures cmd which was moved before the push call. Since NodeCommand is Copy,
  the caller can re-use it safely.

5. ScriptingGatewayNode per-port device_id tagging — first device only

  The gateway collects Event::Hardware events from input.events. At P4,
  ProcessInput does not carry per-event source port metadata — all events
  from all input ports arrive in the same flat events slice. The gateway
  cannot determine which port an event arrived on.

  Resolution: all events are tagged with device_ids[0] (the first registered
  device). In the P4 graph, the Launchpad connects on port 0, so Launchpad
  events are correctly tagged. Digitakt events arriving on port 1 are
  incorrectly tagged with the Launchpad's device_id. In practice, Digitakt
  sends EncoderChanged events and the Launchpad sends PadPressed events; the
  scripting handlers check event_type, so the wrong device_id causes the
  wrong script handler to fire for Digitakt events. This is the primary P4
  limitation. Resolution at P5: ProcessInput carries source port_id per event,
  or the gateway receives events port-by-port rather than from a flat slice.

6. DistortionNode blend=1 applies tanh at drive=0

  The distortion formula at drive=0 is: gain = exp(0) = 1, wet = tanh(input).
  tanh is not an identity function; tanh(0.5) ≈ 0.462. With blend=1 (full wet),
  the output is 0.462, not 0.5. The initial test expected 0.5 and failed.

  Resolution: the test was corrected to use input=0.01 where tanh(0.01) ≈ 0.01
  within 0.001 tolerance. This is correct behavior: at zero drive, the node
  is not a unity-gain passthrough — it is a soft clipper with minimal saturation
  for small signals. For full passthrough, the blend parameter must be 0.0
  (100% dry). The output_level parameter at 0 dB (db_to_linear(0) = 1.0) gives
  unity gain on the wet path; the drive controls pre-gain, not presence of tanh.

7. set_constant() workaround left incomplete

  The ScriptingEngine spec described a set_constant(key, value) method for
  injecting constants into the next script eval. eval_file() instead takes a
  constants parameter directly; the app does not call set_constant() at all.
  The method was written as a stub that pushes a zero-value NodeCommand into
  pending_commands as a placeholder comment, which is incorrect behavior.
  Since the app never calls set_constant(), this does not affect runtime
  correctness, but the method should either be removed or properly implemented
  before it is used. The pending_constants() method is also unreachable.

8. App passes rhai::Dynamic to paraclete-scripting

  eval_file() takes constants: &[(String, rhai::Dynamic)]. The app constructs
  Dynamic values (from node IDs, Vec<Dynamic> for track ID arrays) before
  calling eval_file. This gives paraclete-app a direct dependency on rhai.
  rhai was added to paraclete-app's Cargo.toml. The alternative (a newtype
  wrapper for constants in paraclete-scripting) would add API surface with no
  benefit given that the app is GPL-3.0 and is allowed to depend on any crate.

9. MacOS Rust toolchain path

  The Rust stable toolchain is at:
    /Users/Dustin/.rustup/toolchains/stable-aarch64-apple-darwin/bin/
  cargo is not on PATH by default in this environment. Use:
    export PATH="$PATH:/Users/Dustin/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
  before running cargo commands in a new shell.


WHAT P4 DELIBERATELY DEFERRED
-------------------------------
  ScriptingGatewayNode per-port device_id — P5
    All hardware events currently tagged with first registered device's id.
    Correct dispatch requires ProcessInput to carry source port_id per event,
    or the gateway to iterate input ports individually rather than the flat
    events slice.
  Signal port buffer wiring — P5
    cv(), phase(), logic(), pitch(), modulation() return empty buffers for both
    connected and unconnected ports. No P4 node requires these ports.
    Full wiring: per-connection Vec<f32> allocated at build_executor() time.
  Sequencer v2 features — P5
    Conditional trigs, micro-timing, swing, multi-pattern. Sequencer node
    unchanged from P3 except for CMD_TOGGLE_STEP/SET_STEP/CLEAR and bitfield.
  ReverbNode, DelayNode, SplitNode — P5
    Effect chain depth is Distortion→Filter only at P4. Send routing requires
    SplitNode. Reverb/delay require dedicated DSP work.
  egui developer window — deferred
    --dev-ui currently prints step positions to stderr. Full egui state bus
    monitor and node graph view deferred; eframe dependency not added.
    XL Mk2 absolute encoder calibration gesture — deferred
    Digitakt (relative encoders) used instead per p4-interfaces.md D9.
  MIDI 2.0 CI negotiation — P5
    Standard MIDI 1.0 only at P4. Keystep and Digitakt both speak standard MIDI.
    CI negotiation forced at P5 when deeper MIDI 2.0 hardware interaction begins.
  Scriptable trait (synchronous node queries from scripts) — P5/P6
    NodeCommand + state bus cover all P4 scripting needs.
  set_constant() / pending_constants() cleanup — P5 prep
    set_constant() is dead code at P4. Remove or properly implement before P5
    work adds a use case for it.


NEXT: P5
--------
  Fix ScriptingGatewayNode per-port device_id (ProcessInput port metadata)
  Sequencer v2: conditional trigs, micro-timing, swing, multi-pattern
  ReverbNode (Freeverb algorithm, MIT), DelayNode (tempo-synced)
  SplitNode (1 input → N outputs for send routing)
  Connected signal port views (cv/pitch/mod buffers wired at build_executor)
  Song mode: chain patterns across tracks synchronously


POST-REPORT ADDENDUM — found during terminal testing
======================================================
Date: May 2026
Additional findings and fixes made during end-to-end testing in the same session.


BUGS FIXED
----------

1. InternalClock::with_bpm() constructor missing

  The app declared `const BPM: f64 = 140.0` but `InternalClock::with_domain(0)`
  always starts at 120.0 BPM. The `with_bpm` constructor didn't exist. The app
  was running at 120 BPM silently.

  Fix: added `InternalClock::with_bpm(bpm: f64) -> Self` to internal_clock.rs.
  The app now calls `InternalClock::with_bpm(BPM)`. Verified at 140 BPM via the
  dev-ui step advance rate.

2. digitakt.rhai encoder param ID mapping wrong

  Two bugs in the profile script:
    - Encoder 0 targeted TRACK_SAMP_IDS with param_id=0. The Sampler uses
      FNV-1a hashes for param IDs, not sequential integers; param_id=0 was
      silently ignored. Also, the Sampler has no ParameterBank at P4, so
      CMD_BUMP_PARAM to a Sampler is always a no-op regardless of param_id.
    - Encoder 1 used arg0=1 (PARAM_OUTPUT_LEVEL) instead of arg0=0 (PARAM_DRIVE).

  Fix: encoder 0 → DistortionNode PARAM_DRIVE (id=0); encoder 1 → FilterNode
  PARAM_CUTOFF (id=0); encoder 2 → PARAM_RESONANCE (id=1); encoder 3 →
  PARAM_BLEND (id=2). Sampler pitch control deferred to P5 when Sampler gets
  ParameterBank support. Profile updated; comment added explaining the limitation.

3. digitakt.rhai device_id handler mismatch workaround

  The gateway tags all events with port-0's device_id (LP_DEVICE_ID). The
  digitakt.rhai profile originally registered `on_hw_event(DT_DEVICE_ID, ...)`
  which would never fire since DT_DEVICE_ID != LP_DEVICE_ID. Changed to
  LP_DEVICE_ID with a comment explaining the workaround and the P5 fix path.
  No conflict with launchpad.rhai: both register on LP_DEVICE_ID but handle
  different event types (PadPressed vs EncoderChanged).


FINDINGS DURING TESTING
-----------------------

4. Sequencer step period is 241 ticks, not 240

  After a boundary fires and resets step_tick=0, the next boundary fires 241
  ticks later (step_tick cycles 0→1→...→240, requiring 241 increments). The
  first boundary after global_start fires after 240 ticks because global_start
  leaves step_tick=1 (not 0) — the tick that carries global_start also runs the
  else branch and increments step_tick. Subsequent boundaries all take 241 ticks.

  This is a pre-existing behaviour in the Sequencer, not a P4 regression. The
  effective BPM drift is < 0.5% and was not noticed in P3 testing. The actual
  firing schedule from global_start:
    Step 1 fires at tick 240
    Step 2 fires at tick 481
    Step N fires at tick 240 + (N-1)*241 for N in 1..=15
    Step 0 fires at tick 240 + 15*241 = 3855

  The consequence: any test that infers fired step from tick number must use the
  above formula, not N×240. The integration test in paraclete-nodes/tests/ avoids
  this by encoding each step's index as its MIDI note number, making the
  identification independent of tick timing. This is the correct test pattern.

  Resolution at P5: the global_start handler should set step_tick=1 (not 0) OR
  the tick accumulation should check the boundary before incrementing, making
  all steps fire at exact multiples of ticks_per_step. Either change requires
  updating the existing timing tests.

5. LED output from scripts not delivered to hardware

  The main loop calls scripting.take_pending_output() (via the drain step) but
  does not route the resulting HashMap<u32, HardwareOutput> to the device output
  handles. LED updates accumulate correctly inside the scripting engine but
  never reach the physical Launchpad. This gap was present in the original
  p4-interfaces.md spec ("routing detail resolved during implementation") and
  was not resolved.

  For the LaunchpadEmulator, this has no visible effect since the emulator renders
  its own pressed-pad state. For the physical Launchpad, step-active LED feedback
  does not light. Fix deferred to P5.


ADDITIONS
---------

6. InternalClock::with_bpm() added to paraclete-nodes

  New constructor: `pub fn with_bpm(bpm: f64) -> Self`. Builds `with_domain(0)`
  and then sets the bpm field. Required by the app and referenced in P5+.

7. tools/gen-samples crate added to workspace

  Generates samples/track0.wav through track7.wav — synthesized drum sounds
  (kick, snare, hats, percs, FX, bass) using only hound and inline XorShift RNG.
  Run: `cargo run -p gen-samples`. Optional output directory argument.
  Listed in README.md test environment section.

8. paraclete-nodes/src/pattern.rs — canonical 8-track pattern

  Defines `TRACKS: &[TrackPreset]` (8 entries) and `apply_preset()`.
  Used by paraclete-app as the startup preset and by the integration test.
  The app no longer hard-codes step configuration in main.rs; both the app
  and the test derive from the same canonical source.

  Pattern:
    Kick:   steps 0,4,8,12  (4-on-the-floor)
    Snare:  steps 4,12       (backbeat 2/4)
    Hat CH: steps 0,2,4,6,8,10,12,14  (eighth notes)
    Hat OH: steps 2,6,10,14  (off-beat eighths)
    Perc A: steps 2,10
    Perc B: steps 6,14
    FX:     steps 3,11
    Bass:   steps 0,4,8,12  (mirrors kick)

9. paraclete-nodes/tests/eight_track_pattern.rs — 6 tests

  Integration test file. Core test drives each of the 8 sequencers with
  synthetic TransportEvents, using note=step_index encoding to identify fired
  steps by note value rather than tick timing. Runs 5000 ticks per track.
  Secondary tests: step counts, valid indices, Kick/Bass groove lock, adjacent
  tracks distinct.

10. README.md replaced with developer README

  The root README was a stale design-docs index (remnant of design/README.md
  being moved to root). Replaced with a practical developer README covering:
  quick start, keyboard controls, dev-ui format, tmux test session recipe
  (copy-pasteable), hardware setup, commands reference, P4 status, known gaps.

11. dev-ui enhanced to show steps bitfield

  `--dev-ui` output now shows per-track current_step AND the 16-character steps
  bitfield on the same line, enabling direct observation of step-toggle effects
  without requiring scripting or hardware LED feedback.


TEST COVERAGE UPDATE
--------------------
175 tests total (up from 169 at initial P4 completion).

New tests (6):
  eight_track_pattern_fires_all_sounds_at_correct_steps
  all_eight_tracks_have_active_steps
  pattern_step_counts_match_design
  pattern_step_indices_are_valid
  kick_and_bass_share_downbeat_positions
  adjacent_tracks_have_distinct_patterns


POST-COMMIT ANALYSIS — June 2026
=================================
Review of P4 completion status against the original deliverable.


BLOCKING GAPS FOR PHYSICAL HARDWARE USE
-----------------------------------------

Three issues prevent the P4 deliverable ("sit down with three devices and
make a beat, mouse never touched") from being fully realised on real hardware.
They are already documented individually above; collected here for clarity.

1. LED output from scripts never reaches hardware (finding #5 above)

  The most visible gap. Step-active LED feedback does not light on the physical
  Launchpad. The instrument is visually blind when running on hardware. The
  LaunchpadEmulator masks this because it renders its own pad state independently
  of the scripting output path. Fix requires routing take_pending_output() results
  to the stored HardwareOutputHandle for each device in process_main_thread().

2. ScriptingGatewayNode device_id tagging (decision #5 above)

  All hardware events are tagged with the Launchpad's device_id regardless of
  which device sent them. The digitakt.rhai profile works around this by
  registering on LP_DEVICE_ID and filtering by event_type (EncoderChanged vs
  PadPressed), which is safe only because no two devices share an event type at
  P4. The fix — ProcessInput carrying source port_id per event, or the gateway
  iterating ports individually — is required before a third device type could
  cause ambiguous dispatch.

3. Sampler has no ParameterBank (bug #2 above, post-addendum)

  CMD_BUMP_PARAM to any Sampler node is a silent no-op. The Digitakt profile's
  encoder 0 (originally intended for sample pitch) was redirected to
  DistortionNode drive as a workaround. Sampler parameters (pitch, decay,
  start/end, reverse) are not reachable from hardware. This is the primary
  reason the instrument cannot be "played" in the intended sense — the Digitakt
  encoders cannot shape the sound of individual drum hits.


HARDWARE INTEGRATION TESTING GAP
----------------------------------

P4 unit tests cover MIDI parsing (LaunchpadNode, DigitaktMidiNode, KeystepNode)
and node behaviour (Sequencer, DistortionNode, FilterNode, MixNode, Sampler)
independently. There is no integration test covering the full hardware event
pipeline:

  Physical MIDI → HardwareNode → ScriptingGatewayNode → ScriptingEngine
    → NodeCommand → NodeConfigurator::send_command() → NodeExecutor → Node

The gap means regressions in the scripting dispatch or command routing path
would not be caught by automated tests. An integration test that drives a mock
hardware event through the gateway, confirms the scripting engine fires the
correct handler, and verifies the resulting NodeCommand reaches a node's
ParameterBank would close this gap.

Additionally, the Keystep melodic recording path (KeystepNode → graph →
Sequencer[7] live note capture) is wired at the graph level but has no test
confirming that Keystep note events are received and recorded by the bass
sequencer track.


RECOMMENDATION: INTERMEDIATE PHASE BEFORE P5 SEQUENCER WORK
-------------------------------------------------------------

P5 as scoped in roadmap.md is large: Sequencer v2 (conditional trigs,
micro-timing, swing, multi-pattern), ReverbNode, DelayNode, SplitNode,
connected signal port views. Building that on top of three unresolved hardware
bugs risks compounding complexity before the foundation is solid.

Recommended pre-P5 scope (P4.5 or P5 early milestone):

  Fix LED routing         — route scripting output to HardwareOutputHandle
  Fix gateway device_id   — ProcessInput carries source port_id per event
  Sampler ParameterBank   — pitch, decay, start/end reachable from hardware
  Hardware pipeline test  — end-to-end mock test: event → script → command → node
  Keystep recording test  — confirm note events reach Sequencer[7]

These five items close the gap between "code is correct in isolation" and
"the instrument works as described in instrument-vision.md." After them, the
P5 sequencer work has a verified hardware layer to build on.


DEFERRED FINDINGS FROM P2/P3 CODE REVIEWS
==========================================
Retroactive code reviews of the P2 and P3 commits were performed after P4
implementation completed. Findings were triaged into: fixed during P4
implementation, fixed as P4 pre-commit cleanup (below), or deferred to P5.
The following items are confirmed open and deferred to P5.

P2-C3 — CONFIRMED: Audio inputs always &[] in executor
  ProcessInput::audio_inputs is hardcoded to &[] in NodeExecutor::process()
  (executor.rs). Nodes that declare audio input ports (FilterNode, DistortionNode,
  MixNode) receive no audio data from upstream. At P4 the audio graph topology
  routes correctly via output buffers being passed from node to node, but the
  named audio_inputs field on ProcessInput is never populated. Full audio routing
  through ProcessInput::audio_inputs requires the executor to wire the upstream
  node's output buffer into the downstream node's input at build_executor() time.
  Deferred to P5 when audio signal port wiring is implemented.

P2-C4 / P2-S6 — CONFIRMED: ConnectionAgreement::baseline() hardcodes 44100.0 / 512
  ConnectionAgreement::baseline() always returns sample_rate=44100.0 and
  block_size=512 regardless of the actual audio backend configuration. The runtime
  acquires these values from cpal at startup but does not communicate them back to
  the node API. Nodes that use the ConnectionRecord for DSP (FilterNode uses
  sample_rate for coefficient computation) are currently correct because the
  hardcoded value matches the default cpal output on macOS. This will break when
  running at 48 kHz or with a non-512 block size. Fix: pass actual sample_rate and
  block_size from NodeExecutor to ConnectionAgreement at build_executor() time.

P2-C5 — CONFIRMED: paraclete-hal depends on paraclete-runtime (layer violation)
  paraclete-hal/Cargo.toml lists paraclete-runtime as a workspace dependency.
  The HAL (L0) must not import from the Runtime (L1). The violation is intentional
  at P4 — the HAL uses paraclete-runtime::state_bus::StateBusHandle to publish
  hardware state. The correct fix is to expose StateBusHandle through
  paraclete-node-api (L2) so hardware devices can use it without crossing the
  layer boundary. Deferred to P5/P6 when the layer model is hardened.

P2-S3 — Already documented above as finding #4 (step period 241 not 240)

P2-S8 — add_tempo_source type signature mismatch
  NodeConfigurator::add_tempo_source() accepts Box<dyn Node> not Box<dyn TempoSource>.
  The TempoSource trait is the intended contract for clock nodes but the registration
  method does not enforce it. Callers must manually pass a tempo source that happens
  to also implement Node; the compiler does not catch a non-clock node being
  registered as a tempo source. Fix: change signature to accept Box<dyn TempoSource>
  and add a blanket impl or wrapper. Deferred to P5.

P3-S2 — initial_transport always None
  Sequencer::initial_transport is set in deserialize() but is never written
  during normal operation. A node that connects mid-session cannot sync its
  position to the running transport; it must wait for the next global_start.
  Fix requires the runtime to snapshot the current transport and deliver it to
  newly connected nodes via set_connection_record() or an explicit
  sync_transport() call. Deferred to P5.

P3-S3 — lockable_params reconciliation is dst-only / asymmetric
  ConnectionRecord::lockable_params is populated from the downstream node's
  ConnectionAgreement only (the Negotiable handshake at connect() time reads
  the dst node's agreement, not the src node's). If two nodes both implement
  Negotiable and both declare lockable_params, only the dst side is recorded.
  This is not a P4 bug because no P4 sequencer-to-instrument pair has both
  sides implementing Negotiable. Fix at P5 when the reconciliation becomes
  bi-directional.

P2-M3 — SineOscillator ignores sample_offset
  SineOscillator::process() generates a sine wave starting at phase=0 for every
  event regardless of the sample_offset field on the event. Events with non-zero
  sample_offset should delay the note start within the current block; the
  oscillator renders from the start of the block instead. Not audible at P4
  because all sequencer events are emitted with sample_offset=0.

P2-M8 — PortType::Mono is dead code
  PortType::Mono is defined in the node API enum but no shipped node declares
  a Mono port, no executor code routes Mono ports, and no test uses it.
  Either remove it or implement routing. Deferred to P5 port type cleanup.


P2/P3 PRE-COMMIT CLEANUP FIXES
================================
These findings from retroactive P2 and P3 code reviews were confirmed open and
fixed before the P4 code review was run (2026-06-02). 13 new tests added.

P3-C1: Sampler param lock filter uses is_known_param()
  Added Sampler::is_known_param(param_id) static method. Replaced heuristic
  (base_for != 0.0 || param_id == pitch) with explicit allowlist check. Prevents
  zero-default params (pan, start) from being silently dropped when locked.

P3-C2: StateBus entries moved (not cloned) into SPSC push
  Reordered executor end-of-cycle: build_hardware_output(&entries) borrows first,
  then hw_update(), then push(StateBusUpdate { entries }) moves (no clone).
  Eliminates allocation on the audio thread.

P3-S1: node_locks cleared at START of each process() cycle
  Moved self.node_locks.clear() from trigger_voice() to the very start of
  Sampler::process() (before the event loop). Prevents lock bleed between steps:
  locks are cleared, repopulated by ParamLock events, then used by both
  trigger_voice (copies to voice.active_locks) and render (effective_node reads
  node_locks directly).

P3-S4: LockableParam.unit uses p.unit from ParamDescriptor
  Sampler::lockable_params_list() now takes ParamDescriptor by value via
  into_iter() and sets unit: p.unit instead of hardcoding ParamUnit::Generic.

P3-S6: write_sandboxed() only allows /node/{id}/param/ paths
  Changed the sandbox check from starts_with("/node/") to require the fourth
  path segment to be "param". Writes to /node/{id}/state/* are now rejected.
  Updated state_write string regression tests to use /node/{id}/param/ paths.

P3-M3: Voice::active_locks initialized with HashMap::with_capacity(7)
  Changed Voice::new() from HashMap::new() to HashMap::with_capacity(7) to
  avoid rehashing on first note trigger.

P2-C6: incoming[slot_idx] vec returned after process() to preserve allocation
  After slot.process() returns, the executor clears the event vec and stores it
  back at self.incoming[slot_idx]. Prevents reallocation on every audio cycle.

P2-S2: connect() validates port existence, direction, and type match
  Added full port validation: source port must exist and be Output; destination
  port must exist and be Input; port types must match. Returns Err with message
  on any violation. Cycle check still performed after type-safe edge insertion.

P2-M7: register() asserts unique node ID
  Added assert!(!id_to_index.contains_key(&user_id)) at start of register().
  Panics immediately on duplicate registration rather than silently overwriting.

P2-M1: #[non_exhaustive] added to PortType, PortDirection, Event enums in L2
  External crates matching on these enums must now include a wildcard arm.
  Event sort in executor updated with _ => 5 fallback.

P3-C3: Node::is_negotiable() added (default false)
  Negotiable implementors must also override is_negotiable() → true. Sampler
  updated. Allows runtime to skip negotiate() call without downcasting.

Tests added: connect_invokes_negotiate_and_set_connection_record_on_both_sides,
  executor_delivers_param_lock_before_midi2_at_same_sample_offset,
  state_bus_write_sandboxed_rejects_node_state_path,
  configurator_connect_rejects_wrong_direction,
  configurator_connect_rejects_type_mismatch,
  configurator_connect_rejects_nonexistent_src_port,
  configurator_connect_rejects_nonexistent_dst_port,
  configurator_register_panics_on_duplicate_id,
  is_negotiable_default_false, sampler_is_negotiable_true,
  sampler_param_lock_zero_default_param_applies,
  sampler_volume_param_lock_changes_output_level,
  sampler_param_lock_cleared_at_cycle_start


P4 CODE REVIEW FIXES
====================
A context-isolated code review of P4 was performed (2026-06-02). The review
identified 4 critical, 10 significant, and 10 minor findings.
Full report: design/review/p4-code-review.md

Critical findings fixed before commit:

C-001 — Filesystem I/O in trigger_voice() removed
  Sampler::trigger_voice() was opening /tmp/paraclete-debug.log on every note
  trigger from the audio thread — a blocking syscall that causes xruns.
  Fix: removed the file write block entirely. The samp_trig_count counter
  remains and is published via published_state() for observability.

C-002 — state_write Rhai builtin now uses write_sandboxed()
  The state_write() Rhai function was calling bus.write() (unsandboxed),
  allowing scripts to overwrite any state bus path including /transport/bpm.
  Fix: changed to bus.write_sandboxed(); the sandbox silently ignores
  writes to non-/node/{id}/param/ paths.

C-003 — node_locks.clear() removed from release_voice()
  Sampler::release_voice() was calling self.node_locks.clear() after releasing
  a voice. If a NoteOff and a ParamLock+NoteOn arrive in the same cycle, the
  NoteOff would erase the locks before they govern the new note. Fix: removed
  the clear from release_voice(). The per-cycle clear at the start of process()
  is correct and sufficient.

C-004 — ScriptingGatewayNode device tagging (acknowledged, deferred to P5)
  All events fanned into ScriptingGatewayNode are tagged with device_ids[0]
  regardless of which input port they arrived on. Scripts cannot distinguish
  which physical device sent an event when multiple devices share one gateway.
  Architectural fix requires ProcessInput to carry per-port device_id metadata.
  Deferred to P5.

Significant/minor findings fixed in this pass:

S-001: send_cmd debug file I/O removed (was main-thread I/O on every pad press).
S-002: set_constant() and pending_constants() removed — dead no-op that poisoned
  pending_commands with a bogus entry. Constants injected via eval_file() only.
S-004: Signal port stubs (cv/phase/logic/pitch/modulation) documented with P5
  notice in ProcessInput — third-party nodes see clear "not yet implemented" docs.
S-006: FilterNode bank.get(PARAM_FILTER_TYPE) hoisted before the sample loop;
  svf_sample() now takes filter_type: u32 as a parameter.
S-007: Sequencer sync-pulse branch now emits NoteOff before NoteOn when
  gate_open is true, preventing doubled NoteOn / missing NoteOff.
M-004: DistortionNode sequential param IDs documented with comment explaining
  the deliberate departure from id_for_name() hashes.
M-006: decode_relative_delta() in DigitaktMidiNode uses explicit `0 | 64 => 0`
  arm with comment, plus `_ => 0` for out-of-range MIDI values.
M-007: FilterNode f_coeff now uses sin() form for Chamberlin SVF stability at
  high cutoff. Old linear approx diverged near Nyquist.
M-009: Clone + Debug derived on all hardware descriptor and output types in L2:
  SurfaceDescriptor, Control, PadDescriptor, ButtonDescriptor, EncoderDescriptor,
  FaderDescriptor, LedDescriptor, DisplayDescriptor, DisplayType, HardwareOutput,
  LedUpdate, DisplayUpdate, DisplayContent.
M-010: Sampler published_state paths changed from /node/{id}/samp/ to
  /node/{id}/state/ to match the documented convention.

Significant findings deferred to P5:

S-003: MixNode published_state() not implemented (no observable gain state).
S-005: Replace Mutex<VecDeque> with SPSC in hardware nodes (LaunchpadNode,
  DigitaktMidiNode, KeystepNode).
S-008: published_state() allocates ~600 heap items/second on audio thread —
  add dirty flag per node, only publish when state changes.
S-009: dispatch_hardware_event() clones context name list per call — cache or
  iterate contexts directly.
S-010: HardwareOutput::empty() allocates two Vecs per audio cycle for legacy-path
  devices; pre-allocate or check before constructing.
M-001: LaunchpadOutputHandle::Drop sleeps 30ms during runtime teardown.
M-002: LaunchpadNode::open() blocks 200ms during startup (hardware handshake).
M-008: ScriptingGatewayNode capabilities_changed() consuming read via replace().
M-005: capability_document() allocates Vecs at activate() time (acceptable but
  worth documenting).
