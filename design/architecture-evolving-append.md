
---

> **Note (appended June 2026):** Phase log entries for P4 through P7 were
> not recorded at the time. Brief retroactive entries follow based on the
> phase reports. The phase reports (`p4-report.md` through `p7-report.md`)
> are the authoritative source for those phases. P8 onward is recorded
> in full.

---

### P4 — Hardware Integration (Complete)

**Delivered:** Three-device instrument live. 8-track drum machine graph:
InternalClock → Sequencer[0–7] → Sampler → Distortion → Filter → Mix →
AudioOutput. LaunchpadNode, DigitaktMidiNode, KeystepNode (all optional at
runtime with graceful fallback). ScriptingGatewayNode fan-in; ScriptContext
with hot-reload; Rhai builtins: subscribe(), send_cmd(), set_led(),
def_macro(), bind_macro(). ParameterBank in L2 (ADR-019). HardwareOutputHandle
for main-thread LED output. DistortionNode, FilterNode, MixNode. 188 tests.

**Key findings:**
- LED routing not wired from scripting output to HardwareOutputHandle —
  LED feedback was dark at initial ship; fixed in P4.5
- ParameterBank missing from Sampler — parameter locks fell through; fixed P4.5
- audio_inputs not wired in executor — Sampler received no audio data; fixed P4.5
- Hardware update_output() moved to main thread via HardwareOutputHandle trait;
  the audio thread must never block on LED I/O

**Deferred:** Four P4.5 fixes (shipped separately). State bus canonical path
naming. ScriptContext isolated scopes.

---

### P4.5 — Foundation Verification (Complete)

**Delivered:** Four blocking P4 gaps closed. LED routing wired.
ParameterBank added to Sampler. audio_inputs wired in executor. Integration
test passes (Launchpad → step → Sampler fires → audio output). 212 tests.

**Deferred:** Conditional trigs, micro-timing, swing (P5).

---

### P5 — Sequencer v2 + Effects (Complete)

**Delivered:** Conditional trigs (TrigCondition enum: Always, Fill, Not Fill,
Percent, RepeatN, Repeat2…Repeat8). Micro-timing offset per step. Swing
(ParameterBank entry on Sequencer). ReverbNode (Freeverb), DelayNode
(comb filter), SplitNode. 232 tests.

**Key findings:**
- Negative micro_offset does not push events earlier across the step boundary
  — full signed micro-timing requires cross-boundary emission; deferred
- Sequencer P5 fields (TrigCondition, StepTiming) not serialised — deferred
  through P7

**Deferred:** Signed micro-timing, Sequencer P5 field serialisation.

---

### P6 — Synthesis (Complete)

**Delivered:** L2 API extension (mod_output_mut, logic_output_mut).
OscillatorNode (polyBLEP sine/triangle/saw/square/noise). EnvelopeNode
(AD, ADSR, Looping AD). LfoNode. LadderFilterNode (4-pole Moog topology).
Sampler audio quality: per-voice rubato resampling (rubato crate),
symphonia for WAV/FLAC/OGG. AnalogEngine: KickMachine, SnareMachine,
HiHatMachine. FmEngine: FmKickMachine, FmBellMachine, FmBassMachine.
node_locks / ParamLock pattern codified. 285 tests.

**Key findings:**
- ParamLock routed through bank.handle_commands() bleeds lock value into
  subsequent cycles — node_locks Vec pattern required for all generator
  nodes; codified in ADR-019 amendment
- AnalogEngine/FmEngine are monophonic with retrigger — polyphony deferred
- AnalogEngine/FmEngine audio outputs named "audio_out_l" (not "audio_out")

**Deferred:** Polyphony for synthesis engines. Signed micro-timing.
4-operator FM algorithms.

---

### P6.5 — Hardening (Complete)

**Delivered:** EnvelopeNode Looping AD autonomous cycling fix. Parameter
naming convention codified (ADR-019 amended: canonical names table,
no type-prefix names). LadderFilterNode parameter naming fix (was
"ladder_cutoff"; corrected to "cutoff"). 289 tests.

**Deferred:** Nothing new; P7 preparation.

---

### P7 — DAW Integration (Complete)

**Delivered:** Node::type_name() + published_state() push-down (OQ-9
resolved — pre-allocated state_bufs, agg_state_buf; zero allocation after
first cycle). Per-voice rubato pitch resampling in Sampler (VoiceResampler
struct; SincFixedOut; root_note as ParameterBank entry). Project save/recall
(RON format, ADR-025; NodeConfigurator::all_nodes(), all_edges(); save/load
CLI flags). CLAP adapter infrastructure (paraclete-clap crate; ClapParamBridge;
translate_transport(); set_transport_override()). SingleNodePlugin (Sequencer
as CLAP MIDI effect). SubgraphPlugin (Sequencer + generator subgraph;
inject_event_for_node, inject_node_command). Five machine bank stub binaries.
paraclete-node-api v0.1.0 published to crates.io (LGPL3). 317 tests.

**Key findings:**
- InternalClock initialises playing=true and ignores global_stop events in
  CLAP context — deferred to P8 (Finding 4)
- rubato reset()/set_resample_ratio() ordering: reset() must precede
  set_resample_ratio() at voice trigger; reversed order stalls voice
- SILENCE buffer in context.rs sized 4096 — latent truncation; deferred P8
- Machine bank stub binaries are not real cdylibs — deferred P8
- CLAP_TRANSPORT_IS_PLAYING is bit 4, not bit 0 (bits 0–3 are HAS_* flags)

**Deferred:** InternalClock transport response, machine bank real CLAP FFI,
SILENCE fix (all P8 Commit 1). AnalogEngine/FmEngine polyphony. HiHatMachine
CLAP target. Sequencer P5 field serialisation.

