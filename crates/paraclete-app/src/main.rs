// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete — P2 entry point.
//!
//! Wires the sequencer graph:
//!
//!   InternalClock → Sequencer → SineOscillator → AudioOutput
//!              LaunchpadEmulator → HardwareMappingNode ↗
//!
//! Esc or Ctrl-C to stop.

use paraclete_hal::{AudioBackend, LaunchpadEmulator};
use paraclete_nodes::{Sequencer, HardwareMappingNode, InternalClock, SineOscillator};
use paraclete_runtime::NodeConfigurator;
use paraclete_scripting::ScriptingEngine;

const SAMPLE_RATE: f32 = 44100.0;
const BLOCK_SIZE: usize = 512;

// Node IDs
const ID_CLOCK:      u32 = 1;
const ID_SEQ:        u32 = 2;
const ID_EMULATOR:   u32 = 3;
const ID_MAPPER:     u32 = 4;
const ID_OSCILLATOR: u32 = 5;

fn main() {
    eprintln!("[paraclete] booting P2");

    // ── L4: scripting sandbox ─────────────────────────────────────────────────
    let scripting = ScriptingEngine::new();
    if scripting.eval_str("1 + 1").is_ok() {
        eprintln!("[paraclete] scripting engine OK");
    }

    // ── L1: build graph ───────────────────────────────────────────────────────
    let mut conf = NodeConfigurator::new(SAMPLE_RATE, BLOCK_SIZE);

    // Clock domain
    let (_, _domain_id) = conf.add_tempo_source(
        ID_CLOCK,
        Box::new(InternalClock::with_domain(0)),
    );

    // Sequencer — pre-load a simple chromatic pattern on steps 0, 2, 4, 8
    let mut seq = Sequencer::new();
    seq.set_step(0, 60, 40000, true);  // C4
    seq.set_step(2, 64, 40000, true);  // E4
    seq.set_step(4, 67, 40000, true);  // G4
    seq.set_step(8, 72, 40000, true);  // C5
    conf.add_node(ID_SEQ, Box::new(seq));

    // Controller chain
    conf.add_hardware_device(ID_EMULATOR, Box::new(LaunchpadEmulator::new()));
    conf.add_node(ID_MAPPER, Box::new(HardwareMappingNode::default_chromatic(0)));

    // Sound
    conf.add_node(ID_OSCILLATOR, Box::new(SineOscillator::new()));

    // Clock domain: TempoSource → Sequencer
    conf.connect(ID_CLOCK, InternalClock::PORT_CLOCK_OUT,
                      ID_SEQ,   Sequencer::PORT_CLOCK_IN).expect("clock→seq");

    // Controller: Emulator → Mapper → Sequencer events_in
    conf.connect(ID_EMULATOR, 0,
                      ID_MAPPER,   0).expect("emulator→mapper");
    conf.connect(ID_MAPPER,   1,
                      ID_SEQ,      Sequencer::PORT_EVENTS_IN).expect("mapper→seq");

    // Audio: Sequencer → Oscillator
    conf.connect(ID_SEQ,        Sequencer::PORT_EVENTS_OUT,
                      ID_OSCILLATOR, SineOscillator::PORT_EVENTS_IN).expect("seq→osc");

    let executor = conf.build_executor();
    eprintln!("[paraclete] graph built — TempoSource→Sequencer→SineOscillator");

    // ── L0: audio backend ─────────────────────────────────────────────────────
    let _audio = match AudioBackend::start(executor) {
        Ok(backend) => {
            eprintln!("[paraclete] sequencer running at 120 BPM");
            eprintln!("[paraclete] Esc or Ctrl-C to stop");
            backend
        }
        Err(e) => {
            eprintln!("[paraclete] audio backend error: {e}");
            std::process::exit(1);
        }
    };

    std::thread::park();
}
