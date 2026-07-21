mod action;
pub mod input;
mod model;
mod render;

use std::io::Stdout;
use std::rc::Rc;
use std::cell::RefCell;
use std::time::Instant;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use crossterm::execute;
use paraclete_node_api::{NodeCommand, StateBusHandle};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::action::{Action, Outcome};
use crate::model::Dir;

pub type BusHandle = Rc<RefCell<StateBusHandle>>;

pub struct TheotokosConfig {
    pub clock_id: u32,
    pub seq_ids: Vec<u32>,
    pub gen_ids: Vec<u32>,
    pub gen_names: Vec<String>,
    pub fps: u64,
}

pub struct TheotokosApp {
    model: model::Model,
    pending: Vec<NodeCommand>,
    quit: bool,
    dirty: bool,
    last_render: Instant,
    frame_ms: u64,
}

impl TheotokosApp {
    pub fn new(config: TheotokosConfig) -> Result<Self, String> {
        setup_keyboard_flags()?;

        let model = model::Model::new(
            config.clock_id,
            &config.seq_ids,
            &config.gen_ids,
            &config.gen_names,
        );

        let frame_ms = if config.fps > 0 { 1000 / config.fps } else { 33 };

        Ok(Self {
            model,
            pending: Vec::with_capacity(64),
            quit: false,
            dirty: true,
            last_render: Instant::now(),
            frame_ms,
        })
    }

    pub fn tick(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        bus: &BusHandle,
        _now_ms: u64,
    ) -> Result<(), String> {
        let bus_ref = bus.borrow();
        let bus = &*bus_ref;
        let playing = self.model.playing(bus);

        let mut actions: Vec<Action> = Vec::with_capacity(32);
        while event::poll(std::time::Duration::ZERO).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Key(ev) => {
                    if Self::is_press_or_repeat(ev) {
                        actions.push(input::map_key(self.model.mode, &ev));
                    }
                }
                Event::Resize(_, _) => {
                    self.dirty = true;
                }
                _ => {}
            }
        }

        for action in actions {
            match action {
                Action::Quit => self.quit = true,
                Action::CycleMode(_) => {
                    self.model.cycle_mode();
                    self.dirty = true;
                }
                Action::SelectTrack(i) if i < self.model.tracks.len() => {
                    self.model.active_track = i;
                    self.dirty = true;
                }
                Action::SelectTrack(_) => {
                    // out of range — no-op
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
                            if *pw < max_page.max(0) {
                                *pw += 1;
                            }
                        }
                    }
                    self.dirty = true;
                }
                Action::PlayToggle => {
                    let outcome = action.execute(
                        self.model.clock_id,
                        0,
                        0,
                        playing,
                    );
                    match outcome {
                        Outcome::Command(cmd) => self.pending.push(cmd),
                        Outcome::Quit => self.quit = true,
                        _ => {}
                    }
                }
                Action::ToggleStep { .. } => {
                    let seq_id = self.model.tracks[self.model.active_track].sequencer_id;
                    let pw = self.model.page_windows[self.model.active_track];
                    let outcome = action.execute(
                        self.model.clock_id,
                        seq_id,
                        pw,
                        playing,
                    );
                    match outcome {
                        Outcome::Command(cmd) => self.pending.push(cmd),
                        _ => {}
                    }
                }
                Action::Noop => {}
            }
        }

        let elapsed = self.last_render.elapsed().as_millis() as u64;
        if self.dirty || elapsed >= self.frame_ms {
            let step_state = self.model.read_step_state(bus, self.model.active_track);
            let bpm = self.model.read_bpm(bus);

            let render_data = render::RenderData {
                mode: self.model.mode,
                active_track: self.model.active_track,
                track_names: self.model.tracks.iter().map(|t| t.name.clone()).collect(),
                bpm,
                playing,
                page_window: self.model.page_windows[self.model.active_track],
                step_state,
            };

            drop(bus_ref);

            terminal
                .draw(|frame| render::render(frame, &render_data))
                .map_err(|e| e.to_string())?;

            self.dirty = false;
            self.last_render = Instant::now();
        } else {
            drop(bus_ref);
        }

        Ok(())
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

    fn is_press_or_repeat(ev: KeyEvent) -> bool {
        matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat)
    }
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
