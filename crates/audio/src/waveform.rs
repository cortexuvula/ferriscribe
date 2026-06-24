//! Audio signal analysis utilities.
//!
//! Provides functions for computing RMS level, peak amplitude, decibel
//! conversion, and peak normalization of f32 sample buffers. Used by the
//! capture pipeline and frontend for VU metering and waveform display.

/// Compute the Root Mean Square of the sample buffer.
///
/// Returns `0.0` for an empty slice.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Return the maximum absolute value in the sample buffer.
///
/// Returns `0.0` for an empty slice.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max)
}

/// Convert a linear amplitude to decibels (dB).
///
/// Returns `f32::NEG_INFINITY` when `amplitude` is `0.0`.
pub fn amplitude_to_db(amplitude: f32) -> f32 {
    if amplitude == 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amplitude.abs().log10()
    }
}

/// Scale `samples` so that the peak absolute value equals `1.0`.
///
/// If all samples are `0.0` (silence) the slice is returned unchanged.
pub fn normalize(samples: &[f32]) -> Vec<f32> {
    let p = peak(samples);
    if p == 0.0 {
        return samples.to_vec();
    }
    samples.iter().map(|s| s / p).collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_silence() {
        assert_eq!(rms(&[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn rms_constant() {
        // RMS of a constant signal equals its absolute value.
        let samples = vec![0.5f32; 100];
        assert!((rms(&samples) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rms_empty() {
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn peak_samples() {
        let samples = [0.1f32, -0.9, 0.5, -0.2];
        assert!((peak(&samples) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn peak_empty() {
        assert_eq!(peak(&[]), 0.0);
    }

    #[test]
    fn db_unity() {
        // 20 * log10(1.0) = 0 dB
        assert!((amplitude_to_db(1.0) - 0.0).abs() < 1e-5);
    }

    #[test]
    fn db_half() {
        // 20 * log10(0.5) ≈ -6.0206 dB
        let db = amplitude_to_db(0.5);
        assert!((db - (-6.0206)).abs() < 0.001, "got {db}");
    }

    #[test]
    fn db_zero() {
        assert_eq!(amplitude_to_db(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn normalize_scales() {
        let samples = vec![0.0f32, 0.5, -1.0, 0.25];
        let norm = normalize(&samples);
        assert!((peak(&norm) - 1.0).abs() < 1e-6);
        // Values should be proportionally scaled.
        assert!((norm[2] - (-1.0)).abs() < 1e-6);
        assert!((norm[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn normalize_silence() {
        let samples = vec![0.0f32, 0.0, 0.0];
        let norm = normalize(&samples);
        assert_eq!(norm, samples);
    }
}
