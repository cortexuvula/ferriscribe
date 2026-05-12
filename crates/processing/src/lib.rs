pub mod pipeline;
pub mod batch;
pub mod prompt_resolver;
pub mod soap_generator;
pub mod document_generator;
pub mod vocabulary_corrector;
pub mod edit_distance;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("pipeline error: {0}")]
    Pipeline(String),
    #[error("generation error: {0}")]
    Generation(String),
    #[error("STT error: {0}")]
    Stt(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("processing cancelled")]
    Cancelled,
}

pub type ProcessingResult<T> = Result<T, ProcessingError>;
