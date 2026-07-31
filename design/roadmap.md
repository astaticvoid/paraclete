# Paraclete — Roadmap

> **Plan, not state.** This file holds the phase sequence, the design gates,
> and the standing prioritization decisions. **Live state — open bugs, open
> questions, spec gaps, spikes, provisional implementations — lives in GitHub
> Issues**, not here. See `design/README.md` for the lookup commands.
>
> Do not reintroduce a status block or a `Previous:` revision stack. Git holds
> every prior revision of this file; a changelog inside it is redundant by
> construction.

**Active work:** the open milestone. `gh issue list --milestone TK2.2`.

---

## Active Priorities — triage against the vision

Ranked, from the 2026-07-12 triage. Ranks 1–2 are complete; 3–4 are the
standing north-star, both user-owned.

| Rank | Work | Owner | Status | Notes |
|---|---|---|---|---|
| **1** | **W2 surface spec + ADR-032** — the universal node-view contract; the layered-surface model | user (paired) | **✅ ratified 2026-07-13** | Spec accepted. ADR-032 accepted. |
| **2** | **Debug/test harness (ADR-033/ADR-035)** — structured per-node debug log, regression baselines, CPU meter | agent | **✅ complete 2026-07-13** | Null backend + REPL (`92b8795`), regression baselines (`b74b853`), structured debug log (`73332d5`), CPU meter (`aad9e52`). |
| **3** | **P13 voice: OQ-13 + OQ-14** — two-tier engine model | user | brief drafted | `w2-reference-analysis.md` P13 appendix. OQ-13 is coupled to §6.0 — decide together. |
| **4** | **Openable engines** — Tier-1 monolithic becomes graph-openable | user (later) | north-star, parked | Needs GraphNode / `InnerGraphNode::serialize()` maturity. P13→P14+; **not** a W2 gate. |

The pre-W2 universality / hardcoded-count audit shipped 2026-07-12 — 6 findings
triaged by permanence; **U1** (`&'static str` → `Cow<'static, str>` across the L2
API, ~85 sites, 554 tests green) unblocked dynamic per-client Theoria surfaces.
Detail: `design/review/universality-audit.md`. **U2** folds into ADR-032.

---

## Prioritization Decision (July 2026): Playable Loop First

With the tablet web surface accepted as the **primary control/editing device**
(`design/interface-plan.md`), three forces competed: (a) reach something
modestly useful fast and iterate with paired sessions, (b) long-deferred
functionality bugs, (c) complete baseline standards. Decision, in priority
order:

1. **User-facing correctness that a session would notice is baseline** and
   moves first. These became P10 C0, pulled forward as a pre-flight commit.
2. **A playable feedback loop beats speculative depth.** W0 → P10 C1 → W1 land
   before P10's pattern-depth commits. The vision's "A Session" had never been
   tested against a real session; building full pattern depth before the first
   paired session risks building the wrong depth.
3. **Non-user-facing standards move to a trigger-based backlog.** Layer purity
   and per-cycle micro-allocations don't block sessions and no longer occupy a
   scheduled phase slot (P10.5 dissolved into triggers). Those triggers now
   live on the issues themselves, labelled `deferred`.

**No interim BUG-005 hack:** audit confirmed step param locks *are* serialized
today; the v2 loss was conditions/micro-timing/swing. Data-safe saves arrived
with serializer v3 in P10 C1.

---

## Implementation Order & Design Gates (2026-07-23)

The execution sequence, with **⛔ gates** where work pauses for user design
input (ratification, musical judgment, or a paired session). Agent-executable
stretches need no input between gates.

