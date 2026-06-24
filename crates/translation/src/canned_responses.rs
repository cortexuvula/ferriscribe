//! Pre-built medical phrases with hardcoded translations.
//!
//! Provides a [`CannedResponseSet`] of common clinical phrases (greetings,
//! pain assessment, medication history, discharge instructions) with
//! pre-translated text in Spanish, French, German, and Chinese.
//!
//! Canned responses are the fast path: when the UI presents a known phrase,
//! the app can look up the translation by id and target language — no AI
//! round-trip required. For free-form text, use
//! [`AiTranslationProvider`](crate::ai_translator::AiTranslationProvider)
//! instead.

use std::collections::HashMap;

/// A single pre-built medical response with multilingual translations.
///
/// The canonical text is always English ([`text_en`](Self::text_en)); the
/// [`translations`](Self::translations) map provides the same phrase in
/// other languages, keyed by BCP-47 code.
#[derive(Debug, Clone)]
pub struct CannedResponse {
    /// Unique identifier for this response (e.g. `"assessment_pain_location"`).
    pub id: String,
    /// Category grouping (e.g. `"general"`, `"assessment"`, `"history"`, `"instructions"`).
    pub category: String,
    /// The canonical English text.
    pub text_en: String,
    /// Translations keyed by BCP-47 language code (e.g. `"es"`, `"fr"`, `"de"`, `"zh"`).
    pub translations: HashMap<String, String>,
}

