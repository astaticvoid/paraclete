# Paraclete — Roadmap

> **Living document.** Replace this file when a phase completes or significant
> planning changes occur. Keep it short — current state only.
>
> **Last updated:** 2026-07-10 evening (paired session #2 held on glass)
> **Current phase:** Legibility phase shipped and judged on the iPad over a **USB-C direct link (3.0 ms RTT, zero config — the no-shared-Wi-Fi answer)**. Session #2 verdict (`design/sessions/s2.md`): "improved a lot… it will be the baseline" but **far below bar — the Launchpad-grid mirror is the wrong foundation for Theoria**. New sequence: **(1) BUG-022/023** (seq-vs-trigger kick pitch mismatch; fast-retrigger ducking — sound correctness moves first), **(2) baseline interaction wins** (drag-draw steps, encoder gesture/placement, hide dead grid), **(3) W2 re-scoped → "Theoria native surface", design-first**: paired reference spike (chosen Elektron box manual + Hydrasynth manual → fixed-input rail + contextual window spec, param/env/LFO pages, source→FX channel view) before any further W-feature code; ADR-032 follows the spec. **Launchpad parked** (good version frozen; s1-F7 cleanup → trigger backlog). P10 C2+ engine depth independent/parallel; P13 keystone unchanged. Earlier 2026-07-10 work: legibility items + BUG-016…021 fixed + open-by-default `--token` opt-in (`theoria-legibility-report.md`). BUG-012 still queued for a hardware session.

---

## Prioritization Decision (July 2026): Playable Loop First

With the tablet web surface accepted as the **primary control/editing device**
(`design/interface-plan.md`), three forces competed for the next quarter:
(a) reach something modestly useful fast and iterate with paired usage
sessions, (b) long-deferred functionality bugs, (c) complete baseline
standards. Decision — in priority order:

1. **User-facing correctness that a session would notice is baseline** and
   moves first: BUG-001 (0.4% tempo error — breaks sync when jamming beside
   the Digitakt) and BUG-008 (param loss on reload after topology change).
   These are P10 C0, pulled forward as an immediate pre-flight commit.
2. **A playable feedback loop beats speculative depth.** W0 → P10 C1 → W1 land
   before P10's pattern-depth commits (pages, chaining, polyrhythm). The
   vision's "A Session" has never been tested against a real session; building
   full pattern depth before the first paired session risks building the wrong
   depth. P10 C2–C5 ordering is **re-validated after paired session #1**.
3. **Non-user-facing standards move to a trigger-based backlog** (below).
   Layer purity and per-cycle micro-allocations don't block sessions and no
   longer occupy a scheduled phase slot (P10.5 dissolved into triggers).

**No interim BUG-005 hack:** audit (July 2026) confirmed step param locks *are*
serialized today; the v2 loss is conditions/micro-timing/swing, which the
current control surface barely reaches. Data-safe saves for those arrive
properly with serializer v3 in P10 C1 — scheduled before session #1 regardless.

**Near-term sequence:**

| Order | Work | Why |
|---|---|---|
| 1 | ~~P10 C0 pre-flight~~ — **shipped** (BUG-001 re-diagnosed via measurement harness; BUG-008 fixed) | Done |
| 2 | ~~**W0**~~ — **shipped** (Theoria grid POC: `paraclete-antiphon` crate + canvas grid) | Done |
| 3 | ~~**P10 C1**~~ — **shipped** (`6212242`; `Pattern` struct + serializer v3 = BUG-005) | Done — data-loss class closed before sessions |
| 3.5 | **BUG-012** — device rate/buffer negotiation + FTZ (small standalone commit) | A 48 kHz interface at session #1 would be mistuned; see audio-model review |
| 4 | ~~**W1**~~ — **C0–C4 shipped** (trigger+velocity, path scheme, state mirror, semantic plane, theoria-web) | Runtime side done + web client builds; C5 = the session |
| 5 | ~~**Paired session #1**~~ — **held 2026-07-09** (`design/sessions/s1.md`) | Pipe proven; verdict = UX not legible ("Behringer, needs Elektron"). Delta: discoverability is the keystone |
| 6 | ~~**Theoria legibility phase**~~ — **implemented 2026-07-10** (`7e7a39a`/`e553c62`; report: `theoria-legibility-report.md`) | Minimum bar items 1–4 done + contextual encoders; judged live in Chrome. **Exit gate = paired tablet judgment (next session)**; F7 cleanup + F4/save-reload deferred |
| 6.1 | **BUG-022 + BUG-023** — seq/trigger pitch unity + retrigger ducking (measurement-first) | Session #2: sound correctness is baseline; sessions can't judge the engine with these live |
| 6.2 | ~~**Theoria baseline interaction wins**~~ — **shipped 2026-07-11** (`e1f86cf` + `3356d60`; report: `theoria-baseline-interactions-report.md`) | Drag-draw paints via new authoritative `/script/lp/steps_n` mask mirror; encoder row at bottom edge w/ value bars; dead grid gone (mode-aware cells). Found+fixed BUG-024 (state_write in subscriptions panicked). Tablet judgment pending |
| 6.3 | **W2 re-scoped: Theoria native surface, design-first** — paired reference spike (Elektron box TBD + Hydrasynth manuals) → fixed-input rail + contextual window spec → build; ADR-032 after the spec | Session #2 F1/F7 keystone: "consistent inputs, contextual screen window"; do not improvise UI |
| 7 | ~~P10 C2–C5~~ — **shipped 2026-07-11** (`0a8116b`/`e8f7718`/`50ef64b`/`5306674`; report: `p10-report.md`) | Pattern engine complete: page-loop windows, per-track length/speed (polyrhythm), BUG-004 fixed, cued switching + chain, state-bus surface + TUI indicator. §5.3 Launchpad surface parked per s2; P10 play-test pending (Theoria/W2 or paired TUI session) |

**Paired sessions** are a first-class roadmap instrument from here on: one after
each W-milestone (W1, W2, W3), notes captured append-only in `design/sessions/`,
each producing explicit roadmap deltas (or an explicit "no change").

---

## Roadmap

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0–P9** | Skeleton → Modular Graph | See `architecture-evolving.md` phase log | **Complete** |
| **P9.5** | Device Emulation & Test Harness | Full Launchpad emulator (C1). | **Closed early** — C1 shipped; C2/C3 cancelled (superseded by W0/W1); C4 rescoped into P10 C5 test work; piano mode deferred (physical Keystep exists) |
| **W0** | Theoria grid POC | Browser grid as peer device: `paraclete-antiphon` crate, WS bridge, canvas 8×8 + scene + control, LED mirror, shared `launchpad.rhai` profile | **Shipped** (July 2026; report: `w0-report.md`; localhost touch→LED 24–34 ms; exit criteria needing tablet/Launchpad hardware roll into the next user session) |
| **P10 C0–C1** | Pattern engine foundation | BUG-001/008 pre-flight (C0, runs before W0); `Pattern` struct + serializer v3 = BUG-005 (C1) | **Shipped** (C0 `b0cf2c8`, C1 `6212242`) |
| **W1** | Theoria MVP | Touch encoders (relative → `CMD_BUMP_PARAM`), context display, transport, state mirror v1 → **paired session #1** | **C0–C4 shipped** (`w1-report.md`); C5 = paired session #1 (next) |
| **P10 C2–C5** | Pattern Engine depth | Multi-page (64-step) + page-loop; seamless switching + chaining; per-track length/speed; BUG-004 **+ BUG-013 (sub-block voice starts — micro-timing must be audible) + Sampler Hermite playback** in C3; grid/TUI surface | In design — order re-validated after session #1 |
| **W2** | Theoria editor | Cap-doc-driven parameter pages for every engine; chain view; view-plugin API (ADR-032) → **paired session #2** | — |
| **WT** | Theoria/term | Terminal client over in-process Antiphon transport; parameter pages + grid in the terminal | After W2, parallel W3 |
| **W3** | Sequencer deep views | 64-step pattern view, cue/chain, hold-step p-lock overlay, condition/timing editors | Hard dependency on P10 C2–C5 |
| **P11** | Live Performance | Mute system, temp save/reload, Perform Kit, live record | — (W3 mute view follows) |
| **P12** | Groove & Generation | Retrig, Euclidean, controlled randomness, generative fills | — |
| **P13** | Analog Voice | Full subtractive mono voice — **Pro-One as primary reference** (dual osc + hard sync + poly-mod routing, self-oscillating 4-pole, two envs, glide, arp); Model D / MS-20 as secondary character references only. Paraphonic allocation, **per-voice-expression-aware (OQ-15)**. ZDF ladder is C1 (audio-model review). | — |
| **P14** | FM Voice | Four-operator melodic FM, macro-first | — |
| **P15** | Effects Palette | Distortion variety, chorus/phaser/flanger, BBD/tape delay, spring/plate | — |
| **P16** | Macro & Terminal Control | Macro system; TUI as editing surface | — |
| **W4** | Interface maturity | Ordo layout profiles, multi-client polish, wavetable view, protocol freeze, headless protocol CI driver | Ongoing after W3 |

The interface track (Antiphon server + Theoria clients) is specified in
`design/interface-plan.md` (**accepted July 2026**; ADR-031 authored with W0,
ADR-032 with W2). The terminal is a permanent first-class surface: **WT**
(after W2, parallel to W3) ports `paraclete-tui` to an Antiphon client over an
in-process transport, gaining the same generic views (pages, grid) as the web
client — the P16 "TUI as editing surface" goal delivered early through shared
machinery.

---

## Why P10 Still Matters — The Playability Gap

Unchanged from June 2026: "fun to play" per `instrument-vision.md` needs pages,
patterns, polyrhythm, and durable state, and the engine has none of them
(`CMD_SET_PATTERN` is a stub; single 16-step pattern). P10 closes this; P11
layers performance affordances on top. What changed is *sequencing*: the
foundation commits run now, the depth commits run after real session evidence.

The arc beyond P12 (synthesis P13–P14, effects P15, macro/terminal P16) is
unchanged — scope, not new architecture; P13 remains the keystone of the full
four-pillar instrument.

---

## Deferred-Bug Backlog (trigger-based, replaces P10.5)

| Bug | Fix when this trigger fires |
|---|---|
| BUG-002 (`baseline()` hardcodes 44100/512) | First non-44.1 kHz/512 deployment, or CLAP host reports mismatch — whichever first |
| BUG-003 (hal→runtime layer violation) | Before any LGPL3/crates.io publication of `StateBusHandle` consumers, or when `paraclete-antiphon` wants `StateBusHandle` from L2 |
| BUG-006 (`agg_state_buf` realloc/cycle) | When the web state mirror measurably raises state-bus churn (profile in W1), or any audible xrun traced to it |
| BUG-007 (`publish_bank_state` String alloc) | Folded into W1 C1 (state-path unification) |
| Executor cell `Mutex` → `arc-swap`; master limiter/headroom; xrun/CPU meter to `/engine/cpu`; FM 2-sample feedback average; DT node `rtrb` harmonization | See `design/review/audio-model-review.md` — each has a named trigger there |

Everything else open is scheduled: BUG-001/008 → P10 C0 (now), BUG-005 → P10 C1
(now), BUG-004 → P10 C3.

---

## Known Provisional Implementations

| Item | Status | Resolution | Target |
|---|---|---|---|
| `CMD_SET_PATTERN` stub (always pattern 0) | Active | Multi-pattern model | P10 C2+ |
| Single-pattern, 16-step only | Active | Pattern engine | P10 C2+ |
| `Sequencer::serialize()` drops P5 fields (BUG-005) | **Fixed (P10 C1 shipped)** | Serializer v3 (`6212242`); length-prefixed step + pattern records | Done |
| BUG-008 / BUG-001 | **Fixed (P10 C0 shipped)** | s0 re-diagnosis: 240-tick step + step-0 fire + drift-only snap; mem::take | Done |
| Negative micro-timing == zero (BUG-004) | Active | Emit in prev step's window | P10 C3 |
| Terminal emulator: no RGB, no keyboard encoders | **Accepted permanent** | Web surface supersedes; terminal stays keyboard-grid-only for no-tablet dev | — |
| No headless input injection for CI | Active | In-process injection API, built with P10 C5 surface tests; protocol-level driver at W4 | P10 C5 |
| Encoder hardware (EN16/MFT) unpurchased | **De-escalated** | W1 touch encoders are the only relative path (session 0: Digitakt II verified absolute-only, disqualified; BUG-009 filed); buy a true-relative box later for tactile feel | Post-W1 |
| Hard-coded app node IDs as script/UI contract | Active | W-track binds by discovery (`hello`/`topology` msgs); profiles migrate when it breaks | W2 |
| AnalogEngine/FmEngine monophonic | Active | Voice allocator | P13 |
| L2 `&'static str` in `CapabilityDocument`/`SurfaceDescriptor` (name, vendor, extensions) | Active — **`Box::leak` already shipping** in `paraclete-clap-host/src/bridge.rs:137` per plugin load; blocks dynamic surfaces (Theoria per-client) and dynamic nodes | `Cow<'static, str>` (PortName already models the fix); breaking change is FREE until crates.io publication — do before `paraclete-node-api` v0.1.0 ships | Pre-publication |
| Per-step velocity dropped at engine boundary (`retrigger(note)` has no velocity) | Active | Plumb velocity → level in W1 C0 (spec amended); velocity-mod routing later | **W1 C0** |
| Sampler: one sample per node — no velocity layers, round-robin, or slices | Active | Layering via topology today; slices/layers are engine-internal, spec with the P12+ drum pass | P12+ |
| AnalogEngine: one machine per drum type | Active — vision amended July 2026: machine *family* per type (kick/snare/tom/clap/hat variants, per-track selectable) is the destination | peaks/plaits (MIT) algorithm source; OQ-14 decides selection mechanism | P12+ |
| Inner GraphNode runtime patching; `InnerGraphNode::serialize()` empty | Active | Inner-graph patch + persistence | P11+ |
| CLAP plugin nodes not in `NodeRegistry` | Active | Registry + PluginLibrary arg | P11+ |

---

## Open Questions

| # | Question | Blocking | Notes |
|---|---|---|---|
| OQ-3 | MIDI 2.0 CI depth | — | Deferred; not blocking |
| OQ-4 | Network / distributed nodes | — | The W-track WS protocol is a de-facto first answer; revisit after protocol freeze (W4) |
| OQ-6 | Micro-timing clock representation | P10 C3 | Close out in p10-report |
| OQ-7 | Oversampling strategy | — | Not until CvSignal audio-rate modulation needs it |
| OQ-11 | Pattern/page representation on the Launchpad grid | P10 C5 | Scene = page select; cued blink; now co-designed with the W3 pattern view |
| OQ-12 | Live-record quantisation model | P11 | Step vs live-quantised; interaction with micro-timing |
| OQ-15 | Per-note expression routing (poly aftertouch / MPE) | P12+ mod routing; P13 allocator | No architectural gap — internal events are MIDI 2.0 UMP (per-note pressure native; MPE subsumed). Policy set now: **MPE is translated to UMP per-note at the device-node boundary** (the graph never sees channel-rotation); `HardwareEvent::PadPressure` exists unused (LP X sends 0xA0 in programmer mode — parse it when a consumer exists); engines route pressure → destinations via the same velocity-mod family (P12+); **P13 voice allocator must be per-voice-expression-aware from day one** (pressure/pitch/timbre per voice); CLAP boundary maps to note expressions (already deferred-tracked). Wire msg `pad_pres` reserved in protocol v0. |
| OQ-14 | Drum machine selection: stepped parameter vs type_tag swap | P12+ | Machine-as-parameter (ADR-019 stepped param: kit-savable, live-switchable, no topology change) vs machine-as-type-tag (`analog_engine:kick_hard`, apply_patch swap). Parameter path is the live-performance-friendly default hypothesis; decide in the P12+ spec. |
| OQ-13 | P13 voice: composed-from-primitives vs monolithic machine | **P13 C0** | `instrument-vision.md` says "composed from Paraclete primitives" (GraphNode/ADR-023 path: Phase-port hard sync, topo-order feedforward mod are already expressible); ADR-022's machine pattern + `SubgraphPlugin` CLAP portability argue for a monolithic `AnalogVoice` engine like AnalogEngine. Decide first, in the P13 spec — it shapes the mod-matrix API and CLAP export. |
| WQ-1…9 | Interface-track risks (Wi-Fi jitter, velocity, reconciliation, licensing, terminal parity scope, …) | W0–W2 | Tracked in `design/interface-plan.md` |

---

## Resolved Open Questions

| Question | Resolution | ADR / Phase |
|---|---|---|
| Language / License / Plugin format / Signal model | Rust; GPL3+LGPL3 L2; CLAP; mixed-rate typed ports | ADR-001…004 |
| Scheduler cycles | petgraph DAG; LoopBreakNode | ADR-005, ADR-028 |
| Hardware emulator; Scripting runtime; Node API level; Clock ownership | Required at P1; Rhai; single trait, three levels; federated domains | ADR-006…009 |
| Multi-track; Effects; Cellular architecture; Universal parameter control | Node instances; plain nodes + marker; no non-node components; CMD_SET/BUMP_PARAM | ADR-016…019 |
| Node portability; Instrument encapsulation | L2-only linking; GraphNode | ADR-022, ADR-023 |
| CLAP wrapper / project file / TUI / CLAP host | Hand-rolled adapter; RON; terminal UI + publish_context; clap-sys host | ADR-024…027 |
| Dynamic topology | Pause-rebuild-resume; NodeRegistry | ADR-029 |
| Pattern engine data model | Pattern owns steps; Sequencer owns patterns | ADR-030 |
| Interface architecture (server + clients) | Antiphon server, pluggable transports, surfaces as device nodes; Theoria schema-driven views + plugins | **interface-plan (accepted); ADR-031 @ W0, ADR-032 @ W2** |
| ~~OQ-10~~ XL Mk3 protocol | Moot — XL ruled out; touch encoders (W1) + true-relative box later | — |
