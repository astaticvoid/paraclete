# Universality Audit — hardcoded counts & `&'static str` before the W2 freeze

> **Purpose.** The crystallized vision (`instrument-vision.md`, "Performance Meets
> Limitless Composability") makes limitlessness structural: *no hardcoded count may
> survive into any frozen format* (serializer, wire protocol, published L2 API).
> W2 is about to freeze the interface/view/protocol surface and ADR-032; after the
> W4 protocol freeze and crates.io publication these ceilings become permanent.
> This audit catalogs what to fix **before** which freeze. Standing Universality
> directive ([[feedback-universality]]).
>
> **Method.** Grep sweep across `crates/*/src` for fixed-size array *types*,
> `MAX_/NUM_/_COUNT/_CAPACITY` consts, `&'static str` in published APIs,
> `Box::leak`, and voice/track/channel/step/encoder counts. Triaged by permanence.
> **Date:** 2026-07-12.

## Severity legend
- **FREEZE-CRITICAL** — becomes permanent at a format/API freeze; fix before it.
- **CAP** — real limit vs. the "limitless" vision, but runtime-changeable (not
  permanent). Fix when the ceiling is actually hit or by design decision.
- **OK** — legitimate fixed structure, not a composability cap. Listed so it is
  not re-flagged.

---

## U1 — `&'static str` in the L2 published API  ·  ~~FREEZE-CRITICAL~~ **RESOLVED 2026-07-12**

> **Fixed** (commit pending). Migrated the two offending types to
> `Cow<'static, str>` and deleted both `Box::leak` sites. 554 tests pass,
> `cargo check --all-targets` clean, no new clippy findings.
>
> **Scope correction (learned during the fix):** only *two* types actually needed
> migrating. `ParamUnit::Custom`, `PortName::Static`, and `ParamDisplayAdapter`
> **already** carry a `…Dynamic(String)` / `Dynamic(Box<…>)` sibling — they had
> modeled the hybrid all along. The real offenders:

| Type | Field | Location | Action |
|---|---|---|---|
| `CapabilityDocument` | `name`, `vendor` | `capability.rs` | → `Cow<'static, str>` ✅ |
| `CapabilityDocument` | `extensions: Vec<&'static str>` | `capability.rs` | → `Vec<Cow<'static, str>>` ✅ |
| `SurfaceDescriptor` | `name`, `vendor` | `surface.rs` | → `Cow<'static, str>` ✅ |
| `bridge.rs` `Box::leak` ×2 | runtime plugin name/vendor | `clap-host/bridge.rs:137–138` | deleted → owned `Cow` ✅ |

~85 static construction sites now pass `"name".into()` (zero-alloc `Cow::Borrowed`);
runtime-named nodes/surfaces carry `Cow::Owned` with no leak. `ParamUnit::Custom`
and `PortName::Static` were left as-is (already correct).

**Impact.** Any node/surface whose name is known only at runtime must leak memory:
`paraclete-clap-host/src/bridge.rs:137–138` already calls `Box::leak` on every
CLAP plugin load. This blocks the things W2/W-track need: **dynamic Theoria
per-client surfaces**, dynamic nodes, and honest CLAP-hosted plugin names. Because
this is the LGPL3 third-party contract, the breaking change is **free until the
first crates.io publication of `paraclete-node-api` and permanent after.**

**Fix.** Migrate to `Cow<'static, str>`. The pattern already exists in-tree:
`PortName` is a hybrid `Static(&'static str)` / `Dynamic(String)` — replicate it or
(cleaner) move all five sites to `Cow<'static, str>`, then delete the two
`Box::leak` sites. Static call sites stay ergonomic (`"name".into()`); dynamic
sites stop leaking.

**Fix before:** `paraclete-node-api` v0.1.0 / first crates.io publish. **Also
unblocks W2** (dynamic per-client Theoria surfaces) — so effectively do it as part
of the W2 surface work, not later. This is roadmap **D6** / the Known-Provisional
`Cow` row, now confirmed as W2-blocking, not merely pre-publication.

---

## U2 — Antiphon surface/wire assumes a Launchpad-shaped controller  ·  **FREEZE-CRITICAL**

`paraclete-antiphon` hardcodes an 80-pad + 8-encoder control map:

| Const | Value | Location | Meaning |
|---|---|---|---|
| `SHADOW_SLOTS` | `98` | `kerygma.rs:25` | control ids 0–97: "80 pads, gap 80–89, encoders 90–97" |
| `ENCODER_COUNT` | `8` | `surface.rs:26` | 8 encoders |
| `ENCODER_SLOTS` | `8` | `app/main.rs:250` | app-side 8-encoder assumption |

**Impact.** The vision is "any controller." A 16-encoder box (the eventual
true-relative target), a differently-shaped grid, or a large touch surface does not
fit an 80-pad / 8-encoder id map. These **control ids are wire-visible**, so the
shape risks baking into the protocol at freeze.

**Fix.** Make the shadow table / control-id space surface-descriptor-driven
(size from the surface's declared pad/encoder counts) rather than a global const.
**Fix before:** protocol v1 freeze (W4) — but decide the shape during **W2**, since
W2 defines the surface/view protocol. Flag into ADR-032 / the W2 spec.

---

## U3 — `MAX_CLIENTS = 4` hard-refuses the 5th client  ·  **CAP**

`kerygma.rs:22` — the 5th concurrent Theoria connection is refused `bye
{"reason":"full"}`. Arbitrary; multi-client is a W-track goal. Server-side capacity
(not obviously wire-baked), so runtime-changeable. **Fix when:** multi-client
polish (W4) — make it configurable, not a const.

---

## U4 — Sequencer `STEP_CAPACITY = 64`  ·  **CAP (not a format freeze)**

`sequencer.rs:306`. **Good news:** the serializer is length-prefixed (the step
bitfield is `length` chars, not a fixed 64 — `steps_bitfield()`, `CMD_SET_LENGTH`
clamps 1..=64), so **64 is a runtime allocation cap, not baked into the format.**
It reflects the Launchpad's 8 pages × 8 steps. Not freeze-blocking. **Decision for
the user:** is 64 a permanent design bound, or a controller-era default that should
grow for the mouse+keyboard composition surface? Low urgency; note it.

---

## U5 — Fixed polyphony: Sampler `[Voice; 4]`, engines monophonic  ·  **CAP (scheduled)**

`sampler.rs:189` (`voices: [Voice; 4]`); `AnalogEngine`/`FmEngine` are monophonic
(Known-Provisional). Already tracked for the **P13 voice allocator**. **Action for
P13 (OQ-13):** the allocator must use a **dynamic/configurable voice pool**, not
`[Voice; N]` — paraphony is a first-class vision goal ("cavernous, evolving side of
the sound depends on it"). Bake "no fixed voice count" into the P13 spec from day
one.

---

## U6 — Legitimate fixed structures  ·  **OK (do not re-flag)**

Fixed by genuine topology/DSP, not composability caps:
- `[PortDescriptor; N]` on every node — a node's port count *is* its fixed
  topology. Mixers/modular use `PortName::Dynamic` already.
- DSP internals: reverb `comb[8]`/`allpass[4]` (Freeverb constants), ladder
  `stage[4]` (4-pole), oscillator/filter port arrays, `rgb: [u8; 3]`.
- Ring capacities (`RING_CAPACITY=256`, `NODE_CMD_RING=512`, `INBOUND_RING=256`) —
  backpressure sizing, tunable, not user-facing caps.
- `clap-host MAX_EVENTS = 64` per block — could *drop* events under extreme
  density; low risk, worth a `debug_assert`/counter later, not a freeze concern.

---

## Summary — what to fix before which freeze

| # | Finding | Before | Owner |
|---|---|---|---|
| **U1** | ~~`&'static str` in L2 → `Cow`~~ **DONE 2026-07-12** — `CapabilityDocument`/`SurfaceDescriptor` migrated, `Box::leak` gone | ~~before publish/W2~~ shipped | done |
| **U2** | Launchpad-shaped surface/wire consts → descriptor-driven | protocol freeze (W4); decide in W2 | user (spec) + agent |
| **U3** | `MAX_CLIENTS=4` → configurable | W4 multi-client | agent |
| **U4** | `STEP_CAPACITY=64` bound — confirm or grow | not freeze-blocking | user (design bound) |
| **U5** | Dynamic voice pool, not `[Voice;N]` | P13 spec / allocator | user (OQ-13) + agent |

**Headline:** **U1 is both the highest-permanence risk and W2-blocking** — the
`Cow` migration should move from "pre-publication someday" (D6) to *part of the W2
surface work*, because dynamic per-client Theoria surfaces need it. U2 is the other
freeze-critical item and belongs in the W2/ADR-032 conversation (surface shape).
U1 is a clean, well-scoped, agent-actionable migration whenever you want it started.
