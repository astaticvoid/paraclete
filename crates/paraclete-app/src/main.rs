// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete — P4 entry point.
//!
//! Graph:
//!   InternalClock
//!     ├──→ Sequencer[0..7] → Sampler[0..7] → Distortion[0..7] → Filter[0..7] ──┐
//!     └─────────────────────────────────────────────────────────────────────────→ MixNode → AudioOutput
//!   LaunchpadNode (or emulator) ──→ ScriptingGatewayNode[LP] ──┐
//!   DigitaktMidiNode             ──→ ScriptingGatewayNode[DT] ──┼──→ ScriptingEngine
//!   KeystepNode                  ──→ ScriptingGatewayNode[KS] ──┘
//!   KeystepNode → HardwareMappingNode → Sequencer[7] (bass)
//!
//! Hardware is opened gracefully — missing devices fall back silently.
//! Run with --dev-ui to enable state bus monitoring to stderr.

use std::env;
use std::time::Duration;

use paraclete_hal::{AudioBackend, DigitaktMidiNode, KeystepNode, LaunchpadEmulator, LaunchpadNode};
use paraclete_nodes::{
    DistortionNode, FilterNode, HardwareMappingNode, InternalClock,
    MixNode, Sampler, ScriptEventConsumer, ScriptingGatewayNode, Sequencer, TRACKS,
};
use paraclete_runtime::NodeConfigurator;
use paraclete_scripting::ScriptingEngine;

const SAMPLE_RATE: f32 = 44100.0;
const BLOCK_SIZE:  usize = 512;
const BPM:         f64   = 140.0;

const NUM_TRACKS: usize = 8;


// Auto-assigned node IDs
const ID_CLOCK:    u32 = 1;
const ID_MIX:      u32 = 2;
// Per-track: seq=10+i, samp=20+i, dist=30+i, filt=40+i
const ID_EMULATOR: u32 = 101;
const ID_LAUNCHPAD:u32 = 102;
const ID_DIGITAKT: u32 = 103;
const ID_KEYSTEP:  u32 = 104;
const ID_MAPPER:   u32 = 105;
// One gateway per device — correct device_id tagging without ProcessInput port metadata.
const ID_GW_LP:    u32 = 110; // Launchpad (or emulator) gateway
const ID_GW_DT:    u32 = 111; // Digitakt gateway
const ID_GW_KS:    u32 = 112; // Keystep gateway

fn seq_id(i: usize)  -> u32 { 10 + i as u32 }
fn samp_id(i: usize) -> u32 { 20 + i as u32 }
fn dist_id(i: usize) -> u32 { 30 + i as u32 }
fn filt_id(i: usize) -> u32 { 40 + i as u32 }

