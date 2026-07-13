use std::collections::HashMap;
use paraclete_app::instrument::InstrumentDefinition;

pub struct NameResolver {
    names: HashMap<String, u32>,
}

impl NameResolver {
    pub fn from_instrument(def: &InstrumentDefinition) -> Self {
        let mut names = HashMap::new();
        for node in &def.nodes {
            // Priority: short type_tag name > display_name > full type_tag
            let tag_key = node.type_tag.to_lowercase();
            if let Some(colon) = node.type_tag.find(':') {
                let short = node.type_tag[colon + 1..].to_lowercase();
                if names.contains_key(&short) {
                    eprintln!("[test-driver] warning: duplicate short name '{}' — using last match (id {})",
                        short, node.id);
                }
                names.insert(short, node.id);
            }
            if let Some(dn) = &node.display_name {
                let key = dn.to_lowercase();
                names.entry(key).or_insert(node.id);
            }
            names.entry(tag_key).or_insert(node.id);
        }
        Self { names }
    }

    pub fn resolve(&self, target: &str) -> Option<u32> {
        if let Ok(id) = target.parse::<u32>() {
            return Some(id);
        }
        self.names.get(&target.to_lowercase()).copied()
    }

    pub fn resolve_required(&self, target: &str) -> Result<u32, String> {
        self.resolve(target)
            .ok_or_else(|| format!("target not found: {}", target))
    }

    /// Empty resolver for unit tests. Numeric targets still resolve (they parse
    /// directly); name lookups all miss.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self { names: HashMap::new() }
    }
}
