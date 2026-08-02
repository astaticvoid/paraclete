//! Debug-build validation of a node's view declaration (ADR-041 amendment 5,
//! widened by MM-C8).
//!
//! A `Rule` is a pile of `u32` param references with nothing checking that any
//! of them names a real param. When one does not, composite assembly degrades
//! it to a `param_{id}` placeholder — and the control still *works*, because
//! the sequencer and the bank address params by id regardless of whether a
//! surface can name one. So the failure mode is a live, lockable knob under a
//! meaningless label, which is exactly what #47 (BUG-037) and #156 (BUG-055)
//! both were.
//!
//! **Amendment 5 as literally stated would pass on BUG-037's successor.** It
//! asks that every ref resolve "in the union doc"; the union contains every
//! machine's params, so a page naming a param the *active* machine does not
//! have resolves fine against it. The checks here are therefore per variant,
//! against what that variant actually displays.
//!
//! This lives at L2, beside the types it validates, so a third-party node
//! author gets the same guard rather than only the in-tree engines.

use std::collections::HashMap;

use crate::capability::CapabilityDocument;
use crate::rule::AffordanceHint;

/// One view's name (`None` for the base `Rule`) and the params it places.
type DisplayedSet<'a> = (Option<String>, &'a [(u32, crate::rule::PageRef)]);

/// One problem found in a node's view declaration.
///
/// Returned rather than panicked so a caller can decide: a test lists them
/// all at once, `debug_assert_view` turns them into one panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewDefect {
    /// Which variant it was found in — `None` for the base `Rule`.
    pub variant: Option<String>,
    pub message: String,
}

