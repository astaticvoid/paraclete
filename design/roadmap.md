# Paraclete — Roadmap

> **Living document.** Replace this file when a phase completes or significant
> planning changes occur. Keep it short — current state only.
>
> **Last updated:** 2026-07-23. **Elektron convergence redesign — ADR-038 ✅
> ACCEPTED** (ratified same day, D1–D4): Theotokos interaction model rebuilt
> as a virtual front panel — TRK/PTN hold-chords replace the `qweruiop`
> track row, REC grid-rec toggle + screens replace SEQ/PERF modes,
> continuous two-row trig grid (`q…i`/`a…k`) replaces the split home-row
> grid (split dropped entirely, D3), FUNC(Shift)-layer 8-encoder bank
> replaces slot-jog. Engine scope pre-approved (D2): live-trig command +
> `CANONICAL_PAGE_ORDER` → Digitakt order. **Session #2 held until the TK2
> S0 panel lands (D4)** and tests the new grammar, not the superseded TK1
> hypotheses (`\` leader, number-row patterns, `y`/`Y`). **Full TK2 spec
> drafted same day** (`design/phases/tk2-theotokos.md`): decisions D5–D14
> frozen (live-trig `CMD_TRIG_NOW=38`, sticky-prefix hold fallback,
> FUNC+transport copy/clear/paste scope, encoder bank, screen model,
> remap guardrails), commits C0–C9 + session C10, named tests per commit.
> Next: implement C0 onward.
> Previous: 2026-07-22. **TK1 CODE COMPLETE — C0–C7 shipped** (p-lock
> UI, mutes, composite pages, `:` command line, pattern select, yank/paste,
> `\` leader rebinding, yellow flash, `?` help overlay, suspend-crash fix).
> **TK1 C8** = usability session #2 (user-paired; no code). **60 Theotokos
> tests, 50 workspace suites green.**
> (session #1, `design/sessions/theotokos-1.md`; report + spec-divergence
> table `design/phases/tk0-report.md`). Theotokos is now the default
> surface (`cargo run`; `--emulator` for legacy). **TK1 spec drafted**
> (`design/phases/tk1-theotokos.md`, 8 commits + session #2) — **D1–D4
> ratified** (`paraclete-view-assembly` crate, sequencer-`mute` trig-gate,
> CMD 33–35 lock pair, `\` leader); **design review held — no blockers**,
> findings M1–M7/m1–m12 folded. BUG-034 filed and **FIXED 2026-07-22**
> (page-window stride; `GRID_STEPS=16`).
> Previous: 2026-07-21 (evening). **Theotokos track ACCEPTED** —
> keyboard-first modal performance terminal (`design/theotokos/`, ADR-036 ✅
> 2026-07-21). Review found+fixed pre-ratification: BUG-032 (transport
> unreachable — TK0 C0), BUG-033, 4-track default instrument, Antiphon-bound
> composite Rules. TK0 POC spec: `design/phases/tk0-theotokos.md`.
> Previous: 2026-07-21. **Theotokos track proposed** — keyboard-first
> modal performance terminal (`design/theotokos/`, ADR-036 🟡 proposed; awaits
> user ratification, then TK0 POC). A new track *alongside* the W-track, not
> a fork: WT unaffected, convergence revisited at TK3.
> Previous: 2026-07-20. **Paired session #3 held 2026-07-14** — went to
> Linux/ALSA audio bring-up instead of the W2 §7.1 exit-criteria pass; findings
> BUG-012 (Linux confirmation) + INFRA-004…007 (`design/sessions/s3.md`).
> **BUG-012 fix complete, pending hardware verification** — output ring buffer
> + FTZ/DAZ (`0f3d17b`), `BufferSize::Default` decision (`c3c56db`); the
> chunk-and-discard distortion path is gone. **INFRA-004 shipped** —
> `--no-emulator` headless mode (`c3c56db`). **W2 Commits 0–6 shipped** —
> `Rule` + `ViewPlugin` trait (L2), 7 node impls (L3), Antiphon `view_meta`
> protocol + composite assembly, web four-zone rail layout, param page grid
> rendering, chain view, ViewRegistry wired with real data. All 581 tests pass,
> web builds clean. **Next: W2 Commit 7** — formal §7.1 exit-criteria pass on
> the fixed audio path (session #3 did not run it).
> Previous: 2026-07-14. **BUG-012 shipped** — sample-rate auto-detection +
> callback chunking (later corrected: chunk-and-discard, not a ring buffer —
> see bugs.md). W2 Commits 0–6 shipped. 557 tests.
> Previous: 2026-07-13. **Debug/test harness (ADR-033) promoted to
> Rank 2.** The agent lane pivots from the (completed) universality audit to
> building live-engine interrogation, a null audio backend, audio snapshot/diff,
> and a structured log channel — so W2, P13, and every later feature are
> developed and tested against real engine state (quality upfront, not late
> bug-fixing). W2 *code* now gates on it; the W2 *spec* (Rank 1, user-paired) is
> unaffected.
> Previous: 2026-07-12 (late). **Vision crystallized + W2 groundwork
> done.** North-star thesis captured in `instrument-vision.md` ("Performance Meets
> Limitless Composability"): one graph, layered surfaces (hardware-style
> performance / signal-flow graph / mouse+keyboard floor); every node has a
> graceful-degrading view; "limitless elisp of machines" = two-tier engines (fast
> monolithic + graph-composed); modulation IS the graph; **no hardcoded counts in
> any frozen format.** W2 reference spike complete
> (`design/specs/w2-reference-analysis.md` — all four manuals, 8 convergent
> patterns, decision menus). **§6.0 axis resolved** (layered, not either/or).
> **ADR-032 reframed** as the universal node-view contract. **Active agent work:
> pre-W2 universality/hardcoded-count audit** (must land before the interface/
> protocol/serializer freeze). D1 (W2 spec+ADR-032) and D2 (P13, now two-tier
> framed) remain **user-owned/paired**. See "Active Priorities" below.
> Previous: 2026-07-12 (ADR-034 implemented: D3/D4 closed, INFRA-003 resolved;
> RuntimeCounters + `/engine/*`; both counter paths measured quiet; BUG-012
> confirmed as a hard crash via the live-audio load test).
> Previous: 2026-07-10 evening (paired session #2 held on glass)
> **Current phase:** Legibility phase shipped and judged on the iPad over a **USB-C direct link (3.0 ms RTT, zero config — the no-shared-Wi-Fi answer)**. Session #2 verdict (`design/sessions/s2.md`): "improved a lot… it will be the baseline" but **far below bar — the Launchpad-grid mirror is the wrong foundation for Theoria**. New sequence: **(1) BUG-022/023** (seq-vs-trigger kick pitch mismatch; fast-retrigger ducking — sound correctness moves first), **(2) baseline interaction wins** (drag-draw steps, encoder gesture/placement, hide dead grid), **(3) W2 re-scoped → "Theoria native surface", design-first**: paired reference spike (chosen Elektron box manual + Hydrasynth manual → fixed-input rail + contextual window spec, param/env/LFO pages, source→FX channel view) before any further W-feature code; ADR-032 follows the spec. **Launchpad parked** (good version frozen; s1-F7 cleanup → trigger backlog). P10 C2+ engine depth independent/parallel; P13 keystone unchanged. Earlier 2026-07-10 work: legibility items + BUG-016…021 fixed + open-by-default `--token` opt-in (`theoria-legibility-report.md`). BUG-012 still queued for a hardware session.

---

## Active Priorities (2026-07-12) — triage against the vision

Ranked. Rank 1 is the critical-path **design** milestone (user-owned, paired);
Rank 2 is the active **agent** task — now the debug/test harness, which gates W2
*code* and de-risks every subsequent feature. **A fresh agent starts here.**

| Rank | Work | Owner | Status | Notes |
|---|---|---|---|---|
| **1** | **W2 surface spec + ADR-032** — the universal node-view contract; the layered-surface model | **user (paired)** | **✅ ratified 2026-07-13** | Spec accepted. ADR-032 accepted. W2 implementation begins. |
| **2** | **Debug/test harness (ADR-033/ADR-035)** — structured per-node debug log, regression baselines, CPU meter | **agent** | **COMPLETE 2026-07-13** | ✅ null backend + REPL (ADR-033, `92b8795`), ✅ regression baselines (ADR-035 Part A, `b74b853`), ✅ structured per-node debug log (ADR-035 Part B, `73332d5`), ✅ CPU meter /engine/cpu_us (`aad9e52`). W2 code gate lifted. |
| **3** | **P13 voice: OQ-13 + OQ-14** — two-tier engine model | **user** | brief drafted | `w2-reference-analysis.md` P13 appendix. OQ-13 (monolithic vs composed-from-primitives) is **coupled to §6.0** — decide together. OQ-14 = machine-as-parameter (recommended: a topology swap is an audible gap, measured via BUG-012 load test). Now developed/tested against the Rank 2 harness. |
| **4** | **Openable engines** — Tier-1 monolithic becomes graph-openable | user (later) | north-star, parked | The deepest "elisp of machines." Needs GraphNode / `InnerGraphNode::serialize()` maturity (a stub today). P13→P14+; **not** a W2 gate. |

**Recently completed (former Rank 2):** the pre-W2 universality / hardcoded-count
audit shipped 2026-07-12 — 6 findings triaged by permanence; **U1** (`&'static
str` across the L2 API → `Cow<'static, str>`, both `Box::leak` sites deleted, ~85
sites migrated, 554 tests green) unblocked dynamic per-client Theoria surfaces for
W2. Detail: `design/review/universality-audit.md`. **U2** (Launchpad-shaped
surface consts → descriptor-driven) folds into ADR-032.

**Fresh-agent reading order:** `instrument-vision.md` ("Performance Meets
Limitless Composability") → this section → **`design/phases/w2-interfaces.md`**
(W2 spec) → `design/adr/ADR-032-theoria-view-plugin-api.md` → `handoff.md`.
The user owns Ranks 1/3/4 (paired); the agent's lane is W2 implementation
(once the user ratifies the spec + ADR-032 freeze). **Do not author the W2
spec or ADR-032 further solo — they are drafted, next step is user
ratification.**

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

## Implementation Order & Design Gates (2026-07-23)

The current execution sequence, with **⛔ gates** where work pauses for
user design input (ratification, musical judgment, or a paired session).
Agent-executable stretches need no input between gates.

| Step | Work | Gate before proceeding |
|---|---|---|
| 1 | **TK2 C0–C9** (`tk2-theotokos.md` — panel, live-trig, encoder bank, screens, remapping, live viz) | none — spec is execution-ready |
| 2 | **⛔ Session #2** (TK2 C10, user-paired) | grammar verdicts; OQ-T22/T23/T24 |
| 3 | **⛔ TK2-exit scheduling pass** (user) | order the parallel tracks: P11 spec → impl; AN0(→AN1); ADR-041+042 implementation. All three are independent of each other |
| 4a | **P11**: spec (agent, session-informed) → **⛔ spec ratification** (kit UX OQs) → impl → session | two gates |
| 4b | **AN0–AN1** (pool → capture; R2 transition-trick gate on AN1 exit) → **⛔ sampling session** | AN2 additionally needs P11 KitStore shipped |
| 4c | **ADR-041 + ADR-042 impl** (machine select, MOD page — mechanical vs the ADRs) | none until P14 |
| 5 | **P14**: spec — **⛔ user freezes the musical tables** (algorithm routing, ratio set, `harm` waveforms, macro machine set) → impl → **⛔ baseline patches + session** | needs 4c |
| 6 | **W-track residuals**, any time a session is convened: W2 C7 §7.1 exit pass; BUG-012 hardware verification | ⛔ paired session |
| 7 | **TK3 / WT convergence decision** (OQ-T12), after three Theotokos sessions | ⛔ user |
| 8 | P12 (groove/generation), P13 (analog voice — remaining: **⛔ feature-set freeze**, OQ-15 allocator), P15 (effects) | unscheduled; P13 freeze is user judgment |

Standing rule: phase specs are written only when the phase is next to
start (front-load rule); every session may re-cut the order below it.

**Near-term sequence (historical, W-era — kept for the record):**

| Order | Work | Why |
|---|---|---|
| 1 | ~~P10 C0 pre-flight~~ — **shipped** (BUG-001 re-diagnosed via measurement harness; BUG-008 fixed) | Done |
| 2 | ~~**W0**~~ — **shipped** (Theoria grid POC: `paraclete-antiphon` crate + canvas grid) | Done |
| 3 | ~~**P10 C1**~~ — **shipped** (`6212242`; `Pattern` struct + serializer v3 = BUG-005) | Done — data-loss class closed before sessions |
| 3.5 | ~~**BUG-012**~~ — **fix complete, pending hardware verification** (2026-07-14): sample-rate auto-detection, output ring buffer + FTZ/DAZ (`0f3d17b`), `BufferSize::Default` decision (`c3c56db`). The earlier "ring buffer fallback" claim was a doc error (chunk-and-discard) — corrected in bugs.md. Awaiting Linux ALSA re-test. | Pending verification — see bugs.md |
| 4 | ~~**W1**~~ — **C0–C4 shipped** (trigger+velocity, path scheme, state mirror, semantic plane, theoria-web) | Runtime side done + web client builds; C5 = the session |
| 5 | ~~**Paired session #1**~~ — **held 2026-07-09** (`design/sessions/s1.md`) | Pipe proven; verdict = UX not legible ("Behringer, needs Elektron"). Delta: discoverability is the keystone |
| 6 | ~~**Theoria legibility phase**~~ — **implemented 2026-07-10** (`7e7a39a`/`e553c62`; report: `theoria-legibility-report.md`) | Minimum bar items 1–4 done + contextual encoders; judged live in Chrome. **Exit gate = paired tablet judgment (next session)**; F7 cleanup + F4/save-reload deferred |
| 6.1 | ~~**BUG-022 + BUG-023**~~ — **shipped** (BUG-022 `5071c9a`, BUG-023 exonerated 2026-07-11) | Sound correctness resolved; BUG-023 confirmed macOS speaker protection via headphone A/B |
| 6.2 | ~~**Theoria baseline interaction wins**~~ — **shipped 2026-07-11** (`e1f86cf` + `3356d60`; report: `theoria-baseline-interactions-report.md`) | Drag-draw paints via new authoritative `/script/lp/steps_n` mask mirror; encoder row at bottom edge w/ value bars; dead grid gone (mode-aware cells). Found+fixed BUG-024 (state_write in subscriptions panicked). Tablet judgment pending |
| 6.3 | **W2 re-scoped: Theoria native surface, design-first** — reference spike across three Elektron boxes + Hydrasynth: **Digitakt II** for sample workflows, **Syntakt** for analog/synthesis machine-per-track param pages, **Digitone** for FM page discipline; **Hydrasynth** for signal-chain (source→effect graph) view. Common views (envelopes, LFOs, effects, sequencer) from any. ADR-032 after the spec | Session #2 F1/F7 keystone: "consistent inputs, contextual screen window"; do not improvise UI |
| 7 | ~~P10 C2–C5~~ — **shipped 2026-07-11** (`0a8116b`/`e8f7718`/`50ef64b`/`5306674`; report: `p10-report.md`) | Pattern engine complete: page-loop windows, per-track length/speed (polyrhythm), BUG-004 fixed, cued switching + chain, state-bus surface + TUI indicator. §5.3 Launchpad surface parked per s2; P10 play-test pending (Theoria/W2 or paired TUI session) |

**Paired sessions** are a first-class roadmap instrument from here on: one after
each W-milestone (W1, W2, W3), notes captured append-only in `design/sessions/`,
each producing explicit roadmap deltas (or an explicit "no change").

---

## Design Triage — Open Spec Gaps

> **See "Active Priorities" above for the current ranked view** — it folds this
> triage against the crystallized vision. This section keeps the per-gap detail.

Things the plan gestures at but does **not** yet specify. Triaged 2026-07-12.
**Owner** = who must author it: **user** (design judgment, frontier-tier per
`handoff.md`) or **agent** (mechanical / low-judgment, safe to draft in-session).
Ordered by nearness to the critical path.

| # | Gap | Owner | Next action | Priority |
|---|---|---|---|---|
| D1 | **W2 native surface has no spec.** "Theoria native surface" is a paragraph (fixed-input rail + contextual window; reference spike across Digitakt II / Syntakt / Digitone / Hydrasynth). It is the active next milestone. | **user** | **✅ Done** — spec + ADR-032 ratified (accepted 2026-07-13); W2 Commits 0–6 shipped. Residual: W2 Commit 7 (§7.1 exit-criteria pass, needs a paired session). *(Row was stale "pending ratification" until 2026-07-23.)* | ~~Critical~~ ✅ Done except C7 exit pass |
| D2 | **P13 voice model undecided (OQ-13) + drum selection (OQ-14).** Both deferred to "the P13/P12+ spec," which is not drafted. OQ-13 (composed-from-primitives vs monolithic `AnalogVoice`) shapes the mod-matrix API and CLAP export. | **user** | **OQ-13 + OQ-14 resolved 2026-07-13.** OQ-13: compose from primitives → compile to monolith (reversible). OQ-14: machine-as-parameter (stepped param, ADR-019). Voice feature-set freeze, allocator expression-awareness policy, ZDF ladder C1 still open. | High (downstream keystone) — **partially resolved** |
| D3 | **No ADR owns runtime observability.** ADR-033 covers only the offline/interactive driver. Nothing specs the live `/engine/cpu` counter path or the structured-log channel (see **INFRA-003**, bugs.md). | agent | **✅ Done (2026-07-12).** ADR-034 authored + implemented: `RuntimeCounters` with 4 atomic counters, state-bus `/engine/*` publishing, Antiphon mirror. INFRA-003 resolved; D4 unblocked. | ~~High~~ ✅ Done |
| D4 | **Trigger-based backlog is un-actionable as written.** It claims "each item has a named trigger," but INFRA-003 shows we cannot *observe* triggers firing — so "quiet" is assumed, not measured. | agent | **✅ Done (2026-07-12).** Backlog flagged blocked-on-INFRA-003 in earlier pass; INFRA-003 now resolved with live counters. Triggers are now observable. | ~~Medium~~ ✅ Done |
| D5 | **BUG-031 residual has no rule.** ADR-030 now documents speed×swing, but not swing large enough to overshoot the step at 1× speed. | agent | One sentence in ADR-030: clamp policy (or explicit "author's responsibility") for `swing_amount` beyond the step fraction. | Low |
| D6 | **`Cow<'static, str>` migration has no trigger.** Flagged "FREE until crates.io publication, do before v0.1.0" but sits in the Known Provisional table with no owner/milestone — the pre-publication window can close silently. | agent | Promote it to a Deferred-Bug Backlog row with a concrete trigger ("before `paraclete-node-api` v0.1.0 / first crates.io publish"). | Low (but time-boxed) |

**Progress (2026-07-12):**
- **D3** ✅ done — ADR-034 authored (proposed), implemented: `RuntimeCounters`
  with 4 `AtomicU64` counters (`buffers_processed`, `dropout_lock_miss`,
  `dropout_no_executor`, `state_bus_overflows`), shared via `Arc` between
  audio callback and executor, published to state bus as `/engine/*` paths,
  mirrored by Antiphon. INFRA-003 resolved. 4 unit tests in
  `runtime_integration.rs`. D4 unblocked.
- **D4** ✅ done (earlier pass). **Measured quiet 2026-07-12 — all four counters,
  both paths.** Executor path: `engine_counters_quiet.yaml` (8 s busy 4-track
  render) → `state_bus_overflows = 0`. Callback path: the load test
  `patch_tests.rs::loadtest_topology_churn_under_live_audio` (real cpal, 703 live
  `apply_patch` swaps in 15 s) → `dropout_lock_miss = 0`, `dropout_no_executor =
  0`. The ADR-029 pause-rebuild-resume protocol produces zero self-inflicted
  dropouts under stress. Bonus: the load test surfaced **BUG-012** as a hard
  audio-thread crash on buffer-size mismatch (not graceful degradation) — see
  bugs.md 2026-07-12 confirmation.
- **D5** ✅ done (earlier pass).
- **D6** ✅ done (earlier pass).
- **D1, D2** — open, **user-owned** (unchanged).

---

## Roadmap

| Phase | Name | Deliverable | Status |
|---|---|---|---|
| **P0–P9** | Skeleton → Modular Graph | See `architecture-evolving.md` phase log | **Complete** |
| **P9.5** | Device Emulation & Test Harness | Full Launchpad emulator (C1). | **Closed early** — C1 shipped; C2/C3 cancelled (superseded by W0/W1); C4 rescoped into P10 C5 test work; piano mode deferred (physical Keystep exists) |
| **W0** | Theoria grid POC | Browser grid as peer device: `paraclete-antiphon` crate, WS bridge, canvas 8×8 + scene + control, LED mirror, shared `launchpad.rhai` profile | **Shipped** (July 2026; report: `w0-report.md`; localhost touch→LED 24–34 ms; exit criteria needing tablet/Launchpad hardware roll into the next user session) |
| **P10 C0–C1** | Pattern engine foundation | BUG-001/008 pre-flight (C0, runs before W0); `Pattern` struct + serializer v3 = BUG-005 (C1) | **Shipped** (C0 `b0cf2c8`, C1 `6212242`) |
| **W1** | Theoria MVP | Touch encoders (relative → `CMD_BUMP_PARAM`), context display, transport, state mirror v1 → **paired session #1** | **C0–C4 shipped** (`w1-report.md`); C5 = paired session #1 (next) |
| **P10 C2–C5** | Pattern Engine depth | Multi-page (64-step) + page-loop; seamless switching + chaining; per-track length/speed; BUG-004 **+ BUG-013 (sub-block voice starts — micro-timing must be audible) + Sampler Hermite playback** in C3; grid/TUI surface | **Shipped 2026-07-11** (C2–C5 + BUG-004; BUG-013 landed post-C5: engines `309a9e6`, Sampler Hermite + span-split 2026-07-11; BUG-025 executor deferral and BUG-026 stable sort fixed 2026-07-11) |
| **W2** | Theoria editor | Cap-doc-driven parameter pages for every engine; chain view; view-plugin API (ADR-032) → **paired session #3** | **In progress** — Commits 0–6 shipped (types, node impls, protocol, web rail, param pages, kick vertical slice, chain view); Commit 7 (§7.1 exit criteria + s3.md) next — session #3 held 2026-07-14 went to ALSA bring-up; formal pass pending |
| **WT** | Theoria/term | Terminal client over in-process Antiphon transport; parameter pages + grid in the terminal | After W2, parallel W3 |
| **W3** | Sequencer deep views | 64-step pattern view, cue/chain, hold-step p-lock overlay, condition/timing editors | Hard dependency on P10 C2–C5 |
| **P11** | Live Performance | Mute system (global/pattern/prepared tiers), temp save/reload, kit model + Perform mode, live record | **ADR-039 ✅ accepted 2026-07-23** (R1–R3: model as written, whole-instrument kits, TK2 implementation first) + `p11-problem.md`. Kits = app-owned param snapshots, app-op drain, sequencer CMD 39–45 reserved. Phase spec after TK2/session #2. (W3 mute view follows) |
| **P12** | Groove & Generation | Retrig, Euclidean, controlled randomness, generative fills | — |
| **P13** | Analog Voice | Full subtractive mono voice — **Pro-One as primary reference** (dual osc + hard sync + poly-mod routing, self-oscillating 4-pole, two envs, glide, arp); Model D / MS-20 as secondary character references only. Paraphonic allocation, **per-voice-expression-aware (OQ-15)**. ZDF ladder is C1 (audio-model review). | — |
| **P14** | FM Voice | Four-operator melodic FM, macro-first | **Model ✅ accepted 2026-07-23** — ADR-043 ratified as written (4-op PM, 8 algorithms, SYN1/SYN2 page discipline, machine-variant macro tier); depends on ADR-041 (machine identity) + ADR-042 (MOD page), both ✅ accepted same day. Phase spec when next to start |
| **P15** | Effects Palette | Distortion variety, chorus/phaser/flanger, BBD/tape delay, spring/plate | — |
| **P16** | Macro & Terminal Control | Macro system; ~~TUI as editing surface~~ (terminal-surface half **superseded 2026-07-23** — delivered/owned by the TK track (ADR-036/038) and the WT convergence decision; P16 narrows to the instrument-wide macro system, which ADR-043's macro tier explicitly defers to) | — |
| **W4** | Interface maturity | Ordo layout profiles, multi-client polish, wavetable view, protocol freeze, headless protocol CI driver | Ongoing after W3 |
| **AN** | Anamnesis sampling layer | Capture-to-performance loop: HAL input + recorder rings (retroactive capture, resampling), app-owned sample pool + per-step sample locks, slices/chains, scenes + crossfader morph, pickup-style looping, staged timestretch — AN0–AN3, session-gated | **ADR-040 ✅ accepted 2026-07-23** (R1–R3: model as written; one-gesture transition trick frozen as an AN1 requirement; scheduling decided at TK2 exit) + `design/sampling/{problem,design}.md`. AN2 scenes depend on P11 KitStore |
| **TK** | Theotokos performance terminal | Keyboard-first Elektron-class virtual front panel (continuous trig grid, TRK/PTN hold-chords, REC grid-rec toggle, FUNC-layer encoder bank, `Rule`-driven terminal views) — POC → usability-iterated phases TK0–TK3, session-gated | **TK0 shipped 2026-07-21** (ADR-036). **TK1 code complete 2026-07-22** (C0–C7: p-locks, mutes, composite pages, `:` line, pattern select, yank/paste, leader rebind, flash, help overlay, suspend-crash fix). **Elektron convergence redesign 2026-07-23 (ADR-038 ✅ accepted, D1–D4)** — TK2 re-cut: S0 virtual front panel, key remapping (ADR-037, re-based), CHAIN + TEMPO screens, live-trig command (OQ-T20), live viz, ramp retune. C8 = usability session #2, held until S0 lands (D4), on the ADR-038 grammar. |

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

> **D4 (2026-07-12):** ADR-034 made the engine's dropouts/overflows observable,
> and both paths are now **measured quiet** (no longer assumed): executor path
> `state_bus_overflows = 0` (`engine_counters_quiet.yaml`, 8 s busy render);
> audio-callback path `dropout_lock_miss = 0` / `dropout_no_executor = 0` under
> 703 live topology swaps (`loadtest_topology_churn_under_live_audio`, real cpal).
> The BUG-002/BUG-012 buffer-size trigger, however, is now known to **crash**
> (debug) rather than degrade — pull it forward if any external interface is in play.

| Bug | Fix when this trigger fires |
|---|---|
| BUG-002 (`baseline()` hardcodes 44100/512) | First non-44.1 kHz/512 deployment, or CLAP host reports mismatch — whichever first |
| BUG-003 (hal→runtime layer violation) | Before any LGPL3/crates.io publication of `StateBusHandle` consumers, or when `paraclete-antiphon` wants `StateBusHandle` from L2 |
| BUG-006 (`agg_state_buf` realloc/cycle) | When the web state mirror measurably raises state-bus churn (profile in W1), or any audible xrun traced to it |
| BUG-007 (`publish_bank_state` String alloc) | Folded into W1 C1 (state-path unification) |
| Executor cell `Mutex` → `arc-swap`; master limiter/headroom; xrun/CPU meter to `/engine/cpu`; FM 2-sample feedback average; DT node `rtrb` harmonization | See `design/review/audio-model-review.md` — each has a named trigger there |
| ~~`Cow<'static, str>` L2 migration~~ **DONE 2026-07-12 (audit U1)** | **Shipped.** `CapabilityDocument`/`SurfaceDescriptor` name/vendor/extensions → `Cow<'static, str>`; both `Box::leak` sites in `bridge.rs` deleted. `ParamUnit`/`PortName`/`ParamDisplayAdapter` already modeled the hybrid. See `design/review/universality-audit.md` U1. |

Everything else open is scheduled: BUG-001/008 → P10 C0 (now), BUG-005 → P10 C1
(now), BUG-004 → P10 C3.

---

## Known Provisional Implementations

| Item | Status | Resolution | Target |
|---|---|---|---|
| `CMD_SET_PATTERN` stub (always pattern 0) | **Fixed (P10 C4)** | Multi-pattern with cued switching + chain (`sequencer.rs` `switch_pattern`/`CMD_SET_PATTERN`) | Done |
| Single-pattern, 16-step only | **Fixed (P10 C2–C5)** | Pattern engine: pages (64-step), per-track length/speed, page-loop windows | Done |
| `Sequencer::serialize()` drops P5 fields (BUG-005) | **Fixed (P10 C1 shipped)** | Serializer v3 (`6212242`); length-prefixed step + pattern records | Done |
| BUG-008 / BUG-001 | **Fixed (P10 C0 shipped)** | s0 re-diagnosis: 240-tick step + step-0 fire + drift-only snap; mem::take | Done |
| Negative micro-timing == zero (BUG-004) | **Fixed (P10 C3)** | Emitted in prev step's early-fire window | Done |
| Terminal emulator: no RGB, no keyboard encoders | ~~Accepted permanent~~ **SUPERSEDED 2026-07-23** | The "terminal stays keyboard-grid-only" premise is dead: Theotokos (ADR-036/038) made the terminal a first-class performance surface with a FUNC-layer 8-encoder bank. The Launchpad *emulator* itself remains legacy (`--emulator`) with its known audio-thread-input defect | — |
| No headless input injection for CI | Active | In-process injection API, built with P10 C5 surface tests; protocol-level driver at W4 | P10 C5 |
| Encoder hardware (EN16/MFT) unpurchased | **De-escalated** | W1 touch encoders are the only relative path (session 0: Digitakt II verified absolute-only, disqualified; BUG-009 filed); buy a true-relative box later for tactile feel | Post-W1 |
| Hard-coded app node IDs as script/UI contract | Active | W-track binds by discovery (`hello`/`topology` msgs); profiles migrate when it breaks | W2 |
| AnalogEngine/FmEngine monophonic | Active | Voice allocator | P13 |
| L2 `&'static str` in `CapabilityDocument`/`SurfaceDescriptor` (name, vendor, extensions) | **Fixed 2026-07-12 (audit U1)** | Migrated to `Cow<'static, str>`; both `Box::leak` sites deleted; ~85 sites `"x".into()`. Dynamic surfaces (Theoria per-client) now unblocked. | Done |
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

## Agent Infrastructure Gaps (July 2026)

An agent working on this codebase cannot currently:

| Gap | Impact | See |
|---|---|---|
| **Debug harness** | Can't interrogate engine state live — no REPL to send commands, read params, watch changes, or measure peaks while running | ADR-033 § interactive mode |
| **Null audio backend** | Can't run graph without a physical audio device — blocks CI and headless agent runs | ADR-033 § prerequisite |
| ~~**Artifact detection**~~ | **Closed 2026-07-12 (INFRA-001):** `discontinuity_lt`/`dc_offset_lt`/`dropout_lt_ms` assertions + NaN/Inf checks in the test driver | Shipped `9655cd0` |
| ~~**Audio diff/snapshot**~~ | **Closed 2026-07-13 (ADR-035 Part A):** deterministic baseline fingerprints (peak/rms/dc + 50ms envelope), `--check-baseline` exits 1 on drift, CI-ready | Shipped `b74b853` |
| **Structured log channel** | No per-node debug events (step fires, voice triggers, param changes) — state bus is push-only per cycle | Buildable on ADR-033 `watch` |
| **CPU/xrun meter** | Can't tell if a change degrades performance — each trigger named in audio-model review, none have fired **because our own dropouts are invisible** (INFRA-003) | Trigger-based backlog below; **INFRA-003** (bugs.md) is the concrete first step |
| **Live dropout/xrun detection** | Audio callback fills silence on `try_lock` miss / missing executor with no counter or log; state-bus SPSC overflow drops silently. "No trigger has fired" is unproven, not confirmed | **INFRA-003** — atomic counters on the `fill(0.0)` paths + `/engine/cpu`; lock-free, does **not** need the full ADR-033 harness |

The debug harness (ADR-033 interactive mode) unblocks these. It is the
keystone, and as of 2026-07-13 it is **Rank 2 in Active Priorities** — the active
agent lane, sequenced to land before W2 code so W2/P13 and every later feature
are built and tested against live engine state. **INFRA-003** (live dropout/xrun
counters) was the cheap independent first step and **shipped** with ADR-034
(counters measured quiet); the remaining debug-posture work — ~~null backend →~~
~~interactive mode →~~ ~~audio diff/snapshot~~ → structured log, plus a CPU-% meter — is
tracked under that Rank 2 entry.

---
## Agent Tooling Investigation Spikes (July 2026)

These are proposed tooling changes that carry risk or trade-offs. Do not
implement without investigation; each needs a spike to confirm feasibility.

| # | Spike | Risk | Investigation needed |
|---|---|---|---|
| SPIKE-001 | **`unwrap_used = "deny"` lint** | 148 call sites across the workspace. Many unwraps in the audio hot path reflect invariants guaranteed by graph topology (e.g., `connect()` validation before `process()`). Denying them forces either `unsafe` or per-cycle `Result` allocation — both worse than the status quo. | Catalog unwraps by category (hot-path invariant vs error-recoverable). Determine which could become `expect()` with meaningful messages. Time-box: 2h audit. |
| SPIKE-002 | **`panic = "deny"` lint** | 28 panics, mostly `published_state()` test assertions in production source files (not behind `#[cfg(test)]`). A blanket deny would either break the build or require a migration of all test assertions to test-only gating. | Extract `published_state()` test assertions into `#[cfg(test)]` blocks or separate test modules. Confirm no production panics remain. Then re-evaluate the lint. |
| SPIKE-003 | **Layer-boundary lint enforcement** | `clippy`'s `disallowed-dependencies` only works on external crate names, not workspace-internal deps. A custom `cargo-deny` config or hand-rolled script is needed. Even then, `paraclete-hal` → `paraclete-runtime` is an acknowledged violation (BUG-003) that the spec itself permits. | Prototype a `cargo tree`-based check script in `tools/`. Decide whether it gates CI or is advisory-only. |
| SPIKE-004 | **Remove `default-members`** | The single `paraclete-app` default was likely intentional for cross-platform portability. Some crates (e.g., `paraclete-clap-host`) may pull platform-specific deps that don't compile on all targets. Blowing open to `--workspace` could regress `cargo build` on non-macOS platforms. | Test `cargo build --workspace` on Linux and Windows (or review dependency chains for platform-gated crates). The `.cargo/config.toml` aliases added in this session are the safe alternative. |
| SPIKE-005 | **`insta` snapshot tests for TUI** | Terminal rendering varies by terminal type, color support, and unicode width. Even with `ratatui::TestBackend`, insta snapshots create diff noise on any rendering change — intentional or not. Maintenance cost may outweigh value unless the TUI is under active refactoring. | Add one snapshot test for the grid render path. Run for one week of active development; measure snapshot churn vs catch rate. Decide after the trial. |

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
