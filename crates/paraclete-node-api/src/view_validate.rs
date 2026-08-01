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
