//! TK3 C0 (#180): the sequencer→state-bus→model round-trip that proves the
//! `step_detail` format contract end to end — a real `Sequencer` publishes
//! the bus text, and Theotokos's `parse_step_detail` reads all seven fields
//! back exactly.
//!
//! The cross-crate trip is what no single unit test can see: the producer
//! (`paraclete-nodes::sequencer`) and the consumer
//! (`paraclete_theotokos::model`) live in different crates. A plain unit test
//! on either side would prove only that that side is self-consistent; this
//! proves the wire format they share. (The spec marks this required, not
//! optional — §5.4.)

use paraclete_node_api::{
    AudioBuffer, EventOutputBuffer, ExtendedEventSlab, Node, NodeCommand, ProcessInput,
    ProcessOutput, StateBusValue, TransportInfo,
};
use paraclete_nodes::sequencer::Sequencer;
use paraclete_theotokos::model::{parse_step_detail, Model};

fn run_cmds(seq: &mut Sequencer, cmds: &[NodeCommand]) {
    let block = 64usize;
    let mut audio = AudioBuffer::new(2, block);
    let mut events_out = EventOutputBuffer::new(256);
    let transport = TransportInfo::default();
    let slab = ExtendedEventSlab::empty();
    let audio_ptr: *mut AudioBuffer = &mut audio;
    let audio_ref: &mut AudioBuffer = unsafe { &mut *audio_ptr };
    let mut outs = [audio_ref];
    let input = ProcessInput {
        audio_inputs: &[],
        signal_inputs: &[],
        events: &[],
        transport: &transport,
        sample_rate: 44100.0,
        block_size: block,
        extended_events: &slab,
        commands: cmds,
    };
    let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
    seq.process(&input, &mut output);
}

#[test]
fn sequencer_bus_to_model_round_trip_preserves_all_seven_fields() {
    let mut seq = Sequencer::new();
    seq.set_node_id(5);
    seq.activate(44100.0, 64);

    // Step 3 gets a non-default profile on every field: velocity 0.5,
    // length 0.125, timing +30, condition { probability 80, repeat 2/4,
    // fill 5 (NotFillA) }.
    let cond_enc = |prob: u8, n: u8, m: u8, fill: u8| {
        (prob as u64 | ((n as u64) << 8) | ((m as u64) << 16) | ((fill as u64) << 24)) as f64
    };
    run_cmds(
        &mut seq,
        &[
            NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP_VELOCITY,
                arg0: 3,
                arg1: 0.5,
            },
            NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP_LENGTH,
                arg0: 3,
                arg1: 0.125,
            },
            NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP_TIMING,
                arg0: 3,
                arg1: 30.0,
            },
            NodeCommand {
                target_id: 0,
                type_id: Sequencer::CMD_SET_STEP_CONDITION,
                arg0: 3,
                arg1: cond_enc(80, 2, 4, 5),
            },
        ],
    );

    let mut state = Vec::new();
    seq.published_state(&mut state);
    let text = state
        .iter()
        .find(|(k, _)| k == "/node/5/state/step_detail")
        .expect("step_detail path published")
        .1
        .clone();
    let StateBusValue::Text(text) = text else {
        panic!("step_detail is not Text");
    };

    let steps = parse_step_detail(&text);
    let d = steps.get(3).unwrap_or_else(|| panic!("expected step 3 tuple; got {} steps", steps.len()));
    assert!(
        (d.velocity - 0.5).abs() < 0.001,
        "velocity 0.5, got {}",
        d.velocity
    );
    assert_eq!(d.length, 0.125, "length");
    assert!(
        (d.timing - (30.0 / 47.0)).abs() < 0.001,
        "timing +30, got {}",
        d.timing
    );
    assert_eq!(d.probability, 80);
    assert_eq!(d.repeat_n, 2);
    assert_eq!(d.repeat_m, 4);
    assert_eq!(d.fill, 5, "NotFillA");

    // And the dispatch-side repack preserves the step's condition fields
    // when only the fill changes (read-modify-write on the condition jog).
    let packed = Model::pack_condition(d, 0); // switch to Ignore
    let enc = packed as i64 as u64;
    assert_eq!((enc & 0xFF) as u8, 80, "probability preserved");
    assert_eq!(((enc >> 8) & 0xFF) as u8, 2, "repeat_n preserved");
    assert_eq!(((enc >> 16) & 0xFF) as u8, 4, "repeat_m preserved");
    assert_eq!(((enc >> 24) & 0xFF) as u8, 0, "fill swapped to Ignore");
}
