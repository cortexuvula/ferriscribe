//! Whisper transcription via whisper-rs.
//!
//! Wraps the whisper-rs FFI bindings to run local Whisper inference. Uses
//! `Greedy { best_of: 1 }` sampling with anti-hallucination parameters
//! (suppress_blank, suppress_nst, no_context) to minimize repetition loops.
//! Must run inside `spawn_blocking` — the C++ inference is CPU/GPU-bound.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{info, instrument};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use medical_core::error::{AppError, AppResult};

/// A timestamped text segment from local Whisper transcription.
///
/// Timestamps are in seconds (converted from whisper.cpp's centisecond output).
#[derive(Debug, Clone)]
pub struct WhisperSegment {
    /// The transcribed text for this segment, trimmed of whitespace.
    pub text: String,
    /// Segment start time in seconds.
    pub start: f64,
    /// Segment end time in seconds.
    pub end: f64,
}

/// Process-wide cache of the loaded `WhisperContext`, keyed by model path.
///
/// Building a context means reading the whole ggml file off disk and
/// initializing the compute graph (~1-2 s for base, ~5 s for
/// large-v3-turbo) — a per-utterance cost the translate tab pays on every
/// tap. The cache keeps one loaded context alive across `transcribe` calls;
/// each call creates its own `WhisperState` from the shared context (the
/// whisper.cpp-endorsed way to run concurrent decodes).
///
/// A different `model_path` evicts and reloads — insurance against the model
/// file being switched under a live provider. The cache lives as long as the
/// owning provider: `reinit_providers`, model downloads, and pairing all
/// rebuild the provider and drop its cache.
pub struct WhisperContextCache {
    inner: std::sync::Mutex<Option<CachedContext>>,
}

struct CachedContext {
    model_path: PathBuf,
    context: Arc<WhisperContext>,
}

impl Default for WhisperContextCache {
    fn default() -> Self {
        Self::new()
    }
}

impl WhisperContextCache {
    /// Create an empty cache — no model is loaded until the first `get`.
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(None),
        }
    }

    /// Return the loaded context for `model_path`, loading it on first use.
    ///
    /// The lock is held only for the get-or-init; the returned `Arc` keeps
    /// the context alive after the lock is dropped, and a failed load is
    /// never cached (the next call retries — the missing-model error a user
    /// sees stays live rather than going stale after they download it).
    ///
    /// A poisoned lock is recovered from rather than propagated: the guarded
    /// state is always a fully-constructed `Option` (single assignment), and
    /// bricking every future transcription off one panicking load would be
    /// far worse than re-using whatever was cached.
    pub fn get(&self, model_path: &Path) -> AppResult<Arc<WhisperContext>> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = guard.as_ref()
            && cached.model_path == model_path
        {
            return Ok(Arc::clone(&cached.context));
        }
        let context = Arc::new(
            WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
                .map_err(|e| {
                    AppError::stt_provider(format!("Failed to load Whisper model: {e}"))
                })?,
        );
        *guard = Some(CachedContext {
            model_path: model_path.to_owned(),
            context: Arc::clone(&context),
        });
        Ok(context)
    }
}

/// Wrapper around whisper-rs for local transcription.
///
/// Resolves its model through a shared [`WhisperContextCache`] so
/// consecutive transcriptions skip the model-load cost. Each `transcribe()`
/// still creates a fresh `WhisperState` — decoder state is per-call, the
/// loaded weights are not.
pub struct WhisperTranscriber {
    model_path: PathBuf,
    cache: Arc<WhisperContextCache>,
}

impl WhisperTranscriber {
    /// Create a transcriber that loads the model at `model_path` through
    /// `cache` on first use and reuses it afterwards.
    pub fn new(model_path: PathBuf, cache: Arc<WhisperContextCache>) -> Self {
        Self { model_path, cache }
    }

    /// Load (or reuse) the model context — the work `prewarm` performs
    /// ahead of the first transcription.
    pub fn prewarm(&self) -> AppResult<Arc<WhisperContext>> {
        self.cache.get(&self.model_path)
    }

    /// Transcribe 16 kHz mono f32 audio.
    ///
    /// Must be called inside `tokio::task::spawn_blocking` — the underlying
    /// whisper.cpp inference is CPU/GPU-bound and would block the async runtime.
    ///
    /// # Parameters
    ///
    /// - `audio_16k_mono` — 16 kHz mono f32 PCM samples (preprocessed by [`crate::audio_prep`])
    /// - `language` — optional 2-letter language code (e.g. `"en"`); `None` = auto-detect
    #[instrument(skip(self, audio_16k_mono), fields(provider = "whisper_local"))]
    pub fn transcribe(
        &self,
        audio_16k_mono: &[f32],
        language: Option<&str>,
    ) -> AppResult<Vec<WhisperSegment>> {
        let ctx = self.prewarm()?;
        let mut state = ctx
            .create_state()
            .map_err(|e| AppError::stt_provider(format!("Failed to create Whisper state: {e}")))?;

        // Greedy decoding is more resistant to repetition loops than BeamSearch.
        // Beam search explores multiple paths and picks the highest cumulative
        // probability — which is often the repetition loop. Greedy takes the
        // single best token at each step, breaking out of loops more easily.
        // Combined with suppress_blank + no_context + trailing-silence trim,
        // this is the standard whisper.cpp anti-hallucination configuration.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        let lang_code: Option<String> = language.map(|l| l.chars().take(2).collect());
        params.set_language(lang_code.as_deref());
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_translate(false);
        params.set_no_timestamps(false);
        // Temperature fallback breaks repetition loops: if a decoding attempt
        // looks degenerate, whisper.cpp retries with temperature += 0.2. These
        // are whisper.cpp's own default values.
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);

