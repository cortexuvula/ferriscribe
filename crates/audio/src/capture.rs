//! Real-time microphone capture via cpal.
//!
//! Opens an input stream on a selected audio device, pushes samples through a
//! lock-free ring buffer into a drain thread that writes a 32-bit float WAV
//! file and emits downsampled waveform snapshots for UI visualization.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use crate::{AudioError, AudioResult};

// ──────────────────────────────────────────────────────────────────────────────
// Configuration
// ──────────────────────────────────────────────────────────────────────────────

/// Configuration for the capture pipeline.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Sample rate in Hz (default 16 000).
    pub sample_rate: u32,
    /// Number of channels (default 1 — mono).
    pub channels: u16,
    /// Ring-buffer capacity in frames (default 4 096).
    pub buffer_size: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            channels: 1,
            buffer_size: 4_096,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// CaptureHandle
// ──────────────────────────────────────────────────────────────────────────────

/// A handle to an in-progress audio capture session.
///
/// Dropping the handle stops capture and joins the drain thread.
pub struct CaptureHandle {
    is_paused: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    drain_thread: Option<thread::JoinHandle<()>>,
    // Keep the cpal stream alive as long as the handle lives.
    _stream: cpal::Stream,
}

impl CaptureHandle {
    /// Pause audio capture (samples are discarded while paused).
    pub fn pause(&self) {
        self.is_paused.store(true, Ordering::SeqCst);
    }

    /// Resume audio capture after a pause.
    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
    }

    /// Stop capture and flush the remaining samples to the WAV file.
    ///
    /// Calling `stop()` is equivalent to dropping the handle, but gives you
    /// an explicit place to handle any panic from the drain thread.
    pub fn stop(mut self) {
        self.do_stop();
    }

    fn do_stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.drain_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.do_stop();
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Negotiate a supported `StreamConfig` from the device.
///
/// Tries the requested sample rate first, then falls back to common rates
/// (48 kHz, 44.1 kHz, 16 kHz) and finally the device's default config.
///
/// Two device-side constraints cpal is strict about (especially on Windows
/// WASAPI, where this otherwise surfaces as the unhelpful "stream
/// configuration is not supported by the device" error):
/// - The capture callback consumes `&[f32]`, so we filter
///   `supported_input_configs` to ranges advertising `SampleFormat::F32`
///   when any are available; an I16-only device falls through to
///   `default_input_config` as a last resort.
/// - The channel count in the returned `StreamConfig` MUST equal the
///   matched range's `channels()`. Asking for mono on a stereo-only
///   device fails. Downstream `audio_prep::to_mono` mixes multi-channel
///   captures down for transcription, so returning the device's native
///   channel count is safe.
fn negotiate_stream_config(
    device: &cpal::Device,
    desired: &CaptureConfig,
) -> AudioResult<cpal::StreamConfig> {
    let all_supported: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_input_configs()
        .map_err(|e| AudioError::Capture(format!("Cannot query device configs: {e}")))?
        .collect();

    // Prefer F32-capable ranges since the capture callback consumes &[f32].
    let f32_pool: Vec<&cpal::SupportedStreamConfigRange> = all_supported
        .iter()
        .filter(|r| r.sample_format() == cpal::SampleFormat::F32)
        .collect();

    let pool: Vec<&cpal::SupportedStreamConfigRange> = if !f32_pool.is_empty() {
        f32_pool
    } else if !all_supported.is_empty() {
        all_supported.iter().collect()
    } else {
        // Last resort: ask cpal for its own default.
        return device
            .default_input_config()
            .map(|c| c.into())
            .map_err(|e| AudioError::Capture(format!("No supported configs: {e}")));
    };

    // Rates to try, in priority order: requested rate first, then common rates.
    let candidate_rates: &[u32] = &[desired.sample_rate, 48_000, 44_100, 16_000, 22_050, 96_000];

    for &rate in candidate_rates {
        for range in &pool {
            if range.min_sample_rate().0 <= rate && rate <= range.max_sample_rate().0 {
                return Ok(cpal::StreamConfig {
                    channels: range.channels(),
                    sample_rate: cpal::SampleRate(rate),
                    buffer_size: cpal::BufferSize::Default,
                });
            }
        }
    }

    // If none of the candidate rates match, pick the max rate from the first range.
    let first = pool[0];
    Ok(cpal::StreamConfig {
        channels: first.channels(),
        sample_rate: first.max_sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    })
}

