//! # medical-translation
//!
//! Text translation for clinician–patient conversations.
//!
//! This crate provides two complementary translation strategies:
//!
//! - **AI translation** ([`ai_translator::AiTranslationProvider`]) — wraps any
//!   [`medical_core::traits::AiProvider`] to translate free-form medical text
//!   via LLM prompts. Handles arbitrary text in 12 languages.
//! - **Canned responses** ([`canned_responses::CannedResponseSet`]) — a small
//!   set of pre-translated common medical phrases (greetings, pain assessment,
//!   medication history, instructions) available instantly without an AI
//!   round-trip.
//!
//! Session state is tracked via [`session::TranslationSession`], which records
//! every translated utterance with speaker identity, timestamps, and automatic
//! language-direction inference.
//!
//! ## Dependencies
//!
//! Depends on [`medical_core`] for the [`TranslationProvider`] trait,
//! completion types, and error model. Used by `src-tauri` to power real-time
//! translation during consultations.
//!
//! [`TranslationProvider`]: medical_core::traits::TranslationProvider

pub mod ai_translator;
/// Canned responses — not yet wired into production; tests-only until integrated.
#[cfg(test)]
pub mod canned_responses;
pub mod session;

use thiserror::Error;

/// Errors that can occur during translation operations.
#[derive(Debug, Error)]
pub enum TranslationError {
    /// A translation request failed (network error, model error, etc.).
    #[error("translation error: {0}")]
    Translation(String),

    /// The requested language is not supported by this provider.
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),

    /// Automatic language detection could not determine the source language.
    #[error("language detection error: {0}")]
    Detection(String),
}

/// Convenience result type for translation operations.
pub type TranslationResult<T> = Result<T, TranslationError>;
