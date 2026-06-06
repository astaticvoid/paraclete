Paraclete — P8 Implementation Report
=====================================
Date: June 2026
Status: In progress — 329 tests, 0 failures (after Commit 3)

WHAT SHIPPED (P8)
-----------------

Commit 1 — P7 residuals (paraclete-node-api, paraclete-nodes, paraclete-clap,
           paraclete-machine-{kick,snare,fm-kick,fm-bell,fm-bass})
Tests after: 321 (+4 from baseline 317)

  InternalClock transport stop response (paraclete-nodes/src/internal_clock.rs)

    InternalClock::process() now scans incoming events for TransportEvent flags
    before the tick logic. On global_stop, sets playing = false (halts ticking).
    On global_start when !playing, sets playing = true and re-arms first_tick so
    the downstream GlobalStart signal is re-emitted. The !playing guard prevents
    re-arming when InternalClock is already running in standalone mode.

    Fixes the SubgraphPlugin context: DAW transport stop now silences InternalClock
    without affecting standalone operation. The existing subgraph_plugin_* tests
    continue to pass unchanged.

    New tests: internal_clock_stops_on_global_stop_event,
    internal_clock_resumes_after_global_stop_then_global_start.

  SILENCE buffer fix (paraclete-node-api/src/context.rs)

    static SILENCE promoted to module level (shared by logic() and modulation()).
    Size expanded from 4096 to 65536 f32s (256 KB BSS). An assert! guards the
    slice construction in both methods; fires in both debug and release if a
    caller passes block_size > 65536.

    New test: silence_buffer_at_max_block_size_is_all_zeros.

  Machine bank real CLAP FFI (paraclete-clap/src/ffi.rs, paraclete-machine-*)

    New ffi.rs module: shared PluginWrapper (with pre-allocated events_pool and
    commands_pool to avoid audio-thread allocation), plugin_class() vtable, and
    10 unsafe extern "C" lifecycle callbacks. The spec called for [[bin]] with
    crate-type = ["cdylib"] (invalid Cargo syntax); five separate workspace crates
    are used instead:

      paraclete-machine-kick   — AnalogEngine::kick()
      paraclete-machine-snare  — AnalogEngine::snare()
      paraclete-machine-fm-kick — FmEngine::kick()
      paraclete-machine-fm-bell — FmEngine::bell()
      paraclete-machine-fm-bass — FmEngine::bass()

    Each produces a loadable .dylib/.so (confirmed: file reports "dynamically
    linked shared library"). The old [[bin]] stub targets in paraclete-clap were
    removed.

    SubgraphPlugin: added prev_playing() accessor; process_block() signature
    changed to Option<&TransportEvent> (None auto-computes the transition event).

    New test: machine_bank_ffi_entry_not_null (verifies all 10 vtable callbacks
    are non-null via the library's ffi module).

  Code review findings caught before commit:
    - Audio-thread Vec::new() in plugin_process() → replaced with pool fields
    - [[bin]] cannot produce cdylib → split into 5 workspace crates
    - Duplicate SILENCE statics → promoted to module level

-----

Commit 2 — publish_context() Rhai API (paraclete-scripting)
Tests after: 324 (+3 from 321)

  publish_context(encoder_key, node_id, param_name) Rhai builtin
  (paraclete-scripting/src/lib.rs)

    New engine builtin registered alongside state_write, send_cmd. Writes:
      /context/{encoder_key}/node  → node_id as f64
      /context/{encoder_key}/param → ParamDescriptor::id_for_name(param_name) as f64

    The TUI (Commit 4) subscribes to these paths to label its encoder row.
    Writes to /context/ are not sandboxed — publish_context is a privileged
    scripting API, unlike state_write_sandboxed which restricts to /node/*/param/*.

    Follows the Rc::clone(&state) capture pattern of all other stateful builtins.
    ParamDescriptor::id_for_name is const fn (promoted in P7), so no allocation.

    New tests: publish_context_writes_node_and_param_to_state_bus,
    publish_context_overwrites_previous_mapping,
    publish_context_different_keys_do_not_collide.

-----

Commit 3 — Instrument definition file (paraclete-app, paraclete-node-api,
           paraclete-nodes)
Tests after: 329 (+5 from 324)

  Instrument definition YAML (ADR-026) (paraclete-app/src/instrument.rs,
  paraclete-app/src/builder.rs)

    InstrumentDefinition, NodeDef, EdgeDef, MacroDef — serde-Deserialize types.
    format_version: 1 enforced at parse time; UnknownVersion(n) on mismatch.

    build_from_instrument(def, conf) constructs nodes by type_tag, applies
    initial_params, connects edges by port name or index:
      - Port names resolved from actual node port descriptors (case-insensitive),
        not a hardcoded table — handles any port name correctly.
      - "audio_out" alias resolves to "audio_out_l" for AnalogEngine/FmEngine.
      - InternalClock registered via conf.add_tempo_source() (not add_node) per
        the Configurator API contract; domain_id stored in InstrumentIds.
      - connect() errors reported as InstrumentError::ConnectionError(String),
        distinct from UnknownPort.
      - clap_plugin nodes: MissingField if plugin_id absent, UnknownNodeType
        otherwise (CLAP host not wired until Commit 5).

    Supported type_tags: internal_clock, sequencer, sampler, analog_engine:kick/
    snare/hihat, fm_engine:kick/bell/bass, distortion, filter, mix, audio_output,
    reverb, clap_plugin (returns error at P8).

  Node::set_initial_params() (paraclete-node-api/src/node.rs)

    New default-no-op trait method. Called after construction, before activate().
    DistortionNode and FilterNode override it: store pending params as HashMap,
    apply them to the ParameterBank in activate() after bank construction. Other
    ParameterBank nodes (AnalogEngine, FmEngine, Sampler, ReverbNode) defer to P9.

  AudioOutputNode (paraclete-nodes/src/lib.rs)

    New graph terminus node: single PORT_AUDIO_IN (Input, Audio). No-op process().
    Maps to "audio_output" type_tag in instrument files. Default impl included.

  serde_yaml = "0.9" added to workspace dependencies.

  Code review findings caught before commit:
    - set_initial_params not overridden anywhere → implemented in DistortionNode,
      FilterNode; test updated to assert mechanism works
    - clap_plugin returned UnknownNodeType unconditionally → MissingField path added
    - InternalClock used add_node → fixed to add_tempo_source
    - connect() errors mapped to UnknownPort → ConnectionError variant added

-----

DEFERRED ITEMS (P8+)
--------------------

  - set_initial_params override for AnalogEngine, FmEngine, Sampler, ReverbNode
    (deferred to P9; DistortionNode and FilterNode implemented as proof of mechanism)
  - CLAP plugin MIDI note encoding uses MIDI2 UMP (correct) vs spec example's
    simplified NoteOn struct (spec was outdated — no action needed)
  - serde_yaml 0.9 is deprecated by its author (recommends serde_yml fork);
    migration deferred to P9
