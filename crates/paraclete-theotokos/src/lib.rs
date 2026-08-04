mod action;
pub mod input;
pub mod model;
mod render;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Stdout;
use std::rc::Rc;
use std::time::Instant;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use paraclete_node_api::{CapabilityDocument, NodeCommand, StateBusHandle, StateBusValue};
use paraclete_view_assembly::CompositeView;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::action::{
    Action, Outcome, CMD_CHAIN_CLEAR, CMD_CHAIN_PUSH, CMD_CLEAR, CMD_CLEAR_STEP_LOCK,
    CMD_CLOCK_REWIND, CMD_CLOCK_START, CMD_CLOCK_STOP, CMD_LIVE_ERASE, CMD_PREPARE_PATTERN_MUTE,
    CMD_SET_LOCK_TARGET, CMD_SET_PATTERN, CMD_SET_PATTERN_MUTE, CMD_SET_STEP_CONDITION,
    CMD_SET_STEP_LENGTH, CMD_SET_STEP_LOCK, CMD_SET_STEP_TIMING, CMD_SET_STEP_VELOCITY,
    CMD_TRIG_NOW, PATTERN_BANK_SIZE,
};
use crate::input::{
    button_to_action, key_label, key_to_button, trig_button, HeldState, Keymap, Mods, PanelButton,
};
use crate::model::{
    parse_kit_binding, parse_kit_list, CmdlineVerb, Dir, EncoderTarget, JogTracker, Model,
    RecMode, Screen, Slot, StepDetail, StepParamKind, Tuning, YankedLock, YankedStep,
};

pub type BusHandle = Rc<RefCell<StateBusHandle>>;

/// TK3 C2: the MixNode id the MIX screen reads/writes gains on. Mirrors the
/// hard-coded app graph convention (AGENTS.md node table: 2 = MixNode).
pub const MIX_NODE_ID: u32 = 2;

pub struct TheotokosConfig {
    pub clock_id: u32,
    pub seq_ids: Vec<u32>,
    pub gen_ids: Vec<u32>,
    pub gen_names: Vec<String>,
    /// TK2.1 C0 (D2): the sequencer nodes' instrument-file `display_name`
    /// ("Kick", "Snare", ...) — distinct from `gen_names`, the cap-doc
    /// engine type name ("AnalogKick") used for the contextual header's
    /// second half.
    pub display_names: Vec<String>,
    pub caps: HashMap<u32, CapabilityDocument>,
    /// TK1 C3: composite views, one per track, same order as tracks —
    /// index-aligned, `None` where assembly failed (#152). Never compact this
    /// with `filter_map`; a hole must stay a hole.
    pub composite: Vec<Option<CompositeView>>,
    pub fps: u64,
}

pub struct TheotokosApp {
    model: Model,
    pending: Vec<NodeCommand>,
    /// P11 C1: app-level ops from Theotokos chords/KIT screen,
    /// drained by the main loop each tick.
    pending_app_ops: Vec<paraclete_node_api::app_op::AppOp>,
    quit: bool,
    dirty: bool,
    last_render: Instant,
    frame_ms: u64,
    tuning: Tuning,
    jog_a: JogTracker,
    jog_b: JogTracker,
    /// TK2 C5 (D13): slot C's own ramp tracker (fixes the TK1 `Slot::C`
    /// no-op — review finding, post-C5 hostile review).
    jog_c: JogTracker,
    /// TK2 C5 (D8): one ramp/acceleration tracker per encoder column,
    /// reusing the TK1 jog ramp machinery (`Tuning::jog_step`).
    encoder_trackers: [JogTracker; 8],
    /// TK2 C6 (D12): a ring of up to 4 tap-tempo timestamps (Tempo
    /// screen, YES). Oldest dropped once full.
    tap_times: Vec<Instant>,
    last_debug_event: Option<String>,
    /// TK2.2 C4 (E5): the name of the param the last `Action::EncoderJog`
    /// moved — persists across frames (not one-shot) until a later jog
    /// replaces it, so the panel keeps naming a jog's destination rather
    /// than flashing it for one frame. Whether that destination reads as
    /// a locked step or the live value is decided at render time from the
    /// *current* `lock_target_step` (see `render_status_line`), not
    /// stored here — so the locked-step naming clears the instant the
    /// target itself does, in lockstep, not on the next jog.
    last_jog_param: Option<String>,
    /// TK2 C3/C8 (D11): flat user keymap. Loaded global→local at startup
    /// (`Keymap::load_startup`); `:bind`/`:unbind`/`:reset-bindings` edit
    /// it at runtime; `:save-bindings` is the only write-to-disk path.
    keymap: Keymap,
    /// TK2 C3 (D6): TRK/PTN hold-chord state — kitty-probed at startup.
    held: HeldState,
}

impl TheotokosApp {
    pub fn new(config: TheotokosConfig) -> Result<Self, String> {
        setup_keyboard_flags()?;

        let mut model = Model::new(
            config.clock_id,
            &config.seq_ids,
            &config.gen_ids,
            &config.gen_names,
            &config.display_names,
            config.caps,
            config.composite,
        );

        let frame_ms = if config.fps > 0 {
            1000 / config.fps
        } else {
            33
        };

        // D6: without kitty keyboard-enhancement release events, the hold
        // chord falls back to the sticky one-shot grammar (§0 A9).
        let kitty = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);

        // ADR-046 T3: no startup command needed — `InternalClock` boots
        // `playing: false` at the source (closes BUG-039). ADR-044 D7's
        // startup `CMD_CLOCK_STOP` was a surface-side workaround for the
        // clock's old `playing: true` construction and is retired here,
        // not left as a second mechanism for the same invariant.
        let pending = Vec::with_capacity(64);
        let pending_app_ops = Vec::with_capacity(16);

        // TK2 C8 (D11): global→local YAML load at startup. TK2.1 C6
        // (D14): a stale entry (e.g. a retired button name) no longer
        // blocks the rest of the file — surface what got skipped, if
        // anything, as the app's first status line.
        let (keymap, skipped_bindings) = Keymap::load_startup();
        if !skipped_bindings.is_empty() {
            model.cmdline_status = Some(format!(
                "keymap.yaml: skipped {} unknown binding(s): {}",
                skipped_bindings.len(),
                skipped_bindings.join(", ")
            ));
        }

