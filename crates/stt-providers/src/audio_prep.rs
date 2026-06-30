//! Audio preprocessing: resample to 16 kHz mono for Whisper and pyannote.
//!
//! Both Whisper and pyannote expect 16 kHz mono audio. This module handles:
//! - **Resampling** via rubato polyphase sinc interpolation (Blackman-Harris window)
//! - **Downmixing** stereo/multi-channel to mono by averaging
//! - **Format conversion** between f32 PCM and i16 PCM
//! - **WAV encoding** for upload to remote Whisper servers

use medical_core::types::AudioData;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

const TARGET_RATE: u32 = 16_000;
const RESAMPLE_CHUNK: usize = 4096;

/// Resample audio to 16 kHz mono f32 using polyphase sinc interpolation
/// (rubato SincFixedIn with a Blackman-Harris windowed sinc).
///
/// Replaces the old linear-interpolation resampler: linear interpolation has
/// no anti-aliasing filter, so frequency content between 8 kHz and the source
/// Nyquist aliases back into the speech band, degrading consonant features
/// that Whisper relies on.
pub fn to_16k_mono_f32(audio: &AudioData) -> Vec<f32> {
    if audio.samples.is_empty() {
        return Vec::new();
    }
    let channels = audio.channels.max(1) as usize;
    let mono: Vec<f32> = if channels > 1 {
        audio
            .samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        audio.samples.clone()
    };
    if audio.sample_rate == TARGET_RATE {
        return mono;
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = TARGET_RATE as f64 / audio.sample_rate as f64;
    let mut resampler = match SincFixedIn::<f32>::new(ratio, 2.0, params, RESAMPLE_CHUNK, 1) {
        Ok(r) => r,
        Err(_) => {
            // Extremely unusual ratio (e.g. source_rate near zero); fall back
            // to returning the mono source unchanged rather than panicking.
            return mono;
        }
    };

    let expected_out = ((mono.len() as f64) * ratio).ceil() as usize + RESAMPLE_CHUNK;
    let mut out: Vec<f32> = Vec::with_capacity(expected_out);
    let mut in_buf = vec![vec![0.0_f32; RESAMPLE_CHUNK]];
    let mut pos = 0;
    while pos + RESAMPLE_CHUNK <= mono.len() {
        in_buf[0].copy_from_slice(&mono[pos..pos + RESAMPLE_CHUNK]);
        if let Ok(o) = resampler.process(&in_buf, None) {
            out.extend_from_slice(&o[0]);
        }
        pos += RESAMPLE_CHUNK;
    }
    // Zero-pad the final fractional chunk so the resampler flushes cleanly.
    if pos < mono.len() {
        let tail = mono.len() - pos;
        in_buf[0][..tail].copy_from_slice(&mono[pos..]);
        in_buf[0][tail..].fill(0.0);
        if let Ok(o) = resampler.process(&in_buf, None) {
            out.extend_from_slice(&o[0]);
        }
    }
    out
}

/// Convert f32 PCM samples to i16 (for diarization APIs that expect i16 input).
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s * 32_767.0).clamp(-32_768.0, 32_767.0) as i16)
        .collect()
}

