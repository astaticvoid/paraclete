//! Composite track-rule assembly shared by Antiphon and Theotokos (TK1 C2, ADR-036).
//!
//! This crate depends only on `paraclete-node-api` (L2). Both Antiphon
//! (the web server) and Theotokos (the terminal) consume it, so the canonical
//! page order and assembly logic agree by construction.
//!
//! ## Changes from the pre-extraction Antiphon `view.rs`
//!
//! 1. `affordance` is the `AffordanceHint` value; JSON-string conversion lives
//!    in Antiphon's mapping layer.
//! 2. Routes carry no `value` field (today hardcoded `0.0`); Antiphon adds its
//!    placeholder at mapping time.
//! 3. `chain` lists **rule-bearing chain nodes only, engine first** — viewless
//!    chain nodes stay invisible, matching the current wire.
//! 4. Param-name + display-name lookup goes through `NodeInfo` — the
//!    intersection both sides already have (cap-doc + instrument labels).

use std::borrow::Cow;
use std::collections::HashMap;

use paraclete_node_api::{AffordanceHint, MachineVariant, PageRef, Rule};

pub const CANONICAL_PAGE_ORDER: [&str; 6] = ["TRIG", "SRC", "FLTR", "AMP", "FX", "MOD"];

/// Slots per sub-page — one encoder bank's worth (`PageRef::slot` documents
/// 0–7 as sub-page 1, 8–15 as sub-page 2, …). Each contributor to a merged
/// page is padded to a multiple of this so one node's params never straddle a
/// sub-page boundary (ADR-042 §0 A3).
///
/// Import this rather than writing `8` — Theotokos's sub-page windowing reads
/// it, and a private copy is how `PageNav.tsx` drifted from the canonical page
/// order.
///
/// Ceiling: `PageRef::slot` is a `u8`, so a page cannot exceed 32 sub-pages.
/// Past that the offset arithmetic saturates and contributors stack at slot
/// 255. That needs 31 rule-bearing nodes contributing to one page of a single
/// track chain; the `debug_assert` in `merge_page` catches it in a debug build.
pub const SUB_PAGE_SLOTS: u8 = 8;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub display_name: Option<String>,
    pub params: Vec<ParamInfo>,
}

impl NodeInfo {
    fn param(&self, id: u32) -> Option<&ParamInfo> {
        self.params.iter().find(|p| p.id == id)
    }
}

/// The slice of a `ParamDescriptor` assembly needs. Was a bare
/// `(u32, String)` pair through TK1; MM-C5 needs `stepped` for the wire
/// (ADR-041 amendment 3) and `default` to resolve which machine variant is
/// active without a second lookup path.
#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    /// Integer-stepped parameter (`ParamDescriptor::stepped`).
    pub stepped: bool,
    /// The cap-doc default. For a machine host's identity param this **is**
    /// the machine it was constructed on: `union_params(active)` gives the
    /// identity param `active.value()` as its default
    /// (`analog_engine.rs:224`), and both engines carry a test pinning that.
    /// It is what lets `resolve_variant` pick the right machine with no live
    /// state.
    ///
    /// Only *ranges* are `active`-independent. Every shared param's default
    /// varies with `active` too (`analog_engine.rs:240-242`), and so does the
    /// doc's `name` — the identity param is the reliable oracle because its
    /// default is defined to be the selection, not because nothing else moves.
    pub default: f64,
}

#[derive(Clone, Debug)]
pub struct TrackChain {
    pub engine_node_id: u32,
    pub chain_ids: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct CompositeView {
    pub engine_node_id: u32,
    pub engine_name: String,
    pub display_name: String,
    /// The merged pages for the *currently selected* machine of every
    /// variant-bearing node in the chain.
    pub pages: Vec<CompositePage>,
    /// Rule-bearing chain nodes, engine first.
    pub chain: Vec<u32>,
    pub routes: Vec<CompositeRoute>,
    /// ADR-041: one entry per chain node that is a machine host, in chain
    /// order (engine first). Empty for a chain of ordinary nodes, which is
    /// every track that does not carry `AnalogEngine` or `FmEngine`.
    pub variants: Vec<CompositeVariantSet>,
}

/// Every machine one node can be, each with its pages already merged against
/// the rest of the chain.
///
/// **Pre-merged, not raw.** A surface swapping machines re-reads
/// `variants[i].pages` and draws it; it never re-runs the merge. Sending raw
/// `MachineVariant`s instead would make every client re-implement 8-slot
/// contributor alignment, and clients drift — `PageNav.tsx` already kept a
/// private copy of the canonical page order.
#[derive(Clone, Debug)]
pub struct CompositeVariantSet {
    pub node_id: u32,
    /// Param whose value selects the machine — the one flagged `identity` in
    /// the node's overlays. `None` if the node declares variants but flags no
    /// identity param, in which case `active` is the first variant and
    /// nothing can move it.
    pub select_param: Option<u32>,
    /// `select_param`'s name. Carried alongside the id because the two
    /// consumers want different halves: Theotokos addresses params by id, the
    /// wire by name.
    ///
    /// `None` when the id resolves to no cap-doc param — a page param in that
    /// position gets a `param_{id}` placeholder, but this one addresses a
    /// *write*: it is the param a client sets to change machine, and
    /// `set_param` takes a name. A placeholder here would be a control that
    /// looks live and silently does nothing, which is the `param_{id}` defect
    /// #47 and #156 are both instances of. So it can be `None` for two
    /// reasons — no identity flag, or a flag on an undeclared param.
    pub select_param_name: Option<String>,
    /// The variant `CompositeView::pages` was merged for.
    pub active: u32,
    pub variants: Vec<CompositeVariant>,
}

#[derive(Clone, Debug)]
pub struct CompositeVariant {
    pub value: u32,
    pub name: String,
    /// The whole track's pages with this node on this machine — not just this
    /// node's contribution.
    pub pages: Vec<CompositePage>,
    /// This machine's display ranges (ADR-041 §0 A1). The host's bank stores
    /// the union and is never narrowed to these; a surface displays and clamps
    /// *input* against them.
    pub overlays: Vec<CompositeOverlay>,
}

/// One machine's range for one param. Flattened from
/// `paraclete_node_api::ParamOverlay` with the param's name resolved, so
/// neither consumer has to re-derive it.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeOverlay {
    pub param_id: u32,
    pub param_name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// This param *is* the node's identity rather than a setting. Rejected as
    /// a p-lock target and as a scene-morph destination (ADR-041 §0 A4).
    pub identity: bool,
}

#[derive(Clone, Debug)]
pub struct CompositePage {
    pub id: String,
    pub label: String,
    pub params: Vec<CompositeParam>,
    pub envelopes: Vec<CompositeEnvelope>,
    pub macros: Vec<CompositeMacro>,
}

#[derive(Clone, Debug)]
pub struct CompositeParam {
    pub node_id: u32,
    pub param_id: u32,
    pub name: String,
    pub label: String,
    pub affordance: AffordanceHint,
    pub env_group: Option<u32>,
    pub slot: u8,
    pub routing: Option<String>,
    /// Integer-stepped: a client draws discrete detents, not a continuous arc.
    pub stepped: bool,
    /// Named values for a stepped param, **indexed by the param's value**, so
    /// `options[v]` labels value `v` and an inner `None` is a value with no
    /// name. Populated for a machine host's identity param — the machine
    /// names — and `None` otherwise. `ParamDescriptor::display` cannot supply
    /// these: `ParamDisplayAdapter::Dynamic` panics on clone and the cap-doc
    /// path clones (ADR-042 amendment 5, `capability.rs:63`).
    pub options: Option<Vec<Option<String>>>,
}

