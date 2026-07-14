//! Audio regression baselines (ADR-035 Part A).
//!
//! A baseline is a *derived fingerprint* of a scenario's rendered audio — a
//! scalar summary plus a windowed-RMS envelope — not the raw samples. Raw
//! equality would break on any intended DSP change; the fingerprint is
//! refactor-tolerant but catches regressions (a voice going silent, a decay
//! doubling, DC creeping in). Stored beside the scenario as JSON, diffed on a
//! `--check-baseline` run within per-metric tolerances.

use serde::{Deserialize, Serialize};

use crate::analysis;

/// Envelope window. 50 ms is fine-grained enough to locate a decay/timing shift,
/// coarse enough to stay stable across the ~25% debug-build capture slowdown.
pub const ENVELOPE_WINDOW_MS: f64 = 50.0;

/// Below this magnitude a window/scalar is treated as effectively silent, so
/// near-zero baselines don't make tiny nonzero currents look "infinitely" off.
const SILENCE: f32 = 1e-3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fingerprint {
    pub sample_count: usize,
    pub peak: f32,
    pub rms: f32,
    pub dc_offset: f32,
    pub non_finite: usize,
    pub envelope_window_ms: f64,
    pub rms_envelope: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tolerances {
    /// Relative tolerance on `peak` (fraction of baseline magnitude).
    pub peak_rel: f64,
    /// Relative tolerance on overall `rms`.
    pub rms_rel: f64,
    /// Relative tolerance on each envelope window.
    pub envelope_rel: f64,
    /// Absolute tolerance on `dc_offset`.
    pub dc_offset_abs: f64,
    /// `sample_count` may differ by this many blocks (× block_size samples).
    pub sample_count_blocks: usize,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            peak_rel: 0.01,
            rms_rel: 0.01,
            envelope_rel: 0.015,
            dc_offset_abs: 1e-3,
            sample_count_blocks: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub fingerprint: Fingerprint,
    #[serde(default)]
    pub tolerances: Tolerances,
}

pub fn compute_fingerprint(samples: &[f32], sample_rate: f32) -> Fingerprint {
    let window = (((ENVELOPE_WINDOW_MS / 1000.0) * sample_rate as f64) as usize).max(1);
    // f32::max ignores NaN, so peak is unaffected by non-finite samples (which
    // are counted separately and must be zero).
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    Fingerprint {
        sample_count: samples.len(),
        peak,
        rms: analysis::rms(samples),
        dc_offset: analysis::dc_offset(samples),
        non_finite: analysis::non_finite(samples).0,
        envelope_window_ms: ENVELOPE_WINDOW_MS,
        rms_envelope: analysis::rms_windows(samples, window),
    }
}

/// Compare a fresh fingerprint against the baseline. Returns human-readable
/// drift descriptions, most-structural first; empty = within tolerance.
pub fn compare(baseline: &Baseline, current: &Fingerprint, block_size: usize) -> Vec<String> {
    let t = &baseline.tolerances;
    let b = &baseline.fingerprint;
    let mut drift = Vec::new();

    // A regression that introduces NaN/Inf is always a failure, tolerance or not.
    if current.non_finite > 0 {
        drift.push(format!("non-finite samples: {} (must be 0)", current.non_finite));
    }

    let count_tol = t.sample_count_blocks.saturating_mul(block_size);
    if current.sample_count.abs_diff(b.sample_count) > count_tol {
        drift.push(format!(
            "sample_count {} vs baseline {} (tol ±{})",
            current.sample_count, b.sample_count, count_tol
        ));
    }

    if let Some(d) = rel_drift("peak", current.peak, b.peak, t.peak_rel) {
        drift.push(d);
    }
    if let Some(d) = rel_drift("rms", current.rms, b.rms, t.rms_rel) {
        drift.push(d);
    }

    if (current.dc_offset - b.dc_offset).abs() as f64 > t.dc_offset_abs {
        drift.push(format!(
            "dc_offset {:.6} vs baseline {:.6} (tol ±{:.6})",
            current.dc_offset, b.dc_offset, t.dc_offset_abs
        ));
    }

    // Gross length drift is already reported by `sample_count`; a within-tolerance
    // count can still differ by a partial trailing window, so compare the common
    // prefix rather than flagging a length mismatch (which would double-count).
    let mut reported = 0;
    for (i, (&cur, &base)) in current.rms_envelope.iter().zip(&b.rms_envelope).enumerate() {
        if rel_exceeds(cur, base, t.envelope_rel) {
            let at_ms = i as f64 * b.envelope_window_ms;
            drift.push(format!(
                "envelope window {} (~{:.0}ms): {:.4} vs baseline {:.4} (>{:.1}%)",
                i, at_ms, cur, base, t.envelope_rel * 100.0
            ));
            reported += 1;
            if reported >= 6 {
                drift.push("… further envelope drift truncated".into());
                break;
            }
        }
    }
    drift
}

pub fn load(path: &str) -> Result<Baseline, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read baseline {}: {}", path, e))?;
    serde_json::from_str(&text).map_err(|e| format!("invalid baseline {}: {}", path, e))
}

