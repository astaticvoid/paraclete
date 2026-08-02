use crate::action::GRID_STEPS;
use crate::input::PanelButton;
use crossterm::event::KeyCode;
use paraclete_node_api::{CapabilityDocument, PageRef, ParamDescriptor, StateBusHandle, StateBusValue};
use paraclete_view_assembly::{CompositeOverlay, CompositeView, SUB_PAGE_SLOTS};
use std::collections::HashMap;

/// TK2 C3 (D12): replaces `Mode` (deleted at the wiring flip, per §0 A4).
/// TK2.1 C6 (D12): `Mute` retired — mute state has lived on the track
/// indicator since C0, and `PanelButton::Mute`'s only reachable action was
/// opening this now-dead screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Grid,
    Param(usize),
    Tempo,
    Chain,
    Settings,
}

/// TK2.1 C1 (D5): replaces `grid_rec: bool`. `Off` (default) and `Live`
/// are the pad modes (D6 — trig N addresses track N); `Grid` is
/// step-programming. REC toggles `Off ↔ Grid`; REC held + PLAY escalates
/// to `Live` (or, on the no-kitty fallback, REC while the transport is
/// running arms `Live` directly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecMode {
    Off,
    Grid,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Prev,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
    C,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mag {
    Normal,
    Fine,
    Coarse,
}

#[derive(Clone)]
pub struct SlotBinding {
    pub node_id: u32,
    pub param_id: u32,
    pub param_name: String,
    pub min: f64,
    pub max: f64,
}

/// One placed encoder cell — the param sitting on a specific encoder column.
#[derive(Clone, Debug)]
pub struct EncoderParam {
    pub node_id: u32,
    pub param_id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub stepped: bool,
    /// False only for the composite branch's "no cap-doc entry found" case
    /// (TK2.1 C4). `min`/`max` are placeholder 0..1 when it is.
    pub resolved: bool,
    /// #176 (BUG-067): value-indexed names for a stepped selector —
    /// `options[v]` labels value `v`, an inner `None` a value with no name.
    /// Carried so the encoder can read `lfo_dest tune` rather than
    /// `lfo_dest 1.00`. `None` for anything continuous or unlabelled.
    pub options: Option<Vec<Option<String>>>,
}

/// The label a stepped selector's current value should read as (#176).
///
/// `None` — and so the numeric formatting — for a continuous param, a param
/// with no label set, a non-finite value, or an index outside the table or
/// naming a gap in it. A stepped value is an index, so it is rounded rather
/// than truncated: an encoder that has accumulated to 1.999 is on 2.
pub fn option_label(options: Option<&[Option<String>]>, value: f64) -> Option<&str> {
    let options = options?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    options
        .get(value.round() as usize)
        .and_then(|o| o.as_deref())
}

/// The 8-encoder bank, indexed by column. `None` is an empty column — which a
/// page can now genuinely have, since MM-C1 places by declared slot rather
/// than closing gaps (#150).
pub type EncoderBank = [Option<EncoderParam>; SUB_PAGE_SLOTS as usize];

pub struct TrackInfo {
    pub sequencer_id: u32,
    pub generator_id: u32,
    /// The engine/cap-doc name (e.g. "AnalogKick") — the contextual
    /// window header's second half (TK2.1 C0).
    pub name: String,
    /// The instrument file's `display_name` (e.g. "Kick") — what the
    /// track line, transport and status line show (TK2.1 C0, D2).
    pub display_name: String,
}

pub struct Model {
    /// TK2 C3 (D12): replaces `Mode`.
    pub screen: Screen,
    /// TK2.1 C1 (D5): replaces `grid_rec: bool` — see `RecMode`. Default
    /// `Off` (D5): the reference box boots with pads live, not grid-rec
    /// armed, reversing TK2's "on by default" choice.
    pub rec: RecMode,
    /// TK2.1 C5a (D9): explicit encoder-access mode — while true (and no
    /// TRK/PTN prefix armed, §0 A10), a bare trig resolves to an encoder
    /// jog instead of its pad/step meaning, on any screen.
    pub enc: bool,
    pub active_track: usize,
    pub tracks: Vec<TrackInfo>,
    pub clock_id: u32,
    pub page_windows: Vec<usize>,
    pub caps: HashMap<u32, CapabilityDocument>,
    /// TK1 C3: composite views, one per track — **index-aligned with
    /// `tracks`**, so a track whose assembly fails holds `None` rather than
    /// being absent. It used to be a dense `Vec<CompositeView>` built with
    /// `filter_map`, which silently shifted every later track down one index:
    /// selecting track N then rendered *and edited* track N+1 (BUG-053, #152).
    /// `None` is the honest answer for a track with no chain entry or an
    /// engine carrying no view `Rule`, and it routes to the same engine-local
    /// `Rule` fallback a viewless track always took.
    pub composite: Vec<Option<CompositeView>>,
    pub perf_page: usize,
    pub slot_a: Option<SlotBinding>,
    pub slot_b: Option<SlotBinding>,
    /// TK2 C5 (D13): numpad slot C — extends the TK1 2-slot jog to 3.
    pub slot_c: Option<SlotBinding>,
    /// TK2 C5 (§0 A11): which 8-wide slot window of the active page the
    /// encoder bank shows. Pages with more than 8 params split into
    /// sub-pages; the same Pg key pressed again while already on that
    /// page cycles this (§0 A1 hypothesis — session; `Action::NextSubPage`).
    /// Reset to 0 whenever a different page opens.
    pub sub_page: usize,
    /// TK2 C6 (D12): which pattern (0-7) the Chain screen's bank-row
    /// cursor points at — YES pushes this one onto the chain.
    pub chain_cursor: usize,
    /// TK2.1 C5b (D15): the shared p-lock target — replaces `step_focus`
    /// (deleted; its only reader, `Action::FocusStep`, was unreachable
    /// dead code — nothing has mapped a key to it since the TK2 C3 wiring
    /// flip). Latched via `PanelButton::Lock` — the only p-lock gesture as
    /// of TK2.2 E4; a momentary (kitty trig-hold) path existed briefly and
    /// was retired (BUG-046: the domain hole where the target trig and its
    /// own jog collapse onto one key has no gating fix). While `Some`,
    /// Theotokos's own parameter motion (ENC jog,
    /// numpad slots, `:set`) routes to `CMD_SET_LOCK_TARGET`/
    /// `CMD_SET_STEP_LOCK` on that track's sequencer instead of the live
    /// bank. Published to `/script/theotokos/lock_step`.
    pub lock_target: Option<(usize, usize)>,
    /// TK1 C6: command line editor state (None = closed).
    pub cmdline: Option<String>,
    /// TK1 C6: error message from last command execution (shown in red).
    pub cmdline_error: Option<String>,
    /// TK2 C8: non-error confirmation from the last command (`bindings
    /// saved`, a `:list-bindings` listing, ...) — same lifecycle as
    /// `cmdline_error` (cleared on the next real action) but styled
    /// distinctly so success doesn't read as failure.
    pub cmdline_status: Option<String>,
    /// TK1 C6: fuzzy index built at startup from caps + tracks + static verbs.
    pub fuzzy_index: Vec<FuzzyEntry>,
    /// TK1 C7: yanked pattern data for paste. Kept for TK2 C4, which
    /// rewires copy/paste onto FUNC+REC/STOP (D7) — the underlying
    /// yank/paste logic (`TheotokosApp::yank_active_pattern`/
    /// `paste_pattern`) is unchanged, only its TK1 `y`/`Y` key trigger was
    /// retired at the TK2 C3 wiring flip.
    pub yank_buffer: Vec<YankedStep>,
    /// TK1 C7, extended TK2 C5 (D8): Instant when each slot/encoder value
    /// last changed (for yellow flash) — slots A/B/C, then encoders 1-8.
    pub slot_flash: [Option<std::time::Instant>; 3],
    /// Previous slot values (to detect change).
    pub last_slot_values: [f64; 3],
    pub encoder_flash: [Option<std::time::Instant>; 8],
    pub last_encoder_values: [f64; 8],
    /// Visibility toggle for help overlay (shows mode-specific key bindings).
    pub help_visible: bool,
}

