//! Vocabulary / medical-dictionary types for post-transcription corrections.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Category of a vocabulary entry.
///
/// Used to group corrections by type so the UI can filter and display
/// them in sections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabularyCategory {
    /// Doctor / clinician names.
    DoctorNames,
    /// Medication / drug names.
    MedicationNames,
    /// General medical terminology.
    MedicalTerminology,
    /// Medical abbreviations and acronyms.
    Abbreviations,
    /// Uncategorized entries.
    General,
}

impl Default for VocabularyCategory {
    fn default() -> Self {
        Self::General
    }
}

impl VocabularyCategory {
    /// Returns the canonical string key for this category.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DoctorNames => "doctor_names",
            Self::MedicationNames => "medication_names",
            Self::MedicalTerminology => "medical_terminology",
            Self::Abbreviations => "abbreviations",
            Self::General => "general",
        }
    }

    /// Parse a string into a [`VocabularyCategory`], accepting common
    /// aliases (e.g. `"medications"`, `"meds"` → `MedicationNames`).
    /// Falls back to [`General`](VocabularyCategory::General) for
    /// unrecognized input.
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "doctor_names" | "doctors" | "doctor" => Self::DoctorNames,
            "medication_names" | "medications" | "medication" | "meds" => Self::MedicationNames,
            "medical_terminology" | "terminology" | "medical" => Self::MedicalTerminology,
            "abbreviations" | "abbreviation" | "abbr" => Self::Abbreviations,
            _ => Self::General,
        }
    }
}

/// A find-and-replace entry in the medical vocabulary dictionary.
///
/// After transcription, the vocabulary engine scans the transcript for
/// `find_text` occurrences and replaces them with `replacement`. Entries
/// with higher `priority` are applied first to avoid conflicting
/// replacements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabularyEntry {
    /// Unique entry identifier.
    pub id: Uuid,
    /// The text to search for in transcripts.
    pub find_text: String,
    /// The replacement text.
    pub replacement: String,
    /// Which category this entry belongs to.
    pub category: VocabularyCategory,
    /// Whether matching is case-sensitive.
    pub case_sensitive: bool,
    /// Application priority (higher = applied first).
    pub priority: i32,
    /// Whether this entry is active.
    pub enabled: bool,
    /// When the entry was created.
    pub created_at: DateTime<Utc>,
    /// When the entry was last modified.
    pub updated_at: DateTime<Utc>,
}

/// Summary of a single correction applied during vocabulary processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedCorrection {
    /// The original text that was matched.
    pub find_text: String,
    /// The replacement text.
    pub replacement: String,
    /// The category of the vocabulary entry.
    pub category: VocabularyCategory,
    /// How many times this correction was applied.
    pub count: u32,
}

/// The result of running the vocabulary correction engine on a transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionResult {
    /// The transcript text before corrections.
    pub original_text: String,
    /// The transcript text after corrections.
    pub corrected_text: String,
    /// Breakdown of each distinct correction applied.
    pub corrections_applied: Vec<AppliedCorrection>,
    /// Total number of individual replacements made.
    pub total_replacements: u32,
}
