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
    CMD_CLOCK_START, CMD_CLOCK_STOP, CMD_SET_LOCK_TARGET, CMD_SET_PATTERN, CMD_SET_STEP_LOCK,
    CMD_TRIG_NOW, GRID_STEPS, PATTERN_BANK_SIZE,
};
use crate::input::{
    button_to_action, key_label, key_to_button, trig_button, HeldState, Keymap, Mods, PanelButton,
};
use crate::model::{
    CmdlineVerb, Dir, JogTracker, Model, RecMode, Screen, Slot, Tuning, YankedLock, YankedStep,
};

pub type BusHandle = Rc<RefCell<StateBusHandle>>;

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
    /// TK1 C3: composite views, one per track, same order as tracks.
    pub composite: Vec<CompositeView>,
    pub fps: u64,
}

pub struct TheotokosApp {
    model: Model,
    pending: Vec<NodeCommand>,
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
    /// TK2 C3/C8 (D11): flat user keymap. Loaded global→local at startup
    /// (`Keymap::load_startup`); `:bind`/`:unbind`/`:reset-bindings` edit
    /// it at runtime; `:save-bindings` is the only write-to-disk path.
    keymap: Keymap,
    /// TK2 C3 (D6): TRK/PTN hold-chord state — kitty-probed at startup.
    held: HeldState,
}

impl TheotokosApp {
    /// TK2.1 C1 (D7): the command(s) issued once at startup, pushed onto
    /// `pending` before the first drain — `Model::new` boots with
    /// `rec: RecMode::Off`, but the clock itself (`InternalClock::new`,
    /// BUG-039) still constructs `playing: true`, so this is what actually
    /// makes the instrument boot silent. Honest bound: `main.rs` ticks the
    /// executor before draining commands, so frame 1 still paints
    /// `playing = true` — "boots stopped" holds from the first drain, not
    /// the first frame. A free function (not a method) so both `new` and
    /// the `new_pushes_clock_stop` test can build the same value without
    /// duplicating the `NodeCommand` literal.
    fn startup_commands(clock_id: u32) -> Vec<NodeCommand> {
        vec![NodeCommand {
            target_id: clock_id,
            type_id: CMD_CLOCK_STOP,
            arg0: 0,
            arg1: 0.0,
        }]
    }

