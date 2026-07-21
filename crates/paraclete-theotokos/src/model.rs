use paraclete_node_api::{StateBusHandle, StateBusValue};

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
}

impl Model {
    pub fn new(clock_id: u32, seq_ids: &[u32], gen_ids: &[u32], gen_names: &[String]) -> Self {
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
        Self {
            mode: Mode::Seq,
            active_track: 0,
            tracks,
            clock_id,
            page_windows,
        }
    }

    pub fn cycle_mode(&mut self) {
        self.mode = self.mode.next();
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
}

#[derive(Default)]
pub struct StepState {
    pub current_step: usize,
    pub pattern_length: usize,
    pub steps: Vec<bool>,
    pub page_count: usize,
}
