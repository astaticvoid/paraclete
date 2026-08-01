//! Thin `CompositeView → ServerMsg::ViewMeta` mapper (TK1 C2).
//!
//! Assembly logic was extracted to `paraclete-view-assembly`; this module
//! converts the already-assembled `CompositeView` into the wire format that
//! Theoria clients expect.  Wire format is unchanged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use paraclete_node_api::{AffordanceHint, Rule};
use paraclete_view_assembly::{CompositeView, NodeInfo, TrackChain};

use crate::protocol::{
    ServerMsg, ViewMetaChain, ViewMetaChainRoute, ViewMetaEnvelope, ViewMetaMacro, ViewMetaOverlay,
    ViewMetaPage, ViewMetaParam, ViewMetaRouting, ViewMetaVariant, ViewMetaVariantSet,
};

/// Which machine each machine-host node is *currently* on, by node id.
///
/// Assembly with no selection falls back to each host's cap-doc default — the
/// machine the node was **built** with — and cap-docs are collected once, at
/// startup, before the executor owns the nodes (ADR-041 decision 1: there is
/// no re-query and none is being added). So without this, a client that
/// switched a track to `AnalogSnare` kept being told `active: 0` and drawn
/// `AnalogKick`'s pages for the rest of the process's life (BUG-056, #157).
///
/// Written on the main thread from the state bus during `AntiphonHandle::pump`
/// and read by WS client threads assembling `view_meta`, so the map is behind
/// a `Mutex` — both ends are off the audio thread. Cloning shares the map:
/// the copy the server holds and the copy `pump` writes are the same one.
#[derive(Clone, Default)]
pub struct MachineSelections(Arc<Mutex<HashMap<u32, u32>>>);

impl MachineSelections {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `node_id`'s current machine. A poisoned lock is dropped rather
    /// than propagated: a stale view beats taking down a client thread.
    pub fn set(&self, node_id: u32, value: u32) {
        if let Ok(mut m) = self.0.lock() {
            m.insert(node_id, value);
        }
    }

