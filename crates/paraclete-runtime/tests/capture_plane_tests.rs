/// AN1 C3: the capture-transfer plane — a `RecorderNode`'s fulfilled
/// capture reaches the main thread within one `process()` cycle, and a
/// drained buffer sent back reaches the originating node's
/// `return_capture_buffer()`, even under sustained pressure (D9, mirroring
/// AN0's D7 return-ring stress test in `asset_plane_tests.rs`).
///
/// Uses a local mock node (same style as `asset_plane_tests.rs`'s
/// `SampleCacheNode`) rather than the real `RecorderNode` — this suite
/// exercises the executor/configurator plumbing, not `RecorderNode`'s own
/// capture logic, which `paraclete-nodes::recorder`'s unit tests already
/// cover. Also keeps this crate's tests from needing a dev-dependency on
/// the L3 `paraclete-nodes` crate.
use std::sync::{Arc, Mutex};

use paraclete_node_api::{
    NodeCommand, PortDescriptor, ProcessInput, ProcessOutput, RecorderCapture,
};
use paraclete_node_api::Node;
use paraclete_runtime::NodeConfigurator;

const CMD_MOCK_CAPTURE: u32 = 16;

/// A node whose `process()` reacts to a command by staging a capture
/// (mirroring `RecorderNode`'s own `CMD_RECORDER_CAPTURE` handling shape)
/// and whose `return_capture_buffer()` deposits whatever it's handed into
/// a shared vec the test can inspect.
struct MockRecorderNode {
    ports: Vec<PortDescriptor>,
    node_id: u32,
    pending: Option<RecorderCapture>,
    returned: Arc<Mutex<Vec<Box<[f32]>>>>,
}

impl MockRecorderNode {
    fn new(returned: Arc<Mutex<Vec<Box<[f32]>>>>) -> Self {
        Self { ports: vec![], node_id: 0, pending: None, returned }
    }
}

impl Node for MockRecorderNode {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }
    fn set_node_id(&mut self, id: u32) {
        self.node_id = id;
    }
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        for cmd in input.commands {
            if cmd.type_id == CMD_MOCK_CAPTURE {
                self.pending = Some(RecorderCapture {
                    node_id: self.node_id,
                    request_id: cmd.arg0 as u32,
                    frames: 4,
                    buf: vec![cmd.arg1 as f32; 4].into_boxed_slice(),
                });
            }
        }
    }
    fn drain_pending_capture(&mut self) -> Option<RecorderCapture> {
        self.pending.take()
    }
    fn return_capture_buffer(&mut self, buf: Box<[f32]>) {
        self.returned.lock().unwrap().push(buf);
    }
}

fn cmd(target_id: u32, arg0: i64, arg1: f64) -> NodeCommand {
    NodeCommand { target_id, type_id: CMD_MOCK_CAPTURE, arg0, arg1 }
}

#[test]
fn capture_reaches_the_main_thread_within_one_cycle() {
    let returned = Arc::new(Mutex::new(Vec::new()));
    let node = MockRecorderNode::new(returned);
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(7, Box::new(node));
    let mut exec = conf.build_executor();

    conf.send_command(cmd(7, 42, 9.0)).expect("ring should have room");

    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);

    let capture = conf.poll_recorder_capture().expect("capture must reach the main thread");
    assert_eq!(capture.request_id, 42);
    assert_eq!(capture.frames, 4);
    assert!(capture.buf[..4].iter().all(|&s| s == 9.0));
}

#[test]
fn no_capture_is_available_before_the_command_is_processed() {
    let returned = Arc::new(Mutex::new(Vec::new()));
    let node = MockRecorderNode::new(returned);
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(7, Box::new(node));
    let _exec = conf.build_executor();
    assert!(conf.poll_recorder_capture().is_none());
}

