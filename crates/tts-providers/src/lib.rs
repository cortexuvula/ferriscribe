//! Text-to-speech provider implementations.
//!
//! This crate provides concrete implementations of the [`TtsProvider`] trait
//! (defined in `medical-core`) for two backends:
//!
//! - [`elevenlabs_tts::ElevenLabsTtsProvider`] — calls the ElevenLabs REST API
//!   and returns encoded audio bytes (typically MP3).
//! - [`local_tts::LocalTtsProvider`] — uses the OS-native speech engine
//!   (feature-gated behind `local-tts`). **Does not return audio bytes**;
//!   audio is played directly through the system speakers.
//!
//! The crate also defines [`TtsError`], used for provider-construction failures
//! (invalid API-key headers, HTTP client build errors). Runtime synthesis
//! errors go through `AppError::TtsProvider` from `medical-core`.
//!
//! [`TtsProvider`]: medical_core::traits::TtsProvider

pub mod elevenlabs_tts;
pub mod local_tts;

use thiserror::Error;

/// Errors that can occur when constructing a TTS provider.
///
/// Runtime synthesis errors use `AppError::TtsProvider` instead; this enum
/// covers failures specific to provider initialisation (bad headers, HTTP
/// client build failures).
#[derive(Error, Debug)]
pub enum TtsError {
    /// General synthesis failure (unused by current providers but reserved
    /// for future use).
    #[error("Synthesis failed: {0}")]
    Synthesis(String),

    /// The requested voice identifier was not found.
    #[error("Voice not found: {0}")]
    VoiceNotFound(String),

    /// An HTTP-layer error (e.g. failed to build the `reqwest` client).
    #[error("HTTP error: {0}")]
    Http(String),

    /// A header value could not be parsed (e.g. non-ASCII in an API key).
    #[error("Invalid header: {0}")]
    InvalidHeader(String),
}

/// Convenience alias for results from provider construction.
pub type TtsResult<T> = Result<T, TtsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_header_error_message() {
        let error = TtsError::InvalidHeader("api-key header: invalid ASCII".to_string());
        let message = error.to_string();
        assert!(message.contains("api-key header"));
        assert!(message.contains("invalid ASCII"));
    }
}
