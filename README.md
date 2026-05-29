# Paraclete Design Documents

```
design/
├── README.md                          ← this file
├── architecture-core.md               ← stable reference: layer model, Node API, signal types
├── architecture-evolving.md           ← living: roadmap, open questions, phase notes
├── prior-art-analysis.md              ← failure analysis and crate reference
├── adr/                               ← append-only decision records
│   ├── ADR-001-license.md
│   ├── ADR-002-language.md
│   ├── ADR-003-plugin-format.md
│   ├── ADR-004-signal-model.md
│   ├── ADR-005-scheduler-cycles.md
│   ├── ADR-006-hardware-emulator.md
│   ├── ADR-007-scripting-runtime.md
│   ├── ADR-008-node-api-level.md
│   ├── ADR-009-clock-federation.md
│   ├── ADR-010-state-model.md
│   ├── ADR-011-executor-model.md
│   ├── ADR-012-buffer-allocation.md
│   ├── ADR-013-event-model.md
│   ├── ADR-014-capability-document.md
│   └── ADR-015-connection-negotiation.md
└── phases/                            ← per-phase interface specs and implementation reports
    ├── p0-interfaces.md
    ├── p0-report.md
    ├── p1-interfaces.md
    ├── p1-report.md
    ├── p2-interfaces.md
    └── p2-report.md
```

## Document types

**Stable** (`architecture-core.md`, `prior-art-analysis.md`) — change only when foundational decisions change.

**Living** (`architecture-evolving.md`) — updated after each phase completes or an open question resolves.

**ADRs** (`adr/`) — append-only. Never edit a past ADR; add a new one to supersede it.

**Phase docs** (`phases/`) — interface spec written before implementation, report written after. Both are append-only once the phase ships.
