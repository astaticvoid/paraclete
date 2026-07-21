use std::collections::HashMap;
use paraclete_node_api::{CapabilityDocument, PageRef, StateBusHandle, StateBusValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Seq,
    Perf,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Seq => Mode::Perf,
            Mode::Perf => Mode::Seq,
        }
    }
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

pub struct TrackInfo {
    pub sequencer_id: u32,
    pub generator_id: u32,
    pub name: String,
}

pub struct Model {
    pub mode: Mode,
    pub active_track: usize,
    pub tracks: Vec<TrackInfo>,
    pub clock_id: u32,
    pub page_windows: Vec<usize>,
    pub caps: HashMap<u32, CapabilityDocument>,
    pub perf_page: usize,
    pub slot_a: Option<SlotBinding>,
    pub slot_b: Option<SlotBinding>,
}

impl Model {
    pub fn new(
        clock_id: u32,
        seq_ids: &[u32],
        gen_ids: &[u32],
        gen_names: &[String],
        caps: HashMap<u32, CapabilityDocument>,
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
            })
            .collect();
        let page_windows: Vec<usize> = vec![0; tracks.len()];
        let mut model = Self {
            mode: Mode::Seq,
            active_track: 0,
            tracks,
            clock_id,
            page_windows,
            caps,
            perf_page: 0,
            slot_a: None,
            slot_b: None,
        };
        model.bind_page();
        model
    }

    pub fn cycle_mode(&mut self) {
        self.mode = self.mode.next();
    }

    pub fn select_track(&mut self, i: usize) {
        if i < self.tracks.len() {
            self.active_track = i;
            self.bind_page();
        }
    }

    pub fn select_perf_page(&mut self, idx: usize) {
        let gen_id = self.tracks[self.active_track].generator_id;
        let max = self.caps
            .get(&gen_id)
            .and_then(|c| c.view.as_ref())
            .map(|r| r.page_groups.len())
            .unwrap_or(0);
        if idx >= max {
            return;
        }
        self.perf_page = idx;
        self.bind_page();
    }

    fn bind_page(&mut self) {
        let (a, b) = self.resolve_page_params();
        self.slot_a = a.map(|(nid, pid, name, min, max)| SlotBinding {
            node_id: nid,
            param_id: pid,
            param_name: name,
            min,
            max,
        });
        self.slot_b = b.map(|(nid, pid, name, min, max)| SlotBinding {
            node_id: nid,
            param_id: pid,
            param_name: name,
            min,
            max,
        });
    }

    fn resolve_page_params(&self) -> (
        Option<(u32, u32, String, f64, f64)>,
        Option<(u32, u32, String, f64, f64)>,
    ) {
        let gen_id = self.tracks[self.active_track].generator_id;
        let cap = match self.caps.get(&gen_id) {
            Some(c) => c,
            None => return (None, None),
        };
        let rule = match &cap.view {
            Some(r) => r,
            None => return (None, None),
        };
        let page = match rule.page_groups.get(self.perf_page) {
            Some(p) => p.as_ref(),
            None => {
                let fallback_params = &cap.params;
                let a = fallback_params.first();
                let b = fallback_params.get(1);
                return (
                    a.map(|p| (gen_id, p.id, p.name.to_string(), p.min, p.max)),
                    b.map(|p| (gen_id, p.id, p.name.to_string(), p.min, p.max)),
                );
            }
        };

        let mut params: Vec<&(u32, PageRef)> = rule
            .param_pages
            .iter()
            .filter(|(_, pr)| pr.page.as_ref() == page)
            .collect();
        params.sort_by_key(|(_, pr)| pr.slot);
        let a = params.first().and_then(|(pid, _)| {
            cap.params
                .iter()
                .find(|pd| pd.id == *pid)
                .map(|pd| (gen_id, pd.id, pd.name.to_string(), pd.min, pd.max))
        });
        let b = params.get(1).and_then(|(pid, _)| {
            cap.params
                .iter()
                .find(|pd| pd.id == *pid)
                .map(|pd| (gen_id, pd.id, pd.name.to_string(), pd.min, pd.max))
        });
        (a, b)
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

    pub fn read_step_state(
        &self,
        bus: &StateBusHandle,
        track_idx: usize,
    ) -> StepState {
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

        let steps: Vec<bool> = steps_text
            .chars()
            .map(|c| c == '1')
            .collect();

        let page_count = pattern_length.div_ceil(8);

        StepState {
            current_step,
            pattern_length,
            steps,
            page_count,
        }
    }

    pub fn page_groups_for_active_track(&self) -> Vec<String> {
        let gen_id = self.tracks[self.active_track].generator_id;
        self.caps
            .get(&gen_id)
            .and_then(|c| c.view.as_ref())
            .map(|r| r.page_groups.iter().map(|g| g.to_string()).collect())
            .unwrap_or_default()
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
