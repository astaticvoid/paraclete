# ADR-002: Implementation Language

**Date:** May 2026  
**Status:** Accepted

## Decision

Rust (stable toolchain).

## Context

The two realistic options were Rust and C++. A hybrid was also considered.

## Rationale

- Memory safety without GC — critical for a real-time audio system
- Excellent cross-platform build tooling
- Rust audio ecosystem has matured sufficiently: nih-plug, clack, cpal, fundsp, dasp, rhai, midi2 all exist and are maintained
- The one significant gap is GUI, but GUI is deferred (see `architecture-evolving.md` GUI strategy and ADR-006)
- C++ would provide access to more established DSP libraries (JUCE, etc.) but introduces memory safety risks, build complexity, and weaker ecosystem tooling
- Hybrid (Rust application + C/C++ DSP modules via FFI) was considered but adds complexity and was rejected in favour of Rust-native DSP via Mutable Instruments ports and fundsp

## Consequences

- No JUCE dependency — all audio I/O, MIDI, and plugin hosting must use Rust crates
- DSP implementations must be Rust-native or ported from C/C++ sources
- GUI options are more limited than C++ (see GUI strategy)
- Contributor pool is smaller than C++ but growing rapidly in the audio space