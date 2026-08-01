use paraclete_app::instrument::InstrumentDefinition;
use std::collections::HashMap;

/// Every name one node answers to, plus a human label for diagnostics.
///
/// `keys` is ordered most-specific-first, which is also the order we prefer
/// when *suggesting* an unambiguous handle in an ambiguity error.
struct NodeNames {
    id: u32,
    label: String,
    keys: Vec<String>,
}

/// Resolves a scenario's `target:` to a node id.
///
/// A name claimed by more than one node is a **hard error** listing the
/// candidates, never a silent last-wins (INFRA-012): the old resolver let a
/// voice's short type-tag overwrite a sequencer's display name, so
/// `target: Kick` addressed the engine and every `toggle_step` against it was
/// discarded by a node that does not implement the command — while the
/// scenario still passed.
pub struct NameResolver {
    /// lowercased name -> every node id claiming it; len > 1 is ambiguous
    names: HashMap<String, Vec<u32>>,
    nodes: Vec<NodeNames>,
}

impl NameResolver {
    pub fn from_instrument(def: &InstrumentDefinition) -> Self {
        let mut names: HashMap<String, Vec<u32>> = HashMap::new();
        let mut nodes = Vec::new();

        for node in &def.nodes {
            let tag = node.type_tag.to_lowercase();
            let display = node.display_name.as_ref().map(|d| d.to_lowercase());

            // Most-specific first. `sequencer/kick` is unique wherever tag and
            // display name differ, which is what makes it worth suggesting;
            // two nodes sharing both still collide, and `unique_handle`'s
            // numeric fallback below is what covers that case.
            let mut keys: Vec<String> = Vec::new();
            let mut push = |k: String| {
                if !k.is_empty() && !keys.contains(&k) {
                    keys.push(k);
                }
            };
            if let Some(dn) = &display {
                push(format!("{}/{}", tag, dn));
            }
            push(tag.clone());
            if let Some(dn) = &display {
                push(dn.clone());
            }
            if let Some(colon) = tag.find(':') {
                push(tag[colon + 1..].to_string());
            }

            for key in &keys {
                names.entry(key.clone()).or_default().push(node.id);
            }

            let label = match &node.display_name {
                Some(dn) => format!("node {} ({}, \"{}\")", node.id, node.type_tag, dn),
                None => format!("node {} ({})", node.id, node.type_tag),
            };
            nodes.push(NodeNames {
                id: node.id,
                label,
                keys,
            });
        }

        Self { names, nodes }
    }

    pub fn resolve_required(&self, target: &str) -> Result<u32, String> {
        if let Ok(id) = target.parse::<u32>() {
            return Ok(id);
        }
        match self.names.get(&target.to_lowercase()).map(Vec::as_slice) {
            Some([id]) => Ok(*id),
            Some(ids) if ids.len() > 1 => Err(self.ambiguity_error(target, ids)),
            _ => Err(format!("target not found: {}", target)),
        }
    }

    /// The most specific name that reaches exactly this node, falling back to
    /// its numeric id when every name it answers to is contested.
    fn unique_handle(&self, node: &NodeNames) -> String {
        node.keys
            .iter()
            .find(|k| self.names.get(*k).is_some_and(|ids| ids.len() == 1))
            .cloned()
            .unwrap_or_else(|| node.id.to_string())
    }

    fn ambiguity_error(&self, target: &str, ids: &[u32]) -> String {
        let mut msg = format!(
            "target '{}' is ambiguous — {} nodes answer to that name:",
            target,
            ids.len()
        );
        for id in ids {
            match self.nodes.iter().find(|n| n.id == *id) {
                Some(node) => {
                    msg.push_str(&format!("\n  {} — use `{}`", node.label, self.unique_handle(node)))
                }
                None => msg.push_str(&format!("\n  node {} — use `{}`", id, id)),
            }
        }
        msg
    }

    /// Empty resolver for unit tests. Numeric targets still resolve (they parse
    /// directly); name lookups all miss.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            names: HashMap::new(),
            nodes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_app::instrument::NodeDef;

    fn node(id: u32, type_tag: &str, display_name: Option<&str>) -> NodeDef {
        NodeDef {
            id,
            type_tag: type_tag.to_string(),
            display_name: display_name.map(str::to_string),
            initial_params: HashMap::new(),
            plugin_id: None,
            plugin_path: None,
            channel_count: None,
            default_note: None,
            sample: None,
        }
    }

