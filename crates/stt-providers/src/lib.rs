//! # medical-stt-providers
//!
//! Speech-to-text via Whisper (local whisper.cpp or remote OpenAI-compatible server)
//! and speaker diarization via pyannote + WeSpeaker ONNX models.
//!
//! ## Provider Implementations
//!
//! - [`LocalSttProvider`] — runs whisper-rs locally (Metal GPU on macOS) with optional
//!   local pyannote diarization. The only implementation re-exported at crate root.
//! - [`remote_provider::RemoteSttProvider`] — sends audio to a remote Whisper server
//!   via HTTP POST, with the same local diarization pipeline.
//!
//! Both implement the [`medical_core::traits::SttProvider`] trait. Neither supports
//! streaming transcription.
//!
//! ## Module Overview
//!
//! | Module | Purpose |
//! |---|---|
//! | [`local_provider`] | Local whisper-rs inference + diarization |
//! | [`remote_provider`] | HTTP client for remote Whisper server |
//! | [`endpoint`] | LAN/Tailscale URL resolution with 30s cache |
//! | [`client`] | Multipart HTTP POST + cancellation + error mapping |
//! | [`whisper`] | whisper-rs wrapper: beam search, centisecond timestamps |
//! | [`diarization`] | pyannote VAD + WeSpeaker embeddings + cosine clustering |
//! | [`audio_prep`] | Resampling (rubato), f32↔i16, WAV encoding |
//! | [`merge`] | Merge whisper segments with speaker turns by overlap |
//! | [`models`] | Model catalog, download/delete, path helpers |

pub mod audio_prep;
pub mod models;
pub mod whisper;
pub mod diarization;
pub mod merge;
pub mod local_provider;
pub mod remote_provider;
pub mod endpoint;
pub mod client;

pub use local_provider::LocalSttProvider;

use thiserror::Error;

/// Errors specific to the STT provider layer.
///
/// Most STT errors in practice flow through [`medical_core::error::AppError`]
/// (e.g. `AppError::SttProvider`, `AppError::EndpointOffline`). This enum
/// covers legacy paths in model download/delete that predate the unified
/// error model.
#[derive(Error, Debug)]
pub enum SttError {
    /// Transcription failed (generic — prefer `AppError::SttProvider` for new code).
    #[error("Transcription failed: {0}")]
    Transcription(String),
    /// The STT provider binary or server is not available.
    #[error("Provider unavailable: {0}")]
    Unavailable(String),
    /// The audio data is malformed or in an unsupported format.
    #[error("Audio format error: {0}")]
    AudioFormat(String),
    /// Model download failed or the destination is not writable.
    #[error("Model download error: {0}")]
    ModelDownload(String),
    /// The requested model file does not exist on disk.
    #[error("Model not found: {0}")]
    ModelNotFound(String),
}

/// Convenience result type for [`SttError`].
pub type SttResult<T> = Result<T, SttError>;
