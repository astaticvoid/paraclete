# ADR Index

Machine-readable mapping from ADR numbers to files and topics.
Maintained for agentic tooling — code comments reference ADRs by number
but cannot resolve them without this index.

| ADR | File | Topic |
|-----|------|-------|
| ADR-001 | `ADR-001-license.md` | GPL3+LGPL3 L2 license boundary |
| ADR-002 | `ADR-002-language.md` | Rust implementation language |
| ADR-003 | `ADR-003-plugin-format.md` | CLAP plugin format |
| ADR-004 | `ADR-004-signal-model.md` | Mixed-rate typed port signal model |
| ADR-005 | `ADR-005-scheduler-cycles.md` | Scheduler must support cycles from P0 |
| ADR-006 | `ADR-006-hardware-emulator.md` | Hardware emulator required at P1 |
| ADR-007 | `ADR-007-scripting-runtime.md` | Rhai scripting runtime |
| ADR-008 | `ADR-008-node-api-level.md` | Node API — single trait, three implicit levels |
| ADR-009 | `ADR-009-clock-federation.md` | Clock domain federation |
| ADR-010 | `ADR-010-state-model.md` | State model — cybernetic hybrid |
| ADR-011 | `ADR-011-executor-model.md` | Executor — single-threaded push |
| ADR-012 | `ADR-012-buffer-allocation.md` | Buffer allocation — per-connection, main thread |
| ADR-013 | `ADR-013-event-model.md` | Event model — typed enum with extended slab |
| ADR-014 | `ADR-014-capability-document.md` | Capability document — mandatory with default impl |
| ADR-015 | `ADR-015-connection-negotiation.md` | Connection negotiation — agreed at connection time |
| ADR-016 | `ADR-016-multi-track-sequencer.md` | Multi-track sequencer architecture |
| ADR-017 | `ADR-017-effect-node-architecture.md` | Effect node architecture |
| ADR-018 | `ADR-018-cellular-architecture.md` | Node is the universal primitive — no non-node objects |
| ADR-019 | `ADR-019-universal-parameter-control.md` | Universal parameter control (CMD_SET/BUMP_PARAM) |
| ADR-020 | `ADR-020-launchpad-profile-digitakt.md` | Launchpad X profile — Digitakt-inspired two-mode layout |
| ADR-021 | `ADR-021-launchpad-x-protocol.md` | Launchpad X protocol and device integration |
| ADR-022 | `ADR-022-node-portability.md` | Node portability — L2-only plugin linking |
| ADR-023 | `ADR-023-instrument-encapsulation.md` | GraphNode instrument encapsulation |
| ADR-024 | `ADR-024-clap-wrapper.md` | CLAP plugin wrapper architecture |
| ADR-025 | `ADR-025-project-file-format.md` | RON project file format |
| ADR-026 | `ADR-026-instrument-definition-tui.md` | Declarative instrument definition and terminal UI |
| ADR-027 | `ADR-027-clap-host.md` | CLAP host architecture (clap-sys + libloading) |
| ADR-028 | `ADR-028-loop-break-node.md` | Single-sample feedback loop break node |
| ADR-029 | `ADR-029-dynamic-topology.md` | Dynamic graph topology — patchable modular graph |
| ADR-030 | `ADR-030-pattern-engine.md` | Pattern engine — multi-pattern, multi-page, per-track length/speed |
| ADR-031 | `ADR-031-antiphon-interface-server.md` | Antiphon interface server and protocol |
| ADR-032 | `ADR-032-theoria-view-plugin-api.md` | Theoria view-plugin API — cap-doc view extensions (accepted, 2026-07-13) |
| ADR-033 | `ADR-033-headless-test-driver.md` | Headless test driver |
| ADR-034 | `ADR-034-runtime-observability.md` | Runtime observability — live dropout/xrun/drop counters |
| ADR-035 | `ADR-035-debug-baselines-and-structured-log.md` | Audio regression baselines + structured per-node debug log (🟡 proposed) |
| ADR-036 | `ADR-036-theotokos-performance-terminal.md` | Theotokos keyboard-first performance terminal (accepted, 2026-07-21) |
| ADR-037 | `ADR-037-theotokos-key-remapping.md` | Theotokos runtime key remapping in TK2 (🟡 proposed) |
| ADR-038 | `ADR-038-theotokos-elektron-convergence.md` | Theotokos Elektron convergence — virtual front panel (accepted, 2026-07-23) |
| ADR-039 | `ADR-039-performance-state.md` | Performance state — kits, temp save, perform mode, mute tiers, live record (accepted, 2026-07-23) |