    fn instrument(nodes: Vec<NodeDef>) -> InstrumentDefinition {
        InstrumentDefinition {
            format_version: 1,
            name: "test".to_string(),
            bpm: 120.0,
            nodes,
            edges: Vec::new(),
            macros: Vec::new(),
            profiles: Vec::new(),
        }
    }

    /// Mirrors the shape of the default `instrument.yaml`: sequencers carrying
    /// display names that collide with the voices' short type tags.
    fn default_shaped() -> InstrumentDefinition {
        instrument(vec![
            node(1, "internal_clock", None),
            node(10, "sequencer", Some("Kick")),
            node(11, "sequencer", Some("Snare")),
            node(20, "analog_engine:kick", None),
            node(21, "analog_engine:snare", None),
        ])
    }

    #[test]
    fn numeric_target_bypasses_names() {
        let r = NameResolver::from_instrument(&default_shaped());
        assert_eq!(r.resolve_required("10").unwrap(), 10);
        // An undeclared id resolves too, and the command is then dropped by a
        // node that does not exist — the same silent-no-op INFRA-012 was filed
        // about, reached through the numeric door instead of the name one.
        // Recorded here as current behaviour, NOT endorsed: see the follow-up
        // issue. Do not treat this assertion as the contract.
        assert_eq!(r.resolve_required("999").unwrap(), 999);
    }

    #[test]
    fn unique_name_resolves() {
        let r = NameResolver::from_instrument(&default_shaped());
        assert_eq!(r.resolve_required("internal_clock").unwrap(), 1);
        assert_eq!(r.resolve_required("analog_engine:kick").unwrap(), 20);
    }

    #[test]
    fn name_lookup_is_case_insensitive() {
        let r = NameResolver::from_instrument(&default_shaped());
        assert_eq!(r.resolve_required("Sequencer/KICK").unwrap(), 10);
    }

    /// INFRA-012: this silently resolved to 20 (the voice), so `toggle_step`
    /// against `Kick` landed on a node that ignores it.
    #[test]
    fn display_name_colliding_with_short_tag_is_an_error() {
        let r = NameResolver::from_instrument(&default_shaped());
        let err = r.resolve_required("Kick").unwrap_err();
        assert!(err.contains("ambiguous"), "{}", err);
        assert!(err.contains("node 10"), "{}", err);
        assert!(err.contains("node 20"), "{}", err);
    }

    /// The error has to name the way out, or it just relocates the confusion.
    #[test]
    fn ambiguity_error_suggests_unambiguous_handles() {
        let r = NameResolver::from_instrument(&default_shaped());
        let err = r.resolve_required("kick").unwrap_err();
        assert!(err.contains("`sequencer/kick`"), "{}", err);
        assert!(err.contains("`analog_engine:kick`"), "{}", err);
    }

    /// A type tag shared by several nodes is ambiguous too — the old resolver
    /// silently handed back whichever came first.
    #[test]
    fn shared_type_tag_is_an_error() {
        let r = NameResolver::from_instrument(&default_shaped());
        let err = r.resolve_required("sequencer").unwrap_err();
        assert!(err.contains("ambiguous"), "{}", err);
    }

    #[test]
    fn qualified_name_disambiguates_both_directions() {
        let r = NameResolver::from_instrument(&default_shaped());
        assert_eq!(r.resolve_required("sequencer/kick").unwrap(), 10);
        assert_eq!(r.resolve_required("sequencer/snare").unwrap(), 11);
    }

    #[test]
    fn unknown_name_is_not_found_not_ambiguous() {
        let r = NameResolver::from_instrument(&default_shaped());
        let err = r.resolve_required("ghost").unwrap_err();
        assert!(err.contains("not found"), "{}", err);
    }

    /// A node whose display name equals its own short tag registers one key,
    /// not two — otherwise it would report itself as ambiguous with itself.
    #[test]
    fn display_name_equal_to_own_short_tag_is_not_self_ambiguous() {
        let def = instrument(vec![node(30, "analog_engine:clap", Some("Clap"))]);
        let r = NameResolver::from_instrument(&def);
        assert_eq!(r.resolve_required("clap").unwrap(), 30);
    }
}
