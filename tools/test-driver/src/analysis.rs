//! Audio artifact analysis for post-capture assertions (INFRA-001).
//!
//! Pure scans over the captured mono buffer. Each returns the worst
//! offender's location so a failure message can point at the artifact.

/// Largest absolute difference between adjacent samples.
/// Returns (max |diff|, index of the later sample of the worst pair).
pub fn max_discontinuity(samples: &[f32]) -> (f32, usize) {
    let mut max_jump = 0.0f32;
    let mut max_idx = 0usize;
    for i in 1..samples.len() {
        let jump = (samples[i] - samples[i - 1]).abs();
        if jump > max_jump {
            max_jump = jump;
            max_idx = i;
        }
    }
    (max_jump, max_idx)
}

/// Mean of the window — a clean signal centres on zero.
pub fn dc_offset(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| s as f64).sum();
    (sum / samples.len() as f64) as f32
}

/// NaN/Inf samples defeat ordered comparisons (NaN >= limit is false), so
/// every artifact scan is preceded by this explicit check.
/// Returns (non-finite count, index of first offender).
pub fn non_finite(samples: &[f32]) -> (usize, usize) {
    let mut count = 0usize;
    let mut first = 0usize;
    for (i, s) in samples.iter().enumerate() {
        if !s.is_finite() {
            if count == 0 {
                first = i;
            }
            count += 1;
        }
    }
    (count, first)
}

/// Root-mean-square of the window — overall energy. Empty → 0.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// RMS per fixed-size window — the envelope shape. A trailing partial window is
/// included (it still carries signal). `window` of 0 yields an empty envelope.
pub fn rms_windows(samples: &[f32], window: usize) -> Vec<f32> {
    if window == 0 {
        return Vec::new();
    }
    samples.chunks(window).map(rms).collect()
}

/// Longest run of bitwise-identical consecutive samples (held value or
/// zeros — the dropout signature). Returns (run length, start index).
/// A run of intended silence counts; scope the assertion window past it.
pub fn longest_hold_run(samples: &[f32]) -> (usize, usize) {
    let mut best_len = 0usize;
    let mut best_start = 0usize;
    let mut run_start = 0usize;
    for i in 1..=samples.len() {
        if i == samples.len() || samples[i].to_bits() != samples[run_start].to_bits() {
            let len = i - run_start;
            if len > best_len {
                best_len = len;
                best_start = run_start;
            }
            run_start = i;
        }
    }
    (best_len, best_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discontinuity_finds_worst_jump() {
        let s = [0.0, 0.1, 0.2, -0.6, -0.5];
        let (jump, idx) = max_discontinuity(&s);
        assert!((jump - 0.8).abs() < 1e-6);
        assert_eq!(idx, 3);
    }

    #[test]
    fn discontinuity_of_smooth_ramp_is_step_size() {
        let s: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
        let (jump, _) = max_discontinuity(&s);
        assert!((jump - 0.01).abs() < 1e-6);
    }

    #[test]
    fn discontinuity_empty_and_single_are_zero() {
        assert_eq!(max_discontinuity(&[]).0, 0.0);
        assert_eq!(max_discontinuity(&[0.5]).0, 0.0);
    }

    #[test]
    fn dc_offset_of_centred_signal_is_zero() {
        let s = [0.5, -0.5, 0.25, -0.25];
        assert!(dc_offset(&s).abs() < 1e-6);
    }

    #[test]
    fn dc_offset_of_biased_signal_is_bias() {
        let s = [0.6, 0.4, 0.6, 0.4];
        assert!((dc_offset(&s) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hold_run_finds_longest_repeat() {
        let s = [0.1, 0.3, 0.3, 0.3, 0.2, 0.2];
        let (len, start) = longest_hold_run(&s);
        assert_eq!(len, 3);
        assert_eq!(start, 1);
    }

    #[test]
    fn hold_run_counts_zeros_as_dropout() {
        let s = [0.1, 0.0, 0.0, 0.0, 0.0, 0.1];
        let (len, start) = longest_hold_run(&s);
        assert_eq!(len, 4);
        assert_eq!(start, 1);
    }

    #[test]
    fn hold_run_of_varying_signal_is_one() {
        let s = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(longest_hold_run(&s).0, 1);
    }

    #[test]
    fn non_finite_counts_nan_and_inf() {
        let s = [0.1, f32::NAN, 0.2, f32::INFINITY, f32::NEG_INFINITY];
        let (count, first) = non_finite(&s);
        assert_eq!(count, 3);
        assert_eq!(first, 1);
    }

    #[test]
    fn non_finite_of_clean_signal_is_zero() {
        let s = [0.1, -0.2, 0.0, 1.0];
        assert_eq!(non_finite(&s).0, 0);
    }

    #[test]
    fn rms_of_constant_is_the_constant() {
        assert!((rms(&[0.5, 0.5, 0.5, 0.5]) - 0.5).abs() < 1e-6);
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_of_unit_square_wave_is_one() {
        let s = [1.0, -1.0, 1.0, -1.0];
        assert!((rms(&s) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rms_windows_chunks_including_partial_tail() {
        // 5 samples, window 2 → [win(0,1), win(2,3), win(4)]
        let s = [1.0, -1.0, 0.5, -0.5, 1.0];
        let env = rms_windows(&s, 2);
        assert_eq!(env.len(), 3);
        assert!((env[0] - 1.0).abs() < 1e-6);
        assert!((env[1] - 0.5).abs() < 1e-6);
        assert!((env[2] - 1.0).abs() < 1e-6); // partial tail
        assert!(rms_windows(&s, 0).is_empty());
    }

    #[test]
    fn nan_is_invisible_to_ordered_scans() {
        // Documents why non_finite() must run first: NaN defeats the
        // ordered comparisons inside the other scans.
        let s = [0.0, f32::NAN, 0.0];
        assert_eq!(max_discontinuity(&s).0, 0.0);
    }
}
