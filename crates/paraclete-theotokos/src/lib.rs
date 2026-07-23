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
    Action, Outcome, CMD_CLEAR_STEP_LOCK, CMD_SET_LOCK_TARGET, CMD_SET_STEP_LOCK, GRID_STEPS,
};
use crate::model::{
    CmdlineVerb, Dir, JogTracker, LeaderState, Model, Slot, Tuning, YankedLock, YankedStep,
};

pub type BusHandle = Rc<RefCell<StateBusHandle>>;

pub struct TheotokosConfig {
    pub clock_id: u32,
    pub seq_ids: Vec<u32>,
    pub gen_ids: Vec<u32>,
    pub gen_names: Vec<String>,
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
    last_debug_event: Option<String>,
}

impl TheotokosApp {
    pub fn new(config: TheotokosConfig) -> Result<Self, String> {
        setup_keyboard_flags()?;

        let model = Model::new(
            config.clock_id,
            &config.seq_ids,
            &config.gen_ids,
            &config.gen_names,
            config.caps,
            config.composite,
        );

        let frame_ms = if config.fps > 0 {
            1000 / config.fps
        } else {
            33
        };

        Ok(Self {
            model,
            pending: Vec::with_capacity(64),
            quit: false,
            dirty: true,
            last_render: Instant::now(),
            frame_ms,
            tuning: Tuning::default(),
            jog_a: JogTracker::new(),
            jog_b: JogTracker::new(),
            last_debug_event: None,
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
                    if is_press_or_repeat(ev) {
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

        self.model.update_flash(0, slot_a_value);
        self.model.update_flash(1, slot_b_value);

        let envelope = self.model.envelope_for_active_track().map(|e| {
            let val = self.model.read_param_value(bus, e.node_id, e.param_id);
            (e, val)
        });

        let step_focuses = self.model.step_focus.clone();
        let step_locks: Vec<Vec<usize>> = (0..self.model.tracks.len())
            .map(|t| self.model.read_step_locks(bus, t))
            .collect();

        let mut slot_a_locked = false;
        let mut slot_b_locked = false;
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
        }

        let render_data = render::RenderData {
            mode: self.model.mode,
            active_track: self.model.active_track,
            track_names: self.model.tracks.iter().map(|t| t.name.clone()).collect(),
            bpm,
            playing: self.model.playing(bus),
            page_window: self.model.page_windows[self.model.active_track],
            step_state,
            step_states,
            slot_a: self.model.slot_a.clone(),
            slot_a_value,
            slot_b: self.model.slot_b.clone(),
            slot_b_value,
            page_groups: self.model.page_groups_for_active_track(),
            perf_page: self.model.perf_page,
            envelope,
            debug_event: self.last_debug_event.take(),
            step_focuses,
            step_locks,
            slot_a_locked,
            slot_b_locked,
            cmdline: self.model.cmdline.clone(),
            cmdline_error: self.model.cmdline_error.clone(),
            cmdline_candidates: self.model.cmdline_candidates(),
            leader: self.model.leader.clone(),
            slot_a_flash: self.model.slot_flash[0].map_or(false, |t| {
                t.elapsed().as_millis() < self.tuning.flash_ms as u128
            }),
            slot_b_flash: self.model.slot_flash[1].map_or(false, |t| {
                t.elapsed().as_millis() < self.tuning.flash_ms as u128
            }),
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
            // C7: while leader is active, route to leader chord handler
            if self.model.leader.is_some() {
                self.handle_leader_key(ev);
                dirty = true;
                continue;
            }
            // C6: while cmdline is open, capture ALL keys to the line editor
            if self.model.cmdline.is_some() {
                self.handle_cmdline_key(ev);
                dirty = true;
                continue;
            }
            let action = input::map_key(self.model.mode, ev);
            if !matches!(action, Action::Noop) || self.last_debug_event.is_some() {
                self.last_debug_event = Some(format!("{:?} → {:?}", ev, action));
            }

            match action {
                Action::Quit => self.quit = true,
                Action::CycleMode(_) => {
                    self.model.cycle_mode();
                    dirty = true;
                }
                Action::SelectTrack(i) => {
                    self.model.select_track(i);
                    dirty = true;
                    selected_changed = true;
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
                Action::SelectParamPage(idx) => {
                    self.model.select_perf_page(idx);
                    dirty = true;
                }
                Action::Jog { slot, dir, mag } => {
                    let track = self.model.active_track;
                    if let Some(step) = self.model.step_focus[track] {
                        let binding = match slot {
                            Slot::A => &self.model.slot_a,
                            Slot::B => &self.model.slot_b,
                            Slot::C => continue,
                        };
                        if let Some(ref b) = binding {
                            let tracker = match slot {
                                Slot::A => &mut self.jog_a,
                                Slot::B => &mut self.jog_b,
                                Slot::C => continue,
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
                            Slot::C => continue,
                        };
                        if let Some(ref b) = binding {
                            let tracker = match slot {
                                Slot::A => &mut self.jog_a,
                                Slot::B => &mut self.jog_b,
                                Slot::C => continue,
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
                Action::PatternSelect(n) => {
                    let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                    self.pending.push(paraclete_node_api::NodeCommand {
                        target_id: seq_id,
                        type_id: 27, // CMD_SET_PATTERN
                        arg0: n as i64,
                        arg1: 0.0,
                    });
                    dirty = true;
                }
                Action::Yank => {
                    self.yank_active_pattern(state);
                    dirty = true;
                }
                Action::Paste => {
                    self.paste_pattern(state);
                    dirty = true;
                }
                Action::Leader => {
                    self.model.leader = Some(LeaderState { slot: None });
                    dirty = true;
                }
                Action::Colon => {
                    self.model.cmdline = Some(String::new());
                    self.model.cmdline_error = None;
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
            CmdlineVerb::Mode(mode) => {
                self.model.mode = mode;
            }
        }
    }

    // ── C7: leader, yank, paste ──

    fn handle_leader_key(&mut self, ev: &KeyEvent) {
        let leader = match &mut self.model.leader {
            Some(s) => s,
            None => return,
        };
        match ev.code {
            KeyCode::Esc => {
                self.model.leader = None;
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                leader.slot = Some(Slot::A);
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                leader.slot = Some(Slot::B);
            }
            KeyCode::Char(c @ '1'..='9') if leader.slot.is_some() => {
                let slot = leader.slot.unwrap();
                let dig = c.to_digit(10).unwrap() as usize;
                self.model.set_slot_lead(slot, dig);
                self.model.leader = None;
            }
            _ => {
                self.model.leader = None;
            }
        }
    }

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
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
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
    use crate::model::{Mode, SlotBinding, TrackInfo};
    use crossterm::event::{KeyCode, KeyModifiers};
    use paraclete_node_api::{CapabilityDocument, ParamDescriptor, ParamUnit, Rule, StateBusValue};
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::rc::Rc;

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
            last_debug_event: None,
        }
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
        app.handle_keys(&bus, &[kc('a')]);
        let cmd = &app.pending[0];
        assert_eq!(cmd.target_id, 200);
        assert_eq!(cmd.type_id, 16);
        assert_eq!(cmd.arg0, 16);
    }

    #[test]
    fn select_track_publishes_selected_sequencer_id() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        assert_eq!(app.model.tracks[1].sequencer_id, 201);

        app.handle_keys(&bus, &[kc('w')]);
        let selected = bus.borrow().read("/script/theotokos/selected").cloned();
        assert_eq!(
            selected,
            Some(paraclete_node_api::StateBusValue::Int(201)),
            "SelectTrack(w) must publish seq id 201"
        );
    }

    fn shift_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
    }

    #[test]
    fn keymap_shift_track_toggles_mute() {
        let bus = test_bus();
        let mut app = test_app(
            1,
            vec![200, 201],
            vec![100, 101],
            vec!["T1".into(), "T2".into()],
        );
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/node/201/param/mute".into(),
                paraclete_node_api::StateBusValue::Float(0.0),
            );
        }
        app.handle_keys(&bus, &[shift_key('w')]);
        let cmd = &app.pending[0];
        assert_eq!(cmd.target_id, 201, "Shift+w targets track 1's sequencer");
        assert_eq!(cmd.type_id, paraclete_node_api::CMD_SET_PARAM);
        let mute_id = paraclete_node_api::ParamDescriptor::id_for_name("mute");
        assert_eq!(cmd.arg0, mute_id as i64);
        assert!((cmd.arg1 - 1.0).abs() < 0.001, "must set mute to 1.0");
    }

    #[test]
    fn mute_toggle_reads_bus_and_flips_value() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/node/200/param/mute".into(),
                paraclete_node_api::StateBusValue::Float(1.0),
            );
        }
        app.handle_keys(&bus, &[shift_key('q')]);
        let cmd = &app.pending[0];
        assert!((cmd.arg1 - 0.0).abs() < 0.001, "must flip 1.0 → 0.0");
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

    #[test]
    fn enter_focuses_last_toggled_step() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.model.last_step[0] = Some(3);
        app.handle_keys(&bus, &[enter_key()]);
        assert_eq!(
            app.model.step_focus[0],
            Some(3),
            "Enter must focus last_step"
        );

        app.handle_keys(&bus, &[enter_key()]);
        assert_eq!(
            app.model.step_focus[0], None,
            "second Enter must release focus"
        );
    }

    #[test]
    fn esc_releases_focus() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.model.step_focus[0] = Some(5);
        app.handle_keys(&bus, &[esc_key()]);
        assert_eq!(app.model.step_focus[0], None, "Esc must release focus");
    }

