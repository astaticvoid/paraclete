use std::collections::HashMap;
use crate::node::Node;

/// A value published to the StateBus.
#[derive(Clone, Debug, PartialEq)]
pub enum StateBusValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Text(String),
}

/// A node that publishes named values to the StateBus.
///
/// Path conventions:
///   `/node/{id}/param/{name}`
///   `/node/{id}/state/{key}`
///   `/transport/bpm`  (for TempoSource nodes)
pub trait StatePublisher: Node {}

// ── StateBusHandle ─────────────────────────────────────────────────────────────

/// The stable state bus access API.
///
/// All consumers — Rhai scripts, GUI, hardware LED feedback — use this.
/// The backing implementation (SPSC ring buffer draining) is invisible to
/// callers.
///
/// Owned by `NodeConfigurator` on the main thread. Shared with the scripting
/// engine via `Rc<RefCell<>>` — never crosses a thread boundary.
pub struct StateBusHandle {
    store: HashMap<String, StateBusValue>,
}

impl Default for StateBusHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl StateBusHandle {
    pub fn new() -> Self {
        Self { store: HashMap::new() }
    }

    /// Read a value from the bus. Returns `None` if the path has no value yet.
    pub fn read(&self, path: &str) -> Option<&StateBusValue> {
        self.store.get(path)
    }

    /// Write a value to the bus. Main-thread only.
    pub fn write(&mut self, path: &str, value: StateBusValue) {
        self.store.insert(path.to_string(), value);
    }

    /// Write with sandbox enforcement. Scripts may write to:
    ///   - `/script/` — script-private state (mode flags, UI state)
    ///   - `/node/{id}/param/` — declared node parameters
    ///
    /// Scripts may NOT write to:
    ///   - `/node/{id}/state/` — written by the executor, read-only for scripts
    ///   - `/transport/` — clock domain state
    ///   - `/surface/` — surface (hardware/Theoria) state
    pub fn write_sandboxed(&mut self, path: &str, value: StateBusValue) -> Result<(), &'static str> {
        let allowed = path.starts_with("/script/")
            || (path.starts_with("/node/") && {
                let after_node = &path["/node/".len()..];
                after_node.find('/').map(|slash| {
                    after_node[slash + 1..].starts_with("param/")
                }).unwrap_or(false)
            });
        if !allowed {
            return Err("scripts may not write to this path — only /script/ and /node/{id}/param/ are writable");
        }
        self.store.insert(path.to_string(), value);
        Ok(())
    }

    /// Create a subscription for `path`.
    pub fn subscribe(&self, path: &str) -> StateBusSubscription {
        StateBusSubscription { path: path.to_string(), last_value: None }
    }

    /// Poll a subscription: returns `Some(&value)` if the value at `path`
    /// changed since the last poll, `None` if unchanged.
    pub fn poll_subscription<'a>(
        &'a self,
        sub: &'a mut StateBusSubscription,
    ) -> Option<&'a StateBusValue> {
        let current = self.store.get(&sub.path);
        if current != sub.last_value.as_ref() {
            sub.last_value = current.cloned();
            sub.last_value.as_ref()
        } else {
            None
        }
    }

    /// Apply a batch of updates from the executor's SPSC ring buffer.
    /// Called by `NodeConfigurator::process_state_bus()`.
    pub fn apply_updates(&mut self, entries: Vec<(String, StateBusValue)>) {
        for (k, v) in entries {
            self.store.insert(k, v);
        }
    }

    /// Iterate every path currently on the bus. Additive (W1 Commit 2) — used
    /// by consumers that need a full-scan diff (e.g. the Antiphon state
    /// mirror) rather than a single-path subscription.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &StateBusValue)> {
        self.store.iter().map(|(k, v)| (k.as_str(), v))
    }
}

// ── StateBusSubscription ──────────────────────────────────────────────────────

/// A handle to a subscribed StateBus path.
///
/// Poll via `StateBusHandle::poll_subscription()` to detect updates.
pub struct StateBusSubscription {
    pub path: String,
    last_value: Option<StateBusValue>,
}

