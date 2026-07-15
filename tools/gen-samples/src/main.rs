// SPDX-License-Identifier: GPL-3.0-or-later
//! Generate synthesized drum samples for Paraclete development and testing.
//!
//! Writes samples/track0.wav through samples/track7.wav in the current directory.
//! All files: mono, 44100 Hz, 16-bit PCM.
//!
//! Usage: cargo run -p gen-samples

use std::f64::consts::TAU;
use std::path::Path;

const SR: u32 = 44_100;
const SPEC: hound::WavSpec = hound::WavSpec {
    channels: 1,
    sample_rate: SR,
    bits_per_sample: 16,
    sample_format: hound::SampleFormat::Int,
};

// ── XorShift32 PRNG ──────────────────────────────────────────────────────────

fn xorshift(state: &mut u32) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as f64) / (u32::MAX as f64) * 2.0 - 1.0
}

// ── Sample helpers ────────────────────────────────────────────────────────────

fn to_i16(x: f64) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0) as i16
}

fn frames(ms: u32) -> usize {
    (SR as usize * ms as usize) / 1000
}

fn write_wav(path: &str, samples: &[f64]) {
    let mut writer = hound::WavWriter::create(path, SPEC)
        .unwrap_or_else(|e| panic!("cannot create {path}: {e}"));
    for &s in samples {
        writer.write_sample(to_i16(s)).unwrap();
    }
    writer.finalize().unwrap();
    println!(
        "  wrote {path}  ({} ms)",
        samples.len() * 1000 / SR as usize
    );
}

// ── Synthesis recipes ─────────────────────────────────────────────────────────

/// Kick drum: pitch sweep 120→40 Hz + exponential amplitude decay.
fn kick() -> Vec<f64> {
    let n = frames(600);
    let mut buf = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let sweep = (t * 18.0).exp(); // pitch falls fast at first
        let freq = 40.0 + 80.0 / sweep;
        let amp = (-7.0 * t).exp();
        phase = (phase + TAU * freq / SR as f64) % TAU;
        buf.push(amp * phase.sin() * 0.95);
    }
    buf
}

/// Snare: white noise + body resonance at 200 Hz.
fn snare() -> Vec<f64> {
    let n = frames(350);
    let mut buf = Vec::with_capacity(n);
    let mut rng: u32 = 0x5EED_BEEF;
    let mut phase = 0.0f64;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let amp = (-11.0 * t).exp();
        let noise = xorshift(&mut rng);
        phase = (phase + TAU * 200.0 / SR as f64) % TAU;
        let body = phase.sin();
        buf.push(amp * (0.6 * noise + 0.4 * body));
    }
    buf
}

/// Closed hi-hat: short burst of filtered noise.
fn hat_closed() -> Vec<f64> {
    let n = frames(90);
    let mut buf = Vec::with_capacity(n);
    let mut rng: u32 = 0xDEAD_1234;
    let mut hp = 0.0f64; // one-pole high-pass state
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let amp = (-55.0 * t).exp();
        let raw = xorshift(&mut rng);
        hp = 0.9 * hp + raw - { raw }; // simple high-pass
        let noise = xorshift(&mut rng); // re-draw for true noise floor
        buf.push(amp * noise * 0.5);
    }
    buf
}

/// Open hi-hat: longer version of the same noise.
fn hat_open() -> Vec<f64> {
    let n = frames(380);
    let mut buf = Vec::with_capacity(n);
    let mut rng: u32 = 0xC0FF_EE42;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let amp = (-8.0 * t).exp();
        let noise = xorshift(&mut rng);
        buf.push(amp * noise * 0.45);
    }
    buf
}

/// Perc A: high woodblock click — 900 Hz with very fast decay.
fn perc_a() -> Vec<f64> {
    let n = frames(180);
    let mut buf = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    let mut rng: u32 = 0xABCD_1111;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let amp = (-25.0 * t).exp();
        let click_amp = (-120.0 * t).exp(); // transient click at onset
        phase = (phase + TAU * 900.0 / SR as f64) % TAU;
        let click = xorshift(&mut rng) * click_amp;
        buf.push(amp * phase.sin() * 0.6 + click * 0.4);
    }
    buf
}

/// Perc B: mid conga-like tone at 320 Hz.
fn perc_b() -> Vec<f64> {
    let n = frames(280);
    let mut buf = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    let mut rng: u32 = 0x1234_5678;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let amp = (-14.0 * t).exp();
        let click_amp = (-80.0 * t).exp();
        let freq = 320.0 + 80.0 / (1.0 + t * 40.0); // slight pitch drop
        phase = (phase + TAU * freq / SR as f64) % TAU;
        let click = xorshift(&mut rng) * click_amp;
        buf.push(amp * phase.sin() * 0.7 + click * 0.3);
    }
    buf
}

/// FX: descending metallic sweep — combo of two detuned tones with fast decay.
fn fx() -> Vec<f64> {
    let n = frames(500);
    let mut buf = Vec::with_capacity(n);
    let mut ph1 = 0.0f64;
    let mut ph2 = 0.0f64;
    let mut rng: u32 = 0xF00D_CAFE;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let amp = (-6.0 * t).exp();
        let f1 = 800.0 * (-8.0 * t).exp() + 100.0;
        let f2 = f1 * 1.41; // tritone detuning for metallic flavour
        ph1 = (ph1 + TAU * f1 / SR as f64) % TAU;
        ph2 = (ph2 + TAU * f2 / SR as f64) % TAU;
        let noise_layer = xorshift(&mut rng) * (-20.0 * t).exp() * 0.2;
        buf.push(amp * (0.45 * ph1.sin() + 0.35 * ph2.sin() + noise_layer));
    }
    buf
}

/// Bass: deep sub tone at 60 Hz — Sampler will pitch-shift this per note.
/// Long sustain so the Sampler's full note length plays out cleanly.
fn bass() -> Vec<f64> {
    let n = frames(900);
    let mut buf = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        // Small attack ramp + flat sustain + tail release
        let env = if t < 0.008 {
            t / 0.008
        } else if t < 0.7 {
            1.0
        } else {
            ((0.9 - t) / 0.2).clamp(0.0, 1.0)
        };
        phase = (phase + TAU * 60.0 / SR as f64) % TAU;
        // Slight harmonic saturation for warmth
        let s = phase.sin();
        buf.push(env * (0.8 * s + 0.15 * (2.0 * phase).sin() + 0.05 * (3.0 * phase).sin()) * 0.9);
    }
    buf
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "samples".into());
    let dir = Path::new(&out_dir);
    std::fs::create_dir_all(dir).unwrap();

    let tracks: &[(&str, fn() -> Vec<f64>)] = &[
        ("track0.wav", kick),
        ("track1.wav", snare),
        ("track2.wav", hat_closed),
        ("track3.wav", hat_open),
        ("track4.wav", perc_a),
        ("track5.wav", perc_b),
        ("track6.wav", fx),
        ("track7.wav", bass),
    ];

    println!("Generating {} samples in {}/", tracks.len(), out_dir);
    for (name, gen) in tracks {
        let path = dir.join(name);
        write_wav(path.to_str().unwrap(), &gen());
    }
    println!("Done. Run `cargo run` to start Paraclete with these samples.");
}