fn main() {
    let dev_ui = env::args().any(|a| a == "--dev-ui");

    eprintln!("[paraclete] booting P4");

    // ── L1: build graph ───────────────────────────────────────────────────────
    let mut conf = NodeConfigurator::new(SAMPLE_RATE, BLOCK_SIZE);

    let (_, _domain_id) = conf.add_tempo_source(
        ID_CLOCK,
        Box::new(InternalClock::with_bpm(BPM)),
    );

    // MixNode — 8 stereo inputs
    conf.add_node(ID_MIX, Box::new(MixNode::new(NUM_TRACKS)));

    // AudioOutput is handled by the HAL — not a graph node at P4.
    // The executor sums all audio outputs into the HAL buffer.

    // 8 track chains — initial pattern from pattern::TRACKS
    for i in 0..NUM_TRACKS {
        let preset = &TRACKS[i];
        let mut seq = Sequencer::with_name(preset.name);
        // Preset disabled at startup — use LP pads to build your own pattern.
        // Uncomment apply_preset to restore the default pattern:
        // apply_preset(&mut seq, preset);
        conf.add_node(seq_id(i),  Box::new(seq));
        conf.add_node(samp_id(i), Box::new(Sampler::with_path(&format!("samples/track{}.wav", i))));
        conf.add_node(dist_id(i), Box::new(DistortionNode::new()));
        conf.add_node(filt_id(i), Box::new(FilterNode::new()));

        conf.connect(ID_CLOCK,    InternalClock::PORT_CLOCK_OUT,
                     seq_id(i),   Sequencer::PORT_CLOCK_IN).expect("clock→seq");
        conf.connect(seq_id(i),   Sequencer::PORT_EVENTS_OUT,
                     samp_id(i),  Sampler::PORT_EVENTS_IN).expect("seq→samp");
        conf.connect(samp_id(i),  Sampler::PORT_AUDIO_OUT_L,
                     dist_id(i),  DistortionNode::PORT_AUDIO_IN).expect("samp→dist");
        conf.connect(dist_id(i),  DistortionNode::PORT_AUDIO_OUT,
                     filt_id(i),  FilterNode::PORT_AUDIO_IN).expect("dist→filt");
        conf.connect(filt_id(i),  FilterNode::PORT_AUDIO_OUT,
                     ID_MIX,      i as u32).expect("filt→mix");
    }

    // ── Hardware: one ScriptingGateway per device ─────────────────────────────
    // Each gateway knows its device_id at construction — no multi-port fan-in,
    // so events are always tagged with the correct source.
    let launchpad_id = try_open_launchpad(&mut conf);
    let digitakt_id  = try_open_digitakt(&mut conf);
    let keystep_id   = try_open_keystep(&mut conf);

    // Launchpad (or emulator) gateway — always created.
    let lp_dev_id = launchpad_id.unwrap_or(ID_EMULATOR);
    let (gw_lp, mut consumer_lp) = ScriptingGatewayNode::new(lp_dev_id, 512);
    conf.add_node(ID_GW_LP, Box::new(gw_lp));
    conf.connect(lp_dev_id, 0, ID_GW_LP, 0).ok();

    // Digitakt gateway — only if connected.
    let mut consumer_dt: Option<ScriptEventConsumer> = None;
    if let Some(did) = digitakt_id {
        let (gw_dt, cons) = ScriptingGatewayNode::new(did, 256);
        conf.add_node(ID_GW_DT, Box::new(gw_dt));
        conf.connect(did, 0, ID_GW_DT, 0).ok();
        consumer_dt = Some(cons);
    }

    // Keystep gateway — only if connected; also routes notes to mapper.
    let mut consumer_ks: Option<ScriptEventConsumer> = None;
    if let Some(kid) = keystep_id {
        conf.add_node(ID_MAPPER, Box::new(HardwareMappingNode::default_chromatic(0)));
        conf.connect(kid, 0, ID_MAPPER, 0).ok();
        conf.connect(ID_MAPPER, 1, seq_id(7), Sequencer::PORT_EVENTS_IN).ok();
        let (gw_ks, cons) = ScriptingGatewayNode::new(kid, 256);
        conf.add_node(ID_GW_KS, Box::new(gw_ks));
        conf.connect(kid, 0, ID_GW_KS, 0).ok();
        consumer_ks = Some(cons);
    }

    // ── L4: scripting sandbox ─────────────────────────────────────────────────
    let bus_handle = conf.state_bus_handle();
    let mut scripting = ScriptingEngine::new();
    scripting.bind_state_bus(bus_handle);

    // Build constants for profile injection
    let track_seq_ids:  Vec<rhai::Dynamic> = (0..NUM_TRACKS).map(|i| rhai::Dynamic::from(seq_id(i) as i64)).collect();
    let track_samp_ids: Vec<rhai::Dynamic> = (0..NUM_TRACKS).map(|i| rhai::Dynamic::from(samp_id(i) as i64)).collect();
    let track_dist_ids: Vec<rhai::Dynamic> = (0..NUM_TRACKS).map(|i| rhai::Dynamic::from(dist_id(i) as i64)).collect();
    let track_filt_ids: Vec<rhai::Dynamic> = (0..NUM_TRACKS).map(|i| rhai::Dynamic::from(filt_id(i) as i64)).collect();

    let constants: Vec<(String, rhai::Dynamic)> = vec![
        ("LP_DEVICE_ID".into(),    rhai::Dynamic::from(launchpad_id.unwrap_or(ID_EMULATOR) as i64)),
        ("DT_DEVICE_ID".into(),    rhai::Dynamic::from(digitakt_id.unwrap_or(0) as i64)),
        ("KS_DEVICE_ID".into(),    rhai::Dynamic::from(keystep_id.unwrap_or(0) as i64)),
        ("CLOCK_ID".into(),        rhai::Dynamic::from(ID_CLOCK as i64)),
        ("TRACK_SEQ_IDS".into(),   rhai::Dynamic::from(track_seq_ids)),
        ("TRACK_SAMP_IDS".into(),  rhai::Dynamic::from(track_samp_ids)),
        ("TRACK_DIST_IDS".into(),  rhai::Dynamic::from(track_dist_ids)),
        ("TRACK_FILT_IDS".into(),  rhai::Dynamic::from(track_filt_ids)),
    ];

    for profile in &["launchpad", "digitakt", "keystep"] {
        let path = format!("profiles/{profile}.rhai");
        if std::path::Path::new(&path).exists() {
            if let Err(e) = scripting.eval_file(profile, &path, &constants) {
                eprintln!("[paraclete] profile {profile} error: {e}");
            } else {
                eprintln!("[paraclete] profile {profile} loaded");
            }
        }
    }

    // ── Build executor and start audio ────────────────────────────────────────
    let executor = conf.build_executor();
    eprintln!("[paraclete] graph built — {NUM_TRACKS} tracks at {BPM} BPM");

    let _audio = match AudioBackend::start(executor) {
        Ok(b) => {
            eprintln!("[paraclete] audio running — Esc or Ctrl-C to stop");
            b
        }
        Err(e) => {
            eprintln!("[paraclete] audio backend error: {e}");
            std::process::exit(1);
        }
    };

    // ── Graceful shutdown handler ─────────────────────────────────────────────
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = std::sync::Arc::clone(&running);
    ctrlc::set_handler(move || {
        eprintln!("[paraclete] shutting down...");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    }).ok();

    // ── Main loop ─────────────────────────────────────────────────────────────
    let mut event_buf: Vec<paraclete_node_api::HardwareEventMsg> = Vec::with_capacity(64);
    let mut dev_ui_tick = 0u64;

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(1));

        // 1. Drain state bus + tick hardware output handles.
        conf.process_main_thread();

        // 2. Drain all ScriptingGatewayNode SPSCs (one per connected device).
        event_buf.clear();
        consumer_lp.drain(&mut event_buf);
        if let Some(ref mut c) = consumer_dt { c.drain(&mut event_buf); }
        if let Some(ref mut c) = consumer_ks { c.drain(&mut event_buf); }

        // 3. Dispatch hardware events → Rhai handlers.
        for ev in &event_buf {
            scripting.dispatch_hardware_event(ev);
        }

        // 4. Fire subscription callbacks for changed state bus values.
        if let Ok(bus) = conf.state_bus_handle().try_borrow() {
            scripting.process_subscriptions(&*bus);
        }

        // 5. Flush NodeCommands produced by scripts.
        for cmd in scripting.take_pending_commands() {
            conf.send_command(cmd).ok();
        }

        // 5b. Deliver LED output from scripts to hardware devices.
        let led_output = scripting.take_pending_output();
        if !led_output.is_empty() {
            conf.deliver_script_output(led_output);
        }

        // 6. Dev UI — dump state bus to stderr periodically.
        if dev_ui {
            dev_ui_tick += 1;
            if dev_ui_tick % 1000 == 0 {
                // Show all 8 tracks: current step + steps bitfield on one line each.
                for i in 0..NUM_TRACKS {
                    let step_path  = format!("/node/{}/state/current_step", seq_id(i));
                    let steps_path = format!("/node/{}/state/steps",        seq_id(i));
                    let step  = conf.state_bus_read(&step_path);
                    let steps = conf.state_bus_read(&steps_path);
                    eprintln!("[dev-ui] {:7} step={:?} pattern={:?}",
                        TRACKS[i].name, step, steps);
                }
            }
        }
    }

    eprintln!("[paraclete] stopped.");
    // LP cleanup happens automatically via LaunchpadOutputHandle::drop().
}