#[test]
fn a_returned_buffer_reaches_the_originating_node_within_one_cycle() {
    let returned = Arc::new(Mutex::new(Vec::new()));
    let node = MockRecorderNode::new(returned.clone());
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(7, Box::new(node));
    let mut exec = conf.build_executor();

    conf.send_command(cmd(7, 1, 5.0)).unwrap();
    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2);
    let capture = conf.poll_recorder_capture().expect("capture must be ready");
    assert!(returned.lock().unwrap().is_empty(), "not returned yet");

    conf.return_capture_buffer(paraclete_node_api::CaptureBufferReturn {
        node_id: capture.node_id,
        buf: capture.buf,
    })
    .expect("return ring should have room");
    exec.process(&mut out, 2); // drain_capture_returns() runs at the top of process()

    let got = returned.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].len(), 4);
}

#[test]
fn a_return_for_a_node_no_longer_in_the_graph_is_held_not_dropped_in_place() {
    // Route a CaptureBufferReturn at a node_id that was never added — the
    // orphan path (`drain_capture_returns`'s `orphaned_capture_buffers`).
    // The only thing this test can prove from outside the crate is that it
    // doesn't panic; the "never freed on the audio thread" claim is a
    // structural guarantee (the buffer moves into a Vec, not a drop) that
    // `paraclete-runtime`'s own internals would need a white-box test to
    // assert on directly — this is the black-box half.
    let mut conf = NodeConfigurator::new(44100.0, 64);
    let mut exec = conf.build_executor();
    conf.return_capture_buffer(paraclete_node_api::CaptureBufferReturn {
        node_id: 999,
        buf: vec![1.0f32; 4].into_boxed_slice(),
    })
    .unwrap();
    let mut out = vec![0.0f32; 64 * 2];
    exec.process(&mut out, 2); // must not panic
}

/// D9's backpressure idiom under sustained pressure — mirrors
/// `asset_plane_tests.rs`'s `sustained_return_pressure_never_loses_a_displaced_buffer`.
/// Many capture requests, none drained by `poll_recorder_capture()` in
/// between, must never lose a capture, panic, or exceed the pre-allocated
/// backlog (the `debug_assert!` in `deliver_pending_captures` would fire
/// in this debug-profile test run if that invariant broke).
#[test]
fn sustained_capture_pressure_never_loses_a_capture() {
    let returned = Arc::new(Mutex::new(Vec::new()));
    let node = MockRecorderNode::new(returned);
    let mut conf = NodeConfigurator::new(44100.0, 64);
    conf.add_node(7, Box::new(node));
    let mut exec = conf.build_executor();
    let mut out = vec![0.0f32; 64 * 2];

    // CAPTURE_FORWARD_RING_CAPACITY is 8 (private) — 12 successive
    // requests, each processed one at a time (the mock only ever holds one
    // pending capture, matching RecorderNode's own single-outbox shape),
    // with NO draining until every request has been sent. This genuinely
    // exceeds the ring's capacity (8) partway through, forcing
    // deliver_pending_captures() to hold the overflow in
    // pending_capture_backlog (also capacity 8) rather than the ring
    // alone — unlike draining every few cycles, which would keep the ring
    // comfortably under capacity the whole time and never touch the
    // backlog path at all. 12 stays within ring + backlog capacity
    // (8 + 8 = 16) so this proves the backlog absorbs real overflow
    // without ever tripping deliver_pending_captures()'s debug_assert.
    const N: usize = 12;
    let mut collected = Vec::new();
    for i in 0..N {
        conf.send_command(cmd(7, i as i64, i as f64)).unwrap();
        exec.process(&mut out, 2);
    }
    // Final drain.
    for _ in 0..N {
        exec.process(&mut out, 2);
        while let Some(c) = conf.poll_recorder_capture() {
            collected.push(c.request_id);
        }
    }

    collected.sort_unstable();
    let expected: Vec<u32> = (0..N as u32).collect();
    assert_eq!(collected, expected, "every capture request must eventually be delivered, none lost");
}
