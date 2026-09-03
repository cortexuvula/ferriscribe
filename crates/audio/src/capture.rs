//! Real-time microphone capture via cpal.
//!
//! Opens an input stream on a selected audio device, pushes samples through a
//! lock-free ring buffer into a drain thread that writes a 32-bit float WAV
//! file and emits downsampled waveform snapshots for UI visualization.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

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
// Capture health watchdog
// ──────────────────────────────────────────────────────────────────────────────

/// A chunk (≈50 ms of audio) counts as "sound" when its RMS reaches this
/// level or a single sample reaches `SOUND_CHUNK_PEAK`. Tuned well below
/// conversational speech (typical RMS 0.02–0.3) and at/above room noise.
pub const SOUND_CHUNK_RMS: f32 = 0.002;
pub const SOUND_CHUNK_PEAK: f32 = 0.02;

/// Peak and sum-of-squares over a sample slice (the shared stats kernel for
/// the watchdog and the device probe).
fn sample_stats(samples: &[f32]) -> (f32, f64) {
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    for &s in samples {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += (s as f64) * (s as f64);
    }
    (peak, sum_sq)
}

/// Whether a chunk should count as captured speech/signal for the watchdog.
fn chunk_qualifies_as_sound(chunk: &[f32]) -> bool {
    if chunk.is_empty() {
        return false;
    }
    let (peak, sum_sq) = sample_stats(chunk);
    let rms = (sum_sq / chunk.len() as f64) as f32;
    rms >= SOUND_CHUNK_RMS || peak >= SOUND_CHUNK_PEAK
}

/// Live signal-health accumulator for an in-progress capture.
///
/// The drain thread is the single writer (one `note_*` call per ~50 ms
/// chunk); the capture-stream error callback and readers (event emitter,
/// stop path) take the Mutex briefly. Updated stats are pure numbers —
/// levels and timestamps, never audio content — so they are safe to log
/// and emit under the app's PHI rules.
pub struct CaptureHealth {
    started_at: Instant,
    actual_rate: u32,
    actual_channels: u16,
    is_paused: Arc<AtomicBool>,
    inner: Mutex<HealthInner>,
}

#[derive(Default)]
struct HealthInner {
    peak: f32,
    sum_sq: f64,
    total_samples: u64,
    first_sound_at: Option<Instant>,
    last_sound_at: Option<Instant>,
    last_data_at: Option<Instant>,
    stream_error: Option<String>,
}

/// Point-in-time view of a capture's signal health.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaptureHealthSnapshot {
    /// True while the user has capture paused (suppresses data/silence alerts).
    pub paused: bool,
    /// Wall-clock time since capture start (includes pauses).
    pub elapsed_secs: f64,
    /// Sample-derived audio duration (excludes pauses — no samples flow then).
    pub duration_secs: f64,
    /// Span between the first and last chunk that qualified as signal.
    pub signal_secs: Option<f64>,
    /// Maximum absolute sample so far (0.0–1.0 for float PCM).
    pub peak: f32,
    /// Root-mean-square level over all captured samples.
    pub rms: f32,
    pub total_samples: u64,
    /// Whether any chunk has qualified as signal yet.
    pub has_signal: bool,
    /// Seconds since the device last delivered ANY samples (None = never).
    pub secs_since_last_data: Option<f64>,
    /// Seconds since the last chunk that qualified as signal (None = never).
    pub secs_since_last_sound: Option<f64>,
    /// First OS-level stream error, if the capture callback reported one.
    pub stream_error: Option<String>,
}

impl CaptureHealth {
    fn new(actual_rate: u32, actual_channels: u16, is_paused: Arc<AtomicBool>) -> Self {
        Self {
            started_at: Instant::now(),
            actual_rate,
            actual_channels,
            is_paused,
            inner: Mutex::new(HealthInner::default()),
        }
    }