#[derive(Clone, Debug)]
pub struct CompositeEnvelope {
    pub id: u32,
    pub env_type: String,
    pub label: String,
    pub params: Vec<(u32, String)>, // (param_id, param_name)
}

#[derive(Clone, Debug)]
pub struct CompositeMacro {
    pub name: String,
    pub targets: Vec<(u32, String)>, // (param_id, param_name)
    pub page: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CompositeRoute {
    pub source: u32,
    pub dest: String,
    pub param_id: u32,
    pub param_name: String,
}

// ── Assembly ──────────────────────────────────────────────────────────────────

/// Build a `CompositeView` for a single track, or `None` if the track's engine
/// has no view Rule.
///
/// Each machine host in the chain is drawn on the machine its cap-doc says it
/// was built with. A caller holding live state — a surface watching
/// `/node/{id}/param/machine` — passes it to [`assemble_for`] instead.
pub fn assemble(
    rules: &HashMap<u32, Rule>,
    chains: &[TrackChain],
    track_id: u32,
    nodes: &HashMap<u32, NodeInfo>,
) -> Option<CompositeView> {
    assemble_for(rules, chains, track_id, nodes, &HashMap::new())
}

/// [`assemble`], with an explicit machine selection per node id.
///
/// A node absent from `active` falls back to its cap-doc default (see
/// [`ParamInfo::default`]), so passing an empty map reproduces `assemble`.
/// A value naming no declared variant clamps to the last, matching the
/// engines' own `from_value`.
pub fn assemble_for(
    rules: &HashMap<u32, Rule>,
    chains: &[TrackChain],
    track_id: u32,
    nodes: &HashMap<u32, NodeInfo>,
    active: &HashMap<u32, u32>,
) -> Option<CompositeView> {
    let chain = chains.get(track_id as usize)?;
    let engine_rule = rules.get(&chain.engine_node_id)?;

    let mut chain_rules: Vec<(u32, &Rule)> = vec![(chain.engine_node_id, engine_rule)];
    for &nid in &chain.chain_ids {
        if let Some(r) = rules.get(&nid) {
            chain_rules.push((nid, r));
        }
    }

    let engine_display_name = nodes
        .get(&chain.engine_node_id)
        .and_then(|info| info.display_name.as_deref())
        .unwrap_or(engine_rule.name.as_ref())
        .to_string();

    let routes: Vec<CompositeRoute> = chain_rules
        .iter()
        .flat_map(|(nid, r)| {
            r.routing.iter().map(move |(pid, sem)| CompositeRoute {
                source: *nid,
                dest: sem.destination.to_string(),
                param_id: *pid,
                param_name: param_name(nodes, *nid, *pid),
            })
        })
        .collect();

    // Which machine each variant-bearing node is on, by position in
    // `chain_rules`. Nodes with no variants are absent.
    let selection: HashMap<u32, usize> = chain_rules
        .iter()
        .filter(|(_, r)| !r.variants.is_empty())
        .map(|(nid, r)| (*nid, resolve_variant(r, nodes.get(nid), active.get(nid).copied())))
        .collect();

    let pages = build_pages(&chain_rules, &selection, nodes);

    // Every machine each host can be, with the whole track's pages already
    // merged for it. Other hosts stay on their selected machine — the
    // cross-product of two hosts' machines is not represented, and nothing in
    // the tree has two hosts in one chain (both engines are track sources).
    let variants: Vec<CompositeVariantSet> = chain_rules
        .iter()
        .filter(|(_, r)| !r.variants.is_empty())
        .map(|(nid, r)| {
            let chosen = selection[nid];
            let select_param = identity_param(r);
            CompositeVariantSet {
                node_id: *nid,
                select_param,
                select_param_name: select_param
                    .and_then(|pid| nodes.get(nid)?.param(pid))
                    .map(|p| p.name.clone()),
                active: r.variants[chosen].value,
                variants: r
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let mut sel = selection.clone();
                        sel.insert(*nid, i);
                        CompositeVariant {
                            value: v.value,
                            name: v.name.to_string(),
                            pages: build_pages(&chain_rules, &sel, nodes),
                            overlays: v
                                .overlays
                                .iter()
                                .map(|(pid, o)| CompositeOverlay {
                                    param_id: *pid,
                                    param_name: param_name(nodes, *nid, *pid),
                                    min: o.min,
                                    max: o.max,
                                    default: o.default,
                                    identity: o.identity,
                                })
                                .collect(),
                        }
                    })
                    .collect(),
            }
        })
        .collect();

    // chain lists rule-bearing nodes only, engine first
    let chain_nodes: Vec<u32> = chain_rules.iter().map(|(nid, _)| *nid).collect();

    Some(CompositeView {
        engine_node_id: chain.engine_node_id,
        engine_name: engine_rule.name.to_string(),
        display_name: engine_display_name,
        pages,
        chain: chain_nodes,
        routes,
        variants,
    })
}

// ── Variant resolution ────────────────────────────────────────────────────────

/// The param whose value selects this node's machine: the one its variants
/// flag `identity` (ADR-041 §0 A1).
///
/// Read from whichever variant declares it first. The flag is meant to be
/// repeated in every variant and MM-C8 asserts that; taking the first means a
/// node that missed one still selects on the right param here, so a miss shows
/// up as *only* the lock-rejection failure it is, not as a second symptom.
fn identity_param(rule: &Rule) -> Option<u32> {
    rule.variants.iter().find_map(|v| {
        v.overlays
            .iter()
            .find(|(_, o)| o.identity)
            .map(|(id, _)| *id)
    })
}

/// Index into `rule.variants` of the machine to draw.
fn resolve_variant(rule: &Rule, info: Option<&NodeInfo>, override_value: Option<u32>) -> usize {
    let value = override_value.or_else(|| {
        let pid = identity_param(rule)?;
        let d = info?.param(pid)?.default;
        // Negative or non-finite means the node did not declare a usable
        // identity default; fall through to the first variant rather than
        // wrapping a cast.
        (d.is_finite() && d >= 0.0).then_some(d as u32)
    });
    match value {
        None => 0,
        Some(v) => rule
            .variants
            .iter()
            .position(|x| x.value == v)
            // A value naming no variant falls back to the last one, so the
            // engines' own `from_value` — which clamps rather than panicking
            // on a malformed project (`analog_engine.rs:43`) — and this agree
            // on what a surface draws. They agree *exactly* only for the
            // contiguous `0..N-1` value sets both engines declare: theirs
            // clamps an index into `ALL`, this falls back after a failed
            // lookup by value, and for a sparse set those differ.
            .unwrap_or(rule.variants.len().saturating_sub(1)),
    }
}

/// The variant in force for a node, or `None` when it declares none — in which
/// case the caller uses the rule's base fields.
fn active_variant<'a>(
    rule: &'a Rule,
    node_id: u32,
    selection: &HashMap<u32, usize>,
) -> Option<&'a MachineVariant> {
    let idx = *selection.get(&node_id)?;
    rule.variants.get(idx)
}

