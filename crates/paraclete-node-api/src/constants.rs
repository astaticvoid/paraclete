/// Internal canonical tick resolution.
/// All internal clock domains use this value.
/// External clock domains may use different resolutions —
/// `TransportInfo.ticks_per_beat` always reflects the actual resolution
/// of the current domain.
pub const TICKS_PER_BEAT: u32 = 960;
