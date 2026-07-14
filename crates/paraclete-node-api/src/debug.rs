/// A structured debug event emitted by a node during `process()`. POD, `Copy`,
/// stack-only — no heap allocation on the audio thread.
///
/// `sample_offset` and `node_id` are filled by the executor after `process()`
/// returns; nodes set `kind`, `arg0`, and `arg1` via `ProcessOutput::emit_debug`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugEvent {
    pub sample_offset: u32,
    pub node_id: u32,
    pub kind: DebugEventKind,
    pub arg0: i64,
    pub arg1: f64,
}

/// Kinds of debug events a node can emit.
///
/// `#[non_exhaustive]` — new kinds can be added without breaking consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
#[non_exhaustive]
pub enum DebugEventKind {
    StepFired     = 1,
    VoiceTrigger  = 2,
    ParamChange   = 3,
}

impl DebugEventKind {
    /// Return the canonical snake_case name for JSON/log output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StepFired    => "step_fired",
            Self::VoiceTrigger => "voice_trigger",
            Self::ParamChange  => "param_change",
        }
    }
}