/// One chain node's contribution with its variant already resolved, so
/// `merge_page` never re-decides which machine is showing.
struct Contributor<'a> {
    node_id: u32,
    rule: &'a Rule,
    page_groups: &'a [Cow<'static, str>],
    param_pages: &'a [(u32, PageRef)],
    /// Value-indexed machine names, for the identity param only.
    options: Option<Vec<Option<String>>>,
    identity_param: Option<u32>,
}

fn build_pages(
    chain_rules: &[(u32, &Rule)],
    selection: &HashMap<u32, usize>,
    nodes: &HashMap<u32, NodeInfo>,
) -> Vec<CompositePage> {
    let contributors: Vec<Contributor> = chain_rules
        .iter()
        .map(|&(nid, rule)| match active_variant(rule, nid, selection) {
            Some(v) => Contributor {
                node_id: nid,
                rule,
                page_groups: &v.page_groups,
                param_pages: &v.pages,
                options: machine_options(rule),
                identity_param: identity_param(rule),
            },
            None => Contributor {
                node_id: nid,
                rule,
                page_groups: &rule.page_groups,
                param_pages: &rule.param_pages,
                options: None,
                identity_param: None,
            },
        })
        .collect();

    let mut pages_by_group: HashMap<&str, Vec<&Contributor>> = HashMap::new();
    for c in &contributors {
        for pg in c.page_groups.iter() {
            pages_by_group.entry(pg.as_ref()).or_default().push(c);
        }
    }

    let mut pages: Vec<CompositePage> = Vec::new();
    for &pg_name in &CANONICAL_PAGE_ORDER {
        if let Some(contributors) = pages_by_group.remove(pg_name) {
            if let Some(page) = merge_page(pg_name, &contributors, nodes) {
                pages.push(page);
            }
        }
    }
    // Custom pages (beyond the standard set) in alphabetical order
    let mut custom_keys: Vec<&str> = pages_by_group.keys().copied().collect();
    custom_keys.sort_unstable();
    for key in custom_keys {
        if let Some(contributors) = pages_by_group.remove(key) {
            if let Some(page) = merge_page(key, &contributors, nodes) {
                pages.push(page);
            }
        }
    }
    pages
}

/// Upper bound on an identity param's value before its labels are dropped.
///
/// `machine_options` allocates one slot per value up to the highest declared,
/// so a node that declares `value: 4_000_000_000` would otherwise allocate 4
/// billion entries. Both shipped engines declare 3.
const MAX_DENSE_OPTIONS: u32 = 256;

/// Machine names indexed **by value**, not by declaration position.
///
/// A client reads `options[value]`, so a sparse or out-of-order `value` set
/// would mislabel every machine after the gap. Filling by value rather than
/// pushing in order makes that unrepresentable instead of an unenforced
/// contract.
///
/// **A gap is `None`, not a synthesized label.** An earlier draft filled
/// unclaimed indices with the index rendered as a string, which invents
/// machines: a node declaring `{0, 3}` shipped `["Zeroth","1","2","Third"]`,
/// so a client drew four choices and two of them selected nothing (the
/// engines' `from_value` clamps an unknown value). The authoritative list is
/// always `CompositeVariantSet::variants`, which is sparse-correct by
/// construction; this is the by-value view of the same data and must not
/// disagree with it.
///
/// `None` for the whole list when the values are too spread to index densely
/// — see [`MAX_DENSE_OPTIONS`]. A caller that must label a machine in that
/// case reads `variants` directly.
fn machine_options(rule: &Rule) -> Option<Vec<Option<String>>> {
    let top = rule.variants.iter().map(|v| v.value).max()?;
    if top >= MAX_DENSE_OPTIONS {
        return None;
    }
    let mut out = vec![None; top as usize + 1];
    for v in rule.variants.iter() {
        out[v.value as usize] = Some(v.name.to_string());
    }
    Some(out)
}

