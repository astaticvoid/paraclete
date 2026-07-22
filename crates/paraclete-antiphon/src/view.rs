//! Thin `CompositeView → ServerMsg::ViewMeta` mapper (TK1 C2).
//!
//! Assembly logic was extracted to `paraclete-view-assembly`; this module
//! converts the already-assembled `CompositeView` into the wire format that
//! Theoria clients expect.  Wire format is unchanged.

use std::collections::HashMap;

use paraclete_node_api::{AffordanceHint, Rule};
use paraclete_view_assembly::{CompositeView, NodeInfo, TrackChain};

use crate::protocol::{
    ServerMsg, ViewMetaChain, ViewMetaChainRoute, ViewMetaEnvelope, ViewMetaMacro, ViewMetaPage,
    ViewMetaParam, ViewMetaRouting,
};

pub struct ViewRegistry {
    pub rules: HashMap<u32, Rule>,
    pub chains: Vec<TrackChain>,
    pub node_infos: HashMap<u32, NodeInfo>,
}

impl ViewRegistry {
    pub fn assemble(&self, track_id: u32, nonce: Option<String>) -> Option<ServerMsg> {
        let cv = paraclete_view_assembly::assemble(
            &self.rules,
            &self.chains,
            track_id,
            &self.node_infos,
        )?;
        Some(composite_to_view_meta(cv, track_id, nonce))
    }
}

fn composite_to_view_meta(cv: CompositeView, track_id: u32, nonce: Option<String>) -> ServerMsg {
    let chain_nodes = cv.chain.clone();
    ServerMsg::ViewMeta {
        track_id,
        nonce,
        engine_node_id: cv.engine_node_id,
        engine_name: cv.engine_name,
        display_name: cv.display_name,
        pages: cv.pages.into_iter().map(composite_page).collect(),
        chain: ViewMetaChain {
            nodes: cv.chain,
            node_labels: chain_nodes
                .iter()
                .map(|&nid| (nid, format!("Node {}", nid)))
                .collect(),
            routing: cv
                .routes
                .into_iter()
                .map(|r| ViewMetaChainRoute {
                    source: r.source,
                    dest: r.dest,
                    param_id: r.param_name,
                    value: 0.0, // placeholder; live values are a W-track concern
                })
                .collect(),
        },
    }
}

fn composite_page(page: paraclete_view_assembly::CompositePage) -> ViewMetaPage {
    ViewMetaPage {
        id: page.id,
        label: page.label,
        params: page
            .params
            .into_iter()
            .map(|p| ViewMetaParam {
                id: p.name.clone(),
                node_id: p.node_id,
                label: p.label,
                affordance: affordance_to_json(p.affordance),
                env_group: p.env_group,
                slot: p.slot,
                stepped: None,
                options: None,
                routing: p.routing.map(|dest| ViewMetaRouting { dest }),
            })
            .collect(),
        envelopes: page
            .envelopes
            .into_iter()
            .map(|e| ViewMetaEnvelope {
                id: e.id,
                env_type: e.env_type,
                label: e.label,
                param_ids: e.params.into_iter().map(|(_, name)| name).collect(),
            })
            .collect(),
        macros: page
            .macros
            .into_iter()
            .map(|m| ViewMetaMacro {
                name: m.name,
                targets: m.targets.into_iter().map(|(_, name)| name).collect(),
                page: m.page,
            })
            .collect(),
    }
}

fn affordance_to_json(hint: AffordanceHint) -> String {
    match hint {
        AffordanceHint::None => "None".into(),
        AffordanceHint::EnvelopeCurve { .. } => "EnvelopeCurve".into(),
        AffordanceHint::FilterShape => "FilterShape".into(),
        AffordanceHint::LfoShape => "LfoShape".into(),
        AffordanceHint::Waveform => "Waveform".into(),
        AffordanceHint::DiagramHighlight { .. } => "DiagramHighlight".into(),
    }
}