/// Trim trailing silence from 16 kHz mono f32 audio.
///
/// Scans backward from the end of the buffer in 1-second windows
/// (16,000 samples). If a window's RMS energy is below the threshold,
/// it's considered silence and removed. The scan continues backward
/// until a window exceeds the threshold or the entire buffer is consumed
/// (in which case the original is returned unchanged).
///
/// This prevents whisper.cpp from hallucinating repetition loops on
/// the trailing silence after a conversation ends (the #1 cause of
/// "so so so" and "I see a counsellor..." ×100 patterns).
///
/// A minimum 0.5s tail pad is kept after the last audible window so
/// final words aren't cut off mid-syllable.
///
/// # Parameters
/// - `samples`: 16 kHz mono f32 PCM
/// - `threshold`: RMS energy threshold below which a window is silent.
///   0.01 ≈ -40 dBFS — catches room tone but preserves quiet speech.
pub fn trim_trailing_silence(samples: &[f32], threshold: f32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    const WINDOW: usize = 16_000; // 1 second at 16 kHz
    const TAIL_PAD: usize = 8_000; // 0.5 second pad

    let total = samples.len();
    if total <= WINDOW {
        return samples.to_vec();
    }

    // Scan backward from the end in 1-second windows.
    let mut last_audible_end = None;
    let mut pos = total;
    while pos >= WINDOW {
        let window = &samples[pos - WINDOW..pos];
        let rms = (window.iter().map(|s| s * s).sum::<f32>() / WINDOW as f32).sqrt();
        if rms >= threshold {
            last_audible_end = Some(pos);
            break;
        }
        pos -= WINDOW;
    }

    match last_audible_end {
        None => {
            // Entire audio is silent. Return a minimal stub.
            tracing::warn!("entire recording is silent; trimming to minimal stub");
            samples[..TAIL_PAD.min(total)].to_vec()
        }
        Some(end) if end == total => {
            // No trailing silence found.
            samples.to_vec()
        }
        Some(end) => {
            // Keep a 0.5s pad after the last audible content.
            let trimmed_end = (end + TAIL_PAD).min(total);
            let trimmed = samples[..trimmed_end].to_vec();
            tracing::info!(
                original_samples = total,
                trimmed_samples = trimmed.len(),
                removed_seconds = ((total - trimmed.len()) as f64 / 16_000.0 * 10.0).round() / 10.0,
                "trimmed trailing silence before transcription"
            );
            trimmed
        }
    }
}

/// Encode a 16 kHz mono PCM16 buffer as an in-memory WAV file.
///
/// Produces a RIFF/WAVE payload suitable for upload to any OpenAI-compatible
/// Whisper server. No extra heap allocations after the initial `Vec::with_capacity`.
pub fn write_pcm16_wav_bytes(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut buf = Vec::with_capacity(44 + data_len as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk (PCM, mono, 16-bit)
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // subchunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // 1 channel
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2; // sample_rate * channels * bits_per_sample/8
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }

    buf
}

#[cfg(test)]
mod wav_encode_tests {
    use super::*;

