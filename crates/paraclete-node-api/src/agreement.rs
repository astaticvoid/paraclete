/// Agreement produced by the runtime when two nodes connect.
/// Stub at P0 — baseline only. Full negotiation protocol at Phase 3.
pub struct ConnectionAgreement {
    /// Agreed audio buffer format.
    pub sample_rate: f32,
    pub block_size: usize,
    pub channels: usize,

    /// Negotiated space_ids for custom extended events.
    /// Empty at P0.
    pub space_ids: Vec<(String, u16)>,

    /// Current clock position at connection time.
    /// Downstream nodes initialise their internal transport state from this
    /// so they do not have to wait for the next TransportEvent.
    pub initial_transport: Option<crate::transport::TransportInfo>,
}

impl ConnectionAgreement {
    /// Baseline agreement — no custom extensions, standard stereo audio format, no initial transport.
    pub fn baseline() -> Self {
        Self {
            sample_rate: 44100.0,
            block_size: 512,
            channels: 2,
            space_ids: vec![],
            initial_transport: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_agreement_sample_rate_is_44100() {
        assert_eq!(ConnectionAgreement::baseline().sample_rate, 44100.0);
    }

    #[test]
    fn baseline_agreement_block_size_is_512() {
        assert_eq!(ConnectionAgreement::baseline().block_size, 512);
    }

    #[test]
    fn baseline_agreement_is_stereo() {
        assert_eq!(ConnectionAgreement::baseline().channels, 2);
    }

    #[test]
    fn baseline_agreement_has_no_custom_event_spaces() {
        assert!(ConnectionAgreement::baseline().space_ids.is_empty());
    }
}
