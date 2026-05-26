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

#[derive(Error, Debug)]
pub enum SttError {
    #[error("Transcription failed: {0}")]
    Transcription(String),
    #[error("Provider unavailable: {0}")]
    Unavailable(String),
    #[error("Audio format error: {0}")]
    AudioFormat(String),
    #[error("Model download error: {0}")]
    ModelDownload(String),
    #[error("Model not found: {0}")]
    ModelNotFound(String),
}

pub type SttResult<T> = Result<T, SttError>;
