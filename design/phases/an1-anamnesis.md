# Paraclete — AN1 Anamnesis Specification

> **EXECUTION-READY — 2026-08-21.** Written so a fresh-context session can
> implement without further design decisions: every commit names its files,
> the current behaviour it changes, and its tests. Where a value is a
> tuning knob the default is stated *(tunable)*.
>
> **Design authority:** ADR-040 (accepted 2026-07-23; seven decisions,
> R1–R3 ratification, post-ratification review notes 1–5). Experience
> design: `design/sampling/design.md` §1 (performance loop), §4 (capture
> experience, SAMPLING screen, the transition trick). Problem statement:
> `design/sampling/problem.md`.
> **Baseline:** AN0 closed clean (`an0-report.md`; commits `1d8f20f`..
> `4cd1626`), zero open defects. `cargo test --workspace` green at
> baseline. `SamplePool` + asset-delivery plane + pool-indexed `Sampler`
> already exist; nothing in AN0 recorded anything — there was no recorder
> to record with.
> **Exit:** `cargo test --workspace` green after every commit; `cargo
> clippy --workspace` clean on touched crates; code review on each
> commit; the **⛔ sampling session gate sits here** (ADR-040 R2 — a
> user-paired session evaluating the one-gesture transition trick and the
> SAMPLING screen by ear/hand, not by test). R2 is frozen: AN1 is not
> done without `FUNC+9` = capture-last-2-bars-and-assign working end to
> end.
>
> **Read this first — what this phase is and is not.** AN1 is: HAL audio
> input, `AudioInputNode`, `RecorderNode` (one default instance, always
> recording, wired to the mix bus), the capture-transfer plane, capture
> app-ops (manual / threshold-arm / retroactive), the SAMPLING screen on
> Theotokos, and the `FUNC+9` transition-trick chord. **Not** AN1: slices
> UI, sample chains, scenes, crossfader (AN2); pickup-style live looping
> (AN3); timestretch DSP (AN3, surface already shipped in AN0); a second
> or third simultaneously-recording `RecorderNode`, or a Theoria SAMPLING
> view (both explicitly out — see §1). AN1 does not touch the default
> `instrument.yaml`'s four tracks; it proves the mechanism through a new
> test-driver fixture, same posture AN0 took with `sampler_chain.yaml`.

---

## §0. Decisions made in this spec (no further design input needed)