impl std::fmt::Display for ViewDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.variant {
            Some(v) => write!(f, "[variant {v}] {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

/// Every defect in `doc`'s view declaration. Empty means clean.
///
/// Checks, in the order MM-C8 lists them:
///
/// 1. Every page ref names a param the node declares, and every param the
///    node declares appears on some page. Both directions matter: an
///    undeclared ref is a control with no name, an unpaged param is a control
///    no surface can reach.
/// 2. Non-page references — affordances, envelope groups, macro targets,
///    routing — resolve against the **displayed set of the variant in
///    question**, not merely against the union. These fields have no variant
///    slot, so a base-`Rule` affordance can name a param a given machine does
///    not show.
/// 3. Overlay ids are unique within a variant. `overlays` is a linear assoc
///    list, so duplicates are representable and their precedence is undefined.
/// 4. A param flagged `identity` in any variant is flagged in all of them.
///    The flag lives per overlay (ADR-041 §0 A1), so it has to be repeated;
///    miss one and p-lock rejection stops working for that machine alone.
/// 5. `param_labels` ids resolve against the union doc (ADR-041 amendment
///    2026-08-02). A label array names values of a param, so an id no node
///    declares would make a surface label a param that does not exist.
///
/// **Not checked here, deliberately:** that a param id shared across machines
/// agrees on `name`/`unit`/`stepped` (MM-C8 point 4). That is a property of
/// the per-machine descriptor lists the engine merges, and the merged doc this
/// sees has already dropped the losing declarations — there is nothing left to
/// compare. Both engines carry their own test for it.
pub fn validate_view(doc: &CapabilityDocument) -> Vec<ViewDefect> {
    let Some(rule) = doc.view.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let declared: Vec<u32> = doc.params.iter().map(|p| p.id).collect();
    let name_of = |id: u32| -> String {
        doc.params
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.as_str().to_string())
            .unwrap_or_else(|| format!("param_{id}"))
    };

    // ── The base Rule, and each variant, as separate displayed sets ──────
    let mut views: Vec<DisplayedSet> = vec![(None, rule.param_pages.as_ref())];
    for v in rule.variants.iter() {
        views.push((Some(v.name.to_string()), v.pages.as_ref()));
    }

    for (variant, pages) in &views {
        let mut seen: Vec<u32> = Vec::new();
        for (pid, page_ref) in pages.iter() {
            if !declared.contains(pid) {
                out.push(ViewDefect {
                    variant: variant.clone(),
                    message: format!(
                        "page {} slot {} names param {pid}, which the node does not \
                         declare — it would draw a working control under a \
                         `param_{pid}` label",
                        page_ref.page, page_ref.slot
                    ),
                });
            }
            seen.push(*pid);
        }

        // Non-page refs, against *this* view's displayed set.
        let displayed = |id: u32| seen.contains(&id);
        for (pid, hint) in rule.affordances.iter() {
            if !displayed(*pid) {
                out.push(ViewDefect {
                    variant: variant.clone(),
                    message: format!(
                        "affordance {hint:?} is declared for `{}` ({pid}), which this \
                         view does not display",
                        name_of(*pid)
                    ),
                });
            }
            if let AffordanceHint::EnvelopeCurve { group_idx } = hint {
                if rule.envelopes.get(*group_idx as usize).is_none() {
                    out.push(ViewDefect {
                        variant: variant.clone(),
                        message: format!(
                            "`{}` points at envelope group {group_idx}, but only {} \
                             are declared",
                            name_of(*pid),
                            rule.envelopes.len()
                        ),
                    });
                }
            }
        }
        for env in rule.envelopes.iter() {
            for id in env.param_ids.iter().filter(|id| **id != 0) {
                if !displayed(*id) {
                    out.push(ViewDefect {
                        variant: variant.clone(),
                        message: format!(
                            "envelope `{}` names `{}` ({id}), which this view does not \
                             display",
                            env.label,
                            name_of(*id)
                        ),
                    });
                }
            }
        }
        for m in rule.macros.iter() {
            for id in m.targets.iter() {
                if !displayed(*id) {
                    out.push(ViewDefect {
                        variant: variant.clone(),
                        message: format!(
                            "macro `{}` targets `{}` ({id}), which this view does not \
                             display",
                            m.name,
                            name_of(*id)
                        ),
                    });
                }
            }
        }
        for (pid, sem) in rule.routing.iter() {
            if !declared.contains(pid) {
                out.push(ViewDefect {
                    variant: variant.clone(),
                    message: format!(
                        "routing to `{}` names param {pid}, which the node does not \
                         declare",
                        sem.destination
                    ),
                });
            }
        }
    }

    // ── Every declared param must be reachable from some page ────────────
    //
    // Checked against the union of every view, not per variant: a param only
    // one machine uses is legitimately absent from the others' pages. What is
    // never legitimate is a param no view shows at all — `tune` was in exactly
    // that state on all three FM machines (#47) and no surface could edit it.
    let paged_anywhere: Vec<u32> = views
        .iter()
        .flat_map(|(_, pages)| pages.iter().map(|(pid, _)| *pid))
        .collect();
    for p in &doc.params {
        if !paged_anywhere.contains(&p.id) {
            out.push(ViewDefect {
                variant: None,
                message: format!(
                    "`{}` ({}) is declared but appears on no page — unreachable from \
                     any surface",
                    p.name.as_str(),
                    p.id
                ),
            });
        }
    }

    // ── Overlay hygiene, and the identity flag's per-variant repeat ──────
    let mut identity_anywhere: Vec<u32> = Vec::new();
    let mut flagged_in: HashMap<u32, Vec<String>> = HashMap::new();
    for v in rule.variants.iter() {
        let mut ids: Vec<u32> = Vec::new();
        for (pid, overlay) in v.overlays.iter() {
            if ids.contains(pid) {
                out.push(ViewDefect {
                    variant: Some(v.name.to_string()),
                    message: format!(
                        "`{}` ({pid}) has two overlays; which one wins is undefined",
                        name_of(*pid)
                    ),
                });
            }
            ids.push(*pid);
            if overlay.identity {
                if !identity_anywhere.contains(pid) {
                    identity_anywhere.push(*pid);
                }
                flagged_in.entry(*pid).or_default().push(v.name.to_string());
            }
        }
    }
    for pid in identity_anywhere {
        let flagged = flagged_in.get(&pid).cloned().unwrap_or_default();
        let missing: Vec<String> = rule
            .variants
            .iter()
            .map(|v| v.name.to_string())
            .filter(|n| !flagged.contains(n))
            .collect();
        if !missing.is_empty() {
            out.push(ViewDefect {
                variant: None,
                message: format!(
                    "`{}` ({pid}) is flagged `identity` on {flagged:?} but not on \
                     {missing:?} — p-lock rejection would work on some machines and \
                     not others",
                    name_of(pid)
                ),
            });
        }
    }

    // ── `param_labels` hygiene (ADR-041 amendment 2026-08-02) ────────────
    // A label array names values of a param, so its id must be one the node
    // declares — the same class of defect as BUG-037, which `param_pages`
    // checks above. The array itself is value-indexed, so its LENGTH is not
    // validated against the param's range here: a stepped param whose labels
    // are sparser than its range is legal (gaps are `None`), and a label for
    // a value past the range is unreachable rather than wrong.
    for v in rule.variants.iter() {
        for (pid, _labels) in v.param_labels.iter() {
            if !declared.contains(pid) {
                out.push(ViewDefect {
                    variant: Some(v.name.to_string()),
                    message: format!(
                        "`param_labels` names `{}` ({pid}), which the node does not \
                         declare — a surface would label a param that does not exist",
                        name_of(*pid)
                    ),
                });
            }
        }
    }

    out
}

/// Panic in a debug build if `doc`'s view declaration has any defect.
///
/// Compiled out of release entirely (`debug_assertions`), so a shipped binary
/// pays nothing and a malformed third-party node degrades to the old
/// `param_{id}` behaviour rather than taking the process down mid-set.
pub fn debug_assert_view(doc: &CapabilityDocument) {
    if cfg!(debug_assertions) {
        let defects = validate_view(doc);
        assert!(
            defects.is_empty(),
            "node `{}` has an invalid view declaration:\n  {}",
            doc.name,
            defects
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("\n  ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{ParamDescriptor, ParamUnit};
    use crate::rule::{
        EnvelopeGroup, MachineVariant, MacroBinding, MacroCurve, PageRef, ParamOverlay,
        RoutingSemantics, Rule,
    };
    use std::borrow::Cow;

    const A: u32 = 100;
    const B: u32 = 200;
    const ABSENT: u32 = 999;

    fn pd(id: u32, name: &'static str) -> ParamDescriptor {
        ParamDescriptor {
            id,
            name: name.into(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            stepped: false,
            in_kit: true,
            unit: ParamUnit::Generic,
            display: None,
        }
    }

    fn page(id: u32, slot: u8) -> (u32, PageRef) {
        (
            id,
            PageRef {
                page: Cow::Borrowed("SRC"),
                slot,
            },
        )
    }

    /// A minimal *valid* node: two declared params, both paged, nothing else.
    /// Every case below is this with exactly one thing broken, so a failure
    /// names the defect rather than the fixture.
    fn clean() -> CapabilityDocument {
        CapabilityDocument {
            name: "Test".into(),
            vendor: "test".into(),
            version: (0, 1, 0),
            ports: vec![],
            params: vec![pd(A, "alpha"), pd(B, "beta")],
            extensions: vec![],
            view: Some(Rule {
                name: Cow::Borrowed("Test"),
                page_groups: Cow::Owned(vec![Cow::Borrowed("SRC")]),
                param_pages: Cow::Owned(vec![page(A, 0), page(B, 1)]),
                macros: Cow::Borrowed(&[]),
                affordances: Cow::Borrowed(&[]),
                envelopes: Cow::Borrowed(&[]),
                routing: Cow::Borrowed(&[]),
                diagram: None,
                view_overrides: Cow::Borrowed(&[]),
                variants: Cow::Borrowed(&[]),
            }),
        }
    }

    fn with_rule(f: impl FnOnce(&mut Rule)) -> CapabilityDocument {
        let mut doc = clean();
        let mut rule = doc.view.take().unwrap();
        f(&mut rule);
        doc.view = Some(rule);
        doc
    }

    /// Two machines that both page `alpha` and both flag it `identity` —
    /// the shape a real machine host has.
    fn variant(name: &'static str, value: u32, identity: bool) -> MachineVariant {
        MachineVariant {
            value,
            name: Cow::Borrowed(name),
            page_groups: Cow::Owned(vec![Cow::Borrowed("SRC")]),
            pages: Cow::Owned(vec![page(A, 0), page(B, 1)]),
            overlays: Cow::Owned(vec![(
                A,
                ParamOverlay {
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    identity,
                },
            )]),
            param_labels: Cow::Borrowed(&[]),
        }
    }

    /// The baseline the whole module rests on: a well-formed node reports
    /// nothing. Without this, every case below could pass on a validator that
    /// always complains.
    #[test]
    fn a_clean_declaration_reports_nothing() {
        assert_eq!(validate_view(&clean()), Vec::new());
    }

    #[test]
    fn a_node_with_no_view_reports_nothing() {
        let mut doc = clean();
        doc.view = None;
        assert!(validate_view(&doc).is_empty());
    }

    /// Every defect class, one deliberately-malformed fixture each.
    ///
    /// **This table is the point of the module.** `validate_view` exists to
    /// fire, and every in-tree node is clean — so without a negative case per
    /// branch, most of it had never executed even once (43% line coverage
    /// after MM-C8b). An assertion nothing has ever seen fire is an assertion
    /// nobody knows works.
    #[test]
    fn every_defect_class_is_reported() {
        struct Case {
            what: &'static str,
            doc: CapabilityDocument,
            expect: &'static str,
        }

        let cases = vec![
            Case {
                what: "page ref names an undeclared param (#47, #156)",
                doc: with_rule(|r| {
                    r.param_pages = Cow::Owned(vec![page(A, 0), page(B, 1), page(ABSENT, 2)]);
                }),
                expect: "does not declare",
            },
            Case {
                what: "a declared param is on no page (`tune` before MM-C4)",
                doc: with_rule(|r| {
                    r.param_pages = Cow::Owned(vec![page(A, 0)]);
                }),
                expect: "appears on no page",
            },
            Case {
                what: "an affordance names a param this view does not display",
                doc: with_rule(|r| {
                    r.affordances =
                        Cow::Owned(vec![(ABSENT, AffordanceHint::FilterShape)]);
                }),
                expect: "does not display",
            },
            Case {
                what: "an envelope curve points past the declared groups",
                doc: with_rule(|r| {
                    r.affordances = Cow::Owned(vec![(
                        A,
                        AffordanceHint::EnvelopeCurve { group_idx: 7 },
                    )]);
                }),
                expect: "envelope group 7",
            },
            Case {
                what: "an envelope group names a param this view does not display",
                doc: with_rule(|r| {
                    r.envelopes = Cow::Owned(vec![EnvelopeGroup {
                        env_type: Cow::Borrowed("AD"),
                        label: Cow::Borrowed("Amp"),
                        param_ids: [ABSENT, 0, 0, 0],
                    }]);
                }),
                expect: "does not display",
            },
            Case {
                what: "a macro targets a param this view does not display",
                doc: with_rule(|r| {
                    r.macros = Cow::Owned(vec![MacroBinding {
                        name: Cow::Borrowed("Sweep"),
                        targets: Cow::Owned(vec![ABSENT]),
                        curves: Cow::Owned(vec![MacroCurve::Linear]),
                        page: None,
                    }]);
                }),
                expect: "does not display",
            },
            Case {
                what: "routing names an undeclared param",
                doc: with_rule(|r| {
                    r.routing = Cow::Owned(vec![(
                        ABSENT,
                        RoutingSemantics {
                            destination: Cow::Borrowed("reverb"),
                            source_label: Cow::Borrowed("Kick"),
                        },
                    )]);
                }),
                expect: "does not declare",
            },
            Case {
                what: "a variant declares two overlays for one param",
                doc: with_rule(|r| {
                    let mut v = variant("One", 0, true);
                    let mut o = v.overlays.to_vec();
                    o.push(o[0]);
                    v.overlays = Cow::Owned(o);
                    r.variants = Cow::Owned(vec![v, variant("Two", 1, true)]);
                }),
                expect: "two overlays",
            },
            Case {
                what: "`identity` flagged on one machine but not its sibling",
                doc: with_rule(|r| {
                    r.variants =
                        Cow::Owned(vec![variant("One", 0, true), variant("Two", 1, false)]);
                }),
                expect: "not on",
            },
            Case {
                what: "a variant's `param_labels` names an undeclared param",
                doc: with_rule(|r| {
                    let mut v = variant("One", 0, true);
                    v.param_labels = Cow::Owned(vec![(
                        ABSENT,
                        Cow::Owned(vec![Some(Cow::Borrowed("off"))]),
                    )]);
                    r.variants = Cow::Owned(vec![v]);
                }),
                expect: "does not",
            },
        ];

        for c in cases {
            let defects = validate_view(&c.doc);
            assert!(
                defects.iter().any(|d| d.message.contains(c.expect)),
                "{}: expected a defect containing {:?}, got {:?}",
                c.what,
                c.expect,
                defects.iter().map(|d| d.to_string()).collect::<Vec<_>>()
            );
        }
    }

    /// A defect inside a variant names which machine it is in, so a message
    /// about a six-machine host is actionable.
    #[test]
    fn a_variant_defect_names_its_variant() {
        let doc = with_rule(|r| {
            let mut v = variant("HiHat", 1, true);
            v.pages = Cow::Owned(vec![page(A, 0), page(B, 1), page(ABSENT, 2)]);
            r.variants = Cow::Owned(vec![variant("Kick", 0, true), v]);
        });
        let defects = validate_view(&doc);
        let d = defects
            .iter()
            .find(|d| d.message.contains("does not declare"))
            .expect("the bad page ref is reported");
        assert_eq!(d.variant.as_deref(), Some("HiHat"));
        assert!(d.to_string().starts_with("[variant HiHat]"));
    }

    /// The distinction MM-C8 exists for: a param only *one* machine pages is
    /// legitimate and must not be reported, while an affordance naming it is
    /// reported against the machines that do not display it. Amendment 5 as
    /// literally written — "resolves in the union doc" — passes on both.
    #[test]
    fn a_param_only_one_machine_pages_is_legitimate() {
        let doc = with_rule(|r| {
            let mut only_a = variant("Sparse", 1, true);
            only_a.pages = Cow::Owned(vec![page(A, 0)]);
            r.variants = Cow::Owned(vec![variant("Full", 0, true), only_a]);
        });
        assert_eq!(
            validate_view(&doc),
            Vec::new(),
            "`beta` is paged by one machine and absent from the other, which is \
             exactly what per-machine pages are for"
        );
    }

    #[test]
    fn debug_assert_view_accepts_a_clean_declaration() {
        debug_assert_view(&clean());
    }

    #[test]
    #[should_panic(expected = "invalid view declaration")]
    fn debug_assert_view_panics_on_a_defect() {
        debug_assert_view(&with_rule(|r| {
            r.param_pages = Cow::Owned(vec![page(A, 0), page(B, 1), page(ABSENT, 2)]);
        }));
    }
}
