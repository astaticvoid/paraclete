# Paraclete — Roadmap

> **Living document.** Replace this file when a phase completes or significant
> planning changes occur. Keep it short — current state only.
>
> **Last updated:** July 2026 (playable-loop reprioritization)
> **Current phase:** P10 C0 pre-flight, then W0 (Theoria grid POC);
> W1 → first paired usage session is the near-term milestone

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
| 2 | **W0** — Theoria grid POC (`paraclete-antiphon` crate + canvas grid) | De-risks transport; tablet in hand; supersedes P9.5 C2 |
| 3 | **P10 C1** — `Pattern` struct + serializer v3 (BUG-005) | Foundation commit of ADR-030; kills the data-loss class before sessions |
| 3.5 | **BUG-012** — device rate/buffer negotiation + FTZ (small standalone commit) | A 48 kHz interface at session #1 would be mistuned; see audio-model review |
| 4 | **W1** — touch encoders + context MVP | The "modestly useful" milestone: full playable surface, zero encoder-hardware spend |
| 5 | **Paired session #1** — structured; findings → `design/sessions/s1.md` | Re-validates P10 C2–C5 order, W2 view priorities, and the vision's session walkthrough |
| 6 | P10 C2+ (pages, switching, polyrhythm, surface) interleaved with W2 | Order set by session findings |

**Paired sessions** are a first-class roadmap instrument from here on: one after
each W-milestone (W1, W2, W3), notes captured append-only in `design/sessions/`,
each producing explicit roadmap deltas (or an explicit "no change").

---

## Roadmap

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0–P9** | Skeleton → Modular Graph | See `architecture-evolving.md` phase log | **Complete** |
| **P9.5** | Device Emulation & Test Harness | Full Launchpad emulator (C1). | **Closed early** — C1 shipped; C2/C3 cancelled (superseded by W0/W1); C4 rescoped into P10 C5 test work; piano mode deferred (physical Keystep exists) |
| **W0** | Theoria grid POC | Browser grid as peer device: `paraclete-antiphon` crate, WS bridge, canvas 8×8 + scene + control, LED mirror, shared `launchpad.rhai` profile | **Next** |
| **P10 C0–C1** | Pattern engine foundation | BUG-001/008 pre-flight (C0, runs before W0); `Pattern` struct + serializer v3 = BUG-005 (C1) | **Pulled forward** |
| **W1** | Theoria MVP | Touch encoders (relative → `CMD_BUMP_PARAM`), context display, transport, state mirror v1 → **paired session #1** | — |
| **P10 C2–C5** | Pattern Engine depth | Multi-page (64-step) + page-loop; seamless switching + chaining; per-track length/speed; BUG-004 **+ BUG-013 (sub-block voice starts — micro-timing must be audible) + Sampler Hermite playback** in C3; grid/TUI surface | In design — order re-validated after session #1 |
| **W2** | Theoria editor | Cap-doc-driven parameter pages for every engine; chain view; view-plugin API (ADR-032) → **paired session #2** | — |
| **WT** | Theoria/term | Terminal client over in-process Antiphon transport; parameter pages + grid in the terminal | After W2, parallel W3 |
| **W3** | Sequencer deep views | 64-step pattern view, cue/chain, hold-step p-lock overlay, condition/timing editors | Hard dependency on P10 C2–C5 |
| **P11** | Live Performance | Mute system, temp save/reload, Perform Kit, live record | — (W3 mute view follows) |
| **P12** | Groove & Generation | Retrig, Euclidean, controlled randomness, generative fills | — |
| **P13** | Analog Voice | Full subtractive mono voice — **Pro-One as primary reference** (dual osc + hard sync + poly-mod routing, self-oscillating 4-pole, two envs, glide, arp); Model D / MS-20 as secondary character references only. Paraphonic allocation. ZDF ladder is C1 (audio-model review). | — |
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
| `Sequencer::serialize()` drops P5 fields (BUG-005) | Active | Serializer v3 | **P10 C1 (next)** |
| BUG-008 / BUG-001 | **Fixed (P10 C0 shipped)** | s0 re-diagnosis: 240-tick step + step-0 fire + drift-only snap; mem::take | Done |
| Negative micro-timing == zero (BUG-004) | Active | Emit in prev step's window | P10 C3 |
| Terminal emulator: no RGB, no keyboard encoders | **Accepted permanent** | Web surface supersedes; terminal stays keyboard-grid-only for no-tablet dev | — |
| No headless input injection for CI | Active | In-process injection API, built with P10 C5 surface tests; protocol-level driver at W4 | P10 C5 |
| Encoder hardware (EN16/MFT) unpurchased | **De-escalated** | W1 touch encoders are the only relative path (session 0: Digitakt II verified absolute-only, disqualified; BUG-009 filed); buy a true-relative box later for tactile feel | Post-W1 |
| Hard-coded app node IDs as script/UI contract | Active | W-track binds by discovery (`hello`/`topology` msgs); profiles migrate when it breaks | W2 |
| AnalogEngine/FmEngine monophonic | Active | Voice allocator | P13 |
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
