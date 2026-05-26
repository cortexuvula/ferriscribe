pub mod elevenlabs_tts;
pub mod local_tts;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum TtsError {
    #[error("Synthesis failed: {0}")]
    Synthesis(String),
    #[error("Voice not found: {0}")]
    VoiceNotFound(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Invalid header: {0}")]
    InvalidHeader(String),
}

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