| # | Decision | Reasoning |
|---|---|---|
| D1 | Input opens as a **second, independent `cpal::Stream`** (`host.default_input_device()`), not a duplex/shared stream with output. A new cross-thread `rtrb::RingBuffer<f32>` bridges the input callback thread to `AudioInputNode::process()`, which runs on the *output* stream's callback thread inside `NodeExecutor::process()`. | `paraclete-hal/src/audio.rs` today opens output only (`AudioBackend::start`, `audio.rs:83`); cpal has no portable duplex API, so input and output are physically different OS callback threads even on the same interface. This is a genuinely new problem AN0 never had: `OutputRing` (`audio.rs:426`) buffers *within one callback* (executor renders fixed blocks, the same callback drains them to the device) — it is not cross-thread at all. Input needs a real SPSC ring between two independent real-time threads; rtrb push/pop is O(1), no allocation, safe between two audio-callback threads. |
| D2 | The input ring carries raw interleaved samples at the **device's native channel count** (usually 2); `AudioInputNode` downmixes to mono (average across channels) when it drains the ring each `process()` cycle, matching `load_wav`'s existing downmix (`sampler.rs`, cited in `an0-anamnesis.md` §0 D5). Underrun (ring emptier than a block) silence-pads, mirroring `OutputRing::drain`'s guard (`audio.rs:534-537`). | `SampleData` and the whole `Sampler` pipeline are mono-only (AN0 D5, "inherited limitation... revisit when AN1's input path forces the question"). AN1 is that forcing point, and the honest answer is: still don't force stereo end-to-end. Downmixing at the node boundary (not the HAL) keeps `paraclete-hal` a thin transport layer, matching the existing division of labour (HAL moves bytes; nodes/decode interpret them). Stereo capture stays explicitly deferred, now for the second time, deliberately — not silently reintroduced scope. |
| D3 | `AudioError` gains `NoInputDevice` (mirrors `NoOutputDevice`, `audio.rs:630`), but it is **never a startup-fatal error** — a new `try_open_audio_input(&mut NodeConfigurator) -> Option<InputStreamHandle>` in `main.rs` probes for an input device and returns `None` on any failure (no device, config error, build error), silent on absence (matching `try_open_digitakt`/`try_open_keystep`'s quieter precedent, not `try_open_launchpad`'s — see D4). Only `AudioBackend::start`'s existing **output** path stays fatal (`main.rs:312-320`, `std::process::exit(1)`) — output is non-negotiable for the app to run at all; input is optional hardware, same status as a Launchpad. | Reversed from the original draft after checking `main.rs`'s actual device-opening pattern: every other piece of optional hardware in this codebase degrades gracefully (`Option<T>`, not a `Result` the caller must survive), and there is no reason input should be the one exception — a session without a mic/line-in should boot exactly as it does today, just without the SAMPLING capability (D11 derives that from graph presence, so absence here is self-consistent, not a special case). Return type corrected from an earlier draft's `Option<u32>` to `Option<InputStreamHandle>` — D4 already requires the caller to keep the handle alive for the stream's lifetime, and the node's id is a fixed constant the caller already knows, so there's nothing else worth returning. |
| D4a | **Input/output sample-rate mismatch is not resampled in AN1** — `AudioInputNode` drains its ring 1:1 into output frames at whatever rate the graph runs at (`conf.sample_rate()`, itself set from the *output* device's negotiated rate, `main.rs`). If the default input device's native rate differs from the output device's, captured audio plays back pitch/time-shifted and the cross-thread ring gradually drifts. `try_open_audio_input()` compares the two rates and logs a `log::warn!` on mismatch (review finding) — this is the full extent of AN1's handling; no resampler is added. | Building a real-time-safe rate converter is disproportionate scope for "get input into the graph" (C1) — it's a full DSP algorithm, not a wiring decision, and would need its own design pass (buffer-size implications, quality/latency tradeoffs) the way AN3's timestretch already gets one. Most audio interfaces run input and output at one shared hardware rate in practice, so this is a real but narrow gap, not a common-case defect — following AN0 decision 7's "surface frozen now, DSP staged later" precedent (`play_mode`'s `stretch` variant), silently doing nothing would be the actual mistake; logging it plainly is the proportionate response until a later phase (or a defect, if a session ever actually hits it) justifies the resampler. |
| D4 | New node: **`AudioInputNode`** (`crates/paraclete-nodes/src/audio_input.rs`), id **61** (adjacent to `AudioOutput`'s 60 — both are HAL-facing endpoints). One output port, no input port. `process()` drains its ring consumer into the output buffer, mono. **Not registered in `NodeRegistry`, not YAML-declarable.** `try_open_audio_input()` (D3) opens the input `cpal::Stream` in `paraclete-hal`, gets back `(InputStreamHandle, rtrb::Consumer<f32>)`, constructs `AudioInputNode::new(consumer, channels)` directly (both returned by `InputStreamHandle::start()`; `channels` is what D2's downmix needs — implemented as a real constructor argument, not left implicit), and calls `conf.add_node(ID_AUDIO_INPUT, Box::new(node))` in `main.rs`'s hardware-device block (§4, alongside the Launchpad/Digitakt/Keystep calls) — **before** `conf.build_executor()`, same ordering those already use. `InputStreamHandle` is kept alive next to `main.rs`'s existing `_audio` binding for the stream's lifetime. | Corrects a real conflict in the first draft: `NodeRegistry::register()`'s constructors are `Fn() -> Box<dyn Node>` — genuinely zero-argument (`registry.rs:26-30`), needed so ADR-029's pause-rebuild-resume and project reload (`project.rs:310`) can reconstruct any node from just its type_tag string. `AudioInputNode` cannot satisfy that: it needs a live `rtrb::Consumer<f32>` that only exists once a real input stream is open, and that can't come from nowhere on a project reload. `paraclete-nodes` already depends on the plain `rtrb` crate directly (`paraclete-nodes/Cargo.toml`, not on `paraclete-hal` — the five-layer rule stays intact), so the type itself is fine to hold a `Consumer<f32>`; the fix is in *who constructs it and when*, not the type. This is exactly the shape `ScriptingGatewayNode`s for Launchpad/Digitakt/Keystep already use (`main.rs:169-188`, `conf.add_node(ID_GW_LP, ...)`) — added directly with real arguments, never through the zero-arg registry. Consequence, verified against `project.rs`: nodes added this way never get a `type_tag` (`set_type_tag_for` is called only from `builder.rs:101` and `project.rs:316`, never from `main.rs`'s hardware block), so on save they serialize with an empty `type_tag` and on load they hit the existing "no type_tag; cannot reconstruct — skipping" path (`project.rs:303-309`) — the same graceful, already-tested degradation the gateway nodes get today. No new exclusion logic needed; `AudioInputNode` inherits it for free by following the same construction path. |
| D5 | New node: **`RecorderNode`** (`crates/paraclete-nodes/src/recorder.rs`), id **210** (fresh block, past the highest existing id 200/`ReverbNode` — no free single id near an existing family fits its "one instance, arbitrary future count" shape the way 61 fit `AudioInputNode`). One signal input, no output obligation (ADR-040 decision 2, verbatim: "a node with one signal input and no output obligation"). Owns a **plain pre-allocated circular buffer** (`Box<[f32]>`, overwrite-oldest), written every `process()` cycle from its input — no ring, no cross-thread structure for the write path; this is in-place indexing on the audio thread, same shape as any other node's internal DSP state. | ADR-040 review note 2 (post-ratification, user-approved) explicitly respecs the recorder mechanism this way: "rtrb SPSC is consume-forward with fail-on-full — an always-on ring read only at capture is not expressible in it. The recorder owns a plain circular buffer on the audio thread (pre-allocated, overwrite-oldest)." That review note is binding; this decision just implements it. |
| D6 | **AN1 ships exactly one `RecorderNode` instance**, constructed wired to the mix bus (an edge from `MixNode` id 2, or the post-mix tap point the default graph exposes — see §2.1), satisfying R2's transition-trick default ("capture last 2 bars of the **master**") outright with zero source-selection UI required for the frozen requirement. The SAMPLING screen's Source row (design.md §4) still lets a session **re-wire** that one recorder's input edge to a different tap (external in / a specific track) via the existing ADR-029 pause-rebuild-resume protocol — a `conf.disconnect()` + `conf.connect()` pair, same ~5 ms silence transient every other topology change already accepts. Running N simultaneous always-on recorders (one per candidate source, so nothing is ever missed regardless of which source gets picked after the fact) is a natural, non-breaking future extension — same `NodeRegistry` type-tag mechanism, not foreclosed by anything here — but is not itself required by ADR-040's AN1 row or by R2, and design.md's own OQ table never asks "how many recorders." | "Capture is retroactive" (design.md §1) is about *when* you decide to keep something, not about *how many* signal paths are simultaneously recorded — the single recorder's re-wireable edge already gives every source retroactive capture, just one at a time, which is exactly what a solo performer using one SAMPLING screen actually does (you're looking at one Source row selection when you press capture). Re-wiring on Source-row navigation costing one ADR-029 pause is an honest cost the whole system already pays for every other topology change, not a new category of glitch. Multiple always-on recorders is real RAM (N × the per-recorder ring budget, D8) for a capability nothing in ADR-040 or design.md currently asks for. |
| D7 | `RecorderNode`'s circular-buffer capacity is a **fixed duration in seconds**, not bars: `capacity_seconds: f32`, default **60.0** *(tunable)*, translated to frames at `activate(sample_rate, _)` (`(capacity_seconds * sample_rate) as usize`) — the one and only allocation, main thread, before the audio thread starts. Resolves **OQ-A1**. | Bars are wall-clock-variable (a bar's frame length depends on live BPM, which changes during a session — the sequencer's `InternalClock` already treats tempo as mutable). A ring sized in bars at construction time would need reallocation on tempo change, which is exactly the audio-thread-allocation hazard the whole asset/capture-plane architecture exists to avoid. Fixing the budget in seconds and translating the *UI's* bar options (1/2/4/8 per design.md §4) against the *current* tempo only at the moment of capture keeps the one allocation at `activate()` and never touches it again. 60 s covers design.md's largest listed Retro option (8 bars) down to 30 BPM (8 bars × 4 beats ÷ 30 BPM × 60 = 64 s is the one edge case that *doesn't* fit at the extreme; 60 s comfortably covers 8 bars at any tempo ≥ 32 BPM, which is the practical floor for a rhythmic instrument) with headroom to spare at any tempo a session is likely to run. |
| D8 | The capture-drain leg (RecorderNode → main thread, the actual captured PCM) is **not** a chunked/streaming transfer despite ADR-040 review note 2's "over subsequent blocks" phrasing. Because capture is retroactive, the requested span *already exists* in the always-on ring the instant the capture command lands — assembly is a single, synchronous, in-`process()` copy (with ring-wraparound handling, same shape as `OutputRing::drain`, `audio.rs:476-493`) into one of a small pool of **pre-allocated fixed-capacity scratch buffers** (`Box<[f32]>`, sized to the ring's own capacity as a ceiling — never smaller than what a single capture could need), which is then **moved** (not `Arc`-wrapped — see D9) across a dedicated rtrb ring to the main thread. "Over subsequent blocks" describes how the *source* ring filled up (true, over the preceding several seconds of always-on recording), not how the drain-out happens. | Re-deriving from design.md §4's own feature list: "Retro row: capture last 1/2/4/8 bars" and "threshold... pre-roll (from the ring — the ring means threshold capture *includes* the transient)" are both exclusively backward-looking — nothing in AN1's spec surface asks for a forward "keep recording after the trigger for N more bars" mode (that shape, if ever wanted, is a straightforward AN3-class pickup-loop question, not an AN1 one). A single-shot copy is simpler, is provably RT-safe by the same `debug_assert`-backed reasoning AN0 already used for D7's return-ring stress test, and avoids inventing a streaming protocol AN1's actual UI never asks for. |
| D9 | Capture payload delivery is a **new, dedicated rtrb ring pair** — `recorder_capture_producer`/`consumer` (audio → main, payload `RecorderCapture { request_id: u32, frames: usize, buf: Box<[f32]> }`) and `recorder_capture_return_producer`/`consumer` (main → audio, payload `Box<[f32]>`, the drained buffer handed back for reuse) — modeled directly on the AN0 asset plane's shape (`asset_swap_producer`/`consumer` + `asset_return_producer`/`consumer`, `configurator.rs:188-194`) but using a **plain owned `Box<[f32]>` move, not `Arc<SampleData>`**. | AN0's `SwapSample` needs `Arc` because a decoded sample is potentially shared by many `Sampler` nodes reading the same pool slot (D4 in `an0-anamnesis.md`: `node_id`-unicast but the *data* can be cloned to many targets). A capture chunk has exactly one producer (the one `RecorderNode`) and exactly one consumer (the app assembling it into a pool entry) — ownership genuinely transfers each direction, so a plain `Box` move is correct and simpler; wrapping it in `Arc` would add a refcount nothing ever needs to share. The *return* leg exists for the identical reason AN0's does: the audio thread must never `free()` on its own thread (hard constraint 1), so the drained scratch buffer's deallocation-avoidance is the return ring's whole job, exactly like `state_buf_return`/`asset_return`. |
| D10 | `RecorderNode` reads its own small command vocabulary directly out of `input.commands: &[NodeCommand]` inside `process()` (no `ParameterBank` — its state is two scalars, not a param bank). New node-specific `type_id`s (≥ 16, per-node-type namespaced, `command.rs:6-7`): `CMD_RECORDER_ARM_THRESHOLD = 16` (`arg1` = threshold dB), `CMD_RECORDER_DISARM = 17`, `CMD_RECORDER_CAPTURE = 18` (`arg0` = `request_id` cast to `i64`, `arg1` = requested frame count as `f64`, converted to `usize` and clamped to ring capacity). | `NodeCommand`'s 24-byte fixed-width shape (`command.rs:9-17`) already fits this exactly — a threshold or a frame count is a plain scalar, not heap data, so this traffic belongs on the existing command plane, not a new one. `type_id` numbering restarts at 16 per node type (Sequencer already owns 16–44 in its own dispatch; `RecorderNode`'s `process()` is a separate match arm entirely, so no collision is possible — CLAUDE.md's Sequencer command table and this one describe two different nodes' private vocabularies). |
| D11 | The SAMPLING capability gate is **derived from graph presence**, not app-declared: the app main loop (alongside the existing `/context/perform` publish, `perform_state.rs:76-78`) computes `sampling_available = conf.contains_node(ID_AUDIO_INPUT) || conf.all_nodes().any(|(id, _)| conf.type_tag_for(id) == Some("recorder"))` once per tick and writes it to a new StateBus path, **`/context/sampling_available`** (`StateBusValue::Float`, 1.0/0.0 — same encoding `/context/perform` already uses, `lib.rs:1458`, `lib.rs:5608`). Theotokos's `PanelButton::Sampling` dispatch (`input.rs:964-967`, currently a hard-coded `Action::Noop` — see D12) reads this the same way it already reads `/context/perform`. Resolves the mechanism ADR-040 review note 3 flagged as missing ("cap-docs are node-produced only... the app declares a cap-doc named a mechanism that doesn't exist"). | `CapabilityDocument` (`node.rs:41`, `capability.rs:194`) is per-node and answers "what can *this* node do," not "does *the instrument* have a sampling story" — there is no existing cross-node aggregate capability concept to extend, and inventing one would be new architecture ADR-040 didn't ask for. The StateBus is already the load-bearing channel for exactly this class of instrument-wide derived fact (`/context/perform`, `/context/kit_binding`) — reusing it needs no new plumbing, just a new path and a new read site, matching the review note's own correction verbatim ("derived... delivered via config/bus"). The check can't uniformly use `type_tag_for` (D4's correction: `AudioInputNode` never gets a type_tag, since it's never built through `NodeRegistry`) — `contains_node(ID_AUDIO_INPUT)` (`configurator.rs:387`) checks the fixed id directly instead, which is exactly as cheap and doesn't need one. |
| D12 | The `sampling_hidden_without_capability` test (`lib.rs:5486-5505`) is **replaced**, not kept alongside new tests — it currently asserts key `9` is a pure no-op unconditionally, which becomes false the moment any instrument declares an `audio_input`/`recorder` node. New tests assert both branches: no capability → still a silent no-op (regression-preserved for a Digitone/Syntakt-alike with no recorder); capability present → `Action::OpenScreen(Screen::Sampling)`. | Direct consequence of D11 — the test's docstring ("SAMPLING is hidden entirely unless some capability declares it (none does today)") is a contract this phase's whole point is to change the second half of. Deleting rather than skipping avoids a stale "not yet" test sitting in the suite past the phase that falsifies its premise. |
| D13 | **OQ-A2 resolved: RAM captures freeze to a file at project save**, converting the pool entry from `SampleSource::Ram { frames }` to `SampleSource::File(path)` pointing at a project-relative `captures/slot_{N}.wav`, written from the app-side PCM the pool already needs to hold for node (re)distribution (D14). After the freeze-write, the entry behaves as an ordinary File slot in every subsequent save/load — no lingering `Ram`-vs-`File` special-casing anywhere else in the codebase. | `an0-anamnesis.md` never actually closed OQ-A2 despite `design.md`'s OQ table crediting it to "AN0 spec" — AN0 only ever constructed `SampleSource::File` (D3 in that spec says so explicitly), so there was nothing to freeze yet. AN1 is the first phase that produces real `Ram` entries, so it is the phase that must answer this. "Freeze to file" (not "keep volatile," not "ask") is the only option consistent with every other persisted asset in this project (`KitStore`, `SamplePool`'s File entries) surviving a reload — losing a performer's just-captured take on every reload would be a much larger, more surprising regression than the honest "old-format-no-data" precedent AN0's D9 set for a genuinely legacy path; a freshly captured sample is not a legacy path. |
| D14 | `SamplePool` gains a parallel **`ram_cache: HashMap<usize, Arc<Vec<f32>>>`**, holding decoded PCM for `Ram`-sourced slots only (File slots never need it — they already re-decode from `path` on demand, per AN0's `load_project_v3` rehydration path). Populated at capture-assembly time (C4); consulted both for (a) redistributing to newly-connected/rebuilt nodes exactly like `load_file`'s caller already does, and (b) D13's freeze-to-WAV write at save time; the cache entry is dropped once its slot is frozen to `File` (nothing reads it again after that point). | Symmetric gap AN0 never had to close: for a `File` entry, "rehydrate on load" and "get the PCM to write out" both mean "re-read `path`." A `Ram` entry has no `path` — without an app-side authoritative PCM store, both operations (rehydration after a graph rebuild, and D13's freeze-on-save) would be impossible for anything captured, not merely inconvenient. This is the smallest structure that closes the gap without inventing a third pool-entry lifecycle state. |

