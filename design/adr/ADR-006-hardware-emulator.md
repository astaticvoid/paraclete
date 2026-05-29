# ADR-006: Hardware Emulator Required at P1

**Date:** May 2026  
**Status:** Accepted

## Decision

Every hardware controller must have a software emulator. The Launchpad emulator is a P1 deliverable. CI must pass without physical hardware present.

## Context

OTTO’s failure analysis shows the cost of tight hardware/software coupling. If development requires physical hardware at every step, the project becomes fragile — hardware unavailability, supply chain issues, or developer context switching all block progress.

## Decision detail

The hardware emulator:

- Renders a graphical representation of the controller surface (pads, buttons, encoders, LEDs) in a desktop window
- Responds to keyboard and mouse input mapped to the equivalent hardware events
- Publishes the same state bus events as the physical controller
- Subscribes to the same LED/display outputs as the physical controller
- Is the default target in CI — no physical hardware dependency in automated tests

The emulator is implemented as a separate HAL backend, not as a special case in the physical HAL. It implements the same `HardwareDevice` trait.

## Consequences

- P1 includes both the oscillator node and the Launchpad emulator
- All automated tests run against the emulator
- Physical hardware testing is additional, not primary
- Contributors without a Launchpad can fully develop and test