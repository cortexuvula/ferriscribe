//! `LocalSttProvider` — the single `SttProvider` implementation for local inference.
//!
//! Orchestrates the full local pipeline:
//! 1. Resample to 16 kHz mono via [`crate::audio_prep`]
//! 2. Whisper transcription via [`crate::whisper::WhisperTranscriber`] (whisper-rs, Metal GPU on macOS)
//! 3. Optional pyannote speaker diarization via [`crate::diarization::SpeakerDiarizer`]
//! 4. Merge segments with speaker labels via [`crate::merge`]
//!
//! Both Whisper and diarization run inside `tokio::task::spawn_blocking` to avoid
//! blocking the async runtime. Cancellation is checked before and after each
//! blocking stage — whisper-rs does not support mid-inference interrupt callbacks.

use std::path::PathBuf;

use async_trait::async_trait;
use futures_core::Stream;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use medical_core::error::{AppError, AppResult};
use medical_core::traits::SttProvider;
use medical_core::types::{
    AudioData, AudioStream, SttConfig, Transcript, TranscriptChunk, TranscriptSegment,
};

use crate::audio_prep;
use crate::diarization::SpeakerDiarizer;
use crate::merge;
use crate::whisper::WhisperTranscriber;

/// Local speech-to-text provider using whisper-rs + pyannote diarization.
///
/// Implements [`medical_core::traits::SttProvider`] by running Whisper locally
/// via whisper-rs and optionally running pyannote speaker diarization on the
/// same audio buffer. All blocking inference runs inside `spawn_blocking`.
///
/// # Model Paths
///
/// - `whisper_model_path` — path to a `ggml-*.bin` whisper.cpp model file
/// - `segmentation_model_path` — path to `segmentation-3.0.onnx` (pyannote VAD)
/// - `embedding_model_path` — path to `wespeaker_en_voxceleb_CAM++.onnx`
///
/// Diarization is silently skipped if either pyannote model is missing.
pub struct LocalSttProvider {
    whisper_model_path: PathBuf,
    segmentation_model_path: PathBuf,
    embedding_model_path: PathBuf,
}

impl LocalSttProvider {
    /// Create a new local STT provider with the given model paths.
    ///
    /// No models are loaded at construction time — Whisper and pyannote models
    /// are loaded lazily inside `transcribe()` via `spawn_blocking`.
    pub fn new(
        whisper_model_path: PathBuf,
        segmentation_model_path: PathBuf,
        embedding_model_path: PathBuf,
    ) -> Self {
        Self {
            whisper_model_path,
            segmentation_model_path,
            embedding_model_path,
        }
    }
}

#[async_trait]
impl SttProvider for LocalSttProvider {
    fn name(&self) -> &str {
        "local"
    }

    fn supports_streaming(&self) -> bool {
        false
    }

    fn supports_diarization(&self) -> bool {
        self.segmentation_model_path.exists() && self.embedding_model_path.exists()
    }

    async fn transcribe(
        &self,
        audio: AudioData,
        config: SttConfig,
        cancel: CancellationToken,
    ) -> AppResult<Transcript> {
        // Pre-check: bail immediately if the caller already cancelled.
        // whisper-rs running inside spawn_blocking is not interruptible
        // mid-pass without callback plumbing (out of scope), so the best we
        // can do is check before/after the blocking model invocation.
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }

        if !self.whisper_model_path.exists() {
            return Err(AppError::SttProvider(format!(
                "Whisper model not found at {}. Download a model in Settings → Audio / STT.",
                self.whisper_model_path.display()
            )));
        }

        let duration = audio.duration_seconds();

        // Stage 1: Resample to 16kHz mono
        let audio_16k = audio_prep::to_16k_mono_f32(&audio);

        // Stage 2: Whisper transcription
        let whisper_path = self.whisper_model_path.clone();
        let language = config.language.clone();
        let audio_for_whisper = audio_16k.clone();

        let whisper_segments = tokio::task::spawn_blocking(move || {
            let transcriber = WhisperTranscriber::new(whisper_path);
            transcriber.transcribe(&audio_for_whisper, language.as_deref())
        })
        .await
        .map_err(|e| AppError::SttProvider(format!("Whisper task panicked: {e}")))?
        ?;