---

## §1. Scope

**In:**

- HAL input stream (`paraclete-hal/src/audio.rs`): second `cpal::Stream`,
  cross-thread `rtrb` bridge to `AudioInputNode`, `AudioError::NoInputDevice`
  (D1–D3).
- `AudioInputNode` (L3, id 61): ordinary mono signal source draining the
  input ring, silence-on-underrun (D4).
- `RecorderNode` (L3, id 210): one instance, wired to the mix bus at
  construction; pre-allocated circular buffer; threshold envelope
  follower + `armed`/`peak_db` published state; own command vocabulary
  (`CMD_RECORDER_ARM_THRESHOLD`/`DISARM`/`CAPTURE`) (D5–D7, D10).
- Capture-transfer plane: dedicated rtrb ring pair, `RecorderCapture`
  message, pre-allocated scratch-buffer pool with return-ring reuse,
  RT-safety proven by test mirroring AN0's D7 stress test (D8–D9).
- `NodeRegistry` entry: `"recorder"` type tag (ADR-029 pattern) so
  `RecorderNode` can be declared from `instrument.yaml` and reconnected
  at runtime. `AudioInputNode` is **not** registry/YAML-based — it is
  opened in `main.rs` alongside the other optional hardware (D4).
- `SamplePool` extensions: `SampleSource::Ram` fully populated (frames
  already declared in AN0; `ram_cache` added — D14), a
  `record_capture(...)` method assembling a `RecorderCapture` into a new
  pool slot, and the save-time freeze-to-WAV path (D13).