| Step | Work | Gate before proceeding |
|---|---|---|
| 1 | ~~**TK2 C0–C9**~~ (`tk2-theotokos.md`) | **Shipped** |
| 2 | ~~**⛔ Session #2**~~ (TK2 C10, user-paired) | **Held 2026-07-27 — not a clean sign-off.** Reopened design.md §5.1/§5.2 + the default REC/trig mode model. |
| 2.5 | ~~**⛔ TK2.1 redesign pass**~~ — ADR-044 ✅ ratified 2026-07-28 | **Gate cleared.** Shipped C0–C7; session #3 held 2026-07-29. |
| 2.6 | ~~**TK2.2 fix pass**~~ (`tk2.2-theotokos.md`, ADR-046) | **Code-complete 2026-07-30, not closed.** ⛔ **C6 — usability session #4** is the only remaining step. |
| 3 | **⛔ TK2-exit scheduling pass** (user) — blocked on step 2.6 | Order the parallel tracks: P11 spec → impl; AN0(→AN1); ADR-041+042 implementation. All three are independent of each other. |
| 4a | **P11**: spec (agent, session-informed) → **⛔ spec ratification** (kit UX) → impl → session | two gates |
| 4b | **AN0–AN1** (pool → capture; R2 transition-trick gate on AN1 exit) → **⛔ sampling session** | AN2 additionally needs P11 KitStore shipped |
| 4c | **ADR-041 + ADR-042 impl** (machine select, MOD page — mechanical vs the ADRs) | none until P14 |
| 5 | **P14**: spec — **⛔ user freezes the musical tables** → impl → **⛔ baseline patches + session** | needs 4c |
| 6 | **W-track residuals**, any time a session is convened: W2 C7 §7.1 exit pass; BUG-012 hardware verification | ⛔ paired session |
| 7 | **TK3 / WT convergence decision** (OQ-T12), after three Theotokos sessions | ⛔ user |
| 8 | P12 (groove/generation), P13 (analog voice), P15 (effects) | unscheduled; P13 freeze is user judgment |

**Standing rule:** phase specs are written only when the phase is next to start
(front-load rule); every session may re-cut the order below it.

**Paired sessions** are a first-class roadmap instrument: one after each
milestone, notes captured append-only in `design/sessions/`, each producing
explicit roadmap deltas (or an explicit "no change").

---

## Roadmap