---

### P8 — CLAP Host + Instrument Definition + Terminal UI (Complete)

**Delivered:** P7 residuals resolved (InternalClock stop response, SILENCE
buffer expanded to 65536, machine bank real CLAP FFI via five separate
`paraclete-machine-*` workspace crates). `publish_context()` Rhai builtin
— writes encoder-to-parameter mappings to `/context/{key}/node` and
`/context/{key}/param` (as `id_for_name` hash). Instrument definition file
(YAML, ADR-026): `InstrumentDefinition` YAML schema, `build_from_instrument()`
graph builder, `Node::set_initial_params()` default-no-op trait method.
`paraclete-tui` crate (ratatui + crossterm): transport bar, encoder row,
step row; dirty-flag rendering at ~30 Hz; encoder slot label resolved from
`CapabilityDocument`. `paraclete-clap-host` crate (clap-sys + libloading):
`PluginLibrary`, `HostParamBridge`, `PluginNode` implements `Node`,
`scan_clap_paths()`. App wiring: instrument-file-driven startup, TUI
integration, CLAP plugin loading from instrument YAML. ADR-027 written
and amended. 340 tests.

**Key findings:**
- `[[bin]]` with `crate-type = ["cdylib"]` is invalid Cargo syntax; one
  library crate per machine plugin is the correct model
- `clack` crate referenced in ADR-024 is a placeholder on crates.io; not
  usable. Replaced with `clap-sys + libloading` (ADR-027 amended)
- `StateBusValue` is an enum with Float/Int/Bool/Text variants — prior specs
  treated it as `f64`; the TUI match logic makes the distinction load-bearing.
  InternalClock publishes playing as Bool, Sequencer publishes current_step
  as Int
- Canonical transport state bus paths: `/transport/bpm` and
  `/transport/playing`. Node-scoped paths (`/node/{id}/bpm`) are wrong for
  InternalClock — it publishes to the transport namespace
- `InternalClock` must be registered via `add_tempo_source()`, not
  `add_node()` — instrument builder must use the correct configurator API
- Port names are resolved dynamically from port descriptors, not a hardcoded
  table; AnalogEngine/FmEngine expose "audio_out_l", not "audio_out"
- CLAP plugin Drop lifecycle: `stop_processing()` must precede `deactivate()`
  — omitting it is a CLAP spec violation
- `save_project()` must be called before `build_executor()` — nodes are moved
  into the executor at build time; calling save after build saves an empty list
- Encoder values display as 0.0 in the TUI — no node currently publishes
  per-parameter values to the state bus; TUI value column is non-functional
  until addressed (P9 Commit 1 candidate)
- macOS `.clap` bundles are directories (`Contents/MacOS/<stem>`);
  `PluginLibrary::load()` handles both file and directory forms
- `serde_yaml 0.9` deprecated by author (recommends `serde_yml` fork)
- `capability_document()` leaks two strings per call via `Box::leak`;
  bounded in fixed-topology P8 but will become unbounded in P9

**Deferred:** Encoder value display (per-parameter state bus publishing —
P9 Commit 1). `set_initial_params` for AnalogEngine, FmEngine, Sampler,
ReverbNode (P9). Effect-type CLAP plugins (P9). `serde_yaml` → `serde_yml`
migration (P9). `capability_document()` Box::leak fix (P9, before dynamic
graph). Full CLAP host callbacks, note expressions, preset management (P9+).
Signed micro-timing, Sequencer P5 field serialisation (carried forward).

### P9 — Loop Break + Dynamic Topology + CV (Complete, June 2026)

Six commits (see `design/phases/p9-report.md`; 389 distinct tests, 0
failures): encoder value publishing (`ParameterSlot::name`,
`publish_bank_state()`, `Node::set_node_id()`); housekeeping
(`cap_doc_cache`, `serde_yml` migration, `set_initial_params` coverage,
effect-type CLAP plugins); `LoopBreakNode` + executor pre/post phases
(ADR-028, resolves OQ-5); dynamic topology — `NodeRegistry`,
`apply_patch()` pause-rebuild-resume, project format v2 with `type_tag`
(ADR-029); Sequencer CV outputs (`with_cv_outputs`, `Step::cv_locks`,
sample-and-hold); `GraphNode` marker + `paraclete-graph-nodes` crate with
`InnerGraphNode` (ADR-023).

### P10 — Pattern Engine (Complete, July 2026)

Six commits C0–C5 (see `design/phases/p10-report.md`; 502 workspace tests,
0 failures at close). C0: BUG-001 (240-tick period; the drift was the
bar-sync snap) + BUG-008. C1 (gated): `Pattern` struct, serializer v3
(fixes BUG-005; length-prefixed skip-tolerant records). C2: page-loop
playback window — the wrap IS the cycle boundary; swing per-pattern
authoritative (bank slot = write-through conduit). C3: per-track length +
speed multiplier (fractional periods carry their remainder — drift-free);
BUG-004 fixed (negative micro-timing fires in the previous step's window,
tick-exact); gate is an absolute countdown from note-on; polyrhythm
emergent. C4: pre-allocated 8-pattern bank — zero audio-thread allocation
in the switch path; cued switching at the wrap; volatile chain
(read-then-advance). C5: pattern-engine state paths + TUI playhead window
and pattern/page/speed indicator; the Launchpad §5.3 surface parked with
the Launchpad track (s2). Play-test pending (paired session or Theoria
post-W2). W-track work interleaved the same weeks: Antiphon/Theoria (W0,
W1, legibility phase, baseline interactions) — see `interface-plan.md` and
the session records.