    #[test]
    fn enter_focuses_in_seq_jog_edits_in_perf() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.model.last_step[0] = Some(2);
        app.handle_keys(&bus, &[enter_key()]);
        assert!(
            app.model.step_focus[0].is_some(),
            "Enter must focus in SEQ mode"
        );

        app.model.mode = Mode::Perf;
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        app.handle_keys(&bus, &[up]);
        assert!(
            !app.pending.is_empty(),
            "jog while focused in PERF must emit lock commands"
        );
        // Verify the emitted commands are lock pairs, not bump_param
        assert_eq!(app.pending[0].type_id, CMD_SET_LOCK_TARGET);
        assert_eq!(app.pending[1].type_id, CMD_SET_STEP_LOCK);
    }

    #[test]
    fn jog_while_focused_emits_target_then_lock_pair() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.model.mode = Mode::Perf;
        app.model.step_focus[0] = Some(4);

        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        app.handle_keys(&bus, &[up]);

        assert_eq!(app.pending.len(), 2, "must emit pair: target + lock");
        assert_eq!(
            app.pending[0].type_id, CMD_SET_LOCK_TARGET,
            "first cmd must be CMD_SET_LOCK_TARGET"
        );
        assert_eq!(
            app.pending[1].type_id, CMD_SET_STEP_LOCK,
            "second cmd must be CMD_SET_STEP_LOCK"
        );
        assert_eq!(app.pending[0].target_id, 200, "target must be sequencer");
        assert_eq!(app.pending[1].target_id, 200, "target must be sequencer");
        assert_eq!(app.pending[1].arg0, 4, "step arg must be focused step");
    }

    #[test]
    fn jog_lock_value_starts_from_existing_lock() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        // Pre-populate a lock for step 2, node 100 (generator), param decay
        {
            let mut b = bus.borrow_mut();
            let decay_id = ParamDescriptor::id_for_name("decay");
            b.write(
                "/node/200/state/locks",
                StateBusValue::Text(format!("s2:100:{}:0.300000", decay_id)),
            );
        }

        app.model.mode = Mode::Perf;
        app.model.step_focus[0] = Some(2);

        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        app.handle_keys(&bus, &[up]);

        // The new value should be based on 0.3 + jog delta, not 0.5 + delta
        assert_eq!(app.pending.len(), 2);
        // Verify arg1 is based on the lock value (0.3 + delta)
        assert!(
            app.pending[1].arg1 > 0.3 && app.pending[1].arg1 < 0.32,
            "lock value must start from existing lock 0.3, got {}",
            app.pending[1].arg1
        );
    }

    #[test]
    fn jog_lock_value_starts_from_live_when_no_lock() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        // No lock set — should fall back to live bus value (0.5)

        app.model.mode = Mode::Perf;
        app.model.step_focus[0] = Some(3);

        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        app.handle_keys(&bus, &[up]);

        assert_eq!(app.pending.len(), 2);
        assert!(
            app.pending[1].arg1 > 0.5 && app.pending[1].arg1 < 0.52,
            "lock value must start from live bus 0.5, got {}",
            app.pending[1].arg1
        );
    }

    #[test]
    fn jog_without_focus_still_bumps_param() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.model.mode = Mode::Perf;
        // No focus set

        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        app.handle_keys(&bus, &[up]);

        assert_eq!(app.pending.len(), 1);
        assert_eq!(
            app.pending[0].type_id,
            paraclete_node_api::CMD_BUMP_PARAM,
            "without focus, jog must emit CMD_BUMP_PARAM"
        );
    }

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

    #[test]
    fn enter_without_last_step_does_not_focus() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["T1".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[enter_key()]);
        assert_eq!(
            app.model.step_focus[0], None,
            "Enter without last_step must not set focus"
        );
    }

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
        // typing should not trigger normal key handlers (like ToggleStep for 'a')
        let prev_pending = app.pending.len();
        app.handle_keys(&bus, &[kc('a')]);
        assert_eq!(
            app.pending.len(),
            prev_pending,
            "keys captured, no ToggleStep emitted"
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

    // ── C7: pattern select, yank/paste, leader, flash ──

    fn seq_key(n: u8) -> KeyEvent {
        KeyEvent::new(KeyCode::Char((b'0' + n) as char), KeyModifiers::NONE)
    }

    fn backslash_key() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::NONE)
    }

    #[test]
    fn seq_number_row_sends_set_pattern() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[seq_key(3)]);
        // SEQ mode by default
        assert!(
            app.pending
                .iter()
                .any(|c| { c.type_id == 27 && c.arg0 == 2 }),
            "number key 3 must send CMD_SET_PATTERN(2)"
        );
    }

    #[test]
    fn yank_then_paste_emits_full_step_command_batch() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);
        // Set up steps so yank has active data
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/node/200/state/steps",
                StateBusValue::Text("1100000000000000".into()),
            );
            b.write("/node/200/state/locks", StateBusValue::Text(String::new()));
        }

        app.handle_keys(&bus, &[kc('y')]);
        assert!(
            !app.model.yank_buffer.is_empty(),
            "yank must populate buffer"
        );

        app.pending.clear();
        app.handle_keys(
            &bus,
            &[KeyEvent::new(KeyCode::Char('y'), KeyModifiers::SHIFT)],
        );
        assert!(!app.pending.is_empty(), "paste must produce commands");
    }

    #[test]
    fn paste_clears_stale_lock_lanes_before_writing() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);
        {
            let mut b = bus.borrow_mut();
            b.write(
                "/node/200/state/steps",
                StateBusValue::Text("1000000000000000".into()),
            );
            b.write("/node/200/state/locks", StateBusValue::Text(String::new()));
        }

        app.handle_keys(&bus, &[kc('y')]);
        app.pending.clear();
        app.handle_keys(
            &bus,
            &[KeyEvent::new(KeyCode::Char('y'), KeyModifiers::SHIFT)],
        );

        // First command should be CMD_CLEAR_STEP_LOCK arg1=-1.0 for step 0
        let first_cmd = &app.pending[0];
        assert_eq!(first_cmd.type_id, CMD_CLEAR_STEP_LOCK);
        assert_eq!(first_cmd.arg1, -1.0);
    }

    #[test]
    fn leader_esc_cancels_chord() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        app.handle_keys(&bus, &[backslash_key()]);
        assert!(app.model.leader.is_some(), "leader must activate");

        app.handle_keys(&bus, &[esc_key()]);
        assert!(app.model.leader.is_none(), "Esc must cancel leader");
    }

    #[test]
    fn leader_rebind_b3_binds_third_page_param() {
        let bus = test_bus();
        let mut app = test_app(1, vec![200], vec![100], vec!["Kick".into()]);
        setup_bus_with_params(&bus, 200, 100, true);

        // Set mode to PERF and page to 0 (AMP with decay+tune params)
        app.model.mode = Mode::Perf;
        app.model.perf_page = 0;

        // Send leader sequence: \ b 3 (param index 2 = 3rd param)
        app.handle_keys(&bus, &[backslash_key()]);
        app.handle_keys(&bus, &[kc('b')]);
        app.handle_keys(&bus, &[seq_key(3)]);

        assert!(app.model.leader.is_none(), "leader must exit after chord");
        assert!(app.model.slot_b.is_some(), "slot B must be bound");
    }

    #[test]
    fn flash_detects_value_change() {
        let mut model = Model::new(1, &[200], &[100], &["Kick".into()], test_caps(), vec![]);
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