/// TK1 C7: a yanked step for paste.
#[derive(Clone, Default)]
pub struct YankedStep {
    pub active: bool,
    pub note: i64,
    pub velocity: f64,
    pub length: f64,
    pub timing: i64,
    pub condition: f64,
    pub locks: Vec<YankedLock>,
}

/// TK1 C7: a yanked param lock.
#[derive(Clone)]
pub struct YankedLock {
    pub node_id: u32,
    pub param_id: u32,
    pub value: f64,
}

/// TK1 C6: a searchable entry in the fuzzy command index.
#[derive(Clone)]
pub struct FuzzyEntry {
    pub text: String,
    pub category: String,
}

/// TK1 C6: parsed command from the `:` line.
pub enum CmdlineVerb {
    Set {
        node_id: u32,
        param_name: String,
        value: f64,
    },
    Bpm(f64),
    Track(usize),
    Pattern(usize),
    Mute(usize),
    Unmute(usize),
    Clear,
    LockClear,
    /// TK2 C8 (D11): `:bind <key> <button>` — key/button names are
    /// resolved (and the unbindable guard applied) at parse time, so a
    /// syntactically valid `BindKey` is always safe to apply directly.
    BindKey { code: KeyCode, button: PanelButton },
    /// TK2 C8 (D11): `:unbind <key>`.
    UnbindKey { code: KeyCode },
    /// TK2 C8 (D11): `:list-bindings`.
    ListBindings,
    /// TK2 C8 (D14): `:reset-bindings` — clears all user bindings (full
    /// fall-through to §2 defaults).
    ResetBindings,
    /// TK2 C8 (D14): `:save-bindings` — the *only* write path (no
    /// auto-save).
    SaveBindings,
    /// TK2 C8 (D11): `:load-bindings` — re-runs the global→local startup
    /// load order.
    LoadBindings,
}