fn param_name(nodes: &HashMap<u32, NodeInfo>, node_id: u32, param_id: u32) -> String {
    nodes
        .get(&node_id)
        .and_then(|info| info.param(param_id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| format!("param_{}", param_id))
}

fn merge_page(
    group_name: &str,
    contributors: &[&Contributor],
    nodes: &HashMap<u32, NodeInfo>,
) -> Option<CompositePage> {
    let mut params: Vec<CompositeParam> = Vec::new();
    let mut envelopes: Vec<CompositeEnvelope> = Vec::new();
    let mut macros: Vec<CompositeMacro> = Vec::new();
    let mut envelopes_offset: u32 = 0;
    // Where this contributor's declared slots start. Each contributor is
    // padded to a whole number of 8-slot sub-pages (ADR-042 §0 A3) so a
    // second node's params can never straddle a sub-page boundary — the
    // performer pages through one node's controls at a time.
    let mut contributor_base: u8 = 0;

    for c in contributors {
        let (nid, rule) = (c.node_id, c.rule);
        let mut max_slot: Option<u8> = None;
        for (param_id, page_ref) in c.param_pages.iter() {
            if page_ref.page.as_ref() != group_name {
                continue;
            }
            // ADR-041 §0 A2 prerequisite: honor the slot the node declared.
            // This used to be a sequential counter that ignored `page_ref.slot`
            // entirely, which made `PageRef::slot` documentation-only — while
            // Theotokos's *non*-composite path (model.rs) already sorted by the
            // declared value, so the same param could land in two different
            // places depending on which path drew it.
            let slot = contributor_base.saturating_add(page_ref.slot);
            debug_assert!(
                !params
                    .iter()
                    .any(|p: &CompositeParam| p.node_id == nid && p.slot == slot),
                "node {nid} declares two params at slot {} of page {group_name} — \
                 one would silently cover the other",
                page_ref.slot
            );
            max_slot = Some(max_slot.map_or(page_ref.slot, |m: u8| m.max(page_ref.slot)));
            let env_group = rule
                .affordances
                .iter()
                .find(|(pid, _)| *pid == *param_id)
                .and_then(|(_, hint)| match hint {
                    AffordanceHint::EnvelopeCurve { group_idx } => {
                        Some(*group_idx as u32 + envelopes_offset)
                    }
                    _ => None,
                });

            let affordance = rule
                .affordances
                .iter()
                .find(|(pid, _)| *pid == *param_id)
                .map(|(_, a)| a.clone())
                .unwrap_or(AffordanceHint::None);

            let pname = param_name(nodes, nid, *param_id);

            let routing = rule
                .routing
                .iter()
                .find(|(pid, _)| *pid == *param_id)
                .map(|(_, sem)| sem.destination.to_string());

            // The identity param is stepped over the machine set whether or
            // not the node's descriptor says so; every other param takes the
            // cap-doc's word for it.
            let is_identity = c.identity_param == Some(*param_id);
            let stepped = is_identity
                || nodes
                    .get(&nid)
                    .and_then(|info| info.param(*param_id))
                    .is_some_and(|p| p.stepped);

            params.push(CompositeParam {
                node_id: nid,
                param_id: *param_id,
                name: pname.clone(),
                label: pname,
                affordance,
                env_group,
                slot,
                routing,
                stepped,
                options: if is_identity { c.options.clone() } else { None },
            });
        }

        for env in rule.envelopes.iter() {
            let pids: Vec<(u32, String)> = env
                .param_ids
                .iter()
                .filter(|&&id| id != 0)
                .map(|&id| (id, param_name(nodes, nid, id)))
                .collect();
            envelopes.push(CompositeEnvelope {
                id: envelopes_offset,
                env_type: env.env_type.to_string(),
                label: env.label.to_string(),
                params: pids,
            });
            envelopes_offset = envelopes_offset.saturating_add(1);
        }

        for m in rule.macros.iter() {
            let targets: Vec<(u32, String)> = m
                .targets
                .iter()
                .map(|tid| (*tid, param_name(nodes, nid, *tid)))
                .collect();
            macros.push(CompositeMacro {
                name: m.name.to_string(),
                targets,
                page: m.page.as_ref().map(|p| p.to_string()),
            });
        }

        // Advance past this contributor, rounded up to a whole sub-page. A
        // contributor that put nothing on this page consumes nothing.
        if let Some(max) = max_slot {
            let sub_pages = (max / SUB_PAGE_SLOTS).saturating_add(1);
            contributor_base =
                contributor_base.saturating_add(sub_pages.saturating_mul(SUB_PAGE_SLOTS));
        }
    }

    // Declared slots need not arrive in order, and consumers that read the
    // list positionally (rather than sorting by `slot`) should still see it
    // laid out the way it renders.
    params.sort_by_key(|p| p.slot);

    if params.is_empty() {
        return None;
    }

    let label = match group_name {
        "SRC" => "Source",
        "AMP" => "Amp",
        "FLTR" => "Filter",
        "FX" => "Effects",
        "TRIG" => "Trig",
        "MOD" => "Modulation",
        other => other,
    };

    Some(CompositePage {
        id: group_name.to_string(),
        label: label.to_string(),
        params,
        envelopes,
        macros,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{EnvelopeGroup, PageRef, ParamOverlay};
    use std::borrow::Cow;

    fn node_info(display_name: &str, params: &[(&str, u32)]) -> NodeInfo {
        NodeInfo {
            display_name: Some(display_name.to_string()),
            params: params
                .iter()
                .map(|(n, pid)| ParamInfo {
                    id: *pid,
                    name: n.to_string(),
                    stepped: false,
                    default: 0.0,
                })
                .collect(),
        }
    }

    /// Like `node_info`, but every param carries an explicit `stepped` flag
    /// and cap-doc default — the two fields variant resolution and the wire
    /// read.
    fn node_info_full(display_name: &str, params: &[(&str, u32, bool, f64)]) -> NodeInfo {
        NodeInfo {
            display_name: Some(display_name.to_string()),
            params: params
                .iter()
                .map(|&(n, pid, stepped, default)| ParamInfo {
                    id: pid,
                    name: n.to_string(),
                    stepped,
                    default,
                })
                .collect(),
        }
    }

    /// Slots are assigned sequentially **per page**, the way a real node
    /// declares them. The old helper hardcoded `slot: 0` for every param —
    /// harmless while `merge_page` ignored the field, and a fixture that
    /// described a node no node could be once it stopped ignoring it.
    /// Use `make_rule_slotted` where a test needs specific slots.
    fn make_rule(name: &str, pages: &[&str], param_pages: &[(u32, &str)]) -> Rule {
        let mut next: HashMap<&str, u8> = HashMap::new();
        let slotted: Vec<(u32, &str, u8)> = param_pages
            .iter()
            .map(|&(pid, pg)| {
                let slot = next.entry(pg).or_insert(0);
                let assigned = *slot;
                *slot += 1;
                (pid, pg, assigned)
            })
            .collect();
        make_rule_slotted(name, pages, &slotted)
    }

    fn make_rule_slotted(name: &str, pages: &[&str], param_pages: &[(u32, &str, u8)]) -> Rule {
        Rule {
            name: Cow::Owned(name.to_string()),
            page_groups: Cow::Owned(pages.iter().map(|&s| Cow::Owned(s.to_string())).collect()),
            param_pages: Cow::Owned(
                param_pages
                    .iter()
                    .map(|&(pid, pg, slot)| {
                        (
                            pid,
                            PageRef {
                                page: Cow::Owned(pg.to_string()),
                                slot,
                            },
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        }
    }

    /// A machine host: `MACHINE_PID` is its identity param, and each machine
    /// names its own params and slots. `param_pages` is set to the machine at
    /// `active_idx`, the way both engines build their base fields from the
    /// machine they were constructed on.
    const MACHINE_PID: u32 = 900;

    /// `(machine value, machine name, [(param id, page, slot)])`.
    type MachineSpec<'a> = (u32, &'a str, &'a [(u32, &'a str, u8)]);

    fn make_machine_rule(name: &str, active_idx: usize, machines: &[MachineSpec]) -> Rule {
        let variants: Vec<MachineVariant> = machines
            .iter()
            .map(|&(value, mname, refs)| {
                let mut groups: Vec<Cow<'static, str>> = Vec::new();
                for &(_, pg, _) in refs {
                    let pg = pg.to_string();
                    if !groups.iter().any(|g| g.as_ref() == pg) {
                        groups.push(Cow::Owned(pg));
                    }
                }
                let mut overlays: Vec<(u32, ParamOverlay)> = vec![(
                    MACHINE_PID,
                    ParamOverlay {
                        min: 0.0,
                        max: (machines.len() - 1) as f64,
                        default: value as f64,
                        identity: true,
                    },
                )];
                for &(pid, _, _) in refs {
                    overlays.push((
                        pid,
                        ParamOverlay {
                            // A distinguishable per-machine range, so a test
                            // can tell which machine's overlay it is holding.
                            min: 0.0,
                            max: value as f64 + 1.0,
                            default: 0.0,
                            identity: false,
                        },
                    ));
                }
                MachineVariant {
                    value,
                    name: Cow::Owned(mname.to_string()),
                    page_groups: Cow::Owned(groups),
                    pages: Cow::Owned(
                        refs.iter()
                            .map(|&(pid, pg, slot)| {
                                (
                                    pid,
                                    PageRef {
                                        page: Cow::Owned(pg.to_string()),
                                        slot,
                                    },
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                    overlays: Cow::Owned(overlays),
                }
            })
            .collect();

        let active = &machines[active_idx];
        let mut rule = make_rule_slotted(name, &[], active.2);
        rule.page_groups = variants[active_idx].page_groups.clone();
        rule.variants = Cow::Owned(variants);
        rule
    }

    #[test]
    fn canonical_order_is_trig_first() {
        assert_eq!(CANONICAL_PAGE_ORDER, ["TRIG", "SRC", "FLTR", "AMP", "FX", "MOD"]);
    }

    #[test]
    fn assemble_merges_engine_and_chain_pages_in_canonical_order() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_rule("KickEngine", &["AMP", "SRC"], &[(1, "AMP"), (2, "SRC")]),
        );
        rules.insert(30, make_rule("Dist", &["FX"], &[(1, "FX")]));

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("MyKick", &[("decay", 1), ("drive", 2)]));
        nodes.insert(30, node_info("Dist", &[("drive", 1)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.pages.len(), 3);
        assert_eq!(cv.pages[0].id, "SRC");
        assert_eq!(cv.pages[1].id, "AMP");
        assert_eq!(cv.pages[2].id, "FX");
        assert_eq!(cv.chain, vec![20, 30]);
        assert_eq!(cv.display_name, "MyKick");
    }

    #[test]
    fn assemble_custom_pages_alphabetical_after_canonical() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_rule("Kick", &["ZETA", "BETA"], &[(1, "ZETA"), (2, "BETA")]),
        );

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Kick", &[("a", 1), ("b", 2)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.pages.len(), 2);
        assert_eq!(cv.pages[0].id, "BETA");
        assert_eq!(cv.pages[1].id, "ZETA");
    }

    #[test]
    fn assemble_envelope_group_indices_offset_across_nodes() {
        let mut rules = HashMap::new();
        let mut r1 = make_rule("A", &["AMP"], &[(1, "AMP"), (2, "AMP")]);
        r1.envelopes = Cow::Owned(vec![EnvelopeGroup {
            env_type: "AD".into(),
            label: "EnvA".into(),
            param_ids: [1, 0, 0, 0],
        }]);
        r1.affordances = Cow::Owned(vec![(1, AffordanceHint::EnvelopeCurve { group_idx: 0 })]);
        rules.insert(20, r1);

        let mut r2 = make_rule("B", &["AMP"], &[(3, "AMP")]);
        r2.envelopes = Cow::Owned(vec![EnvelopeGroup {
            env_type: "AR".into(),
            label: "EnvB".into(),
            param_ids: [3, 0, 0, 0],
        }]);
        r2.affordances = Cow::Owned(vec![(3, AffordanceHint::EnvelopeCurve { group_idx: 0 })]);
        rules.insert(30, r2);

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("A", &[("p1", 1), ("p2", 2)]));
        nodes.insert(30, node_info("B", &[("p3", 3)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.pages[0].envelopes.len(), 2);
        assert_eq!(cv.pages[0].envelopes[0].id, 0);
        assert_eq!(cv.pages[0].envelopes[1].id, 1);
    }

    /// ADR-041 §0 A2 prerequisite. `merge_page` used to assign slots from a
    /// sequential counter and never read `PageRef::slot`, so a node could
    /// declare slot 5 and be drawn at 0. Both ADR-041 (machine-select on a
    /// specific TRIG slot) and ADR-042 (8-aligned MOD contributions) need the
    /// declared value to be the rendered value.
    #[test]
    fn declared_slots_are_honored_not_renumbered() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_rule_slotted("Eng", &["SRC"], &[(1, "SRC", 5), (2, "SRC", 0)]),
        );

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Eng", &[("tune", 1), ("decay", 2)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let page = &cv.pages[0];

        let slot_of = |pid: u32| page.params.iter().find(|p| p.param_id == pid).unwrap().slot;
        assert_eq!(slot_of(1), 5, "param declared at slot 5 must land at 5");
        assert_eq!(slot_of(2), 0, "param declared at slot 0 must land at 0");
        // Declaration order was 5 then 0; the emitted list is slot-ordered.
        assert_eq!(
            page.params.iter().map(|p| p.slot).collect::<Vec<_>>(),
            vec![0, 5]
        );
    }

    /// ADR-042 §0 A3: a second contributor starts on a fresh sub-page, so its
    /// params can never straddle a boundary the performer pages across.
    #[test]
    fn second_contributor_starts_on_a_fresh_sub_page() {
        let mut rules = HashMap::new();
        // Engine uses slots 0..=2 — well inside the first sub-page.
        rules.insert(
            20,
            make_rule_slotted("Eng", &["FLTR"], &[(1, "FLTR", 0), (2, "FLTR", 2)]),
        );
        // Chain node also declares from 0; it must not collide with the engine.
        rules.insert(30, make_rule_slotted("Chn", &["FLTR"], &[(3, "FLTR", 0)]));

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Eng", &[("cutoff", 1), ("resonance", 2)]));
        nodes.insert(30, node_info("Chn", &[("drive", 3)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let page = &cv.pages[0];

        let of = |nid: u32, pid: u32| {
            page.params
                .iter()
                .find(|p| p.node_id == nid && p.param_id == pid)
                .unwrap()
                .slot
        };
        assert_eq!(of(20, 1), 0);
        assert_eq!(of(20, 2), 2);
        assert_eq!(
            of(30, 3),
            SUB_PAGE_SLOTS,
            "the chain node's slot 0 must be re-based onto sub-page 2, not \
             collide with the engine's slot 0"
        );
        // No two params share a slot.
        let mut slots: Vec<u8> = page.params.iter().map(|p| p.slot).collect();
        let before = slots.len();
        slots.dedup();
        assert_eq!(slots.len(), before, "slots must be unique across the page");
    }

    /// A contributor spilling past slot 7 consumes both sub-pages, so the next
    /// contributor starts at 16 rather than overlapping the spill.
    #[test]
    fn contributor_spanning_two_sub_pages_reserves_both() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_rule_slotted("Eng", &["SRC"], &[(1, "SRC", 0), (2, "SRC", 9)]),
        );
        rules.insert(30, make_rule_slotted("Chn", &["SRC"], &[(3, "SRC", 0)]));

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Eng", &[("a", 1), ("b", 2)]));
        nodes.insert(30, node_info("Chn", &[("c", 3)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let page = &cv.pages[0];

        let chn = page.params.iter().find(|p| p.node_id == 30).unwrap();
        assert_eq!(
            chn.slot,
            SUB_PAGE_SLOTS * 2,
            "an engine reaching slot 9 occupies two sub-pages; the next \
             contributor starts after both"
        );
    }

    /// A node that contributes nothing to this page must not push the next
    /// contributor down an empty sub-page.
    #[test]
    fn contributor_absent_from_a_page_consumes_no_slots() {
        let mut rules = HashMap::new();
        rules.insert(20, make_rule_slotted("Eng", &["SRC"], &[(1, "SRC", 0)]));
        // Declares the SRC page group but puts no param on it.
        rules.insert(30, make_rule_slotted("Mid", &["SRC"], &[]));
        rules.insert(40, make_rule_slotted("Chn", &["SRC"], &[(3, "SRC", 0)]));

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Eng", &[("a", 1)]));
        nodes.insert(30, node_info("Mid", &[]));
        nodes.insert(40, node_info("Chn", &[("c", 3)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30, 40],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let page = &cv.pages[0];

        let chn = page.params.iter().find(|p| p.node_id == 40).unwrap();
        assert_eq!(
            chn.slot, SUB_PAGE_SLOTS,
            "the empty contributor between them must not reserve a sub-page"
        );
    }

    #[test]
    fn assemble_param_carries_owning_node_id() {
        let mut rules = HashMap::new();
        rules.insert(20, make_rule("Eng", &["AMP"], &[(1, "AMP")]));
        rules.insert(30, make_rule("Chn", &["AMP"], &[(1, "AMP")]));

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Eng", &[("decay", 1)]));
        nodes.insert(30, node_info("Chn", &[("drive", 1)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.pages[0].params.len(), 2);
        assert_eq!(cv.pages[0].params[0].node_id, 20);
        assert_eq!(cv.pages[0].params[1].node_id, 30);
    }

    #[test]
    fn assemble_missing_engine_rule_returns_none() {
        let rules: HashMap<u32, Rule> = HashMap::new();
        let nodes: HashMap<u32, NodeInfo> = HashMap::new();
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        assert!(assemble(&rules, &chains, 0, &nodes).is_none());
    }

    #[test]
    fn chain_lists_rule_bearing_nodes_only() {
        let mut rules = HashMap::new();
        rules.insert(20, make_rule("Eng", &["AMP"], &[(1, "AMP")]));
        rules.insert(30, make_rule("Fx", &["FX"], &[(1, "FX")]));

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Eng", &[("a", 1)]));
        nodes.insert(30, node_info("Fx", &[("b", 1)]));
        nodes.insert(40, node_info("NoView", &[]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![40, 30],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(
            cv.chain,
            vec![20, 30],
            "node 40 has no rule, must be excluded"
        );
    }

    #[test]
    fn assemble_display_name_prefers_instrument_label() {
        let mut rules = HashMap::new();
        rules.insert(20, make_rule("AnalogKick", &["SRC"], &[(1, "SRC")]));

        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("My Fat Kick", &[("punch", 1)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.display_name, "My Fat Kick");
    }

    // ── MM-C5: machine variants ──────────────────────────────────────────────

    /// Two machines with different param sets, plus a chain node that has no
    /// variants at all. The chain node's contribution must re-base onto a
    /// fresh sub-page *for each machine* — including a machine whose own
    /// params stop short of the one the base fields describe.
    fn variant_fixture() -> (
        HashMap<u32, Rule>,
        Vec<TrackChain>,
        HashMap<u32, NodeInfo>,
    ) {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_machine_rule(
                "Eng",
                0,
                &[
                    (0, "Kick", &[(1, "SRC", 0), (2, "SRC", 1), (3, "SRC", 2)]),
                    // HiHat-shaped: fewer params, and `tune` (1) absent — the
                    // exact case #47 could not represent.
                    (1, "HiHat", &[(2, "SRC", 1)]),
                ],
            ),
        );
        rules.insert(30, make_rule_slotted("Chn", &["SRC"], &[(4, "SRC", 0)]));

        let mut nodes = HashMap::new();
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                &[
                    ("machine", MACHINE_PID, true, 0.0),
                    ("tune", 1, false, 0.0),
                    ("tone", 2, false, 0.0),
                    ("punch", 3, false, 0.0),
                ],
            ),
        );
        nodes.insert(30, node_info("Chn", &[("drive", 4)]));

        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];
        (rules, chains, nodes)
    }

    fn slots_of(page: &CompositePage) -> Vec<(u32, u32, u8)> {
        page.params
            .iter()
            .map(|p| (p.node_id, p.param_id, p.slot))
            .collect()
    }

    #[test]
    fn variants_are_pre_merged_against_the_rest_of_the_chain() {
        let (rules, chains, nodes) = variant_fixture();
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();

        assert_eq!(cv.variants.len(), 1, "one machine host in this chain");
        let set = &cv.variants[0];
        assert_eq!(set.node_id, 20);
        assert_eq!(set.select_param, Some(MACHINE_PID));
        assert_eq!(set.variants.len(), 2);

        let kick = &set.variants[0].pages[0];
        assert_eq!(
            slots_of(kick),
            vec![(20, 1, 0), (20, 2, 1), (20, 3, 2), (30, 4, SUB_PAGE_SLOTS)],
            "Kick's three params then the chain node on sub-page 2"
        );

        let hihat = &set.variants[1].pages[0];
        assert_eq!(
            slots_of(hihat),
            vec![(20, 2, 1), (30, 4, SUB_PAGE_SLOTS)],
            "HiHat drops params 1 and 3; the chain node still starts at 8, \
             because a contributor reserves whole sub-pages"
        );
    }

    /// A machine whose params spill onto a second sub-page pushes everything
    /// downstream of it down with them. Nothing in `variant_fixture` can show
    /// this — both its machines fit one sub-page — so the sub-page advance
    /// would still pass there if it read the base rule instead of the variant.
    #[test]
    fn the_chain_nodes_base_moves_with_the_active_machines_footprint() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_machine_rule(
                "Eng",
                0,
                &[
                    (0, "Small", &[(1, "SRC", 0)]),
                    (1, "Large", &[(1, "SRC", 0), (2, "SRC", 9)]),
                ],
            ),
        );
        rules.insert(30, make_rule_slotted("Chn", &["SRC"], &[(4, "SRC", 0)]));

        let mut nodes = HashMap::new();
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                &[
                    ("machine", MACHINE_PID, true, 0.0),
                    ("tune", 1, false, 0.0),
                    ("extra", 2, false, 0.0),
                ],
            ),
        );
        nodes.insert(30, node_info("Chn", &[("drive", 4)]));
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];

        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let set = &cv.variants[0];
        let chain_slot = |pages: &[CompositePage]| {
            pages[0]
                .params
                .iter()
                .find(|p| p.node_id == 30)
                .unwrap()
                .slot
        };
        assert_eq!(
            chain_slot(&set.variants[0].pages),
            SUB_PAGE_SLOTS,
            "Small occupies one sub-page"
        );
        assert_eq!(
            chain_slot(&set.variants[1].pages),
            SUB_PAGE_SLOTS * 2,
            "Large reaches slot 9 and occupies two, so the chain node follows"
        );
    }

    /// The pre-merged pages a client renders are the same objects the local
    /// surface renders — not a second layout computed a second way.
    #[test]
    fn active_variant_pages_equal_the_composite_pages() {
        let (rules, chains, nodes) = variant_fixture();
        for (active, idx) in [(0u32, 0usize), (1, 1)] {
            let cv = assemble_for(
                &rules,
                &chains,
                0,
                &nodes,
                &HashMap::from([(20u32, active)]),
            )
            .unwrap();
            let set = &cv.variants[0];
            assert_eq!(set.active, active);
            assert_eq!(
                cv.pages.iter().map(slots_of).collect::<Vec<_>>(),
                set.variants[idx]
                    .pages
                    .iter()
                    .map(slots_of)
                    .collect::<Vec<_>>(),
                "machine {active}: the merged view and its own pre-merged \
                 variant must agree"
            );
        }
    }

    /// With no live state, the machine drawn is the one the cap-doc default of
    /// the identity param names — which is the machine the node was built on.
    #[test]
    fn active_variant_comes_from_the_identity_params_cap_doc_default() {
        let (rules, chains, mut nodes) = variant_fixture();
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.variants[0].active, 0);
        assert_eq!(slots_of(&cv.pages[0]).len(), 4);

        // Rebuild the node's cap-doc on the second machine and nothing else.
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                &[
                    ("machine", MACHINE_PID, true, 1.0),
                    ("tune", 1, false, 0.0),
                    ("tone", 2, false, 0.0),
                    ("punch", 3, false, 0.0),
                ],
            ),
        );
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.variants[0].active, 1);
        assert_eq!(
            slots_of(&cv.pages[0]),
            vec![(20, 2, 1), (30, 4, SUB_PAGE_SLOTS)],
            "the HiHat-shaped machine, not the base fields"
        );
    }

    /// A live surface's state-bus value beats the startup cap-doc.
    #[test]
    fn explicit_selection_overrides_the_cap_doc_default() {
        let (rules, chains, nodes) = variant_fixture();
        let cv = assemble_for(&rules, &chains, 0, &nodes, &HashMap::from([(20u32, 1)]))
            .unwrap();
        assert_eq!(cv.variants[0].active, 1);
        assert_eq!(slots_of(&cv.pages[0]), vec![(20, 2, 1), (30, 4, SUB_PAGE_SLOTS)]);
    }

    /// Mirrors the engines' `from_value`, which clamps rather than panicking.
    /// Drawing a machine that is not sounding would be worse than drawing the
    /// last one, which is what the engine actually selected.
    #[test]
    fn out_of_range_selection_clamps_to_the_last_machine() {
        let (rules, chains, nodes) = variant_fixture();
        let cv = assemble_for(&rules, &chains, 0, &nodes, &HashMap::from([(20u32, 99)]))
            .unwrap();
        assert_eq!(cv.variants[0].active, 1);
        assert_eq!(
            slots_of(&cv.pages[0]),
            vec![(20, 2, 1), (30, 4, SUB_PAGE_SLOTS)],
            "the pages must be the clamped machine's too, not just the label"
        );
    }

    /// Values are dense `0..N-1` in both engines, but a third-party node need
    /// not oblige. A gap must read as "no name at this value", never as a
    /// machine — an earlier draft filled it with the index as a string, so a
    /// client drew choices that selected nothing.
    #[test]
    fn a_gap_in_the_machine_values_is_a_hole_not_an_invented_machine() {
        let rule = make_machine_rule(
            "Eng",
            0,
            &[(0, "Zeroth", &[(1, "SRC", 0)]), (3, "Third", &[(1, "SRC", 0)])],
        );
        assert_eq!(
            machine_options(&rule).unwrap(),
            vec![
                Some("Zeroth".to_string()),
                None,
                None,
                Some("Third".to_string()),
            ]
        );
    }

    /// A value so large that indexing by it would allocate absurdly drops the
    /// labels rather than the process. The names are still reachable — the
    /// variant list carries them with their values.
    #[test]
    fn an_absurd_machine_value_drops_the_labels_instead_of_allocating() {
        let rule = make_machine_rule(
            "Eng",
            0,
            &[
                (0, "Zeroth", &[(1, "SRC", 0)]),
                (MAX_DENSE_OPTIONS, "Absurd", &[(1, "SRC", 0)]),
            ],
        );
        assert!(machine_options(&rule).is_none());
        // …and the authoritative list is untouched.
        assert_eq!(rule.variants.len(), 2);
        assert_eq!(rule.variants[1].value, MAX_DENSE_OPTIONS);
    }

    /// A node may declare variants and flag no identity param. It then has a
    /// machine but no way to change it, which must degrade to "draw the first
    /// one" rather than to a panic or a control that writes nowhere.
    #[test]
    fn variants_without_an_identity_param_draw_the_first_and_offer_no_selector() {
        let mut rules = HashMap::new();
        let mut rule = make_machine_rule(
            "Eng",
            1,
            &[(0, "A", &[(1, "SRC", 0)]), (1, "B", &[(2, "SRC", 1)])],
        );
        // Strip every identity flag.
        let variants: Vec<MachineVariant> = rule
            .variants
            .iter()
            .map(|v| {
                let mut v = v.clone();
                v.overlays = Cow::Owned(
                    v.overlays
                        .iter()
                        .filter(|(pid, _)| *pid != MACHINE_PID)
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                v
            })
            .collect();
        rule.variants = Cow::Owned(variants);
        rules.insert(20, rule);

        let mut nodes = HashMap::new();
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                &[
                    ("machine", MACHINE_PID, true, 1.0),
                    ("a", 1, false, 0.0),
                    ("b", 2, false, 0.0),
                ],
            ),
        );
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let set = &cv.variants[0];
        assert_eq!(set.select_param, None);
        assert_eq!(set.select_param_name, None);
        assert_eq!(
            set.active, 0,
            "with nothing to read the selection from, the first machine shows \
             — the cap-doc default of 1.0 must not be trusted, since without \
             an identity flag nothing says that param is the selector"
        );
        assert!(
            cv.pages[0].params.iter().all(|p| p.options.is_none()),
            "no param may claim machine names when none selects the machine"
        );
    }

    /// The identity flag naming a param the cap-doc does not declare. The name
    /// is what a client would `set_param` on, so a `param_{id}` placeholder
    /// would be a selector that looks live and writes nowhere — #47's defect
    /// class. It must come back `None`.
    #[test]
    fn an_identity_param_missing_from_the_cap_doc_yields_no_name() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_machine_rule(
                "Eng",
                0,
                &[(0, "A", &[(1, "SRC", 0)]), (1, "B", &[(1, "SRC", 0)])],
            ),
        );
        let mut nodes = HashMap::new();
        // `machine` deliberately absent from the cap-doc.
        nodes.insert(20, node_info_full("Eng", &[("tune", 1, false, 0.0)]));
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let set = &cv.variants[0];
        assert_eq!(set.select_param, Some(MACHINE_PID), "the flag is still read");
        assert_eq!(
            set.select_param_name, None,
            "but there is no name to write to"
        );
        assert_eq!(set.active, 0, "and no default to resolve from");
    }

    /// No `NodeInfo` at all for a variant-bearing node: names degrade to
    /// placeholders and the first machine shows. Reachable if a cap-doc fails
    /// to collect, and it must not panic.
    #[test]
    fn a_variant_node_with_no_node_info_degrades_without_panicking() {
        let (rules, chains, mut nodes) = variant_fixture();
        nodes.remove(&20);
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(cv.variants[0].active, 0);
        assert_eq!(cv.variants[0].select_param_name, None);
        let names: Vec<&str> = cv.pages[0]
            .params
            .iter()
            .filter(|p| p.node_id == 20)
            .map(|p| p.name.as_str())
            .collect();
        assert!(
            names.iter().all(|n| n.starts_with("param_")),
            "unresolvable names degrade rather than vanish: {names:?}"
        );
    }

    /// Two machine hosts in one chain. Unreachable in the shipped graph — an
    /// engine has no audio input, so it can never be another track's chain
    /// node — but the fallback must be defined rather than accidental: each
    /// host's entries hold the *other* host at its own selection.
    #[test]
    fn a_second_machine_host_is_held_at_its_own_selection() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_machine_rule(
                "Eng",
                0,
                &[(0, "EngA", &[(1, "SRC", 0)]), (1, "EngB", &[(1, "SRC", 0)])],
            ),
        );
        rules.insert(
            30,
            make_machine_rule(
                "Fx",
                1,
                &[
                    (0, "FxA", &[(2, "SRC", 0)]),
                    // FxB reaches slot 9, so it occupies two sub-pages.
                    (1, "FxB", &[(2, "SRC", 0), (3, "SRC", 9)]),
                ],
            ),
        );
        let mut nodes = HashMap::new();
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                &[("machine", MACHINE_PID, true, 0.0), ("tune", 1, false, 0.0)],
            ),
        );
        nodes.insert(
            30,
            node_info_full(
                "Fx",
                &[
                    ("machine", MACHINE_PID, true, 1.0),
                    ("cut", 2, false, 0.0),
                    ("res", 3, false, 0.0),
                ],
            ),
        );
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![30],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();

        assert_eq!(cv.variants.len(), 2, "both hosts are reported");
        let eng = cv.variants.iter().find(|s| s.node_id == 20).unwrap();
        let fx = cv.variants.iter().find(|s| s.node_id == 30).unwrap();
        assert_eq!((eng.active, fx.active), (0, 1));

        // Every one of the engine's entries carries FxB — node 30's *own*
        // selection — never FxA. That is the documented limitation: a client
        // that switched node 30 cannot trust node 20's pre-merged pages.
        for v in &eng.variants {
            let fx_params: Vec<u32> = v.pages[0]
                .params
                .iter()
                .filter(|p| p.node_id == 30)
                .map(|p| p.param_id)
                .collect();
            assert_eq!(
                fx_params,
                vec![2, 3],
                "engine variant {} must hold node 30 on FxB, its own selection",
                v.name
            );
        }
    }

    /// Duplicate `value`s are representable and their precedence is not
    /// designed — the same shape as `PageRef::slot` before MM-C0 and overlay
    /// ids before MM-C8. Pin what the code does today so a future MM-C8
    /// assertion changes a *test*, not silently a behaviour.
    #[test]
    fn duplicate_machine_values_resolve_to_the_first_and_label_with_the_last() {
        let rule = make_machine_rule(
            "Eng",
            0,
            &[(0, "First", &[(1, "SRC", 0)]), (0, "Second", &[(2, "SRC", 1)])],
        );
        assert_eq!(
            machine_options(&rule).unwrap(),
            vec![Some("Second".to_string())],
            "the by-value table is last-writer-wins"
        );
        let info = node_info_full(
            "Eng",
            &[("machine", MACHINE_PID, true, 0.0), ("a", 1, false, 0.0)],
        );
        assert_eq!(
            resolve_variant(&rule, Some(&info), None),
            0,
            "selection is first-match — so the label and the drawn pages \
             disagree here, which is why MM-C8 should assert uniqueness"
        );
    }

    /// Every node that is not a machine host — which is all of them but two —
    /// must assemble exactly as it did before variants existed.
    #[test]
    fn a_node_without_variants_uses_its_base_fields_and_reports_no_variant_set() {
        let mut rules = HashMap::new();
        rules.insert(20, make_rule_slotted("Eng", &["SRC"], &[(1, "SRC", 3)]));
        let mut nodes = HashMap::new();
        nodes.insert(20, node_info("Eng", &[("tune", 1)]));
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert!(cv.variants.is_empty());
        assert_eq!(slots_of(&cv.pages[0]), vec![(20, 1, 3)]);
    }

    /// `options` is indexed by the machine's `value`, so a client reading
    /// `options[value]` is right even when the values are sparse or declared
    /// out of order. Positional order would silently mislabel here.
    #[test]
    fn machine_options_are_indexed_by_value_not_declaration_order() {
        let rule = make_machine_rule(
            "Eng",
            0,
            &[
                (3, "Third", &[(1, "SRC", 0)]),
                (0, "Zeroth", &[(1, "SRC", 0)]),
            ],
        );
        assert_eq!(
            machine_options(&rule).unwrap(),
            vec![
                Some("Zeroth".to_string()),
                None,
                None,
                Some("Third".to_string()),
            ],
            "declared second, `Third` still lands at index 3 — its value"
        );
    }

    /// The identity param carries its machine names to whatever draws it, and
    /// is stepped whether or not the descriptor remembered to say so.
    #[test]
    fn the_identity_param_is_stepped_and_carries_machine_names() {
        let mut rules = HashMap::new();
        let mut rule = make_machine_rule(
            "Eng",
            0,
            &[
                (0, "Kick", &[(1, "SRC", 0)]),
                (1, "HiHat", &[(1, "SRC", 0)]),
            ],
        );
        // Page the identity param, the way MM-C6 pages it on TRIG.
        let mut variants = rule.variants.to_vec();
        for v in variants.iter_mut() {
            let mut pages = v.pages.to_vec();
            pages.push((
                MACHINE_PID,
                PageRef {
                    page: Cow::Borrowed("TRIG"),
                    slot: 0,
                },
            ));
            v.pages = Cow::Owned(pages);
            v.page_groups = Cow::Owned(vec![Cow::Borrowed("TRIG"), Cow::Borrowed("SRC")]);
        }
        rule.variants = Cow::Owned(variants);
        rule.page_groups = Cow::Owned(vec![Cow::Borrowed("TRIG"), Cow::Borrowed("SRC")]);
        rules.insert(20, rule);

        let mut nodes = HashMap::new();
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                // `stepped: false` on purpose — the flag must not be the only
                // thing keeping a machine selector off a continuous arc.
                &[("machine", MACHINE_PID, false, 0.0), ("tune", 1, false, 0.0)],
            ),
        );
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();

        let trig = cv.pages.iter().find(|p| p.id == "TRIG").unwrap();
        let m = &trig.params[0];
        assert_eq!(m.param_id, MACHINE_PID);
        assert!(m.stepped, "a machine selector is never a continuous arc");
        assert_eq!(
            m.options.as_deref().unwrap(),
            [Some("Kick".to_string()), Some("HiHat".to_string())]
        );

        let src = cv.pages.iter().find(|p| p.id == "SRC").unwrap();
        assert!(
            src.params[0].options.is_none(),
            "only the identity param carries machine names"
        );
        assert!(!src.params[0].stepped);
    }

    /// `stepped` is the cap-doc's, for every param that is not the selector.
    #[test]
    fn stepped_is_carried_through_from_the_cap_doc() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_rule_slotted("Eng", &["SRC"], &[(1, "SRC", 0), (2, "SRC", 1)]),
        );
        let mut nodes = HashMap::new();
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                &[("mode", 1, true, 0.0), ("tune", 2, false, 0.0)],
            ),
        );
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert!(cv.pages[0].params[0].stepped);
        assert!(!cv.pages[0].params[1].stepped);
        assert!(
            cv.pages[0].params[0].options.is_none(),
            "a stepped param with no variant set has no names to offer yet"
        );
    }

    /// Overlays travel with the machine they belong to — a surface clamping
    /// input needs this machine's range, never the union the bank stores.
    #[test]
    fn each_variant_carries_its_own_overlays() {
        let (rules, chains, nodes) = variant_fixture();
        let cv = assemble(&rules, &chains, 0, &nodes).unwrap();
        let set = &cv.variants[0];

        for (i, v) in set.variants.iter().enumerate() {
            let o = v.overlays.iter().find(|o| o.param_id == 2).unwrap();
            assert_eq!(
                o.max,
                i as f64 + 1.0,
                "machine {i} must carry its own range for param 2"
            );
            assert_eq!(o.param_name, "tone", "overlays resolve their own name");
            let ident = v
                .overlays
                .iter()
                .find(|o| o.param_id == MACHINE_PID)
                .unwrap();
            assert!(ident.identity, "every machine flags the selector");
        }
        assert_eq!(set.select_param_name.as_deref(), Some("machine"));
    }

    /// A machine that contributes to a page group its siblings do not must
    /// bring that page with it — `page_groups` is per variant, not per node.
    #[test]
    fn page_groups_follow_the_active_machine() {
        let mut rules = HashMap::new();
        rules.insert(
            20,
            make_machine_rule(
                "Eng",
                0,
                &[
                    (0, "Plain", &[(1, "SRC", 0)]),
                    (1, "Filtered", &[(1, "SRC", 0), (2, "FLTR", 0)]),
                ],
            ),
        );
        let mut nodes = HashMap::new();
        nodes.insert(
            20,
            node_info_full(
                "Eng",
                &[
                    ("machine", MACHINE_PID, true, 0.0),
                    ("tune", 1, false, 0.0),
                    ("cutoff", 2, false, 0.0),
                ],
            ),
        );
        let chains = vec![TrackChain {
            engine_node_id: 20,
            chain_ids: vec![],
        }];

        let plain = assemble(&rules, &chains, 0, &nodes).unwrap();
        assert_eq!(
            plain.pages.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["SRC"]
        );
        let filtered =
            assemble_for(&rules, &chains, 0, &nodes, &HashMap::from([(20u32, 1)])).unwrap();
        assert_eq!(
            filtered
                .pages
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec!["SRC", "FLTR"]
        );
    }
}
