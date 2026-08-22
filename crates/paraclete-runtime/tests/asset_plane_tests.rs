/// AN0 C1: the asset-delivery plane — `SwapSample` reaches a node's
/// `swap_sample()` within one `process()` cycle, and a displaced
/// `Arc<SampleData>` reaches the configurator's return path (never dropped
/// on the audio thread) even under sustained return-ring pressure (D7).
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use paraclete_node_api::{
    PortDescriptor, ProcessInput, ProcessOutput, SampleData, SwapSample,
};
use paraclete_node_api::Node;
use paraclete_runtime::NodeConfigurator;

/// A node that caches whatever `SwapSample` it's sent, mirroring the real
/// `Sampler`'s `pool_cache: HashMap<u32, Arc<SampleData>>` shape (AN0-C2).
/// `received` lets the test observe delivery from outside without reaching
/// into the executor (the node itself is moved there by `build_executor()`).
struct SampleCacheNode {
    ports: Vec<PortDescriptor>,
    received: Arc<Mutex<Vec<u32>>>,
    cache: HashMap<u32, Arc<SampleData>>,
}

impl SampleCacheNode {
    fn new(received: Arc<Mutex<Vec<u32>>>) -> Self {
        Self { ports: vec![], received, cache: HashMap::new() }
    }
}

impl Node for SampleCacheNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }
    fn process(&mut self, _input: &ProcessInput, _output: &mut ProcessOutput) {}
    fn swap_sample(&mut self, pool_slot: u32, data: Arc<SampleData>) -> Option<Arc<SampleData>> {
        self.received.lock().unwrap().push(pool_slot);
        self.cache.insert(pool_slot, data)
    }
}

fn sample(tag: f32) -> Arc<SampleData> {
    Arc::new(SampleData { samples: vec![tag], rate: 44100.0, frames: 1 })
}

#[test]
fn swap_sample_reaches_the_target_node_within_one_cycle() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(5, Box::new(SampleCacheNode::new(received.clone())));
    let mut exec = conf.build_executor();

    conf.send_swap_sample(SwapSample { node_id: 5, pool_slot: 7, data: sample(1.0) })
        .expect("ring should have room");

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    assert_eq!(&*received.lock().unwrap(), &[7]);
}

/// Review finding: a `SwapSample` addressed to a `node_id` not in the graph
/// (or, symmetrically, to a `Device` — no `swap_sample` method) must not
/// let its `Arc<SampleData>` fall out of scope inside `process()` — that's
/// an audio-thread `free()` the same hazard D7 exists to prevent for an
/// actually-displaced buffer. The `strong_count == 2` check right after
/// `process()`, *before* `process_main_thread()` runs, is the part that
/// proves this: it can only hold if the buffer is still alive somewhere
/// (the return ring or `pending_sample_drops`), not already dropped in
/// place on the audio thread — a bare "eventually drops to 1" check can't
/// tell those two apart, since both converge to the same end state once
/// `process_main_thread()` has run.
#[test]
fn swap_sample_for_a_node_not_in_the_graph_returns_its_buffer_without_dropping_it_in_process() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(5, Box::new(SampleCacheNode::new(received.clone())));
    let mut exec = conf.build_executor();

    let orphan = sample(1.0);
    conf.send_swap_sample(SwapSample { node_id: 999, pool_slot: 0, data: orphan.clone() })
        .expect("ring should have room");

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2); // must not panic, must not drop `orphan` here

    assert_eq!(
        Arc::strong_count(&orphan),
        2,
        "the buffer must still be alive (in the return ring) right after process() — \
         if this is 1, it was dropped in place on the audio thread"
    );
    assert!(received.lock().unwrap().is_empty(), "no live node claimed it");

    conf.process_main_thread(); // drains the return ring, drops it here instead
    assert_eq!(Arc::strong_count(&orphan), 1);
}

#[test]
fn a_second_swap_on_the_same_slot_returns_the_displaced_buffer_for_main_thread_drop() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(5, Box::new(SampleCacheNode::new(received.clone())));
    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 64 * 2];

    let data1 = sample(1.0);
    conf.send_swap_sample(SwapSample { node_id: 5, pool_slot: 3, data: data1.clone() })
        .unwrap();
    exec.process(&mut out, 2);
    conf.process_main_thread();
    // Held by: this test's clone + the node's cache entry. Nothing displaced yet.
    assert_eq!(Arc::strong_count(&data1), 2);

    let data2 = sample(2.0);
    conf.send_swap_sample(SwapSample { node_id: 5, pool_slot: 3, data: data2.clone() })
        .unwrap();
    exec.process(&mut out, 2); // node.swap_sample returns Some(data1's clone)

    // Still alive right after process() — proves the displaced clone went
    // onto the return ring rather than being dropped in place inside it.
    assert_eq!(Arc::strong_count(&data1), 2);

    conf.process_main_thread(); // drains the return ring, drops it here

    assert_eq!(
        Arc::strong_count(&data1),
        1,
        "the node's clone of data1 should have been returned and dropped on the main thread"
    );
    assert_eq!(Arc::strong_count(&data2), 2, "data2 is now the node's cached entry");
}

/// D7: sustained return-ring pressure (many displacements queued without
/// `process_main_thread()` draining in between) must never lose a buffer or
/// panic — `pending_sample_drops` backpressures instead of growing past its
/// pre-allocated capacity (the growth path carries a `debug_assert!` that
/// would fire in this (debug-profile) test run if the invariant broke).
#[test]
fn sustained_return_pressure_never_loses_a_displaced_buffer() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(5, Box::new(SampleCacheNode::new(received.clone())));
    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 64 * 2];

    // ASSET_RETURN_RING_CAPACITY is 32 (private) — 64 successive swaps on the
    // same slot guarantees more displacements than the ring (or a single
    // process_main_thread drain) can hold, without ever calling
    // process_main_thread() in between, so the backlog is forced.
    const N: usize = 64;
    let mut buffers: Vec<Arc<SampleData>> = (0..N).map(|i| sample(i as f32)).collect();

    for buf in &buffers {
        // Retry on a full forward ring — it's small (32) too.
        let mut msg = SwapSample { node_id: 5, pool_slot: 3, data: buf.clone() };
        loop {
            match conf.send_swap_sample(msg) {
                Ok(()) => break,
                Err(returned) => {
                    msg = returned;
                    exec.process(&mut out, 2); // drain some forward-ring capacity
                }
            }
        }
    }

    // Drain everything: enough process()/process_main_thread() cycles for
    // the forward ring, the backpressure retry loop, and the return ring to
    // all fully empty. No single call is guaranteed to finish the job.
    for _ in 0..N * 2 {
        exec.process(&mut out, 2);
        conf.process_main_thread();
    }

    let last = buffers.pop().expect("N > 0");
    for (i, buf) in buffers.drain(..).enumerate() {
        assert_eq!(
            Arc::strong_count(&buf),
            1,
            "buffer {i} should have been displaced and returned by now, \
             not lost or left dangling in the node/queues"
        );
    }
    // The last-sent buffer is still the node's live cache entry — never
    // displaced, since nothing later swapped slot 3 again. Held by this
    // local `last` plus the node's cache: strong count 2, not 1.
    assert_eq!(Arc::strong_count(&last), 2);
    assert_eq!(received.lock().unwrap().len(), N);
}