impl StateBusSubscription {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into(), last_value: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_bus_value_is_clone() {
        let v = StateBusValue::Float(3.14);
        let w = v.clone();
        assert_eq!(v, w);
    }

    #[test]
    fn state_bus_value_variants_are_distinct() {
        assert_ne!(StateBusValue::Int(1), StateBusValue::Float(1.0));
        assert_ne!(StateBusValue::Bool(true), StateBusValue::Int(1));
    }

    #[test]
    fn state_bus_handle_read_returns_none_for_unknown_path() {
        let handle = StateBusHandle::new();
        assert!(handle.read("/transport/bpm").is_none());
    }

    #[test]
    fn state_bus_handle_write_then_read_returns_value() {
        let mut handle = StateBusHandle::new();
        handle.write("/transport/bpm", StateBusValue::Float(140.0));
        assert_eq!(handle.read("/transport/bpm"), Some(&StateBusValue::Float(140.0)));
    }

    #[test]
    fn state_bus_handle_apply_updates_inserts_entries() {
        let mut handle = StateBusHandle::new();
        handle.apply_updates(vec![
            ("/node/1/state/step".to_string(), StateBusValue::Int(3)),
        ]);
        assert_eq!(handle.read("/node/1/state/step"), Some(&StateBusValue::Int(3)));
    }

    #[test]
    fn state_bus_subscription_detects_first_change() {
        let mut handle = StateBusHandle::new();
        handle.write("/node/1/state/x", StateBusValue::Int(0));
        let mut sub = handle.subscribe("/node/1/state/x");

        assert!(handle.poll_subscription(&mut sub).is_some());
        assert!(handle.poll_subscription(&mut sub).is_none());

        handle.write("/node/1/state/x", StateBusValue::Int(1));
        assert!(handle.poll_subscription(&mut sub).is_some());
    }

    #[test]
    fn state_bus_write_sandboxed_rejects_transport_path() {
        let mut handle = StateBusHandle::new();
        assert!(handle.write_sandboxed("/transport/bpm", StateBusValue::Float(120.0)).is_err());
    }

    #[test]
    fn state_bus_write_sandboxed_accepts_node_param_path() {
        let mut handle = StateBusHandle::new();
        assert!(handle.write_sandboxed("/node/1/param/pitch", StateBusValue::Float(2.0)).is_ok());
    }

    #[test]
    fn state_bus_write_sandboxed_rejects_node_state_path() {
        let mut handle = StateBusHandle::new();
        assert!(handle.write_sandboxed("/node/1/state/steps", StateBusValue::Text("1010".into())).is_err());
    }

    #[test]
    fn state_bus_write_sandboxed_accepts_script_path() {
        let mut handle = StateBusHandle::new();
        assert!(handle.write_sandboxed("/script/lp/mode", StateBusValue::Text("sequence".into())).is_ok());
        assert_eq!(handle.read("/script/lp/mode"), Some(&StateBusValue::Text("sequence".into())));
    }

    #[test]
    fn state_bus_write_sandboxed_rejects_transport_and_surface_paths() {
        let mut handle = StateBusHandle::new();
        assert!(handle.write_sandboxed("/transport/bpm", StateBusValue::Float(120.0)).is_err());
        assert!(handle.write_sandboxed("/surface/led/1", StateBusValue::Int(0)).is_err());
    }

    #[test]
    fn state_bus_handle_iter_yields_all_paths() {
        let mut handle = StateBusHandle::new();
        handle.write("/node/1/param/cutoff", StateBusValue::Float(0.5));
        handle.write("/transport/bpm", StateBusValue::Float(120.0));
        let mut seen: Vec<(&str, &StateBusValue)> = handle.iter().collect();
        seen.sort_by_key(|(k, _)| *k);
        assert_eq!(
            seen,
            vec![
                ("/node/1/param/cutoff", &StateBusValue::Float(0.5)),
                ("/transport/bpm", &StateBusValue::Float(120.0)),
            ]
        );
    }
}
