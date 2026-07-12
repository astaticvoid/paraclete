use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TestScenario {
    pub format_version: u32,
    pub instrument: String,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: f32,
    #[serde(default = "default_block_size")]
    pub block_size: usize,
    pub duration_secs: f64,
    #[serde(default = "default_output")]
    pub output: String,
    #[serde(default = "default_play")]
    pub play: bool,
    #[serde(default)]
    pub timeline: Vec<TimelineEntry>,
    #[serde(default)]
    pub assert: Vec<Assertion>,
    #[serde(default)]
    pub probe: Vec<Probe>,
}

fn default_sample_rate() -> f32 { 44100.0 }
fn default_block_size() -> usize { 512 }
fn default_output() -> String { "/tmp/paraclete_test.wav".into() }
fn default_play() -> bool { true }

#[derive(Debug, Deserialize)]
pub struct TimelineEntry {
    pub at: f64,
    #[serde(flatten)]
    pub action: TimelineAction,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineAction {
    SetParam { target: String, param: String, value: f64 },
    BumpParam { target: String, param: String, delta: f64 },
    Trigger { target: String, #[serde(default = "default_note")] note: i64, #[serde(default = "default_velocity")] velocity: f64 },
    ToggleStep { target: String, step: i64 },
    SetStep { target: String, step: i64, note: i64 },
    Clear { target: String },
    SetPattern { target: String, pattern: i64 },
    SetLength { target: String, steps: i64 },
    SetSpeed { target: String, speed: f64 },
    SetPageLoop { target: String, start_page: i64, end_page: i64 },
    SetStepTiming { target: String, step: i64, micro_offset: i64 },
    SetFillA { target: String, active: bool },
    SetFillB { target: String, active: bool },
    SetStepCondition { target: String, step: i64, probability: u8, repeat_n: u8, repeat_m: u8, fill: u8 },
    ChainPush { target: String, pattern: i64 },
    ChainClear { target: String },
}

#[derive(Debug)]
pub enum ResolvedActionKind {
    SetParam { target_id: u32, param_name: String, value: f64 },
    BumpParam { target_id: u32, param_name: String, delta: f64 },
    Trigger { target_id: u32, note: i64, velocity: f64 },
    ToggleStep { target_id: u32, step: i64 },
    SetStep { target_id: u32, step: i64, note: i64 },
    Clear { target_id: u32 },
    SetPattern { target_id: u32, pattern: i64 },
    SetLength { target_id: u32, steps: i64 },
    SetSpeed { target_id: u32, speed: f64 },
    SetPageLoop { target_id: u32, start_page: i64, end_page: i64 },
    SetStepTiming { target_id: u32, step: i64, micro_offset: i64 },
    SetFillA { target_id: u32, active: bool },
    SetFillB { target_id: u32, active: bool },
    SetStepCondition { target_id: u32, step: i64, probability: u8, repeat_n: u8, repeat_m: u8, fill: u8 },
    ChainPush { target_id: u32, pattern: i64 },
    ChainClear { target_id: u32 },
}

fn default_velocity() -> f64 { 0.79 }

// CMD_TRIGGER contract (ADR-033): arg0 < 0 means "engine default note".
// A plain 0 would be a valid MIDI note and retune the voice (BUG-028).
fn default_note() -> i64 { -1 }

#[derive(Debug, Deserialize)]
pub struct Assertion {
    // Live assertions (state bus / peak) fire once `at` seconds elapse.
    // Artifact assertions ignore `at`: they scan the captured buffer after
    // the render completes, windowed by `from`/`until` (seconds; defaults
    // to the whole capture).
    #[serde(default)]
    pub at: f64,
    pub path: Option<String>,
    pub eq: Option<f64>,
    pub between: Option<[f64; 2]>,
    pub peak_gte: Option<f64>,
    pub peak_lt: Option<f64>,
    pub window_ms: Option<f64>,
    pub discontinuity_lt: Option<f64>,
    pub dc_offset_lt: Option<f64>,
    pub dropout_lt_ms: Option<f64>,
    pub from: Option<f64>,
    pub until: Option<f64>,
}

impl Assertion {
    pub fn has_artifact_check(&self) -> bool {
        self.discontinuity_lt.is_some()
            || self.dc_offset_lt.is_some()
            || self.dropout_lt_ms.is_some()
    }
}

#[derive(Debug, Deserialize)]
pub struct Probe {
    pub at: f64,
    pub path: String,
}

pub fn parse_scenario(yaml: &str) -> Result<TestScenario, String> {
    let scenario: TestScenario = serde_yml::from_str(yaml)
        .map_err(|e| format!("failed to parse test scenario: {}", e))?;
    if scenario.format_version != 1 {
        return Err(format!("unsupported format_version: {}", scenario.format_version));
    }
    Ok(scenario)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_without_note_defaults_to_engine_default() {
        let yaml = r#"
format_version: 1
instrument: instrument.yaml
duration_secs: 1
timeline:
  - at: 0.5
    trigger: { target: kick, velocity: 1.0 }
"#;
        let s = parse_scenario(yaml).unwrap();
        match &s.timeline[0].action {
            TimelineAction::Trigger { note, .. } => {
                assert_eq!(*note, -1, "omitted note must mean engine default (< 0), not note 0");
            }
            other => panic!("expected trigger action, got {:?}", other),
        }
    }
}
