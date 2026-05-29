# ADR-015: Connection Negotiation — Agreed at Connection Time

**Date:** May 2026  
**Status:** Accepted

## Decision

When two nodes connect, the runtime brokers a capability handshake. Each node’s `CapabilityDocument` is exchanged and a `ConnectionAgreement` is produced. The agreement records the shared event vocabulary, negotiated space_ids for custom events, and any connection-specific configuration. space_ids are local to a connection — there is no global registry.

## Context

Three space_id assignment strategies were considered:

**Static registry:** space_ids registered centrally like IANA port numbers. Authoritative but requires coordination overhead.

**Hash-based:** space_id is a hash of a namespace string. No coordination but nonzero collision probability.

**Negotiated at connection time (chosen):** space_ids are agreed between the two nodes during the handshake. They are meaningful only within that connection. No global state. No coordination required. Fits the Plan 9 capability negotiation model.

## Handshake protocol

```
Node A connects to Node B
  → Runtime calls A.capability_document() → CapDoc_A
  → Runtime calls B.capability_document() → CapDoc_B
  → Runtime calls A.negotiate(&CapDoc_B)  → Agreement_A
  → Runtime calls B.negotiate(&CapDoc_A)  → Agreement_B
  → Runtime reconciles → ConnectionAgreement
  → Both nodes receive the final ConnectionAgreement
```

The `ConnectionAgreement` contains:

- Agreed audio buffer format (sample rate, channel count, block size)
- Active event types for this connection
- Negotiated space_id assignments for custom extended events
- Any capability-specific configuration (e.g. MPE zone layout, polyphony limit)

## Baseline agreement

If either node does not override `negotiate()`, the default implementation produces a baseline agreement: core Paraclete event types only, no custom extensions, standard audio format. This means two nodes that know nothing about each other can still connect and exchange core events correctly.

## Inspectability

`ConnectionAgreement` is a first-class runtime value. It is published to the state bus at `/node/{id}/connections/{port}` and is readable by scripts, the GUI, and other nodes. The full connection state of the graph is always introspectable.

## Consequences

- Connection setup is slightly more expensive than a simple port match — acceptable since connections change on the main thread, not the audio thread
- Nodes that speak the same custom event vocabulary discover this at connection time without any external coordination
- The runtime never needs to understand custom event semantics — it negotiates space_ids and transports bytes
- Custom event protocols are entirely bilateral — defined by the two nodes involved
- `ConnectionAgreement` is part of the project save format — connections are restored with their agreed configuration