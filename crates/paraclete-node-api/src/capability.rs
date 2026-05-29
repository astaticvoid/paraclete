use crate::port::{PortDescriptor, PortName};

// ── ParamUnit ─────────────────────────────────────────────────────────────────

/// Units for parameter display.
pub enum ParamUnit {
    Generic,
    Hz,
    Decibels,
    Milliseconds,
    Seconds,
    Semitones,
    Cents,
    Percent,
    Beats,
    /// Compile-time custom unit label.
    Custom(&'static str),
    /// Runtime-generated unit label. Allocates — only use when necessary.
    CustomDynamic(String),
}

// ── ParamDisplay ──────────────────────────────────────────────────────────────

/// Override default unit formatting. Used for stepped parameters with named
/// values (waveform selectors, algorithm pickers, etc.).
pub trait ParamDisplay: Send + Sync {
    fn format(&self, value: f64) -> String;
    fn parse(&self, s: &str) -> Option<f64>;
}

pub enum ParamDisplayAdapter {
    /// Baked into the binary at compile time. Zero allocation.
    Static(&'static dyn ParamDisplay),
    /// Runtime-constructed. Use for dynamic label sets (e.g. sample slice names).
    Dynamic(Box<dyn ParamDisplay>),
}

impl ParamDisplayAdapter {
    pub fn format(&self, value: f64) -> String {
        match self {
            Self::Static(d) => d.format(value),
            Self::Dynamic(d) => d.format(value),
        }
    }

    pub fn parse(&self, s: &str) -> Option<f64> {
        match self {
            Self::Static(d) => d.parse(s),
            Self::Dynamic(d) => d.parse(s),
        }
    }
}

// ── ParamDescriptor ───────────────────────────────────────────────────────────

/// Describes one parameter exposed by a node.
/// Used by the sequencer for parameter-lock discovery, by the GUI for display,
/// and by the scripting layer for automation.
pub struct ParamDescriptor {
    /// Hash of `name`. Stable per node type — same name always produces the same id.
    /// Parameter locks survive capability renegotiation as long as the name is present.
    pub id: u32,

    pub name: PortName,
    pub min: f64,
    pub max: f64,
    pub default: f64,

    /// True for integer-stepped parameters (algorithm selectors, etc.)
    pub stepped: bool,

    pub unit: ParamUnit,

    /// Override display formatting. `None` = use `ParamUnit` formatting.
    pub display: Option<ParamDisplayAdapter>,
}

impl ParamDescriptor {
    /// Compute the stable parameter ID from a name string.
    /// Uses FNV-1a 32-bit hash for determinism and compile-time usability.
    pub fn id_for_name(name: &str) -> u32 {
        const FNV_OFFSET: u32 = 2_166_136_261;
        const FNV_PRIME: u32 = 16_777_619;
        let mut hash = FNV_OFFSET;
        for byte in name.bytes() {
            hash ^= byte as u32;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }
}

// ── CapabilityDocument ───────────────────────────────────────────────────────

/// A node's complete self-description. Returned by `Node::capability_document()`.
/// Built on the main thread — allocation is fine here.
pub struct CapabilityDocument {
    pub name: &'static str,
    pub vendor: &'static str,
    /// Semantic version: (major, minor, patch).
    pub version: (u32, u32, u32),
    pub ports: Vec<PortDescriptor>,
    pub params: Vec<ParamDescriptor>,

    /// Extension identifiers this node implements.
    /// e.g. `"paraclete.instrument"`, `"paraclete.sequencer"`,
    ///      `"com.yourcompany.custom_protocol"`.
    pub extensions: Vec<&'static str>,
}

impl CapabilityDocument {
    /// Build a minimal document from port declarations only.
    /// Used as the default implementation of `Node::capability_document()`.
    pub fn from_ports(ports: &[PortDescriptor]) -> Self {
        Self {
            name: "unnamed",
            vendor: "unknown",
            version: (0, 0, 0),
            ports: ports.to_vec(),
            params: vec![],
            extensions: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{PortDescriptor, PortDirection, PortType};

    fn make_port(id: u32, name: &'static str) -> PortDescriptor {
        PortDescriptor {
            id,
            name: name.into(),
            direction: PortDirection::Output,
            port_type: PortType::Audio,
        }
    }

    #[test]
    fn capability_document_from_ports_preserves_port_list() {
        let ports = [make_port(0, "audio_out"), make_port(1, "cv_out")];
        let doc = CapabilityDocument::from_ports(&ports);

        assert_eq!(doc.ports.len(), 2);
        assert_eq!(doc.ports[0].name.as_str(), "audio_out");
        assert_eq!(doc.ports[1].name.as_str(), "cv_out");
    }

    #[test]
    fn capability_document_from_ports_has_empty_params_and_extensions() {
        let doc = CapabilityDocument::from_ports(&[make_port(0, "out")]);
        assert!(doc.params.is_empty());
        assert!(doc.extensions.is_empty());
    }

    #[test]
    fn capability_document_from_empty_ports_is_valid() {
        let doc = CapabilityDocument::from_ports(&[]);
        assert!(doc.ports.is_empty());
    }

    #[test]
    fn param_descriptor_id_for_same_name_is_stable() {
        let id_a = ParamDescriptor::id_for_name("cutoff");
        let id_b = ParamDescriptor::id_for_name("cutoff");
        assert_eq!(id_a, id_b);
    }

    #[test]
    fn param_descriptor_id_differs_for_different_names() {
        let id_cutoff = ParamDescriptor::id_for_name("cutoff");
        let id_resonance = ParamDescriptor::id_for_name("resonance");
        assert_ne!(id_cutoff, id_resonance);
    }

    #[test]
    fn param_descriptor_id_is_nonzero_for_nonempty_name() {
        // FNV-1a of any non-empty string should not collide with the zero sentinel.
        assert_ne!(ParamDescriptor::id_for_name("x"), 0);
    }
}