- `AppOp` additions: `AppOp::RecorderArmThreshold { recorder_id, db }`,
  `AppOp::RecorderDisarm { recorder_id }`, `AppOp::RecorderCapture
  { recorder_id, bars: f32 }`, `AppOp::SetRecorderSource { recorder_id,
  source_id }`, `AppOp::CaptureTransition { recorder_id, sampler_id }`
  (the `FUNC+9` chord's single composite op — captures 2 bars, assigns
  to the pool, delivers `SwapSample` to `sampler_id`, sets a 16-slice
  grid, all in one drain-loop turn). Drained the same way
  `AppOp::PoolLoadFile` already is (`app_ops.rs:18-32`,
  `perform_state.rs::execute()`).
- `/context/sampling_available` StateBus publish (D11), main-loop poll
  driving threshold auto-capture (the app watches `/node/{id}/state/armed`
  the same tick it watches everything else and issues
  `AppOp::RecorderCapture` itself when it flips — no node ever commands
  the app or another node, ADR-018).
- Theotokos: `Screen::Sampling` variant, capability-gated dispatch
  (D11–D12), a `render_sampling_screen` module (Source row / Arm row /
  Monitor toggle / Retro row / post-capture preview+trim+assign, per
  design.md §4), the `FUNC+9` chord (`PanelButton::Sampling if
  mods.func`, placed before the bare arm exactly like the existing
  `PanelButton::Kit` pair, `input.rs:962-963`).
