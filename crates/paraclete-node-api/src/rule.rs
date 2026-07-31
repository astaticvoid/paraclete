//! ADR-032 — Theoria view-plugin API types.
//!
//! `Rule` is the view-data struct stored on `CapabilityDocument`, built once
//! per node at construction time from `ViewPlugin::to_rule()`.
//!
//! Serialize derives are gated behind the `serialize` feature flag (optional
//! serde). The `ViewPlugin` trait is always available — third-party node
//! authors implement it without depending on serde.
//!
//! **`Rule` does not currently reach the wire through serde.** No crate in
//! the workspace enables the `serialize` feature, so those derives are
//! inert; Antiphon holds `HashMap<u32, Rule>` and hand-maps it into its own
//! protocol types. An earlier version of this doc said the server
//! "serializes this to assemble the `view_meta` JSON message" — it does not.
//! Enabling the feature would put this entire internal shape on the wire as
//! a side effect, which is a protocol decision, not a build-flag one.

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

    /// ADR-041: per-machine view variants for a machine-host engine, one per
    /// value of its `machine` param.
    ///
    /// **Empty means "this node has one fixed view"** — the base fields above
    /// are used as-is, which is every node that is not a machine host. A
    /// surface watches the node's `machine` param and swaps the displayed
    /// variant locally; there is no runtime capability re-query and none is
    /// added (ADR-041 decision 1).
    pub variants: Cow<'static, [MachineVariant]>,
}

// ── MachineVariant + ParamOverlay ─────────────────────────────────────────────

/// One machine's view of its host node (ADR-041 decision 3).
///
/// Carries that machine's own page layout, so a `param_pages` entry naming a
/// param the active machine does not declare — BUG-037's shape — becomes
/// unrepresentable rather than silently degrading to a `param_{id}`
/// placeholder.
///
/// **That covers `param_pages` only.** `Rule`'s other reference-bearing
/// fields — `affordances`, `envelopes`, `macros`, `routing` — have no variant
/// slot and stay machine-invariant, so they can still name a param the active
/// variant does not display. That is live, not hypothetical: both engines
/// build affordances and envelope groups outside the per-machine `match`
/// (`analog_engine.rs:280-303`, `fm_engine.rs:296-303`). Widening this type is
/// deliberately deferred — ADR-041 decision 3 and §0 A1 specify exactly these
/// five fields — so the guard is an assertion instead (see MM-C8), and it must
/// check refs against the **active variant's displayed set**, not just the
/// union doc, or it passes on precisely this case.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub struct MachineVariant {
    /// The `machine` param value that selects this variant.
    pub value: u32,

    /// Display name for this machine (e.g. "AnalogKick").
    pub name: Cow<'static, str>,

    /// Page group IDs this machine contributes to — may differ per machine.
    pub page_groups: Cow<'static, [Cow<'static, str>]>,

    /// This machine's param placements, replacing `Rule::param_pages` while
    /// it is active.
    pub pages: Cow<'static, [(u32, PageRef)]>,

    /// Per-param range/default overrides for this machine, keyed by param id.
    ///
    /// The host's parameter bank stores the **union** (widest-envelope) range
    /// and is never narrowed — these are what a *surface* displays and clamps
    /// input against (ADR-041 §0 A1).
    ///
    /// **Absence of an overlay does not mean the machine ignores the param.**
    /// It is ambiguous between "this machine does not use it" and "this
    /// machine uses it at the full union range, so there is nothing to
    /// narrow" — and the second is common (per MM-C3's table, Kick's `decay`
    /// *is* the union). The discriminator for "does this machine use it" is
    /// membership in `pages`. Keying display-suppression on overlay absence
    /// would hide params that are merely un-narrowed.
    ///
    /// Duplicate ids are representable here and their precedence is
    /// undefined; MM-C8's assertion checks uniqueness rather than the type
    /// enforcing it.
    pub overlays: Cow<'static, [(u32, ParamOverlay)]>,
}

/// One machine's range and default for a param it shares with its siblings.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize))]
pub struct ParamOverlay {
    pub min: f64,
    pub max: f64,
    pub default: f64,

    /// True for params that *are* the node's identity rather than a setting —
    /// `machine` itself. Identity params are rejected as p-lock targets and as
    /// scene-morph destinations (ADR-041 §0 A4, decision 6): per-step machine
    /// switching is undesigned, and stepped identity is not morphable.
    /// Enforced surface/app-side — the sequencer stores opaque
    /// `(node_id, param_id)` locks and cannot know a foreign node's params.
    ///
    /// **Machine-invariant in practice, but stored per variant** (ADR-041 §0
    /// A1 puts the flag here). `machine` exists on every machine of a host, so
    /// `identity: true` has to be repeated in every variant's `overlays`; miss
    /// one and lock rejection silently stops working *for that machine only*.
    /// MM-C8's assertion checks that a param flagged in any variant is flagged
    /// in all of them. Hoisting it to a `Rule`-level `identity_params` would
    /// remove the hazard structurally but deviates from the ratified shape, so
    /// it is not taken here.
    pub identity: bool,
}

