use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use paraclete_node_api::StateBusValue;

/// The shared StateBus snapshot.
///
/// Written by the executor after each cycle (very briefly), read by the
/// NodeConfigurator on the main thread. The RwLock ensures correct concurrent access.
/// At P4 this will be replaced by a lock-free SPSC structure.
///
pub type StateBusSnapshot = Arc<RwLock<HashMap<String, StateBusValue>>>;

/// Create a new empty StateBus snapshot.
pub fn new_snapshot() -> StateBusSnapshot {
    Arc::new(RwLock::new(HashMap::new()))
}

/// A handle to a subscribed StateBus path.
///
/// Poll with `changed()` to detect updates since the last poll.
pub struct StateBusSubscription {
    pub path: String,
    last_value: Option<StateBusValue>,
}

impl StateBusSubscription {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into(), last_value: None }
    }

    /// Returns the current value if it changed since last poll, `None` otherwise.
    pub fn changed(
        &mut self,
        snapshot: &HashMap<String, StateBusValue>,
    ) -> Option<&StateBusValue> {
        let current = snapshot.get(&self.path);
        if current != self.last_value.as_ref() {
            self.last_value = current.cloned();
            self.last_value.as_ref()
        } else {
            None
        }
    }
}
