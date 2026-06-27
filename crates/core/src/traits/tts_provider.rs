//! Text-to-speech provider trait.

use async_trait::async_trait;

use crate::error::AppResult;
use crate::types::{TtsConfig, VoiceInfo};

/// Abstraction over any text-to-speech provider.
///
/// Implemented by the TTS provider crate. Returns raw PCM audio bytes
/// from [`synthesize`](TtsProvider::synthesize).
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// The canonical name of this provider (e.g. `"local"`).
    fn name(&self) -> &str;

    /// Returns the voices available from this provider.
    async fn available_voices(&self) -> AppResult<Vec<VoiceInfo>>;

    /// Synthesize the given text and return raw PCM audio bytes.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::TtsProvider`](crate::error::AppError::TtsProvider)
    /// on synthesis failure.
    async fn synthesize(&self, text: &str, config: TtsConfig) -> AppResult<Vec<u8>>;
}
