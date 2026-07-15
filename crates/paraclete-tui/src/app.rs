// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use paraclete_node_api::{CapabilityDocument, StateBusHandle, StateBusValue};

use crate::layout;
use crate::state::{EncoderSlot, TuiState};
use crate::{TuiConfig, TuiError};

pub struct TuiApp {
    pub state: TuiState,
    bus: Rc<RefCell<StateBusHandle>>,
    config: TuiConfig,
    cap_docs: HashMap<u32, CapabilityDocument>,
}

impl TuiApp {
    pub fn new(
        bus: Rc<RefCell<StateBusHandle>>,
        config: TuiConfig,
        cap_docs: HashMap<u32, CapabilityDocument>,
    ) -> Self {
        let encoder_count = config.encoder_count as usize;
        let state = TuiState {
            encoders: (0..encoder_count).map(|_| EncoderSlot::default()).collect(),
            ..Default::default()
        };
        Self {
            state,
            bus,
            config,
            cap_docs,
        }
    }

    pub fn tick(
        &mut self,
        terminal: &mut ratatui::Terminal<impl ratatui::backend::Backend>,
    ) -> Result<(), TuiError> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.tick_with_time(terminal, now_ms)
    }

    pub fn tick_with_time(
        &mut self,
        terminal: &mut ratatui::Terminal<impl ratatui::backend::Backend>,
        now_ms: u64,
    ) -> Result<(), TuiError> {
        let bus = self.bus.borrow();

        if let Some(StateBusValue::Float(v)) = bus.read("/transport/bpm") {
            if *v != self.state.bpm {
                self.state.bpm = *v;
                self.state.dirty = true;
            }
        }

        if let Some(StateBusValue::Bool(b)) = bus.read("/transport/playing") {
            if *b != self.state.playing {
                self.state.playing = *b;
                self.state.dirty = true;
            }
        }

        let active_track = if let Some(StateBusValue::Float(v)) = bus.read("/script/selected_track")
        {
            (*v as usize).min(self.config.seq_ids.len().saturating_sub(1))
        } else {
            0
        };
        if active_track != self.state.active_track {
            self.state.active_track = active_track;
            self.state.dirty = true;
        }

        if let Some(&seq_id) = self.config.seq_ids.get(self.state.active_track) {
            let step_path = format!("/node/{}/state/current_step", seq_id);
            if let Some(StateBusValue::Int(v)) = bus.read(&step_path) {
                let step = *v as u8;
                if step != self.state.current_step {
                    self.state.current_step = step;
                    self.state.dirty = true;
                }
            }

            // P10 C5 pattern-engine surface: pattern/page/speed indicator
            // sources, all published by Sequencer::published_state().
            let read_int = |key: &str| -> Option<i64> {
                let path = format!("/node/{}/state/{}", seq_id, key);
                match bus.read(&path) {
                    Some(StateBusValue::Int(v)) => Some(*v),
                    _ => None,
                }
            };
            let set_usize = |cur: usize, v: Option<i64>| -> (usize, bool) {
                match v {
                    Some(v) if (v.max(0) as usize) != cur => ((v.max(0) as usize), true),
                    _ => (cur, false),
                }
            };
            let (v, d) = set_usize(self.state.pattern_length, read_int("pattern_length"));
            self.state.pattern_length = v;
            self.state.dirty |= d;
            let (v, d) = set_usize(self.state.active_pattern, read_int("active_pattern"));
            self.state.active_pattern = v;
            self.state.dirty |= d;
            let (v, d) = set_usize(self.state.current_page, read_int("current_page"));
            self.state.current_page = v;
            self.state.dirty |= d;
            let (v, d) = set_usize(self.state.page_count, read_int("page_count"));
            self.state.page_count = v;
            self.state.dirty |= d;
            if let Some(v) = read_int("cued_pattern") {
                if v != self.state.cued_pattern {
                    self.state.cued_pattern = v;
                    self.state.dirty = true;
                }
            }
            let speed_path = format!("/node/{}/state/speed_mult", seq_id);
            if let Some(StateBusValue::Float(v)) = bus.read(&speed_path) {
                if *v != self.state.speed_mult {
                    self.state.speed_mult = *v;
                    self.state.dirty = true;
                }
            }

            // The row shows the 16-step window containing the playhead
            // (patterns reach 64 steps at P10; /state/steps is the full
            // pattern by convention — consumers slice).
            let window_base = (self.state.current_step as usize / 16) * 16;
            if window_base != self.state.window_base {
                self.state.window_base = window_base;
                self.state.dirty = true;
            }
            let steps_path = format!("/node/{}/state/steps", seq_id);
            if let Some(StateBusValue::Text(s)) = bus.read(&steps_path) {
                let mut new_steps = [false; 16];
                for (i, ch) in s.chars().skip(window_base).enumerate().take(16) {
                    new_steps[i] = ch == '1';
                }
                if new_steps != self.state.steps {
                    self.state.steps = new_steps;
                    self.state.dirty = true;
                }
            }
        }

        for i in 0..self.config.encoder_count as usize {
            let node_path = format!("/context/encoder_{}/node", i);
            let param_path = format!("/context/encoder_{}/param", i);

            let node_id = match bus.read(&node_path) {
                Some(StateBusValue::Float(v)) => *v as u32,
                _ => continue,
            };
            let param_id = match bus.read(&param_path) {
                Some(StateBusValue::Float(v)) => *v as u32,
                _ => continue,
            };

            let (label, min, max) = if let Some(doc) = self.cap_docs.get(&node_id) {
                if let Some(p) = doc.params.iter().find(|p| p.id == param_id) {
                    (p.name.as_str().to_string(), p.min, p.max)
                } else {
                    (String::new(), 0.0, 1.0)
                }
            } else {
                (String::new(), 0.0, 1.0)
            };

            let value_path = format!("/node/{}/param/{}", node_id, label);
            let value = match bus.read(&value_path) {
                Some(StateBusValue::Float(v)) => *v,
                _ => 0.0,
            };

            if let Some(slot) = self.state.encoders.get_mut(i) {
                let changed = slot.node_id != node_id
                    || slot.param_id != param_id
                    || (slot.value - value).abs() > f64::EPSILON;

                slot.node_id = node_id;
                slot.param_id = param_id;
                slot.label = label;
                slot.min = min;
                slot.max = max;

                if changed {
                    slot.value = value;
                    slot.recently_changed = true;
                    slot.changed_at_ms = now_ms;
                    self.state.dirty = true;
                } else if slot.recently_changed && now_ms.saturating_sub(slot.changed_at_ms) > 500 {
                    slot.recently_changed = false;
                    self.state.dirty = true;
                }
            }
        }

        drop(bus);

        if self.state.dirty {
            terminal
                .draw(|f| layout::render(f, &self.state))
                .map_err(|e| TuiError::Draw(e.to_string()))?;
            self.state.dirty = false;
        }

        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), TuiError> {
        use crossterm::{
            execute,
            terminal::{disable_raw_mode, LeaveAlternateScreen},
        };
        disable_raw_mode()?;
        execute!(std::io::stdout(), LeaveAlternateScreen)?;
        Ok(())
    }
}
