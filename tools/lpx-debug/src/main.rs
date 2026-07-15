// SPDX-License-Identifier: GPL-3.0-or-later
//! Launchpad X hardware debug tool — automated state probing.
//!
//! Runs all steps automatically with timed pauses. Probes LP state via
//! SysEx readback at each transition so the agent can read /tmp/lpx-debug.log
//! and detect exactly when mode changes occur.
//!
//! Usage: cargo run -p lpx-debug

use std::io::Write as IoWrite;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Log ───────────────────────────────────────────────────────────────────────

fn ts(start: Instant) -> f64 {
    start.elapsed().as_secs_f64()
}

fn log(f: &Arc<Mutex<std::fs::File>>, start: Instant, tag: &str, msg: &str) {
    let line = format!("[{:6.2}s] {:12} {}\n", ts(start), tag, msg);
    if let Ok(mut g) = f.lock() {
        g.write_all(line.as_bytes()).ok();
        g.flush().ok();
    }
    eprint!("{}", line);
}

// ── MIDI helpers ──────────────────────────────────────────────────────────────

fn find_out(o: &midir::MidiOutput, s: &str) -> Option<midir::MidiOutputPort> {
    o.ports()
        .into_iter()
        .find(|p| o.port_name(p).map(|n| n.contains(s)).unwrap_or(false))
}
fn find_in(i: &midir::MidiInput, s: &str) -> Option<midir::MidiInputPort> {
    i.ports()
        .into_iter()
        .find(|p| i.port_name(p).map(|n| n.contains(s)).unwrap_or(false))
}

fn layout_name(b: u8) -> &'static str {
    match b {
        0x00 => "Session(DAW-only)",
        0x01 => "NoteMode",
        0x04..=0x07 => "CustomMode",
        0x0D => "DAWFaders",
        0x7F => "PROGRAMMER",
        _ => "Unknown",
    }
}
fn mode_name(b: u8) -> &'static str {
    if b == 1 {
        "Programmer"
    } else {
        "Live"
    }
}

const HDR: [u8; 6] = [0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C];

/// Send SysEx readback queries, wait up to timeout_ms, log results.
fn probe(
    out: &mut midir::MidiOutputConnection,
    inbox: &Arc<Mutex<Vec<Vec<u8>>>>,
    log_f: &Arc<Mutex<std::fs::File>>,
    start: Instant,
    label: &str,
) {
    let mut collect = |cmd: u8| -> Option<u8> {
        inbox.lock().unwrap().clear();
        let _ = out.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, cmd, 0xF7]);
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            if let Ok(msgs) = inbox.lock() {
                for m in msgs.iter() {
                    if m.len() == 9 && m[..6] == HDR && m[6] == cmd && m[8] == 0xF7 {
                        return Some(m[7]);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    };

    let layout = collect(0x00);
    std::thread::sleep(Duration::from_millis(20));
    let mode = collect(0x0E);

    let ls = layout
        .map(|b| format!("{:#04X}={}", b, layout_name(b)))
        .unwrap_or_else(|| "TIMEOUT".into());
    let ms = mode
        .map(|b| format!("{}={}", b, mode_name(b)))
        .unwrap_or_else(|| "TIMEOUT".into());

    log(
        log_f,
        start,
        "PROBE",
        &format!("{label:30} layout={ls:25} mode={ms}"),
    );
}

fn wait(ms: u64, running: &Arc<AtomicBool>) -> bool {
    let n = ms / 50;
    for _ in 0..n {
        if !running.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    running.load(Ordering::SeqCst)
}

fn cleanup(
    out: &mut midir::MidiOutputConnection,
    log_f: &Arc<Mutex<std::fs::File>>,
    start: Instant,
) {
    log(
        log_f,
        start,
        "CLEANUP",
        "reverting LP → Standalone + NoteMode",
    );
    let _ = out.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x10, 0x00, 0xF7]);
    std::thread::sleep(Duration::from_millis(40));
    let _ = out.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x00, 0x01, 0xF7]);
    log(log_f, start, "CLEANUP", "done");
}

