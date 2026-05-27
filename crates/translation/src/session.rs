//! Translation session state.
//!
//! Tracks an active translation conversation between a healthcare provider
//! and a patient. Each translated utterance is stored as a [`TranslationEntry`]
//! with speaker identity, timestamps, and language direction.
//!
//! The session automatically infers translation direction from the speaker:
//! - [`Speaker::Provider`] utterances flow `source_lang → target_lang`
//! - [`Speaker::Patient`] utterances flow `target_lang → source_lang`

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Determines the direction(s) in which translation flows during a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TranslationMode {
    /// Both provider→patient and patient→provider translations are active.
    Bidirectional,
    /// Translation flows in one direction only (source → target).
    OneWay,
}

/// Identifies who spoke an utterance.
///
/// Used by [`TranslationSession::add_entry`] to determine translation direction:
/// the provider speaks the session's source language, the patient speaks the
/// target language.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    /// The healthcare provider (clinician, nurse, etc.).
    Provider,
    /// The patient (or their companion/interpreter).
    Patient,
}

/// A single translated utterance stored in session history.
///
/// Created by [`TranslationSession::add_entry`], which sets `source_lang` and
/// `target_lang` automatically based on the [`Speaker`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationEntry {
    /// The original (untranslated) text as spoken.
    pub original: String,
    /// The translated text.
    pub translated: String,
    /// Language of the original text (BCP-47 code, e.g. `"en"`, `"es"`).
    pub source_lang: String,
    /// Language of the translated text (BCP-47 code).
    pub target_lang: String,
    /// When the utterance was added to the session.
    pub timestamp: DateTime<Utc>,
    /// Who spoke this utterance.
    pub speaker: Speaker,
}

/// An active translation session between a provider and a patient.
///
/// Tracks the language pair, translation mode, and a chronological history
/// of translated utterances. Use [`add_entry`](Self::add_entry) to record
/// each utterance — direction is inferred from the [`Speaker`].
///
/// Sessions are serializable (via serde), so they can be persisted or
/// transmitted to the frontend for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationSession {
    /// Primary language of the healthcare provider (BCP-47, e.g. `"en"`).
    pub source_lang: String,
    /// Primary language of the patient (BCP-47, e.g. `"es"`).
    pub target_lang: String,
    /// Chronological list of translated utterances in this session.
    pub history: Vec<TranslationEntry>,
    /// Whether translation is bidirectional or one-way.
    pub mode: TranslationMode,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
}

impl TranslationSession {
    /// Creates a new translation session.
    pub fn new(source_lang: impl Into<String>, target_lang: impl Into<String>, mode: TranslationMode) -> Self {
        Self {
            source_lang: source_lang.into(),
            target_lang: target_lang.into(),
            history: Vec::new(),
            mode,
            created_at: Utc::now(),
        }
    }

    /// Adds a translated entry to the session history.
    ///
    /// - `Provider` utterances are translated from `source_lang` → `target_lang`.
    /// - `Patient` utterances are translated from `target_lang` → `source_lang`.
    pub fn add_entry(
        &mut self,
        original: impl Into<String>,
        translated: impl Into<String>,
        speaker: Speaker,
    ) {
        let (entry_source, entry_target) = match speaker {
            Speaker::Provider => (self.source_lang.clone(), self.target_lang.clone()),
            Speaker::Patient => (self.target_lang.clone(), self.source_lang.clone()),
        };

        self.history.push(TranslationEntry {
            original: original.into(),
            translated: translated.into(),
            source_lang: entry_source,
            target_lang: entry_target,
            timestamp: Utc::now(),
            speaker,
        });
    }

    /// Returns the number of entries in the session.
    pub fn entry_count(&self) -> usize {
        self.history.len()
    }

    /// Produces a human-readable transcript of the session, ordered chronologically.
    ///
    /// Each entry is formatted as:
    /// `[HH:MM:SS] ROLE (src→tgt): original\n  → translated`
    pub fn export_text(&self) -> String {
        let mut lines = Vec::new();
        for entry in &self.history {
            let time = entry.timestamp.format("%H:%M:%S").to_string();
            let role = match entry.speaker {
                Speaker::Provider => "Provider",
                Speaker::Patient => "Patient",
            };
            lines.push(format!(
                "[{time}] {role} ({src}→{tgt}): {orig}\n  → {trans}",
                time = time,
                role = role,
                src = entry.source_lang,
                tgt = entry.target_lang,
                orig = entry.original,
                trans = entry.translated,
            ));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session() {
        let session = TranslationSession::new("en", "es", TranslationMode::Bidirectional);
        assert_eq!(session.source_lang, "en");
        assert_eq!(session.target_lang, "es");
        assert_eq!(session.mode, TranslationMode::Bidirectional);
        assert_eq!(session.entry_count(), 0);
    }

    #[test]
    fn add_entries_verify_lang_direction() {
        let mut session = TranslationSession::new("en", "es", TranslationMode::Bidirectional);

        session.add_entry("Hello", "Hola", Speaker::Provider);
        session.add_entry("Me duele la cabeza", "My head hurts", Speaker::Patient);

        assert_eq!(session.entry_count(), 2);

        // Provider entry: source_lang (en) → target_lang (es)
        let provider_entry = &session.history[0];
        assert_eq!(provider_entry.source_lang, "en");
        assert_eq!(provider_entry.target_lang, "es");
        assert_eq!(provider_entry.original, "Hello");
        assert_eq!(provider_entry.translated, "Hola");

        // Patient entry: target_lang (es) → source_lang (en)
        let patient_entry = &session.history[1];
        assert_eq!(patient_entry.source_lang, "es");
        assert_eq!(patient_entry.target_lang, "en");
        assert_eq!(patient_entry.original, "Me duele la cabeza");
        assert_eq!(patient_entry.translated, "My head hurts");
    }

    #[test]
    fn export_text() {
        let mut session = TranslationSession::new("en", "es", TranslationMode::OneWay);
        session.add_entry("How are you?", "¿Cómo está?", Speaker::Provider);

        let text = session.export_text();
        assert!(text.contains("Provider"));
        assert!(text.contains("en→es"));
        assert!(text.contains("How are you?"));
        assert!(text.contains("¿Cómo está?"));
    }

    #[test]
    fn session_serializes() {
        let mut session = TranslationSession::new("en", "fr", TranslationMode::Bidirectional);
        session.add_entry("Good morning", "Bonjour", Speaker::Provider);

        let json = serde_json::to_string(&session).expect("serialize");
        let deserialized: TranslationSession = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.source_lang, "en");
        assert_eq!(deserialized.target_lang, "fr");
        assert_eq!(deserialized.entry_count(), 1);
        assert_eq!(deserialized.history[0].original, "Good morning");
        assert_eq!(deserialized.mode, TranslationMode::Bidirectional);
    }
}
