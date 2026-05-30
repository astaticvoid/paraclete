// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete — P3 entry point.
//!
//! Wires the P3 graph:
//!
//!   InternalClock → Sequencer → Sampler → AudioOutput
//!              LaunchpadEmulator → HardwareMappingNode ↗
//!
//! Esc or Ctrl-C to stop.

use std::time::Duration;

use paraclete_hal::{AudioBackend, LaunchpadEmulator};
use paraclete_nodes::{Sequencer, HardwareMappingNode, InternalClock, Sampler};
use paraclete_runtime::NodeConfigurator;
use paraclete_scripting::ScriptingEngine;

const SAMPLE_RATE: f32 = 44100.0;
const BLOCK_SIZE: usize = 512;

// Node IDs
const ID_CLOCK:    u32 = 1;
const ID_SEQ:      u32 = 2;
const ID_EMULATOR: u32 = 3;
const ID_MAPPER:   u32 = 4;
const ID_SAMPLER:  u32 = 5;

fn main() {
    eprintln!("[paraclete] booting P3");

    // ── L1: build graph ───────────────────────────────────────────────────────
    let mut conf = NodeConfigurator::new(SAMPLE_RATE, BLOCK_SIZE);

    let (_, _domain_id) = conf.add_tempo_source(
        ID_CLOCK,
        Box::new(InternalClock::with_domain(0)),
    );

    // Sequencer — active steps 0, 2, 4, 8
    let mut seq = Sequencer::new();
    seq.set_step(0, 60, 40000, true);  // C4
    seq.set_step(2, 64, 40000, true);  // E4
    seq.set_step(4, 67, 40000, true);  // G4
    seq.set_step(8, 72, 40000, true);  // C5
    conf.add_node(ID_SEQ, Box::new(seq));

    conf.add_hardware_device(ID_EMULATOR, Box::new(LaunchpadEmulator::new()));
    conf.add_node(ID_MAPPER, Box::new(HardwareMappingNode::default_chromatic(0)));

    // Sampler — loads kick.wav if present, silent otherwise.
    conf.add_node(ID_SAMPLER, Box::new(Sampler::with_path("samples/kick.wav")));

    // Clock → Sequencer
    conf.connect(ID_CLOCK, InternalClock::PORT_CLOCK_OUT,
                 ID_SEQ,   Sequencer::PORT_CLOCK_IN).expect("clock→seq");

    // Controller: Emulator → Mapper → Sequencer
    conf.connect(ID_EMULATOR, 0, ID_MAPPER, 0).expect("emulator→mapper");
    conf.connect(ID_MAPPER,   1, ID_SEQ, Sequencer::PORT_EVENTS_IN).expect("mapper→seq");

    // Sequencer → Sampler (triggers Negotiable handshake)
    conf.connect(ID_SEQ,     Sequencer::PORT_EVENTS_OUT,
                 ID_SAMPLER, Sampler::PORT_EVENTS_IN).expect("seq→sampler");

    // ── L4: scripting sandbox ─────────────────────────────────────────────────
    let bus_handle = conf.state_bus_handle();
    let mut scripting = ScriptingEngine::new();
    scripting.bind_state_bus(bus_handle);
    if scripting.eval_str("1 + 1").is_ok() {
        eprintln!("[paraclete] scripting engine OK");
    }

    let executor = conf.build_executor();
    eprintln!("[paraclete] graph built — InternalClock→Sequencer→Sampler");

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

    // Main loop — drain state bus between audio cycles.
    loop {
        conf.process_state_bus();
        std::thread::sleep(Duration::from_millis(1));
    }
}
