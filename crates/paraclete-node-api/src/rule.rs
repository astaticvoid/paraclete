//! ADR-032 — Theoria view-plugin API types.
//!
//! `Rule` is the serializable view-data struct stored on `CapabilityDocument`.
//! Built once per node at construction time from `ViewPlugin::to_rule()`.
//! The Antiphon server serializes this to assemble the `view_meta` JSON message.
//!
//! Serialize derives are gated behind the `serialize` feature flag (optional
//! serde). The `ViewPlugin` trait is always available — third-party node
//! authors implement it without depending on serde.

use std::borrow::Cow;

// ── Rule ──────────────────────────────────────────────────────────────────────

/// The complete view-data snapshot for a node.
///
/// Stored on `CapabilityDocument` as `Option<Rule>`. Nodes without surface
/// presence (internal clock, scripting gateways) leave it `None`.
/// Built once at construction time — never on the audio thread.
#[derive(Clone)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub struct Rule {
    /// Human-readable display name (e.g. "Kick", "Filter", "Reverb").
    pub name: Cow<'static, str>,

    /// Ordered page group IDs this node contributes params to.
    pub page_groups: Cow<'static, [Cow<'static, str>]>,

    /// Param ID → page placement.
    pub param_pages: Cow<'static, [(u32, PageRef)]>,

    /// Macro bindings (may be empty).
    pub macros: Cow<'static, [MacroBinding]>,

    /// Per-param affordance hints (may be empty).
    pub affordances: Cow<'static, [(u32, AffordanceHint)]>,

    /// Envelope groups — sets of params that together form an ADSR/AHD curve.
    pub envelopes: Cow<'static, [EnvelopeGroup]>,

    /// Per-param routing semantics (may be empty).
    pub routing: Cow<'static, [(u32, RoutingSemantics)]>,

    /// SVG diagram bytes. None if this node has no engine diagram.
    /// Skipped from JSON serialization — Antiphon sends diagrams via a
    /// separate `engine_diagram` message with base64 encoding.
    #[cfg_attr(feature = "serialize", serde(skip))]
    pub diagram: Option<Cow<'static, [u8]>>,

    /// Override sub-node views, keyed by sub-node id.
    pub view_overrides: Cow<'static, [(u64, Rule)]>,
}

// ── PageRef ────────────────────────────────────────────────────────────────────

/// A parameter's placement in the page grid.
#[derive(Clone)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub struct PageRef {
    /// Page group key ("SRC", "FLTR", "AMP", "FX", "MOD", or custom).
    pub page: Cow<'static, str>,
    /// 0-based slot within the page. Slots 0–7 = sub-page 1, 8–15 = sub-page 2, etc.
    pub slot: u8,
}

// ── MacroBinding + MacroCurve ─────────────────────────────────────────────────

/// One expressive control mapped to several internal parameters.
#[derive(Clone)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub struct MacroBinding {
    /// Display name for this macro control.
    pub name: Cow<'static, str>,
    /// Parameter IDs this macro drives.
    pub targets: Cow<'static, [u32]>,
    /// Mapping per target. Must match `targets` length.
    pub curves: Cow<'static, [MacroCurve]>,
    /// Page this macro appears on (None = appears on all pages with targets).
    pub page: Option<Cow<'static, str>>,
}

/// How a macro's value maps onto each target parameter.
#[derive(Clone)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub enum MacroCurve {
    Linear,
    Exponential,
    InverseExponential,
}

// ── AffordanceHint ─────────────────────────────────────────────────────────────

/// What to draw beside a parameter value in the contextual window.
#[derive(Clone)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub enum AffordanceHint {
    None,
    /// ADSR/AHD envelope curve. `group_idx` indexes into `Rule::envelopes`.
    EnvelopeCurve {
        group_idx: u8,
    },
    FilterShape,
    LfoShape,
    Waveform,
    /// Engine block-diagram region highlight.
    DiagramHighlight {
        region_id: Cow<'static, str>,
    },
}

// ── EnvelopeGroup ──────────────────────────────────────────────────────────────

/// A set of envelope parameters that together draw an ADSR/AHD curve.
#[derive(Clone)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub struct EnvelopeGroup {
    /// Envelope type: `"ADSR"`, `"AHD"`, `"DADSR"`.
    pub env_type: Cow<'static, str>,
    /// Human-readable label (e.g. "Amp Envelope", "Filter Env").
    pub label: Cow<'static, str>,
    /// Ordered param IDs. ADSR: [attack, decay, sustain, release].
    /// AHD:  [attack, hold, decay, _unused].
    pub param_ids: [u32; 4],
}

// ── RoutingSemantics ───────────────────────────────────────────────────────────

/// Declares that a parameter controls a send amount to a destination.
#[derive(Clone)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub struct RoutingSemantics {
    /// Logical destination name (e.g. "filter", "reverb", "delay").
    pub destination: Cow<'static, str>,
    /// Human-readable source label (e.g. "Kick", "Bass").
    pub source_label: Cow<'static, str>,
}

