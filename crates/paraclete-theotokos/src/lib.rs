mod action;
pub mod input;
pub mod model;
mod render;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Stdout;
use std::rc::Rc;
use std::time::Instant;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use crossterm::execute;
use paraclete_node_api::{CapabilityDocument, NodeCommand, StateBusHandle};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::action::{Action, Outcome};
use crate::model::{Dir, JogTracker, Model, Slot, Tuning};

pub type BusHandle = Rc<RefCell<StateBusHandle>>;

pub struct TheotokosConfig {
    pub clock_id: u32,
    pub seq_ids: Vec<u32>,
    pub gen_ids: Vec<u32>,
    pub gen_names: Vec<String>,
    pub caps: HashMap<u32, CapabilityDocument>,
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

        let envelope = self.model.envelope_for_active_track().map(|e| {
            let val = self.model.read_param_value(bus, e.node_id, e.param_id);
            (e, val)
        });

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
        let bus = &*bus_ref;
        let playing = self.model.playing(bus);
        let now = Instant::now();
        let tick_ms = now.elapsed().as_millis() as u64;

        let mut dirty = false;
        for ev in key_events {
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
                }
                Action::PageWindow(dir) => {
                    let max_page = self
                        .model
                        .read_step_state(bus, self.model.active_track)
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
                Action::PlayToggle => {
                    let outcome = action.execute(self.model.clock_id, 0, 0, playing);
                    match outcome {
                        Outcome::Command(cmd) => self.pending.push(cmd),
                        Outcome::Quit => self.quit = true,
                        _ => {}
                    }
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
            }
        }
        drop(bus_ref);
        dirty
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
    use crossterm::event::{KeyCode, KeyModifiers};
    use paraclete_node_api::{CapabilityDocument, ParamDescriptor, ParamUnit};
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
        caps.insert(
            100,
            CapabilityDocument {
                name: "Engine".into(),
                vendor: "test".into(),
                version: (0, 1, 0),
                ports: vec![],
                params: vec![ParamDescriptor {
                    id: ParamDescriptor::id_for_name("decay"),
                    name: "decay".into(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    stepped: false,
                    unit: ParamUnit::Generic,
                    display: None,
                }],
                extensions: vec![],
                view: None,
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
            model: Model::new(clock_id, &seq_ids, &gen_ids, &gen_names, test_caps()),
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
    fn bracket_right_increments_page_window() {
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
        app.handle_keys(&bus, &[kc(']')]);
        assert_eq!(app.model.page_windows[0], 1, "']' must advance to page 2");
    }

    #[test]
    fn bracket_left_clamps_at_zero() {
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

        app.handle_keys(&bus, &[kc('[')]);
        assert_eq!(
            app.model.page_windows[0], 0,
            "'[' clamped at zero must stay 0"
        );
    }

    #[test]
    fn bracket_right_clamps_at_page_count() {
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
        app.handle_keys(&bus, &[kc(']')]);
        assert_eq!(
            app.model.page_windows[0], 2,
            "']' must not exceed page count"
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
}
