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

use std::collections::HashMap;

use paraclete_node_api::{AffordanceHint, Rule};

pub const CANONICAL_PAGE_ORDER: [&str; 6] = ["TRIG", "SRC", "FLTR", "AMP", "FX", "MOD"];

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub display_name: Option<String>,
    pub params: Vec<(u32, String)>, // (param_id, param_name)
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
    pub pages: Vec<CompositePage>,
    /// Rule-bearing chain nodes, engine first.
    pub chain: Vec<u32>,
    pub routes: Vec<CompositeRoute>,
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
pub fn assemble(
    rules: &HashMap<u32, Rule>,
    chains: &[TrackChain],
    track_id: u32,
    nodes: &HashMap<u32, NodeInfo>,
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
                param_name: nodes
                    .get(nid)
                    .and_then(|info| {
                        info.params
                            .iter()
                            .find(|(id, _)| *id == *pid)
                            .map(|(_, n)| n.clone())
                    })
                    .unwrap_or_else(|| format!("param_{}", pid)),
            })
        })
        .collect();

    // Collect pages by group name
    let mut pages_by_group: HashMap<String, Vec<(u32, &Rule)>> = HashMap::new();
    for &(nid, r) in &chain_rules {
        for pg in r.page_groups.iter() {
            pages_by_group
                .entry(pg.to_string())
                .or_default()
                .push((nid, r));
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
    let mut custom_keys: Vec<String> = pages_by_group.keys().cloned().collect();
    custom_keys.sort();
    for key in custom_keys {
        if let Some(contributors) = pages_by_group.remove(&key) {
            if let Some(page) = merge_page(&key, &contributors, nodes) {
                pages.push(page);
            }
        }
    }

    // chain lists rule-bearing nodes only, engine first
    let chain_nodes: Vec<u32> = chain_rules.iter().map(|(nid, _)| *nid).collect();

    Some(CompositeView {
        engine_node_id: chain.engine_node_id,
        engine_name: engine_rule.name.to_string(),
        display_name: engine_display_name,
        pages,
        chain: chain_nodes,
        routes,
    })
}

fn merge_page(
    group_name: &str,
    contributors: &[(u32, &Rule)],
    nodes: &HashMap<u32, NodeInfo>,
) -> Option<CompositePage> {
    let mut params: Vec<CompositeParam> = Vec::new();
    let mut envelopes: Vec<CompositeEnvelope> = Vec::new();
    let mut macros: Vec<CompositeMacro> = Vec::new();
    let mut envelopes_offset: u32 = 0;
    let mut slot: u8 = 0;

    for &(nid, rule) in contributors {
        for (param_id, page_ref) in rule.param_pages.iter() {
            if page_ref.page.as_ref() != group_name {
                continue;
            }
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

            let pname = nodes
                .get(&nid)
                .and_then(|info| {
                    info.params
                        .iter()
                        .find(|(id, _)| *id == *param_id)
                        .map(|(_, n)| n.clone())
                })
                .unwrap_or_else(|| format!("param_{}", param_id));

            let routing = rule
                .routing
                .iter()
                .find(|(pid, _)| *pid == *param_id)
                .map(|(_, sem)| sem.destination.to_string());

            params.push(CompositeParam {
                node_id: nid,
                param_id: *param_id,
                name: pname.clone(),
                label: pname,
                affordance,
                env_group,
                slot,
                routing,
            });
            slot = slot.saturating_add(1);
        }

        for env in rule.envelopes.iter() {
            let pids: Vec<(u32, String)> = env
                .param_ids
                .iter()
                .filter(|&&id| id != 0)
                .map(|&id| {
                    let name = nodes
                        .get(&nid)
                        .and_then(|info| {
                            info.params
                                .iter()
                                .find(|(pid, _)| *pid == id)
                                .map(|(_, n)| n.clone())
                        })
                        .unwrap_or_else(|| format!("param_{}", id));
                    (id, name)
                })
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
                .map(|tid| {
                    let name = nodes
                        .get(&nid)
                        .and_then(|info| {
                            info.params
                                .iter()
                                .find(|(id, _)| *id == *tid)
                                .map(|(_, n)| n.clone())
                        })
                        .unwrap_or_else(|| format!("param_{}", tid));
                    (*tid, name)
                })
                .collect();
            macros.push(CompositeMacro {
                name: m.name.to_string(),
                targets,
                page: m.page.as_ref().map(|p| p.to_string()),
            });
        }
    }

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
    use paraclete_node_api::{EnvelopeGroup, PageRef};
    use std::borrow::Cow;

    fn node_info(display_name: &str, params: &[(&str, u32)]) -> NodeInfo {
        NodeInfo {
            display_name: Some(display_name.to_string()),
            params: params
                .iter()
                .map(|(n, pid)| (*pid, n.to_string()))
                .collect(),
        }
    }

    fn make_rule(name: &str, pages: &[&str], param_pages: &[(u32, &str)]) -> Rule {
        Rule {
            name: Cow::Owned(name.to_string()),
            page_groups: Cow::Owned(pages.iter().map(|&s| Cow::Owned(s.to_string())).collect()),
            param_pages: Cow::Owned(
                param_pages
                    .iter()
                    .map(|&(pid, pg)| {
                        (
                            pid,
                            PageRef {
                                page: Cow::Owned(pg.to_string()),
                                slot: 0,
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
        }
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
}
