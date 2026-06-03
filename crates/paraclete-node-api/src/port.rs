/// The signal type flowing through a port connection.
/// Enforced at connection time by the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PortType {
    /// Stereo or multi-channel audio buffer.
    Audio,
    /// Mono audio buffer (channels == 1).
    Mono,
    /// Audio-rate CV — single-sample capable. Enables feedback loops.
    Cv,
    /// Phase ramp — 0.0 to just below 1.0.
    Phase,
    /// Gate / trigger — 1.0 (high) / 0.0 (low).
    Logic,
    /// Semitone pitch CV.
    Pitch,
    /// Sub-audio modulation — LFOs, envelopes, automation.
    Modulation,
    /// Timestamped event stream.
    Event,
    /// Tempo and position (TransportInfo).
    Clock,
}

/// The direction of signal flow through a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PortDirection {
    Input,
    Output,
}

/// Hybrid static / dynamic port name.
/// Use `Static` for fixed ports (the common case).
/// Use `Dynamic` for nodes with runtime-determined ports (mixers, modular).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortName {
    Static(&'static str),
    Dynamic(String),
}

impl PortName {
    pub fn as_str(&self) -> &str {
        match self {
            PortName::Static(s) => s,
            PortName::Dynamic(s) => s.as_str(),
        }
    }
}

impl From<&'static str> for PortName {
    fn from(s: &'static str) -> Self {
        PortName::Static(s)
    }
}

impl std::fmt::Display for PortName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A node's declaration of a single port. Returned from `Node::ports()`.
#[derive(Clone, Debug)]
pub struct PortDescriptor {
    pub id: u32,
    pub name: PortName,
    pub direction: PortDirection,
    pub port_type: PortType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_name_static_as_str_returns_the_literal() {
        let name = PortName::Static("audio_in");
        assert_eq!(name.as_str(), "audio_in");
    }

    #[test]
    fn port_name_dynamic_as_str_returns_owned_string() {
        let name = PortName::Dynamic("channel_3".to_string());
        assert_eq!(name.as_str(), "channel_3");
    }

    #[test]
    fn port_name_display_matches_as_str() {
        let static_name = PortName::Static("out");
        assert_eq!(format!("{static_name}"), "out");

        let dynamic_name = PortName::Dynamic("dyn".into());
        assert_eq!(format!("{dynamic_name}"), "dyn");
    }

    #[test]
    fn port_name_from_static_str_produces_static_variant() {
        let name: PortName = "cv_in".into();
        assert!(matches!(name, PortName::Static("cv_in")));
    }

    #[test]
    fn port_descriptor_fields_are_accessible() {
        let desc = PortDescriptor {
            id: 7,
            name: "pitch".into(),
            direction: PortDirection::Input,
            port_type: PortType::Pitch,
        };
        assert_eq!(desc.id, 7);
        assert_eq!(desc.name.as_str(), "pitch");
        assert!(matches!(desc.direction, PortDirection::Input));
        assert!(matches!(desc.port_type, PortType::Pitch));
    }
}