pub fn save(path: &str, baseline: &Baseline) -> Result<(), String> {
    let text = serde_json::to_string_pretty(baseline)
        .map_err(|e| format!("cannot serialize baseline: {}", e))?;
    std::fs::write(path, text).map_err(|e| format!("cannot write baseline {}: {}", path, e))
}

/// True when `cur` differs from `base` by more than `rel` (relative to baseline
/// magnitude, floored so near-silent values don't blow up the ratio). Two
/// effectively-silent values never drift.
fn rel_exceeds(cur: f32, base: f32, rel: f64) -> bool {
    if cur.abs() < SILENCE && base.abs() < SILENCE {
        return false;
    }
    let denom = (base.abs() as f64).max(SILENCE as f64);
    ((cur - base).abs() as f64) / denom > rel
}

fn rel_drift(name: &str, cur: f32, base: f32, rel: f64) -> Option<String> {
    if rel_exceeds(cur, base, rel) {
        Some(format!("{} {:.4} vs baseline {:.4} (>{:.1}%)", name, cur, base, rel * 100.0))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline_from(samples: &[f32]) -> Baseline {
        Baseline {
            fingerprint: compute_fingerprint(samples, 1000.0),
            tolerances: Tolerances::default(),
        }
    }

    #[test]
    fn identical_render_has_no_drift() {
        let s: Vec<f32> = (0..500).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let base = baseline_from(&s);
        let cur = compute_fingerprint(&s, 1000.0);
        assert!(compare(&base, &cur, 64).is_empty());
    }

    #[test]
    fn halved_level_drifts_peak_rms_and_envelope() {
        let s: Vec<f32> = (0..500).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let quiet: Vec<f32> = s.iter().map(|x| x * 0.5).collect();
        let base = baseline_from(&s);
        let cur = compute_fingerprint(&quiet, 1000.0);
        let drift = compare(&base, &cur, 64);
        assert!(drift.iter().any(|d| d.starts_with("peak")), "{:?}", drift);
        assert!(drift.iter().any(|d| d.starts_with("rms")), "{:?}", drift);
        assert!(drift.iter().any(|d| d.contains("envelope window")), "{:?}", drift);
    }

    #[test]
    fn nan_is_always_drift() {
        let s: Vec<f32> = (0..500).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let base = baseline_from(&s);
        let mut bad = s.clone();
        bad[100] = f32::NAN;
        let cur = compute_fingerprint(&bad, 1000.0);
        let drift = compare(&base, &cur, 64);
        assert!(drift.iter().any(|d| d.contains("non-finite")), "{:?}", drift);
    }

    #[test]
    fn silent_render_matches_silent_baseline() {
        let s = vec![0.0f32; 500];
        let base = baseline_from(&s);
        let cur = compute_fingerprint(&vec![0.0f32; 500], 1000.0);
        assert!(compare(&base, &cur, 64).is_empty());
    }

    #[test]
    fn sample_count_within_one_block_is_ok_but_more_is_drift() {
        let s = vec![0.3f32; 500];
        let base = baseline_from(&s);
        let near = compute_fingerprint(&vec![0.3f32; 540], 1000.0); // +40, < 1 block (64)
        assert!(compare(&base, &near, 64).is_empty());
        let far = compute_fingerprint(&vec![0.3f32; 700], 1000.0); // +200, > 1 block
        assert!(compare(&base, &far, 64).iter().any(|d| d.starts_with("sample_count")));
    }

    #[test]
    fn roundtrip_serialization() {
        let s: Vec<f32> = (0..300).map(|i| (i as f32 * 0.2).sin() * 0.4).collect();
        let base = baseline_from(&s);
        let json = serde_json::to_string_pretty(&base).unwrap();
        let back: Baseline = serde_json::from_str(&json).unwrap();
        let cur = compute_fingerprint(&s, 1000.0);
        assert!(compare(&back, &cur, 64).is_empty());
    }
}