- Test-driver fixture proving the mechanism end-to-end (mirrors
  `sampler_chain.yaml`), same posture as AN0 — the default
  `instrument.yaml` is not touched.

**Out, with a citation:**

- **A second or further simultaneous `RecorderNode` instance** — D6
  explicitly defers this; nothing here forecloses it, the `NodeRegistry`
  type-tag mechanism already generalizes.
- **Slices UI, sample chains, scenes, crossfader** — AN2 (ADR-040 phase
  table; `design.md` §2 feature-map rows). AN1's capture path writes a
  16-slice grid as part of the `FUNC+9` composite (R2's literal
  requirement) but does **not** ship a slice-editing UI — that default
  grid is written once and is otherwise inert until AN2.
- **Pickup-style live looping, overdub, BPM-locked stretch playback
  DSP** — AN3 (`design.md` §6, ADR-040 decision 7 / OQ-A8). AN0 already
  shipped the `play_mode` surface; AN1 adds no DSP behind `stretch`.
- **A Theoria (web) SAMPLING view** — Theotokos-first, matching every
  other Elektron-convergence screen's rollout order (`ADR-038`); no
  `SamplingView.tsx` exists and none is added here. Cited the same way
  AN0 cited "no Theoria pool browser."
- **Cue outputs / multi-out device routing** — OQ-A7, unscoped, later.
- **Any change to the default `instrument.yaml`'s four tracks** — a
  scope decision about the shipped default instrument's sound, not an
  AN1 architecture question, exactly the posture AN0's own §1 took for
  wiring a `Sampler` into the default graph.