/// Forward-compatible construction, for the reason spelled out on
/// `impl Default for Rule` — these are new all-public-field types on the same
/// LGPL3 boundary, and `MachineVariant` is the one most likely to grow (its
/// doc names two fields it may need). Adding a field after MM-C3–C5 have
/// written variant literals costs strictly more than this does now.
impl Default for MachineVariant {
    fn default() -> Self {
        Self {
            value: 0,
            name: Cow::Borrowed(""),
            page_groups: Cow::Borrowed(&[]),
            pages: Cow::Borrowed(&[]),
            overlays: Cow::Borrowed(&[]),
        }
    }
}

/// Hand-written, not derived: `#[derive(Default)]` would give
/// `min: 0.0, max: 0.0`, a degenerate range that clamps every write to zero.
/// A unit range is the harmless default for a param whose range nobody set.
impl Default for ParamOverlay {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            identity: false,
        }
    }
}

// ── PageRef ────────────────────────────────────────────────────────────────────

/// A parameter's placement in the page grid.
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug)]
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
            variants: Cow::Borrowed(&[]),
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
            variants: Cow::Borrowed(&[]),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.page_groups.is_empty()
    }
}

/// Forward-compatible construction for third-party node authors.
///
/// `Rule` has all-public fields at the **LGPL3 boundary** (L2), so every field
/// added to it breaks every external `Rule { .. }` literal — as this crate's
/// own 14 literals just demonstrated when `variants` landed. A node written as
/// `Rule { name: …, page_groups: …, ..Default::default() }` survives the next
/// addition; one that spells out every field does not.
///
/// Flagged per the standing universality check: this crate is pre-1.0, so the
/// window to make additions non-breaking is now. `#[non_exhaustive]` is the
/// other option and was **not** taken — it forbids literal construction
/// outside the defining crate entirely, which would break `paraclete-nodes`
/// and every third-party node at once, for a stricter guarantee than the
/// problem needs.
impl Default for Rule {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ParamDescriptor;

    /// MM-C2: `variants` is additive and inert. Every node that is not a
    /// machine host leaves it empty, and empty must mean "use the base fields
    /// as-is" — the property the whole additive design rests on (ADR-041
    /// decision 3).
    #[test]
    fn convenience_constructors_leave_variants_empty() {
        assert!(Rule::empty().variants.is_empty());
        assert!(Rule::single_page("Filter", "FLTR").variants.is_empty());
        assert!(Rule::default().variants.is_empty());
    }

    /// `Rule::empty()` is declared `const fn`, but nothing in the workspace
    /// calls it in const position — so a future field with a non-const default
    /// would silently drop the qualifier and no build would notice. This pins
    /// it.
    #[test]
    fn rule_empty_is_usable_in_const_position() {
        const EMPTY: Rule = Rule::empty();
        assert!(EMPTY.is_empty());
    }

    /// A node written against the forward-compatible form gets a usable Rule
    /// without naming `variants` — which is the point of the `Default` impl.
    #[test]
    fn default_spread_construction_compiles_and_is_inert() {
        let rule = Rule {
            name: Cow::Borrowed("ThirdParty"),
            page_groups: Cow::Owned(vec![Cow::Borrowed("SRC")]),
            ..Default::default()
        };
        assert_eq!(rule.name.as_ref(), "ThirdParty");
        assert!(rule.variants.is_empty());
        assert!(
            !rule.is_empty(),
            "one page group means it has surface presence"
        );
    }

    /// The overlay carries the flag p-lock and scene-assign rejection key on
    /// (ADR-041 §0 A4), so a defaulted overlay must not read as identity —
    /// that would reject locks on an ordinary param. Also pins the range:
    /// a derived `Default` would give `min == max == 0.0`, which clamps every
    /// write to zero.
    #[test]
    fn defaulted_overlay_is_an_ordinary_unit_range_param() {
        let d = ParamOverlay::default();
        assert!(!d.identity, "a defaulted overlay must not read as identity");
        assert!(d.min < d.max, "degenerate range clamps every write to min");
        assert_eq!((d.min, d.max), (0.0, 1.0));
    }

    /// MM-C2 is inert: a defaulted variant contributes no pages and no
    /// overlays, so adding one to a `Rule` cannot change what renders until a
    /// later commit populates it.
    #[test]
    fn defaulted_variant_contributes_nothing() {
        let v = MachineVariant::default();
        assert!(v.pages.is_empty());
        assert!(v.overlays.is_empty());
        assert!(v.page_groups.is_empty());
    }

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
            variants: Cow::Borrowed(&[]),
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
