/// Current clock state at the start of a processing buffer.
/// Always present in `ProcessInput`. Clock domain changes *within* the buffer
/// arrive as `Event::Transport` in the event list.
#[derive(Clone, Copy, Debug)]
pub struct TransportInfo {
    pub domain_id: u32,
    pub bar: i32,
    pub beat: u32,
    pub tick: u32,
    /// For the internal clock domain this equals `TICKS_PER_BEAT`.
    /// For external domains this reflects the domain's actual resolution.
    pub ticks_per_beat: u32,
    pub bpm: f64,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub playing: bool,
    pub recording: bool,
    pub looping: bool,
}

impl Default for TransportInfo {
    fn default() -> Self {
        Self {
            domain_id: 0,
            bar: 1,
            beat: 0,
            tick: 0,
            ticks_per_beat: crate::TICKS_PER_BEAT,
            bpm: 120.0,
            time_sig_num: 4,
            time_sig_den: 4,
            playing: false,
            recording: false,
            looping: false,
        }
    }
}

// ── Transport events ──────────────────────────────────────────────────────────

/// Flags describing the state change carried by a `TransportEvent`.
///
#[derive(Clone, Copy, Debug, Default)]
pub struct TransportFlags {
    pub playing: bool,
    pub recording: bool,
    pub looping: bool,
    /// This event marks a loop-start boundary crossing.
    pub loop_start: bool,
    /// This event marks a loop-end boundary crossing.
    pub loop_end: bool,
    /// Master transport stop. All TempoSource nodes respond regardless of domain.
    pub global_stop: bool,
    /// Master transport start. All TempoSource nodes respond regardless of domain.
    pub global_start: bool,
    /// Full position sync pulse. Emitted every bar by the TempoSource node.
    /// Downstream nodes should snap their internal position to this event.
    pub sync_pulse: bool,
}

/// A clock domain state change occurring at a specific sample within a buffer.
/// Distinct from `TransportInfo`, which is the state at the *start* of the buffer.
///
#[derive(Clone, Copy, Debug)]
pub struct TransportEvent {
    pub domain_id: u32,
    pub bar: i32,
    pub beat: u32,
    pub tick: u32,
    pub ticks_per_beat: u32,
    pub bpm: f64,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub flags: TransportFlags,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICKS_PER_BEAT;

    #[test]
    fn ticks_per_beat_constant_is_960() {
        assert_eq!(TICKS_PER_BEAT, 960);
    }

    #[test]
    fn transport_info_default_bpm_is_120() {
        let t = TransportInfo::default();
        assert_eq!(t.bpm, 120.0);
    }

    #[test]
    fn transport_info_default_ticks_per_beat_matches_constant() {
        let t = TransportInfo::default();
        assert_eq!(t.ticks_per_beat, TICKS_PER_BEAT);
    }

    #[test]
    fn transport_info_default_is_stopped_at_bar_one() {
        let t = TransportInfo::default();
        assert!(!t.playing);
        assert!(!t.recording);
        assert_eq!(t.bar, 1);
        assert_eq!(t.beat, 0);
        assert_eq!(t.tick, 0);
    }

    #[test]
    fn transport_info_default_time_sig_is_4_4() {
        let t = TransportInfo::default();
        assert_eq!(t.time_sig_num, 4);
        assert_eq!(t.time_sig_den, 4);
    }
}