        // Post-check: if the user cancelled while whisper was running,
        // discard the transcript rather than continuing into diarization
        // and segment merging.
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }

        // Stage 3: Speaker diarization (optional)
        // Track whether diarization was actually attempted (models present and
        // diarize requested) vs. skipped. An empty speaker_turns after a
        // successful run just means a single speaker was detected — that's not
        // a failure and must not trigger a "models missing" warning.
        let (speaker_turns, diarization_attempted) = if config.diarize && self.supports_diarization() {
            let seg_path = self.segmentation_model_path.clone();
            let emb_path = self.embedding_model_path.clone();
            let audio_i16 = audio_prep::f32_to_i16(&audio_16k);

            let turns = match tokio::task::spawn_blocking(move || {
                let diarizer = SpeakerDiarizer::new(seg_path, emb_path);
                diarizer.diarize(&audio_i16, 16000)
            })
            .await
            {
                Ok(Ok(turns)) => turns,
                Ok(Err(e)) => {
                    warn!(error = %e, "Diarization failed — proceeding without speaker labels");
                    Vec::new()
                }
                Err(e) => {
                    warn!(error = %e, "Diarization task panicked — proceeding without speaker labels");
                    Vec::new()
                }
            };
            (turns, true)
        } else {
            if config.diarize && !self.supports_diarization() {
                warn!("Diarization requested but models not found — skipping");
            }
            (Vec::new(), false)
        };

        // Stage 4: Merge whisper segments with speaker turns
        let segments: Vec<TranscriptSegment> =
            merge::merge_segments_with_speakers(&whisper_segments, &speaker_turns);

        let full_text: String = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        info!(
            segments = segments.len(),
            text_len = full_text.len(),
            "Local transcription complete"
        );

        Ok(Transcript {
            text: full_text,
            segments,
            language: config.language.clone(),
            duration_seconds: Some(duration),
            provider: "local".to_owned(),
            metadata: serde_json::json!({
                "whisper_model": self.whisper_model_path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown"),
                "diarization": !speaker_turns.is_empty(),
                // True when the diarization pipeline was actually invoked
                // (models present, diarize requested). A false value here
                // means diarization was skipped — models missing or not
                // requested — which is what the frontend warning checks.
                "diarization_attempted": diarization_attempted,
            }),
        })
    }

    async fn transcribe_stream(
        &self,
        _stream: AudioStream,
        _config: SttConfig,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<TranscriptChunk>> + Send + Unpin>> {
        Err(AppError::SttProvider(
            "Local provider does not support streaming transcription".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::{AudioData, SttConfig};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn dummy_audio() -> AudioData {
        // 1 second of silent 16 kHz mono f32.
        AudioData {
            samples: vec![0.0_f32; 16_000],
            sample_rate: 16_000,
            channels: 1,
        }
    }

    /// Build a LocalSttProvider pointing at non-existent model paths.
    /// Suitable for tests that exercise pre-model-load behavior (e.g. cancellation).
    fn local_provider_for_test() -> LocalSttProvider {
        LocalSttProvider::new(
            PathBuf::from("/nonexistent/whisper-model.bin"),
            PathBuf::from("/nonexistent/segmentation.onnx"),
            PathBuf::from("/nonexistent/embedding.onnx"),
        )
    }

    #[tokio::test]
    async fn transcribe_returns_cancelled_immediately_when_token_pre_cancelled() {
        // The local provider can't interrupt whisper-rs once it's running, but
        // it should bail immediately if the token is already cancelled when
        // transcribe is called.
        let provider = local_provider_for_test();
        let cancel = CancellationToken::new();
        cancel.cancel(); // pre-cancelled

        let result = provider
            .transcribe(dummy_audio(), SttConfig::default(), cancel)
            .await;

        assert!(result.is_err(), "expected Err on pre-cancelled token");
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("cancel"),
            "expected error to mention cancel, got: {msg}"
        );
    }
}
