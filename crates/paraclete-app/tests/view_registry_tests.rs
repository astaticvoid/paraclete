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

/// #176 (BUG-067): the labels a stepped selector needs must reach an assembled
/// view when the node is a **real** engine, not a fixture.
///
/// The issue reported two symptoms — `lfo_dest 1.00` and `machine 2.00` — and
/// diagnosed both as "Theotokos never reads `ParamDescriptor.display`". That is
/// right about `lfo_dest` and wrong about `machine`: the identity param's names
/// come from the Rule's `variants` (`machine_options`), not from a display
/// adapter, so `machine` needs no adapter and must not grow one. `variants` is
/// the authoritative list, and a second derivation of the same names is the
/// drift the assembler's own docs warn about.
///
/// Both routes are exercised here, on both engine families, because they are
/// different code paths that happen to produce the same shape.
#[test]
fn a_real_engines_stepped_selectors_carry_their_names() {
    for (label, node) in [
        ("analog", Box::new(AnalogEngine::hihat()) as Box<dyn Node>),
        ("fm", Box::new(FmEngine::bell()) as Box<dyn Node>),
    ] {
        let doc = node.capability_document();
        let rule = doc.view.clone().expect("engines declare a view Rule");
        let nodes = HashMap::from([(
            20u32,
            NodeInfo {
                display_name: None,
                params: doc
                    .params
                    .iter()
                    .map(|p| ParamInfo {
                        id: p.id,
                        name: p.name.to_string(),
                        stepped: p.stepped,
                        // Exactly what `build_view_registry` does.
                        options: p.value_labels(),
                        default: p.default,
                    })
                    .collect(),
            },
        )]);
        let cv = paraclete_view_assembly::assemble(
            &HashMap::from([(20u32, rule)]),
            &[TrackChain {
                engine_node_id: 20,
                chain_ids: vec![],
            }],
            0,
            &nodes,
        )
        .expect("a one-engine chain assembles");

        let find = |name: &str| {
            cv.pages
                .iter()
                .flat_map(|p| p.params.iter())
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{label}: {name} must be paged"))
        };

        // Route 1 — the descriptor's `ParamDisplay`, via `value_labels`.
        let dest = find("lfo_dest");
        let dest_names: Vec<&str> = dest
            .options
            .as_deref()
            .unwrap_or_else(|| panic!("{label}: lfo_dest must carry dest names"))
            .iter()
            .filter_map(|o| o.as_deref())
            .collect();
        // #179: the dest list is per-machine now, so this asserts against the
        // machine actually under test rather than a fixed name. A HiHat has
        // no `tune` destination at all — which is the point of #179, and the
        // reason this assertion could not stay as `contains("tune")`.
        assert!(
            !dest_names.is_empty(),
            "{label}: every machine offers at least one destination"
        );
        assert!(
            dest_names.iter().all(|n| !n.is_empty()),
            "{label}: a named entry must not be blank; got {dest_names:?}"
        );
        // m2: pin a NAME the machine genuinely offers, so a regression back
        // to machine-invariant (or otherwise wrong) labels is caught at the
        // app layer too, not only engine-side. Each machine under test has a
        // different canonical name: HiHat offers `tone` but not `tune`; Bell
        // offers `ratio` but not `attack` (that is Bass's).
        let (has, lacks) = match label {
            "analog" => ("tone", "tune"),
            "fm" => ("ratio", "attack"),
            _ => unreachable!("only the two engine families are under test"),
        };
        assert!(
            dest_names.contains(&has),
            "{label}: `{has}` is a destination this machine reads and must be \
             offered; got {dest_names:?}"
        );
        assert!(
            !dest_names.contains(&lacks),
            "{label}: `{lacks}` belongs to a different machine's table and \
             must not be offered here; got {dest_names:?}"
        );

        // Route 2 — the Rule's `variants`, via `machine_options`. No display
        // adapter is involved, and none should be added.
        let machine = find("machine");
        assert!(machine.stepped, "{label}: a machine selector is stepped");
        let machine_names: Vec<&str> = machine
            .options
            .as_deref()
            .unwrap_or_else(|| panic!("{label}: machine must carry variant names"))
            .iter()
            .filter_map(|o| o.as_deref())
            .collect();
        assert_eq!(
            machine_names.len(),
            3,
            "{label}: three machines per family; got {machine_names:?}"
        );
        assert!(
            machine_names.iter().all(|n| !n.is_empty()),
            "{label}: every machine names itself; got {machine_names:?}"
        );
        assert!(
            doc.params
                .iter()
                .find(|p| p.name.to_string() == "machine")
                .expect("machine is declared")
                .display
                .is_none(),
            "{label}: `machine` must NOT declare a display adapter — its names \
             come from `variants`, and two sources of the same names drift"
        );
    }
}

/// ADR-041 amendment 2026-08-02 (M1): the app layer must show the
/// SELECTED machine's dest labels, not the node's construction-time ones.
/// The construction-time labels are exactly what a doc built on a machine
/// freezes into `ParamInfo.options`, so this builds the doc on one
/// machine (HiHat/Bell) and assembles for a DIFFERENT one (Kick/Bass):
/// the assembled `lfo_dest` options must name the selected machine's
/// destinations, and must NOT leak the frozen node-level ones. A
/// regression back to frozen-at-construction labels fails this (the
/// same-machine assertions above cannot see it — the frozen and the
/// active variant's labels are identical then).
#[test]
fn assembled_dest_labels_follow_the_selected_machine_not_the_built_machine() {
        for (label, node, selected, want, dont_want) in [
            // HiHat offers tone/decay/open; Kick offers tune/punch/decay/drive/tone.
            ("analog", Box::new(AnalogEngine::hihat()) as Box<dyn Node>, 0u32, "tune", "open"),
            // Bell offers tune/ratio/index/decay/feedback; Bass offers
            // tune/ratio/index/attack/decay/drive — `feedback` is Bell-only,
            // `attack` Bass-only.
            ("fm", Box::new(FmEngine::bell()) as Box<dyn Node>, 2u32, "attack", "feedback"),
        ] {
            let doc = node.capability_document();
            let rule = doc.view.clone().expect("engines declare a view Rule");
            let nodes = HashMap::from([(
                20u32,
                NodeInfo {
                    display_name: None,
                    params: doc
                        .params
                        .iter()
                        .map(|p| ParamInfo {
                            id: p.id,
                            name: p.name.to_string(),
                            stepped: p.stepped,
                            options: p.value_labels(),
                            default: p.default,
                        })
                        .collect(),
                },
            )]);
            let cv = paraclete_view_assembly::assemble_for(
                &HashMap::from([(20u32, rule)]),
                &[TrackChain {
                    engine_node_id: 20,
                    chain_ids: vec![],
                }],
                0,
                &nodes,
                &HashMap::from([(20u32, selected)]),
            )
            .expect("a one-engine chain assembles");

            let dest_names: Vec<&str> = cv
                .pages
                .iter()
                .flat_map(|p| p.params.iter())
                .find(|p| p.name == "lfo_dest")
                .and_then(|p| p.options.as_deref())
                .expect("lfo_dest must carry labels")
                .iter()
                .filter_map(|o| o.as_deref())
                .collect();
            assert!(
                dest_names.contains(&want),
                "{label}: selected machine's dest `{want}` must be offered; got {dest_names:?}"
            );
            assert!(
                !dest_names.contains(&dont_want),
                "{label}: the built machine's dest `{dont_want}` must NOT leak from the \
                 frozen node-level labels; got {dest_names:?}"
            );
        }
}