    pub fn snapshot(&self) -> HashMap<u32, u32> {
        self.0.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct ViewRegistry {
    pub rules: HashMap<u32, Rule>,
    pub chains: Vec<TrackChain>,
    pub node_infos: HashMap<u32, NodeInfo>,
    /// Live machine selection, shared with `AntiphonHandle`. Empty by
    /// default, which reproduces the pre-#157 cap-doc-default behaviour.
    pub selections: MachineSelections,
}

impl ViewRegistry {
    pub fn assemble(&self, track_id: u32, nonce: Option<String>) -> Option<ServerMsg> {
        let cv = paraclete_view_assembly::assemble_for(
            &self.rules,
            &self.chains,
            track_id,
            &self.node_infos,
            &self.selections.snapshot(),
        )?;
        Some(composite_to_view_meta(cv, track_id, nonce))
    }

    /// `/node/{id}/param/{name}` → node id, for every machine host in every
    /// track chain. This is the set of state-bus paths whose value *is* a
    /// machine selection; `pump` watches exactly these.
    ///
    /// Built by assembling each track once at startup, because the identity
    /// param is decided inside assembly (`CompositeVariantSet::select_param`)
    /// and re-deriving it here would be a second implementation of the same
    /// rule — the drift `PageNav.tsx` already demonstrated.
    pub fn machine_select_paths(&self) -> HashMap<String, u32> {
        let mut out = HashMap::new();
        for track in 0..self.chains.len() as u32 {
            let Some(cv) = paraclete_view_assembly::assemble(
                &self.rules,
                &self.chains,
                track,
                &self.node_infos,
            ) else {
                continue;
            };
            for set in cv.variants {
                if let Some(name) = set.select_param_name {
                    out.insert(format!("/node/{}/param/{}", set.node_id, name), set.node_id);
                }
            }
        }
        out
    }
}

fn composite_to_view_meta(cv: CompositeView, track_id: u32, nonce: Option<String>) -> ServerMsg {
    let chain_nodes = cv.chain.clone();
    let variants = cv
        .variants
        .into_iter()
        .map(|set| ViewMetaVariantSet {
            node_id: set.node_id,
            select_param: set.select_param_name,
            active: set.active,
            variants: set
                .variants
                .into_iter()
                .map(|v| ViewMetaVariant {
                    value: v.value,
                    name: v.name,
                    pages: v.pages.into_iter().map(composite_page).collect(),
                    overlays: v
                        .overlays
                        .into_iter()
                        .map(|o| ViewMetaOverlay {
                            param: o.param_name,
                            min: o.min,
                            max: o.max,
                            default: o.default,
                            identity: o.identity,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    ServerMsg::ViewMeta {
        track_id,
        nonce,
        engine_node_id: cv.engine_node_id,
        engine_name: cv.engine_name,
        display_name: cv.display_name,
        pages: cv.pages.into_iter().map(composite_page).collect(),
        variants,
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
                // Both were hardwired `None` until MM-C5 (ADR-041 amendment
                // 3): a wire client could not tell a stepped param from a
                // continuous one, and *nothing* anywhere — Theotokos included
                // — had names for a stepped param's values. Theotokos did
                // already read `stepped` itself, straight off the cap-doc
                // (`theotokos/src/model.rs:461`), so the gap was the wire's
                // half of it plus the names.
                //
                // Only send `stepped` when true: an absent field and a false
                // one mean the same to a client, and all but a handful of
                // params are continuous.
                stepped: p.stepped.then_some(true),
                options: p.options,
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

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{MachineVariant, PageRef, ParamOverlay};
    use paraclete_view_assembly::ParamInfo;
    use std::borrow::Cow;

    const MACHINE: u32 = 900;
    const TUNE: u32 = 901;

    /// A two-machine host paging its selector on TRIG and one param on SRC.
    fn machine_rule() -> Rule {
        let variant = |value: u32, name: &'static str, tune_max: f64| MachineVariant {
            value,
            name: Cow::Borrowed(name),
            page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG"), Cow::Borrowed("SRC")]),
            pages: Cow::Owned(vec![
                (
                    MACHINE,
                    PageRef {
                        page: Cow::Borrowed("TRIG"),
                        slot: 0,
                    },
                ),
                (
                    TUNE,
                    PageRef {
                        page: Cow::Borrowed("SRC"),
                        slot: 0,
                    },
                ),
            ]),
            overlays: Cow::Owned(vec![
                (
                    MACHINE,
                    ParamOverlay {
                        min: 0.0,
                        max: 1.0,
                        default: value as f64,
                        identity: true,
                    },
                ),
                (
                    TUNE,
                    ParamOverlay {
                        min: -24.0,
                        max: tune_max,
                        default: 0.0,
                        identity: false,
                    },
                ),
            ]),
        };
        let variants = vec![variant(0, "Kick", 24.0), variant(1, "Bell", 12.0)];
        Rule {
            name: Cow::Borrowed("Kick"),
            page_groups: variants[0].page_groups.clone(),
            param_pages: variants[0].pages.clone(),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Borrowed(&[]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Owned(variants),
        }
    }

    fn registry() -> ViewRegistry {
        ViewRegistry {
            selections: MachineSelections::new(),
            rules: HashMap::from([(20u32, machine_rule())]),
            chains: vec![TrackChain {
                engine_node_id: 20,
                chain_ids: vec![],
            }],
            node_infos: HashMap::from([(
                20u32,
                NodeInfo {
                    display_name: Some("Kick".into()),
                    params: vec![
                        ParamInfo {
                            id: MACHINE,
                            name: "machine".into(),
                            stepped: true,
                            options: None,
                            default: 0.0,
                        },
                        ParamInfo {
                            id: TUNE,
                            name: "tune".into(),
                            stepped: false,
                            options: None,
                            default: 0.0,
                        },
                    ],
                },
            )]),
        }
    }

    fn view_meta(msg: ServerMsg) -> (Vec<ViewMetaPage>, Vec<ViewMetaVariantSet>) {
        match msg {
            ServerMsg::ViewMeta {
                pages, variants, ..
            } => (pages, variants),
            other => panic!("expected view_meta, got {other:?}"),
        }
    }

    /// ADR-041 amendment 3. Both fields were hardwired `None`, so until now no
    /// client could tell a machine selector from a continuous knob, let alone
    /// name its positions.
    #[test]
    fn stepped_and_options_reach_the_wire_for_the_machine_param() {
        let (pages, _) = view_meta(registry().assemble(0, None).unwrap());
        let trig = pages.iter().find(|p| p.id == "TRIG").unwrap();
        let m = &trig.params[0];
        assert_eq!(m.id, "machine");
        assert_eq!(m.stepped, Some(true));
        assert_eq!(
            m.options.as_deref().unwrap(),
            [Some("Kick".to_string()), Some("Bell".to_string())],
            "indexed by value: options[0] is machine 0"
        );

        let src = pages.iter().find(|p| p.id == "SRC").unwrap();
        assert_eq!(src.params[0].stepped, None, "a continuous param sends no flag");
        assert!(src.params[0].options.is_none());
    }

    /// BUG-056 (#157). Assembly with no selection resolves every machine host
    /// from its cap-doc default — the machine the node was *built* with — and
    /// cap-docs are collected once at startup. So a client that switched a
    /// track to machine 1 kept being told `active: 0`, with machine 0's pages,
    /// for the rest of the process's life.
    #[test]
    fn view_meta_reports_the_machine_the_node_is_on_not_the_one_it_was_built_with() {
        // Give machine 1 an empty SRC page, so the switch is observable in the
        // merged pages and not only in `active`.
        let mut rule = machine_rule();
        let mut vs = rule.variants.to_vec();
        vs[1].pages = Cow::Owned(vec![(
            MACHINE,
            PageRef {
                page: Cow::Borrowed("TRIG"),
                slot: 0,
            },
        )]);
        rule.variants = Cow::Owned(vs);
        let reg = ViewRegistry {
            rules: HashMap::from([(20u32, rule)]),
            ..registry()
        };

        let drawn = |msg: ServerMsg| {
            let (pages, variants) = view_meta(msg);
            let ids: Vec<String> = pages.into_iter().map(|p| p.id).collect();
            (variants[0].active, ids)
        };

        assert_eq!(
            drawn(reg.assemble(0, None).unwrap()),
            (0, vec!["TRIG".to_string(), "SRC".to_string()]),
            "with no selection recorded, the cap-doc default still stands"
        );

        reg.selections.set(20, 1);
        assert_eq!(
            drawn(reg.assemble(0, None).unwrap()),
            (1, vec!["TRIG".to_string()]),
            "after the switch, `active` and the merged pages must both follow"
        );
    }

    /// The path `pump` has to watch to keep `selections` current. Derived from
    /// assembly rather than re-implemented, so it cannot name a param the
    /// assembler does not treat as the selector.
    #[test]
    fn machine_select_paths_names_the_hosts_identity_param() {
        assert_eq!(
            registry().machine_select_paths(),
            HashMap::from([("/node/20/param/machine".to_string(), 20u32)])
        );
    }

    /// A chain of ordinary nodes has no selector to watch — `pump` must not
    /// be handed a path that never carries a machine.
    #[test]
    fn a_plain_track_contributes_no_machine_select_path() {
        let mut rule = machine_rule();
        rule.variants = Cow::Borrowed(&[]);
        let reg = ViewRegistry {
            rules: HashMap::from([(20u32, rule)]),
            ..registry()
        };
        assert!(reg.machine_select_paths().is_empty());
    }

    #[test]
    fn every_machine_reaches_the_wire_with_its_own_pages_and_overlays() {
        let (pages, variants) = view_meta(registry().assemble(0, None).unwrap());
        assert_eq!(variants.len(), 1);
        let set = &variants[0];
        assert_eq!(set.node_id, 20);
        assert_eq!(set.select_param.as_deref(), Some("machine"));
        assert_eq!(set.active, 0);
        assert_eq!(set.variants.len(), 2);

        // The active machine's pre-merged pages are the ones already sent.
        // Looked up by `value`, not by index — they coincide in this fixture,
        // so indexing would still pass on code that confused the two.
        let active = set
            .variants
            .iter()
            .find(|v| v.value == set.active)
            .expect("active names a declared machine");
        assert_eq!(active.pages, pages);

        let tune_max = |i: usize| {
            set.variants[i]
                .overlays
                .iter()
                .find(|o| o.param == "tune")
                .unwrap()
                .max
        };
        assert_eq!(tune_max(0), 24.0);
        assert_eq!(
            tune_max(1),
            12.0,
            "each machine carries its own range; the bank's union would be 24"
        );
        assert!(
            set.variants[1]
                .overlays
                .iter()
                .find(|o| o.param == "machine")
                .unwrap()
                .identity,
            "the selector is flagged on every machine, not just the first"
        );
    }

    /// A track of ordinary continuous params — every track in the default
    /// instrument but the four voices — must serialize exactly as it did
    /// before: no empty `variants` key, no `stepped: false` noise.
    #[test]
    fn a_plain_track_adds_nothing_to_the_wire() {
        let mut reg = registry();
        let mut rule = machine_rule();
        rule.variants = Cow::Borrowed(&[]);
        reg.rules.insert(20, rule);
        for p in reg.node_infos.get_mut(&20).unwrap().params.iter_mut() {
            p.stepped = false;
        }

        let msg = reg.assemble(0, None).unwrap();
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("variants"), "{json}");
        assert!(!json.contains("stepped"), "{json}");
        assert!(!json.contains("options"), "{json}");
    }

    /// …but a stepped param on a plain node still declares itself. `stepped`
    /// is the cap-doc's answer, not a machine-host special case — a sampler's
    /// slice selector gets it too.
    #[test]
    fn a_stepped_param_declares_itself_without_any_variants() {
        let mut reg = registry();
        let mut rule = machine_rule();
        rule.variants = Cow::Borrowed(&[]);
        reg.rules.insert(20, rule);

        let (pages, variants) = view_meta(reg.assemble(0, None).unwrap());
        assert!(variants.is_empty());
        let trig = pages.iter().find(|p| p.id == "TRIG").unwrap();
        assert_eq!(trig.params[0].stepped, Some(true));
        assert!(
            trig.params[0].options.is_none(),
            "stepped without variants has no names to offer"
        );
    }
}