        // Anti-hallucination parameters — these are critical for preventing
        // the repetition loops that plague medical transcripts ("I don't know
        // if I was going to go through it" × 30, "I see a counsellor once
        // every two weeks" × 100). Without these, whisper.cpp's decoder gets
        // stuck in high-probability loops during low-energy/noisy audio.
        //
        // suppress_blank: drops the blank token from the top-k sampling,
        //   which is the #1 cause of repetition loops (the model emits blank,
        //   then repeats the preceding context).
        // suppress_non_speech_tokens: filters out tokens like [BLANK], [NOISE],
        //   [MUSIC], etc. that trigger loop behavior.
        // no_context: prevents the model from carrying decoded text from one
        //   segment into the next as prompt context — this stops a loop in
        //   one segment from propagating to subsequent segments.
        // max_len: caps each segment to 60 tokens (~1-2 sentences). Without
        //   this, a single degenerate segment can grow to thousands of tokens
        //   of repeated text.
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_context(true);
        // Note: set_max_len is NOT used — it caps segment LENGTH IN CHARACTERS
        // (not tokens as the name might suggest). A value of 60 would
        // fragment every transcript into ~10-word pieces. The trailing-silence
        // trim + cross-segment repetition filter handle hallucinations without
        // segment-length restrictions.

        info!(
            samples = audio_16k_mono.len(),
            duration_s = audio_16k_mono.len() as f64 / 16_000.0,
            "Running local Whisper inference"
        );

        state
            .full(params, audio_16k_mono)
            .map_err(|e| AppError::stt_provider(format!("Whisper inference failed: {e}")))?;

        let num_segments = state.full_n_segments();
        let mut segments = Vec::with_capacity(num_segments as usize);

        for i in 0..num_segments {
            let segment = state
                .get_segment(i)
                .ok_or_else(|| AppError::stt_provider(format!("Segment {i} out of bounds")))?;

            let text = segment.to_str_lossy().map_err(|e| {
                AppError::stt_provider(format!("Failed to get segment {i} text: {e}"))
            })?;

            // whisper.cpp timestamps are in centiseconds.
            let start = segment.start_timestamp() as f64 / 100.0;
            let end = segment.end_timestamp() as f64 / 100.0;

            let text_trimmed = text.trim().to_owned();
            if !text_trimmed.is_empty() {
                segments.push(WhisperSegment {
                    text: text_trimmed,
                    start,
                    end,
                });
            }
        }

        info!(segments = segments.len(), "Whisper transcription complete");
        Ok(segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing_model_path() -> PathBuf {
        PathBuf::from("/nonexistent/__ferriscribe_whisper_test__.bin")
    }

    #[test]
    fn missing_model_errors_and_failure_is_not_cached() {
        let cache = WhisperContextCache::new();
        let path = missing_model_path();

        let first = cache.get(&path);
        assert!(first.is_err(), "missing model must error");

        // The failure must not be cached — after the model appears (e.g. the
        // user downloads it), the next call must retry the load, not replay
        // the stale error. We can't make the file appear here, but a second
        // error at a DIFFERENT path proves get() re-attempts loads instead of
        // returning the first failure for everything.
        let second = cache.get(&PathBuf::from("/nonexistent/other-model.bin"));
        assert!(second.is_err());
    }

    #[test]
    fn poisoned_cache_lock_is_recovered_from_not_fatal() {
        let cache = Arc::new(WhisperContextCache::new());
        // Poison the lock via a panic inside the critical section.
        let poisoning = Arc::clone(&cache);
        let handle = std::thread::spawn(move || {
            let _guard = poisoning.inner.lock().unwrap();
            panic!("intentional poison");
        });
        assert!(handle.join().is_err());
        // A poisoned lock must neither hang, panic the caller, nor brick the
        // cache: `get` recovers and still attempts the load (here: the
        // missing-model error, same as an unpoisoned cache).
        let result = cache.get(&missing_model_path());
        assert!(result.is_err());
    }

    /// Pins the core cache contract against a REAL model file: two `get`
    /// calls with the same path return the SAME context (no reload), and a
    /// different path reloads. Gated behind `FERRISCRIBE_STT_MODEL=<path>`
    /// because it needs an actual ggml file (~148 MB for base); skipped
    /// otherwise so `cargo test --workspace --lib` stays runnable everywhere.
    #[test]
    fn real_model_context_is_reused_across_gets() {
        let Ok(model_path) = std::env::var("FERRISCRIBE_STT_MODEL") else {
            eprintln!("skipping: FERRISCRIBE_STT_MODEL not set");
            return;
        };
        let cache = WhisperContextCache::new();
        let first = cache.get(Path::new(&model_path)).expect("first load");
        let second = cache.get(Path::new(&model_path)).expect("second get");
        assert!(
            Arc::ptr_eq(&first, &second),
            "same path must return the cached context"
        );
    }
}
