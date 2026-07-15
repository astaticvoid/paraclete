use crate::node::Node;

/// Clock domain priority for federation.
/// Higher values take precedence when multiple TempoSource nodes compete.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClockPriority {
    /// Polyrhythmic sub-sequencer, node-local.
    SubDomain = 0,
    /// TempoSource node — default standalone clock.
    Internal = 1,
    /// CV or MIDI clock input.
    ExternalHardware = 2,
    /// Ableton Link — future.
    AbletonLink = 3,
    /// DAW host transport (P7).
    DawHost = 4,
}

/// A node that provides a clock domain.
///
/// Implements `Node` for graph participation and this trait to declare
/// its clock authority and priority. The runtime federates between all
/// registered TempoSource nodes by priority ordering.
pub trait TempoSource: Node {
    /// The domain identifier assigned by the runtime at registration.
    /// Used to tag TransportEvent emissions so consumers know which domain
    /// they are receiving from.
    fn domain_id(&self) -> u32;

    /// Priority for clock federation. Higher priority wins when multiple
    /// TempoSource nodes are active simultaneously.
    fn priority(&self) -> ClockPriority;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_priority_ordering_daw_host_is_highest() {
        assert!(ClockPriority::DawHost > ClockPriority::AbletonLink);
        assert!(ClockPriority::AbletonLink > ClockPriority::ExternalHardware);
        assert!(ClockPriority::ExternalHardware > ClockPriority::Internal);
        assert!(ClockPriority::Internal > ClockPriority::SubDomain);
    }
}