    pub fn new(config: TheotokosConfig) -> Result<Self, String> {
        setup_keyboard_flags()?;

        let model = Model::new(
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

        let mut pending = Vec::with_capacity(64);
        pending.extend(Self::startup_commands(config.clock_id));

        Ok(Self {
            model,
            pending,
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
            // TK2 C8 (D11): global→local YAML load at startup.
            keymap: Keymap::load_startup(),
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
        let step_states: Vec<_> = (0..self.model.tracks.len())
            .map(|t| self.model.read_step_state(bus, t))
            .collect();
        let step_state = step_states
            .get(self.model.active_track)
            .cloned()
            .unwrap_or_default();
        let bpm = self.model.read_bpm(bus);

        let slot_a_value = self
            .model
            .slot_a
            .as_ref()
            .map(|s| self.model.read_param_value(bus, s.node_id, s.param_id))
            .unwrap_or(0.0);
        let slot_b_value = self
            .model
            .slot_b
            .as_ref()
            .map(|s| self.model.read_param_value(bus, s.node_id, s.param_id))
            .unwrap_or(0.0);
        let slot_c_value = self
            .model
            .slot_c
            .as_ref()
            .map(|s| self.model.read_param_value(bus, s.node_id, s.param_id))
            .unwrap_or(0.0);

        self.model.update_flash(0, slot_a_value);
        self.model.update_flash(1, slot_b_value);
        // TK2 C5 (D13): slot C flashes too — was bound but never tracked
        // (review finding, post-C5 hostile review).
        self.model.update_flash(2, slot_c_value);

        let envelope = self.model.envelope_for_active_track().map(|e| {
            let val = self.model.read_param_value(bus, e.node_id, e.param_id);
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

        let step_focuses = self.model.step_focus.clone();
        let step_locks: Vec<Vec<usize>> = (0..self.model.tracks.len())
            .map(|t| self.model.read_step_locks(bus, t))
            .collect();
        // TK2 C4 (D12): the Mute screen renders every track's mute state.
        let mute_states: Vec<bool> = self
            .model
            .tracks
            .iter()
            .map(|t| {
                bus.read(&format!("/node/{}/param/mute", t.sequencer_id))
                    .is_some_and(|v| matches!(v, paraclete_node_api::StateBusValue::Float(f) if *f >= 0.5))
            })
            .collect();

        // TK2 C6 (D12): Chain screen state, read from the active track's
        // published sequencer state.
        let active_seq_id = self.model.tracks[self.model.active_track].sequencer_id;
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

        // TK2 C5 (D8/§0 A11): the active page's params in Rule order,
        // restricted to the current sub-page's 8-wide window (pages with
        // more than 8 params split into sub-pages instead of silently
        // truncating). Resolved fresh each render, matching the jog
        // dispatch below.
        let encoder_params = self.model.resolve_encoder_params();
        let encoder_cells: Vec<Option<render::EncoderCell>> = (0..8)
            .map(|i| {
                encoder_params
                    .get(i)
                    .map(|(nid, pid, name, min, max, _stepped, resolved)| {
                        let value = self.model.read_param_value(bus, *nid, *pid);
                        render::EncoderCell {
                            name: name.clone(),
                            value,
                            min: *min,
                            max: *max,
                            resolved: *resolved,
                        }
                    })
            })
            .collect();
        for (i, cell) in encoder_cells.iter().enumerate() {
            if let Some(c) = cell {
                self.model.update_encoder_flash(i, c.value);
            }
        }
        let encoder_flash: Vec<bool> = (0..8)
            .map(|i| {
                self.model.encoder_flash[i]
                    .is_some_and(|t| t.elapsed().as_millis() < self.tuning.flash_ms as u128)
            })
            .collect();

        let mut slot_a_locked = false;
        let mut slot_b_locked = false;
        let mut slot_c_locked = false;
        if let Some(focus) = step_focuses.get(self.model.active_track).copied().flatten() {
            if let Some(ref s) = self.model.slot_a {
                let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                slot_a_locked = self
                    .model
                    .read_lock_value(bus, seq_id, focus, s.node_id, s.param_id)
                    .is_some();
            }
            if let Some(ref s) = self.model.slot_b {
                let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                slot_b_locked = self
                    .model
                    .read_lock_value(bus, seq_id, focus, s.node_id, s.param_id)
                    .is_some();
            }
            if let Some(ref s) = self.model.slot_c {
                let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                slot_c_locked = self
                    .model
                    .read_lock_value(bus, seq_id, focus, s.node_id, s.param_id)
                    .is_some();
            }
        }

        // TK2.1 C2 (D3/D4): key chip labels, resolved fresh each render
        // against the live keymap (bindings can change via `:bind` at
        // runtime).
        let trig_key_labels: Vec<Option<String>> = (0..16)
            .map(|i| trig_button(i).and_then(|b| key_label(&self.keymap, b)))
            .collect();
        let track_key_labels: Vec<Option<String>> =
            trig_key_labels[..self.model.tracks.len().min(16)].to_vec();
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
        ]
        .into_iter()
        .filter_map(|b| key_label(&self.keymap, b).map(|k| (b, k)))
        .collect();

        let render_data = render::RenderData {
            screen: self.model.screen,
            rec: self.model.rec,
            armed_prefix: match self.held.armed {
                Some(input::Hold::Trk) => Some("TRK…".to_string()),
                Some(input::Hold::Ptn) => Some("PTN…".to_string()),
                // REC has its own three-state transport/status indicator
                // (D5) — it isn't an "armed prefix" chip like TRK/PTN.
                Some(input::Hold::Rec) | None => None,
            },
            active_track: self.model.active_track,
            track_names: self.model.tracks.iter().map(|t| t.name.clone()).collect(),
            display_names: self.model.tracks.iter().map(|t| t.display_name.clone()).collect(),
            trig_key_labels,
            track_key_labels,
            legend_key_labels,
            bpm,
            playing: self.model.playing(bus),
            page_window: self.model.page_windows[self.model.active_track],
            step_state,
            step_states,
            slot_a: self.model.slot_a.clone(),
            slot_a_value,
            slot_b: self.model.slot_b.clone(),
            slot_b_value,
            slot_c: self.model.slot_c.clone(),
            slot_c_value,
            page_groups: self.model.page_groups_for_active_track(),
            perf_page: self.model.perf_page,
            sub_page: self.model.sub_page,
            sub_page_count: self.model.page_sub_page_count(),
            envelope,
            live_env_level,
            live_lfo_phase,
            debug_event: self.last_debug_event.take(),
            step_focuses,
            step_locks,
            mute_states,
            slot_a_locked,
            slot_b_locked,
            slot_c_locked,
            cmdline: self.model.cmdline.clone(),
            cmdline_error: self.model.cmdline_error.clone(),
            cmdline_status: self.model.cmdline_status.clone(),
            cmdline_candidates: self.model.cmdline_candidates(),
            slot_a_flash: self.model.slot_flash[0].map_or(false, |t| {
                t.elapsed().as_millis() < self.tuning.flash_ms as u128
            }),
            slot_b_flash: self.model.slot_flash[1].map_or(false, |t| {
                t.elapsed().as_millis() < self.tuning.flash_ms as u128
            }),
            slot_c_flash: self.model.slot_flash[2].map_or(false, |t| {
                t.elapsed().as_millis() < self.tuning.flash_ms as u128
            }),
            help_visible: self.model.help_visible,
            encoder_cells,
            encoder_cursor: self.model.encoder_cursor,
            encoder_flash,
            kitty: self.held.kitty,
            pattern_bank_size: PATTERN_BANK_SIZE,
            active_pattern,
            cued_pattern,
            chain_len,
            page_loop,
            chain_cursor: self.model.chain_cursor,
        };

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
        for ev in key_events {
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
                            KeyEventKind::Repeat if button == PanelButton::Rec => true,
                            _ => self.held.on_kitty_press(button),
                        }
                    } else {
                        self.held.on_press(button)
                    };
                    if consumed {
                        dirty = true;
                        continue;
                    }

                    let held_for_resolution = HeldState {
                        kitty: self.held.kitty,
                        armed: armed_before,
                        pressed: Default::default(),
                    };
                    let screen_state = input::ScreenState {
                        screen: self.model.screen,
                        rec: self.model.rec,
                    };
                    button_to_action(&held_for_resolution, &screen_state, button, mods)
                }
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

            if !matches!(action, Action::Noop) || self.last_debug_event.is_some() {
                self.last_debug_event = Some(format!("{:?} → {:?}", ev, action));
            }

            // Clear any stale echo (a D9 out-of-range clamp, KIT's
            // "reserved" message, ...) once a genuinely different action
            // fires, so it can't persist indefinitely and mask a later,
            // more relevant one — e.g. a stray KIT press pinning "reserved
            // (kit)" over the screen forever (post-C6 hostile review). The
            // arms below that need a fresh echo (out-of-range
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
                    if let Some(step) = self.model.step_focus[track] {
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
                Action::ToggleStep { col } => {
                    let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                    let pw = self.model.page_windows[self.model.active_track];
                    let global_step = pw * GRID_STEPS + col;
                    self.model.last_step[self.model.active_track] = Some(global_step);
                    let outcome = action.execute(self.model.clock_id, seq_id, pw, playing);
                    match outcome {
                        Outcome::Command(cmd) => self.pending.push(cmd),
                        _ => {}
                    }
                }
                Action::Noop => {}
                Action::FocusStep => {
                    let track = self.model.active_track;
                    if self.model.step_focus[track].is_some() {
                        self.model.step_focus[track] = None;
                    } else if let Some(ls) = self.model.last_step[track] {
                        self.model.step_focus[track] = Some(ls);
                    }
                    dirty = true;
                }
                Action::ReleaseFocus => {
                    self.model.step_focus[self.model.active_track] = None;
                    dirty = true;
                }
                Action::ClearAllLocks => {
                    let track = self.model.active_track;
                    if let Some(step) = self.model.step_focus[track] {
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
                    if let Some(step) = self.model.step_focus[track] {
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
                // engine's defaults (note 60, velocity 0.5). A column past
                // the discovered track count is a silent no-op (no echo —
                // D3 also gives those columns no chip); order is
                // normative, selection lands before the trig command.
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
                    let params = self.model.resolve_encoder_params();
                    match params.get(col).cloned() {
                        None => {
                            self.model.cmdline_error = Some(format!("no encoder {}", col + 1));
                        }
                        Some((node_id, param_id, _name, min, max, stepped, _resolved)) => {
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
                                let range = max - min;
                                self.tuning.jog_step(range, held, mag)
                            };
                            let signed = match dir {
                                Dir::Next => delta,
                                Dir::Prev => -delta,
                            };
                            if let Some(step) = self.model.step_focus[track] {
                                let seq_id = self.model.tracks[track].sequencer_id;
                                let current = self
                                    .model
                                    .read_lock_value(state, seq_id, step, node_id, param_id)
                                    .unwrap_or_else(|| {
                                        self.model.read_param_value(state, node_id, param_id)
                                    });
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
                self.pending.push(paraclete_node_api::NodeCommand {
                    target_id: node_id,
                    type_id: paraclete_node_api::CMD_SET_PARAM,
                    arg0: param_id as i64,
                    arg1: value,
                });
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
                if let Some(step) = self.model.step_focus[track] {
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
                self.keymap = Keymap::load_startup();
                self.model.cmdline_status = Some(format!(
                    "loaded {} binding(s)",
                    self.keymap.bindings.len()
                ));
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
    use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    // §0 A2: REPORT_EVENT_TYPES alone yields no release events for text
    // keys (Tab, `p`, all trigs) — exactly the events TK2 C3's kitty
    // hold-chord branch (HeldState::on_kitty_press/release) needs. Without
    // the other two flags, TRK/PTN would arm on press and never receive
    // the release that disarms them.
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    )
    .map(|_| {})
    .map_err(|e| format!("kitty flags: {e}"))
}

fn pop_keyboard_flags() -> Result<(), String> {
    use crossterm::event::PopKeyboardEnhancementFlags;
    execute!(std::io::stdout(), PopKeyboardEnhancementFlags)
        .map(|_| {})
        .map_err(|e| format!("kitty flags pop: {e}"))
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
    use paraclete_view_assembly::{CompositeParam, CompositePage};
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
                }],
                envelopes: vec![],
                macros: vec![],
            }],
            chain: vec![],
            routes: vec![],
        }
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
            keymap: Keymap::default(),
            held: HeldState::new(false),
        }
    }

    fn kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// TK2.1 C1 (D7): `test_app` bypasses `TheotokosApp::new` (it runs
    /// `setup_keyboard_flags`, unusable headless in CI), so this asserts
    /// directly on the `startup_commands` seam `new` calls.
    #[test]
    fn new_pushes_clock_stop() {
        let commands = TheotokosApp::startup_commands(7);
        assert!(
            commands
                .iter()
                .any(|c| c.target_id == 7 && c.type_id == CMD_CLOCK_STOP),
            "startup must push a CMD_CLOCK_STOP for the clock node, so the \
             first command drain boots the instrument silent (D7)"
        );
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
            page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG")]),
            param_pages: Cow::Owned(vec![
                // Declared decay-then-tune below, but Rule order is
                // tune (slot 0) then decay (slot 1) — the reverse.
                (tune_id, PageRef {
                    page: Cow::Borrowed("TRIG"),
                    slot: 0,
                }),
                (decay_id, PageRef {
                    page: Cow::Borrowed("TRIG"),
                    slot: 1,
                }),
            ]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
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

        app.pending.clear();
        app.handle_keys(&bus, &[func_trig('w')]);
        assert!(
            app.pending.iter().any(|c| c.type_id
                == paraclete_node_api::CMD_BUMP_PARAM
                && c.arg0 == decay_id as i64),
            "encoder 2 must target the Rule's slot-1 param (decay)"
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
            Some("no encoder 4"),
            "must echo the out-of-range encoder index"
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
        app.model.step_focus[0] = Some(3);

        app.handle_keys(&bus, &[func_trig('q')]);
        assert_eq!(app.pending.len(), 2, "focused jog must emit target + lock");
        assert_eq!(app.pending[0].type_id, CMD_SET_LOCK_TARGET);
        assert_eq!(app.pending[1].type_id, CMD_SET_STEP_LOCK);
        assert_eq!(app.pending[1].arg0, 3, "step arg must be the focused step");
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
            unit: ParamUnit::Generic,
            display: None,
        });
        app.model.composite = vec![composite_view_with_param(100, param_id)];

        let params = app.model.resolve_encoder_params();
        assert_eq!(params.len(), 1);
        let (_, _, _, min, max, stepped, resolved) = params[0].clone();
        assert!(resolved, "must resolve against the cap-doc, not fall back");
        assert_eq!(min, 20.0);
        assert_eq!(max, 20000.0);
        assert!(!stepped);
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
            unit: ParamUnit::Generic,
            display: None,
        });
        app.model.composite = vec![composite_view_with_param(100, param_id)];

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
            unit: ParamUnit::Generic,
            display: None,
        });
        app.model.composite = vec![composite_view_with_param(100, param_id)];
        app.model.step_focus[0] = Some(0);
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

    #[test]
    fn kit_button_echoes_reserved() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        app.handle_keys(&bus, &[kc('7')]);
        assert_eq!(app.model.cmdline_error.as_deref(), Some("reserved (kit)"));
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
    /// Tempo, Param, and Mute were dead ends with no way back except
    /// quitting. Found live in the TK2 C7 agent smoke pass; NO doubles as
    /// the conventional "back" gesture everywhere it isn't already
    /// claimed (Chain's clear, tested separately, still wins there).
    #[test]
    fn esc_returns_to_grid_from_other_screens() {
        let bus = test_bus();
        for screen in [
            Screen::Settings,
            Screen::Tempo,
            Screen::Param(0),
            Screen::Mute,
        ] {
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

        app.model.step_focus[0] = Some(3);
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

        app.model.step_focus[0] = Some(5);
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
        let locks = "s2:100:500:0.300;s3:100:500:0.700;s0:200:600:0.100";
        assert_eq!(Model::parse_lock_value(locks, 2, 100, 500), Some(0.3));
        assert_eq!(Model::parse_lock_value(locks, 3, 100, 500), Some(0.7));
        assert_eq!(Model::parse_lock_value(locks, 0, 200, 600), Some(0.1));
    }

    #[test]
    fn parse_lock_value_returns_none_for_mismatch() {
        let locks = "s2:100:500:0.300";
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
        cmdline_type(&mut app, &bus, "bind w mute");
        app.handle_keys(&bus, &[enter_key()]);

        assert!(app.model.cmdline.is_none(), "cmdline closes on success");
        assert_eq!(
            crate::input::key_to_button(&app.keymap, kc('w')),
            Some(crate::input::PanelButton::Mute),
            "'w' must now resolve to Mute, not the default Trig2"
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
        cmdline_type(&mut app, &bus, "bind w mute");
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
                crate::input::PanelButton::Mute,
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
            cmdline_type(&mut app, &bus, "bind w mute");
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
                Some(crate::input::PanelButton::Mute),
                "load-bindings must restore the saved binding"
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