/// Start an audio capture session.
///
/// Returns a `(CaptureHandle, Receiver<Vec<f32>>)`.  The receiver delivers
/// downsampled waveform snapshots (~128 points, every ~50 ms) so callers can
/// draw a live VU meter without seeing every raw sample.
///
/// Samples are written to `output_path` as a 32-bit float WAV file.
pub fn start_capture(
    device: &cpal::Device,
    config: CaptureConfig,
    output_path: &Path,
) -> AudioResult<(CaptureHandle, mpsc::Receiver<Vec<f32>>)> {
    // ── Build cpal StreamConfig ───────────────────────────────────────────────
    let stream_config = negotiate_stream_config(device, &config)?;

    // Use the negotiated values (may differ from the requested config).
    let actual_rate = stream_config.sample_rate.0;
    let actual_channels = stream_config.channels;

    tracing::info!(
        "Audio capture: requested {}Hz {}ch, using {}Hz {}ch",
        config.sample_rate,
        config.channels,
        actual_rate,
        actual_channels
    );

    // ── Ring buffer (2 seconds of audio) ─────────────────────────────────────
    let ring_capacity = (actual_rate as usize)
        .saturating_mul(actual_channels as usize)
        .saturating_mul(2)
        .max(config.buffer_size.saturating_mul(4));
    let rb = HeapRb::<f32>::new(ring_capacity);
    let (mut prod, mut cons) = rb.split();

    let is_paused = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));

    let is_paused_cb = Arc::clone(&is_paused);

    // ── cpal input stream callback ────────────────────────────────────────────
    let stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                if is_paused_cb.load(Ordering::Relaxed) {
                    return;
                }
                // Push as many samples as fit; silently drop the rest when the
                // ring buffer is full (back-pressure is acceptable for audio).
                prod.push_slice(data);
            },
            |err| {
                tracing::error!("cpal input stream error: {err}");
            },
            None,
        )
        .map_err(|e| AudioError::Capture(e.to_string()))?;

    stream
        .play()
        .map_err(|e| AudioError::Capture(e.to_string()))?;

    // ── WAV writer setup ──────────────────────────────────────────────────────
    let wav_spec = hound::WavSpec {
        channels: actual_channels,
        sample_rate: actual_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let output_path = output_path.to_path_buf();
    let stop_flag_drain = Arc::clone(&stop_flag);

    // ── waveform channel ──────────────────────────────────────────────────────
    // Bounded channel — if the UI-side consumer stalls (slow event system,
    // backgrounded app, etc.), the drain thread drops the newest waveform
    // frames via try_send rather than growing the queue without limit.
    // 32 × 50 ms ≈ 1.6 s of buffered waveform — plenty for a responsive UI,
    // cheap to drop if the consumer is gone.
    let (waveform_tx, waveform_rx) = mpsc::sync_channel::<Vec<f32>>(32);

    // Chunk size to accumulate before computing & sending a waveform snapshot.
    // ~50 ms worth of samples.
    let waveform_chunk = (actual_rate / 20) as usize;

    // ── Drain thread ──────────────────────────────────────────────────────────
    let drain_handle = thread::spawn(move || {
        let mut writer = match hound::WavWriter::create(&output_path, wav_spec) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("failed to create WAV writer: {e}");
                return;
            }
        };

        let mut acc: Vec<f32> = Vec::with_capacity(waveform_chunk * 2);
        let mut batch: Vec<f32> = Vec::with_capacity(waveform_chunk * 4);

        loop {
            // Drain available samples from the ring buffer.
            batch.clear();
            batch.extend(cons.pop_iter());

            if !batch.is_empty() {
                for &s in &batch {
                    if let Err(e) = writer.write_sample(s) {
                        tracing::error!("WAV write error: {e}");
                    }
                    acc.push(s);
                }

                // Emit waveform snapshot(s). try_send drops on full so a
                // stalled UI consumer can't grow the channel without bound.
                while acc.len() >= waveform_chunk {
                    let chunk = acc.drain(..waveform_chunk).collect::<Vec<_>>();
                    let waveform = downsample_waveform(&chunk, 128);
                    let _ = waveform_tx.try_send(waveform);
                }
            } else if stop_flag_drain.load(Ordering::Relaxed) {
                // Flush remaining accumulator.
                if !acc.is_empty() {
                    let waveform = downsample_waveform(&acc, 128);
                    let _ = waveform_tx.try_send(waveform);
                }
                // Final drain: capture any samples that arrived between the
                // empty-check above and the stop_flag-check (race window).
                loop {
                    batch.clear();
                    batch.extend(cons.pop_iter());
                    if batch.is_empty() {
                        break;
                    }
                    for &s in &batch {
                        if let Err(e) = writer.write_sample(s) {
                            tracing::error!("WAV write error (final drain): {e}");
                        }
                    }
                }
                break;
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        }

        if let Err(e) = writer.finalize() {
            tracing::error!("WAV finalize error: {e}");
        }
    });

    let handle = CaptureHandle {
        is_paused,
        stop_flag,
        drain_thread: Some(drain_handle),
        _stream: stream,
    };

    Ok((handle, waveform_rx))
}

