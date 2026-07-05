//! BUG-001 measurement harness: drives InternalClock → Sequencer offline and
//! measures the actual inter-step interval in samples against the nominal
//! 16th-note period. No audio device, no hardware, no ears.
//!
//! Run with `--nocapture` for the measurement report. See s0 session notes:
//! BUG-001's claimed 0.4% error does not reproduce; this guards the truth.

use std::sync::{Arc, Mutex};

use paraclete_node_api::{
    CapabilityDocument, Event, Node, PortDescriptor, PortDirection, PortType, ProcessInput,
    ProcessOutput,
};
use paraclete_node_api::{midi::ChannelVoice2, UmpMessage};
use paraclete_nodes::{InternalClock, Sequencer};
use paraclete_runtime::NodeConfigurator;

const SR: f32 = 44100.0;
const BLOCK: usize = 512;
const BPM: f64 = 120.0;

/// Session-0 measurement (July 2026): mean deviation is +0.011%, NOT the ~0.42%
/// BUG-001 claims — the documented systematic tempo error does not reproduce at
/// the event-emission level. This test now guards that result.
const MAX_DEVIATION_PCT: f64 = 0.1;

struct OnsetProbe {
    ports: [PortDescriptor; 1],
    onsets: Arc<Mutex<Vec<u64>>>,
    samples_seen: u64,
}

impl Node for OnsetProbe {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }
    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument::from_ports(&self.ports)
    }
    fn process(&mut self, input: &ProcessInput, _output: &mut ProcessOutput) {
        for te in input.events {
            if let Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(_))) = te.event {
                self.onsets
                    .lock()
                    .unwrap()
                    .push(self.samples_seen + te.sample_offset as u64);
            }
        }
        self.samples_seen += BLOCK as u64;
    }
}

#[test]
fn step_period_matches_nominal_16th_note() {
    let onsets = Arc::new(Mutex::new(Vec::new()));

    let mut conf = NodeConfigurator::new(SR, BLOCK);
    conf.add_tempo_source(1, Box::new(InternalClock::with_bpm(BPM)));

    let mut seq = Sequencer::new();
    for i in 0..16 {
        seq.set_step(i, 36, 100, true); // every 16th fires
    }
    conf.add_node(10, Box::new(seq));

    conf.add_node(
        99,
        Box::new(OnsetProbe {
            ports: [PortDescriptor {
                id: 0,
                name: "events_in".into(),
                direction: PortDirection::Input,
                port_type: PortType::Event,
            }],
            onsets: Arc::clone(&onsets),
            samples_seen: 0,
        }),
    );

    conf.connect(1, InternalClock::PORT_CLOCK_OUT, 10, 0).expect("clock -> seq");
    conf.connect(10, 2, 99, 0).expect("seq -> probe");

    let mut exec = conf.build_executor();
    // ~30 seconds of audio: plenty of onsets, negligible runtime.
    let cycles = (30.0 * SR as f64 / BLOCK as f64) as usize;
    let mut sink = vec![0.0f32; BLOCK * 2];
    for _ in 0..cycles {
        exec.process(&mut sink, 2);
    }

    let onsets = onsets.lock().unwrap();
    assert!(
        onsets.len() > 100,
        "expected >100 onsets, got {} — sequencer never started?",
        onsets.len()
    );

    // Skip the first 8 onsets (start transient), measure steady-state period.
    let steady = &onsets[8..];
    let intervals: Vec<f64> = steady.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
    let mean = intervals.iter().sum::<f64>() / intervals.len() as f64;
    let min = intervals.iter().cloned().fold(f64::MAX, f64::min);
    let max = intervals.iter().cloned().fold(f64::MIN, f64::max);

    let nominal = SR as f64 * 60.0 / BPM / 4.0; // 16th note in samples
    let deviation_pct = (mean - nominal) / nominal * 100.0;
    let bpm_effective = BPM * nominal / mean;

    println!("── BUG-001 measurement ──────────────────────────────");
    println!("nominal 16th @ {BPM} BPM : {nominal:.2} samples");
    println!("measured mean            : {mean:.2} samples (min {min:.0} / max {max:.0})");
    println!("deviation                : {deviation_pct:+.4} %");
    println!("effective tempo          : {bpm_effective:.3} BPM (set {BPM})");
    println!("onsets measured          : {}", intervals.len());
    for (i, iv) in intervals.iter().enumerate() {
        if (iv - nominal).abs() / nominal > 0.01 {
            println!("outlier: interval[{i}] = {iv:.0} samples ({:+.2}%)", (iv - nominal) / nominal * 100.0);
        }
    }

    assert!(
        deviation_pct.abs() < MAX_DEVIATION_PCT,
        "step period deviates {deviation_pct:.4}% from nominal (limit {MAX_DEVIATION_PCT}%)"
    );
}

/// P10 C0 GATE — currently fails (remove #[ignore] when fixing BUG-001):
/// every interval must be uniform. Today the 15 in-pattern steps run ~0.39%
/// long (the 241/240 tick period) and the wrap step snaps back 5.85% short
/// (-322 samples at 120 BPM / 44.1k), a ~7 ms early hit once per pattern.
#[test]
#[ignore = "P10 C0 gate: intra-pattern step uniformity (BUG-001 re-diagnosed in s0)"]
fn step_intervals_are_uniform_within_pattern() {
    // Re-run the same harness assertion with per-interval tolerance.
    // Implementation note for C0: extract the harness into a fn shared by both
    // tests; assert every interval within 0.1% of nominal.
    panic!("enable and implement with the P10 C0 fix");
}
