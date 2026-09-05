//! Speech-to-text provider trait.

use async_trait::async_trait;
use futures_core::Stream;
use tokio_util::sync::CancellationToken;

use crate::error::AppResult;
use crate::types::{AudioData, AudioStream, SttConfig, Transcript, TranscriptChunk};

/// Abstraction over any speech-to-text provider.
///
/// Implemented by `stt-providers` (local whisper-rs and remote Whisper
/// server). The `audio` and `processing` crates depend only on this
/// trait for provider-agnostic transcription.
///
/// # Cancellation
///
/// The `transcribe` method accepts a [`CancellationToken`]. When fired,
/// the provider should return [`AppError::Cancelled`](crate::error::AppError::Cancelled)
/// as promptly as possible. Remote providers cancel in-flight HTTP via
/// `tokio::select!`; local providers check the token before/after the
/// blocking model invocation but cannot interrupt it mid-pass.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// The canonical name of this provider (e.g. `"whisper-local"`, `"whisper-remote"`).
    fn name(&self) -> &str;

    /// Returns `true` if this provider supports streaming transcription.
    fn supports_streaming(&self) -> bool;

    /// Returns `true` if this provider supports speaker diarization.
    fn supports_diarization(&self) -> bool;

    /// Transcribe a complete audio buffer and return the full transcript.
    ///
    /// # Errors
    ///
    /// - [`AppError::SttProvider`](crate::error::AppError::SttProvider) on API failure.
    /// - [`AppError::Cancelled`](crate::error::AppError::Cancelled) if the token fires.
    /// - [`AppError::EndpointOffline`](crate::error::AppError::EndpointOffline) if
    ///   a remote endpoint is unreachable.
    async fn transcribe(
        &self,
        audio: AudioData,
        config: SttConfig,
        cancel: CancellationToken,
    ) -> AppResult<Transcript>;

    /// Transcribe a live audio stream, yielding chunks as they are recognized.
    ///
    /// Only available if [`supports_streaming`](SttProvider::supports_streaming)
    /// returns `true`.
    async fn transcribe_stream(
        &self,
        stream: AudioStream,
        config: SttConfig,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<TranscriptChunk>> + Send + Unpin>>;

    /// Best-effort hint to warm provider resources (e.g. load the whisper
    /// model) ahead of the next [`transcribe`](SttProvider::transcribe) call.
    ///
    /// Callers fire this when they know a transcription is coming but the
    /// audio isn't ready yet (the translate tab starts a capture seconds
    /// before the user stops talking) and must treat errors as advisory —
    /// a failed prewarm never blocks or fails the later transcription.
    /// The default is a no-op for providers with nothing to warm (the
    /// remote server holds its own model).
    async fn prewarm(&self) -> AppResult<()> {
        Ok(())
    }
}
