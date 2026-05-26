//! Translation provider trait.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;

/// A supported language.
///
/// Returned by [`TranslationProvider::supported_languages`]. Uses BCP-47
/// codes (e.g. `"en"`, `"es"`, `"fr"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Language {
    /// BCP-47 language code.
    pub code: String,
    /// Human-readable language name.
    pub name: String,
}

/// Abstraction over any translation backend.
///
/// Implemented by the `translation` crate. Uses BCP-47 language codes
/// for source and target languages.
#[async_trait]
pub trait TranslationProvider: Send + Sync {
    /// The canonical name of this provider (e.g. `"ai_translator"`).
    fn name(&self) -> &str;

    /// Returns all languages this provider can translate to/from.
    async fn supported_languages(&self) -> AppResult<Vec<Language>>;

    /// Translate `text` from `source_language` into `target_language`.
    ///
    /// If `source_language` is `None`, the provider will attempt to
    /// detect it automatically. Language codes are BCP-47 (e.g. `"en"`,
    /// `"es"`).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Translation`](crate::error::AppError::Translation)
    /// on failure.
    async fn translate(
        &self,
        text: &str,
        source_language: Option<&str>,
        target_language: &str,
    ) -> AppResult<String>;

    /// Detect the language of the supplied text.
    ///
    /// Returns the BCP-47 language code of the most likely language.
    async fn detect_language(&self, text: &str) -> AppResult<String>;
}