impl CannedResponse {
    fn new(
        id: impl Into<String>,
        category: impl Into<String>,
        text_en: impl Into<String>,
        translations: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Self {
        Self {
            id: id.into(),
            category: category.into(),
            text_en: text_en.into(),
            translations: translations
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
}

/// A collection of canned medical responses, searchable by id or category.
///
/// Use [`default_medical`](Self::default_medical) to get the built-in set,
/// or construct a custom set by populating the [`responses`](Self::responses)
/// field directly.
#[derive(Debug, Clone, Default)]
pub struct CannedResponseSet {
    /// The underlying list of canned responses.
    pub responses: Vec<CannedResponse>,
}

impl CannedResponseSet {
    /// Returns the built-in set of 7 pre-built medical responses with
    /// translations in Spanish (`es`), French (`fr`), German (`de`), and
    /// Chinese (`zh`).
    ///
    /// The responses span four categories:
    /// - **general** — greeting
    /// - **assessment** — pain location, pain scale
    /// - **history** — allergies, medications, symptom duration
    /// - **instructions** — return-visit guidance
    pub fn default_medical() -> Self {
        let responses = vec![
            CannedResponse::new(
                "general_greeting",
                "general",
                "Hello, how are you feeling today?",
                [
                    ("es", "Hola, ¿cómo se siente hoy?"),
                    ("fr", "Bonjour, comment vous sentez-vous aujourd'hui?"),
                    ("de", "Hallo, wie fühlen Sie sich heute?"),
                    ("zh", "你好，你今天感觉怎么样？"),
                ],
            ),
            CannedResponse::new(
                "assessment_pain_location",
                "assessment",
                "Where does it hurt?",
                [
                    ("es", "¿Dónde le duele?"),
                    ("fr", "Où avez-vous mal?"),
                    ("de", "Wo tut es weh?"),
                    ("zh", "哪里疼？"),
                ],
            ),
            CannedResponse::new(
                "assessment_pain_scale",
                "assessment",
                "On a scale of 1 to 10, how would you rate your pain?",
                [
                    (
                        "es",
                        "En una escala del 1 al 10, ¿cómo calificaría su dolor?",
                    ),
                    (
                        "fr",
                        "Sur une échelle de 1 à 10, comment évalueriez-vous votre douleur?",
                    ),
                    (
                        "de",
                        "Auf einer Skala von 1 bis 10, wie würden Sie Ihre Schmerzen bewerten?",
                    ),
                    ("zh", "在1到10的范围内，您如何评估您的疼痛？"),
                ],
            ),
            CannedResponse::new(
                "history_allergies",
                "history",
                "Are you allergic to any medications?",
                [
                    ("es", "¿Es alérgico a algún medicamento?"),
                    ("fr", "Êtes-vous allergique à des médicaments?"),
                    ("de", "Sind Sie gegen Medikamente allergisch?"),
                    ("zh", "您对任何药物过敏吗？"),
                ],
            ),
            CannedResponse::new(
                "history_medications",
                "history",
                "What medications are you currently taking?",
                [
                    ("es", "¿Qué medicamentos está tomando actualmente?"),
                    ("fr", "Quels médicaments prenez-vous actuellement?"),
                    ("de", "Welche Medikamente nehmen Sie derzeit?"),
                    ("zh", "您目前正在服用什么药物？"),
                ],
            ),
            CannedResponse::new(
                "history_symptom_duration",
                "history",
                "How long have you had these symptoms?",
                [
                    ("es", "¿Cuánto tiempo lleva con estos síntomas?"),
                    ("fr", "Depuis combien de temps avez-vous ces symptômes?"),
                    ("de", "Wie lange haben Sie diese Symptome schon?"),
                    ("zh", "您有这些症状多久了？"),
                ],
            ),
            CannedResponse::new(
                "instructions_return",
                "instructions",
                "Please come back if your symptoms get worse.",
                [
                    ("es", "Por favor regrese si sus síntomas empeoran."),
                    ("fr", "Veuillez revenir si vos symptômes s'aggravent."),
                    (
                        "de",
                        "Bitte kommen Sie zurück, wenn sich Ihre Symptome verschlechtern.",
                    ),
                    ("zh", "如果您的症状加重，请回来就诊。"),
                ],
            ),
        ];

        Self { responses }
    }

    /// Returns all responses in a given category.
    pub fn by_category(&self, category: &str) -> Vec<&CannedResponse> {
        self.responses
            .iter()
            .filter(|r| r.category == category)
            .collect()
    }

    /// Returns a response by its id, or `None` if not found.
    pub fn get(&self, id: &str) -> Option<&CannedResponse> {
        self.responses.iter().find(|r| r.id == id)
    }

    /// Returns the translation for a response id in the given language,
    /// or `None` if either the id or the language is missing.
    pub fn get_translation(&self, id: &str, lang: &str) -> Option<&str> {
        self.get(id)?.translations.get(lang).map(|s| s.as_str())
    }

    /// Returns a sorted, deduplicated list of all categories in this set.
    pub fn categories(&self) -> Vec<&str> {
        let mut cats: Vec<&str> = self.responses.iter().map(|r| r.category.as_str()).collect();
        cats.sort();
        cats.dedup();
        cats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_set_has_responses() {
        let set = CannedResponseSet::default_medical();
        assert!(set.responses.len() >= 7);
    }

    #[test]
    fn get_translation_spanish() {
        let set = CannedResponseSet::default_medical();
        let translation = set.get_translation("general_greeting", "es");
        assert_eq!(translation, Some("Hola, ¿cómo se siente hoy?"));
    }

    #[test]
    fn missing_lang_none() {
        let set = CannedResponseSet::default_medical();
        let translation = set.get_translation("general_greeting", "ja");
        assert!(translation.is_none());
    }

    #[test]
    fn missing_id_none() {
        let set = CannedResponseSet::default_medical();
        let translation = set.get_translation("nonexistent_id", "es");
        assert!(translation.is_none());
    }

    #[test]
    fn by_category() {
        let set = CannedResponseSet::default_medical();
        let history = set.by_category("history");
        assert!(!history.is_empty());
        for r in &history {
            assert_eq!(r.category, "history");
        }
        let general = set.by_category("general");
        assert!(!general.is_empty());
    }

    #[test]
    fn categories_deduped() {
        let set = CannedResponseSet::default_medical();
        let cats = set.categories();
        // Check sorted
        let mut sorted = cats.clone();
        sorted.sort();
        assert_eq!(cats, sorted);
        // Check deduplicated
        let unique: Vec<_> = {
            let mut seen = std::collections::HashSet::new();
            cats.iter().filter(|c| seen.insert(*c)).copied().collect()
        };
        assert_eq!(cats.len(), unique.len());
        // Check expected categories are present
        assert!(cats.contains(&"assessment"));
        assert!(cats.contains(&"general"));
        assert!(cats.contains(&"history"));
        assert!(cats.contains(&"instructions"));
    }
}