- **Custom LFO shapes for capture-related modulation, MOD-page
  interaction** — OQ-A6, unrelated to this phase, P12+.

---

## §2. Architecture notes

### §2.1 HAL input stream and the cross-thread bridge

`AudioBackend::start_with_callback` (`audio.rs:223`) already demonstrates
the pattern AN1's input side reuses almost verbatim for opening a device
and building a typed stream — but for **input**, not output:
`device.build_input_stream::<f32, _, _>(&config, move |data: &[f32], _| {
...}, err_fn, None)`. The callback's only job is to push `data` into the
producer half of a pre-allocated `rtrb::RingBuffer<f32>` (capacity: a few
blocks' worth, mirroring `RING_BLOCKS = 4`, `audio.rs:541`) — on
`PushError::Full`, drop the oldest-arriving samples silently (log a
counter, don't block; the ring being full for more than a few blocks
means the executor thread has stalled, which is already an
executor-thread-hang scenario the rest of the system doesn't specially
handle either).

`AudioInputNode::process()` pops what it can from the consumer half —
exactly `block_size` mono frames after downmixing — and silence-pads any
shortfall, the same underrun posture `audio_callback_f32` already takes
(`audio.rs:534-537`) for the *output* side. Ring construction and the
`rtrb::Producer`/`Consumer` split happen inside `try_open_audio_input()`
(`main.rs`'s hardware-device block, D4) — **not** at YAML graph-build
time (`builder.rs`), unlike the AN0 asset plane's rings, which are
built once in `NodeConfigurator::new()` (`configurator.rs:239-241`) for
*every* graph regardless of content. The input ring only comes into
existence at all when real input hardware is actually found; the
producer half stays with the HAL's `InputStreamHandle`, the consumer
half moves directly into the `AudioInputNode::new(consumer, channels)`
call that follows in the same function.

### §2.2 RecorderNode internals

```
struct RecorderNode {
    ring: Box<[f32]>,       // capacity_seconds * sample_rate, D7
    write_pos: usize,
    total_written: u64,
    armed: bool,
    threshold_db: f64,
    envelope: f32,          // one-pole peak follower
    scratch_pool: Vec<Box<[f32]>>,  // D9 — pre-allocated at activate()
    scratch_free: Vec<usize>,       // indices into scratch_pool currently free
}
```

`process()` each cycle: (1) write the input block into `ring` at
`write_pos`, wrapping; (2) update `envelope` (one-pole: `envelope +=
(peak - envelope) * COEFF`); (3) if `armed && envelope_db >= threshold_db`,
flip an internal `just_triggered` flag (published next as
`/node/{id}/state/armed` going `false→true` is what the app watches for
— see §2.4) and clear `armed` (one-shot per arm, matching design.md's
"arm" verb — re-arming is an explicit app-op, not automatic); (4) drain
`input.commands` for `CMD_RECORDER_ARM_THRESHOLD` / `_DISARM` /
`_CAPTURE`; (5) on `_CAPTURE`, copy the last `arg1` frames out of `ring`
(wraparound-aware, same shape as `OutputRing::drain`) into a scratch
buffer popped from `scratch_free` — if none is free, the command is
**not** requeued or blocked on; it is dropped with a
`runtime_counters`-style dropout increment, and the app is expected to
retry (a session pressing capture twice in the same millisecond before
the first has drained is not a case worth adding complexity for); (6)
push the filled `RecorderCapture` onto `recorder_capture_producer`
(`PushError::Full` → hold the scratch buffer, retry next cycle, same
backpressure idiom as AN0's D7); (7) drain
`recorder_capture_return_consumer` each cycle, pushing any returned
buffer's index back onto `scratch_free`.

`published_state()` pushes `/node/{id}/state/armed` (Bool-as-Float) and
`/node/{id}/state/peak_db` (Float) every cycle — the SAMPLING screen's
live input meter reads the latter directly off the bus, no new
plumbing.

### §2.3 Capture assembly and the transition trick

`perform_state.rs::execute()` gains an arm for `AppOp::RecorderCapture`
(and the `CaptureTransition` composite) that: (1) issues the
`CMD_RECORDER_CAPTURE` `NodeCommand` via `conf.send_command()` (existing
path, `command.rs`/`configurator.rs`, same as every other node command);
(2) on the *next* main-loop tick where `recorder_capture_consumer.pop()`
yields a `RecorderCapture` matching the outstanding `request_id`, calls a
new `SamplePool::record_capture(&mut self, samples: &[f32], rate: f32,
bars: Option<f32>) -> (usize, Vec<f32>)` (mirrors `load_file`'s
`(&PoolEntry, Vec<f32>)` return shape, `sample_pool.rs:65`), pushes the
result into `ram_cache` (D14), and distributes it via `SwapSample`
exactly like `AppOp::PoolLoadFile`'s handler already does
(`perform_state.rs:171`) — for `CaptureTransition`, additionally: sets a
16-slot even slice grid on the new entry (`slice_table`, R2's literal
default) and targets the `SwapSample` at the caller-supplied
`sampler_id`.

`request_id` matching handles the one genuine race the split (main
thread issues command; audio thread fulfils it one or more ticks later)
introduces: `PerformState` keeps a small `pending_captures:
HashMap<u32, PendingCapture>` (request context — which sampler to assign
to, whether it's a transition-trick composite) keyed by a
monotonically-incrementing `request_id`, cleared on match. No timeout is
needed — a capture command that never completes (recorder removed from
the graph mid-flight, an edge case ADR-029's rebuild already handles by
simply not having the node anymore) just leaves an inert, harmless
`HashMap` entry; this mirrors the existing tolerance for "undelivered
messages simply stay queued" language already accepted for
`drain_asset_swaps` (`executor.rs:461`).

### §2.4 Threshold auto-capture

The app main loop, at the same point it already reads `/context/perform`
(`main.rs:512-515`), reads each live recorder's `/node/{id}/state/armed`.
A `false → true` transition observed on a tick is the app's cue to issue
`AppOp::RecorderCapture` itself, using whatever pre-roll span the
SAMPLING screen's threshold-arm mode was configured with — **OQ-A3
resolved: pre-roll default 500 ms** *(tunable)*, chosen because it's
comfortably larger than any plausible transient attack time without
eating meaningfully into the 60 s ring budget (D7). This keeps
`RecorderNode` itself fully passive about *deciding* to capture — it only
ever reports state; the app decides and acts, the same division of
labour `/context/perform`'s existing "app reads state, app acts" pattern
already establishes, and the one ADR-018 already requires ("nodes cannot
command nodes" extends cleanly to "nodes cannot command the app").

### §2.5 SAMPLING screen

Follows `Screen::Kit`'s shape (app-owned store + dedicated `Screen`
variant + own render/input module), not the per-node `Rule`/page-group
system (`rule.rs:24`) — SAMPLING has no single owning node the way SRC/AMP
pages belong to a `Sampler`; it is instrument-wide state, exactly the
reasoning AN0's D1 already used to model `SamplePool` on `KitStore`
rather than inventing a new persistence idiom. `render_sampling_screen`
(new function in `render.rs`, registered in the `Screen::Sampling =>` arm
of the existing screen-dispatch match, `render.rs:240`) draws: Source row
(reads the recorder's currently-connected input edge — a new
`/context/recorder_source` StateBus text field, same encoding style as
`/context/kit_binding`), Arm row (manual/threshold — `z`-style toggle,
modeled on `RecMode`'s `Off↔Grid` toggle, `model.rs:88-92`,
`lib.rs:867-883`), Monitor toggle, Retro row (bar-count selector), and
after a capture: preview/trim/assign per design.md §4 (trim handles are
a stretch goal within this commit — if time-boxed out, ship
preview+assign+discard first and file trim as a defect, not a silent
scope cut, per CLAUDE.md guardrail 5).

---

## §3. Commit plan

**C0 — HAL input stream.** `paraclete-hal/src/audio.rs`: input `cpal::Stream`
open + cross-thread `rtrb` ring (§2.1), `AudioError::NoInputDevice`,
`query_input_sample_rate()` (mirrors `query_sample_rate()`, `audio.rs:61`).
Tests: ring push/pop round-trip without a real device (stand-in producer/
consumer, same style as `asset_plane_tests.rs`); underrun silence-pad.
No node yet — this commit is pure HAL plumbing and must build standalone
(no caller exists until C1).

**C1 — `AudioInputNode`.** `crates/paraclete-nodes/src/audio_input.rs`, id
61, constructed with an `rtrb::Consumer<f32>` and the input device's
channel count (`AudioInputNode::new(consumer, channels)` — both values
come from `InputStreamHandle::start()`'s return, D4).
`main.rs`: `try_open_audio_input()` (D3–D4), added to the hardware-device
block (§4) alongside `try_open_launchpad`/`try_open_digitakt`/
`try_open_keystep`, `conf.add_node(ID_AUDIO_INPUT, ...)` before
`conf.build_executor()`. **No `NodeRegistry` entry, no `instrument.yaml`
grammar** — D4 explains why a zero-argument registry constructor can't
carry a live consumer. Tests: downmix correctness (stereo → mono
averaging), silence-on-underrun; a `main.rs`-level test isn't practical
(no headless input device in CI) — covered instead by a unit test on
`AudioInputNode` alone, constructed with a hand-fed `rtrb::Producer` half
standing in for the HAL callback (same style as `asset_plane_tests.rs`).

**C2 — `RecorderNode`, no capture drain yet.** `crates/paraclete-nodes/src/recorder.rs`,
id 210, `"recorder"` tag. Circular buffer write path, threshold envelope
follower, `armed`/`peak_db` published state, `CMD_RECORDER_ARM_THRESHOLD`/
`_DISARM` command handling. **No `CMD_RECORDER_CAPTURE` handling yet** —
that needs the capture-transfer plane (C3), kept as its own commit the
same way AN0 split C1 (asset plane) from C2 (Sampler pool-indexing) even
though they're related. Tests: write-wraparound correctness, envelope
crossing flips `armed`, RT-safety (`process()` never allocates —
`debug_assert`/allocation-counting harness if one exists, otherwise a
loom/miri-style manual audit note, matching AN0's own testing posture
for D7).

**C3 — Capture-transfer plane.** New rtrb ring pair in
`configurator.rs`/`executor.rs` (`recorder_capture_producer`/`consumer`,
`recorder_capture_return_producer`/`consumer`, D9), `RecorderCapture`
struct in `paraclete-node-api`'s `asset.rs` (sibling to `SwapSample`),
`RecorderNode::process()` wired to fulfil `CMD_RECORDER_CAPTURE` (D8,
§2.2 step 5–7). Tests: capture-then-drain round-trip within one
`process()` cycle at the point of the command; return-ring stress test
mirroring `asset_plane_tests.rs`'s D7 test (sustained capture requests
under return-ring pressure never drop a scratch buffer, never allocate
on the audio thread past `activate()`).

**C4 — `SamplePool` capture assembly + freeze-on-save.** `sample_pool.rs`:
`ram_cache` (D14), `record_capture()`, save-path freeze-to-WAV (D13,
writing into a new `captures/` project subdirectory using the same
decode/encode utility `decode_file` already pairs with — a plain WAV
writer, not a new dependency if one already exists for
`tools/gen-samples`; otherwise a minimal one). `perform_state.rs`:
`pending_captures` map, `AppOp::RecorderCapture`/`RecorderArmThreshold`/
`RecorderDisarm`/`SetRecorderSource` handlers (§2.3). Tests: capture →
pool slot → `SwapSample` delivery integration test (mirrors AN0's
`PoolLoadFile` integration test, `perform_state.rs` tests); save/reload
round-trip proving a `Ram` slot survives as a `File` slot after one
save (no silent data loss); `ram_cache` eviction after freeze.

**C5 — Theotokos SAMPLING screen + `FUNC+9`.** `model.rs`: `Screen::Sampling`
variant. `input.rs`: replace the D12 stub — capability-gated dispatch via
`/context/sampling_available` (D11), `PanelButton::Sampling if mods.func
=> Action::CaptureTransition` **before** the bare arm (mirrors
`PanelButton::Kit`'s ordering, `input.rs:962-963`), delete/replace
`sampling_hidden_without_capability` (D12). `render.rs`:
`render_sampling_screen` (§2.5). `lib.rs`: `AppOp::CaptureTransition`
dispatch wiring (the `FUNC+9` composite, R2). Tests: capability-gated
visibility both ways (D12); `FUNC+9` end-to-end via a test-driver-style
harness driving a real graph with an `AudioInputNode`+`RecorderNode`+
`Sampler` (new fixture, mirrors `sampler_chain.yaml`) through
`NodeExecutor::process()`, asserting a pool slot exists, a `Sampler`'s
`pool_cache` received it, and the slice table has 16 entries — this is
R2's literal acceptance test, automatable even though the *session* gate
(§ header) is what actually closes the phase.

Each commit: `cargo test --workspace` green, `cargo clippy --workspace`
diffed against baseline, code review via the `code-reviewer` subagent
before the next commit starts — same discipline AN0's report documents
(five review passes, all findings fixed pre-commit).

---

## Exit criteria

| # | Criterion | Notes |
|---|-----------|-------|
| 1 | `cargo test --workspace` green after every commit | |
| 2 | `cargo clippy --workspace` clean on touched crates (diff against baseline) | |
| 3 | Each commit carries unit tests for its new logic | |
| 4 | Input-ring and capture-transfer-ring RT-safety proven under stress (C0, C3), mirroring AN0's D7 test | mutation-checked where AN0's equivalent tests were |
| 5 | Code review on each commit or logical batch | |
| 6 | The `FUNC+9` transition-trick chord works end to end against a real graph (C5's automated fixture) | necessary but not sufficient — see 7 |
| 7 | **⛔ User-paired sampling session** verifies the transition trick and SAMPLING screen by ear/hand (ADR-040 R2) | **This is the phase-closing gate.** Unlike AN0, AN1 does not close on report strength alone. |
| 8 | OQ-A1, OQ-A2, OQ-A3 resolved | D7, D13, D14, §2.4 |

## Deferred items (unchanged scope, carried forward)

See `design.md` §2's feature map and ADR-040's phase table. AN2: slices,
chains, scenes, crossfader (needs P11's KitStore, already shipped, plus
AN2's own spec). AN3: pickup loops, stretch DSP, transition polish.
Multiple simultaneous recorders, Theoria SAMPLING view, cue outputs: not
scheduled, not foreclosed (D6, §1).

## Gate

AN1 does not close on this report existing — unlike AN0, the roadmap's
sampling-session gate (ADR-040 R2) sits at **this** phase's exit. Once
C0–C5 land, `cargo test --workspace` is green, and clippy is clean, the
next step is scheduling the paired session, not writing an
`an1-report.md` and moving on.
