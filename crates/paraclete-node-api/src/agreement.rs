use crate::capability::ParamUnit;
use crate::transport::TransportInfo;

// ── LockableParam ─────────────────────────────────────────────────────────────

/// A parameter exposed for locking by a connected sequencer.
///
/// Declared by instrument nodes in `ConnectionAgreement::lockable_params`
/// during the connection handshake. The sequencer reads this list to discover
/// which parameters can be overridden per-step.
#[derive(Clone, Debug)]
pub struct LockableParam {
    /// Hash of parameter name. Matches `ParamDescriptor::id`.
    pub param_id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: ParamUnit,
}

// ── ConnectionAgreement ──────────────────────────────────────────────────────

/// Agreement produced during the connection handshake.
/// Both nodes receive a reconciled copy via `set_connection_record()`.
#[derive(Clone, Debug)]
pub struct ConnectionAgreement {
    pub sample_rate: f32,
    pub block_size: usize,
    pub channels: usize,

    /// Negotiated space IDs for custom extended events. Empty at P0.
    pub space_ids: Vec<(String, u16)>,

    /// Current clock position at connection time.
    pub initial_transport: Option<TransportInfo>,

    /// Parameters this node will accept `ParamLockEvent` for.
    /// Populated by the receiving node (e.g. `Sampler`) in `negotiate()`.
    /// The sending node (e.g. `Sequencer`) reads this to discover what it
    /// can lock per step.
    pub lockable_params: Vec<LockableParam>,
}

impl ConnectionAgreement {
    /// Baseline agreement — standard stereo audio, no custom event spaces,
    /// no lockable parameters.
    pub fn baseline() -> Self {
        Self {
            sample_rate: 44100.0,
            block_size: 512,
            channels: 2,
            space_ids: vec![],
            initial_transport: None,
            lockable_params: vec![],
        }
    }
}

// ── ConnectionRecord ──────────────────────────────────────────────────────────

/// Delivered to both nodes after the connection handshake completes.
///
/// Each node receives the reconciled agreement and its partner's node ID.
/// Nodes that implement `Negotiable` store this to know who they are
/// connected to and what was agreed.
#[derive(Clone, Debug)]
pub struct ConnectionRecord {
    pub agreement: ConnectionAgreement,
    /// The `NodeId` (user-assigned `u32`) of the connected partner.
    pub partner_id: u32,
    /// Which of this node's ports this record applies to.
    pub local_port_id: u32,
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

    #[test]
    fn baseline_agreement_has_no_lockable_params() {
        assert!(ConnectionAgreement::baseline().lockable_params.is_empty());
    }

    #[test]
    fn connection_agreement_carries_lockable_params() {
        let mut ag = ConnectionAgreement::baseline();
        ag.lockable_params.push(LockableParam {
            param_id: 42,
            name: "pitch".into(),
            min: -24.0,
            max: 24.0,
            default: 0.0,
            unit: ParamUnit::Semitones,
        });
        assert_eq!(ag.lockable_params.len(), 1);
        assert_eq!(ag.lockable_params[0].param_id, 42);
    }

    #[test]
    fn lockable_param_is_clone_and_debug() {
        let p = LockableParam {
            param_id: 1,
            name: "volume".into(),
            min: 0.0,
            max: 1.0,
            default: 0.8,
            unit: ParamUnit::Generic,
        };
        let q = p.clone();
        assert_eq!(q.param_id, 1);
        let _ = format!("{:?}", q);
    }

    #[test]
    fn connection_record_stores_partner_id_and_port() {
        let record = ConnectionRecord {
            agreement: ConnectionAgreement::baseline(),
            partner_id: 7,
            local_port_id: 2,
        };
        assert_eq!(record.partner_id, 7);
        assert_eq!(record.local_port_id, 2);
        let _ = format!("{:?}", record);
    }
}