    /// Record that the device delivered samples (any content, including
    /// digital silence) at `now`.
    fn note_data(&self, now: Instant) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_data_at = Some(now);
        }
    }

    /// Fold a chunk into the running stats; updates first/last-sound when
    /// the chunk qualifies as signal.
    fn note_chunk(&self, chunk: &[f32], now: Instant) {
        if let Ok(mut inner) = self.inner.lock() {
            let (peak, sum_sq) = sample_stats(chunk);
            if peak > inner.peak {
                inner.peak = peak;
            }
            inner.sum_sq += sum_sq;
            inner.total_samples += chunk.len() as u64;
            if chunk_qualifies_as_sound(chunk) {
                if inner.first_sound_at.is_none() {
                    inner.first_sound_at = Some(now);
                }
                inner.last_sound_at = Some(now);
            }
        }
    }

    /// Record an OS-level stream error from the cpal callback. Uses
    /// try_lock semantics via a plain lock held for microseconds — the
    /// drain thread locks at most every ~50 ms, so contention is
    /// practically impossible; on the rare contended error the message is
    /// simply skipped (a following error will land).
    fn note_stream_error(&self, message: String) {
        if let Ok(mut inner) = self.inner.lock()
            && inner.stream_error.is_none()
        {
            inner.stream_error = Some(message);
        }
    }

    /// Compute the current snapshot. Call after the drain thread has joined
    /// for the authoritative final stats.
    pub fn snapshot(&self, now: Instant) -> CaptureHealthSnapshot {
        // A poisoned lock means the drain thread panicked — report what the
        // default (zeroed) state implies: no signal ever seen.
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed_secs = now.duration_since(self.started_at).as_secs_f64();
        let duration_secs = inner.total_samples as f64
            / (self.actual_rate as f64 * self.actual_channels.max(1) as f64);
        CaptureHealthSnapshot {
            paused: self.is_paused.load(Ordering::Relaxed),
            elapsed_secs,
            duration_secs,
            signal_secs: inner
                .first_sound_at
                .zip(inner.last_sound_at)
                .map(|(first, last)| last.duration_since(first).as_secs_f64()),
            peak: inner.peak,
            rms: if inner.total_samples > 0 {
                (inner.sum_sq / inner.total_samples as f64) as f32
            } else {
                0.0
            },
            total_samples: inner.total_samples,
            has_signal: inner.first_sound_at.is_some(),
            secs_since_last_data: inner
                .last_data_at
                .map(|t| now.duration_since(t).as_secs_f64()),
            secs_since_last_sound: inner
                .last_sound_at
                .map(|t| now.duration_since(t).as_secs_f64()),
            stream_error: inner.stream_error.clone(),
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
    /// Shared watchdog stats — keeps accumulating until the drain thread
    /// joins; `health()` clones the Arc so callers can read a final
    /// snapshot after `stop()` consumed the handle.
    health: Arc<CaptureHealth>,
}

impl CaptureHandle {
    /// Arc handle to this capture's health accumulator.
    pub fn health(&self) -> Arc<CaptureHealth> {
        Arc::clone(&self.health)
    }

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

    let health = Arc::new(CaptureHealth::new(
        actual_rate,
        actual_channels,
        Arc::clone(&is_paused),
    ));
    let health_error_cb = Arc::clone(&health);

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
            move |err| {
                tracing::error!("cpal input stream error: {err}");
                // Surface the first OS-level stream error to the health
                // watchdog so the UI can alert instead of silently
                // producing a truncated/empty recording.
                health_error_cb.note_stream_error(err.to_string());
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
    let health_drain = Arc::clone(&health);
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
                // The device delivered samples (any content) — feeds the
                // "no data arriving" side of the watchdog.
                health_drain.note_data(Instant::now());

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
                    health_drain.note_chunk(&chunk, Instant::now());
                    let waveform = downsample_waveform(&chunk, 128);
                    let _ = waveform_tx.try_send(waveform);
                }
            } else if stop_flag_drain.load(Ordering::Relaxed) {
                // Flush remaining accumulator.
                if !acc.is_empty() {
                    health_drain.note_chunk(&acc, Instant::now());
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
                    health_drain.note_chunk(&batch, Instant::now());
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
        health,
    };

    Ok((handle, waveform_rx))
}

// ──────────────────────────────────────────────────────────────────────────────
// Device probe
// ──────────────────────────────────────────────────────────────────────────────

/// Result of a short `probe_device` capture.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceProbe {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: usize,
    pub peak: f32,
    pub rms: f64,
}

