# Theoria Legibility Phase — Implementation Report

**Session:** 2026-07-10, autonomous (driver AFK; Claude driving the web client
live over the Chrome connector against a local build).
**Commits:** `7e7a39a` (runtime/server), `e553c62` (web client), plus this
docs commit.
**Tests:** 470 workspace (was 466), 0 failures. Clippy clean on touched hunks.

## Scope delivered (s1.md roadmap-delta minimum bar)

| s1 item | Status |
|---|---|
| 1. Labeled first-class track select (F8/F9 keystone) | **Done** — `TrackBar`: number + name per sequencer from the welcome topology; highlight follows mirrored `/script/lp/selected`; tap still speaks the SHIFT+pad gesture (one profile, every surface) |
| 2. Visible mode (F2) | **Done** — TRIG\|STEP segmented switch (idempotent) + `GridHeader` sentence; `/script/lp/mode_n` numeric twin mirrored |
| 3. Selected-track ↔ grid legibility | **Done** — header names the edited track; pads print step numbers (STEP) / track names (TRIG); active-region outline |
| 4. Topology-driven track count (F5) | **Done** — 4 tracks on the default graph, zero dead buttons; welcome/topology is the only source |
| 5. LED-routing cleanup (F7) | **Not done** — deliberately deferred; the `main.rs:297` mirror + `LP_DEVICE_ID` hardcode still stand. F3 log spam fixed independently (warn-once). |
| 6. Semantic pattern channel (F4) / on-glass save-reload | **Not done** — both need protocol additions (Text state frames / new messages); protocol changes gate on the user per handoff guardrails |

Beyond the minimum bar, the s1 exit criterion "select kick → encoders show
kick params → drag changes sound" now works end-to-end: the app auto-publishes
`/context/encoder_*` for the selected track's generator from its cap doc, and
the encoder row drives `bump_param` (verified tone 4000 → 3817 over the wire).

## W1 gaps found and fixed (see BUG-016…020 in bugs.md)

Driving the client for real immediately surfaced four wire-level defects that
headless tests and session #1 missed: no state replay on connect (BUG-016),
the encoder surface-plane gate never opened (BUG-017), the absolute
`bump_param` delta cap freezing wide-range params (BUG-018), and the
context-slot key mismatch (BUG-019), plus a client layout race (BUG-020).
**Every one of them sat between "tests pass" and "a human can use it" — the
exact gap session #1's verdict named.**

## Spec conflict record (handoff guardrail 1)

`w1-interfaces.md` §Commit 4 places encoder detent→delta scaling in the
profile: "`launchpad.rhai` maps `EncoderChanged` ids 90–97 → `CMD_BUMP_PARAM`
on the context-mapped param, delta × per-param step = (max−min)/256". Profiles
have no capability-document access, so they cannot know `(max−min)` — the
sentence is unimplementable as written. Resolution chosen: the **client**
computes the delta from the welcome snapshot's param ranges and sends the
semantic `bump_param` that the same spec's Commit 3 built (server re-validates
against cap docs and clamps). The raw `enc` surface event still flows (gate
open, unmapped encoders fall back to it), so a future hardware-encoder profile
path stays available. **User review requested** — if profile-side mapping is
still wanted, the missing piece is cap-doc (or at least param-range) access
for Rhai.

## Decisions of record

- `ContextSlot.enc` = encoder **slot index 0–7**, not a surface control id
  (documented on both protocol structs). Closes the W1 C2 "boring documented
  choice" left for C4.
- `/script/*` **numeric** values are mirrored to Antiphon clients; Text values
  remain unmirrored. Profiles that want text-ish state on clients publish a
  numeric twin (`/script/lp/mode_n`). CLAUDE.md path table updated.
- `NodeSummary.name` prefers the instrument-file `display_name` over the
  cap-doc per-type constant.
- Auto-context lives in the **app** (only layer holding cap docs + the
  track→generator map); instrument-file `macros` stay authoritative when
  declared.

## Known gaps / notes for the next session

- **Tablet judgment pending.** Everything above was judged visually in Chrome
  at desktop size. The Elektron bar is called on the actual tablet, paired —
  glass ergonomics (button sizes, drag feel, two-finger fine mode) untested.
