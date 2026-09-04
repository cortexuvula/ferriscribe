//! Transcription pipeline orchestration and document generation for FerriScribe.
//!
//! This crate turns recordings into transcripts, and transcripts into clinical
//! documents. It is the central "work" crate — everything upstream captures
//! audio, everything downstream displays results.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`pipeline`] | Pipeline configuration and step/progress vocabulary types. |
//! | [`batch`] | Multi-recording batch job state tracker. |
//! | [`soap_generator`] | SOAP note system/user prompts and AI-output post-processing. |
//! | [`document_generator`] | Prompt builders for referrals, letters, and synopses. |
//! | [`prompt_resolver`] | `{key}` placeholder substitution in user-editable templates. |
//! | [`sanitize`] | Shared prompt-injection filter for user-supplied prompt text. |
//! | [`vocabulary_corrector`] | Word-boundary-aware find-and-replace for medical abbreviations. |
//! | [`edit_distance`] | Word-level Levenshtein distance and ratio. |
//!
//! # Error Handling
//!
//! All fallible operations return [`ProcessingResult<T>`], a convenience alias
//! for `Result<T, [`ProcessingError`]>`. The error enum covers pipeline failures,
//! generation failures, STT failures, database failures, and cancellation.
//!
//! # Critical Constraint: SOAP Prompt Precision
//!
//! The SOAP system prompt in [`soap_generator`] is treated as a precision
//! instrument. Background-supplied patient context (medications, allergies,
//! conditions, supplementary notes) populates **historical Subjective fields
//! only** — it must never alter today's Objective findings, Assessment,
//! Differential Diagnosis, or Plan. The prompt contains explicit guards and a
//! 10-point self-check checklist to enforce this. If you modify the SOAP prompt,
//! run the full `soap_generator` test suite — it encodes dozens of invariants.

pub mod batch;
pub mod document_generator;
pub mod edit_distance;
// Private: contract documentation + anti-drift tests for the two markdown
// strippers (document_generator::strip_markdown vs postprocess::clean_text).
mod markdown;
pub mod ocr;
pub mod peer_discussion;
pub mod pipeline;
pub mod prompt_resolver;
pub mod sanitize;
pub mod soap_generator;
pub mod vocabulary_corrector;

use thiserror::Error;

/// Unified error type for all processing operations in this crate.
///
/// Variants cover the four failure domains a caller needs to distinguish:
///
/// | Variant | When |
/// |---|---|
/// | `Pipeline` | Pipeline orchestration failure (step sequencing, channel errors). |
/// | `Generation` | Document generation failure (SOAP, referral, letter, synopsis). |
/// | `Stt` | Speech-to-text provider failure (local whisper.cpp or remote STT). |
/// | `Database` | Persistence layer failure (transcript or document save). |
/// | `Cancelled` | The user or system cancelled the operation. |
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

/// Convenience alias: `Result<T, ProcessingError>`.
///
/// All public fallible functions in this crate return this type.
pub type ProcessingResult<T> = Result<T, ProcessingError>;
