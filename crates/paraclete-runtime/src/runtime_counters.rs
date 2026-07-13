use std::sync::atomic::AtomicU64;

use paraclete_node_api::StateBusValue;

#[derive(Default)]
pub struct RuntimeCounters {
    pub buffers_processed: AtomicU64,
    pub dropout_lock_miss: AtomicU64,
    pub dropout_no_executor: AtomicU64,
    pub state_bus_overflows: AtomicU64,
}

impl RuntimeCounters {
    pub fn publish_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        use std::sync::atomic::Ordering;

        let load = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64;
        buf.push(("/engine/buffers_processed".to_string(), StateBusValue::Float(load(&self.buffers_processed))));
        buf.push(("/engine/dropout_lock_miss".to_string(), StateBusValue::Float(load(&self.dropout_lock_miss))));
        buf.push(("/engine/dropout_no_executor".to_string(), StateBusValue::Float(load(&self.dropout_no_executor))));
        buf.push(("/engine/state_bus_overflows".to_string(), StateBusValue::Float(load(&self.state_bus_overflows))));
    }
}