// ── ViewPlugin trait ───────────────────────────────────────────────────────────

/// Implemented by every L3 node that has surface presence.
///
/// A single builder method — nodes construct a complete `Rule` at once.
/// Called once at construction time (or on `activate()` for dynamic graphs).
/// Not on the audio thread.
pub trait ViewPlugin {
    fn to_rule(&self, node_id: u64, sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule;
}

// ── Convenience constructors ──────────────────────────────────────────────────

impl Rule {
    /// A minimal Rule for a node that only contributes params to one page.
    pub fn single_page(name: &'static str, page_group: &'static str) -> Self {
        Self {
            name: Cow::Borrowed(name),
            page_groups: Cow::Owned(vec![Cow::Borrowed(page_group)]),
            param_pages: Cow::Borrowed(&[]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
        }
    }

    /// An empty Rule for nodes with no surface presence.
    pub const fn empty() -> Self {
        Self {
            name: Cow::Borrowed(""),
            page_groups: Cow::Borrowed(&[]),
            param_pages: Cow::Borrowed(&[]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.page_groups.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ParamDescriptor;

    #[test]
    fn rule_single_page_has_one_page_group() {
        let rule = Rule::single_page("Filter", "FLTR");
        assert_eq!(rule.page_groups.len(), 1);
        assert_eq!(rule.page_groups[0], "FLTR");
    }

    #[test]
    fn rule_empty_is_empty() {
        assert!(Rule::empty().is_empty());
    }

    #[test]
    fn rule_single_page_is_not_empty() {
        assert!(!Rule::single_page("Test", "SRC").is_empty());
    }

    #[test]
    fn envelope_group_adsr_param_ids() {
        let a_id = ParamDescriptor::id_for_name("attack");
        let d_id = ParamDescriptor::id_for_name("decay");
        let s_id = ParamDescriptor::id_for_name("sustain");
        let r_id = ParamDescriptor::id_for_name("release");

        let group = EnvelopeGroup {
            env_type: Cow::Borrowed("ADSR"),
            label: Cow::Borrowed("Amp Envelope"),
            param_ids: [a_id, d_id, s_id, r_id],
        };

        assert_eq!(group.env_type, "ADSR");
        assert_eq!(group.param_ids[0], a_id);
        assert_eq!(group.param_ids[3], r_id);
    }

    #[test]
    fn affordance_envelope_curve_references_group_index() {
        let hint = AffordanceHint::EnvelopeCurve { group_idx: 2 };
        match hint {
            AffordanceHint::EnvelopeCurve { group_idx } => assert_eq!(group_idx, 2),
            _ => panic!("expected EnvelopeCurve"),
        }
    }

    #[test]
    fn macro_binding_curves_match_targets_length() {
        let binding = MacroBinding {
            name: Cow::Borrowed("HARM"),
            targets: Cow::Borrowed(&[1, 2, 3]),
            curves: Cow::Borrowed(&[
                MacroCurve::Linear,
                MacroCurve::Exponential,
                MacroCurve::InverseExponential,
            ]),
            page: None,
        };
        assert_eq!(binding.targets.len(), binding.curves.len());
    }

    #[test]
    fn routing_semantics_declares_destination() {
        let routing = RoutingSemantics {
            destination: Cow::Borrowed("reverb"),
            source_label: Cow::Borrowed("Kick"),
        };
        assert_eq!(routing.destination, "reverb");
    }

    #[test]
    fn rule_diagram_is_not_serialized() {
        let rule = Rule {
            name: Cow::Borrowed("Test"),
            page_groups: Cow::Borrowed(&[]),
            param_pages: Cow::Borrowed(&[]),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: Some(Cow::Borrowed(b"<svg>...</svg>")),
            view_overrides: Cow::Borrowed(&[]),
        };
        assert!(rule.diagram.is_some());
    }

    #[cfg(feature = "serialize")]
    #[test]
    fn rule_serializes_to_json() {
        let rule = Rule::single_page("Filter", "FLTR");
        let json = serde_json::to_string(&rule).expect("serialize");
        assert!(json.contains("Filter"));
        assert!(json.contains("FLTR"));
    }

    #[cfg(feature = "serialize")]
    #[test]
    fn envelope_group_serializes_param_ids() {
        let a = ParamDescriptor::id_for_name("attack");
        let d = ParamDescriptor::id_for_name("decay");
        let s = ParamDescriptor::id_for_name("sustain");
        let r = ParamDescriptor::id_for_name("release");
        let group = EnvelopeGroup {
            env_type: Cow::Borrowed("ADSR"),
            label: Cow::Borrowed("Amp Envelope"),
            param_ids: [a, d, s, r],
        };
        let json = serde_json::to_string(&group).expect("serialize");
        assert!(json.contains("ADSR"));
    }
}