        Ok(Self {
            model,
            pending,
            pending_app_ops,
            quit: false,
            dirty: true,
            last_render: Instant::now(),
            frame_ms,
            tuning: Tuning::default(),
            jog_a: JogTracker::new(),
            jog_b: JogTracker::new(),
            jog_c: JogTracker::new(),
            encoder_trackers: std::array::from_fn(|_| JogTracker::new()),
            tap_times: Vec::new(),
            last_debug_event: None,
            last_jog_param: None,
            keymap,
            held: HeldState::new(kitty),
        })
    }

    pub fn process_events(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        bus: &BusHandle,
        _now_ms: u64,
        key_events: &[KeyEvent],
    ) -> Result<(), String> {
        // MM-C6 / ADR-041 decision 1: a machine switch repaints the pages
        // locally, from variants MM-C5 already merged. Polled rather than
        // subscribed because the switch can come from anywhere Theotokos does
        // not own — a Theoria client, a profile script, `:set machine` — and
        // it must repaint the same way whichever it was.
        self.dirty |= self.model.sync_machine_selection(&bus.borrow());
        self.dirty |= self.handle_keys(bus, key_events);
        self.render_if_needed(terminal, bus)
    }

    pub fn tick(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        bus: &BusHandle,
        now_ms: u64,
    ) -> Result<(), String> {
        let mut events: Vec<KeyEvent> = Vec::with_capacity(32);
        while event::poll(std::time::Duration::ZERO).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Key(ev) => {
                    // TK2 C3 (D6): a kitty terminal needs Release events
                    // too, to disarm a real-hold TRK/PTN prefix.
                    if self.held.kitty || is_press_or_repeat(ev) {
                        events.push(ev);
                    }
                }
                Event::Resize(_, _) => {
                    self.dirty = true;
                }
                // BUG-050: focus leaving mid-hold means the release that
                // would disarm a TRK/PTN prefix is never delivered. Treat it
                // as a release of everything rather than waiting for the user
                // to notice they are latched and find Esc.
                Event::FocusLost => {
                    self.held.on_focus_lost();
                    self.dirty = true;
                }
                _ => {}
            }
        }
        self.process_events(terminal, bus, now_ms, &events)
    }

    fn render_if_needed(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        bus: &BusHandle,
    ) -> Result<(), String> {
        let elapsed = self.last_render.elapsed().as_millis() as u64;
        if !self.dirty && elapsed < self.frame_ms {
            return Ok(());
        }

        let bus_ref = bus.borrow();
        let bus = &*bus_ref;
        let render_data = build_render_data(
            &mut self.model,
            bus,
            &self.held,
            &self.keymap,
            &self.tuning,
            self.last_jog_param.clone(),
            self.last_debug_event.take(),
        );

        drop(bus_ref);

        terminal
            .draw(|frame| render::render(frame, &render_data))
            .map_err(|e| e.to_string())?;

        self.dirty = false;
        self.last_render = Instant::now();
        Ok(())
    }

    /// Process key events without rendering — the test seam.
    /// Returns whether a redraw is needed.
    pub fn handle_keys(&mut self, bus: &BusHandle, key_events: &[KeyEvent]) -> bool {
        let bus_ref = bus.borrow();
        let state = &*bus_ref;
        let playing = self.model.playing(state);
        let now = Instant::now();
        let tick_ms = now.elapsed().as_millis() as u64;

        let mut dirty = false;
        let mut selected_changed = false;
        let mut lock_target_changed = false;
        for ev in key_events {
            // TK2.1 C6 (D11, hostile review finding): a fresh timestamp
            // per event, not the batch-level `now` above — `handle_keys`
            // can receive several buffered events in one call (`tick()`
            // drains the whole non-blocking poll queue before dispatching),
            // and `on_press`'s auto-repeat guard window needs to see the
            // real spacing between same-prefix presses, not a single
            // frozen instant shared by the whole batch (which would judge
            // every event in a multi-event batch as inside the guard
            // window, regardless of actual spacing).
            let event_now = Instant::now();
            // C6: while cmdline is open, capture ALL keys to the line editor
            if self.model.cmdline.is_some() {
                self.handle_cmdline_key(ev);
                dirty = true;
                continue;
            }

            // TK2 C3 (D14, §2): Ctrl-C, `:`, `?`, Backspace are fixed
            // utility keys outside the panel/keymap system entirely —
            // "unchanged from TK1" (§2); Ctrl-C and `:` are also
            // explicitly unbindable (D14). They do not interact with the
            // TRK/PTN hold-chord state machine below.
            let direct_action: Option<Action> = if ev.code == KeyCode::Char('c')
                && ev.modifiers == KeyModifiers::CONTROL
            {
                Some(Action::Quit)
            } else if matches!(ev.code, KeyCode::Char(':'))
                || (ev.code == KeyCode::Char(';') && ev.modifiers.contains(KeyModifiers::SHIFT))
            {
                // A6: `:` any-modifiers is the primary binding; `;`+SHIFT
                // is kept as the legacy alias.
                Some(Action::Colon)
            } else if ev.code == KeyCode::Char('?') {
                Some(Action::ToggleHelp)
            } else if ev.code == KeyCode::Backspace {
                // D12: on the Chain screen, Backspace clears the chain
                // (the same gesture as NO) instead of its usual
                // lock-clear meaning — still a direct/unbindable key
                // (§2), just screen-dependent.
                Some(if matches!(self.model.screen, Screen::Chain) {
                    Action::ChainClear
                } else if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    Action::ClearSlotLocks
                } else {
                    Action::ClearAllLocks
                })
            } else {
                None
            };

            let action = match direct_action {
                Some(a) => {
                    // D6: "any non-trig key disarms" — these direct keys
                    // bypass the button/hold system entirely, but a sticky
                    // (non-kitty) armed prefix must still drop on any other
                    // key. Kitty's real-hold prefix is exempt: it disarms
                    // only on physical release, not on an unrelated key
                    // pressed while TRK/PTN is still held down.
                    if !self.held.kitty {
                        self.held.armed = None;
                    }
                    a
                }
                None => {
                    let button = match key_to_button(&self.keymap, *ev) {
                        Some(b) => b,
                        None => continue,
                    };
                    let mods = Mods {
                        func: input::func_held(ev),
                        ctrl: ev.modifiers.contains(KeyModifiers::CONTROL),
                    };

                    // TK2.1 C5b (D15): snapshot BEFORE the Lock/Esc blocks
                    // below mutate `lock_target` — the eventual
                    // `button_to_action` resolution (the retap-clears check
                    // in particular) must see the state as it stood before
                    // THIS press (the same "resolve against the state
                    // before this press" principle `armed_before` already
                    // applies to TRK/PTN below).
                    let lock_target_step_before = self.model.lock_step_for_active_track();

                    // TK2.1 C5b (D15): pressing Lock while a target is
                    // already SET clears it — this needs `Model.lock_target`,
                    // which `HeldState` deliberately doesn't know about, so
                    // it's intercepted here rather than inside `on_press`.
                    // The OTHER "press Lock again" case — cancelling a
                    // still-*pending* arm (no target set yet) — is
                    // deliberately NOT special-cased here (TK2.1 C6,
                    // hostile review finding): it used to be, but that
                    // bypassed `on_press`'s D11 auto-repeat guard entirely,
                    // so an OS auto-repeat pulse while Lock was merely
                    // held down would immediately cancel the pending arm —
                    // exactly the failure D11 exists to prevent, and for
                    // the highest-value use of this whole hold-chord
                    // machinery (p-lock authoring). Falling through to the
                    // generic arm-on-press machinery below routes the
                    // pending-cancel case through the same guarded
                    // same-prefix-re-tap logic Trk/Ptn already get.
                    if button == PanelButton::Lock
                        && matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                        && self.model.lock_target.is_some()
                    {
                        self.model.lock_target = None;
                        self.held.armed = None;
                        lock_target_changed = true;
                        dirty = true;
                        continue;
                    }

                    // TK2.1 C5b (D15): Esc also clears an already-set
                    // lock target — a side effect layered on top of Esc's
                    // ordinary resolution (back to Grid, Chain clear,
                    // ...), not a replacement for it, so this does not
                    // `continue`.
                    if button == PanelButton::No
                        && matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                        && self.model.lock_target.is_some()
                    {
                        self.model.lock_target = None;
                        lock_target_changed = true;
                        dirty = true;
                    }

                    // D6: resolve against the hold state as it stood
                    // BEFORE this press — a completed chord (on_press /
                    // on_kitty_press already disarmed it as a side effect)
                    // must still resolve using the prefix that was armed
                    // when the trig landed.
                    let armed_before = self.held.armed;
                    let consumed = if self.held.kitty {
                        match ev.kind {
                            KeyEventKind::Release => {
                                self.held.on_kitty_release(button);
                                // P11 C6 (OQ-T25): releasing a bare NO
                                // while the transport plays disarms live
                                // erase on the selected track (the press
                                // armed it; see below).
                                if !mods.func && button == PanelButton::No && playing {
                                    if let Some(seq_id) = self
                                        .model
                                        .tracks
                                        .get(self.model.active_track)
                                        .map(|t| t.sequencer_id)
                                    {
                                        self.pending.push(NodeCommand {
                                            target_id: seq_id,
                                            type_id: CMD_LIVE_ERASE,
                                            arg0: 0,
                                            arg1: 0.0,
                                        });
                                        dirty = true;
                                    }
                                }
                                true
                            }
                            // TK2.1 C1 (D5, hostile review finding): unlike
                            // Trk/Ptn, REC's press is deliberately NOT
                            // consumed by `on_kitty_press` (its own action
                            // must fire on every press) — but that means an
                            // OS/terminal auto-repeat stream while the key
                            // stays physically down (this app requests
                            // `REPORT_EVENT_TYPES`, so repeats do arrive)
                            // would re-fire `Action::ToggleRec` once per
                            // pulse, flipping the mode on repeat-count
                            // parity instead of "once per physical press".
                            // Repeats are consumed silently; `armed` is
                            // already `Some(Hold::Rec)` from the initial
                            // press and stays that way.
                            //
                            // TK2.2 C1 (BUG-046): trig buttons need the same
                            // treatment. `on_kitty_press` returns `false`
                            // for a trig (it isn't a hold prefix), so
                            // without this arm a physical hold would
                            // resolve `button_to_action` on every repeat
                            // pulse — a held `Grid` trig rapid-toggling the
                            // step it's sitting on. A step write is
                            // once-per-physical-press; only the initial
                            // `Press` (which falls through to
                            // `on_kitty_press` below and is NOT consumed)
                            // should ever resolve an action.
                            KeyEventKind::Repeat
                                if button == PanelButton::Rec
                                    || input::trig_col(button).is_some() =>
                            {
                                true
                            }
                            // D6: "Esc disarms unconditionally" — implemented
                            // in the sticky path by `on_press`'s catch-all,
                            // but never wired here, so `on_esc` sat as dead
                            // code while the kitty branch cleared `armed`
                            // only on the matching physical release. A
                            // release that never arrives (focus taken
                            // mid-hold, protocol renegotiated) left every
                            // trig diverting with no way back but quitting
                            // (BUG-050). Not consumed: Esc's own action still
                            // resolves on this press, and it is never one of
                            // the diverted buttons (`button_to_action` only
                            // diverts trigs while armed).
                            KeyEventKind::Press if button == PanelButton::No => {
                                self.held.on_esc();
                                false
                            }
                            _ => self.held.on_kitty_press(button),
                        }
                    } else {
                        self.held.on_press(button, event_now)
                    };
                    if consumed {
                        dirty = true;
                        continue;
                    }

                    let mut held_for_resolution = HeldState::new(self.held.kitty);
                    held_for_resolution.armed = armed_before;
                    let screen_state = input::ScreenState {
                        screen: self.model.screen,
                        rec: self.model.rec,
                        enc: self.model.enc,
                        lock_target_step: lock_target_step_before,
                    };
                    // P11 C6 (OQ-T25, user decision 2026-08-02): a bare NO
                    // press while the transport plays arms live erase on
                    // the selected track's sequencer (`CMD_LIVE_ERASE 1`).
                    // The kitty path disarms on the matching release (the
                    // Release arm above); the sticky fallback has no
                    // release event, so it arms on press only and relies on
                    // the engine disarming at transport stop — a documented
                    // limitation of terminals without kitty support.
                    // Grid-only (review finding): on the Chain screen NO
                    // means "clear chain" — a bare NO there must not ALSO
                    // arm step erasing (a surprising destructive side
                    // effect); on Tempo/Settings NO navigates back to Grid.
                    if !mods.func
                        && button == PanelButton::No
                        && matches!(ev.kind, KeyEventKind::Press)
                        && playing
                        && self.model.screen == Screen::Grid
                    {
                        if let Some(seq_id) = self
                            .model
                            .tracks
                            .get(self.model.active_track)
                            .map(|t| t.sequencer_id)
                        {
                            self.pending.push(NodeCommand {
                                target_id: seq_id,
                                type_id: CMD_LIVE_ERASE,
                                arg0: 1,
                                arg1: 0.0,
                            });
                            dirty = true;
                        }
                    }
                    button_to_action(&held_for_resolution, &screen_state, button, mods)
                }
            };
            // TK3 C5 (OQ-T23b): FUNC+Space is a GLOBAL tap-tempo chord from
            // any screen — the "global chord" path. It must fire before the
            // A12 guard below (which turns FUNC+Space's collapsed ClearLane
            // into a Noop): this intercept produces TapTempo using the raw
            // key + FUNC, so A12's ClearLane check never matches.
            let action = if input::func_held(ev) && ev.code == KeyCode::Char(' ') {
                Action::TapTempo
            } else {
                action
            };
            // A12 (normative): FUNC+Space must stay a no-op — Space is a
            // transport-only Play alias; the destructive clear requires
            // the literal `x` home. `key_to_button` necessarily collapses
            // Space and `x` onto the same PanelButton::Play (D11), so
            // button_to_action has no way to tell them apart; override
            // using the raw key here (post-C4 hostile review: this
            // collapse previously let FUNC+Space silently wipe the active
            // pattern, the exact violation A12 prohibits).
            let action = if matches!(action, Action::ClearLane) && ev.code == KeyCode::Char(' ') {
                Action::Noop
            } else {
                action
            };

            // TK3 C2: FUNC+8 opens the MIX screen. Settings is bound to
            // bare `8`; the FUNC chord distinguishes MIX from Settings using
            // the raw key + modifier, the same way A12 above does.
            let action = if input::func_held(ev) && ev.code == KeyCode::Char('8') {
                Action::OpenScreen(Screen::Mix)
            } else {
                action
            };

            if !matches!(action, Action::Noop) || self.last_debug_event.is_some() {
                self.last_debug_event = Some(format!("{:?} → {:?}", ev, action));
            }

            // Clear any stale echo (a D9 out-of-range clamp, ...) once a
            // genuinely different action fires, so it can't persist
            // indefinitely and mask a later, more relevant one. The arms
            // below that need a fresh echo (out-of-range
            // SelectTrack/SelectPattern, Action::Echo itself) re-set it
            // after this clear, in the same dispatch. `cmdline_status`
            // (TK2 C8's success confirmations) gets the same treatment.
            if !matches!(action, Action::Noop) {
                self.model.cmdline_error = None;
                self.model.cmdline_status = None;
            }

            match action {
                Action::Quit => self.quit = true,
                Action::SelectTrack(i) => {
                    if i < self.model.tracks.len() {
                        self.model.select_track(i);
                        selected_changed = true;
                    } else {
                        self.model.cmdline_error = Some(format!("no track {}", i + 1));
                    }
                    dirty = true;
                }
                Action::PageWindow(dir) => {
                    let max_page = self
                        .model
                        .read_step_state(state, self.model.active_track)
                        .page_count
                        .saturating_sub(1);
                    let pw = &mut self.model.page_windows[self.model.active_track];
                    match dir {
                        Dir::Prev => *pw = pw.saturating_sub(1),
                        Dir::Next => {
                            if *pw < max_page {
                                *pw += 1;
                            }
                        }
                    }
                    dirty = true;
                }
                Action::Jog { slot, dir, mag } => {
                    let track = self.model.active_track;
                    if let Some(step) = self.model.lock_step_for_active_track() {
                        let binding = match slot {
                            Slot::A => &self.model.slot_a,
                            Slot::B => &self.model.slot_b,
                            Slot::C => &self.model.slot_c,
                        };
                        if let Some(ref b) = binding {
                            let tracker = match slot {
                                Slot::A => &mut self.jog_a,
                                Slot::B => &mut self.jog_b,
                                Slot::C => &mut self.jog_c,
                            };
                            let held = match tracker.repeat(now, tick_ms) {
                                Some(h) => h,
                                None => {
                                    tracker.press(now, tick_ms);
                                    0
                                }
                            };
                            let range = b.max - b.min;
                            let delta = self.tuning.jog_step(range, held, mag);
                            let signed = match dir {
                                Dir::Next => delta,
                                Dir::Prev => -delta,
                            };
                            let seq_id = self.model.tracks[track].sequencer_id;
                            let current = self
                                .model
                                .read_lock_value(state, seq_id, step, b.node_id, b.param_id)
                                .unwrap_or_else(|| {
                                    self.model.read_param_value(state, b.node_id, b.param_id)
                                });
                            let new_value = (current + signed).clamp(b.min, b.max);
                            self.pending.push(NodeCommand {
                                target_id: seq_id,
                                type_id: CMD_SET_LOCK_TARGET,
                                arg0: b.node_id as i64,
                                arg1: b.param_id as f64,
                            });
                            self.pending.push(NodeCommand {
                                target_id: seq_id,
                                type_id: CMD_SET_STEP_LOCK,
                                arg0: step as i64,
                                arg1: new_value,
                            });
                            dirty = true;
                        }
                    } else {
                        let binding = match slot {
                            Slot::A => &self.model.slot_a,
                            Slot::B => &self.model.slot_b,
                            Slot::C => &self.model.slot_c,
                        };
                        if let Some(ref b) = binding {
                            let tracker = match slot {
                                Slot::A => &mut self.jog_a,
                                Slot::B => &mut self.jog_b,
                                Slot::C => &mut self.jog_c,
                            };
                            let held = match tracker.repeat(now, tick_ms) {
                                Some(h) => h,
                                None => {
                                    tracker.press(now, tick_ms);
                                    0
                                }
                            };
                            let range = b.max - b.min;
                            let delta = self.tuning.jog_step(range, held, mag);
                            let signed = match dir {
                                Dir::Next => delta,
                                Dir::Prev => -delta,
                            };
                            self.pending.push(NodeCommand {
                                target_id: b.node_id,
                                type_id: paraclete_node_api::CMD_BUMP_PARAM,
                                arg0: b.param_id as i64,
                                arg1: signed,
                            });
                            dirty = true;
                        }
                    }
                }
                Action::PlayToggle => {
                    let outcome = action.execute(self.model.clock_id, 0, 0, playing);
                    match outcome {
                        Outcome::Command(cmd) => self.pending.push(cmd),
                        Outcome::Quit => self.quit = true,
                        _ => {}
                    }
                }
                // ADR-046 T5: bare STOP = halt in place, then rewind to
                // the window start. Two commands — outside `execute()`'s
                // single-`Outcome::Command` shape, so dispatched directly
                // here, same pattern as EnterLiveRec/CopyLane/PasteLane.
                Action::Stop => {
                    self.pending.push(NodeCommand {
                        target_id: self.model.clock_id,
                        type_id: CMD_CLOCK_STOP,
                        arg0: 0,
                        arg1: 0.0,
                    });
                    self.pending.push(NodeCommand {
                        target_id: self.model.clock_id,
                        type_id: CMD_CLOCK_REWIND,
                        arg0: 0,
                        arg1: 0.0,
                    });
                    dirty = true;
                }
                Action::ToggleStep { .. } => {
                    let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                    let pw = self.model.page_windows[self.model.active_track];
                    let outcome = action.execute(self.model.clock_id, seq_id, pw, playing);
                    match outcome {
                        Outcome::Command(cmd) => self.pending.push(cmd),
                        _ => {}
                    }
                }
                Action::Noop => {}
                Action::ClearAllLocks => {
                    let track = self.model.active_track;
                    if let Some(step) = self.model.lock_step_for_active_track() {
                        let seq_id = self.model.tracks[track].sequencer_id;
                        self.pending.push(NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_CLEAR_STEP_LOCK,
                            arg0: step as i64,
                            arg1: -1.0,
                        });
                        dirty = true;
                    }
                }
                Action::ClearSlotLocks => {
                    let track = self.model.active_track;
                    if let Some(step) = self.model.lock_step_for_active_track() {
                        let seq_id = self.model.tracks[track].sequencer_id;
                        if let Some(ref slot) = self.model.slot_a {
                            self.pending.push(NodeCommand {
                                target_id: seq_id,
                                type_id: CMD_SET_LOCK_TARGET,
                                arg0: slot.node_id as i64,
                                arg1: slot.param_id as f64,
                            });
                            self.pending.push(NodeCommand {
                                target_id: seq_id,
                                type_id: CMD_CLEAR_STEP_LOCK,
                                arg0: step as i64,
                                arg1: slot.param_id as f64,
                            });
                            dirty = true;
                        }
                    }
                }
                Action::ToggleMute(i) => {
                    if i < self.model.tracks.len() {
                        let seq_id = self.model.tracks[i].sequencer_id;
                        let current = state
                            .read(&format!("/node/{}/param/mute", seq_id))
                            .and_then(|v| match v {
                                paraclete_node_api::StateBusValue::Float(f) => Some(f),
                                _ => None,
                            })
                            .unwrap_or(&0.0);
                        let new_mute = if *current >= 0.5 { 0.0 } else { 1.0 };
                        let mute_id = paraclete_node_api::ParamDescriptor::id_for_name("mute");
                        self.pending.push(paraclete_node_api::NodeCommand {
                            target_id: seq_id,
                            type_id: paraclete_node_api::CMD_SET_PARAM,
                            arg0: mute_id as i64,
                            arg1: new_mute,
                        });
                        dirty = true;
                    }
                }
                Action::ToggleHelp => {
                    self.model.help_visible = !self.model.help_visible;
                    dirty = true;
                }
                Action::Colon => {
                    self.model.cmdline = Some(String::new());
                    self.model.cmdline_error = None;
                    dirty = true;
                }
                // TK2 C2/C3 (D9): PTN-hold + trig. Clamped against the
                // engine's fixed pattern bank (mirrors Sequencer's
                // PATTERN_BANK_SIZE); out of range is a no-op + echo, not
                // a malformed command.
                Action::SelectPattern(n) => {
                    if n < PATTERN_BANK_SIZE {
                        let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                        self.pending.push(NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_SET_PATTERN,
                            arg0: n as i64,
                            arg1: 0.0,
                        });
                    } else {
                        self.model.cmdline_error = Some(format!("no pattern {}", n + 1));
                    }
                    dirty = true;
                }
                // TK2.1 C1 (D6): a trig in a pad mode (Off/Live) sounds
                // AND selects track `col` — arg0/arg1 = 0 resolve to the
                // track's own default_note and velocity 0.5 (BUG-044: this
                // must match what that track's own sequenced steps sound,
                // not a fixed constant). A column past the discovered track
                // count is a silent no-op (no echo — D3 also gives those
                // columns no chip); order is normative, selection lands
                // before the trig command.
                Action::LiveTrig { col } => {
                    if col < self.model.tracks.len() {
                        self.model.select_track(col);
                        selected_changed = true;
                        let seq_id = self.model.tracks[col].sequencer_id;
                        self.pending.push(NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_TRIG_NOW,
                            arg0: 0,
                            arg1: 0.0,
                        });
                        dirty = true;
                    }
                }
                // TK2.1 C1 (D5): bare REC. From Live, always back to Off.
                // Otherwise: kitty path toggles Off<->Grid; the no-kitty
                // fallback arms by transport state (Live while running,
                // Grid while stopped) since it has no release event to
                // build a REC+PLAY chord from.
                Action::ToggleRec => {
                    let was_live = self.model.rec == RecMode::Live;
                    self.model.rec = if was_live {
                        RecMode::Off
                    } else if self.held.kitty {
                        match self.model.rec {
                            RecMode::Off => RecMode::Grid,
                            RecMode::Grid => RecMode::Off,
                            // Unreachable: the outer `if` above already
                            // peels off `Live`. Kept explicit (no `_`) so a
                            // future RecMode variant fails to compile here
                            // instead of silently routing to Off.
                            RecMode::Live => RecMode::Off,
                        }
                    } else if playing {
                        RecMode::Live
                    } else {
                        RecMode::Grid
                    };
                    // TK2.1 C3b (D8): leaving Live disarms engine-side
                    // recording on every track; the no-kitty fallback's
                    // own path into Live (no separate EnterLiveRec chord
                    // exists there — see D5) must arm it just the same,
                    // or live record silently does nothing on exactly the
                    // terminals that need the fallback (hostile review
                    // finding).
                    if was_live {
                        self.set_live_rec_for_all_tracks(0.0);
                    } else if self.model.rec == RecMode::Live {
                        self.set_live_rec_for_all_tracks(1.0);
                    }
                    dirty = true;
                }
                // TK2.1 C1 (D5/D8): REC held + PLAY (kitty only) — arms
                // Live and starts the transport. TK2.1 C3b (D8): also arms
                // engine-side `live_rec` on every track sequencer — no
                // step computation happens on the surface.
                Action::EnterLiveRec => {
                    self.model.rec = RecMode::Live;
                    self.pending.push(NodeCommand {
                        target_id: self.model.clock_id,
                        type_id: CMD_CLOCK_START,
                        arg0: 0,
                        arg1: 0.0,
                    });
                    self.set_live_rec_for_all_tracks(1.0);
                    dirty = true;
                }
                Action::OpenScreen(screen) => {
                    self.model.screen = screen;
                    // A different page opening always starts at sub-page 0
                    // (§0 A11) — `NextSubPage`, below, is the only thing
                    // that advances it.
                    self.model.sub_page = 0;
                    if let Screen::Param(idx) = screen {
                        self.model.select_perf_page(idx);
                    }
                    dirty = true;
                }
                Action::NextSubPage => {
                    let count = self.model.page_sub_page_count().max(1);
                    self.model.sub_page = (self.model.sub_page + 1) % count;
                    dirty = true;
                }
                // TK2 C4 (D7): FUNC+REC/PLAY/STOP copy/clear/paste,
                // reusing the unchanged TK1 yank/paste logic.
                Action::CopyLane => {
                    self.yank_active_pattern(state);
                    dirty = true;
                }
                Action::ClearLane => {
                    // §0 A8: CMD_CLEAR clears steps only — locks survive
                    // unless explicitly cleared per step.
                    let track = self.model.active_track;
                    let seq_id = self.model.tracks[track].sequencer_id;
                    let pattern_length = self.model.read_step_state(state, track).pattern_length;
                    self.pending.push(NodeCommand {
                        target_id: seq_id,
                        type_id: CMD_CLEAR,
                        arg0: 0,
                        arg1: 0.0,
                    });
                    for step in 0..pattern_length {
                        self.pending.push(NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_CLEAR_STEP_LOCK,
                            arg0: step as i64,
                            arg1: -1.0,
                        });
                    }
                    dirty = true;
                }
                Action::PasteLane => {
                    self.paste_pattern(state);
                    dirty = true;
                }
                // TK2 C5 (D8/§0 A11): encoder N = the active sub-page's Nth
                // param in Rule order. Beyond the sub-page's param count is
                // a no-op + echo, not a malformed command. Under step
                // focus, jog routes to that step's p-lock instead of a
                // live bump — the same TK1 step-focus path `Action::Jog`
                // uses (CMD 33/34), reusing the ramp/acceleration
                // machinery (`Tuning::jog_step`) via a per-column tracker.
                Action::EncoderJog { col, dir, mag } => {
                    // TK3 C5 (OQ-T23b): Tempo screen, encoder column 1
                    // (index 0) = continuous BPM jog — NudgeBpm semantics
                    // driven by encoder rotation instead of arrow keys.
                    if self.model.screen == Screen::Tempo && col == 0 {
                        let tracker = &mut self.encoder_trackers[col];
                        let held = match tracker.repeat(now, tick_ms) {
                            Some(h) => h,
                            None => {
                                tracker.press(now, tick_ms);
                                0
                            }
                        };
                        // Continuous over the 20..300 bpm span.
                        let delta = self.tuning.jog_step(280.0, held, mag);
                        let signed = match dir {
                            Dir::Next => delta,
                            Dir::Prev => -delta,
                        };
                        let current = self.model.read_bpm(state);
                        let bpm_id = paraclete_node_api::ParamDescriptor::id_for_name("bpm");
                        self.pending.push(NodeCommand {
                            target_id: self.model.clock_id,
                            type_id: paraclete_node_api::CMD_SET_PARAM,
                            arg0: bpm_id as i64,
                            arg1: (current + signed).max(1.0),
                        });
                        self.last_jog_param = Some("bpm".to_string());
                        dirty = true;
                        continue;
                    }
                    // TK3 C2: on the MIX screen the encoder bank's columns
                    // map to the MixNode's per-track gains (1..N) and master
                    // (column 8), not the active page's params.
                    if self.model.screen == Screen::Mix {
                        let name = if col < self.model.tracks.len() {
                            format!("input_gain_{}", col)
                        } else if col == 7 {
                            "master_gain".to_string()
                        } else {
                            self.model.cmdline_error =
                                Some(format!("encoder {} unbound on MIX", col + 1));
                            dirty = true;
                            continue;
                        };
                        let param_id =
                            paraclete_node_api::ParamDescriptor::id_for_name(&name);
                        let tracker = &mut self.encoder_trackers[col];
                        let held = match tracker.repeat(now, tick_ms) {
                            Some(h) => h,
                            None => {
                                tracker.press(now, tick_ms);
                                0
                            }
                        };
                        // MixNode gains span 0.0–2.0. TK3 C4: the step-size
                        // tier scales the base (range) before the ramp.
                        let tier_mult = (2f64).powi(self.model.step_size_tier as i32);
                        let delta = self.tuning.jog_step(2.0 * tier_mult, held, mag);
                        let signed = match dir {
                            Dir::Next => delta,
                            Dir::Prev => -delta,
                        };
                        let path = format!("/node/{MIX_NODE_ID}/param/{name}");
                        let current = match state.read(&path) {
                            Some(StateBusValue::Float(f)) => *f,
                            _ => 0.0,
                        };
                        if (signed > 0.0 && current >= 2.0)
                            || (signed < 0.0 && current <= 0.0)
                        {
                            self.model.cmdline_error =
                                Some(format!("{} at limit", name));
                            dirty = true;
                            continue;
                        }
                        self.last_jog_param = Some(name.clone());
                        self.pending.push(NodeCommand {
                            target_id: MIX_NODE_ID,
                            type_id: paraclete_node_api::CMD_BUMP_PARAM,
                            arg0: param_id as i64,
                            arg1: signed,
                        });
                        dirty = true;
                        continue;
                    }
                    let params = self.model.resolve_encoder_params();
                    // MM-C1: `col` indexes the placed bank directly, so an
                    // encoder whose slot no node declared is empty and jogs
                    // nothing — it no longer silently drives whichever param
                    // happened to be next in the list.
                    match params.get(col).cloned().flatten() {
                        None => {
                            // MM-C1: a declared gap is an existing encoder with
                            // nothing bound to it, which "no encoder N"
                            // described poorly once gaps became possible.
                            self.model.cmdline_error =
                                Some(format!("encoder {} unbound", col + 1));
                        }
                        Some(model::EncoderParam {
                            node_id,
                            param_id,
                            name,
                            min,
                            max,
                            stepped,
                            resolved: _,
                            options: _,
                            target,
                        }) => {
                            let track = self.model.active_track;
                            let tracker = &mut self.encoder_trackers[col];
                            let held = match tracker.repeat(now, tick_ms) {
                                Some(h) => h,
                                None => {
                                    tracker.press(now, tick_ms);
                                    0
                                }
                            };
                            // TK2.1 C4 (D10): a stepped param (an integer
                            // selector) moves exactly one unit per press,
                            // ignoring range/magnitude/ramp — held is still
                            // tracked above so the ramp resumes correctly
                            // if the user later reaches an unstepped param.
                            let delta = if stepped {
                                self.tuning.jog_step_stepped()
                            } else {
                                // TK3 C4 (OQ-T4): the tier scales the base
                                // step (`max(0.001, range/128)`) before the
                                // acceleration ramp — scaling the range feeds
                                // the same base, so the ramp curve is
                                // unchanged and its output is scaled.
                                let tier_mult =
                                    (2f64).powi(self.model.step_size_tier as i32);
                                let range = (max - min) * tier_mult;
                                self.tuning.jog_step(range, held, mag)
                            };
                            let signed = match dir {
                                Dir::Next => delta,
                                Dir::Prev => -delta,
                            };

                            // TK3 C0 (#180): a TRIG virtual param routes to a
                            // dedicated CMD_SET_STEP_* against the focused
                            // step, not a bank bump. See `EncoderTarget`.
                            if let EncoderTarget::VirtualStep { seq_id, kind } = target {
                                // "Focused step": the p-lock target when
                                // armed, else the current playing step — the
                                // same step the p-lock jog routes through
                                // (no new focus model).
                                let focus = self
                                    .model
                                    .lock_step_for_active_track()
                                    .unwrap_or_else(|| {
                                        self.model.read_step_state(state, track).current_step
                                    });
                                let detail =
                                    self.model.read_step_detail(state, seq_id, focus);
                                let current = detail.map_or(0.0, |d| match kind {
                                    StepParamKind::Velocity => d.velocity,
                                    StepParamKind::Length => d.length,
                                    StepParamKind::Timing => d.timing,
                                    StepParamKind::Condition => d.fill as f64,
                                });
                                if (signed > 0.0 && current >= max)
                                    || (signed < 0.0 && current <= min)
                                {
                                    self.model.cmdline_error =
                                        Some(format!("{} at limit", name));
                                    dirty = true;
                                    continue;
                                }
                                self.last_jog_param = Some(name.clone());
                                let new_value = (current + signed).clamp(min, max);
                                let (type_id, arg1) = match kind {
                                    StepParamKind::Velocity => {
                                        (CMD_SET_STEP_VELOCITY, new_value)
                                    }
                                    StepParamKind::Length => {
                                        (CMD_SET_STEP_LENGTH, new_value)
                                    }
                                    StepParamKind::Timing => {
                                        (CMD_SET_STEP_TIMING, new_value)
                                    }
                                    // Condition is a read-modify-write: only
                                    // the fill field changes; probability and
                                    // repeat are preserved from the step.
                                    StepParamKind::Condition => {
                                        let d = detail.unwrap_or(StepDetail {
                                            velocity: 0.0,
                                            length: 0.0,
                                            timing: 0.0,
                                            probability: 100,
                                            repeat_n: 0,
                                            repeat_m: 0,
                                            fill: 0,
                                        });
                                        (
                                            CMD_SET_STEP_CONDITION,
                                            crate::model::Model::pack_condition(
                                                &d,
                                                new_value.round() as u8,
                                            ),
                                        )
                                    }
                                };
                                self.pending.push(NodeCommand {
                                    target_id: seq_id,
                                    type_id,
                                    arg0: focus as i64,
                                    arg1,
                                });
                                dirty = true;
                                continue;
                            }

                            // MM-C6 / ADR-041 §0 A4: an identity param is what
                            // the node *is*, not a setting, so it is refused
                            // as a p-lock target — per-step machine switching
                            // is undesigned.
                            //
                            // It has to be refused *here*, surface-side. The
                            // sequencer stores opaque `(node_id, param_id)`
                            // pairs and cannot know it is holding a foreign
                            // node's identity param, so decision 6's
                            // "CMD_SET_LOCK_TARGET validation" cannot live
                            // there. The engines refuse the switch too — they
                            // read the bank rather than `get_param` — but that
                            // is the belt to this brace, and silently: without
                            // this the performer would author a lock, see it
                            // stored, and hear nothing happen.
                            if self.model.lock_step_for_active_track().is_some()
                                && self.model.is_identity_param(node_id, param_id)
                            {
                                self.model.cmdline_error =
                                    Some(format!("{} is the machine — cannot be locked", name));
                                dirty = true;
                                continue;
                            }
                            // BUG-064 item 2: read the current value once
                            // before both the boundary check and the command
                            // paths — for a locked step, use the lock value
                            // (falling back to live); for a live jog, use
                            // the live value.
                            let lock_step = self.model.lock_step_for_active_track();
                            let current = if let Some(step) = lock_step {
                                let seq_id = self.model.tracks[track].sequencer_id;
                                self.model
                                    .read_lock_value(
                                        state, seq_id, step, node_id, param_id,
                                    )
                                    .unwrap_or_else(|| {
                                        self.model.read_param_value(
                                            state, node_id, param_id,
                                        )
                                    })
                            } else {
                                self.model.read_param_value(state, node_id, param_id)
                            };

                            // BUG-064 item 2: range-end feedback — when the
                            // performer jogs into the end of a selector or
                            // parameter range, tell them rather than silently
                            // ignoring the press (#171).
                            if (signed > 0.0 && current >= max)
                                || (signed < 0.0 && current <= min)
                            {
                                self.model.cmdline_error =
                                    Some(format!("{} at limit", name));
                                dirty = true;
                                continue;
                            }

                            // TK2.2 C4 (E5): record what this jog is about
                            // to write — set after both the identity-param
                            // and range-end checks, so the status line never
                            // advertises a destination that was refused (#171).
                            self.last_jog_param = Some(name.clone());
                            if let Some(step) = lock_step {
                                let seq_id = self.model.tracks[track].sequencer_id;
                                let new_value = (current + signed).clamp(min, max);
                                self.pending.push(NodeCommand {
                                    target_id: seq_id,
                                    type_id: CMD_SET_LOCK_TARGET,
                                    arg0: node_id as i64,
                                    arg1: param_id as f64,
                                });
                                self.pending.push(NodeCommand {
                                    target_id: seq_id,
                                    type_id: CMD_SET_STEP_LOCK,
                                    arg0: step as i64,
                                    arg1: new_value,
                                });
                            } else {
                                self.pending.push(NodeCommand {
                                    target_id: node_id,
                                    type_id: paraclete_node_api::CMD_BUMP_PARAM,
                                    arg0: param_id as i64,
                                    arg1: signed,
                                });
                            }
                        }
                    }
                    dirty = true;
                }
                // TK2 C6 (D12): a ring of up to 4 taps; 2+ taps required
                // before a bpm is derived (a single tap has no interval to
                // measure). Averaging the whole window (not just the last
                // gap) smooths out one uneven tap.
                Action::TapTempo => {
                    self.tap_times.push(now);
                    if self.tap_times.len() > 4 {
                        self.tap_times.remove(0);
                    }
                    if self.tap_times.len() >= 2 {
                        let intervals: Vec<f64> = self
                            .tap_times
                            .windows(2)
                            .map(|w| w[1].duration_since(w[0]).as_secs_f64())
                            .collect();
                        let avg = intervals.iter().sum::<f64>() / intervals.len() as f64;
                        if avg > 0.0 {
                            let bpm = (60.0 / avg).clamp(20.0, 300.0);
                            let bpm_id = paraclete_node_api::ParamDescriptor::id_for_name("bpm");
                            self.pending.push(NodeCommand {
                                target_id: self.model.clock_id,
                                type_id: paraclete_node_api::CMD_SET_PARAM,
                                arg0: bpm_id as i64,
                                arg1: bpm,
                            });
                        }
                    }
                    dirty = true;
                }
                Action::NudgeBpm(delta) => {
                    let current = self.model.read_bpm(state);
                    let bpm_id = paraclete_node_api::ParamDescriptor::id_for_name("bpm");
                    self.pending.push(NodeCommand {
                        target_id: self.model.clock_id,
                        type_id: paraclete_node_api::CMD_SET_PARAM,
                        arg0: bpm_id as i64,
                        arg1: (current + delta).max(1.0),
                    });
                    dirty = true;
                }
                // TK3 C4 (OQ-T4): change the step-size tier, clamped to
                // 0..=4 (×1..×16). Persists until changed.
                Action::SetStepSizeTier(delta) => {
                    let new = (self.model.step_size_tier as i8 + delta).clamp(0, 4) as u8;
                    if new != self.model.step_size_tier {
                        self.model.step_size_tier = new;
                        dirty = true;
                    }
                }
                Action::ChainPush => {
                    let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                    self.pending.push(NodeCommand {
                        target_id: seq_id,
                        type_id: CMD_CHAIN_PUSH,
                        arg0: self.model.chain_cursor as i64,
                        arg1: 0.0,
                    });
                    dirty = true;
                }
                Action::ChainClear => {
                    let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                    self.pending.push(NodeCommand {
                        target_id: seq_id,
                        type_id: CMD_CHAIN_CLEAR,
                        arg0: 0,
                        arg1: 0.0,
                    });
                    dirty = true;
                }
                Action::MoveChainCursor(dir) => {
                    self.model.chain_cursor = match dir {
                        Dir::Prev => (self.model.chain_cursor + PATTERN_BANK_SIZE - 1) % PATTERN_BANK_SIZE,
                        Dir::Next => (self.model.chain_cursor + 1) % PATTERN_BANK_SIZE,
                    };
                    dirty = true;
                }
                Action::Echo(msg) => {
                    self.model.cmdline_error = Some(msg.to_string());
                    dirty = true;
                }
                // TK2.1 C5a (D9).
                Action::ToggleEnc => {
                    self.model.enc = !self.model.enc;
                    dirty = true;
                }
                // TK2.1 C5b (D15): the latched-arm case (Lock armed, this
                // trig consumes it) and the re-tap-clears case both land
                // here via `button_to_action`.
                Action::SetLockTarget(col) => {
                    self.model.lock_target = Some((self.model.active_track, col));
                    lock_target_changed = true;
                    dirty = true;
                }
                // BUG-064 item 3: Lock armed outside Grid mode —
                // surface the refusal rather than silently consuming
                // the arm indistinguishably from success (#171).
                Action::LockTargetRefused(_col) => {
                    self.model.cmdline_error =
                        Some("lock target needs GRID mode".into());
                    dirty = true;
                }
                Action::ClearLockTarget => {
                    self.model.lock_target = None;
                    lock_target_changed = true;
                    dirty = true;
                }
                // ── P11 C6: Theotokos surfaces ──
                // C6e: TRK+FUNC+trig — immediate pattern-mute toggle on
                // track N's sequencer (arg0 = 2).
                Action::TogglePatternMute(i) => {
                    if i < self.model.tracks.len() {
                        let seq_id = self.model.tracks[i].sequencer_id;
                        self.pending.push(NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_SET_PATTERN_MUTE,
                            arg0: 2,
                            arg1: 0.0,
                        });
                        dirty = true;
                    }
                }
                // C6e: TRK+FUNC+Ctrl+trig — prepared (deferred) pattern
                // mute. The target direction is read from the CURRENT
                // `/node/{id}/state/pattern_muted` so a second press of the
                // same chord queues the opposite state, mirroring how
                // ToggleMute reads the current mute before toggling.
                Action::PreparePatternMute(i) => {
                    if i < self.model.tracks.len() {
                        let seq_id = self.model.tracks[i].sequencer_id;
                        let muted = state
                            .read(&format!("/node/{}/state/pattern_muted", seq_id))
                            .is_some_and(|v| matches!(v, StateBusValue::Bool(b) if *b));
                        self.pending.push(NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_PREPARE_PATTERN_MUTE,
                            arg0: if muted { 0 } else { 1 },
                            arg1: 0.0,
                        });
                        dirty = true;
                    }
                }
                // C6a: encoder scroll on the KIT screen.
                Action::KitListScroll(dir) => {
                    self.model.scroll_kit_list(dir);
                    dirty = true;
                }
                // C6a: KIT screen LOAD/SAVE/COMMIT/RELD — app-level ops,
                // drained by the main loop's PerformState.
                Action::KitLoad => {
                    let id = (self.model.kit_cursor as u8).min(paraclete_node_api::app_op::KitId::MAX);
                    self.pending_app_ops.push(paraclete_node_api::app_op::AppOp::KitLoad(
                        paraclete_node_api::app_op::KitId(id),
                    ));
                    dirty = true;
                }
                Action::KitSaveAs => {
                    // C6a spec: auto-names "Kit N" from the selected slot.
                    self.pending_app_ops.push(
                        paraclete_node_api::app_op::AppOp::KitSaveAs(format!(
                            "Kit {}",
                            self.model.kit_cursor + 1
                        )),
                    );
                    dirty = true;
                }
                Action::KitCommit => {
                    self.pending_app_ops
                        .push(paraclete_node_api::app_op::AppOp::KitCommit);
                    dirty = true;
                }
                Action::KitReload => {
                    self.pending_app_ops
                        .push(paraclete_node_api::app_op::AppOp::KitReload);
                    dirty = true;
                }
                // C6b: FUNC+KIT toggles perform mode — the new value is
                // the inverse of the published `/context/perform`.
                Action::TogglePerformMode => {
                    let current = state
                        .read("/context/perform")
                        .is_some_and(|v| matches!(v, StateBusValue::Float(f) if *f >= 0.5));
                    self.pending_app_ops.push(
                        paraclete_node_api::app_op::AppOp::SetPerformMode(!current),
                    );
                    dirty = true;
                }
                // C6c: FUNC+YES / FUNC+NO — temp snapshot save/reload.
                Action::TempSave => {
                    self.pending_app_ops
                        .push(paraclete_node_api::app_op::AppOp::TempSave);
                    dirty = true;
                }
                Action::TempReload => {
                    self.pending_app_ops
                        .push(paraclete_node_api::app_op::AppOp::TempReload);
                    dirty = true;
                }
            }
        }
        drop(bus_ref);
        if selected_changed {
            if let Some(track) = self.model.tracks.get(self.model.active_track) {
                let mut bus_mut = bus.borrow_mut();
                bus_mut.write(
                    "/script/theotokos/selected",
                    paraclete_node_api::StateBusValue::Int(track.sequencer_id as i64),
                );
            }
        }
        // TK2.1 C5b (D15): published alongside `/script/theotokos/selected`
        // — the state a future cross-surface lock capture (ADR-045,
        // parked) will consume. -1 means "no lock target".
        if lock_target_changed {
            let mut bus_mut = bus.borrow_mut();
            bus_mut.write(
                "/script/theotokos/lock_step",
                paraclete_node_api::StateBusValue::Int(
                    self.model.lock_target.map(|(_, s)| s as i64).unwrap_or(-1),
                ),
            );
        }
        dirty
    }

    fn handle_cmdline_key(&mut self, ev: &KeyEvent) {
        let cmdline = match &mut self.model.cmdline {
            Some(s) => s,
            None => return,
        };
        match ev.code {
            KeyCode::Esc => {
                self.model.cmdline = None;
                self.model.cmdline_error = None;
            }
            KeyCode::Char('c') if ev.modifiers == KeyModifiers::CONTROL => {
                self.model.cmdline = None;
                self.model.cmdline_error = None;
                self.quit = true;
            }
            KeyCode::Enter => {
                let input = std::mem::take(cmdline);
                self.model.cmdline = None;
                self.model.cmdline_error = None;
                self.model.cmdline_status = None;
                match self.model.parse_cmdline(&input) {
                    Ok(verb) => {
                        self.dispatch_cmdline_verb(verb);
                    }
                    Err(msg) => {
                        self.model.cmdline_error = Some(msg);
                        // Re-open cmdline for error feedback
                        self.model.cmdline = Some(input);
                    }
                }
            }
            KeyCode::Backspace => {
                cmdline.pop();
            }
            KeyCode::Char(c) => {
                cmdline.push(c);
            }
            _ => {}
        }
    }

    fn dispatch_cmdline_verb(&mut self, verb: CmdlineVerb) {
        let track = self.model.active_track;
        let tracks = &self.model.tracks;
        match verb {
            CmdlineVerb::Set {
                node_id,
                param_name,
                value,
            } => {
                let param_id = paraclete_node_api::ParamDescriptor::id_for_name(&param_name);
                // TK2.1 C5b (D15): while a lock target is set ON THE
                // ACTIVE TRACK, Theotokos's own parameter motion routes to
                // that track's sequencer lock commands instead of the live
                // bank — gated the same way Action::Jog/EncoderJog are
                // (hostile review finding: this previously routed to the
                // lock target's OWN track unconditionally, so setting a
                // lock on track A, switching to track B, then `:set`ting a
                // param on B wrote a lock command to A's sequencer
                // referencing B's node — nonsensical engine-side). `:set`'s
                // `node_id` is always resolved against the active track,
                // never the locked one, so the active track's own
                // sequencer is the only sequencer that can be correct here.
                match self.model.lock_step_for_active_track().zip(tracks.get(track)) {
                    Some((step, active)) => {
                        let seq_id = active.sequencer_id;
                        self.pending.push(paraclete_node_api::NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_SET_LOCK_TARGET,
                            arg0: node_id as i64,
                            arg1: param_id as f64,
                        });
                        self.pending.push(paraclete_node_api::NodeCommand {
                            target_id: seq_id,
                            type_id: CMD_SET_STEP_LOCK,
                            arg0: step as i64,
                            arg1: value,
                        });
                    }
                    None => {
                        self.pending.push(paraclete_node_api::NodeCommand {
                            target_id: node_id,
                            type_id: paraclete_node_api::CMD_SET_PARAM,
                            arg0: param_id as i64,
                            arg1: value,
                        });
                    }
                }
            }
            CmdlineVerb::Bpm(val) => {
                let bpm_id = paraclete_node_api::ParamDescriptor::id_for_name("bpm");
                self.pending.push(paraclete_node_api::NodeCommand {
                    target_id: self.model.clock_id,
                    type_id: paraclete_node_api::CMD_SET_PARAM,
                    arg0: bpm_id as i64,
                    arg1: val,
                });
            }
            CmdlineVerb::Track(n) => {
                self.model.select_track(n);
            }
            CmdlineVerb::Pattern(n) => {
                if track < tracks.len() {
                    let seq_id = tracks[track].sequencer_id;
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: 27, // CMD_SET_PATTERN
                        arg0: n as i64,
                        arg1: 0.0,
                    });
                }
            }
            CmdlineVerb::Mute(n) => {
                if n < tracks.len() {
                    let seq_id = tracks[n].sequencer_id;
                    let mute_id = paraclete_node_api::ParamDescriptor::id_for_name("mute");
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: paraclete_node_api::CMD_SET_PARAM,
                        arg0: mute_id as i64,
                        arg1: 1.0,
                    });
                }
            }
            CmdlineVerb::Unmute(n) => {
                if n < tracks.len() {
                    let seq_id = tracks[n].sequencer_id;
                    let mute_id = paraclete_node_api::ParamDescriptor::id_for_name("mute");
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: paraclete_node_api::CMD_SET_PARAM,
                        arg0: mute_id as i64,
                        arg1: 0.0,
                    });
                }
            }
            CmdlineVerb::Clear => {
                if track < tracks.len() {
                    let seq_id = tracks[track].sequencer_id;
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: 18, // CMD_CLEAR
                        arg0: 0,
                        arg1: 0.0,
                    });
                }
            }
            CmdlineVerb::LockClear => {
                if let Some(step) = self.model.lock_step_for_active_track() {
                    let seq_id = tracks[track].sequencer_id;
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: CMD_CLEAR_STEP_LOCK,
                        arg0: step as i64,
                        arg1: -1.0,
                    });
                }
            }
            // TK2 C8 (D11): key/button names are already resolved and the
            // unbindable guard already applied by `parse_cmdline` — this
            // just inserts. `normalize_code` matches `key_to_button`'s
            // lookup key so an uppercase-typed binding still resolves.
            CmdlineVerb::BindKey { code, button } => {
                self.keymap.bindings.insert(
                    input::KeyBinding {
                        code: input::normalize_code(code),
                    },
                    button,
                );
                self.model.cmdline_status = Some(format!(
                    "{} → {}",
                    input::key_name(code),
                    input::button_name(button)
                ));
            }
            CmdlineVerb::UnbindKey { code } => {
                self.keymap.bindings.remove(&input::KeyBinding {
                    code: input::normalize_code(code),
                });
                self.model.cmdline_status = Some(format!("unbound {}", input::key_name(code)));
            }
            // TK2 C8 (D11): reuses the shared echo/status slot (`cmdline_status`,
            // styled distinctly from `cmdline_error` — post-C8 hostile
            // review: success confirmations previously reused the red error
            // slot, reading as failures) to list the active user bindings —
            // there is no dedicated overlay for it.
            CmdlineVerb::ListBindings => {
                if self.keymap.bindings.is_empty() {
                    self.model.cmdline_status = Some("no user bindings".to_string());
                } else {
                    let mut entries: Vec<String> = self
                        .keymap
                        .bindings
                        .iter()
                        .map(|(k, v)| format!("{}={}", input::key_name(k.code), input::button_name(*v)))
                        .collect();
                    entries.sort();
                    self.model.cmdline_status = Some(entries.join(" "));
                }
            }
            // TK2 C8 (D14): full fall-through to §2 defaults.
            CmdlineVerb::ResetBindings => {
                self.keymap.bindings.clear();
                self.model.cmdline_status = Some("all user bindings reset".to_string());
            }
            // TK2 C8 (D14): the only write path — no auto-save anywhere else.
            CmdlineVerb::SaveBindings => match self.keymap.save_global() {
                Ok(()) => self.model.cmdline_status = Some("bindings saved".to_string()),
                Err(e) => self.model.cmdline_error = Some(e),
            },
            // TK2 C8 (D11): re-runs the global→local startup load order,
            // replacing the runtime keymap outright (not merged with
            // whatever the session had bound since launch).
            CmdlineVerb::LoadBindings => {
                let (keymap, skipped) = Keymap::load_startup();
                self.keymap = keymap;
                self.model.cmdline_status = Some(if skipped.is_empty() {
                    format!("loaded {} binding(s)", self.keymap.bindings.len())
                } else {
                    format!(
                        "loaded {} binding(s); skipped {}: {}",
                        self.keymap.bindings.len(),
                        skipped.len(),
                        skipped.join(", ")
                    )
                });
            }
        }
    }

    /// TK2.1 C3b (D8, ADR-039 decision 7): arms/disarms engine-side
    /// `live_rec` on every track sequencer. No step computation happens
    /// here or anywhere on the surface — the sequencer quantizes a
    /// consumed `CMD_TRIG_NOW` to the nearest step itself while this flag
    /// is set and the transport is running.
    fn set_live_rec_for_all_tracks(&mut self, value: f64) {
        let live_rec_id = paraclete_node_api::ParamDescriptor::id_for_name("live_rec");
        for track in &self.model.tracks {
            self.pending.push(NodeCommand {
                target_id: track.sequencer_id,
                type_id: paraclete_node_api::CMD_SET_PARAM,
                arg0: live_rec_id as i64,
                arg1: value,
            });
        }
    }

    // ── C7: yank, paste (called by TK2 C4's CopyLane/PasteLane, D7) ──

    fn yank_active_pattern(&mut self, bus: &StateBusHandle) {
        let track = self.model.active_track;
        if track >= self.model.tracks.len() {
            return;
        }
        let seq_id = self.model.tracks[track].sequencer_id;
        let steps_text = bus
            .read(&format!("/node/{}/state/steps", seq_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let locks_text = bus
            .read(&format!("/node/{}/state/locks", seq_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let mut yanked: Vec<YankedStep> = Vec::with_capacity(steps_text.len());
        for (i, ch) in steps_text.chars().enumerate() {
            let active = ch == '1';
            let mut locks: Vec<YankedLock> = Vec::new();
            for entry in locks_text.split(';') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = entry.splitn(4, [':', '=']).collect();
                if parts.len() != 4 {
                    continue;
                }
                if let Some(rest) = parts[0].strip_prefix('s') {
                    if let Ok(s) = rest.parse::<usize>() {
                        if s == i {
                            let nid: u32 = parts[1].parse().unwrap_or(0);
                            let pid: u32 = parts[2].parse().unwrap_or(0);
                            let val: f64 = parts[3].parse().unwrap_or(0.0);
                            locks.push(YankedLock {
                                node_id: nid,
                                param_id: pid,
                                value: val,
                            });
                        }
                    }
                }
            }
            yanked.push(YankedStep {
                active,
                note: if active { 36 } else { -1 },
                velocity: if active { 1.0 } else { 0.0 },
                length: 1.0,
                timing: 0,
                condition: 0.0,
                locks,
            });
        }
        self.model.yank_buffer = yanked;
    }

    fn paste_pattern(&mut self, bus: &StateBusHandle) {
        if self.model.yank_buffer.is_empty() {
            return;
        }
        let src_track = self.model.active_track; // same-track paste for now
        let dst_track = src_track;
        if dst_track >= self.model.tracks.len() {
            return;
        }
        let seq_id = self.model.tracks[dst_track].sequencer_id;
        let src_gen = self.model.tracks[src_track].generator_id;
        let dst_gen = self.model.tracks[dst_track].generator_id;

        let dst_steps = bus
            .read(&format!("/node/{}/state/steps", seq_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.len()),
                _ => None,
            })
            .unwrap_or(16);
        let max_steps = self.model.yank_buffer.len().min(dst_steps);

        for i in 0..max_steps {
            let step = &self.model.yank_buffer[i];
            // 1. Clear stale locks
            self.pending.push(paraclete_node_api::NodeCommand {
                target_id: seq_id,
                type_id: CMD_CLEAR_STEP_LOCK,
                arg0: i as i64,
                arg1: -1.0,
            });
            // 2. Set step active + note
            self.pending.push(paraclete_node_api::NodeCommand {
                target_id: seq_id,
                type_id: 17, // CMD_SET_STEP
                arg0: i as i64,
                arg1: step.note as f64,
            });
            // 3. Velocity + length
            self.pending.push(paraclete_node_api::NodeCommand {
                target_id: seq_id,
                type_id: 36, // CMD_SET_STEP_VELOCITY
                arg0: i as i64,
                arg1: step.velocity,
            });
            self.pending.push(paraclete_node_api::NodeCommand {
                target_id: seq_id,
                type_id: 37, // CMD_SET_STEP_LENGTH
                arg0: i as i64,
                arg1: step.length,
            });
            // 4. Timing + condition
            self.pending.push(paraclete_node_api::NodeCommand {
                target_id: seq_id,
                type_id: 25, // CMD_SET_STEP_TIMING
                arg0: i as i64,
                arg1: step.timing as f64,
            });
            self.pending.push(paraclete_node_api::NodeCommand {
                target_id: seq_id,
                type_id: 26, // CMD_SET_STEP_CONDITION
                arg0: i as i64,
                arg1: step.condition,
            });
            // 5. Lock pairs
            for lock in &step.locks {
                let nid = if lock.node_id == src_gen {
                    dst_gen
                } else {
                    lock.node_id
                };
                if nid == dst_gen || src_track == dst_track {
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: CMD_SET_LOCK_TARGET,
                        arg0: nid as i64,
                        arg1: lock.param_id as f64,
                    });
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: CMD_SET_STEP_LOCK,
                        arg0: i as i64,
                        arg1: lock.value,
                    });
                }
            }
        }
    }

    pub fn take_pending_commands(&mut self) -> Vec<NodeCommand> {
        std::mem::take(&mut self.pending)
    }

    /// P11 C1: drain app-level ops (kit load/save, temp save/reload,
    /// perform mode, etc.) produced by Theotokos key chords and the
    /// KIT screen.  Called by the main loop each tick.
    pub fn take_pending_app_ops(
        &mut self,
    ) -> Vec<paraclete_node_api::app_op::AppOp> {
        std::mem::take(&mut self.pending_app_ops)
    }

    /// TK3 C3 (#184): non-destructive observer for the pending app ops — a
    /// test can inspect them (e.g. after driving key chords through
    /// `handle_keys`) without consuming them, so the normal destructive
    /// drain path can still run afterwards. `take_pending_app_ops` remains
    /// the drain; this is its read-only companion.
    pub fn pending_app_ops(&self) -> &[paraclete_node_api::app_op::AppOp] {
        &self.pending_app_ops
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn shutdown(&self) -> Result<(), String> {
        pop_keyboard_flags()?;
        Ok(())
    }
}