fn try_open_launchpad(conf: &mut NodeConfigurator) -> Option<u32> {
    match LaunchpadNode::open() {
        Ok(node) => {
            conf.add_hardware_device(ID_LAUNCHPAD, Box::new(node));
            eprintln!("[paraclete] Launchpad MK2 connected");
            Some(ID_LAUNCHPAD)
        }
        Err(e) => {
            eprintln!("[paraclete] Launchpad not found ({e}), using terminal emulator");
            conf.add_hardware_device(ID_EMULATOR, Box::new(LaunchpadEmulator::new()));
            Some(ID_EMULATOR)
        }
    }
}

fn try_open_digitakt(conf: &mut NodeConfigurator) -> Option<u32> {
    match DigitaktMidiNode::open() {
        Ok(node) => {
            conf.add_hardware_device(ID_DIGITAKT, Box::new(node));
            eprintln!("[paraclete] Digitakt connected");
            Some(ID_DIGITAKT)
        }
        Err(e) => {
            eprintln!("[paraclete] Digitakt not found ({e}), skipping");
            None
        }
    }
}

fn try_open_keystep(conf: &mut NodeConfigurator) -> Option<u32> {
    match KeystepNode::open() {
        Ok(node) => {
            conf.add_hardware_device(ID_KEYSTEP, Box::new(node));
            eprintln!("[paraclete] Keystep 37 connected");
            Some(ID_KEYSTEP)
        }
        Err(e) => {
            eprintln!("[paraclete] Keystep not found ({e}), skipping");
            None
        }
    }
}
