# ADR-003: Plugin Format

**Date:** May 2026  
**Status:** Accepted

## Decision

CLAP as the primary plugin format. VST3 as secondary via nih-plug.

## Rationale

- CLAP is MIT licensed — no Steinberg license concerns
- CLAP has a clean event model with sample-accurate timestamping within buffers
- CLAP’s extension system maps cleanly to the platform’s capability negotiation model
- CLAP is supported by 15 DAWs and 93 plugin producers as of 2025
- nih-plug provides both CLAP and VST3 from a single Rust implementation
- VST2 is excluded — deprecated, proprietary, and its limitations (no polyphonic expression, no sample-accurate parameter automation) are exactly what the platform is designed to avoid

## Consequences

- Internal Node API (L2) is NOT CLAP — it is its own abstraction. nih-plug translates between L2 and CLAP at the boundary. If CLAP is superseded, only the translation layer changes.
- AAX (Pro Tools) support is not planned. Pro Tools adoption is not a goal for the initial platform.