// ──────────────────────────────────────────────────────────────────────────────
// Waveform helper
// ──────────────────────────────────────────────────────────────────────────────

/// Downsample `samples` to `target_len` points by taking the peak absolute
/// value within each chunk.
///
/// If `samples.len() <= target_len` the original slice is returned as-is.
pub fn downsample_waveform(samples: &[f32], target_len: usize) -> Vec<f32> {
    if samples.is_empty() || target_len == 0 {
        return Vec::new();
    }
    if samples.len() <= target_len {
        return samples.to_vec();
    }
    let n = samples.len();
    (0..target_len)
        .map(|i| {
            // Map output index i to an input window [start, end).
            let start = i * n / target_len;
            let end = ((i + 1) * n / target_len).min(n);
            samples[start..end]
                .iter()
                .map(|s| s.abs())
                .fold(0.0f32, f32::max)
        })
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capture_config() {
        let c = CaptureConfig::default();
        assert_eq!(c.sample_rate, 16_000);
        assert_eq!(c.channels, 1);
        assert_eq!(c.buffer_size, 4_096);
    }

    #[test]
    fn downsample_reduces_length() {
        let samples: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let result = downsample_waveform(&samples, 128);
        assert_eq!(result.len(), 128);
    }

    #[test]
    fn downsample_preserves_short() {
        let samples = vec![0.1f32, 0.5, 0.3];
        let result = downsample_waveform(&samples, 128);
        assert_eq!(result, samples);
    }

    #[test]
    fn downsample_takes_peak() {
        // One chunk: [-0.9, 0.5, 0.3] → peak abs = 0.9
        let samples = vec![-0.9f32, 0.5, 0.3, 0.1, 0.2, 0.4];
        // target_len = 2 → chunk_size = 3
        let result = downsample_waveform(&samples, 2);
        assert_eq!(result.len(), 2);
        assert!(
            (result[0] - 0.9).abs() < 1e-6,
            "first peak should be 0.9, got {}",
            result[0]
        );
        assert!(
            (result[1] - 0.4).abs() < 1e-6,
            "second peak should be 0.4, got {}",
            result[1]
        );
    }
}