Per-phase narrative lives in the phase spec and report under
`design/phases/` — not in this table. Status here is one line.

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0–P9** | Skeleton → Modular Graph | See `architecture-evolving.md` phase log | **Complete** |
| **P9.5** | Device Emulation & Test Harness | Full Launchpad emulator (C1) | **Closed early** — C1 shipped; C2/C3 superseded by W0/W1; C4 rescoped into P10 C5 |
| **W0** | Theoria grid POC | `paraclete-antiphon` crate, WS bridge, canvas 8×8, LED mirror | **Shipped** — `w0-report.md` |
| **P10 C0–C1** | Pattern engine foundation | BUG-001/008 pre-flight; `Pattern` struct + serializer v3 | **Shipped** (`b0cf2c8`, `6212242`) |
| **W1** | Theoria MVP | Touch encoders, context display, transport, state mirror v1 | **C0–C4 shipped** — `w1-report.md` |
| **P10 C2–C5** | Pattern engine depth | Multi-page + page-loop, cued switching + chaining, per-track length/speed | **Shipped 2026-07-11** — `p10-report.md` |
| **W2** | Theoria editor | Cap-doc-driven param pages, chain view, view-plugin API (ADR-032) | **In progress** — C0–C6 shipped; C7 (§7.1 exit criteria) pending a formal pass |
| **WT** | Theoria/term | Terminal client over in-process Antiphon transport | After W2, parallel W3. Interacts with OQ-T12 |
| **W3** | Sequencer deep views | 64-step pattern view, cue/chain, hold-step p-lock overlay | Hard dependency on P10 C2–C5 |
| **P11** | Live Performance | Mute tiers, temp save/reload, kit model + Perform mode, live record | **ADR-039 ✅ accepted** + `p11-problem.md`. Phase spec after TK2 exit |
| **P12** | Groove & Generation | Retrig, Euclidean, controlled randomness, generative fills | — |
| **P13** | Analog Voice | Subtractive mono voice — Pro-One primary reference; paraphonic, per-voice-expression-aware | — |
| **P14** | FM Voice | Four-operator melodic FM, macro-first | **ADR-043 ✅ accepted**; depends on ADR-041 + ADR-042. Spec when next to start |
| **P15** | Effects Palette | Distortion variety, chorus/phaser/flanger, BBD/tape delay, spring/plate | — |
| **P16** | Macro Control | Instrument-wide macro system (terminal-surface half superseded by the TK track) | Also the target side of W5's MIDI learn |
| **W4** | Interface maturity | Ordo layout profiles, multi-client polish, protocol freeze, headless protocol CI | Ongoing after W3 |
| **W5** | Patch editor & control mapping | Drag-and-drop graph wiring with full node visibility; MIDI learn for arbitrary controllers | **Unscheduled.** Design spike → ADR → impl. Needs P16 macros for the target side. Every binding must declare a sync policy; pickup/soft-takeover is not an option (SPIKE-006) |
| **AN** | Anamnesis sampling layer | Capture-to-performance loop: recorder rings, sample pool, slices, scenes, staged timestretch | **ADR-040 ✅ accepted** + `design/sampling/`. AN2 depends on P11 KitStore |
| **TK** | Theotokos performance terminal | Keyboard-first Elektron-class virtual front panel — POC → usability-iterated TK0–TK3, session-gated | **TK0–TK2.1 shipped. TK2.2 code-complete 2026-07-30, not closed** — C6 (session #4) remains. Full narrative: `tk2.1-report.md`, `tk2.2-theotokos.md` |
| **TKW** | Theotokos window host | A platform-agnostic host for the Theotokos panel, so keyboard capability stops depending on the host terminal | **Unscheduled.** Three candidate routes — (a) native window, (b) panel compiled to WASM over antiphon *(strongest architectural fit; latency is the risk to measure)*, (c) hand-written browser panel *(weakest — duplicates `render.rs`)*. **Design spike + ADR first.** Bears on OQ-T12 |

The interface track (Antiphon server + Theoria clients) is specified in
`design/interface-plan.md` (**accepted July 2026**; ADR-031 authored with W0,
ADR-032 with W2). The terminal is a permanent first-class surface: **WT** ports
`paraclete-tui` to an Antiphon client over an in-process transport, gaining the
same generic views as the web client.

---

## Why P10 Still Matters — The Playability Gap

Unchanged from June 2026: "fun to play" per `instrument-vision.md` needs pages,
patterns, polyrhythm, and durable state. P10 closes this; P11 layers performance
affordances on top. What changed is *sequencing*: the foundation commits run
first, the depth commits after real session evidence.

The arc beyond P12 (synthesis P13–P14, effects P15, macro P16) is unchanged —
scope, not new architecture; P13 remains the keystone of the full four-pillar
instrument.

---

## Standing design tiebreakers

From paired session #2 and the 2026-07-12 vision pass:

- **Synthesis voices are the emotional core** — "sit down with a nice kick
  engine and tune it". Interface work starts from studied reference manuals,
  never improvised.
- **One graph, layered surfaces** (hardware-style performance / signal-flow
  graph / mouse+keyboard floor).
- **Every node has a graceful-degrading view** — ADR-032 is the *universal
  node-view contract*, not just engine param pages.
- **Two-tier engines** ("elisp of machines" — fast monolithic *and*
  graph-composed, never forced to choose).
- **Modulation is graph edges** — limitless LFOs/envs, no slot count.
- **No hardcoded counts in any frozen format.**
- **Tone (public repo):** *do* name Elektron / Hydrasynth as the aspirational
  bar — they set the standard. Frame humbly and clear-eyed: a small open
  project may not match their per-surface polish. The pitch is the
  *combination* (performance immediacy + open composability, one graph, any
  controller, free license), **never** superiority over a named product.

**Do not revive the Digitakt-as-encoder-controller idea.** Checked on hardware
2026-07-04 and disqualified: the Digitakt II transmits **absolute** CC and
Elektron's MIDI implementation has no relative mode
(`design/sessions/s0-hardware-checks.md`, Check 1). It survived in later docs as
a "standing offer" long after it was settled — it is neither. Paraclete's
relative-only encoder contract needs a controller that transmits knob *deltas*;
no such device is on hand. Digitakt remains a *design reference* for workflow,
per naming policy — never a control surface for parameter editing.