- Mode switch double-tap race: a second tap before the ~33 ms mirror
  round-trip re-toggles. Acceptable at 14 ms LAN RTT; revisit if it annoys.
- `ENCODER_SLOTS = 8` is hardcoded in three places (app, TUI config, client);
  the wire (`encoder_{i}` keys) supports any count. Universality note: derive
  from the surface descriptor before any format freeze.
- The review workflow deviated: subagent quota was exhausted mid-session, so
  the pre-commit review was a documented inline self-review (5 angles) instead
  of independent finder agents. One finding applied (derived `tracks` state);
  re-run a full multi-agent review when quota returns if desired.
- Server restart while a client is connected: STALE overlay + reconnect +
  full replay verified working.

## Addendum (same session): tablet-typeable session code (E2)

Driver returned and hit the E1 sibling: the tokened URL is untypeable on
glass, and the bare URL dead-ended in STALE. Shipped in `2daceac`:

- `.antiphon-token` is now a **6-digit code** (auto-rotates from the 32-hex
  format). Posture trade recorded: home-LAN threat model, `--open` still the
  zero-token mode, delete the file to rotate.
- **TokenGate**: the bare URL shows a code-entry card on handshake rejection;
  the code persists in localStorage — a tablet types 6 digits once, ever.
  `?t=` still wins over the stored code (desktop click-through unchanged).
- The bare-URL STALE dead-end was **BUG-021**: two status-stomp paths in
  `connection.ts` meant no hard-error screen (bad token, protocol mismatch)
  could ever render. Found by driving the exact flow the driver described.

## Addendum 2 (same session): open by default; token is opt-in (user decision)

"Why does the session need a code at all?" — it doesn't, today: the protocol
reaches music state only. Default flipped by user decision (`--token` opts
into the 6-digit code for untrusted networks; `--open` is a legacy no-op).
Open mode now accepts **any** client token, so a code remembered from an
earlier `--token` run never bounces a client. **Standing note: revisit the
default when the protocol gains project save/overwrite** — that is the point
where unauthenticated control starts costing real work.

Also added: a `by name: http://<Bonjour-name>.local:7274/` stderr line
(scutil on macOS; first hostname label elsewhere). This is step one of the
no-shared-Wi-Fi story (below) — mDNS resolves on any shared link, including
a direct ethernet cable with link-local addresses, where the routed-IP guess
(`lan_ip()`) prints the wrong interface.

### Open item: tablet link without shared Wi-Fi (user request, next hardware session)

Candidate paths, in rough order of promise:
1. **Direct ethernet cable** (tablet USB-C→ethernet adapter ↔ Mac): zero
   config — both ends self-assign link-local 169.254/16, mDNS resolves
   `Nimbus.local`, no router involved. Most deterministic latency.
2. **Mac-hosted Wi-Fi** (Internet Sharing → Wi-Fi, no upstream needed):
   tablet joins the Mac's own network; mDNS works; no house router.
3. **USB tethering** — OS-dependent (iPad-to-Mac USB networking works via
   Internet Sharing over the "iPad USB" interface; Android reverse
   tethering is painful). Evaluate only if 1–2 disappoint.
All three need a hardware session to verify; no code expected beyond what
shipped (server already binds 0.0.0.0 and prints the mDNS name).

### Verified same session: USB-C direct link (no shared Wi-Fi) — WORKS

iPad connected to the MacBook by USB-C cable; **zero configuration on either
side**. macOS brought up an ethernet-over-USB interface (`en6`), both ends
self-assigned link-local IPv4, and mDNS resolved across the cable in both
directions (Mac saw the iPad's `.local` name; iPad loaded
`http://Nimbus.local:7274/` — driver-confirmed). Option 1 from the list
above is therefore the answer; options 2–3 are moot for the iPad. The
`by name:` stderr line is the URL to use on cable (the `lan_ip()` line
prints the Wi-Fi address, which is wrong for this link). Cable RTT: **3.0 ms** (driver-read, vs ~14-15 ms on Wi-Fi in session #1) —
5x tighter and deterministic; the cable is the primary link for serious
playing, Wi-Fi the convenience path.