/// Capture `duration` of audio from `device` and report its level stats —
/// the "Test microphone" pre-flight used by Settings → Audio so a dead/
/// muted input can be caught before clinic starts. No file is written.
///
/// Must run on a dedicated thread (the cpal stream is !Send); a probe
/// while a recording is in progress is the caller's call — most platforms
/// allow a second stream on the same device.
pub fn probe_device(device: &cpal::Device, duration: Duration) -> AudioResult<DeviceProbe> {
    let stream_config = negotiate_stream_config(
        device,
        &CaptureConfig {
            sample_rate: 16_000,
            ..CaptureConfig::default()
        },
    )?;

    // Channel (not a shared buffer) keeps the audio callback lock-free;
    // allocation per callback is fine for a ~1 s probe.
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                let _ = tx.send(data.to_vec());
            },
            |err| {
                tracing::warn!("cpal probe stream error: {err}");
            },
            None,
        )
        .map_err(|e| AudioError::Capture(e.to_string()))?;

    stream
        .play()
        .map_err(|e| AudioError::Capture(e.to_string()))?;

    let mut samples: Vec<f32> = Vec::new();
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(chunk) => samples.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(stream);

    if samples.len() < 100 {
        return Err(AudioError::Capture(
            "the microphone delivered no samples during the probe — check that the device is connected and not exclusively held by another app".to_string(),
        ));
    }
    let (peak, sum_sq) = sample_stats(&samples);
    let rms = (sum_sq / samples.len() as f64).sqrt();
    Ok(DeviceProbe {
        sample_rate: stream_config.sample_rate.0,
        channels: stream_config.channels,
        samples: samples.len(),
        peak,
        rms,
    })
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
    fn chunk_sound_classification() {
        // Digital silence never qualifies.
        assert!(!chunk_qualifies_as_sound(&[0.0; 800]));
        // Faint room noise (rms 0.0005) stays below the RMS threshold…
        let noise = vec![0.0005f32; 800];
        assert!(!chunk_qualifies_as_sound(&noise));
        // …but a single transient peak qualifies (device clicks, pops).
        let mut transient = vec![0.0f32; 800];
        transient[10] = 0.05;
        assert!(chunk_qualifies_as_sound(&transient));
        // Conversational-level audio (rms ≈ 0.05) qualifies.
        let speech: Vec<f32> = (0..800).map(|i| 0.05 * (i as f32 * 0.1).sin()).collect();
        assert!(chunk_qualifies_as_sound(&speech));
        assert!(!chunk_qualifies_as_sound(&[]));
    }

    #[test]
    fn capture_health_tracks_signal_span_and_levels() {
        let health = CaptureHealth::new(16_000, 1, Arc::new(AtomicBool::new(false)));
        let t0 = Instant::now();

        // 3 s of silence with data flowing (the "device alive but muted"
        // case): data arrives, but nothing qualifies as sound.
        health.note_data(t0);
        health.note_chunk(&[0.0; 800], t0 + Duration::from_millis(50));
        health.note_chunk(&[0.0; 800], t0 + Duration::from_millis(100));

        // Speech at t0+2s..t0+5s.
        let speech: Vec<f32> = (0..800).map(|i| 0.1 * (i as f32 * 0.05).sin()).collect();
        health.note_data(t0 + Duration::from_secs(2));
        health.note_chunk(&speech, t0 + Duration::from_secs(2));
        health.note_data(t0 + Duration::from_secs(5));
        health.note_chunk(&speech, t0 + Duration::from_secs(5));

        let snap = health.snapshot(t0 + Duration::from_secs(12));
        assert!(snap.has_signal);
        assert_eq!(snap.signal_secs, Some(3.0));
        // 3200 samples (1600 silence + 1600 speech) at 16 kHz mono = 0.2 s.
        assert!((snap.duration_secs - 0.2).abs() < 0.001);
        assert!((snap.peak - 0.1).abs() < 0.01);
        assert!(snap.rms > 0.0);
        assert_eq!(snap.total_samples, 3200);
        // Data last arrived at t0+5 → 7 s stale at snapshot time.
        assert!((snap.secs_since_last_data.unwrap() - 7.0).abs() < 0.1);
        assert!((snap.secs_since_last_sound.unwrap() - 7.0).abs() < 0.1);
        assert!(!snap.paused);
        assert_eq!(snap.stream_error, None);
    }

    #[test]
    fn capture_health_silent_start_reports_no_signal() {
        let health = CaptureHealth::new(16_000, 1, Arc::new(AtomicBool::new(false)));
        let t0 = Instant::now();
        health.note_data(t0);
        for i in 1..=4 {
            health.note_chunk(&[0.0; 800], t0 + Duration::from_millis(50 * i));
        }
        let snap = health.snapshot(t0 + Duration::from_secs(10));
        assert!(!snap.has_signal);
        assert_eq!(snap.signal_secs, None);
        assert_eq!(snap.secs_since_last_sound, None);
        assert!(snap.secs_since_last_data.is_some());
        assert_eq!(snap.rms, 0.0);
    }

    #[test]
    fn capture_health_records_first_stream_error_only_and_pause_flag() {
        let paused = Arc::new(AtomicBool::new(false));
        let health = CaptureHealth::new(48_000, 2, Arc::clone(&paused));
        let t0 = Instant::now();
        health.note_stream_error("device disconnected".to_string());
        health.note_stream_error("later error".to_string());

        paused.store(true, Ordering::Relaxed);
        let snap = health.snapshot(t0 + Duration::from_secs(1));
        assert_eq!(snap.stream_error.as_deref(), Some("device disconnected"));
        assert!(snap.paused);
        // No samples: duration 0, rms 0.
        assert_eq!(snap.duration_secs, 0.0);
        assert_eq!(snap.rms, 0.0);
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
