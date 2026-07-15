use std::sync::atomic::AtomicU64;

use paraclete_node_api::StateBusValue;

#[derive(Default)]
pub struct RuntimeCounters {
    pub buffers_processed: AtomicU64,
    pub dropout_lock_miss: AtomicU64,
    pub dropout_no_executor: AtomicU64,
    pub state_bus_overflows: AtomicU64,
    /// EWMA of `NodeExecutor::process()` elapsed time in microseconds.
    /// Stored as `f64::to_bits()` in the atomic; published to `/engine/cpu_us`.
    pub cpu_ewma_us: AtomicU64,
}

impl RuntimeCounters {
    pub fn publish_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        use std::sync::atomic::Ordering;

        let load = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64;
        buf.push((
            "/engine/buffers_processed".to_string(),
            StateBusValue::Float(load(&self.buffers_processed)),
        ));
        buf.push((
            "/engine/dropout_lock_miss".to_string(),
            StateBusValue::Float(load(&self.dropout_lock_miss)),
        ));
        buf.push((
            "/engine/dropout_no_executor".to_string(),
            StateBusValue::Float(load(&self.dropout_no_executor)),
        ));
        buf.push((
            "/engine/state_bus_overflows".to_string(),
            StateBusValue::Float(load(&self.state_bus_overflows)),
        ));
        let cpu = f64::from_bits(self.cpu_ewma_us.load(Ordering::Relaxed));
        buf.push(("/engine/cpu_us".to_string(), StateBusValue::Float(cpu)));
    }

    /// Update the process-time EWMA. Called once per `process()` call from the
    /// audio thread. `elapsed_us` is the wall-clock duration in microseconds.
    /// Alpha = 0.1 (fast response to CPU spikes, moderate smoothing).
    pub fn update_cpu_time(&self, elapsed_us: f64) {
        use std::sync::atomic::Ordering;
        const ALPHA: f64 = 0.1;
        let prev = f64::from_bits(self.cpu_ewma_us.load(Ordering::Relaxed));
        let ewma = if prev == 0.0 {
            elapsed_us
        } else {
            ALPHA * elapsed_us + (1.0 - ALPHA) * prev
        };
        self.cpu_ewma_us.store(ewma.to_bits(), Ordering::Relaxed);
    }
}
