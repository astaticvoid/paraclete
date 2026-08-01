// SPDX-License-Identifier: GPL-3.0-or-later
//! The seam between view assembly and what a node actually publishes.
//!
//! `paraclete-app` is the only crate that can see both an engine (L3
//! `paraclete-nodes`) and a `ViewRegistry` (`paraclete-antiphon`), so this is
//! the only place the two halves of #157 can be made to meet a real node.

use std::collections::HashMap;

use paraclete_antiphon::view::ViewRegistry;
use paraclete_node_api::{Node, StateBusValue};
use paraclete_nodes::{AnalogEngine, FmEngine};
use paraclete_view_assembly::{NodeInfo, ParamInfo, TrackChain};

/// Build a one-track registry around `node`, the way `build_view_registry`
/// does: rules from the cap-doc's `view`, `NodeInfo` from its params.
fn registry_for(node_id: u32, node: &dyn Node) -> ViewRegistry {
    let doc = node.capability_document();
    let rule = doc
        .view
        .clone()
        .expect("a machine host must declare a view Rule");
    ViewRegistry {
        rules: HashMap::from([(node_id, rule)]),
        chains: vec![TrackChain {
            engine_node_id: node_id,
            chain_ids: vec![],
        }],
        node_infos: HashMap::from([(
            node_id,
            NodeInfo {
                display_name: None,
                params: doc
                    .params
                    .iter()
                    .map(|p| ParamInfo {
                        id: p.id,
                        name: p.name.to_string(),
                        stepped: p.stepped,
                        options: None,
                        default: p.default,
                    })
                    .collect(),
            },
        )]),
        selections: Default::default(),
    }
}

fn published_paths(node: &dyn Node) -> Vec<String> {
    let mut buf: Vec<(String, StateBusValue)> = Vec::new();
    node.published_state(&mut buf);
    buf.into_iter().map(|(path, _)| path).collect()
}

/// #157's load-bearing guard. `machine_select_paths()` builds
/// `/node/{id}/param/{name}` from the assembler's idea of the identity param;
/// `publish_bank_state` builds the same shape from the bank slot's name. Both
/// read `doc.params[].name` today, so they agree — but they are two `format!`
/// calls in two crates, and if either drifts, `pump` watches a path nothing
/// ever writes. Every antiphon-side test would stay green and the feature
/// would be silently dead.
///
/// So: assert the paths the registry asks to watch are paths the node really
/// emits, against the shipped engines rather than a fixture.
#[test]
fn every_watched_machine_path_is_one_the_engine_actually_publishes() {
    let mut engines: Vec<(u32, Box<dyn Node>)> = vec![
        (20, Box::new(AnalogEngine::kick())),
        (21, Box::new(AnalogEngine::snare())),
        (22, Box::new(AnalogEngine::hihat())),
        (27, Box::new(FmEngine::bass())),
    ];
    // `publish_bank_state` caches its path strings in a `OnceLock` on first
    // call (BUG-007), so the id has to be set before anything publishes — the
    // app assigns it at `add_node_tagged` time, well before the first cycle.
    for (node_id, node) in engines.iter_mut() {
        node.set_node_id(*node_id);
        node.activate(44_100.0, 512);
    }

    for (node_id, node) in &engines {
        let watched = registry_for(*node_id, node.as_ref()).machine_select_paths();
        assert_eq!(
            watched.len(),
            1,
            "node {node_id} hosts machines, so exactly one selector is watched; got {watched:?}"
        );
        let published = published_paths(node.as_ref());
        for path in watched.keys() {
            assert!(
                published.contains(path),
                "node {node_id}: the registry watches `{path}`, which the node \
                 never publishes — a machine switch would never reach \
                 `view_meta`. Published: {published:?}"
            );
        }
    }
}

/// The other direction of the same seam: the watched path has to be the
/// *identity* param's, not merely some param that exists. Pins the name so a
/// rename of the canonical `machine` param cannot pass silently on both sides.
#[test]
fn the_watched_path_is_the_identity_params() {
    let engine = AnalogEngine::kick();
    let watched = registry_for(20, &engine).machine_select_paths();
    assert_eq!(
        watched,
        HashMap::from([("/node/20/param/machine".to_string(), 20u32)])
    );
}