fn is_press_or_repeat(ev: KeyEvent) -> bool {
    matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn setup_keyboard_flags() -> Result<(), String> {
    use crossterm::event::{
        EnableFocusChange, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    };
    // §0 A2: REPORT_EVENT_TYPES alone yields no release events for text
    // keys (Tab, `p`, all trigs) — exactly the events TK2 C3's kitty
    // hold-chord branch (HeldState::on_kitty_press/release) needs. Without
    // the other two flags, TRK/PTN would arm on press and never receive
    // the release that disarms them.
    //
    // EnableFocusChange is what makes that failure recoverable (BUG-050): a
    // hold interrupted by focus loss never gets its release, so the panel
    // needs to hear about the focus change to disarm.
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        ),
        EnableFocusChange
    )
    .map(|_| {})
    .map_err(|e| format!("kitty flags: {e}"))
}

fn pop_keyboard_flags() -> Result<(), String> {
    use crossterm::event::{DisableFocusChange, PopKeyboardEnhancementFlags};
    execute!(
        std::io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableFocusChange
    )
        .map(|_| {})
        .map_err(|e| format!("kitty flags pop: {e}"))
}


/// TK3 C1 (#163): assemble the data the panel draws — extracted from
/// `render_if_needed`, which used to build `RenderData` inline. No I/O
/// (no `Terminal`, no bus writes): reads the state bus and the model.
/// It is not functionally pure — it takes `&mut Model` because the slot/
/// encoder flash timestamps are a bookkeeping side effect.
///
/// Signature note: the spec sketch listed a `now: Instant` param; it is
/// omitted because the assembly uses stored `Instant` timestamps
/// internally (`update_flash` stamps `Instant::now()`), so an external
/// `now` would be dead.
pub fn build_render_data(
    model: &mut Model,
    bus: &StateBusHandle,
    held: &HeldState,
    keymap: &Keymap,
    tuning: &Tuning,
    last_jog_param: Option<String>,
    debug_event: Option<String>,
) -> render::RenderData {
        let step_states: Vec<_> = (0..model.tracks.len())
            .map(|t| model.read_step_state(bus, t))
            .collect();
        let step_state = step_states
            .get(model.active_track)
            .cloned()
            .unwrap_or_default();
        let bpm = model.read_bpm(bus);

        let slot_a_value = model
            .slot_a
            .as_ref()
            .map(|s| model.read_param_value(bus, s.node_id, s.param_id))
            .unwrap_or(0.0);
        let slot_b_value = model
            .slot_b
            .as_ref()
            .map(|s| model.read_param_value(bus, s.node_id, s.param_id))
            .unwrap_or(0.0);
        let slot_c_value = model
            .slot_c
            .as_ref()
            .map(|s| model.read_param_value(bus, s.node_id, s.param_id))
            .unwrap_or(0.0);

        model.update_flash(0, slot_a_value);
        model.update_flash(1, slot_b_value);
        // TK2 C5 (D13): slot C flashes too — was bound but never tracked
        // (review finding, post-C5 hostile review).
        model.update_flash(2, slot_c_value);

        let envelope = model.envelope_for_active_track().map(|e| {
            let val = model.read_param_value(bus, e.node_id, e.param_id);
            (e, val)
        });
        let live_env_level = envelope.as_ref().and_then(|(env, _)| {
            bus.read(&format!("/node/{}/state/env_level", env.node_id))
                .and_then(|v| match &v {
                    StateBusValue::Float(f) => Some(*f),
                    _ => None,
                })
        });

        let live_lfo_phase: Option<f64> = {
            bus.iter()
                .find(|(k, _)| k.ends_with("/state/lfo_phase"))
                .and_then(|(_, v)| match v {
                    StateBusValue::Float(f) => Some(*f),
                    _ => None,
                })
        };

        let step_locks: Vec<Vec<usize>> = (0..model.tracks.len())
            .map(|t| model.read_step_locks(bus, t))
            .collect();
        // TK2 C4 (D12): per-track mute state, rendered on the track
        // indicator — the dedicated Mute screen this was originally
        // built for was retired in TK2.1 C6.
        let mute_states: Vec<bool> = model
            .tracks
            .iter()
            .map(|t| {
                bus.read(&format!("/node/{}/param/mute", t.sequencer_id))
                    .is_some_and(|v| matches!(v, paraclete_node_api::StateBusValue::Float(f) if *f >= 0.5))
            })
            .collect();

        // TK2 C6 (D12): Chain screen state, read from the active track's
        // published sequencer state.
        let active_seq_id = model.tracks[model.active_track].sequencer_id;
        let read_int = |path: String| -> Option<i64> {
            match bus.read(&path) {
                Some(paraclete_node_api::StateBusValue::Int(i)) => Some(*i),
                _ => None,
            }
        };
        let active_pattern =
            read_int(format!("/node/{active_seq_id}/state/active_pattern")).unwrap_or(0) as usize;
        let cued_pattern_raw =
            read_int(format!("/node/{active_seq_id}/state/cued_pattern")).unwrap_or(-1);
        let cued_pattern = if cued_pattern_raw >= 0 {
            Some(cued_pattern_raw as usize)
        } else {
            None
        };
        let chain_len =
            read_int(format!("/node/{active_seq_id}/state/chain_len")).unwrap_or(0) as usize;
        let page_loop = (
            read_int(format!("/node/{active_seq_id}/state/page_loop_start")).unwrap_or(0) as u8,
            read_int(format!("/node/{active_seq_id}/state/page_loop_end")).unwrap_or(0) as u8,
        );

        // P11 C6a: KIT screen state from the app-published context paths.
        // `/context/kits` is names-only (`idx:name;...`) — the encoder-bank
        // preview therefore shows the selected kit's name as a placeholder
        // for its entries (values are not on the bus in P11).
        let read_text = |path: &str| -> String {
            bus.read(path)
                .and_then(|v| match v {
                    StateBusValue::Text(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        };
        let kit_list = parse_kit_list(&read_text("/context/kits"));
        let kit_binding = parse_kit_binding(&read_text("/context/kit_binding"));
        // The "loaded" kit is the one bound to the selected track's active
        // pattern slot (from `/context/kit_binding`, `slot:kit` pairs).
        let kit_loaded_slot = kit_binding
            .iter()
            .find(|(slot, kit)| *slot == active_pattern && *kit >= 0)
            .map(|(_, kit)| *kit as usize);
        // P11 C6b: perform-mode indicator — `/context/perform` Float ≥ 0.5.
        let perform_mode = bus
            .read("/context/perform")
            .is_some_and(|v| matches!(v, StateBusValue::Float(f) if *f >= 0.5));
        // P11 C6e: per-track pattern-mute state for the track indicator.
        let pattern_muted_states: Vec<bool> = model
            .tracks
            .iter()
            .map(|t| {
                bus.read(&format!("/node/{}/state/pattern_muted", t.sequencer_id))
                    .is_some_and(|v| matches!(v, StateBusValue::Bool(b) if *b))
            })
            .collect();

        // TK3 C2: per-track + master gains for the MIX screen. Read by name
        // directly (MixNode may not be in `model.caps`). Paths are
        // `input_gain_0..n-1` and `master_gain` on the hard-coded MixNode.
        let read_gain = |path: &str| -> f64 {
            match bus.read(path) {
                Some(StateBusValue::Float(f)) => *f,
                _ => 0.0,
            }
        };
        let mix_gains: Vec<f64> = (0..model.tracks.len())
            .map(|i| read_gain(&format!("/node/{MIX_NODE_ID}/param/input_gain_{i}")))
            .collect();
        let mix_master = read_gain(&format!("/node/{MIX_NODE_ID}/param/master_gain"));

        // TK2 C5 (D8/§0 A11): the active page's params in Rule order,
        // restricted to the current sub-page's 8-wide window (pages with
        // more than 8 params split into sub-pages instead of silently
        // truncating). Resolved fresh each render, matching the jog
        // dispatch below.
        let encoder_params = model.resolve_encoder_params();
        let encoder_cells: Vec<Option<render::EncoderCell>> = encoder_params
            .iter()
            .map(|cell| {
                cell.as_ref()
                    .map(|p| {
                        // TK3 C0 (#180): a virtual step cell reads the
                        // focused step's per-step detail, not a bank param.
                        let value = match p.target {
                            model::EncoderTarget::VirtualStep { seq_id, kind } => {
                                let focus = model
                                    .lock_step_for_active_track()
                                    .unwrap_or_else(|| {
                                        model
                                            .read_step_state(bus, model.active_track)
                                            .current_step
                                    });
                                model
                                    .read_step_detail(bus, seq_id, focus)
                                    .map(|d| match kind {
                                        model::StepParamKind::Velocity => d.velocity,
                                        model::StepParamKind::Length => d.length,
                                        model::StepParamKind::Timing => d.timing,
                                        model::StepParamKind::Condition => d.fill as f64,
                                    })
                                    .unwrap_or(0.0)
                            }
                            model::EncoderTarget::Real => {
                                model.read_param_value(bus, p.node_id, p.param_id)
                            }
                        };
                        render::EncoderCell {
                            name: p.name.clone(),
                            value,
                            min: p.min,
                            max: p.max,
                            resolved: p.resolved,
                            options: p.options.clone(),
                            target: p.target,
                        }
                    })
            })
            .collect();
        for (i, cell) in encoder_cells.iter().enumerate() {
            if let Some(c) = cell {
                model.update_encoder_flash(i, c.value);
            }
        }
        let encoder_flash: Vec<bool> = (0..8)
            .map(|i| {
                model.encoder_flash[i]
                    .is_some_and(|t| t.elapsed().as_millis() < tuning.flash_ms as u128)
            })
            .collect();

        let mut slot_a_locked = false;
        let mut slot_b_locked = false;
        let mut slot_c_locked = false;
        if let Some(focus) = model.lock_step_for_active_track() {
            if let Some(ref s) = model.slot_a {
                let seq_id = model.tracks[model.active_track].sequencer_id;
                slot_a_locked = model
                    .read_lock_value(bus, seq_id, focus, s.node_id, s.param_id)
                    .is_some();
            }
            if let Some(ref s) = model.slot_b {
                let seq_id = model.tracks[model.active_track].sequencer_id;
                slot_b_locked = model
                    .read_lock_value(bus, seq_id, focus, s.node_id, s.param_id)
                    .is_some();
            }
            if let Some(ref s) = model.slot_c {
                let seq_id = model.tracks[model.active_track].sequencer_id;
                slot_c_locked = model
                    .read_lock_value(bus, seq_id, focus, s.node_id, s.param_id)
                    .is_some();
            }
        }

        // TK2.1 C2 (D3/D4): key chip labels, resolved fresh each render
        // against the live keymap (bindings can change via `:bind` at
        // runtime).
        let trig_key_labels: Vec<Option<String>> = (0..16)
            .map(|i| trig_button(i).and_then(|b| key_label(&keymap, b)))
            .collect();
        let track_key_labels: Vec<Option<String>> =
            trig_key_labels[..model.tracks.len().min(16)].to_vec();
        let legend_key_labels: HashMap<PanelButton, String> = [
            PanelButton::Trk,
            PanelButton::Ptn,
            PanelButton::Rec,
            PanelButton::Play,
            PanelButton::Stop,
            PanelButton::Song,
            PanelButton::Tempo,
            PanelButton::Settings,
            PanelButton::Yes,
            PanelButton::No,
            PanelButton::Enc,
            PanelButton::Lock,
        ]
        .into_iter()
        .filter_map(|b| key_label(&keymap, b).map(|k| (b, k)))
        .collect();

        let render_data = render::RenderData {
            screen: model.screen,
            rec: model.rec,
            armed_prefix: match held.armed {
                Some(input::Hold::Trk) => Some("TRK…".to_string()),
                Some(input::Hold::Ptn) => Some("PTN…".to_string()),
                // TK2.1 C5b: Lock armed and waiting for the next trig.
                Some(input::Hold::Lock) => Some("LOCK…".to_string()),
                // REC has its own three-state transport/status indicator
                // (D5) — it isn't an "armed prefix" chip like TRK/PTN.
                Some(input::Hold::Rec) | None => None,
            },
            active_track: model.active_track,
            // #161: per-track engine label, machine-aware — not
            // `TrackInfo.name`, which freezes at the built-with machine.
            track_names: (0..model.tracks.len())
                .map(|t| model.engine_label(t))
                .collect(),
            display_names: model.tracks.iter().map(|t| t.display_name.clone()).collect(),
            trig_key_labels,
            track_key_labels,
            legend_key_labels,
            bpm,
            playing: model.playing(bus),
            page_window: model.page_windows[model.active_track],
            step_state,
            step_states,
            slot_a: model.slot_a.clone(),
            slot_a_value,
            slot_b: model.slot_b.clone(),
            slot_b_value,
            slot_c: model.slot_c.clone(),
            slot_c_value,
            page_groups: model.page_groups_for_active_track(),
            perf_page: model.perf_page,
            sub_page: model.sub_page,
            sub_page_count: model.page_sub_page_count(),
            envelope,
            live_env_level,
            live_lfo_phase,
            debug_event,
            enc: model.enc,
            lock_target_step: model.lock_step_for_active_track(),
            last_jog_param,
            step_locks,
            mute_states,
            slot_a_locked,
            slot_b_locked,
            slot_c_locked,
            cmdline: model.cmdline.clone(),
            cmdline_error: model.cmdline_error.clone(),
            cmdline_status: model.cmdline_status.clone(),
            cmdline_candidates: model.cmdline_candidates(),
            slot_a_flash: model.slot_flash[0].map_or(false, |t| {
                t.elapsed().as_millis() < tuning.flash_ms as u128
            }),
            slot_b_flash: model.slot_flash[1].map_or(false, |t| {
                t.elapsed().as_millis() < tuning.flash_ms as u128
            }),
            slot_c_flash: model.slot_flash[2].map_or(false, |t| {
                t.elapsed().as_millis() < tuning.flash_ms as u128
            }),
            help_visible: model.help_visible,
            encoder_cells,
            encoder_flash,
            kitty: held.kitty,
            pattern_bank_size: PATTERN_BANK_SIZE,
            active_pattern,
            cued_pattern,
            chain_len,
            page_loop,
            chain_cursor: model.chain_cursor,
            kit_scroll: model.kit_scroll,
            kit_cursor: model.kit_cursor,
            kit_list,
            kit_loaded_slot,
            perform_mode,
            pattern_muted_states,
            mix_gains,
            mix_master,
            step_size_tier: model.step_size_tier,
        };
        render_data

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Screen, SlotBinding, TrackInfo};
    use crossterm::event::{KeyCode, KeyModifiers};
    use paraclete_node_api::{
        AffordanceHint, CapabilityDocument, PageRef, ParamDescriptor, ParamUnit, Rule,
        StateBusValue,
    };
    use paraclete_view_assembly::{
        CompositeOverlay, CompositeParam, CompositePage, CompositeVariant, CompositeVariantSet,
    };
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// TK2.1 C4: a one-page, one-param composite view — the shape
    /// `resolve_encoder_params` must resolve against `Model::caps` rather
    /// than the `0.0, 1.0` placeholder (BUG-040).
    fn composite_view_with_param(node_id: u32, param_id: u32) -> CompositeView {
        CompositeView {
            engine_node_id: node_id,
            engine_name: "Test".into(),
            display_name: "Test".into(),
            pages: vec![CompositePage {
                id: "p1".into(),
                label: "P1".into(),
                params: vec![CompositeParam {
                    node_id,
                    param_id,
                    name: "param".into(),
                    label: "Param".into(),
                    affordance: AffordanceHint::None,
                    env_group: None,
                    slot: 0,
                    routing: None,
                    stepped: false,
                    options: None,
                }],
                envelopes: vec![],
                macros: vec![],
            }],
            chain: vec![],
            routes: vec![],
            variants: vec![],
        }
    }

    /// A one-page composite whose params sit at the given `(param_id, slot)`
    /// pairs — for asserting placement, which a dense fixture cannot show.
    fn composite_view_with_slots(node_id: u32, params: &[(u32, u8)]) -> CompositeView {
        CompositeView {
            engine_node_id: node_id,
            engine_name: "Test".into(),
            display_name: "Test".into(),
            pages: vec![CompositePage {
                id: "p1".into(),
                label: "P1".into(),
                params: params
                    .iter()
                    .map(|&(param_id, slot)| CompositeParam {
                        node_id,
                        param_id,
                        name: format!("p{param_id}"),
                        label: format!("P{param_id}"),
                        affordance: AffordanceHint::None,
                        env_group: None,
                        slot,
                        routing: None,
                        stepped: false,
                        options: None,
                    })
                    .collect(),
                envelopes: vec![],
                macros: vec![],
            }],
            chain: vec![],
            routes: vec![],
            variants: vec![],
        }
    }

    // ── MM-C6 fixtures: a machine host ───────────────────────────────────

    const MACHINE_PID: u32 = 900;
    const TONE_PID: u32 = 901;
    const PUNCH_PID: u32 = 902;

    /// A two-machine host. Machine 0 pages `machine`, `tone` and `punch`;
    /// machine 1 drops `punch` and narrows `tone` — the AnalogKick/AnalogHiHat
    /// shape, where a param one machine declares is simply absent on another.
    ///
    /// `machine` is paged here so the p-lock rejection has an encoder to
    /// reject. **No shipped node pages it yet** — where machine-select is
    /// declared is the one MM-C6 decision left open (ADR-041 amendment 2 says
    /// TRIG but not who declares it), so this fixture stands in for whichever
    /// way that lands.
    fn machine_host_view(node_id: u32) -> CompositeView {
        // ADR-041 amendment 2026-08-02: the per-machine pages carry that
        // machine's `options` for a stepped param whose labels differ by
        // machine (`lfo_dest`-shaped) — the `param_labels` the engines now
        // declare land here via assembly, and the machine switch swaps whole
        // pages, so the labels must follow. `tone` carries machine-specific
        // options so a test can tell which machine's labels are showing.
        #[allow(clippy::type_complexity)] // a test fixture tuple, not a wire type
        let page = |params: &[(u32, &str, u8, Option<Vec<Option<String>>>)]| CompositePage {
            id: "SRC".into(),
            label: "Source".into(),
            params: params
                .iter()
                .map(|&(param_id, name, slot, ref options)| CompositeParam {
                    node_id,
                    param_id,
                    name: name.into(),
                    label: name.into(),
                    affordance: AffordanceHint::None,
                    env_group: None,
                    slot,
                    routing: None,
                    stepped: param_id == MACHINE_PID,
                    options: options.clone(),
                })
                .collect(),
            envelopes: vec![],
            macros: vec![],
        };
        let overlay = |param_id, min, max, identity| CompositeOverlay {
            param_id,
            param_name: "x".into(),
            min,
            max,
            default: min,
            identity,
        };
        let m0 = page(&[
            (MACHINE_PID, "machine", 0, None),
            (TONE_PID, "tone", 1, Some(vec![Some("kick".into()), Some("room".into())])),
            (PUNCH_PID, "punch", 2, None),
        ]);
        let m1 = page(&[
            (MACHINE_PID, "machine", 0, None),
            (TONE_PID, "tone", 1, Some(vec![Some("bell".into()), Some("sparkle".into())])),
        ]);
        CompositeView {
            engine_node_id: node_id,
            engine_name: "Host".into(),
            display_name: "Host".into(),
            pages: vec![m0.clone()],
            chain: vec![node_id],
            routes: vec![],
            variants: vec![CompositeVariantSet {
                node_id,
                select_param: Some(MACHINE_PID),
                select_param_name: Some("machine".into()),
                active: 0,
                variants: vec![
                    CompositeVariant {
                        value: 0,
                        name: "Zero".into(),
                        pages: vec![m0],
                        overlays: vec![
                            overlay(MACHINE_PID, 0.0, 1.0, true),
                            overlay(TONE_PID, 200.0, 8000.0, false),
                            overlay(PUNCH_PID, 0.0, 1.0, false),
                        ],
                    },
                    CompositeVariant {
                        value: 1,
                        name: "One".into(),
                        pages: vec![m1],
                        overlays: vec![
                            overlay(MACHINE_PID, 0.0, 1.0, true),
                            overlay(TONE_PID, 1000.0, 18000.0, false),
                        ],
                    },
                ],
            }],
        }
    }

    /// Cap-doc for the host: the **union** bank, deliberately wider than
    /// either machine's overlay (`tone` 200..18000 spans both), so a test can
    /// tell an overlay range from the descriptor's.
    fn machine_host_caps(node_id: u32) -> HashMap<u32, CapabilityDocument> {
        let mut caps = test_caps();
        let pd = |id, name: &'static str, min, max, stepped| paraclete_node_api::ParamDescriptor {
            id,
            name: name.into(),
            min,
            max,
            default: min,
            stepped,
            in_kit: name != "machine",
            unit: paraclete_node_api::ParamUnit::Generic,
            display: None,
        };
        caps.insert(
            node_id,
            CapabilityDocument {
                name: "Host".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![
                    pd(MACHINE_PID, "machine", 0.0, 1.0, true),
                    pd(TONE_PID, "tone", 200.0, 18000.0, false),
                    pd(PUNCH_PID, "punch", 0.0, 1.0, false),
                ],
                extensions: vec![],
                view: None,
            },
        );
        caps
    }

    fn machine_host_app(node_id: u32, seq_id: u32) -> TheotokosApp {
        let mut app = test_app(1, vec![seq_id], vec![node_id], vec!["Host".into()]);
        app.model.caps = machine_host_caps(node_id);
        app.model.composite = vec![Some(machine_host_view(node_id))];
        app.model.perf_page = 0;
        app
    }

    fn set_machine(bus: &BusHandle, node_id: u32, value: f64) {
        bus.borrow_mut().write(
            &format!("/node/{node_id}/param/machine"),
            paraclete_node_api::StateBusValue::Float(value),
        );
    }

    fn test_bus() -> BusHandle {
        Rc::new(RefCell::new(StateBusHandle::default()))
    }

    fn test_caps() -> HashMap<u32, CapabilityDocument> {
        let mut caps = HashMap::new();
        caps.insert(
            1,
            CapabilityDocument {
                name: "TestClock".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![],
                extensions: vec![],
                view: None,
            },
        );
        let empty_rule = Rule {
            name: "Engine".into(),
            page_groups: Cow::Borrowed(&[]),
            param_pages: Cow::Borrowed(&[]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        };
        caps.insert(
            100,
            CapabilityDocument {
                name: "Engine".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![
                    ParamDescriptor {
                        id: ParamDescriptor::id_for_name("decay"),
                        name: "decay".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        stepped: false,
                        in_kit: true,
                        unit: ParamUnit::Generic,
                        display: None,
                    },
                    ParamDescriptor {
                        id: ParamDescriptor::id_for_name("tune"),
                        name: "tune".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.0,
                        stepped: false,
                        in_kit: true,
                        unit: ParamUnit::Generic,
                        display: None,
                    },
                    // TK2 C5: a 3rd param so slot C (D13) and encoder-bank
                    // tests have something past decay/tune to resolve.
                    ParamDescriptor {
                        id: ParamDescriptor::id_for_name("width"),
                        name: "width".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        stepped: false,
                        in_kit: true,
                        unit: ParamUnit::Generic,
                        display: None,
                    },
                ],
                extensions: vec![],
                view: Some(empty_rule),
            },
        );
        caps.insert(
            200,
            CapabilityDocument {
                name: "Seq".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![],
                extensions: vec![],
                view: None,
            },
        );
        caps
    }

    fn test_app(
        clock_id: u32,
        seq_ids: Vec<u32>,
        gen_ids: Vec<u32>,
        gen_names: Vec<String>,
    ) -> TheotokosApp {
        TheotokosApp {
            model: Model::new(
                clock_id,
                &seq_ids,
                &gen_ids,
                &gen_names,
                &gen_names, // display_names: no separate fixture in unit tests
                test_caps(),
                vec![], // no composite views in unit tests
            ),
            pending: Vec::new(),
            pending_app_ops: Vec::new(),
            quit: false,
            dirty: true,
            last_render: Instant::now(),
            frame_ms: 1000,
            tuning: Tuning::default(),
            jog_a: JogTracker::new(),
            jog_b: JogTracker::new(),
            jog_c: JogTracker::new(),
            encoder_trackers: std::array::from_fn(|_| JogTracker::new()),
            tap_times: Vec::new(),
            last_debug_event: None,
            last_jog_param: None,
            keymap: Keymap::default(),
            held: HeldState::new(false),
        }
    }

    // ── BUG-053 (#152) ───────────────────────────────────────────────────

    /// `composite` is indexed by track, so a track whose view fails to
    /// assemble has to hold `None` and keep its slot. The app built the Vec
    /// with `filter_map` until #152, which dropped the failing track and
    /// shifted every later one down an index — selecting track 0 then
    /// rendered *and edited* track 1's params.
    #[test]
    fn a_track_that_fails_to_assemble_does_not_shift_the_tracks_after_it() {
        const PID: u32 = 7777;
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        app.model.composite = vec![None, Some(composite_view_with_param(101, PID))];

        app.model.select_track(1);
        assert_eq!(
            app.model
                .resolve_page_params_n(8)
                .iter()
                .map(|p| p.0)
                .collect::<Vec<_>>(),
            [101],
            "track 1's view is at index 1 and stays there"
        );
        assert_eq!(app.model.page_groups_for_active_track(), ["P1"]);

        app.model.select_track(0);
        assert!(
            app.model
                .resolve_page_params_n(8)
                .iter()
                .all(|p| p.0 != 101),
            "track 0 has no composite view of its own — it must fall back to \
             the engine-local Rule, never borrow track 1's params"
        );
        assert!(
            app.model.page_groups_for_active_track().is_empty(),
            "and it must not show track 1's page labels either"
        );
    }

    /// The #152 × MM-C6 seam: `sync_machine_selection` walks *every* track, so
    /// it is the one caller that indexes `composite` directly rather than
    /// through `active_composite()`. It must step over a hole, sync the hosts
    /// after it, and — because a switch on any track sets `changed` — must not
    /// clamp a viewless active track's page selection to 0 on the way out.
    #[test]
    fn a_machine_switch_syncs_past_a_track_with_no_composite_view() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        let mut caps = machine_host_caps(101);
        // Track 0's engine has no composite view but *does* declare pages, so
        // its page count comes from the engine-local `Rule` fallback. Without
        // that fallback in the clamp the switch below drags it back to page 0.
        caps.get_mut(&100).unwrap().view = Some(Rule {
            name: "Engine".into(),
            page_groups: Cow::Owned(vec!["TRIG".into(), "SRC".into(), "AMP".into()]),
            param_pages: Cow::Borrowed(&[]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        });
        app.model.caps = caps;
        app.model.composite = vec![None, Some(machine_host_view(101))];

        // Sit on track 0 — the hole, on its third page — while track 1's
        // machine moves.
        app.model.select_track(0);
        app.model.perf_page = 2;
        set_machine(&bus, 101, 1.0);

        assert!(
            app.model.sync_machine_selection(&bus.borrow()),
            "the host on track 1 moved, so the panel repaints"
        );
        assert_eq!(
            app.model.composite[1].as_ref().unwrap().variants[0].active,
            1,
            "the track after the hole still syncs"
        );
        assert!(
            app.model.composite[0].is_none(),
            "and the hole is still a hole"
        );
        assert_eq!(
            app.model.perf_page, 2,
            "track 0 never moved; its page selection must survive track 1's switch"
        );
    }

    // ── BUG-058 (#161) ───────────────────────────────────────────────────

    /// The contextual header draws `{display_name} — {engine_label}`. Before
    /// #161 the second half came from `TrackInfo.name`, captured once in
    /// `Model::new` from the startup cap-doc — so it kept naming the machine
    /// the node was *constructed* with while the params below it were the
    /// machine the performer had switched to.
    #[test]
    fn the_engine_label_follows_the_machine_the_track_is_on() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        assert_eq!(app.model.engine_label(0), "Zero");

        set_machine(&bus, 100, 1.0);
        assert!(app.model.sync_machine_selection(&bus.borrow()));
        assert_eq!(
            app.model.engine_label(0),
            "One",
            "the header must name the machine the track is on, not the one it \
             was built with"
        );
    }

    /// `variants` lists every machine host in the chain, engine first. A plain
    /// engine with a variant-bearing *effect* behind it must not borrow the
    /// effect's machine name for the track header — the header names the
    /// track's engine.
    #[test]
    fn a_chain_effects_machine_is_not_the_tracks_engine_label() {
        let mut app = machine_host_app(100, 200);
        {
            let cv = app.model.composite[0].as_mut().unwrap();
            // The host is a node behind the engine, not the engine itself.
            cv.engine_node_id = 999;
        }
        assert_eq!(
            app.model.engine_label(0),
            "Host",
            "falls back to the track's own engine name, not the effect's machine"
        );
    }

    /// A track with no composite view at all (#152) keeps the startup name —
    /// there is nothing better to say, and the header must not go blank.
    #[test]
    fn a_track_with_no_composite_view_keeps_its_startup_engine_name() {
        let mut app = machine_host_app(100, 200);
        app.model.composite = vec![None];
        assert_eq!(app.model.engine_label(0), "Host");
    }

    // ── MM-C6 ────────────────────────────────────────────────────────────

    fn page_param_names(app: &TheotokosApp) -> Vec<String> {
        app.model.composite[0].as_ref().unwrap().pages[0]
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect()
    }

    /// ADR-041 decision 1: selecting a machine repaints that machine's params
    /// with **zero runtime negotiation** — no cap-doc is re-queried, because
    /// MM-C5 already merged every machine's pages.
    #[test]
    fn selecting_a_machine_repaints_the_page_to_that_machines_params() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        assert_eq!(page_param_names(&app), ["machine", "tone", "punch"]);

        set_machine(&bus, 100, 1.0);
        assert!(
            app.model.sync_machine_selection(&bus.borrow()),
            "a machine change must report dirty so the panel repaints"
        );
        assert_eq!(app.model.composite[0].as_ref().unwrap().variants[0].active, 1);
        assert_eq!(
            page_param_names(&app),
            ["machine", "tone"],
            "machine 1 does not declare `punch`, so it must leave the page"
        );
    }

    /// A param the newly selected machine does not use disappears from the
    /// panel, and Theotokos writes nothing to it — the value keeps living in
    /// the union bank so it is still there on the way back. Theotokos issuing
    /// any command here is the bug: that is how a "reset inactive params on
    /// switch" would creep in surface-side.
    #[test]
    fn an_inert_param_leaves_the_page_without_being_written_to() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        set_machine(&bus, 100, 1.0);
        app.model.sync_machine_selection(&bus.borrow());

        assert!(
            !page_param_names(&app).contains(&"punch".to_string()),
            "an inert param is not displayed"
        );
        assert!(
            app.pending.is_empty(),
            "a machine switch must issue no node commands: {:?}",
            app.pending.iter().map(|c| c.type_id).collect::<Vec<_>>()
        );
        // It is still reachable on the machine that declares it.
        set_machine(&bus, 100, 0.0);
        app.model.sync_machine_selection(&bus.borrow());
        assert!(page_param_names(&app).contains(&"punch".to_string()));
    }

    /// ADR-041 §0 A1: the encoder shows and clamps against the **selected
    /// machine's** overlay, not the bank's union. The union is what storage
    /// needs and what a knob must not have — see `Model::active_overlay`.
    #[test]
    fn encoder_range_is_the_active_machines_overlay_not_the_bank_union() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);

        let tone = |app: &TheotokosApp| {
            app.model
                .resolve_encoder_params()
                .iter()
                .flatten()
                .find(|e| e.param_id == TONE_PID)
                .map(|e| (e.min, e.max))
                .expect("tone is on the page")
        };
        // The cap-doc says 200..18000 for both machines; the overlays do not.
        assert_eq!(tone(&app), (200.0, 8000.0), "machine 0's range");

        set_machine(&bus, 100, 1.0);
        app.model.sync_machine_selection(&bus.borrow());
        assert_eq!(tone(&app), (1000.0, 18000.0), "machine 1's range");
    }

    /// ADR-041 amendment 2026-08-02 (M1): a stepped param's LABELS follow
    /// the machine too, not just its range. The node-level cap-doc freezes
    /// labels at the construction-time machine; the per-machine pages carry
    /// each machine's `options`, and `sync_machine_selection` swaps whole
    /// pages — so after a switch the encoder must show the new machine's
    /// names, not the old one's. This is the defect the review found: the
    /// range narrowed while the displayed names stayed the old machine's,
    /// so value 1 read "tune" but modulated `tone`.
    #[test]
    fn stepped_param_labels_follow_a_machine_switch() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);

        let tone_options = |app: &TheotokosApp| -> Vec<String> {
            app.model
                .resolve_encoder_params()
                .iter()
                .flatten()
                .find(|e| e.param_id == TONE_PID)
                .and_then(|e| e.options.as_ref())
                .map(|o| o.iter().filter_map(|x| x.clone()).collect())
                .unwrap_or_default()
        };
        // Machine 0's labels, from its prebuilt page.
        assert_eq!(tone_options(&app), ["kick", "room"]);

        set_machine(&bus, 100, 1.0);
        assert!(
            app.model.sync_machine_selection(&bus.borrow()),
            "a machine change must report dirty so the panel repaints"
        );
        assert_eq!(
            tone_options(&app),
            ["bell", "sparkle"],
            "the encoder must name machine 1's destinations after the switch"
        );
    }

    /// A node with no variants keeps taking its range from the descriptor —
    /// every track that is not a machine host, which is most of them.
    #[test]
    fn a_node_without_variants_still_uses_its_descriptor_range() {
        let mut app = machine_host_app(100, 200);
        app.model.composite[0].as_mut().unwrap().variants.clear();
        let tone = app
            .model
            .resolve_encoder_params()
            .iter()
            .flatten()
            .find(|e| e.param_id == TONE_PID)
            .map(|e| (e.min, e.max))
            .unwrap();
        assert_eq!(tone, (200.0, 18000.0), "the cap-doc's union, unmodified");
    }

    /// ADR-041 §0 A4 / amendment 4: `machine` is identity, not a setting, so
    /// it is refused as a p-lock target — surface-side, because the sequencer
    /// holds opaque `(node_id, param_id)` pairs and cannot know.
    #[test]
    fn p_locking_the_machine_param_is_refused_with_a_message() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        app.model.lock_target = Some((0, 4));
        app.model.enc = true;

        let dirty = app.handle_keys(&bus, &[kc('q')]);

        assert!(dirty);
        assert!(
            app.pending.is_empty(),
            "no lock may be stored for an identity param: {:?}",
            app.pending.iter().map(|c| c.type_id).collect::<Vec<_>>()
        );
        let msg = app.model.cmdline_error.as_deref().unwrap_or("");
        assert!(
            msg.contains("machine"),
            "the performer must be told why, got {msg:?}"
        );
    }

    /// ADR-041 §0 A1 puts the `identity` flag on the *overlay*, so it has to
    /// be repeated in every machine's overlays. Miss one and rejection would
    /// work on one machine and not another — "p-locking machine works on
    /// HiHat but not Kick", which no ordinary test catches by accident. The
    /// check reads the union of the flags across variants so a missed repeat
    /// costs nothing here; MM-C8's assertion is what will complain about the
    /// declaration itself.
    #[test]
    fn an_identity_flag_missing_from_one_variant_still_rejects_the_lock() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        // Strip the flag from the FIRST variant only — the one a naive
        // "read the active machine's overlays" would consult.
        for o in app.model.composite[0].as_mut().unwrap().variants[0].variants[0]
            .overlays
            .iter_mut()
        {
            o.identity = false;
        }
        assert_eq!(app.model.composite[0].as_ref().unwrap().variants[0].active, 0, "on machine 0");
        assert!(
            app.model.is_identity_param(100, MACHINE_PID),
            "the flag survives on machine 1, so the param is still identity"
        );

        app.model.lock_target = Some((0, 4));
        app.model.enc = true;
        app.handle_keys(&bus, &[kc('q')]);
        assert!(
            app.pending.is_empty(),
            "rejection must not depend on which machine happens to be selected"
        );
    }

    /// The same encoder still jogs the live value when no lock target is
    /// armed — the rejection is of *locking* it, not of selecting a machine.
    #[test]
    fn the_machine_param_still_jogs_live_when_no_lock_is_armed() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        app.model.lock_target = None;
        app.model.enc = true;

        app.handle_keys(&bus, &[kc('q')]);

        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM
                    && c.arg0 == MACHINE_PID as i64),
            "selecting a machine is the whole point of the control"
        );
        assert!(app.model.cmdline_error.is_none());
    }

    /// A `machine` value naming no declared variant must not move the panel.
    /// The engines clamp such a value rather than panicking; drawing a machine
    /// that is not the one sounding would be worse than leaving it alone.
    #[test]
    fn an_unknown_machine_value_leaves_the_panel_where_it_is() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        for bad in [7.0, -1.0, f64::NAN] {
            set_machine(&bus, 100, bad);
            assert!(
                !app.model.sync_machine_selection(&bus.borrow()),
                "{bad} names no variant and must change nothing"
            );
            assert_eq!(app.model.composite[0].as_ref().unwrap().variants[0].active, 0);
            assert_eq!(page_param_names(&app), ["machine", "tone", "punch"]);
        }
    }

    /// An absent state-bus path must not read as machine 0 — that would drag
    /// the performer back to the first machine on every frame before the
    /// engine has published anything.
    #[test]
    fn a_missing_machine_path_is_not_read_as_machine_zero() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        set_machine(&bus, 100, 1.0);
        app.model.sync_machine_selection(&bus.borrow());
        assert_eq!(app.model.composite[0].as_ref().unwrap().variants[0].active, 1);

        let empty = test_bus();
        assert!(
            !app.model.sync_machine_selection(&empty.borrow()),
            "no published value means no opinion, not machine 0"
        );
        assert_eq!(app.model.composite[0].as_ref().unwrap().variants[0].active, 1);
    }

    fn kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn equals_increments_page_window() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/transport/playing".into(),
                paraclete_node_api::StateBusValue::Bool(true),
            );
            b.write(
                "/transport/bpm".into(),
                paraclete_node_api::StateBusValue::Float(140.0),
            );
            b.write(
                "/node/200/state/current_step".into(),
                paraclete_node_api::StateBusValue::Int(0),
            );
            b.write(
                "/node/200/state/pattern_length".into(),
                paraclete_node_api::StateBusValue::Int(32),
            );
            b.write(
                "/node/200/state/steps".into(),
                paraclete_node_api::StateBusValue::Text("00000000000000000000000000000000".into()),
            );
        }

        assert_eq!(app.model.page_windows[0], 0);
        app.handle_keys(&bus, &[kc('=')]);
        assert_eq!(app.model.page_windows[0], 1, "'=' must advance to page 2");
    }

    #[test]
    fn minus_clamps_at_zero() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/transport/playing".into(),
                paraclete_node_api::StateBusValue::Bool(true),
            );
            b.write(
                "/node/200/state/pattern_length".into(),
                paraclete_node_api::StateBusValue::Int(16),
            );
        }

        app.handle_keys(&bus, &[kc('-')]);
        assert_eq!(
            app.model.page_windows[0], 0,
            "'-' clamped at zero must stay 0"
        );
    }

    #[test]
    fn equals_clamps_at_page_count() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/transport/playing".into(),
                paraclete_node_api::StateBusValue::Bool(true),
            );
            b.write(
                "/node/200/state/pattern_length".into(),
                paraclete_node_api::StateBusValue::Int(16),
            );
        }

        app.model.page_windows[0] = 2;
        app.handle_keys(&bus, &[kc('=')]);
        assert_eq!(
            app.model.page_windows[0], 2,
            "'=' must not exceed page count"
        );
    }

    #[test]
    fn toggle_step_includes_page_window_offset() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/transport/playing".into(),
                paraclete_node_api::StateBusValue::Bool(true),
            );
            b.write(
                "/node/200/state/pattern_length".into(),
                paraclete_node_api::StateBusValue::Int(16),
            );
        }

        app.model.page_windows[0] = 1;
        // TK2.1 C1 (D5): default rec is now Off (a pad mode) — this test's
        // own intent is the ToggleStep offset arithmetic, which needs
        // Grid mode explicitly.
        app.model.rec = RecMode::Grid;
        // TK2 C3: the continuous grid claims 'a' as Trig9 (col 8); use
        // 'q' (Trig1, col 0) to keep this test's intent (page-window
        // offset arithmetic) independent of which column fires.
        app.handle_keys(&bus, &[kc('q')]);
        let cmd = &app.pending[0];
        assert_eq!(cmd.target_id, 200);
        assert_eq!(cmd.type_id, 16);
        assert_eq!(cmd.arg0, 16);
    }

    // TK2 C3: `select_track_publishes_selected_sequencer_id` and
    // `keymap_shift_track_toggles_mute`/`mute_toggle_reads_bus_and_flips_value`
    // tested the TK1 bare-`qweruiop` track row and Shift+track mute chord —
    // both explicitly retired at the wiring flip (§2 removed-bindings
    // list). Track select now goes through a TRK-hold + trig chord (see
    // `select_track_via_trk_chord_publishes_selected` below); the mute
    // chord moves to TRK-held + FUNC+trig in TK2 C4 (D7/A10).

    fn tab_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    #[test]
    fn select_track_via_trk_chord_publishes_selected() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        assert_eq!(app.model.tracks[1].sequencer_id, 201);

        // Tab arms TRK (sticky fallback — kitty is false in tests); 'w'
        // (Trig2) chords to select track index 1.
        app.handle_keys(&bus, &[tab_key(), kc('w')]);
        let selected = bus.borrow().read("/script/theotokos/selected").cloned();
        assert_eq!(
            selected,
            Some(paraclete_node_api::StateBusValue::Int(201)),
            "TRK-hold + Trig2 must select track 1 (seq id 201)"
        );
    }

    /// D6: "any non-trig key disarms" — the direct-utility keys (Ctrl-C,
    /// `:`, `?`, Backspace) bypass the button/hold system entirely (D14),
    /// but a sticky armed prefix must still drop when one of them fires,
    /// or the next trig would silently resolve as a track/pattern select
    /// instead of the ordinary gesture the user expects (post-C3 hostile
    /// review finding).
    #[test]
    fn direct_utility_key_disarms_sticky_prefix() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        app.handle_keys(&bus, &[tab_key()]);
        assert_eq!(app.held.armed, Some(input::Hold::Trk));

        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)]);
        assert_eq!(
            app.held.armed, None,
            "a direct utility key must disarm a sticky TRK/PTN prefix"
        );
    }

    /// TK2.1 C1 (D5): renamed from `grid_rec_off_trig_key_emits_trig_now_command`.
    #[test]
    fn pad_mode_trig_key_emits_trig_now_command() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Off;

        app.handle_keys(&bus, &[kc('q')]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 200 && c.type_id == CMD_TRIG_NOW),
            "a trig key in a pad mode (Off) must emit CMD_TRIG_NOW"
        );
    }

    /// TK2.1 C1 (D6): a pad-mode trig both selects and sounds track `col`.
    #[test]
    fn pad_press_selects_track_and_trigs_that_track() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        app.handle_keys(&bus, &[kc('w')]); // Trig2 -> col 1
        assert_eq!(
            app.model.active_track, 1,
            "a pad press must select the track under the pressed key"
        );
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 201 && c.type_id == CMD_TRIG_NOW),
            "the trig must land on the newly selected track's sequencer"
        );
    }

    /// TK2.1 C1 (D6): a pad column past the discovered track count is a
    /// silent no-op — no selection change, no command, no echo (D3: those
    /// columns get no chip either).
    #[test]
    fn pad_beyond_track_count_is_silent() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.handle_keys(&bus, &[kc('w')]); // Trig2 -> col 1, no track 1
        assert_eq!(app.model.active_track, 0, "selection must not change");
        assert!(
            app.pending.iter().all(|c| c.type_id != CMD_TRIG_NOW),
            "a column past the track count must not trig anything"
        );
        assert!(
            app.model.cmdline_error.is_none(),
            "must be silent — no echo (D6)"
        );
    }

    /// TK2.1 C1 (D5): kitty path — bare REC toggles Off <-> Grid.
    #[test]
    fn rec_toggles_off_and_grid() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.held.kitty = true;
        assert_eq!(app.model.rec, RecMode::Off);

        app.handle_keys(&bus, &[kc('z')]);
        assert_eq!(app.model.rec, RecMode::Grid, "REC must toggle Off -> Grid");

        app.handle_keys(&bus, &[kc('z')]);
        assert_eq!(app.model.rec, RecMode::Off, "REC must toggle Grid -> Off");
    }

    /// TK2.1 C1 (D5, hostile review finding): a sustained physical hold on
    /// a kitty terminal streams `KeyEventKind::Repeat` events (this app
    /// requests `REPORT_EVENT_TYPES`) — REC's own action must fire once
    /// per physical press, not once per repeat pulse, or the resulting
    /// mode depends on hold duration instead of "acts on press" (D5).
    #[test]
    fn rec_hold_repeat_events_do_not_re_toggle() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.held.kitty = true;

        app.handle_keys(&bus, &[kc('z')]); // press: Off -> Grid
        assert_eq!(app.model.rec, RecMode::Grid);

        let repeat = KeyEvent::new_with_kind(
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        );
        app.handle_keys(&bus, &[repeat, repeat, repeat]);
        assert_eq!(
            app.model.rec,
            RecMode::Grid,
            "auto-repeat pulses while REC stays physically held must not \
             re-toggle the mode"
        );
    }

    /// TK2.2 C1 (BUG-046): a held trig on a kitty terminal streams
    /// `KeyEventKind::Repeat` the same as REC (D5) — before this fix only
    /// REC's repeats were consumed, so holding a trig in Grid mode re-fired
    /// `Action::ToggleStep` once per repeat pulse and rapid-flipped the
    /// step for as long as the key stayed physically down. A step write is
    /// once-per-physical-press.
    #[test]
    fn held_trig_in_grid_toggles_step_exactly_once() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.held.kitty = true;
        app.model.rec = RecMode::Grid;

        app.handle_keys(&bus, &[kc('q')]); // press: Trig1 toggles step 0

        let repeat =
            KeyEvent::new_with_kind(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Repeat);
        app.handle_keys(&bus, &[repeat, repeat, repeat]);

        let release = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        app.handle_keys(&bus, &[release]);

        let toggles = app
            .pending
            .iter()
            .filter(|c| c.type_id == crate::action::CMD_TOGGLE_STEP)
            .count();
        assert_eq!(
            toggles, 1,
            "a held trig (Press + N*Repeat + Release) must toggle the step \
             exactly once, not once per repeat pulse"
        );
    }

    /// MM-C1 (#150, BUG-052): the encoder column is the param's **declared
    /// slot**, not its rank in the window. Before this, a node declaring
    /// slots 0, 2, 5 rendered on encoders 1, 2, 3 — the gaps closed up, and
    /// ADR-041 §0 A2's "machine-select on a TRIG slot by convention" could
    /// not mean anything.
    #[test]
    fn encoder_column_is_the_declared_slot_not_the_rank() {
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.composite = vec![Some(composite_view_with_slots(100, &[(1, 0), (2, 2), (3, 5)]))];

        let bank = app.model.resolve_encoder_params();
        let occupied: Vec<usize> = bank
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref().map(|_| i))
            .collect();
        assert_eq!(
            occupied,
            vec![0, 2, 5],
            "params declared at slots 0/2/5 must sit on those columns"
        );
        assert_eq!(bank[2].as_ref().unwrap().param_id, 2);
        assert_eq!(bank[5].as_ref().unwrap().param_id, 3);
    }

    /// A declared gap is an empty encoder, not a closed-up one — and jogging
    /// it must do nothing rather than drive whichever param was next.
    #[test]
    fn jogging_an_undeclared_column_is_a_no_op() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.composite = vec![Some(composite_view_with_slots(100, &[(1, 0), (2, 2)]))];
        app.model.enc = true;

        // Encoder 2 (column 1) is a declared gap.
        app.pending.clear();
        app.handle_keys(&bus, &[func_trig('w')]);
        assert!(
            app.pending.is_empty(),
            "an empty column must emit no command; got {:?}",
            app.pending
        );
        assert_eq!(
            app.model.cmdline_error.as_deref(),
            Some("encoder 2 unbound"),
            "and should say which column, rather than failing silently or \
             reporting some unrelated error"
        );
    }

    /// The second sub-page's window re-bases to columns 0..8, so a param at
    /// slot 9 is encoder 2 of sub-page 2 — not encoder 2 of sub-page 1.
    #[test]
    fn sub_page_two_rebases_slots_to_columns() {
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.composite = vec![Some(composite_view_with_slots(100, &[(1, 0), (2, 9)]))];

        let first = app.model.resolve_encoder_params();
        assert!(first[0].is_some(), "slot 0 on sub-page 1");
        assert!(
            first.iter().skip(1).all(|c| c.is_none()),
            "slot 9 must not appear on sub-page 1"
        );

        app.model.sub_page = 1;
        let second = app.model.resolve_encoder_params();
        assert!(second[0].is_none(), "nothing declared at slot 8");
        assert_eq!(
            second[1].as_ref().expect("slot 9 -> column 1").param_id,
            2,
            "slot 9 is column 1 of sub-page 2"
        );
    }

    /// BUG-050: on a kitty terminal the TRK arm was cleared *only* by Tab's
    /// physical release. If that release never arrived — focus taken
    /// mid-hold, protocol renegotiated — every trig kept diverting to
    /// `SelectTrack` with no way out but quitting. D6 says "Esc disarms
    /// unconditionally"; the sticky path got that from `on_press`'s
    /// catch-all, the kitty path never wired it and `on_esc` was dead code.
    ///
    /// Feeds the wedged shape directly: a Tab press with no release.
    #[test]
    fn kitty_esc_disarms_a_latched_trk_prefix() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200, 201], vec![100, 101], vec!["T1".into(), "T2".into()]);
        app.held.kitty = true;
        app.model.rec = RecMode::Grid;

        // Tab down, no release: TRK is now latched.
        let tab_press =
            KeyEvent::new_with_kind(KeyCode::Tab, KeyModifiers::NONE, KeyEventKind::Press);
        app.handle_keys(&bus, &[tab_press]);
        assert_eq!(
            app.held.armed,
            Some(input::Hold::Trk),
            "a kitty Tab press with no release must latch TRK — otherwise \
             this test is not reproducing the wedge"
        );

        let esc = KeyEvent::new_with_kind(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press);
        app.handle_keys(&bus, &[esc]);
        assert_eq!(app.held.armed, None, "Esc must disarm unconditionally (D6)");

        // And the panel is actually usable again: the next trig writes a step
        // instead of selecting a track.
        app.pending.clear();
        app.handle_keys(&bus, &[kc('w')]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == crate::action::CMD_TOGGLE_STEP),
            "after Esc a bare trig must resolve normally, not divert to \
             SelectTrack"
        );
    }

    /// BUG-050: the release that would disarm a hold is exactly what focus
    /// loss eats, so losing focus disarms too — the user should not have to
    /// discover they are latched before they can recover.
    #[test]
    fn focus_loss_disarms_a_latched_prefix() {
        let mut held = HeldState::new(true);
        held.on_kitty_press(PanelButton::Trk);
        assert_eq!(held.armed, Some(input::Hold::Trk));

        held.on_focus_lost();
        assert_eq!(held.armed, None, "focus loss must disarm");

        // The stale press must be forgotten as well, or the next press of the
        // same key would be followed by a release that disarms a *different*
        // arm than the one it belongs to.
        held.on_kitty_press(PanelButton::Trk);
        held.on_kitty_release(PanelButton::Trk);
        assert_eq!(held.armed, None, "press/release pairs stay in step");
    }

    /// TK2.2 C1 (E4): the bare trig has exactly one owner per mode. With no
    /// lock target armed, a bare trig in Grid with ENC on jogs the encoder
    /// — it must not also, as a side effect, arm a p-lock target the way
    /// the now-retired hold-to-arm gesture used to (BUG-046's root cause:
    /// the two gestures shared one physical key with no way to
    /// disambiguate them).
    #[test]
    fn bare_trig_in_grid_enc_on_jogs_and_leaves_lock_target_none() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.held.kitty = true;
        app.model.rec = RecMode::Grid;
        app.model.enc = true;

        let param_id = ParamDescriptor::id_for_name("cutoff");
        app.model
            .caps
            .get_mut(&100)
            .unwrap()
            .params
            .push(ParamDescriptor {
                id: param_id,
                name: "cutoff".into(),
                min: 20.0,
                max: 20000.0,
                default: 1000.0,
                stepped: false,
                in_kit: true,
                unit: ParamUnit::Generic,
                display: None,
            });
        app.model.composite = vec![Some(composite_view_with_param(100, param_id))];

        app.handle_keys(&bus, &[kc('q')]); // bare Trig1, ENC on

        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM),
            "ENC on must jog the live param"
        );
        assert!(
            !app.pending
                .iter()
                .any(|c| c.type_id == crate::action::CMD_TOGGLE_STEP),
            "ENC on owns the bare trig exclusively — no ToggleStep alongside the jog"
        );
        assert_eq!(
            app.model.lock_target, None,
            "a bare jog trig must not incidentally arm a p-lock target (E4: \
             one owner per mode)"
        );
    }

    /// TK2.1 C1 (D5): REC held (kitty) + PLAY escalates to Live and starts
    /// the transport.
    #[test]
    fn rec_held_plus_play_enters_live_rec() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.held.kitty = true;

        app.handle_keys(
            &bus,
            &[KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)],
        );
        app.handle_keys(
            &bus,
            &[KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)],
        );
        assert_eq!(app.model.rec, RecMode::Live);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 1 && c.type_id == CMD_CLOCK_START),
            "entering Live must start the transport"
        );
    }

    /// TK2.1 C1 (D5): REC pressed again from Live always returns to Off,
    /// regardless of kitty/fallback path.
    #[test]
    fn rec_from_live_returns_to_off() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Live;
        app.handle_keys(&bus, &[kc('z')]);
        assert_eq!(app.model.rec, RecMode::Off);
    }

    /// TK2.1 C3b (D8): entering Live sends `CMD_SET_PARAM live_rec = 1.0`
    /// to every track's sequencer — no step computation on the surface.
    #[test]
    fn entering_live_arms_live_rec_on_every_track() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        app.held.kitty = true;
        let live_rec_id = paraclete_node_api::ParamDescriptor::id_for_name("live_rec");

        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)]);
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)]);

        for seq_id in [200, 201] {
            assert!(
                app.pending.iter().any(|c| c.target_id == seq_id
                    && c.type_id == paraclete_node_api::CMD_SET_PARAM
                    && c.arg0 == live_rec_id as i64
                    && c.arg1 == 1.0),
                "entering Live must arm live_rec on sequencer {seq_id}"
            );
        }
    }

    /// TK2.1 C3b (D8): leaving Live sends `CMD_SET_PARAM live_rec = 0.0`
    /// to every track's sequencer.
    #[test]
    fn leaving_live_disarms_live_rec() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        app.model.rec = RecMode::Live;
        let live_rec_id = paraclete_node_api::ParamDescriptor::id_for_name("live_rec");

        app.handle_keys(&bus, &[kc('z')]); // bare REC: Live -> Off

        assert_eq!(app.model.rec, RecMode::Off);
        for seq_id in [200, 201] {
            assert!(
                app.pending.iter().any(|c| c.target_id == seq_id
                    && c.type_id == paraclete_node_api::CMD_SET_PARAM
                    && c.arg0 == live_rec_id as i64
                    && c.arg1 == 0.0),
                "leaving Live must disarm live_rec on sequencer {seq_id}"
            );
        }
    }

    /// TK2.1 C1 (D5): the REC/PLAY cycle's pass-through hazard — an
    /// ordinary PLAY press must never silently convert Grid into Live.
    #[test]
    fn grid_rec_survives_a_later_play_press() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Grid;
        app.handle_keys(&bus, &[kc('x')]); // bare PLAY
        assert_eq!(
            app.model.rec,
            RecMode::Grid,
            "a later PLAY press must not convert Grid into Live"
        );
    }

    /// TK2.1 C1 (D5): no-kitty fallback — REC while the transport is
    /// running arms Live directly (no chord needed). TK2.1 C3b (D8,
    /// hostile review finding): this path has no separate `EnterLiveRec`
    /// chord to arm `live_rec` for it — `ToggleRec` itself must, or live
    /// record silently does nothing on exactly the terminals (no kitty
    /// support) that need this fallback.
    #[test]
    fn fallback_rec_while_running_arms_live() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        bus.borrow_mut().write(
            "/transport/playing",
            paraclete_node_api::StateBusValue::Bool(true),
        );
        app.handle_keys(&bus, &[kc('z')]);
        assert_eq!(app.model.rec, RecMode::Live);

        let live_rec_id = paraclete_node_api::ParamDescriptor::id_for_name("live_rec");
        assert!(
            app.pending.iter().any(|c| c.target_id == 200
                && c.type_id == paraclete_node_api::CMD_SET_PARAM
                && c.arg0 == live_rec_id as i64
                && c.arg1 == 1.0),
            "the fallback entry into Live must also arm live_rec"
        );
    }

    /// TK2.1 C1 (D5): no-kitty fallback — REC while stopped arms Grid.
    #[test]
    fn fallback_rec_while_stopped_arms_grid() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.handle_keys(&bus, &[kc('z')]);
        assert_eq!(app.model.rec, RecMode::Grid);
    }

    #[test]
    fn pattern_chord_clamps_with_echo() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        // 'p' (hold) arms PTN; 'a' is Trig9 (col 8), >= PATTERN_BANK_SIZE
        // (8) — must clamp to a no-op + echo (D9), not send a command.
        app.handle_keys(&bus, &[kc('p'), kc('a')]);
        assert!(
            !app.pending.iter().any(|c| c.type_id == CMD_SET_PATTERN),
            "an out-of-range pattern chord must not emit CMD_SET_PATTERN"
        );
        assert_eq!(
            app.model.cmdline_error.as_deref(),
            Some("no pattern 9"),
            "must echo the out-of-range pattern index"
        );
    }

    // ── TK2 C4: FUNC+transport chords (D7) ──

    /// A legacy-terminal FUNC+letter chord: uppercase char AND the SHIFT
    /// flag (§0 A1) — not the synthetic lowercase+SHIFT combination A1
    /// flags as the "BUG-035 false-pass class" real terminals never send.
    fn func_key(c: char) -> KeyEvent {
        KeyEvent::new(
            KeyCode::Char(c.to_ascii_uppercase()),
            KeyModifiers::SHIFT,
        )
    }

    #[test]
    fn func_rec_copies_active_lane() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/node/200/state/steps",
                StateBusValue::Text("1100000000000000".into()),
            );
            b.write("/node/200/state/locks", StateBusValue::Text(String::new()));
        }

        // FUNC+REC ('z' + SHIFT) copies the active track's active lane.
        app.handle_keys(&bus, &[func_key('z')]);
        assert!(
            !app.model.yank_buffer.is_empty(),
            "FUNC+REC must populate the yank buffer"
        );
    }

    #[test]
    fn func_stop_pastes() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/node/200/state/steps",
                StateBusValue::Text("1100000000000000".into()),
            );
            b.write("/node/200/state/locks", StateBusValue::Text(String::new()));
        }

        app.handle_keys(&bus, &[func_key('z')]); // FUNC+REC copies first.
        app.pending.clear();
        app.handle_keys(&bus, &[func_key('c')]); // FUNC+STOP pastes.
        assert!(
            !app.pending.is_empty(),
            "FUNC+STOP must produce paste commands"
        );
    }

    /// ADR-046 T5: bare STOP is halt-in-place then rewind — two commands,
    /// STOP before REWIND (the ordering the phase spec's decomposition
    /// hazard note relies on: a stop-then-rewind pair applied to a running
    /// InternalClock must land as "halted at the window start", not
    /// "rewound then immediately restarted").
    #[test]
    fn bare_stop_emits_stop_then_rewind_in_order() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        app.handle_keys(&bus, &[kc('c')]);

        assert_eq!(
            app.pending.len(),
            2,
            "bare STOP must emit exactly two commands; got: {:?}",
            app.pending
        );
        assert_eq!(
            app.pending[0].type_id, CMD_CLOCK_STOP,
            "STOP must be emitted first"
        );
        assert_eq!(
            app.pending[1].type_id, CMD_CLOCK_REWIND,
            "REWIND must be emitted second"
        );
        assert_eq!(app.pending[0].target_id, app.model.clock_id);
        assert_eq!(app.pending[1].target_id, app.model.clock_id);
    }

    #[test]
    fn func_play_clears_lane_and_locks() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write("/node/200/state/pattern_length", StateBusValue::Int(4));
        }

        // FUNC+PLAY ('x' + SHIFT) clears the active track's pattern: §0
        // A8 — CMD_CLEAR (steps only) plus a CMD_CLEAR_STEP_LOCK per step.
        app.handle_keys(&bus, &[func_key('x')]);
        assert_eq!(app.pending[0].type_id, CMD_CLEAR);
        let lock_clears = app
            .pending
            .iter()
            .filter(|c| c.type_id == CMD_CLEAR_STEP_LOCK)
            .count();
        assert_eq!(
            lock_clears, 4,
            "must clear locks for every step of the pattern length"
        );
    }

    /// A12 (normative): FUNC+Space must stay a no-op — `key_to_button`
    /// collapses Space and `x` onto the same `PanelButton::Play` (D11),
    /// so without an explicit guard FUNC+Space silently wiped the active
    /// pattern the same as FUNC+`x` (post-C4 hostile review blocker).
    #[test]
    fn func_space_does_not_clear_lane() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write("/node/200/state/pattern_length", StateBusValue::Int(4));
        }

        app.handle_keys(
            &bus,
            &[KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT)],
        );
        assert!(
            app.pending.is_empty(),
            "FUNC+Space must not clear the pattern; got {:?}",
            app.pending
        );
    }

    // ── TK2 C5: encoder bank (D8) ──

    /// A legacy-terminal FUNC+top-row-trig chord (§0 A1's uppercase+SHIFT
    /// shape) — resolves to encoder jog (`col < 8` in the top row).
    fn func_trig(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c.to_ascii_uppercase()), KeyModifiers::SHIFT)
    }

    /// FUNC+Ctrl+trig: fine magnitude (D8).
    fn func_fine_trig(c: char) -> KeyEvent {
        KeyEvent::new(
            KeyCode::Char(c.to_ascii_uppercase()),
            KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        )
    }

    /// A capability doc with a real, populated `Rule` whose `param_pages`
    /// slot assignment is the REVERSE of param declaration order — proves
    /// encoder resolution follows the Rule's `slot` field, not incidental
    /// `Vec` position. (`test_caps()`'s shared `empty_rule` has no
    /// `page_groups`, so every test using it hits the plain
    /// cap.params-declaration-order fallback, never this sort — review
    /// finding, post-C5 hostile review.)
    fn rule_ordered_caps() -> HashMap<u32, CapabilityDocument> {
        let decay_id = ParamDescriptor::id_for_name("decay");
        let tune_id = ParamDescriptor::id_for_name("tune");
        let rule = Rule {
            name: "Engine".into(),
            // Not "TRIG" on purpose: this fixture is about Rule-slot ordering
            // vs rank, and the TRIG page now carries the four virtual per-step
            // params (TK3 C0 #180) that would fill the slot-1 gap this test
            // asserts is empty. A non-TRIG page keeps the gap semantics intact.
            page_groups: Cow::Owned(vec![Cow::Borrowed("AMP")]),
            param_pages: Cow::Owned(vec![
                // Declared decay-then-tune below, but Rule order is
                // tune (slot 0) then decay (slot 2) — reversed, AND sparse.
                // MM-C1: the gap at slot 1 is what distinguishes placement
                // from rank; with slots 0/1 both conventions agree.
                (tune_id, PageRef {
                    page: Cow::Borrowed("AMP"),
                    slot: 0,
                }),
                (decay_id, PageRef {
                    page: Cow::Borrowed("AMP"),
                    slot: 2,
                }),
            ]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        };
        let mut caps = HashMap::new();
        caps.insert(
            100,
            CapabilityDocument {
                name: "Engine".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![
                    ParamDescriptor {
                        id: decay_id,
                        name: "decay".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.5,
                        stepped: false,
                        in_kit: true,
                        unit: ParamUnit::Generic,
                        display: None,
                    },
                    ParamDescriptor {
                        id: tune_id,
                        name: "tune".into(),
                        min: 0.0,
                        max: 1.0,
                        default: 0.0,
                        stepped: false,
                        in_kit: true,
                        unit: ParamUnit::Generic,
                        display: None,
                    },
                ],
                extensions: vec![],
                view: Some(rule),
            },
        );
        caps.insert(
            200,
            CapabilityDocument {
                name: "Seq".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![],
                extensions: vec![],
                view: None,
            },
        );
        caps
    }

    #[test]
    fn encoder_col_maps_to_page_param_in_rule_order() {
        let bus = test_bus();
        let mut app = TheotokosApp {
            model: Model::new(1, &[200], &[100], &["T1".into()], &["T1".into()], rule_ordered_caps(), vec![]),
            pending: Vec::new(),
            pending_app_ops: Vec::new(),
            quit: false,
            dirty: true,
            last_render: Instant::now(),
            frame_ms: 1000,
            tuning: Tuning::default(),
            jog_a: JogTracker::new(),
            jog_b: JogTracker::new(),
            jog_c: JogTracker::new(),
            encoder_trackers: std::array::from_fn(|_| JogTracker::new()),
            tap_times: Vec::new(),
            last_debug_event: None,
            last_jog_param: None,
            keymap: Keymap::default(),
            held: HeldState::new(false),
        };
        let decay_id = ParamDescriptor::id_for_name("decay");
        let tune_id = ParamDescriptor::id_for_name("tune");

        // The Rule assigns tune to slot 0 and decay to slot 1 — the
        // REVERSE of their declaration order — so encoder 1 (col 0, 'q')
        // must resolve to tune, not decay, proving the slot sort runs.
        app.handle_keys(&bus, &[func_trig('q')]);
        assert!(
            app.pending.iter().any(|c| c.type_id
                == paraclete_node_api::CMD_BUMP_PARAM
                && c.arg0 == tune_id as i64),
            "encoder 1 must target the Rule's slot-0 param (tune), not decay"
        );

        // MM-C1: slot 1 is a declared gap. Under the old rank-based
        // placement this column held decay (the second entry after the slot
        // sort); it must now be unbound.
        app.pending.clear();
        app.handle_keys(&bus, &[func_trig('w')]);
        assert!(
            app.pending.is_empty(),
            "encoder 2 is a declared gap and must bind nothing; got {:?}",
            app.pending
        );

        app.pending.clear();
        app.handle_keys(&bus, &[func_trig('e')]);
        assert!(
            app.pending.iter().any(|c| c.type_id
                == paraclete_node_api::CMD_BUMP_PARAM
                && c.arg0 == decay_id as i64),
            "encoder 3 must target the Rule's slot-2 param (decay)"
        );
    }

    // ── TK3 C0 (#180): TRIG page virtual per-step params ────────────────

    /// A cap whose single TRIG page names only `machine` at slot 0 — the
    /// shape a real engine host produces — so the four virtual per-step
    /// params (slots 1–4) resolve via the cap-doc path (no composite view
    /// in these tests) to `EncoderTarget::VirtualStep`.
    fn trig_virtual_caps() -> HashMap<u32, CapabilityDocument> {
        let machine_id = ParamDescriptor::id_for_name("machine");
        let rule = Rule {
            name: "Engine".into(),
            page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG")]),
            param_pages: Cow::Owned(vec![(
                machine_id,
                PageRef {
                    page: Cow::Borrowed("TRIG"),
                    slot: 0,
                },
            )]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        };
        let mut caps = HashMap::new();
        caps.insert(
            100,
            CapabilityDocument {
                name: "Engine".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![ParamDescriptor {
                    id: machine_id,
                    name: "machine".into(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    stepped: true,
                    in_kit: false,
                    unit: ParamUnit::Generic,
                    display: None,
                }],
                extensions: vec![],
                view: Some(rule),
            },
        );
        caps.insert(
            200,
            CapabilityDocument {
                name: "Seq".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![],
                extensions: vec![],
                view: None,
            },
        );
        caps
    }

    fn write_step_detail(bus: &BusHandle, seq_id: u32, steps: &[(u8, u8)]) {
        // `steps`: (step, fill) — build a default 8-step blob with the given
        // steps carrying a non-default condition (prob 75, repeat 1/2).
        let mut detail = String::new();
        for i in 0..8 {
            if i > 0 {
                detail.push(';');
            }
            if let Some((_, fill)) = steps.iter().find(|(s, _)| *s == i as u8) {
                detail.push_str(&format!("32768,0.75,0,75,1,2,{fill}"));
            } else {
                detail.push_str("32768,0.75,0,100,0,0,0");
            }
        }
        bus.borrow_mut().write(
            &format!("/node/{seq_id}/state/step_detail"),
            paraclete_node_api::StateBusValue::Text(detail),
        );
    }

    #[test]
    fn trig_page_resolves_virtual_step_params() {
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.caps = trig_virtual_caps();
        let bank = app.model.resolve_encoder_params();

        assert!(
            matches!(bank[0], Some(ref p) if p.target == EncoderTarget::Real),
            "machine at slot 0 must stay a Real param"
        );
        let kinds = [
            StepParamKind::Velocity,
            StepParamKind::Length,
            StepParamKind::Timing,
            StepParamKind::Condition,
        ];
        for (i, kind) in kinds.iter().enumerate() {
            let col = i + 1;
            match &bank[col] {
                Some(p) => assert!(
                    matches!(p.target, EncoderTarget::VirtualStep { seq_id, kind: k }
                        if seq_id == 200 && k == *kind),
                    "col {col}: expected VirtualStep {kind:?} to seq 200, got {:?}",
                    p.target
                ),
                None => panic!("col {col} should hold a virtual param"),
            }
        }
        assert!(
            bank[5].is_none() && bank[6].is_none() && bank[7].is_none(),
            "slots 5–7 stay empty on TRIG"
        );
    }

    #[test]
    fn non_trig_page_has_no_virtual_params() {
        // test_caps' engine has an empty rule (page_groups empty) -> position
        // fallback; the TRIG gate must not fire.
        let app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        let bank = app.model.resolve_encoder_params();
        assert!(
            bank.iter().flatten().all(|p| p.target == EncoderTarget::Real),
            "no VirtualStep may leak to a non-TRIG page"
        );
    }

    #[test]
    fn encoder_jog_on_virtual_velocity_emits_set_step_velocity() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.caps = trig_virtual_caps();
        // Focused step = current playing step (no lock target armed).
        bus.borrow_mut().write(
            "/node/200/state/current_step",
            paraclete_node_api::StateBusValue::Int(2),
        );
        write_step_detail(&bus, 200, &[(2, 1)]);

        app.handle_keys(&bus, &[func_trig('w')]); // col 1 = velocity
        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == CMD_SET_STEP_VELOCITY && c.target_id == 200 && c.arg0 == 2),
            "must emit CMD_SET_STEP_VELOCITY to the focused step; got {:?}",
            app.pending
        );
        assert!(
            !app.pending
                .iter()
                .any(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM),
            "a virtual step jog must not be a bank bump"
        );
    }

    #[test]
    fn encoder_jog_on_condition_read_modify_write() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.caps = trig_virtual_caps();
        bus.borrow_mut().write(
            "/node/200/state/current_step",
            paraclete_node_api::StateBusValue::Int(4),
        );
        // Step 4: prob 75, repeat 1/2, fill 1 (FillA).
        write_step_detail(&bus, 200, &[(4, 1)]);

        app.handle_keys(&bus, &[func_trig('t')]); // col 4 = condition
        let cond = app
            .pending
            .iter()
            .find(|c| c.type_id == CMD_SET_STEP_CONDITION)
            .unwrap_or_else(|| panic!("no condition command; got {:?}", app.pending));
        assert_eq!(cond.target_id, 200);
        assert_eq!(cond.arg0, 4, "targets the focused step");
        let enc = cond.arg1 as i64 as u64;
        assert!(((enc >> 24) & 0xFF) as u8 != 1, "fill must change");
        assert_eq!((enc & 0xFF) as u8, 75, "probability preserved");
        assert_eq!(((enc >> 8) & 0xFF) as u8, 1, "repeat_n preserved");
        assert_eq!(((enc >> 16) & 0xFF) as u8, 2, "repeat_m preserved");
    }

    // ── TK3 C1 (#163): build_render_data is a testable free function ────

    /// Drive `build_render_data` directly — the seam #163 exists to give.
    fn render_data(bus: &BusHandle, mut model: Model, held: &HeldState) -> render::RenderData {
        let b = bus.borrow();
        build_render_data(
            &mut model,
            &*b,
            held,
            &Keymap::default(),
            &Tuning::default(),
            None,
            None,
        )
    }

    fn render_model() -> Model {
        Model::new(
            1,
            &[200],
            &[100],
            &["T1".to_string()],
            &["T1".to_string()],
            test_caps(),
            vec![],
        )
    }

    #[test]
    fn build_render_data_default_state() {
        let bus = test_bus();
        let held = HeldState::new(false);
        let data = render_data(&bus, render_model(), &held);
        assert_eq!(data.screen, Screen::Grid);
        assert_eq!(data.bpm, 120.0, "no tempo on the bus -> default 120");
        assert!(data.armed_prefix.is_none(), "nothing armed");
        assert_eq!(data.mute_states, vec![false]);
        assert!(data.kit_list.is_empty());
    }

    #[test]
    fn build_render_data_param_page_carries_encoder_cells() {
        let bus = test_bus();
        let held = HeldState::new(false);
        // test_caps' engine (empty rule) resolves positionally to its
        // declared params: decay, tune, width at cols 0..2.
        let data = render_data(&bus, render_model(), &held);
        assert_eq!(data.encoder_cells.len(), 8);
        assert_eq!(data.encoder_cells[0].as_ref().unwrap().name, "decay");
        assert_eq!(data.encoder_cells[1].as_ref().unwrap().name, "tune");
        assert!(data.encoder_cells[3..].iter().all(|c| c.is_none()));
    }

    #[test]
    fn build_render_data_reads_mute_states() {
        let bus = test_bus();
        let held = HeldState::new(false);
        bus.borrow_mut().write(
            "/node/200/param/mute",
            paraclete_node_api::StateBusValue::Float(1.0),
        );
        let data = render_data(&bus, render_model(), &held);
        assert_eq!(data.mute_states, vec![true]);
    }

    #[test]
    fn build_render_data_armed_prefix() {
        let bus = test_bus();
        let mut held = HeldState::new(false);
        let data = render_data(&bus, render_model(), &held);
        assert!(data.armed_prefix.is_none());
        held.armed = Some(input::Hold::Trk);
        let data = render_data(&bus, render_model(), &held);
        assert_eq!(data.armed_prefix.as_deref(), Some("TRK…"));
    }

    #[test]
    fn build_render_data_kit_screen_state() {
        let bus = test_bus();
        let held = HeldState::new(false);
        bus.borrow_mut().write(
            "/context/kits",
            paraclete_node_api::StateBusValue::Text("0:MyKit;1:Other".into()),
        );
        bus.borrow_mut().write(
            "/context/kit_binding",
            paraclete_node_api::StateBusValue::Text("0:0".into()),
        );
        // Active pattern 0 -> kit_loaded_slot = slot 0 -> kit 0.
        bus.borrow_mut().write(
            "/node/200/state/active_pattern",
            paraclete_node_api::StateBusValue::Int(0),
        );
        let mut model = render_model();
        model.screen = Screen::Kit;
        let data = render_data(&bus, model, &held);
        assert_eq!(data.screen, Screen::Kit);
        assert_eq!(data.kit_list.len(), 2);
        assert_eq!(data.kit_loaded_slot, Some(0));
    }

    // ── TK3 C2: MIX screen ──────────────────────────────────────────────

    #[test]
    fn func8_opens_mix_bare8_opens_settings() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        // Bare 8 -> Settings.
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char('8'), KeyModifiers::NONE)]);
        assert_eq!(app.model.screen, Screen::Settings);
        // FUNC+8 -> MIX (the new chord), not Settings.
        app.model.screen = Screen::Grid;
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char('8'), KeyModifiers::SHIFT)]);
        assert_eq!(app.model.screen, Screen::Mix);
    }

    #[test]
    fn mix_screen_encoder_jog_bumps_mixnode_gains() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Mix;
        // Col 0 = track 0 = input_gain_0 on the MixNode.
        app.handle_keys(&bus, &[func_trig('q')]);
        let pid = paraclete_node_api::ParamDescriptor::id_for_name("input_gain_0");
        assert!(
            app.pending.iter().any(|c| {
                c.type_id == paraclete_node_api::CMD_BUMP_PARAM
                    && c.target_id == MIX_NODE_ID
                    && c.arg0 == pid as i64
            }),
            "MIX col 0 must bump input_gain_0 on MixNode; got {:?}",
            app.pending
        );
        app.pending.clear();
        // Col 7 = master_gain.
        app.handle_keys(&bus, &[func_trig('i')]);
        let mpid = paraclete_node_api::ParamDescriptor::id_for_name("master_gain");
        assert!(
            app.pending.iter().any(|c| {
                c.type_id == paraclete_node_api::CMD_BUMP_PARAM
                    && c.target_id == MIX_NODE_ID
                    && c.arg0 == mpid as i64
            }),
            "MIX col 7 must bump master_gain; got {:?}",
            app.pending
        );
    }

    #[test]
    fn build_render_data_populates_mix_gains() {
        let bus = test_bus();
        bus.borrow_mut().write(
            "/node/2/param/input_gain_0",
            paraclete_node_api::StateBusValue::Float(1.25),
        );
        bus.borrow_mut().write(
            "/node/2/param/master_gain",
            paraclete_node_api::StateBusValue::Float(0.75),
        );
        let held = HeldState::new(false);
        let data = render_data(&bus, render_model(), &held);
        assert_eq!(data.mix_gains, vec![1.25]);
        assert_eq!(data.mix_master, 0.75);
    }

    // ── TK3 C4 (OQ-T4): step-size scaling ───────────────────────────────

    #[test]
    fn set_step_size_tier_dispatch_clamps() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.enc = true;
        app.model.step_size_tier = 3;
        // +1 -> 4 (cap).
        app.handle_keys(&bus, &[func_fine_trig('q')]);
        assert_eq!(app.model.step_size_tier, 4);
        // +1 at cap -> stays 4.
        app.handle_keys(&bus, &[func_fine_trig('q')]);
        assert_eq!(app.model.step_size_tier, 4);
        // -1 -> 3.
        app.handle_keys(&bus, &[func_fine_trig('a')]);
        assert_eq!(app.model.step_size_tier, 3);
        // floor: set to 0, -1 stays 0.
        app.model.step_size_tier = 0;
        app.handle_keys(&bus, &[func_fine_trig('a')]);
        assert_eq!(app.model.step_size_tier, 0);
    }

    /// Tier 0: jog step is the base. Tier 2: the base is ×4. Measured on a
    /// real (continuous) param's emitted bump delta — the tier scales the
    /// base before the acceleration ramp.
    #[test]
    fn step_size_tier_scales_base_jog_step() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        // test_caps engine (empty rule) -> decay on col 0, range 0..1.
        app.handle_keys(&bus, &[func_trig('q')]);
        let base = app
            .pending
            .iter()
            .find(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM)
            .expect("jog must bump a param")
            .arg1;
        assert!(
            (base - 1.0 / 128.0).abs() < 1e-9,
            "tier 0 base for a range-1 param = 1/128, got {base}"
        );

        app.pending.clear();
        app.model.step_size_tier = 2; // ×4
        app.handle_keys(&bus, &[func_trig('q')]);
        let scaled = app
            .pending
            .iter()
            .find(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM)
            .expect("jog must bump a param")
            .arg1;
        assert!(
            (scaled - base * 4.0).abs() < 1e-9,
            "tier 2 must be 4× the tier-0 base: base={base} scaled={scaled}"
        );
    }

    // ── TK3 C5 (OQ-T23b): dual-path tap tempo ───────────────────────────

    #[test]
    fn func_space_is_global_tap_tempo_on_any_screen() {
        let bus = test_bus();
        // Grid screen.
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT)]);
        assert!(
            app.pending.iter().all(|c| c.type_id != CMD_CLEAR),
            "FUNC+Space must NOT clear the lane (A12 preserved); got {:?}",
            app.pending
        );
        assert!(
            app.last_debug_event.as_deref().unwrap_or("").contains("TapTempo"),
            "FUNC+Space on Grid must resolve to TapTempo; debug={:?}",
            app.last_debug_event
        );

        // Param screen.
        let mut app2 = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app2.model.screen = Screen::Param(0);
        app2.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT)]);
        assert!(
            app2.last_debug_event.as_deref().unwrap_or("").contains("TapTempo"),
            "FUNC+Space on a Param screen must resolve to TapTempo; debug={:?}",
            app2.last_debug_event
        );
    }

    #[test]
    fn bare_space_still_toggles_transport() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == CMD_CLOCK_START || c.type_id == CMD_CLOCK_STOP),
            "bare Space must toggle the transport; got {:?}",
            app.pending
        );
        assert!(
            !app.last_debug_event.as_deref().unwrap_or("").contains("TapTempo"),
            "bare Space must NOT be tap tempo"
        );
    }

    #[test]
    fn tempo_screen_encoder_jog_nudges_bpm() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Tempo;
        let bpm_id = paraclete_node_api::ParamDescriptor::id_for_name("bpm");
        app.handle_keys(&bus, &[func_trig('q')]); // encoder col 0 on Tempo
        assert!(
            app.pending.iter().any(|c| {
                c.type_id == paraclete_node_api::CMD_SET_PARAM
                    && c.arg0 == bpm_id as i64
                    && c.target_id == app.model.clock_id
            }),
            "Tempo encoder col 1 must emit a bpm set; got {:?}",
            app.pending
        );
    }

    #[test]
    fn encoder_beyond_param_count_echoes_noop() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        // test_caps' page has 3 params (decay/tune/width); encoder 4
        // (col 3, 'r') is past the count.
        app.handle_keys(&bus, &[func_trig('r')]);
        assert!(
            app.pending.is_empty(),
            "must not emit a command past the page's param count"
        );
        assert_eq!(
            app.model.cmdline_error.as_deref(),
            // MM-C1: the encoder exists, nothing is bound to it — which is
            // what a column past the param count now is, the same as a
            // declared gap.
            Some("encoder 4 unbound"),
            "must echo the unbound encoder index"
        );
    }

    #[test]
    fn encoder_jog_emits_bump_param() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        app.handle_keys(&bus, &[func_trig('q')]);
        assert_eq!(app.pending.len(), 1, "without step focus, jog is a bump");
        assert_eq!(app.pending[0].type_id, paraclete_node_api::CMD_BUMP_PARAM);
        assert_eq!(app.pending[0].target_id, 100);
    }

    #[test]
    fn encoder_fine_scales_step() {
        let bus = test_bus();
        let mut app_normal = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app_normal.handle_keys(&bus, &[func_trig('q')]);
        let normal_delta = app_normal.pending[0].arg1.abs();

        let mut app_fine = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app_fine.handle_keys(&bus, &[func_fine_trig('q')]);
        let fine_delta = app_fine.pending[0].arg1.abs();

        assert!(
            fine_delta < normal_delta,
            "FUNC+Ctrl must scale the step down: fine={fine_delta}, normal={normal_delta}"
        );
    }

    #[test]
    fn encoder_jog_routes_to_lock_when_step_focused() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.lock_target = Some((0, 3));

        app.handle_keys(&bus, &[func_trig('q')]);
        assert_eq!(app.pending.len(), 2, "focused jog must emit target + lock");
        assert_eq!(app.pending[0].type_id, CMD_SET_LOCK_TARGET);
        assert_eq!(app.pending[1].type_id, CMD_SET_STEP_LOCK);
        assert_eq!(app.pending[1].arg0, 3, "step arg must be the focused step");
    }

    /// A second jog on the same locked step must build on the value already
    /// stored there, not restart from the live param — otherwise a lock can
    /// never be moved further than one increment from the live value.
    ///
    /// The accumulation is not local: it round-trips through the sequencer's
    /// published `/node/{seq}/state/locks`. This feeds that string back in the
    /// format `Sequencer::published_state` writes today
    /// (`"s{step}:{nid}:{pid}={:.6}"`, `sequencer.rs:1564`).
    ///
    /// **This catches reader-side drift only.** The literal is hardcoded here,
    /// so a change to the *writer's* format leaves this test green while
    /// jogging silently reverts to "always one increment from the live value"
    /// — and takes the trig strip's lock dots (`Model::read_step_locks`) with
    /// it. `the_published_lock_string_has_the_shape_readers_parse`
    /// (`sequencer.rs`) is the writer-side half; the two must be changed
    /// together. They cannot be one test: `paraclete-theotokos` does not
    /// depend on `paraclete-nodes` and must not start (L3 is not a dependency
    /// of a surface crate).
    ///
    /// `encoder_jog_routes_to_lock_when_step_focused` above covers the command
    /// *pair*; it asserts nothing about `arg1`, which is what this adds.
    #[test]
    fn a_second_jog_accumulates_from_the_stored_lock_value() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.lock_target = Some((0, 3));

        app.handle_keys(&bus, &[func_trig('q')]);
        let v1 = app.pending[1].arg1;
        let node_id = app.pending[0].arg0 as u32;
        let param_id = app.pending[0].arg1 as u32;
        app.pending.clear();

        // Exactly `Sequencer::published_state`'s format: "s{step}:{nid}:{pid}={:.6}"
        bus.borrow_mut().write(
            "/node/200/state/locks",
            paraclete_node_api::StateBusValue::Text(format!("s3:{node_id}:{param_id}=0.250000")),
        );

        app.handle_keys(&bus, &[func_trig('q')]);
        let v2 = app.pending[1].arg1;

        // The exact value, not just a floor: `> 0.25` alone would also pass if
        // the jog read the lock and then applied its delta twice, or re-clamped
        // against the wrong range.
        assert!(
            (v2 - (0.25 + v1)).abs() < 1e-9,
            "second jog must be the stored lock 0.25 plus one increment — \
             v1={v1}, v2={v2}, expected {}, node={node_id}, param={param_id}",
            0.25 + v1
        );
        // And pin the no-lock fallback the first jog took, so a change to the
        // jog step shows up here rather than only in the sum above.
        assert!(
            (v1 - 1.0 / 128.0).abs() < 1e-9,
            "first jog, with nothing on the bus, must be one base step from the \
             live value (range 1.0 / base_divisor 128) — got {v1}"
        );
    }

    /// TK2.2 C4 (E5): a resolved jog records the name of the param it
    /// moved, regardless of whether it landed as a lock or a live bump —
    /// `render_status_line` decides that framing separately, from the
    /// current lock target, not from anything stored here.
    #[test]
    fn jog_records_the_last_jogged_param_name() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        assert!(app.last_jog_param.is_none(), "sanity: no jog yet");

        let expected_name = app.model.resolve_encoder_params()[0]
            .as_ref()
            .expect("encoder 0 is populated")
            .name
            .clone();
        app.handle_keys(&bus, &[func_trig('q')]);

        assert_eq!(
            app.last_jog_param.as_deref(),
            Some(expected_name.as_str()),
            "a resolved jog must record the name of the param it moved"
        );
    }

    /// TK2.2 C4 (E5): clearing the lock target must NOT clear the
    /// recorded jog — the panel keeps naming the last-jogged param and
    /// only its lock/live framing changes (`render_status_line` derives
    /// that from the current target each frame). This is the mechanism
    /// behind "the indicator clears with the target": the *step naming*
    /// clears because the target is gone, not because this field was
    /// reset.
    #[test]
    fn jog_record_survives_lock_target_clearing() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Grid;
        app.model.lock_target = Some((0, 0));

        app.handle_keys(&bus, &[func_trig('q')]);
        assert!(
            app.last_jog_param.is_some(),
            "sanity: the jog must have recorded a param name"
        );
        let recorded = app.last_jog_param.clone();

        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)]);
        assert_eq!(
            app.model.lock_target, None,
            "sanity: Esc must have cleared the target"
        );
        assert_eq!(
            app.last_jog_param, recorded,
            "clearing the lock target must not clear the recorded jog"
        );
    }

    /// BUG-064 item 1: jogging an identity param (`machine`) with a lock
    /// target armed must NOT set `last_jog_param`, so the status line does
    /// not advertise a lock destination that will be refused (#171).
    #[test]
    fn jog_on_identity_param_does_not_record_last_jog_param() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        app.model.rec = RecMode::Grid;
        // machine_host_app puts `machine` at encoder column 0
        app.model.lock_target = Some((0, 4));

        assert!(
            app.model.is_identity_param(100, MACHINE_PID),
            "sanity: machine must be flagged identity"
        );
        assert!(
            app.model.lock_step_for_active_track().is_some(),
            "sanity: lock target must be armed"
        );

        app.handle_keys(&bus, &[func_trig('q')]); // encoder 0 → machine

        assert!(
            app.last_jog_param.is_none(),
            "identity-param jog must not record last_jog_param"
        );
        assert!(
            app.model.cmdline_error.is_some(),
            "identity-param refusal must set a cmdline error"
        );
    }

    /// BUG-064 item 2: jogging a param that is already at its max (or min)
    /// must set `cmdline_error` rather than silently ignoring the press,
    /// so the performer can tell "end of range" from "dropped keypress" (#171).
    #[test]
    fn jog_at_range_end_sets_cmdline_error() {
        let bus = test_bus();
        let mut app = machine_host_app(100, 200);
        // machine_host_app puts `machine` (range 0..1) at encoder column 0
        set_machine(&bus, 100, 1.0); // at max
        app.model.sync_machine_selection(&bus.borrow());

        // Jog encoder 0 (machine) forward — already at max
        app.handle_keys(&bus, &[func_trig('q')]);

        assert!(
            app.model.cmdline_error.is_some(),
            "encoder at max must set a cmdline error"
        );
        assert!(
            app.last_jog_param.is_none(),
            "refused jog must not record last_jog_param"
        );
    }

    /// BUG-064 item 3: arming Lock target outside Grid mode must surface
    /// a refusal — the arm is consumed but nothing was targeted (#171).
    #[test]
    fn lock_armed_outside_grid_sets_cmdline_error() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Off; // not Grid

        // Press Lock, then a trig — same gesture as Grid mode, different rec.
        let dirty = app.handle_keys(&bus, &[kc('m'), kc('q')]);
        assert!(dirty, "the refusal must flag dirty");

        assert!(
            app.model.cmdline_error.is_some(),
            "lock target refused must set a cmdline error: {:?}",
            app.model.cmdline_error
        );
        assert!(
            app.model.lock_target.is_none(),
            "lock target must NOT be set outside Grid mode"
        );
    }

    /// TK2.1 C5b (D15): named to match the phase spec's literal test list.
    #[test]
    fn jog_with_lock_target_emits_lock_pair_not_bump() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.lock_target = Some((0, 5));

        app.handle_keys(&bus, &[func_trig('q')]);
        assert!(
            app.pending.iter().any(|c| c.type_id == CMD_SET_LOCK_TARGET),
            "a set lock target must route the jog to CMD_SET_LOCK_TARGET"
        );
        assert!(
            app.pending.iter().any(|c| c.type_id == CMD_SET_STEP_LOCK),
            "a set lock target must route the jog to CMD_SET_STEP_LOCK"
        );
        assert!(
            !app.pending
                .iter()
                .any(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM),
            "must not also bump the live bank"
        );
    }

    /// TK2.1 C5b (D15): named to match the phase spec's literal test list.
    #[test]
    fn jog_without_lock_target_emits_bump() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        assert!(app.model.lock_target.is_none());

        app.handle_keys(&bus, &[func_trig('q')]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM),
            "with no lock target, the jog must bump the live bank"
        );
        assert!(
            !app.pending.iter().any(|c| c.type_id == CMD_SET_STEP_LOCK),
            "must not emit a step lock with no target set"
        );
    }

    /// TK2.1 C5b (D15): the latched path (`m` arms, a Grid-mode trig
    /// sets), a re-tap clears, and Esc also clears — end to end through
    /// `handle_keys`.
    #[test]
    fn lock_target_clears_on_esc_and_on_retap() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Grid;

        app.handle_keys(&bus, &[kc('m')]); // Lock: arm
        app.handle_keys(&bus, &[kc('q')]); // Trig1: sets (0, 0)
        assert_eq!(app.model.lock_target, Some((0, 0)));

        app.handle_keys(&bus, &[kc('q')]); // retap: clears
        assert_eq!(app.model.lock_target, None, "retapping the locked step must clear it");

        app.handle_keys(&bus, &[kc('m')]);
        app.handle_keys(&bus, &[kc('q')]);
        assert_eq!(app.model.lock_target, Some((0, 0)));

        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)]);
        assert_eq!(app.model.lock_target, None, "Esc must clear an already-set target");
    }

    /// TK2.1 C6 (D11, hostile review finding): pressing Lock again while
    /// it's merely PENDING (no target set yet) used to cancel the arm
    /// unconditionally — a `lib.rs`-level special case that bypassed
    /// `on_press`'s D11 auto-repeat guard entirely, so an OS auto-repeat
    /// pulse while Lock was held down would wipe the pending arm before
    /// the user ever reached a trig. Two presses back to back (as fast as
    /// this test can drive them — far inside `REPEAT_GUARD_MS`) must now
    /// be tolerated exactly like Trk/Ptn: the arm survives, and a trig
    /// pressed afterward still sets the target.
    #[test]
    fn lock_key_rapid_repress_does_not_cancel_pending_arm() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Grid;

        app.handle_keys(&bus, &[kc('m')]); // Lock: arm
        app.handle_keys(&bus, &[kc('m')]); // rapid re-press: must not cancel
        assert_eq!(
            app.held.armed,
            Some(input::Hold::Lock),
            "a same-prefix press inside the guard window must not cancel the pending arm"
        );

        app.handle_keys(&bus, &[kc('q')]); // Trig1: the arm must still be live
        assert_eq!(
            app.model.lock_target,
            Some((0, 0)),
            "the pending arm must have survived to set the target"
        );
    }

    /// TK2.1 C5b (D15): published alongside `/script/theotokos/selected`.
    #[test]
    fn lock_target_is_published_to_the_bus() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.rec = RecMode::Grid;

        app.handle_keys(&bus, &[kc('m')]);
        app.handle_keys(&bus, &[kc('w')]); // Trig2 -> col 1

        let published = bus.borrow().read("/script/theotokos/lock_step").cloned();
        assert_eq!(
            published,
            Some(paraclete_node_api::StateBusValue::Int(1)),
            "the lock step must publish to the bus when set"
        );

        app.handle_keys(&bus, &[kc('w')]); // retap clears
        let published_after_clear = bus.borrow().read("/script/theotokos/lock_step").cloned();
        assert_eq!(
            published_after_clear,
            Some(paraclete_node_api::StateBusValue::Int(-1)),
            "clearing the target must publish -1"
        );
    }

    /// TK2.1 C4 (D10, closes BUG-040 §1): a composite page's encoder cell
    /// must resolve the real cap-doc range, not the `0.0, 1.0` placeholder.
    #[test]
    fn encoder_uses_descriptor_range_on_composite_pages() {
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        let param_id = ParamDescriptor::id_for_name("cutoff");
        app.model.caps.get_mut(&100).unwrap().params.push(ParamDescriptor {
            id: param_id,
            name: "cutoff".into(),
            min: 20.0,
            max: 20000.0,
            default: 1000.0,
            stepped: false,
            in_kit: true,
            unit: ParamUnit::Generic,
            display: None,
        });
        app.model.composite = vec![Some(composite_view_with_param(100, param_id))];

        let params = app.model.resolve_encoder_params();
        assert_eq!(
            params.iter().filter(|c| c.is_some()).count(),
            1,
            "one declared param, so one populated column"
        );
        let p = params[0].as_ref().expect("declared at slot 0");
        assert!(p.resolved, "must resolve against the cap-doc, not fall back");
        assert_eq!(p.min, 20.0);
        assert_eq!(p.max, 20000.0);
        assert!(!p.stepped);
    }

    /// TK2.1 C4 (D10, closes BUG-040 §2): a stepped param (an integer
    /// selector) jogs by exactly one unit, ignoring its (possibly wide)
    /// range entirely.
    #[test]
    fn stepped_param_jogs_by_exactly_one() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        let param_id = ParamDescriptor::id_for_name("waveform");
        app.model.caps.get_mut(&100).unwrap().params.push(ParamDescriptor {
            id: param_id,
            name: "waveform".into(),
            min: 0.0,
            max: 4.0,
            default: 0.0,
            stepped: true,
            in_kit: true,
            unit: ParamUnit::Generic,
            display: None,
        });
        app.model.composite = vec![Some(composite_view_with_param(100, param_id))];

        app.handle_keys(&bus, &[func_trig('q')]); // EncoderJog{col:0, dir:Next}

        let bump = app
            .pending
            .iter()
            .find(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM)
            .expect("must emit CMD_BUMP_PARAM");
        assert_eq!(
            bump.arg1, 1.0,
            "a stepped param must jog by exactly one, ignoring its range"
        );
    }

    /// TK2.1 C4 (D10, closes BUG-040 §1): the step-focus p-lock clamp must
    /// use the real resolved range, not the `0.0, 1.0` fallback that
    /// silently truncated a lock on a wide-range param to 1.0.
    #[test]
    fn plock_clamp_uses_real_range() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        let param_id = ParamDescriptor::id_for_name("cutoff");
        app.model.caps.get_mut(&100).unwrap().params.push(ParamDescriptor {
            id: param_id,
            name: "cutoff".into(),
            min: 20.0,
            max: 20000.0,
            default: 1000.0,
            stepped: false,
            in_kit: true,
            unit: ParamUnit::Generic,
            display: None,
        });
        app.model.composite = vec![Some(composite_view_with_param(100, param_id))];
        app.model.lock_target = Some((0, 0));
        bus.borrow_mut()
            .write("/node/100/param/cutoff", StateBusValue::Float(5000.0));

        app.handle_keys(&bus, &[func_trig('q')]); // focused jog -> lock path

        let lock_cmd = app
            .pending
            .iter()
            .rev()
            .find(|c| c.type_id == CMD_SET_STEP_LOCK)
            .expect("must emit CMD_SET_STEP_LOCK");
        assert!(
            lock_cmd.arg1 > 1000.0,
            "clamp must use the real range (20..20000), not fall back to \
             0..1 and truncate; got {}",
            lock_cmd.arg1
        );
    }

    #[test]
    fn page_select_rebinds_slots_a_b_c() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        // test_caps' page has 3 params — all three numpad slots (D13,
        // extended from TK1's 2) bind on construction/page select.
        assert!(app.model.slot_a.is_some());
        assert!(app.model.slot_b.is_some());
        assert!(app.model.slot_c.is_some());
        let c_before = app.model.slot_c.as_ref().unwrap().param_name.clone();
        assert_eq!(c_before, "width");

        // Re-selecting the same page (Pg1) re-resolves and rebinds again.
        app.handle_keys(&bus, &[kc('1')]);
        assert!(app.model.slot_a.is_some());
        assert!(app.model.slot_b.is_some());
        assert!(app.model.slot_c.is_some());
    }

    /// A capability doc with `n` params spread one-per-slot across a
    /// single Rule page — enough to force multiple 8-wide sub-pages.
    fn many_params_caps(n: usize) -> HashMap<u32, CapabilityDocument> {
        let mut params = Vec::with_capacity(n);
        let mut param_pages = Vec::with_capacity(n);
        for i in 0..n {
            let id = 1000 + i as u32;
            params.push(ParamDescriptor {
                id,
                name: paraclete_node_api::PortName::Dynamic(format!("p{i}")),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                stepped: false,
                in_kit: true,
                unit: ParamUnit::Generic,
                display: None,
            });
            param_pages.push((
                id,
                PageRef {
                    page: Cow::Borrowed("TRIG"),
                    slot: i as u8,
                },
            ));
        }
        let rule = Rule {
            name: "Engine".into(),
            page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG")]),
            param_pages: Cow::Owned(param_pages),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        };
        let mut caps = HashMap::new();
        caps.insert(
            100,
            CapabilityDocument {
                name: "Engine".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params,
                extensions: vec![],
                view: Some(rule),
            },
        );
        caps.insert(
            200,
            CapabilityDocument {
                name: "Seq".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![],
                extensions: vec![],
                view: None,
            },
        );
        caps
    }

    /// §0 A11: a page over 8 params must split into sub-pages instead of
    /// silently truncating — the same Pg key pressed again while already
    /// on that page cycles it (§0 A1 hypothesis) — post-C5 hostile review
    /// blocker fix (this behavior was entirely absent before).
    #[test]
    fn same_pg_key_cycles_sub_page() {
        let bus = test_bus();
        let mut app = TheotokosApp {
            model: Model::new(1, &[200], &[100], &["T1".into()], &["T1".into()], many_params_caps(10), vec![]),
            pending: Vec::new(),
            pending_app_ops: Vec::new(),
            quit: false,
            dirty: true,
            last_render: Instant::now(),
            frame_ms: 1000,
            tuning: Tuning::default(),
            jog_a: JogTracker::new(),
            jog_b: JogTracker::new(),
            jog_c: JogTracker::new(),
            encoder_trackers: std::array::from_fn(|_| JogTracker::new()),
            tap_times: Vec::new(),
            last_debug_event: None,
            last_jog_param: None,
            keymap: Keymap::default(),
            held: HeldState::new(false),
        };

        // 10 params split into 2 sub-pages (8 + 2).
        app.handle_keys(&bus, &[kc('1')]);
        assert_eq!(app.model.sub_page, 0);
        assert_eq!(app.model.page_sub_page_count(), 2);

        // Pg1 again (already on Param(0)) cycles the sub-page, not a reopen.
        app.handle_keys(&bus, &[kc('1')]);
        assert_eq!(
            app.model.sub_page, 1,
            "the same Pg key pressed again must cycle sub-page, not reopen at 0"
        );

        // Encoder col 0 on sub-page 1 must resolve to slot 8's param, not slot 0's.
        app.handle_keys(&bus, &[func_trig('q')]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == paraclete_node_api::CMD_BUMP_PARAM && c.arg0 == 1008),
            "encoder col 0 on sub-page 1 must target the param at slot 8"
        );
    }

    // ── TK2 C6: Tempo/Settings/Chain screens (D12) ──

    fn up_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)
    }

    fn func_up_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)
    }

    #[test]
    fn tempo_screen_yes_taps_set_bpm() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Tempo;

        // A single tap has no interval to measure yet.
        app.handle_keys(&bus, &[enter_key()]);
        assert!(
            app.pending.is_empty(),
            "one tap alone must not set a bpm (2+ taps required)"
        );

        // A 50ms gap is 1200bpm pre-clamp (60 / 0.05s) — comfortably past
        // the 300bpm ceiling even allowing generous scheduler slop (the
        // gap would need to stretch past ~200ms before the clamp stopped
        // saturating), so asserting the exact clamped value is safe, not
        // flaky, and — unlike just checking "a command was sent" — would
        // actually catch a broken averaging computation (review finding,
        // post-C6 hostile review: the original assertion never inspected
        // arg1 at all).
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.handle_keys(&bus, &[enter_key()]);
        let bpm_id = ParamDescriptor::id_for_name("bpm");
        let cmd = app
            .pending
            .iter()
            .find(|c| c.target_id == 1 && c.type_id == paraclete_node_api::CMD_SET_PARAM
                && c.arg0 == bpm_id as i64)
            .expect("a second tap must derive and send a bpm");
        assert!(
            (cmd.arg1 - 300.0).abs() < 0.01,
            "a ~50ms tap gap must clamp to the 300bpm ceiling, got {}",
            cmd.arg1
        );
    }

    #[test]
    fn tempo_arrows_nudge_bpm() {
        let bus = test_bus();
        {
            let mut b = bus.borrow_mut();
            b.write("/transport/bpm", StateBusValue::Float(120.0));
        }
        let bpm_id = ParamDescriptor::id_for_name("bpm");

        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Tempo;
        app.handle_keys(&bus, &[up_key()]);
        let coarse = app
            .pending
            .iter()
            .find(|c| c.arg0 == bpm_id as i64)
            .expect("UP must nudge bpm");
        assert!((coarse.arg1 - 121.0).abs() < 0.01, "bare UP must be +1");

        let mut app_fine = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app_fine.model.screen = Screen::Tempo;
        app_fine.handle_keys(&bus, &[func_up_key()]);
        let fine = app_fine
            .pending
            .iter()
            .find(|c| c.arg0 == bpm_id as i64)
            .expect("FUNC+UP must nudge bpm");
        assert!((fine.arg1 - 120.1).abs() < 0.01, "FUNC+UP must be +0.1");
    }

    #[test]
    fn chain_screen_yes_pushes_cursor_pattern() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Chain;
        app.model.chain_cursor = 3;

        app.handle_keys(&bus, &[enter_key()]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 200 && c.type_id == CMD_CHAIN_PUSH && c.arg0 == 3),
            "YES on the Chain screen must push the cursor pattern (3)"
        );
    }

    #[test]
    fn chain_screen_clear_sends_chain_clear() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Chain;

        app.handle_keys(&bus, &[esc_key()]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 200 && c.type_id == CMD_CHAIN_CLEAR),
            "NO on the Chain screen must clear the chain"
        );

        app.pending.clear();
        app.handle_keys(&bus, &[backspace_key()]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 200 && c.type_id == CMD_CHAIN_CLEAR),
            "Backspace on the Chain screen must also clear the chain (D12)"
        );
    }

    /// P11 C6a: the KIT button (7) opens the KIT screen (it no longer
    /// echoes "reserved (kit)" — the screen exists now).
    #[test]
    fn kit_button_opens_kit_screen() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.handle_keys(&bus, &[kc('7')]);
        assert_eq!(app.model.screen, Screen::Kit);
    }

    #[test]
    fn sampling_hidden_without_capability() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        let screen_before = app.model.screen;

        app.handle_keys(&bus, &[kc('9')]);
        assert!(
            app.pending.is_empty(),
            "SAMPLING must not emit any command"
        );
        assert!(
            app.model.cmdline_error.is_none(),
            "SAMPLING must not even echo (unlike KIT) — it's hidden entirely"
        );
        assert_eq!(
            app.model.screen, screen_before,
            "SAMPLING must not navigate anywhere"
        );
    }

    /// §2/D12 name no "return to Grid" gesture anywhere — Settings,
    /// Tempo, and Param were dead ends with no way back except quitting.
    /// Found live in the TK2 C7 agent smoke pass; NO doubles as the
    /// conventional "back" gesture everywhere it isn't already claimed
    /// (Chain's clear, tested separately, still wins there).
    #[test]
    fn esc_returns_to_grid_from_other_screens() {
        let bus = test_bus();
        for screen in [Screen::Settings, Screen::Tempo, Screen::Param(0), Screen::Kit] {
            let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
            app.model.screen = screen;
            app.handle_keys(&bus, &[esc_key()]);
            assert_eq!(
                app.model.screen,
                Screen::Grid,
                "Esc from {screen:?} must return to Grid"
            );
        }

        // Chain keeps its own meaning (clear), not a generic "back".
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Chain;
        app.handle_keys(&bus, &[esc_key()]);
        assert_eq!(
            app.model.screen,
            Screen::Chain,
            "Esc on Chain must clear, not navigate away"
        );
    }

    // ── P11 C6: Theotokos surfaces (KIT screen, perform, temp chords,
    //    pattern mute, live erase) ──

    use paraclete_node_api::app_op::{AppOp, KitId};

    /// P11 C6a: KIT screen trigs 13-16 push the matching AppOps (LOAD,
    /// SAVE with the "Kit N" auto-name, COMMIT, RELD).
    #[test]
    fn kit_screen_trig13_16_push_the_right_app_ops() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Kit;
        app.model.kit_cursor = 5;

        app.handle_keys(&bus, &[kc('g')]); // Trig13 = LOAD
        app.handle_keys(&bus, &[kc('h')]); // Trig14 = SAVE
        app.handle_keys(&bus, &[kc('j')]); // Trig15 = COMMIT
        app.handle_keys(&bus, &[kc('k')]); // Trig16 = RELD

        let ops = &app.pending_app_ops;
        assert!(
            ops.iter().any(|op| matches!(op, AppOp::KitLoad(KitId(5)))),
            "LOAD must push KitLoad(cursor=5), got {:?}",
            ops
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, AppOp::KitSaveAs(name) if name == "Kit 6")),
            "SAVE must auto-name 'Kit 6' from slot 5, got {:?}",
            ops
        );
        assert!(
            ops.iter().any(|op| matches!(op, AppOp::KitCommit)),
            "COMMIT must push KitCommit, got {:?}",
            ops
        );
        assert!(
            ops.iter().any(|op| matches!(op, AppOp::KitReload)),
            "RELD must push KitReload, got {:?}",
            ops
        );
    }

    /// P11 C6a: the encoder scroll on the KIT screen moves the cursor and
    /// page-aligns the list window; Prev clamps at 0.
    #[test]
    fn kit_screen_encoder_scroll_moves_cursor_and_clamps() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.model.screen = Screen::Kit;

        // FUNC+Trig9 (bottom row, col 8) scrolls toward the end.
        app.handle_keys(&bus, &[func_key('a')]);
        assert_eq!(app.model.kit_cursor, 1);
        assert_eq!(app.model.kit_scroll, 0);

        // FUNC+Trig1 (top row) scrolls back toward slot 0.
        app.handle_keys(&bus, &[func_key('q')]);
        assert_eq!(app.model.kit_cursor, 0);

        // Prev at 0 clamps.
        app.handle_keys(&bus, &[func_key('q')]);
        assert_eq!(app.model.kit_cursor, 0, "Prev must clamp at slot 0");
    }

    /// P11 C6b: FUNC+KIT toggles perform mode — the pushed AppOp is the
    /// inverse of the published `/context/perform`.
    #[test]
    fn func_kit_toggles_perform_mode_app_op() {
        let bus = test_bus();
        bus.borrow_mut()
            .write("/context/perform", StateBusValue::Float(1.0));
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        app.handle_keys(&bus, &[func_key('7')]);
        assert!(
            app.pending_app_ops
                .iter()
                .any(|op| matches!(op, AppOp::SetPerformMode(false))),
            "perform is on → FUNC+KIT must push SetPerformMode(false), got {:?}",
            app.pending_app_ops
        );

        bus.borrow_mut()
            .write("/context/perform", StateBusValue::Float(0.0));
        app.pending_app_ops.clear();
        app.handle_keys(&bus, &[func_key('7')]);
        assert!(
            app.pending_app_ops
                .iter()
                .any(|op| matches!(op, AppOp::SetPerformMode(true))),
            "perform is off → FUNC+KIT must push SetPerformMode(true), got {:?}",
            app.pending_app_ops
        );
    }

    /// P11 C6c: FUNC+YES pushes TempSave; FUNC+NO pushes TempReload.
    #[test]
    fn func_yes_and_func_no_push_temp_save_and_reload() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)]);
        assert!(
            app.pending_app_ops.iter().any(|op| matches!(op, AppOp::TempSave)),
            "FUNC+YES must push TempSave, got {:?}",
            app.pending_app_ops
        );

        app.pending_app_ops.clear();
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Esc, KeyModifiers::SHIFT)]);
        assert!(
            app.pending_app_ops.iter().any(|op| matches!(op, AppOp::TempReload)),
            "FUNC+NO must push TempReload, got {:?}",
            app.pending_app_ops
        );
    }

    /// TK3 C3 (#184): the new `pending_app_ops()` observer is
    /// NON-destructive — a test can inspect the ops without consuming
    /// them, so the normal drain path still finds them afterwards.
    #[test]
    fn pending_app_ops_is_a_non_destructive_observer() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        // FUNC+YES -> TempSave, surfaced through the accessor.
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)]);
        assert!(
            app.pending_app_ops()
                .iter()
                .any(|op| matches!(op, AppOp::TempSave)),
            "pending_app_ops() must surface the TempSave produced by FUNC+YES"
        );
        // Reading must not consume: the op is still there on a second read.
        assert!(
            app.pending_app_ops()
                .iter()
                .any(|op| matches!(op, AppOp::TempSave)),
            "pending_app_ops() must NOT consume the ops it observes"
        );
        // And the destructive drain still returns it.
        let drained = app.take_pending_app_ops();
        assert!(
            drained.iter().any(|op| matches!(op, AppOp::TempSave)),
            "take_pending_app_ops() must still see the op after the read"
        );

        // FUNC+Esc -> TempReload via the accessor.
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Esc, KeyModifiers::SHIFT)]);
        assert!(
            app.pending_app_ops()
                .iter()
                .any(|op| matches!(op, AppOp::TempReload)),
            "pending_app_ops() must surface the TempReload produced by FUNC+NO"
        );

        // FUNC+KIT -> SetPerformMode, both directions, via the accessor.
        bus.borrow_mut()
            .write("/context/perform", StateBusValue::Float(0.0));
        app.take_pending_app_ops();
        app.handle_keys(&bus, &[func_key('7')]);
        assert!(
            app.pending_app_ops()
                .iter()
                .any(|op| matches!(op, AppOp::SetPerformMode(true))),
            "perform off → FUNC+KIT must push SetPerformMode(true)"
        );
        bus.borrow_mut()
            .write("/context/perform", StateBusValue::Float(1.0));
        app.take_pending_app_ops();
        app.handle_keys(&bus, &[func_key('7')]);
        assert!(
            app.pending_app_ops()
                .iter()
                .any(|op| matches!(op, AppOp::SetPerformMode(false))),
            "perform on → FUNC+KIT must push SetPerformMode(false)"
        );
    }

    /// P11 C6e: TRK+FUNC+trig emits CMD_SET_PATTERN_MUTE (arg0=2) for the
    /// pressed column's track sequencer.
    #[test]
    fn trk_func_trig_emits_pattern_mute_toggle_for_that_track() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );

        // 'w' is Trig2 → column 1 → track 1's sequencer (201).
        app.handle_keys(&bus, &[tab_key(), func_trig('w')]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 201
                    && c.type_id == CMD_SET_PATTERN_MUTE
                    && c.arg0 == 2),
            "TRK+FUNC+Trig2 must toggle pattern mute on sequencer 201, got {:?}",
            app.pending
        );
    }

    /// P11 C6e: TRK+FUNC+Ctrl+trig emits CMD_PREPARE_PATTERN_MUTE with
    /// arg0 read from the current `/node/{id}/state/pattern_muted` — 1
    /// when unmuted, 0 when already muted.
    #[test]
    fn trk_func_ctrl_trig_prepares_pattern_mute_from_current_state() {
        let bus = test_bus();
        // Unmuted → the prepared direction is ON (arg0 = 1).
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        app.handle_keys(&bus, &[tab_key(), func_fine_trig('w')]);
        let prepared = app
            .pending
            .iter()
            .find(|c| c.type_id == CMD_PREPARE_PATTERN_MUTE)
            .expect("TRK+FUNC+Ctrl+trig must emit CMD_PREPARE_PATTERN_MUTE");
        assert_eq!(prepared.target_id, 201, "must target the column's track");
        assert_eq!(prepared.arg0, 1, "unmuted → prepared on");

        // Muted → the prepared direction is OFF (arg0 = 0).
        bus.borrow_mut().write(
            "/node/201/state/pattern_muted",
            StateBusValue::Bool(true),
        );
        let mut app2 = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        app2.handle_keys(&bus, &[tab_key(), func_fine_trig('w')]);
        let prepared2 = app2
            .pending
            .iter()
            .find(|c| c.type_id == CMD_PREPARE_PATTERN_MUTE)
            .expect("second press must emit CMD_PREPARE_PATTERN_MUTE");
        assert_eq!(prepared2.arg0, 0, "muted → prepared off");
    }

    /// P11 C6 (OQ-T25): a bare NO press while the transport plays queues
    /// CMD_LIVE_ERASE 1 to the selected track; the kitty release queues 0.
    #[test]
    fn live_erase_arms_on_no_press_and_disarms_on_release() {
        let bus = test_bus();
        bus.borrow_mut()
            .write("/transport/playing", StateBusValue::Bool(true));
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        app.held.kitty = true;
        app.model.active_track = 1; // erase targets the selected track

        // Press: Esc = NO.
        app.handle_keys(&bus, &[KeyEvent::new_with_kind(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 201 && c.type_id == CMD_LIVE_ERASE && c.arg0 == 1),
            "NO press while playing must arm live erase on the selected \
             track (201), got {:?}",
            app.pending
        );

        // Release: disarms.
        app.pending.clear();
        app.handle_keys(&bus, &[KeyEvent::new_with_kind(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )]);
        assert!(
            app.pending
                .iter()
                .any(|c| c.target_id == 201 && c.type_id == CMD_LIVE_ERASE && c.arg0 == 0),
            "NO release must disarm live erase, got {:?}",
            app.pending
        );
    }

    /// P11 C6 (OQ-T25): live erase does not arm when the transport is
    /// stopped, and FUNC+NO (temp reload) never arms it either.
    #[test]
    fn live_erase_not_armed_when_stopped_or_with_func() {
        let bus = test_bus();
        bus.borrow_mut()
            .write("/transport/playing", StateBusValue::Bool(false));
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);

        app.handle_keys(&bus, &[esc_key()]);
        assert!(
            app.pending.iter().all(|c| c.type_id != CMD_LIVE_ERASE),
            "stopped transport must not arm live erase, got {:?}",
            app.pending
        );

        bus.borrow_mut()
            .write("/transport/playing", StateBusValue::Bool(true));
        app.pending.clear();
        app.handle_keys(&bus, &[KeyEvent::new(KeyCode::Esc, KeyModifiers::SHIFT)]);
        assert!(
            app.pending.iter().all(|c| c.type_id != CMD_LIVE_ERASE),
            "FUNC+NO is the temp-reload chord, not live erase, got {:?}",
            app.pending
        );
        assert!(
            app.pending_app_ops
                .iter()
                .any(|op| matches!(op, AppOp::TempReload)),
            "FUNC+NO must still push TempReload, got {:?}",
            app.pending_app_ops
        );
    }

    // ── C5: p-lock UI tests ──

    fn enter_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    fn esc_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn backspace_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
    }

    fn shift_backspace_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::SHIFT)
    }

    fn setup_bus_with_params(bus: &BusHandle, seq_id: u32, gen_id: u32, steps_active: bool) {
        let mut b = bus.borrow_mut();
        let steps: String = (0..16)
            .map(|_| if steps_active { '1' } else { '0' })
            .collect();
        b.write("/transport/playing", StateBusValue::Bool(true));
        b.write("/transport/bpm", StateBusValue::Float(120.0));
        b.write(
            &format!("/node/{}/state/current_step", seq_id),
            StateBusValue::Int(0),
        );
        b.write(
            &format!("/node/{}/state/pattern_length", seq_id),
            StateBusValue::Int(16),
        );
        b.write(
            &format!("/node/{}/state/steps", seq_id),
            StateBusValue::Text(steps),
        );
        b.write(
            &format!("/node/{}/param/decay", gen_id),
            StateBusValue::Float(0.5),
        );
    }

    // TK2 C3: `enter_focuses_last_toggled_step`, `esc_releases_focus`,
    // `enter_focuses_in_seq_jog_edits_in_perf`,
    // `jog_while_focused_emits_target_then_lock_pair`,
    // `jog_lock_value_starts_from_existing_lock`,
    // `jog_lock_value_starts_from_live_when_no_lock`, and
    // `jog_without_focus_still_bumps_param` all tested TK1's Enter→FocusStep
    // and arrow-key→Jog triggers — both retired at the wiring flip (Enter
    // now resolves to `Yes`/`Action::Noop`; arrows are navigation buttons,
    // §2). `Action::FocusStep`/`Action::Jog` and their dispatch logic are
    // kept (D8: the encoder bank reuses the step-focus + jog/ramp
    // machinery in TK2 C5) but are temporarily unreachable from any key
    // until a new gesture is specified.

    #[test]
    fn backspace_clears_all_lanes() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.model.lock_target = Some((0, 3));
        app.handle_keys(&bus, &[backspace_key()]);

        assert_eq!(app.pending.len(), 1);
        assert_eq!(app.pending[0].type_id, CMD_CLEAR_STEP_LOCK);
        assert_eq!(app.pending[0].target_id, 200);
        assert_eq!(app.pending[0].arg0, 3);
        assert_eq!(app.pending[0].arg1, -1.0, "arg1=-1.0 clears all lanes");
    }

    #[test]
    fn shift_backspace_emits_target_then_clear_pair() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.model.lock_target = Some((0, 5));
        app.handle_keys(&bus, &[shift_backspace_key()]);

        assert_eq!(app.pending.len(), 2, "Shift+Backspace emits pair");
        assert_eq!(app.pending[0].type_id, CMD_SET_LOCK_TARGET);
        assert_eq!(app.pending[1].type_id, CMD_CLEAR_STEP_LOCK);
        assert_eq!(
            app.pending[1].arg1, app.pending[0].arg1,
            "clear arg1 must match target arg1 (param_id)"
        );
    }

    #[test]
    fn parse_lock_value_finds_exact_match() {
        // `=` before the value, matching what `Sequencer::published_state`
        // actually emits (`sequencer.rs:1564`). These fixtures used a colon
        // there and passed anyway, because `splitn(4, [':', '='])` accepts
        // both — a shape no writer produces (AGENTS.md design-learning 4).
        let locks = "s2:100:500=0.300;s3:100:500=0.700;s0:200:600=0.100";
        assert_eq!(Model::parse_lock_value(locks, 2, 100, 500), Some(0.3));
        assert_eq!(Model::parse_lock_value(locks, 3, 100, 500), Some(0.7));
        assert_eq!(Model::parse_lock_value(locks, 0, 200, 600), Some(0.1));
    }

    /// #181 (BUG-071): a malformed entry costs that entry, not the scan.
    ///
    /// `published_state` emits a whole pattern's locks as one `;`-joined
    /// string ordered by step, so aborting on a bad entry hid every lock
    /// after it — and the caller cannot distinguish that from "no lock here",
    /// so the jog path would overwrite the lock it could not see. Each case
    /// below corrupts a different field, since each had its own `?`.
    #[test]
    fn a_malformed_lock_entry_does_not_hide_the_ones_after_it() {
        let good = "s9:100:500=0.900";
        for bad in [
            "sX:100:500=0.300", // step not a number
            "s2:1zz:500=0.300", // node id not a number
            "s2:100:5e5=0.300", // param id not a number
            "s2:100:500=0.300", // well-formed, just not the one we want
            "2:100:500=0.300",  // step missing its `s` prefix
        ] {
            let locks = format!("{bad};{good}");
            assert_eq!(
                Model::parse_lock_value(&locks, 9, 100, 500),
                Some(0.9),
                "entry after {bad:?} must still be found"
            );
        }
    }

    #[test]
    fn parse_lock_value_returns_none_for_mismatch() {
        let locks = "s2:100:500=0.300";
        assert_eq!(Model::parse_lock_value(locks, 2, 100, 999), None);
        assert_eq!(Model::parse_lock_value(locks, 9, 100, 500), None);
        assert_eq!(Model::parse_lock_value("", 2, 100, 500), None);
    }

    #[test]
    fn backspace_noop_when_not_focused() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[backspace_key()]);
        assert!(app.pending.is_empty(), "Backspace without focus is no-op");
    }

    // TK2 C3: `enter_without_last_step_does_not_focus` is retired alongside
    // the other Enter→FocusStep tests above — under the new grammar it
    // would pass vacuously (Enter no longer produces `FocusStep` at all).

    // ── C6: command line tests ──

    fn colon_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Char(';'), KeyModifiers::SHIFT)
    }

    fn cmdline_type(app: &mut TheotokosApp, bus: &BusHandle, text: &str) {
        for c in text.chars() {
            app.handle_keys(bus, &[KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)]);
        }
    }

    #[test]
    fn colon_opens_and_esc_cancels() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        assert!(app.model.cmdline.is_some(), "colon must open cmdline");

        app.handle_keys(&bus, &[esc_key()]);
        assert!(app.model.cmdline.is_none(), "Esc must cancel cmdline");
    }

    #[test]
    fn cmdline_captures_all_keys_while_open() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        // typing should not trigger normal key handlers (like the trig 'a'
        // would otherwise resolve to)
        let prev_pending = app.pending.len();
        app.handle_keys(&bus, &[kc('a')]);
        assert_eq!(
            app.pending.len(),
            prev_pending,
            "keys captured, no trig command emitted"
        );
        assert!(
            app.model.cmdline.as_deref().unwrap().contains('a'),
            "text must accumulate"
        );
    }

    #[test]
    fn enter_executes_set_with_fuzzy_param_match() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        // Open cmdline, type "set dec 0.8", execute
        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "set dec 0.8");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(
            app.pending.iter().any(|c| {
                c.type_id == paraclete_node_api::CMD_SET_PARAM && (c.arg1 - 0.8).abs() < 0.01
            }),
            "must emit CMD_SET_PARAM decay=0.8"
        );
    }

    // ── #176 (BUG-067): a stepped selector reads its name, not its index ──

    #[test]
    fn option_label_indexes_by_value_and_declines_what_it_cannot_name() {
        let opts = vec![
            Some("off".to_string()),
            Some("tune".to_string()),
            None, // a value the display declines to name
            Some("decay".to_string()),
        ];
        let l = |v: f64| model::option_label(Some(&opts), v);

        assert_eq!(l(0.0), Some("off"));
        assert_eq!(l(1.0), Some("tune"));
        assert_eq!(l(3.0), Some("decay"));
        // A stepped value is an index: an encoder accumulated to 0.999 is on 1.
        assert_eq!(l(0.999), Some("tune"));
        assert_eq!(l(1.4), Some("tune"));
        // A gap must fall back to the number, never synthesize a name.
        assert_eq!(l(2.0), None);
        // Off the end, negative, and non-finite all fall back too.
        assert_eq!(l(4.0), None);
        assert_eq!(l(-1.0), None);
        assert_eq!(l(f64::NAN), None);
        assert_eq!(l(f64::INFINITY), None);
        // No table at all: every value is numeric.
        assert_eq!(model::option_label(None, 1.0), None);
    }

    /// The readout itself: a labelled value reads as its name, everything
    /// else keeps the two-decimal formatting.
    #[test]
    fn encoder_cell_value_text_prefers_the_label() {
        let cell = |value: f64, options: Option<Vec<Option<String>>>| render::EncoderCell {
            name: "lfo_dest".into(),
            value,
            min: 0.0,
            max: 8.0,
            resolved: true,
            options,
            target: crate::model::EncoderTarget::Real,
        };
        let dests = Some(vec![Some("off".to_string()), Some("tune".to_string())]);
        assert_eq!(cell(1.0, dests.clone()).value_text(), "tune");
        assert_eq!(cell(0.0, dests.clone()).value_text(), "off");
        // Past the table — the number, not a panic or a wrong name.
        assert_eq!(cell(5.0, dests).value_text(), "5.00");
        // Continuous params are untouched.
        assert_eq!(cell(0.25, None).value_text(), "0.25");
    }

    /// The consumer gap #176 is actually about: assembly has carried these
    /// labels since MM-C11 (view-assembly's
    /// `a_stepped_param_carries_labels_from_its_descriptor` and the machine
    /// selector's `options`), and Theotokos dropped them on the floor —
    /// AGENTS.md design-learning 9, a declared contract with no consumer.
    /// The producer-side tests all passed the whole time.
    #[test]
    fn composite_param_labels_reach_the_encoder_bank() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        let param_id = ParamDescriptor::id_for_name("lfo_dest");
        app.model
            .caps
            .get_mut(&100)
            .unwrap()
            .params
            .push(ParamDescriptor {
                id: param_id,
                name: "lfo_dest".into(),
                min: 0.0,
                max: 8.0,
                default: 0.0,
                stepped: true,
                in_kit: false,
                unit: ParamUnit::Generic,
                display: None,
            });
        let mut cv = composite_view_with_param(100, param_id);
        cv.pages[0].params[0].name = "lfo_dest".into();
        cv.pages[0].params[0].stepped = true;
        cv.pages[0].params[0].options = Some(vec![
            Some("off".into()),
            Some("tune".into()),
            Some("tone".into()),
        ]);
        app.model.composite = vec![Some(cv)];

        let bank = app.model.resolve_encoder_params();
        let cell = bank[0].as_ref().expect("slot 0 is populated");
        assert_eq!(
            cell.options.as_deref().unwrap(),
            [
                Some("off".to_string()),
                Some("tune".to_string()),
                Some("tone".to_string())
            ],
            "the encoder bank must carry assembly's labels through"
        );
    }

    /// The other source: the non-composite fallback path reads the labels off
    /// the descriptor itself, via `ParamDescriptor::value_labels`.
    #[test]
    fn descriptor_labels_reach_the_encoder_bank_without_a_composite_view() {
        use paraclete_node_api::{ParamDisplay, ParamDisplayAdapter};

        struct Dests;
        impl ParamDisplay for Dests {
            fn format(&self, value: f64) -> String {
                ["off", "tune", "tone"]
                    .get(value as usize)
                    .unwrap_or(&"?")
                    .to_string()
            }
            fn parse(&self, _s: &str) -> Option<f64> {
                None
            }
        }
        static DESTS: Dests = Dests;

        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        let caps = app.model.caps.get_mut(&100).unwrap();
        caps.params.clear();
        caps.params.push(ParamDescriptor {
            id: ParamDescriptor::id_for_name("lfo_dest"),
            name: "lfo_dest".into(),
            min: 0.0,
            max: 2.0,
            default: 0.0,
            stepped: true,
            in_kit: false,
            unit: ParamUnit::Generic,
            display: Some(ParamDisplayAdapter::Static(&DESTS)),
        });
        // `test_caps`'s Rule declares no `page_groups`, so this lands in the
        // "no Rule pagination" branch — positional, cap-doc order. Leaving
        // `view: None` instead would return an empty bank before reaching it.
        assert!(caps.view.as_ref().unwrap().page_groups.is_empty());
        app.model.composite = vec![None];

        let bank = app.model.resolve_encoder_params();
        let cell = bank[0].as_ref().expect("slot 0 is populated");
        assert_eq!(
            cell.options.as_deref().unwrap(),
            [
                Some("off".to_string()),
                Some("tune".to_string()),
                Some("tone".to_string())
            ]
        );
    }

    /// #177 (BUG-068): `:set` must clamp to the param's **declared** range.
    ///
    /// It clamped to a literal 0..1, so a param whose range sits above 1.0
    /// always landed on its minimum (`ParameterBank::set` clamps the 1.0 back
    /// *up*) and the negative half of a signed range was unreachable. Every
    /// existing `set` test uses `"set dec 0.8"` — `decay` is 0..1, the one
    /// range that cannot expose it — so this builds its own caps rather than
    /// extending the shared fixture, whose params are all 0..1 by design.
    #[test]
    fn set_clamps_to_the_params_declared_range_not_a_literal_0_1() {
        let param = |name: &'static str, min: f64, max: f64| ParamDescriptor {
            id: ParamDescriptor::id_for_name(name),
            name: name.into(),
            min,
            max,
            default: min,
            stepped: false,
            in_kit: !name.starts_with("lfo_"),
            unit: ParamUnit::Generic,
            display: None,
        };
        let mut caps = HashMap::new();
        caps.insert(
            100,
            CapabilityDocument {
                name: "Engine".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![
                    param("tone", 200.0, 8000.0),
                    param("lfo_fade", -1.0, 1.0),
                    param("decay", 0.0, 1.0),
                ],
                extensions: vec![],
                view: None,
            },
        );
        let names = vec!["Kick".to_string()];
        let model = Model::new(1, &[200], &[100], &names, &names, caps, vec![]);

        let set_value = |cmd: &str| match model.parse_cmdline(cmd) {
            Ok(CmdlineVerb::Set { value, .. }) => value,
            Ok(_) => panic!("{cmd:?} did not parse as `set`"),
            Err(e) => panic!("{cmd:?} failed to parse: {e}"),
        };

        // Range entirely above 1.0: the value must survive, and the bounds
        // must be the descriptor's.
        assert_eq!(set_value("set tone 4000"), 4000.0);
        assert_eq!(set_value("set tone 200"), 200.0);
        assert_eq!(set_value("set tone 99999"), 8000.0, "clamped to max, not 1.0");
        assert_eq!(set_value("set tone 0.5"), 200.0, "clamped up to min");

        // Signed range: the fade-out half was unreachable past the 0.0 floor.
        assert_eq!(set_value("set lfo_fade -0.5"), -0.5);
        assert_eq!(set_value("set lfo_fade -9"), -1.0);
        assert_eq!(set_value("set lfo_fade 1"), 1.0);

        // The 0..1 case the old literal happened to get right still holds.
        assert_eq!(set_value("set decay 0.8"), 0.8);
        assert_eq!(set_value("set decay 5"), 1.0);
    }

    /// TK2.1 C5b (D15): `:set` routes to the lock target when it's on the
    /// active track.
    #[test]
    fn set_routes_to_lock_when_active_track_is_locked() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);
        app.model.lock_target = Some((0, 3));

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "set dec 0.8");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(
            app.pending
                .iter()
                .any(|c| c.type_id == CMD_SET_LOCK_TARGET && c.target_id == 200),
            "must route to the active track's own sequencer's lock target"
        );
        assert!(
            app.pending.iter().any(|c| c.type_id == CMD_SET_STEP_LOCK
                && c.arg0 == 3
                && (c.arg1 - 0.8).abs() < 0.01),
            "must write the step lock at the locked step with the set value"
        );
    }

    /// TK2.1 C5b (D15, hostile review finding): `:set` must fall back to
    /// the live bank when the lock target is on a DIFFERENT track than
    /// the one `:set`'s node actually belongs to — `:set`'s node_id is
    /// always resolved against the active track, never the locked one, so
    /// routing to the locked track's sequencer here would write a lock
    /// command referencing the wrong node entirely.
    #[test]
    fn set_falls_back_to_live_when_lock_target_is_on_a_different_track() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);
        app.model.lock_target = Some((5, 2)); // a different track

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "set dec 0.8");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(
            app.pending.iter().any(|c| c.type_id == paraclete_node_api::CMD_SET_PARAM
                && c.target_id == 100
                && (c.arg1 - 0.8).abs() < 0.01),
            "must fall back to the live bank on the resolved node"
        );
        assert!(
            !app.pending.iter().any(|c| c.type_id == CMD_SET_LOCK_TARGET),
            "must not emit a lock-target command for a lock on another track"
        );
    }

    #[test]
    fn fuzzy_index_contains_params_and_verbs() {
        let caps = test_caps();
        let tracks = vec![TrackInfo {
            sequencer_id: 200,
            generator_id: 100,
            name: "Kick".into(),
            display_name: "Kick".into(),
        }];
        let index = Model::build_fuzzy_index(&caps, &tracks);
        let entries: Vec<String> = index.iter().map(|e| e.text.clone()).collect();
        assert!(
            entries.contains(&"decay".to_string()),
            "index must contain decay param"
        );
        assert!(
            entries.contains(&"bpm".to_string()),
            "index must contain bpm verb"
        );
    }

    #[test]
    fn stale_error_cleared_on_successful_command() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        // Fail a command
        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "badcmd");
        app.handle_keys(&bus, &[enter_key()]);
        assert!(
            app.model.cmdline_error.is_some(),
            "must have error after bad cmd"
        );

        // Edit to a valid command and succeed
        app.handle_keys(&bus, &[backspace_key()]);
        app.handle_keys(&bus, &[backspace_key()]);
        app.handle_keys(&bus, &[backspace_key()]);
        app.handle_keys(&bus, &[backspace_key()]);
        app.handle_keys(&bus, &[backspace_key()]);
        app.handle_keys(&bus, &[backspace_key()]);
        cmdline_type(&mut app, &bus, "bpm 130");
        app.handle_keys(&bus, &[enter_key()]);
        assert!(app.model.cmdline.is_none(), "cmdline closed on success");
        assert!(
            app.model.cmdline_error.is_none(),
            "error must be cleared on success"
        );
    }

    #[test]
    fn ctrl_c_during_cmdline_quits_app() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        app.handle_keys(
            &bus,
            &[KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)],
        );
        assert!(app.model.cmdline.is_none(), "cmdline must close");
        assert!(app.should_quit(), "Ctrl+C must set quit flag");
    }

    #[test]
    fn empty_cmdline_returns_error() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        app.handle_keys(&bus, &[enter_key()]);
        assert!(
            app.model.cmdline_error.is_some(),
            "empty cmdline must error"
        );
        assert!(app.model.cmdline.is_some(), "cmdline must stay open");
    }

    #[test]
    fn bpm_command_sends_set_param_to_clock() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "bpm 140");
        app.handle_keys(&bus, &[enter_key()]);

        let bpm_id = paraclete_node_api::ParamDescriptor::id_for_name("bpm");
        assert!(
            app.pending.iter().any(|c| {
                c.target_id == 1
                    && c.type_id == paraclete_node_api::CMD_SET_PARAM
                    && c.arg0 == bpm_id as i64
                    && (c.arg1 - 140.0).abs() < 0.01
            }),
            "must emit CMD_SET_PARAM bpm=140 on clock"
        );
    }

    #[test]
    fn mute_command_sends_explicit_value() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        let mute_id = paraclete_node_api::ParamDescriptor::id_for_name("mute");

        // mute 1
        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "mute 1");
        app.handle_keys(&bus, &[enter_key()]);
        assert!(
            app.pending
                .iter()
                .any(|c| { c.target_id == 200 && c.arg0 == mute_id as i64 && c.arg1 == 1.0 }),
            "mute 1 must set mute to 1.0"
        );

        // unmute 1
        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "unmute 1");
        app.handle_keys(&bus, &[enter_key()]);
        assert!(
            app.pending.iter().any(|c| { c.arg1 == 0.0 }),
            "unmute 1 must set mute to 0.0"
        );
    }

    #[test]
    fn unknown_command_echoes_error_no_crash() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "foobar 123");
        app.handle_keys(&bus, &[enter_key()]);

        // Should re-open cmdline with error
        assert!(app.model.cmdline.is_some(), "cmdline stays open on error");
        assert!(app.model.cmdline_error.is_some(), "must set error message");
        assert!(
            app.model.cmdline_error.as_deref().unwrap().starts_with('?'),
            "error must start with ?"
        );
    }

    // ── TK2 C8: key remapping (D11/D14) ──

    #[test]
    fn bind_verb_adds_a_working_binding() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "bind w song");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(app.model.cmdline.is_none(), "cmdline closes on success");
        assert_eq!(
            crate::input::key_to_button(&app.keymap, kc('w')),
            Some(crate::input::PanelButton::Song),
            "'w' must now resolve to Song, not the default Trig2"
        );

        // `:unbind` removes it — 'w' falls back to the built-in default.
        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "unbind w");
        app.handle_keys(&bus, &[enter_key()]);
        assert_eq!(
            crate::input::key_to_button(&app.keymap, kc('w')),
            Some(crate::input::PanelButton::Trig2),
        );
    }

    #[test]
    fn bind_unbindable_key_errors() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "bind : trig1");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(
            app.model.cmdline.is_some(),
            "cmdline stays open on error"
        );
        assert!(
            app.model.cmdline_error.is_some(),
            "must error on the unbindable ':' key"
        );
        assert!(
            app.keymap.bindings.is_empty(),
            "the rejected bind must not be applied"
        );
    }

    #[test]
    fn unknown_button_name_echoes_error() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "bind q notabutton");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(app.model.cmdline.is_some(), "cmdline stays open on error");
        assert!(
            app.model.cmdline_error.is_some(),
            "unknown button name must echo an error"
        );
        assert!(app.keymap.bindings.is_empty());
    }

    #[test]
    fn reset_clears_all_user_bindings() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "bind w song");
        app.handle_keys(&bus, &[enter_key()]);
        assert!(!app.keymap.bindings.is_empty());

        app.handle_keys(&bus, &[colon_key()]);
        cmdline_type(&mut app, &bus, "reset-bindings");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(
            app.keymap.bindings.is_empty(),
            "reset-bindings must clear every user binding"
        );
        assert_eq!(
            crate::input::key_to_button(&app.keymap, kc('w')),
            Some(crate::input::PanelButton::Trig2),
            "must fall through to the §2 default after reset"
        );
    }

    /// TK2 C8: serializes tests that mutate the process-global `$HOME` env
    /// var (`Keymap::global_path`/`save_global`/`load_startup` all read
    /// it). `cargo test`'s default multi-threaded runner would otherwise
    /// let two such tests interleave and clobber each other's `$HOME` mid-
    /// test — a real hazard flagged as a blocker in post-C8 hostile review
    /// (could point a test at, and write into, the developer's actual
    /// `~/.config/paraclete/keymap.yaml`).
    static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Runs `f` with `$HOME` pointed at a scratch directory unique to this
    /// call, holding `HOME_ENV_LOCK` for the duration so no other
    /// `$HOME`-touching test can interleave. Restores the real `$HOME` and
    /// removes the scratch directory afterward even if `f` panics (an
    /// assertion failure must not leave `$HOME` pointed at a deleted
    /// scratch dir for the rest of the test process).
    fn with_scratch_home(tag: &str, f: impl FnOnce()) {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = std::env::temp_dir().join(format!(
            "paraclete-theotokos-test-home-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &scratch);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&scratch);

        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    /// TK2 C8 (D14): "no auto-save" — quitting (Ctrl-C) must never write
    /// `~/.config/paraclete/keymap.yaml`, only the explicit `:save-bindings`
    /// verb does. Uses a scratch `$HOME` so the assertion can check the
    /// real save path without touching the developer's actual config.
    #[test]
    fn bindings_do_not_autosave_on_quit() {
        with_scratch_home("autosave", || {
            let bus = test_bus();
            let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
            setup_bus_with_params(&bus, 200, 100, true);
            app.keymap.bindings.insert(
                crate::input::KeyBinding {
                    code: KeyCode::Char('w'),
                },
                crate::input::PanelButton::Song,
            );

            app.handle_keys(
                &bus,
                &[KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)],
            );
            assert!(app.should_quit());

            let saved_path = crate::input::Keymap::global_path().expect("HOME is set");
            assert!(
                !saved_path.exists(),
                "quitting must not write the keymap file"
            );
        });
    }

    #[test]
    fn save_bindings_writes_and_load_bindings_reads_back() {
        with_scratch_home("save-load", || {
            let bus = test_bus();
            let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
            setup_bus_with_params(&bus, 200, 100, true);

            app.handle_keys(&bus, &[colon_key()]);
            cmdline_type(&mut app, &bus, "bind w song");
            app.handle_keys(&bus, &[enter_key()]);

            app.handle_keys(&bus, &[colon_key()]);
            cmdline_type(&mut app, &bus, "save-bindings");
            app.handle_keys(&bus, &[enter_key()]);
            let saved_path = crate::input::Keymap::global_path().expect("HOME is set");
            assert!(saved_path.exists(), "save-bindings must write the file");

            // Clear the runtime keymap, then reload from disk.
            app.keymap.bindings.clear();
            app.handle_keys(&bus, &[colon_key()]);
            cmdline_type(&mut app, &bus, "load-bindings");
            app.handle_keys(&bus, &[enter_key()]);
            assert_eq!(
                crate::input::key_to_button(&app.keymap, kc('w')),
                Some(crate::input::PanelButton::Song),
                "load-bindings must restore the saved binding"
            );
        });
    }

    /// TK2.1 C6 (D14, hostile review finding): the skip-report plumbing
    /// (`Keymap::from_yaml`'s `Vec<String>`) is unit-tested at the
    /// `Keymap` level, but nothing previously exercised the end-to-end
    /// path — a real `keymap.yaml` with a stale entry, loaded through
    /// `:load-bindings`, actually surfacing the skip via `cmdline_status`.
    #[test]
    fn load_bindings_reports_skipped_entries_in_cmdline_status() {
        with_scratch_home("load-skip-report", || {
            let bus = test_bus();
            let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
            setup_bus_with_params(&bus, 200, 100, true);

            let path = crate::input::Keymap::global_path().expect("HOME is set");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "q: Trig9\nm: Mute\n").unwrap();

            app.handle_keys(&bus, &[colon_key()]);
            cmdline_type(&mut app, &bus, "load-bindings");
            app.handle_keys(&bus, &[enter_key()]);

            assert_eq!(
                crate::input::key_to_button(&app.keymap, kc('q')),
                Some(crate::input::PanelButton::Trig9),
                "the well-formed entry must still load"
            );
            let status = app.model.cmdline_status.as_deref().unwrap_or_default();
            assert!(
                status.contains("m: Mute"),
                "cmdline_status must report the skipped stale entry, got: {status:?}"
            );
        });
    }

    // ── C7: flash ──
    //
    // TK2 C3: `seq_number_row_sends_set_pattern` (number-row 1-8 pattern
    // select), `yank_then_paste_emits_full_step_command_batch` /
    // `paste_clears_stale_lock_lanes_before_writing` ('y'/'Y'), and
    // `leader_esc_cancels_chord` / `leader_rebind_b3_binds_third_page_param`
    // ('\' leader) all tested TK1 bindings retired at the wiring flip (§2).
    // Pattern select now goes through `select_pattern_via_ptn_chord_...`-
    // style tests (see `pattern_chord_clamps_with_echo` below); copy/paste
    // moves to FUNC+REC/STOP in TK2 C4 (D7), reusing the unchanged
    // `yank_active_pattern`/`paste_pattern` functions below; the leader
    // mechanism (`LeaderState`, `set_slot_lead`) is retired outright with
    // no planned replacement (slot binding becomes automatic page-select
    // binding + the encoder bank, D13/D8).

    #[test]
    fn flash_detects_value_change() {
        let mut model = Model::new(1, &[200], &[100], &["Kick".into()], &["Kick".into()], test_caps(), vec![]);
        model.slot_a = Some(SlotBinding {
            node_id: 100,
            param_id: ParamDescriptor::id_for_name("decay"),
            param_name: "decay".into(),
            min: 0.0,
            max: 1.0,
        });
        model.last_slot_values[0] = 0.5;
        assert!(model.slot_flash[0].is_none(), "no flash initially");

        model.update_flash(0, 0.7);
        assert!(
            model.slot_flash[0].is_some(),
            "value change must trigger flash"
        );

        model.update_flash(0, 0.7);
        // second update with same value should not reset flash time
    }
}
