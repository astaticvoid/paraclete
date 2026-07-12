# ADR Latent Issue Review — Investigation Plan

**Date:** 2026-07-11
**Scope:** Retroactive hostile review of ADR-001 through ADR-031
**Purpose:** Identify latent design issues to investigate with the debug harness
(ADR-033), ranked by severity. Not a redesign — a testing agenda.

## Design claims invalidated

Three ADR claims were found to be architecturally wrong, not just buggy:

| ADR | Invalidated claim | Why |
|---|---|---|
| **025** | "The audio engine need not be stopped to load a project" | Main-thread `deserialize()` writes step/pattern arrays while the audio thread reads them. Data race. |
| **025** | `deserialize()` before `activate()` | Contradicts AGENTS.md and ADR-019: `activate()` resets the bank; `deserialize()` must come AFTER to restore saved values. |
| **027** | `Arc<PluginLibrary>` protects audio-thread borrow | Arc tracks reference count; when the last `PluginNode` is dropped on the main thread, `deinit()` unloads the shared library. The audio thread may still be executing code in those pages. SIGSEGV. |

These are not implementation bugs — the design is wrong at the claim level.
ADR-025 needs its "hot-reload" claim removed or qualified with a pause
requirement (matching ADR-029's `apply_patch` protocol). ADR-027 needs a
deferred-drop mechanism or explicit "stop audio before unloading."

## Investigation priority list

Ranked by severity × likelihood × harness testability. The debug harness
(ADR-033 interactive mode) can test items marked **[harness]**.

### Critical — investigate first

| # | ADR | Issue | Test with harness |
|---|---|---|---|
| 1 | **025** | `deserialize()` ordering: is it called before or after `activate()`? The ADR says before; AGENTS.md says after. One is wrong. | Save project with non-default params. Load. Read every param via `{"cmd":"read"}`. Flag any at default. |
| 2 | **025** | Hot-reload data race: main thread calls `deserialize()` while audio thread reads step data. | Load project during active playback. Check for torn step values (pattern length != actual steps played). Run under ThreadSanitizer if available. |
| 3 | **024** | `translate_transport()` emits `GlobalStart` every buffer. | Load instrument with sequencer. Start playback. Poll `{"cmd":"read","path":"/node/10/state/current_step"}`. Assert it advances past step 0. **[harness]** |

### High — investigate with harness

| # | ADR | Issue | Test with harness |
|---|---|---|---|
| 4 | **029** | Failed patch: if middle entry fails, graph stranded paused. | Send a patch with a cycle-forming `AddEdge` in position 3. Verify the engine resumes (either with old state or error-reported). **[harness: interactive command + probe]** |
| 5 | **029** | `cap_doc_cache` stale on add/remove cycle. | Add node, read cap doc, remove node, add different node at same ID, read cap doc. Assert old cap doc not returned. **[harness: add/remove + dump]** |
| 6 | **030** | Empty patterns Vec from corrupt deserialize → OOB panic. | Send `{"cmd":"read","path":"/node/10/state/active_pattern"}`. If 0 with zero patterns, trigger a step advance. Assert no panic, silence or error. **[harness: read + trigger after load]** |
| 7 | **007** | Rhai infinite loop starves main loop. | Write a script with `while true {}`. Watchdog should detect >5ms stall and abort the script. Test: does the main loop recover? **[harness: set_param after script load]** |
| 8 | **013** | Event sort unstable at same offset (BUG-026). | Send two `trigger` commands in same buffer. Verify note order preserved. **[harness: trigger ×2 at same at:**] |
| 9 | **018** | SurfaceOutputHandle SPSC overflow under stall. | Send rapid LED bursts while pausing main loop for 5ms. Read back LED state after. Assert no dropped messages. **[harness: stress + probe]** |
| 10 | **023** | GraphNode inner ParamLock silently fails. | Wrap a synth in a GraphNode. Send `ParamLockEvent` targeting the inner cutoff from the outer graph. Verify the inner voice's cutoff changes. **[harness: set_param on inner node ID → read inner param]** |
| 11 | **030** | `active_pattern` OOB from v3 deserialize. | Deserialize blob with pattern count=2, active_pattern=7. Assert graceful error, no panic. **[harness: save/load corrupt project → read state]** |
| 12 | **031** | State mirror permanently diverges on dropped diff. | Drop every 7th state diff. After 1000 updates, compare client mirror to engine state. Assert divergence detected. **[harness: dump vs client state]** |

### Medium — investigate when harness is mature

| # | ADR | Issue | Test with harness |
|---|---|---|---|
| 13 | **008** | Sample rate change without reactivate → stale DSP. | Simulate rate change mid-session. Poll params, verify no buffer overrun. **[harness: stress]** |
| 14 | **009** | Clock domain handoff glitch. | Toggle priority clock source on/off mid-bar. Count transport events. Flag duplicates or gaps. **[harness: probe transport events]** |
| 15 | **011** | Same-level node execution order non-deterministic. | Create two sibling nodes pushing incrementing counters. Run 100 cycles. Assert deterministic order. **[harness: read counter values]** |
| 16 | **012** | Output buffers not zeroed between cycles. | Create a node that writes only on cycle 1, skips cycle 2. Assert cycle 2 downstream sees zero. **[harness: peak measurement across cycles]** |
| 17 | **014** | Parameter lock suspension list unbounded growth. | Simulate 10k preset changes on locked node. Measure memory. **[harness: memory probe]** |
| 18 | **015** | Connection reconciliation for conflicting space_ids. | Two nodes both claim space_id=0 for different events. Assert recon assigns distinct IDs. **[harness: read negotiated agreements]** |
| 19 | **019** | ParamLock bleed: third-party node using bank.handle_commands for locks. | Write mock node that bleeds. Assert next unlocked step shows base value, not locked value. **[harness: probe params across steps]** |
| 20 | **021** | Launchpad SysEx LED update — 328 bytes across 64-byte USB MIDI. | Send full-frame LED update, read back each pad color. Assert no truncation. **[hardware required — not harness-testable]** |
| 21 | **022** | CLAP state truncation at host buffer boundary. | Node with 500 params, serialize, pass through CLAP host with configurable buffer. Assert truncation detected. **[harness: render + probe]** |
| 22 | **024** | CLAP state deserialize during active audio processing. | Load state while audio running. Check for parameter value glitches. **[harness: set_param + read during load]** |
| 23 | **026** | `publish_context` and `send_cmd` deliver on different paths → TUI lag. | Turn encoder rapidly, capture TUI frames. Assert label and value match. **[harness + TUI probe]** |
| 24 | **028** | LoopBreakNode → non-cycle consumers get wrong-cycle data. | Connect LoopBreakNode to one cycle node and one non-cycle node. Compare sample values. Assert same buffer cycle. **[harness: peak/timing comparison]** |
| 25 | **029** | Concurrent `apply_patch` from multiple threads. | Trigger two patches simultaneously. Assert exactly one succeeds, other errors. **[harness: stress]** |
| 26 | **029** | Clock continues ticking during pause → sequencer jumps forward on resume. | Pause 50ms at 120 BPM. Resume. Assert step position accounts for elapsed ticks or resets cleanly. **[harness: read current_step before/after pause]** |
| 27 | **030** | Speed multiplier × swing composition: scaling before or after swing? | Set swing=0.5, speed=2.0. Measure inter-step intervals. Assert behavior matches documented rule. **[harness: timing measurement]** |
| 28 | **030** | Cue vs chain precedence when both active. | Set both cue and chain. Assert deterministic resolution. **[harness: set cue + chain → probe pattern index]** |
| 29 | **031** | JSON f64 precision round-trip between in-process and WS. | Set param to 0.123456789. Read from TUI channel and WS. Assert bit-identical. **[harness: dump vs client mirror]** |
| 30 | **031** | Protocol version mismatch handling. | Connect with `protocol: 999`. Assert clean rejection, no panic. **[harness: WS connect with bad version]** |

### Low — document, defer investigation

| # | ADR | Issue |
|---|---|---|
| 31 | 004 | CvSignal stubbed — behavior contract undefined |
| 32 | 016 | Song mode pattern chain drift with heterogeneous lengths |
| 33 | 022 | Machine bank subgraph requires executor-level buffer allocation |
| 34 | 031 | 20-client SPSC drain scaling in main loop |

## Investigation protocol for debug harness

For each item above, the standard investigation loop is:

```
1. Write a minimal test YAML that isolates the concern
2. Run in batch mode: cargo run -p test-driver -- test.yaml
3. If batch passes/fails as expected, move on
4. If batch is ambiguous, use interactive mode:
   - cargo run -p test-driver -- -i instrument.yaml --interactive
   - Send commands: set_param, trigger, read, dump, peak
   - Observe stdout for unexpected values
   - Repeat with timing variations (at: offsets, trigger spacing)
5. Record result: confirmed / not-reproducible / needs-code-fix / design-invalidated
```

Items marked **[harness]** are directly testable with ADR-033. Items marked
**[harness: stress]** require extended runs or memory probes. Items marked
**[hardware required]** need physical devices.