impl Model {
    pub fn new(
        clock_id: u32,
        seq_ids: &[u32],
        gen_ids: &[u32],
        gen_names: &[String],
        display_names: &[String],
        caps: HashMap<u32, CapabilityDocument>,
        composite: Vec<Option<CompositeView>>,
    ) -> Self {
        let count = seq_ids.len().min(gen_ids.len());
        let tracks: Vec<TrackInfo> = (0..count)
            .map(|i| TrackInfo {
                sequencer_id: seq_ids[i],
                generator_id: gen_ids[i],
                name: gen_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("Trk{}", i + 1)),
                display_name: display_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("Trk{}", i + 1)),
            })
            .collect();
        let track_count = tracks.len();
        let page_windows: Vec<usize> = vec![0; track_count];
        let fuzzy_index = Self::build_fuzzy_index(&caps, &tracks);
        let mut model = Self {
            screen: Screen::Grid,
            rec: RecMode::Off,
            enc: false,
            active_track: 0,
            tracks,
            clock_id,
            page_windows,
            caps,
            composite,
            perf_page: 0,
            slot_a: None,
            slot_b: None,
            slot_c: None,
            sub_page: 0,
            chain_cursor: 0,
            lock_target: None,
            cmdline: None,
            cmdline_error: None,
            cmdline_status: None,
            fuzzy_index,
            yank_buffer: Vec::new(),
            slot_flash: [None; 3],
            last_slot_values: [0.0; 3],
            encoder_flash: [None; 8],
            last_encoder_values: [0.0; 8],
            help_visible: false,
        };
        model.bind_page();
        model
    }

    /// The active track's composite view, if it assembled. Every read goes
    /// through here so the `None`-per-track alignment of `composite` (#152)
    /// cannot be re-flattened by a future call site.
    pub fn active_composite(&self) -> Option<&CompositeView> {
        self.composite.get(self.active_track).and_then(Option::as_ref)
    }

    /// The engine label for `track` — what the contextual header draws after
    /// the display name (`{display_name} — {engine_label}`).
    ///
    /// For a machine host this is the machine the track is **on**, read from
    /// the variant set `sync_machine_selection` keeps current. `TrackInfo.name`
    /// cannot answer it: it is captured once in `Model::new` from the startup
    /// cap-doc, so it names the machine the node was *constructed* with and
    /// kept reading `AnalogKick` on a track switched to `AnalogHiHat`, with
    /// HiHat's params drawn underneath it (#161).
    ///
    /// Only the *engine's* own host counts. `variants` is every machine host
    /// in the chain, engine first — but a chain whose engine is plain and
    /// whose effect hosts machines would otherwise label the track with the
    /// effect's machine, which is not what the header means.
    pub fn engine_label(&self, track: usize) -> String {
        let fallback = || {
            self.tracks
                .get(track)
                .map(|t| t.name.clone())
                .unwrap_or_default()
        };
        let Some(cv) = self.composite.get(track).and_then(Option::as_ref) else {
            return fallback();
        };
        cv.variants
            .iter()
            .find(|set| set.node_id == cv.engine_node_id)
            .and_then(|set| set.variants.iter().find(|v| v.value == set.active))
            .map(|v| v.name.clone())
            .unwrap_or_else(fallback)
    }

    pub fn select_track(&mut self, i: usize) {
        if i < self.tracks.len() {
            self.active_track = i;
            self.bind_page();
        }
    }

    /// TK2.1 C5b (D15): `lock_target`'s step, but only if it's on the
    /// active track — the shape every value-routing call site needs
    /// (`lock_target` itself doesn't move when the selection does).
    pub fn lock_step_for_active_track(&self) -> Option<usize> {
        self.lock_target
            .and_then(|(t, s)| if t == self.active_track { Some(s) } else { None })
    }

    /// How many pages the active track has: its composite view's, or — for a
    /// track with no composite view (#152) — the engine-local `Rule`'s page
    /// groups, the same fallback `resolve_page_params_n` and
    /// `page_sub_page_count` take. Shared so the three cannot disagree about
    /// how many pages a viewless track has.
    pub fn page_count_for_active_track(&self) -> usize {
        self.active_composite()
            .map(|cv| cv.pages.len())
            .unwrap_or_else(|| {
                let gen_id = self.tracks[self.active_track].generator_id;
                self.caps
                    .get(&gen_id)
                    .and_then(|c| c.view.as_ref())
                    .map(|r| r.page_groups.len())
                    .unwrap_or(0)
            })
    }

    pub fn select_perf_page(&mut self, idx: usize) {
        if idx >= self.page_count_for_active_track() {
            return;
        }
        self.perf_page = idx;
        self.bind_page();
    }

    fn bind_page(&mut self) {
        let params = self.resolve_page_params_n(3);
        let to_binding = |p: Option<&(u32, u32, String, f64, f64)>| {
            p.map(|(nid, pid, name, min, max)| SlotBinding {
                node_id: *nid,
                param_id: *pid,
                param_name: name.clone(),
                min: *min,
                max: *max,
            })
        };
        self.slot_a = to_binding(params.first());
        self.slot_b = to_binding(params.get(1));
        self.slot_c = to_binding(params.get(2));
    }

    /// TK2 C5 (D8): the active page's params in `Rule` order, up to `n`.
    /// Generalizes the TK1 2-slot resolver (`bind_page`) so the 8-encoder
    /// bank can resolve against the same page — composite pages first,
    /// falling back to the engine-local `Rule` (existing TK0 path).
    ///
    /// **Positional, and knowingly inconsistent with `resolve_encoder_params`**
    /// since MM-C1: this reads `page.params` by position, so it binds the
    /// first `n` params of the page rather than the params at slots `0..n`.
    /// It feeds `slot_a`/`slot_b`/`slot_c`, which **are** on screen — the
    /// `A:`/`B:`/`C:` readout on every Param screen (`render.rs`). Only the
    /// A/B/C *jog* is descoped (BUG-038 / OQ-T24; `Action::Jog` is emitted
    /// nowhere). So on a page with sparse slots the readout and the encoder
    /// bank would disagree, each showing a different convention. Harmless
    /// while every shipped node declares densely from 0; reviving the
    /// cluster means routing it through the same placement, not re-deriving
    /// one here.
    pub fn resolve_page_params_n(&self, n: usize) -> Vec<(u32, u32, String, f64, f64)> {
        if let Some(cv) = self.active_composite() {
            if let Some(page) = cv.pages.get(self.perf_page) {
                if !page.params.is_empty() {
                    return page
                        .params
                        .iter()
                        .take(n)
                        .map(|p| (p.node_id, p.param_id, p.name.clone(), 0.0, 1.0))
                        .collect();
                }
            }
        }
        // Fallback: engine-local Rule (existing TK0 path).
        let gen_id = self.tracks[self.active_track].generator_id;
        let cap = match self.caps.get(&gen_id) {
            Some(c) => c,
            None => return Vec::new(),
        };
        let rule = match &cap.view {
            Some(r) => r,
            None => return Vec::new(),
        };
        let page = match rule.page_groups.get(self.perf_page) {
            Some(p) => p.as_ref(),
            None => {
                return cap
                    .params
                    .iter()
                    .take(n)
                    .map(|p| (gen_id, p.id, p.name.to_string(), p.min, p.max))
                    .collect();
            }
        };

        let mut params: Vec<&(u32, PageRef)> = rule
            .param_pages
            .iter()
            .filter(|(_, pr)| pr.page.as_ref() == page)
            .collect();
        params.sort_by_key(|(_, pr)| pr.slot);
        params
            .iter()
            .take(n)
            .filter_map(|(pid, _)| {
                cap.params
                    .iter()
                    .find(|pd| pd.id == *pid)
                    .map(|pd| (gen_id, pd.id, pd.name.to_string(), pd.min, pd.max))
            })
            .collect()
    }

    /// TK2 C5 (D8/§0 A11): the active page's params in `Rule` order,
    /// restricted to the current sub-page's 8-wide slot window (slots
    /// 0-7 = sub-page 1, 8-15 = sub-page 2, ... — `PageRef`/`CompositeParam`
    /// both carry this `slot` field already). Unlike `resolve_page_params_n`
    /// (used for the 3 numpad slots, always the page's absolute first
    /// params), this is what the 8-encoder bank resolves against, so a
    /// page with more than 8 params splits into sub-pages instead of
    /// silently truncating.
    /// TK2.1 C4 (D10, closes BUG-040): looks up `(node_id, param_id)`
    /// against `Model::caps` — verified to contain every node in the
    /// instrument file, not just generators — for the descriptor's real
    /// `min`/`max`/`stepped`. `None` when the node or param isn't found
    /// (caller falls back to 0..1 and renders the cell dimmed).
    fn resolve_param_descriptor(&self, node_id: u32, param_id: u32) -> Option<&ParamDescriptor> {
        self.caps.get(&node_id)?.params.iter().find(|pd| pd.id == param_id)
    }

    // ── MM-C6: machine variants ──────────────────────────────────────────

    /// The overlay in force for `(node_id, param_id)` on the active track —
    /// the *selected machine's* range, not the bank's (ADR-041 §0 A1).
    ///
    /// The engine's `ParameterBank` stores the widest envelope across every
    /// machine and is never narrowed; narrowing it would truncate values
    /// belonging to machines that are not selected, on load. So the union is
    /// right for storage and wrong for a knob: on `FmBell`, `decay` runs to
    /// 8 s, on `FmBass` to 4, and the bank says 8 for both. A surface that
    /// clamps to the bank lets a performer dial a `FmBass` decay the machine
    /// does not use.
    ///
    /// `None` for every param of every node that is not a machine host, which
    /// is all of them but the two engines — the caller then uses the
    /// descriptor, exactly as before MM-C6.
    pub fn active_overlay(&self, node_id: u32, param_id: u32) -> Option<&CompositeOverlay> {
        let cv = self.active_composite()?;
        let set = cv.variants.iter().find(|s| s.node_id == node_id)?;
        let variant = set.variants.iter().find(|v| v.value == set.active)?;
        variant.overlays.iter().find(|o| o.param_id == param_id)
    }

    /// Is this param the node's *identity* rather than a setting (ADR-041
    /// §0 A4)? Identity params are refused as p-lock targets and as
    /// scene-morph destinations.
    ///
    /// Checked against **every** variant, not just the selected one. The flag
    /// lives per overlay and has to be repeated in each machine, so a variant
    /// that forgot it would otherwise make rejection work on one machine and
    /// not another — "p-locking machine works on HiHat but not Kick", which
    /// no test catches by accident. Reading the union of the flags means a
    /// missed repeat costs nothing here; MM-C8's assertion is what will say
    /// the declaration itself is inconsistent.
    pub fn is_identity_param(&self, node_id: u32, param_id: u32) -> bool {
        self.active_composite()
            .and_then(|cv| cv.variants.iter().find(|s| s.node_id == node_id))
            .is_some_and(|set| {
                set.variants.iter().any(|v| {
                    v.overlays
                        .iter()
                        .any(|o| o.param_id == param_id && o.identity)
                })
            })
    }

    /// Re-point every machine host at the machine its `machine` param now
    /// names, swapping in that variant's pre-merged pages. Returns true if
    /// anything moved, so the caller can repaint.
    ///
    /// **This is the whole of ADR-041 decision 1's "swap the displayed
    /// variant locally".** No capability is re-queried and no query channel
    /// exists to re-query it with: MM-C5 pre-merged every machine's pages into
    /// `CompositeVariantSet`, so a switch is a swap of an already-built page
    /// list. Cap-docs are still collected once, at startup.
    ///
    /// Called every frame rather than on a state-bus subscription because the
    /// switch can come from anywhere — a Theoria client, a profile script, a
    /// `:set` on the command line — and Theotokos owns none of those paths.
    /// The read is a handful of `HashMap` lookups over the chain's hosts, and
    /// the clone only happens on an actual change.
    pub fn sync_machine_selection(&mut self, bus: &StateBusHandle) -> bool {
        let mut changed = false;
        for track in 0..self.composite.len() {
            // A track that failed to assemble holds `None` and keeps its index
            // (#152); it simply has no hosts to sync.
            let Some(view) = self.composite[track].as_ref() else {
                continue;
            };
            let hosts: Vec<(u32, u32, u32)> = view
                .variants
                .iter()
                .filter_map(|s| Some((s.node_id, s.select_param?, s.active)))
                .collect();
            for (node_id, select_param, active) in hosts {
                let Some(raw) = self.read_param_opt(bus, node_id, select_param) else {
                    continue;
                };
                // The bank slot is an f64 a malformed project could carry.
                // Mirror the engines' `from_value`: clamp rather than panic,
                // and never index with a negative or non-finite value.
                if !raw.is_finite() || raw < 0.0 {
                    continue;
                }
                let want = raw as u32;
                if want == active {
                    continue;
                }
                let view = self.composite[track]
                    .as_mut()
                    .expect("`hosts` is only non-empty for an assembled track");
                let Some(idx) = view
                    .variants
                    .iter()
                    .find(|s| s.node_id == node_id)
                    .and_then(|s| s.variants.iter().position(|v| v.value == want))
                else {
                    continue;
                };
                let pages = {
                    let set = view
                        .variants
                        .iter_mut()
                        .find(|s| s.node_id == node_id)
                        .expect("found immediately above");
                    set.active = want;
                    set.variants[idx].pages.clone()
                };
                view.pages = pages;
                changed = true;
            }
        }
        if changed {
            // A machine with fewer pages can leave the selection past the end,
            // and one with a shorter page can leave the sub-page past the end.
            // Both would render an empty bank that no key could escape.
            //
            // Through the same fallback `select_perf_page` uses: another
            // track switching machine sets `changed`, and a viewless active
            // track (#152) still has its `Rule`'s pages. Clamping it to 0
            // here would snap the performer to page 0 of a track that never
            // moved.
            let pages = self.page_count_for_active_track();
            if self.perf_page >= pages {
                self.perf_page = pages.saturating_sub(1);
            }
            let subs = self.page_sub_page_count();
            if self.sub_page >= subs {
                self.sub_page = subs.saturating_sub(1);
            }
            self.bind_page();
        }
        changed
    }

    /// `read_param_value`'s answer, but distinguishing "absent" from 0.0 —
    /// `sync_machine_selection` must not read a missing path as machine 0 and
    /// yank the performer back to the first machine every frame.
    fn read_param_opt(&self, bus: &StateBusHandle, node_id: u32, param_id: u32) -> Option<f64> {
        let name = self
            .caps
            .get(&node_id)?
            .params
            .iter()
            .find(|p| p.id == param_id)
            .map(|p| p.name.to_string())?;
        match bus.read(&format!("/node/{}/param/{}", node_id, name))? {
            StateBusValue::Float(f) => Some(*f),
            StateBusValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// The encoder bank for the active page and sub-page, **already placed**:
    /// index `n` is the param on encoder `n`, `None` is an empty column.
    ///
    /// MM-C1 (#150, BUG-052): this used to return a `Vec` in slot order and
    /// let each caller index it positionally, which made the encoder column
    /// the param's *rank* in the window rather than its declared slot — a
    /// node declaring slots 0, 2, 5 rendered on encoders 1, 2, 3, closing the
    /// gaps it asked for. Placement now happens once, here, so the render and
    /// jog paths cannot disagree about it. ADR-041 §0 A2 puts machine-select
    /// on a TRIG slot by convention, which needs the declared slot to survive
    /// this far.
    ///
    /// `resolved` is false only for the composite branch's "no cap-doc entry
    /// found" case (TK2.1 C4) — `min`/`max` are 0..1 and not to be trusted
    /// when it is.
    pub fn resolve_encoder_params(&self) -> EncoderBank {
        let width = SUB_PAGE_SLOTS as usize;
        let lo = (self.sub_page * width) as u16;
        let hi = lo + SUB_PAGE_SLOTS as u16;
        let mut bank: EncoderBank = std::array::from_fn(|_| None);
        // `lo` is always a multiple of SUB_PAGE_SLOTS, so within the window
        // `slot - lo` is the column and is unique per param.
        let column = |slot: u8| (slot as u16).checked_sub(lo).map(|c| c as usize);

        if let Some(cv) = self.active_composite() {
            if let Some(page) = cv.pages.get(self.perf_page) {
                if !page.params.is_empty() {
                    for p in page
                        .params
                        .iter()
                        .filter(|p| (p.slot as u16) >= lo && (p.slot as u16) < hi)
                    {
                        let Some(col) = column(p.slot).filter(|c| *c < width) else {
                            continue;
                        };
                        // TK2.1 C4: an unresolvable ref keeps its column and
                        // renders dimmed at a placeholder 0..1 range, rather
                        // than being dropped.
                        let (mut min, mut max, stepped, resolved) =
                            match self.resolve_param_descriptor(p.node_id, p.param_id) {
                                Some(pd) => (pd.min, pd.max, pd.stepped, true),
                                None => (0.0, 1.0, false, false),
                            };
                        // MM-C6: on a machine host, display and clamp against
                        // the SELECTED machine's overlay. The descriptor above
                        // is the bank's union across every machine and is
                        // deliberately wider — see `active_overlay`. Only the
                        // range moves; the stored value is never re-clamped,
                        // which is what lets a value belonging to another
                        // machine survive a round trip.
                        if resolved {
                            if let Some(o) = self.active_overlay(p.node_id, p.param_id) {
                                min = o.min;
                                max = o.max;
                            }
                        }
                        bank[col] = Some(EncoderParam {
                            node_id: p.node_id,
                            param_id: p.param_id,
                            name: p.name.clone(),
                            min,
                            max,
                            stepped,
                            resolved,
                            // #176: assembly already resolved these — the
                            // machine names for an identity param, and any
                            // stepped param's `ParamDisplay` labels for the
                            // rest (`main.rs::stepped_labels`). Nothing here
                            // needs to know what an LFO or a machine is.
                            options: p.options.clone(),
                        });
                    }
                    return bank;
                }
            }
        }
        let gen_id = self.tracks[self.active_track].generator_id;
        let cap = match self.caps.get(&gen_id) {
            Some(c) => c,
            None => return bank,
        };
        let rule = match &cap.view {
            Some(r) => r,
            None => return bank,
        };
        let page = match rule.page_groups.get(self.perf_page) {
            Some(p) => p.as_ref(),
            None => {
                // No Rule pagination — no declared slots to place by, so this
                // branch alone is genuinely positional, and only sub-page 0
                // has content (matches `resolve_page_params_n`'s same
                // fallback).
                if self.sub_page > 0 {
                    return bank;
                }
                for (col, p) in cap.params.iter().take(width).enumerate() {
                    bank[col] = Some(EncoderParam {
                        node_id: gen_id,
                        param_id: p.id,
                        name: p.name.to_string(),
                        min: p.min,
                        max: p.max,
                        stepped: p.stepped,
                        resolved: true,
                        options: p.value_labels(),
                    });
                }
                return bank;
            }
        };
        for (pid, pr) in rule.param_pages.iter().filter(|(_, pr)| {
            pr.page.as_ref() == page && (pr.slot as u16) >= lo && (pr.slot as u16) < hi
        }) {
            let Some(col) = column(pr.slot).filter(|c| *c < width) else {
                continue;
            };
            // MM-C1 behaviour change, latent today: a `param_pages` entry
            // whose id is absent from the cap-doc (BUG-037's shape) used to be
            // dropped, closing the gap and shifting every later param one
            // column left. It now leaves the column empty. A hole is honest;
            // a silent left-shift is the thing BUG-052 was about. Unreachable
            // in the shipped app — the composite branch above always wins —
            // until #152 drops a track's composite view.
            if let Some(pd) = cap.params.iter().find(|pd| pd.id == *pid) {
                bank[col] = Some(EncoderParam {
                    node_id: gen_id,
                    param_id: pd.id,
                    name: pd.name.to_string(),
                    min: pd.min,
                    max: pd.max,
                    stepped: pd.stepped,
                    resolved: true,
                    options: pd.value_labels(),
                });
            }
        }
        bank
    }

    /// TK2 C5 (§0 A11): how many sub-pages the active page has (min 1) —
    /// drives the same-Pg-key-toggles-sub-page gesture and the render
    /// indicator.
    pub fn page_sub_page_count(&self) -> usize {
        let composite_max = self.active_composite().and_then(|cv| {
            cv.pages
                .get(self.perf_page)
                .and_then(|page| page.params.iter().map(|p| p.slot).max())
        });
        let max_slot = composite_max.or_else(|| {
            let gen_id = self.tracks[self.active_track].generator_id;
            let cap = self.caps.get(&gen_id)?;
            let rule = cap.view.as_ref()?;
            let page = rule.page_groups.get(self.perf_page)?.as_ref();
            rule.param_pages
                .iter()
                .filter(|(_, pr)| pr.page.as_ref() == page)
                .map(|(_, pr)| pr.slot)
                .max()
        });
        match max_slot {
            Some(s) => (s as usize / SUB_PAGE_SLOTS as usize) + 1,
            None => 1,
        }
    }

    pub fn playing(&self, bus: &StateBusHandle) -> bool {
        bus.read("/transport/playing")
            .map(|v| matches!(v, StateBusValue::Bool(true)))
            .unwrap_or(false)
    }

    pub fn read_bpm(&self, bus: &StateBusHandle) -> f64 {
        bus.read("/transport/bpm")
            .and_then(|v| match v {
                StateBusValue::Float(f) => Some(*f),
                _ => None,
            })
            .unwrap_or(120.0)
    }

    pub fn read_param_value(&self, bus: &StateBusHandle, node_id: u32, param_id: u32) -> f64 {
        let param_name = self
            .caps
            .get(&node_id)
            .and_then(|c| c.params.iter().find(|p| p.id == param_id))
            .map(|p| p.name.to_string());

        match param_name {
            Some(name) => bus
                .read(&format!("/node/{}/param/{}", node_id, name))
                .and_then(|v| match v {
                    StateBusValue::Float(f) => Some(*f),
                    StateBusValue::Int(i) => Some(*i as f64),
                    _ => None,
                })
                .unwrap_or(0.0),
            None => 0.0,
        }
    }

    pub fn read_step_state(&self, bus: &StateBusHandle, track_idx: usize) -> StepState {
        let seq_id = self.tracks[track_idx].sequencer_id;

        let current_step = bus
            .read(&format!("/node/{}/state/current_step", seq_id))
            .and_then(|v| match v {
                StateBusValue::Int(i) => Some(*i as usize),
                _ => None,
            })
            .unwrap_or(0);

        let pattern_length = bus
            .read(&format!("/node/{}/state/pattern_length", seq_id))
            .and_then(|v| match v {
                StateBusValue::Int(i) => Some(*i as usize),
                _ => None,
            })
            .unwrap_or(16);

        let steps_text = bus
            .read(&format!("/node/{}/state/steps", seq_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let steps: Vec<bool> = steps_text.chars().map(|c| c == '1').collect();

        let page_count = pattern_length.div_ceil(GRID_STEPS);

        StepState {
            current_step,
            pattern_length,
            steps,
            page_count,
        }
    }

    pub fn page_groups_for_active_track(&self) -> Vec<String> {
        // TK1 C3: composite page labels first.
        if let Some(cv) = self.active_composite() {
            if !cv.pages.is_empty() {
                return cv.pages.iter().map(|p| p.label.clone()).collect();
            }
        }
        let gen_id = self.tracks[self.active_track].generator_id;
        self.caps
            .get(&gen_id)
            .and_then(|c| c.view.as_ref())
            .map(|r| r.page_groups.iter().map(|g| g.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn read_lock_value(
        &self,
        bus: &StateBusHandle,
        sequencer_id: u32,
        step: usize,
        node_id: u32,
        param_id: u32,
    ) -> Option<f64> {
        let locks_text = bus
            .read(&format!("/node/{}/state/locks", sequencer_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        Self::parse_lock_value(&locks_text, step, node_id, param_id)
    }

    pub fn parse_lock_value(
        locks_text: &str,
        step: usize,
        node_id: u32,
        param_id: u32,
    ) -> Option<f64> {
        for entry in locks_text.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let parts: Vec<&str> = entry.splitn(4, [':', '=']).collect();
            if parts.len() != 4 {
                continue;
            }
            // #181 (BUG-071): `continue`, not `?`. An unreadable entry costs
            // that entry, not the rest of the scan — `published_state` emits a
            // whole pattern's locks as one `;`-joined string, so aborting here
            // would hide every lock after the bad one. The caller cannot tell
            // that from "no lock on this step", and `lib.rs`'s jog path then
            // falls back to the live param value and overwrites the lock it
            // could not see.
            let Some(entry_step) = parts[0]
                .strip_prefix('s')
                .and_then(|s| s.parse::<usize>().ok())
            else {
                continue;
            };
            let Ok(entry_nid) = parts[1].parse::<u32>() else {
                continue;
            };
            let Ok(entry_pid) = parts[2].parse::<u32>() else {
                continue;
            };
            if entry_step == step && entry_nid == node_id && entry_pid == param_id {
                return parts[3].parse::<f64>().ok();
            }
        }
        None
    }

    pub fn read_step_locks(&self, bus: &StateBusHandle, track_idx: usize) -> Vec<usize> {
        let seq_id = self.tracks[track_idx].sequencer_id;
        let locks_text = bus
            .read(&format!("/node/{}/state/locks", seq_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let mut steps: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for entry in locks_text.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if let Some(rest) = entry.strip_prefix('s') {
                if let Some(colon) = rest.find(':') {
                    if let Ok(s) = rest[..colon].parse::<usize>() {
                        steps.insert(s);
                    }
                }
            }
        }
        let mut sorted: Vec<usize> = steps.into_iter().collect();
        sorted.sort_unstable();
        sorted
    }

    pub fn envelope_for_active_track(&self) -> Option<EnvelopeData> {
        let gen_id = self.tracks[self.active_track].generator_id;
        let cap = self.caps.get(&gen_id)?;
        let rule = cap.view.as_ref()?;
        let env = rule.envelopes.first()?;
        let pid = env.param_ids[0];
        if pid == 0 {
            return None;
        }
        let param = cap.params.iter().find(|p| p.id == pid)?;
        Some(EnvelopeData {
            param_id: pid,
            param_name: param.name.to_string(),
            node_id: gen_id,
            env_type: env.env_type.to_string(),
            min: param.min,
            max: param.max,
        })
    }

    // ── C6: command line ──

    pub fn build_fuzzy_index(
        caps: &HashMap<u32, CapabilityDocument>,
        tracks: &[TrackInfo],
    ) -> Vec<FuzzyEntry> {
        let mut entries = Vec::new();
        // static verbs
        for verb in &[
            "set",
            "bpm",
            "track",
            "pattern",
            "mute",
            "unmute",
            "clear",
            "lock-clear",
            "bind",
            "unbind",
            "list-bindings",
            "reset-bindings",
            "save-bindings",
            "load-bindings",
        ] {
            entries.push(FuzzyEntry {
                text: verb.to_string(),
                category: "verb".into(),
            });
        }
        // param names from all cap-docs
        for cap in caps.values() {
            for p in &cap.params {
                entries.push(FuzzyEntry {
                    text: p.name.to_string(),
                    category: "param".into(),
                });
            }
        }
        // track names
        for t in tracks {
            entries.push(FuzzyEntry {
                text: t.name.to_string(),
                category: "track".into(),
            });
        }
        entries
    }

    /// Returns top candidates matching `query` using subsequence fuzzy match.
    pub fn cmdline_candidates(&self) -> Vec<String> {
        let query = match &self.cmdline {
            Some(s) if !s.is_empty() => s,
            _ => return vec![],
        };
        let lower = query.to_lowercase();
        let mut scored: Vec<(&FuzzyEntry, usize)> = self
            .fuzzy_index
            .iter()
            .filter_map(|e| {
                let text = e.text.to_lowercase();
                let score = Self::fuzzy_score(&lower, &text)?;
                Some((e, score))
            })
            .collect();
        scored.sort_by_key(|(e, s)| (*s, e.text.len()));
        scored.dedup_by_key(|(e, _)| &e.text);
        scored
            .into_iter()
            .take(5)
            .map(|(e, _)| e.text.clone())
            .collect()
    }

    fn fuzzy_score(query: &str, target: &str) -> Option<usize> {
        let mut qi = query.chars();
        let mut qc = qi.next()?;
        let mut score = 0usize;
        for (i, tc) in target.char_indices() {
            if tc.to_ascii_lowercase() == qc {
                // first char match = prefix bonus
                if i == 0 && score == 0 {
                    score = 0;
                }
                qc = match qi.next() {
                    Some(c) => c,
                    None => return Some(score),
                };
            }
            score = score.saturating_add(1);
        }
        if qi.next().is_some() {
            None // not all query chars consumed
        } else {
            Some(score)
        }
    }

    pub fn parse_cmdline(&self, input: &str) -> Result<CmdlineVerb, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("empty command".into());
        }
        let (verb, rest) = match input.split_once(char::is_whitespace) {
            Some((v, r)) => (v, r.trim()),
            None => (input, ""),
        };
        match verb {
            "set" => {
                let (name, val) = rest
                    .rsplit_once(char::is_whitespace)
                    .ok_or_else(|| "set <param> <value>".to_string())?;
                let value: f64 = val.parse().map_err(|_| format!("invalid value: {val}"))?;
                let lname = name.to_lowercase();
                let best = self
                    .fuzzy_index
                    .iter()
                    .filter(|e| e.category == "param")
                    .filter_map(|e| {
                        let score = Self::fuzzy_score(&lname, &e.text.to_lowercase())?;
                        Some((e, score))
                    })
                    .min_by_key(|(_, s)| *s)
                    .ok_or_else(|| format!("unknown param: {name}"))?;
                // Find the node that has this param on the active track, and
                // keep its declared range — #177 (BUG-068): this used to
                // record only *whether* the param exists and then clamp to a
                // literal 0..1, so every param whose range is not a subset of
                // that was unreachable. `tone` (200..8000) landed on 200
                // whatever you typed, because `ParameterBank::set` then
                // clamped the 1.0 back *up* to the minimum; `lfo_fade` (-1..1)
                // lost its whole fade-out half to the lower bound.
                let param_lname = best.0.text.to_lowercase();
                let range_on = |nid: u32, pid: Option<u32>| -> Option<(f64, f64)> {
                    self.caps.get(&nid).and_then(|c| {
                        c.params
                            .iter()
                            .find(|p| {
                                p.name.to_string().to_lowercase() == param_lname
                                    && pid.is_none_or(|want| p.id == want)
                            })
                            .map(|p| (p.min, p.max))
                    })
                };

                let track = &self.tracks[self.active_track];
                let mut node_id = track.generator_id;
                let mut range = range_on(node_id, None);
                // Also check composite chain nodes. Matched by **name**: this
                // used to take the first page entry whose id merely existed in
                // its own node's cap-doc, i.e. the first composite param on the
                // track regardless of what the performer typed. Harmless while
                // the range was a literal; not once the range comes from
                // whichever descriptor we land on.
                if range.is_none() {
                    if let Some(cv) = self.active_composite() {
                        'pages: for page in &cv.pages {
                            for cp in &page.params {
                                if let Some(r) = range_on(cp.node_id, Some(cp.param_id)) {
                                    node_id = cp.node_id;
                                    range = Some(r);
                                    break 'pages;
                                }
                            }
                        }
                    }
                }
                let Some((min, max)) = range else {
                    return Err(format!("param {} not found on active track", best.0.text));
                };
                // Still a silent clamp, not an acknowledgement: telling the
                // performer their value was adjusted is #147 (OQ-T31), which
                // governs how every command reports itself and is parked.
                Ok(CmdlineVerb::Set {
                    node_id,
                    param_name: best.0.text.clone(),
                    value: value.clamp(min, max),
                })
            }
            "bpm" => {
                let val: f64 = rest.parse().map_err(|_| "bpm <number>".to_string())?;
                Ok(CmdlineVerb::Bpm(val.clamp(20.0, 300.0)))
            }
            "track" => {
                let n: usize = rest
                    .parse::<usize>()
                    .map_err(|_| "track <number>".to_string())?;
                if n < 1 || n > self.tracks.len() {
                    return Err(format!(
                        "track {} out of range (1-{})",
                        n,
                        self.tracks.len()
                    ));
                }
                Ok(CmdlineVerb::Track(n - 1))
            }
            "pattern" => {
                let n: usize = rest.parse().map_err(|_| "pattern <number>".to_string())?;
                if n < 1 {
                    return Err("pattern <number>".into());
                }
                Ok(CmdlineVerb::Pattern(n - 1))
            }
            "mute" => {
                let n: usize = rest
                    .parse()
                    .map_err(|_| "mute <track number>".to_string())?;
                if n < 1 || n > self.tracks.len() {
                    return Err(format!("track {} out of range", n));
                }
                Ok(CmdlineVerb::Mute(n - 1))
            }
            "unmute" => {
                let n: usize = rest
                    .parse()
                    .map_err(|_| "unmute <track number>".to_string())?;
                if n < 1 || n > self.tracks.len() {
                    return Err(format!("track {} out of range", n));
                }
                Ok(CmdlineVerb::Unmute(n - 1))
            }
            "clear" => Ok(CmdlineVerb::Clear),
            "lock-clear" => Ok(CmdlineVerb::LockClear),
            // TK2 C8 (D11/D14): key remapping verbs.
            "bind" => {
                let (key_str, button_str) = rest
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| "bind <key> <button>".to_string())?;
                let button_str = button_str.trim();
                let code = crate::input::key_from_name(key_str)
                    .ok_or_else(|| format!("unknown key: {key_str}"))?;
                if crate::input::is_unbindable(code) {
                    return Err(format!("{key_str} is reserved and cannot be rebound"));
                }
                let button = crate::input::button_from_name(button_str)
                    .ok_or_else(|| format!("unknown button: {button_str}"))?;
                Ok(CmdlineVerb::BindKey { code, button })
            }
            "unbind" => {
                let key_str = rest.trim();
                if key_str.is_empty() {
                    return Err("unbind <key>".into());
                }
                let code = crate::input::key_from_name(key_str)
                    .ok_or_else(|| format!("unknown key: {key_str}"))?;
                Ok(CmdlineVerb::UnbindKey { code })
            }
            "list-bindings" => Ok(CmdlineVerb::ListBindings),
            "reset-bindings" => Ok(CmdlineVerb::ResetBindings),
            "save-bindings" => Ok(CmdlineVerb::SaveBindings),
            "load-bindings" => Ok(CmdlineVerb::LoadBindings),
            _ => Err(format!("?{input}")),
        }
    }

    // ── C7: flash ──

    pub fn update_flash(&mut self, slot: usize, new_value: f64) {
        if (new_value - self.last_slot_values[slot]).abs() > 0.0001 {
            self.slot_flash[slot] = Some(std::time::Instant::now());
            self.last_slot_values[slot] = new_value;
        }
    }

    /// TK2 C5 (D8): flash generalized from 2 slots to 8 encoders.
    pub fn update_encoder_flash(&mut self, col: usize, new_value: f64) {
        if (new_value - self.last_encoder_values[col]).abs() > 0.0001 {
            self.encoder_flash[col] = Some(std::time::Instant::now());
            self.last_encoder_values[col] = new_value;
        }
    }
}

#[derive(Clone, Default)]
pub struct StepState {
    pub current_step: usize,
    pub pattern_length: usize,
    pub steps: Vec<bool>,
    pub page_count: usize,
}

#[derive(Clone)]
pub struct EnvelopeData {
    pub param_id: u32,
    pub param_name: String,
    pub node_id: u32,
    pub env_type: String,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone)]
pub struct Tuning {
    pub base_divisor: f64,
    pub min_step: f64,
    pub fine_divisor: f64,
    pub coarse_multiplier: f64,
    pub ramp_hz: f64,
    pub ramp_dwell_ms: u64,
    pub ramp_accel_factor: f64,
    pub ramp_accel_cap: f64,
    /// TK1 C7: duration slot values display in Yellow after a change (ms).
    pub flash_ms: u64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            base_divisor: 128.0,
            min_step: 0.001,
            fine_divisor: 8.0,
            coarse_multiplier: 4.0,
            ramp_hz: 60.0,
            ramp_dwell_ms: 150,
            ramp_accel_factor: 1.05,
            ramp_accel_cap: 8.0,
            flash_ms: 400,
        }
    }
}

impl Tuning {
    pub fn jog_step(&self, range: f64, held_ms: u64, mag: Mag) -> f64 {
        let base = (range / self.base_divisor).max(self.min_step);
        let step = match mag {
            Mag::Normal => base,
            Mag::Fine => base / self.fine_divisor,
            Mag::Coarse => base * self.coarse_multiplier,
        };
        if held_ms > self.ramp_dwell_ms {
            let n = (held_ms - self.ramp_dwell_ms) as f64 / (1000.0 / self.ramp_hz);
            let mult = self.ramp_accel_factor.powf(n).min(self.ramp_accel_cap);
            step * mult
        } else {
            step
        }
    }

    /// TK2.1 C4 (D10, closes BUG-040 §2): a stepped param (an integer
    /// selector — algorithm, machine, waveform) moves exactly one unit
    /// per press, ignoring range, magnitude and ramp entirely. A separate
    /// method (not a branch inside `jog_step`) so §4.2's constants and the
    /// existing `Tuning` tests stay untouched.
    pub fn jog_step_stepped(&self) -> f64 {
        1.0
    }
}

pub struct JogTracker {
    pub held_since: Option<std::time::Instant>,
    pub last_tick_ms: u64,
}

impl JogTracker {
    pub fn new() -> Self {
        Self {
            held_since: None,
            last_tick_ms: 0,
        }
    }

    pub fn press(&mut self, now: std::time::Instant, tick_ms: u64) -> u64 {
        self.held_since = Some(now);
        self.last_tick_ms = tick_ms;
        0
    }

    pub fn repeat(&mut self, now: std::time::Instant, tick_ms: u64) -> Option<u64> {
        let held_since = self.held_since?;
        if tick_ms <= self.last_tick_ms + 200 {
            self.last_tick_ms = tick_ms;
            let held = now.duration_since(held_since).as_millis() as u64;
            Some(held)
        } else {
            self.held_since = None;
            self.last_tick_ms = 0;
            None
        }
    }

    pub fn release(&mut self) {
        self.held_since = None;
        self.last_tick_ms = 0;
    }
}

impl Default for JogTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn jog_step_normal() {
        let t = Tuning::default();
        let step = t.jog_step(1.0, 0, Mag::Normal);
        assert!((step - 1.0 / 128.0).abs() < 0.0001);
    }

    #[test]
    fn jog_step_fine() {
        let t = Tuning::default();
        let step = t.jog_step(1.0, 0, Mag::Fine);
        assert!((step - 1.0 / 128.0 / 8.0).abs() < 0.00001);
    }

    #[test]
    fn jog_step_coarse() {
        let t = Tuning::default();
        let step = t.jog_step(1.0, 0, Mag::Coarse);
        assert!((step - 1.0 / 128.0 * 4.0).abs() < 0.0001);
    }

    #[test]
    fn jog_step_minimum() {
        let t = Tuning::default();
        let step = t.jog_step(0.0001, 0, Mag::Normal);
        assert!((step - 0.001).abs() < 0.0001, "must floor at min_step");
    }

    #[test]
    fn jog_step_ramp_accelerates() {
        let t = Tuning::default();
        let base = t.jog_step(1.0, 0, Mag::Normal);
        let ramped = t.jog_step(1.0, 500, Mag::Normal);
        assert!(ramped > base, "ramp must accelerate over time");
    }

    #[test]
    fn jog_step_ramp_capped() {
        let t = Tuning::default();
        let base = t.jog_step(1.0, 0, Mag::Normal);
        let capped = t.jog_step(1.0, 10000, Mag::Normal);
        let ratio = capped / base;
        assert!(ratio <= 8.0 + 0.01, "ramp must not exceed cap ×8");
    }

    #[test]
    fn jog_tracker_press_sets_held_returns_zero() {
        let mut jt = JogTracker::new();
        let now = Instant::now();
        let held = jt.press(now, 0);
        assert_eq!(held, 0);
    }

    #[test]
    fn jog_tracker_repeat_within_window_returns_duration() {
        let mut jt = JogTracker::new();
        let t0 = Instant::now();
        jt.press(t0, 0);
        let t1 = t0 + Duration::from_millis(100);
        let held = jt.repeat(t1, 100);
        assert!(held.is_some());
        assert!(held.unwrap() >= 90);
    }

    #[test]
    fn jog_tracker_repeat_outside_window_resets() {
        let mut jt = JogTracker::new();
        let t0 = Instant::now();
        jt.press(t0, 0);
        let t1 = t0 + Duration::from_millis(300);
        let held = jt.repeat(t1, 300);
        assert!(held.is_none(), "200ms+ gap must reset tracker");
    }

    #[test]
    fn jog_tracker_release_clears_state() {
        let mut jt = JogTracker::new();
        jt.press(Instant::now(), 0);
        jt.release();
        let t1 = Instant::now() + Duration::from_millis(10);
        let held = jt.repeat(t1, 10);
        assert!(held.is_none(), "release must prevent future repeats");
    }
}