fn main() {
    let start = Instant::now();
    let log_f: Arc<Mutex<std::fs::File>> = Arc::new(Mutex::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open("/tmp/lpx-debug.log")
            .unwrap(),
    ));
    log(
        &log_f,
        start,
        "INIT",
        "lpx-debug starting — log: /tmp/lpx-debug.log",
    );

    let running = Arc::new(AtomicBool::new(true));
    let r = Arc::clone(&running);
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .ok();

    // ── Step 0: Reset via DAW port ────────────────────────────────────────────
    log(
        &log_f,
        start,
        "STEP-0",
        "reset: DAW→Standalone + NoteMode via DAW port",
    );
    {
        let o = midir::MidiOutput::new("r").unwrap();
        if let Some(p) = find_out(&o, "LPX DAW").or_else(|| find_out(&o, "Launchpad")) {
            if let Ok(mut c) = o.connect(&p, "r") {
                let _ = c.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x10, 0x00, 0xF7]);
                std::thread::sleep(Duration::from_millis(50));
                let _ = c.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x00, 0x01, 0xF7]);
            }
        }
    }
    if !wait(500, &running) {
        return;
    }

    // ── Open MIDI I/O ─────────────────────────────────────────────────────────
    log(&log_f, start, "STEP-1", "opening MIDI output");
    let o = midir::MidiOutput::new("lpx-out").unwrap();
    let op = find_out(&o, "LPX MIDI")
        .or_else(|| find_out(&o, "Launchpad"))
        .unwrap();
    log(
        &log_f,
        start,
        "STEP-1",
        &format!("out: {:?}", o.port_name(&op)),
    );
    let mut conn_out = o.connect(&op, "o").unwrap();

    let inbox: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let ib = Arc::clone(&inbox);
    let lf2 = Arc::clone(&log_f);
    let start2 = start;
    log(&log_f, start, "STEP-1", "opening MIDI input");
    let i = midir::MidiInput::new("lpx-in").unwrap();
    let ip = find_in(&i, "LPX MIDI")
        .or_else(|| find_in(&i, "Launchpad"))
        .unwrap();
    log(
        &log_f,
        start,
        "STEP-1",
        &format!("in: {:?}", i.port_name(&ip)),
    );
    let _ci = i
        .connect(
            &ip,
            "i",
            move |_, b, _| {
                if b.is_empty() || b[0] == 0xF8 {
                    return;
                }
                let h = b
                    .iter()
                    .map(|x| format!("{x:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                log(&lf2, start2, "LP-IN", &h);
                ib.lock().unwrap().push(b.to_vec());
            },
            (),
        )
        .unwrap();

    if !wait(200, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }
    probe(
        &mut conn_out,
        &inbox,
        &log_f,
        start,
        "after-connections-open",
    );
    if !wait(2000, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }

    // ── Programmer Mode SysEx ─────────────────────────────────────────────────
    log(&log_f, start, "STEP-1", "sending Programmer Mode SysEx");
    let _ = conn_out.send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x00, 0x7F, 0xF7]);
    if !wait(200, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }
    probe(
        &mut conn_out,
        &inbox,
        &log_f,
        start,
        "after-programmer-sysex",
    );
    if !wait(500, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }

    // ── Note On: red pad ──────────────────────────────────────────────────────
    log(&log_f, start, "STEP-1", "note 81 vel=5 (top-left red)");
    let _ = conn_out.send(&[0x90, 81, 5]);
    if !wait(200, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }
    probe(&mut conn_out, &inbox, &log_f, start, "after-note-on");
    log(
        &log_f,
        start,
        "STEP-1",
        "observing 3s (expect: red pad, no march)",
    );
    if !wait(3000, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }

    // ── Step 2: cpal audio ────────────────────────────────────────────────────
    log(&log_f, start, "STEP-2", "starting cpal audio backend");
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = host.default_output_device().unwrap();
    log(
        &log_f,
        start,
        "STEP-2",
        &format!("device: {:?}", device.name()),
    );
    let cfg = device.default_output_config().unwrap();
    log(&log_f, start, "STEP-2", &format!("config: {cfg:?}"));
    let stream = device
        .build_output_stream(
            &cfg.into(),
            |d: &mut [f32], _| {
                d.fill(0.0);
            },
            |e| eprintln!("audio err: {e}"),
            None,
        )
        .unwrap();
    stream.play().unwrap();
    log(&log_f, start, "STEP-2", "audio stream playing");
    if !wait(100, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }
    probe(
        &mut conn_out,
        &inbox,
        &log_f,
        start,
        "immediately-after-audio-start",
    );
    log(
        &log_f,
        start,
        "STEP-2",
        "observing 5s (does audio trigger march?)",
    );
    if !wait(5000, &running) {
        cleanup(&mut conn_out, &log_f, start);
        return;
    }
    probe(&mut conn_out, &inbox, &log_f, start, "after-audio-5s");

    // ── Step 3: 1ms loop ──────────────────────────────────────────────────────
    log(&log_f, start, "STEP-3", "running 1ms loop for 8s");
    let t = Instant::now();
    while running.load(Ordering::SeqCst) && t.elapsed().as_secs() < 8 {
        std::thread::sleep(Duration::from_millis(1));
    }
    probe(&mut conn_out, &inbox, &log_f, start, "after-8s-loop");

    cleanup(&mut conn_out, &log_f, start);
}
