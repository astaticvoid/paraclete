//! Composite `view_meta` assembly for W2.

use std::collections::HashMap;

use paraclete_node_api::{AffordanceHint, Rule};

use crate::protocol::{
    NodeSummary, ServerMsg, ViewMetaChain, ViewMetaChainRoute, ViewMetaEnvelope, ViewMetaMacro,
    ViewMetaPage, ViewMetaParam, ViewMetaRouting,
};

pub struct TrackChain {
    pub engine_node_id: u32,
    pub chain_ids: Vec<u32>,
}

pub struct ViewRegistry {
    pub rules: HashMap<u32, Rule>,
    pub chains: Vec<TrackChain>,
}

impl ViewRegistry {
    pub fn assemble(
        &self,
        track_id: u32,
        nonce: Option<String>,
        nodes: &[NodeSummary],
    ) -> Option<ServerMsg> {
        let chain = self.chains.get(track_id as usize)?;
        let engine_rule = self.rules.get(&chain.engine_node_id)?;

        let param_names = build_param_name_map(nodes);

        let mut chain_rules: Vec<(u32, &Rule)> = vec![(chain.engine_node_id, engine_rule)];
        for &nid in &chain.chain_ids {
            if let Some(r) = self.rules.get(&nid) {
                chain_rules.push((nid, r));
            }
        }

        let node_labels: Vec<(u32, String)> = chain_rules
            .iter()
            .map(|(nid, r)| (*nid, r.name.to_string()))
            .collect();

        let routing: Vec<ViewMetaChainRoute> = chain_rules
            .iter()
            .flat_map(|(nid, r)| {
                let names = &param_names;
                r.routing.iter().map(move |(pid, sem)| ViewMetaChainRoute {
                    source: *nid,
                    dest: sem.destination.to_string(),
                    param_id: names
                        .get(nid)
                        .and_then(|m| m.get(pid))
                        .cloned()
                        .unwrap_or_else(|| format!("param_{}", pid)),
                    value: 0.0,
                })
            })
            .collect();

        let chain_meta = ViewMetaChain {
            nodes: chain_rules.iter().map(|(nid, _)| *nid).collect(),
            node_labels,
            routing,
        };

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

        let page_order = ["SRC", "AMP", "FLTR", "FX", "TRIG", "MOD"];
        let mut pages: Vec<ViewMetaPage> = Vec::new();
        for &pg_name in &page_order {
            if let Some(contributors) = pages_by_group.remove(pg_name) {
                if let Some(page) =
                    Self::merge_page(pg_name, &contributors, &param_names)
                {
                    pages.push(page);
                }
            }
        }
        // Custom pages (beyond the standard set) in alphabetical order
        let mut custom_keys: Vec<String> = pages_by_group.keys().cloned().collect();
        custom_keys.sort();
        for key in custom_keys {
            if let Some(contributors) = pages_by_group.remove(&key) {
                if let Some(page) = Self::merge_page(&key, &contributors, &param_names) {
                    pages.push(page);
                }
            }
        }

        let engine_display_name = nodes
            .iter()
            .find(|n| n.id == chain.engine_node_id)
            .map(|n| n.name.as_str())
            .unwrap_or(engine_rule.name.as_ref());

        Some(ServerMsg::ViewMeta {
            track_id,
            nonce,
            engine_node_id: chain.engine_node_id,
            engine_name: engine_rule.name.to_string(),
            display_name: engine_display_name.to_string(),
            pages,
            chain: chain_meta,
        })
    }

    fn merge_page(
        group_name: &str,
        contributors: &[(u32, &Rule)],
        param_names: &HashMap<u32, HashMap<u32, String>>,
    ) -> Option<ViewMetaPage> {
        let mut params: Vec<ViewMetaParam> = Vec::new();
        let mut envelopes: Vec<ViewMetaEnvelope> = Vec::new();
        let mut macros: Vec<ViewMetaMacro> = Vec::new();
        let mut envelopes_offset: u32 = 0;
        let mut slot: u8 = 0;

        for &(nid, rule) in contributors {
            for (param_id, page_ref) in rule.param_pages.iter() {
                if page_ref.page.as_ref() != group_name {
                    continue;
                }
                let affordance_str = affordance_to_json(rule, *param_id);
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

                let pname = param_name(param_names, nid, *param_id);

                let routing =
                    rule.routing
                        .iter()
                        .find(|(pid, _)| *pid == *param_id)
                        .map(|(_, sem)| ViewMetaRouting {
                            dest: sem.destination.to_string(),
                        });

                params.push(ViewMetaParam {
                    id: pname.clone(),
                    node_id: nid,
                    label: pname,
                    affordance: affordance_str,
                    env_group,
                    slot,
                    stepped: None,
                    options: None,
                    routing,
                });
                slot = slot.saturating_add(1);
            }

            for env in rule.envelopes.iter() {
                let pids: Vec<String> = env
                    .param_ids
                    .iter()
                    .filter(|&&id| id != 0)
                    .map(|&id| param_name(param_names, nid, id))
                    .collect();
                envelopes.push(ViewMetaEnvelope {
                    id: envelopes_offset,
                    env_type: env.env_type.to_string(),
                    label: env.label.to_string(),
                    param_ids: pids,
                });
                envelopes_offset = envelopes_offset.saturating_add(1);
            }

            for m in rule.macros.iter() {
                macros.push(ViewMetaMacro {
                    name: m.name.to_string(),
                    targets: m
                        .targets
                        .iter()
                        .map(|tid| param_name(param_names, nid, *tid))
                        .collect(),
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

        Some(ViewMetaPage {
            id: group_name.to_string(),
            label: label.to_string(),
            params,
            envelopes,
            macros,
        })
    }
}

fn build_param_name_map(nodes: &[NodeSummary]) -> HashMap<u32, HashMap<u32, String>> {
    let mut map: HashMap<u32, HashMap<u32, String>> = HashMap::new();
    for node in nodes {
        let inner: HashMap<u32, String> =
            node.params.iter().map(|p| (p.id, p.name.clone())).collect();
        map.insert(node.id, inner);
    }
    map
}

fn param_name(
    map: &HashMap<u32, HashMap<u32, String>>,
    node_id: u32,
    param_id: u32,
) -> String {
    map.get(&node_id)
        .and_then(|m| m.get(&param_id))
        .cloned()
        .unwrap_or_else(|| format!("param_{}", param_id))
}

fn affordance_to_json(rule: &Rule, param_id: u32) -> String {
    match rule.affordances.iter().find(|(pid, _)| *pid == param_id) {
        Some((_, AffordanceHint::None)) => "None".into(),
        Some((_, AffordanceHint::EnvelopeCurve { .. })) => "EnvelopeCurve".into(),
        Some((_, AffordanceHint::FilterShape)) => "FilterShape".into(),
        Some((_, AffordanceHint::LfoShape)) => "LfoShape".into(),
        Some((_, AffordanceHint::Waveform)) => "Waveform".into(),
        Some((_, AffordanceHint::DiagramHighlight { .. })) => "DiagramHighlight".into(),
        None => "None".into(),
    }
}
