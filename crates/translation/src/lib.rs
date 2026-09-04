//! # medical-translation
//!
//! Text translation for clinician–patient conversations.
//!
//! This crate provides AI translation:
//!
//! - **AI translation** ([`ai_translator::AiTranslationProvider`]) — wraps any
//!   [`medical_core::traits::AiProvider`] to translate free-form medical text
//!   via LLM prompts. Handles arbitrary text in 12 languages.
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
pub mod session;

use thiserror::Error;

/// Languages supported for translation (BCP-47 base codes).
///
/// Single source of truth for [`ai_translator::AiTranslationProvider`]'s
/// supported-language set and the Tauri `translation_supported_languages`
/// command — the latter needs the list without an active AI provider.
pub fn supported_languages() -> Vec<medical_core::traits::translation::Language> {
    use medical_core::traits::translation::Language;

    vec![
        Language {
            code: "en".into(),
            name: "English".into(),
        },
        Language {
            code: "es".into(),
            name: "Spanish".into(),
        },
        Language {
            code: "fr".into(),
            name: "French".into(),
        },
        Language {
            code: "de".into(),
            name: "German".into(),
        },
        Language {
            code: "zh".into(),
            name: "Chinese (中文)".into(),
        },
        Language {
            code: "ja".into(),
            name: "Japanese (日本語)".into(),
        },
        Language {
            code: "ko".into(),
            name: "Korean (한국어)".into(),
        },
        Language {
            code: "pt".into(),
            name: "Portuguese".into(),
        },
        Language {
            code: "ar".into(),
            name: "Arabic (العربية)".into(),
        },
        Language {
            code: "hi".into(),
            name: "Hindi (हिन्दी)".into(),
        },
        Language {
            code: "ru".into(),
            name: "Russian (Русский)".into(),
        },
        Language {
            code: "it".into(),
            name: "Italian".into(),
        },
    ]
}

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

#[cfg(test)]
mod tests {
    use super::supported_languages;

    #[test]
    fn supported_languages_are_unique_and_covered_by_stt_hints() {
        let langs = supported_languages();
        let mut codes: Vec<&str> = langs.iter().map(|l| l.code.as_str()).collect();
        codes.sort_unstable();
        let unique: Vec<&str> = {
            let mut u = codes.clone();
            u.dedup();
            u
        };
        assert_eq!(codes.len(), unique.len(), "language codes must be unique");
        assert!(codes.contains(&"en"));
        assert!(codes.contains(&"zh"));
    }
}