    #[test]
    fn encodes_header_and_data_length() {
        let samples = [0i16, 1, -1, 32767, -32768];
        let wav = write_pcm16_wav_bytes(&samples, 16000);
        // RIFF header
        assert_eq!(&wav[0..4], b"RIFF");
        // file size (36 + data)
        let file_size = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(file_size, 36 + 10);
        assert_eq!(&wav[8..12], b"WAVE");
        // data chunk length
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_len, 10);
        // total bytes
        assert_eq!(wav.len(), 44 + 10);
    }

    #[test]
    fn sample_rate_in_header_matches_input() {
        let wav = write_pcm16_wav_bytes(&[0i16; 4], 22050);
        let sr = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        assert_eq!(sr, 22050);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::AudioData;

    #[test]
    fn passthrough_16k_mono() {
        let samples: Vec<f32> = (0..160).map(|i| i as f32 / 160.0).collect();
        let audio = AudioData {
            samples: samples.clone(),
            sample_rate: 16_000,
            channels: 1,
        };
        let result = to_16k_mono_f32(&audio);
        assert_eq!(result.len(), samples.len());
        for (a, b) in result.iter().zip(samples.iter()) {
            assert!((a - b).abs() < 1e-6, "passthrough mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn stereo_to_mono() {
        // L=1.0, R=0.0 interleaved: [1.0, 0.0, 1.0, 0.0, ...]
        let samples: Vec<f32> = (0..20)
            .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let audio = AudioData {
            samples,
            sample_rate: 16_000,
            channels: 2,
        };
        let result = to_16k_mono_f32(&audio);
        assert_eq!(result.len(), 10);
        for &v in &result {
            assert!((v - 0.5).abs() < 1e-6, "stereo mix expected 0.5, got {v}");
        }
    }

    #[test]
    fn downsample_44100_to_16000() {
        let num_samples = 44_100usize;
        let samples: Vec<f32> = (0..num_samples).map(|i| (i as f32).sin()).collect();
        let audio = AudioData {
            samples,
            sample_rate: 44_100,
            channels: 1,
        };
        let result = to_16k_mono_f32(&audio);
        // Expected ~16000 samples. Rubato's polyphase resampler adds a small
        // amount of latency + chunk-padding at the boundary, so the output
        // length can differ by a few percent from the strict rate-ratio calc.
        // ±5% is well within the tolerance downstream callers need.
        let expected = (num_samples as f64 * 16_000.0 / 44_100.0).ceil() as usize;
        let delta = (result.len() as isize - expected as isize).unsigned_abs();
        assert!(
            delta * 100 <= expected * 5,
            "expected ~{expected} samples, got {} ({} off, >{}% delta)",
            result.len(),
            delta,
            5
        );
    }

    #[test]
    fn f32_to_i16_conversion() {
        let samples = vec![1.0f32, -1.0, 0.0, 0.5];
        let result = f32_to_i16(&samples);
        assert_eq!(result[0], 32_767);
        assert_eq!(result[1], -32_767);
        assert_eq!(result[2], 0);
        // 0.5 * 32767 = 16383.5 → truncates to 16383
        assert_eq!(result[3], 16_383);
    }

    #[test]
    fn empty_audio() {
        let audio = AudioData {
            samples: vec![],
            sample_rate: 44_100,
            channels: 1,
        };
        let resampled = to_16k_mono_f32(&audio);
        assert!(
            resampled.is_empty(),
            "expected empty output for empty input"
        );

        let converted = f32_to_i16(&[]);
        assert!(
            converted.is_empty(),
            "expected empty i16 output for empty input"
        );
    }

    #[test]
    fn trim_removes_trailing_silence() {
        // 3s loud + 2s silence = 80,000 samples at 16kHz
        let mut samples = vec![0.5f32; 48_000]; // 3s of speech
        samples.extend(vec![0.0f32; 32_000]); // 2s silence
        let trimmed = trim_trailing_silence(&samples, 0.01);
        assert!(
            trimmed.len() < samples.len(),
            "should have trimmed silence: {} vs {}",
            trimmed.len(),
            samples.len()
        );
        // Should keep ~3s speech + 0.5s pad
        assert!(
            trimmed.len() >= 48_000,
            "should keep the speech: got {}",
            trimmed.len()
        );
        assert!(
            trimmed.len() <= 56_000,
            "should keep speech + 0.5s pad max: got {}",
            trimmed.len()
        );
    }

    #[test]
    fn trim_keeps_audio_without_trailing_silence() {
        // 2s loud, no silence
        let samples = vec![0.5f32; 32_000];
        let trimmed = trim_trailing_silence(&samples, 0.01);
        assert_eq!(
            trimmed.len(),
            samples.len(),
            "no trailing silence = unchanged"
        );
    }

    #[test]
    fn trim_preserves_short_audio() {
        // Shorter than 1 window
        let samples = vec![0.5f32; 100];
        let trimmed = trim_trailing_silence(&samples, 0.01);
        assert_eq!(trimmed.len(), samples.len(), "short audio unchanged");
    }

    #[test]
    fn trim_handles_all_silence() {
        // 5s of pure silence
        let samples = vec![0.0f32; 80_000];
        let trimmed = trim_trailing_silence(&samples, 0.01);
        // Should trim heavily but keep the tail pad
        assert!(
            trimmed.len() <= 8_000,
            "all-silence should be trimmed to ~tail pad: got {}",
            trimmed.len()
        );
    }
}
