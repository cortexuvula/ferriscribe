//! Word-boundary-aware vocabulary correction for STT output.
//!
//! Applies a prioritised list of find-and-replace rules to raw transcribed
//! text, correcting common medical abbreviations and misrecognised terms
//! (e.g., "htn" → "hypertension", "dm type 2" → "diabetes mellitus type 2").
//!
//! # Correction semantics
//!
//! - Entries are sorted by priority (descending), then by match length
//!   (descending) so higher-priority and longer-match entries take precedence.
//! - Each entry is matched at word boundaries (`\b...\b`) via regex.
//! - Case sensitivity is per-entry.
//! - Compiled regex patterns are cached per `(find_text, case_sensitive)` pair.
//! - Disabled entries (`enabled = false`) are silently skipped.
//! - Multiple occurrences of the same find_text in the input are all replaced
//!   and counted.

use std::collections::HashMap;

use regex::Regex;
use tracing::{debug, info};

use medical_core::types::vocabulary::{
    AppliedCorrection, CorrectionResult, VocabularyEntry,
};

/// Apply vocabulary corrections to raw STT text.
///
/// Returns a [`CorrectionResult`] containing the corrected text, the list of
/// corrections applied (with per-entry counts), and the total replacement count.
///
/// # Behaviour
///
/// - Empty text or empty entries → returns the input unchanged.
/// - Entries are sorted by priority (desc) then match length (desc) before
///   application, so "dm type 2" matches before "dm" at the same priority.
/// - Matching uses word boundaries (`\b...\b`) — "washington" does not match
///   an entry for "washing".
/// - Regex patterns are compiled once per `(find_text, case_sensitive)` pair
///   and cached for the duration of this call.
/// - Entries with `enabled = false` are skipped.
pub fn apply_corrections(text: &str, entries: &[VocabularyEntry]) -> CorrectionResult {
    if text.is_empty() || entries.is_empty() {
        return CorrectionResult {
            original_text: text.to_string(),
            corrected_text: text.to_string(),
            corrections_applied: vec![],
            total_replacements: 0,
        };
    }

    let mut sorted: Vec<&VocabularyEntry> = entries.iter().filter(|e| e.enabled).collect();
    sorted.sort_by(|a, b| {
        b.priority.cmp(&a.priority)
            .then_with(|| b.find_text.len().cmp(&a.find_text.len()))
    });

    let mut corrected = text.to_string();
    let mut applied = Vec::new();
    let mut total = 0u32;
    let mut cache: HashMap<(String, bool), Option<Regex>> = HashMap::new();

    for entry in sorted {
        let key = (entry.find_text.clone(), entry.case_sensitive);
        let pattern = cache.entry(key).or_insert_with(|| {
            let escaped = regex::escape(&entry.find_text);
            let pat = format!(r"\b{escaped}\b");
            let flags = if entry.case_sensitive { "" } else { "(?i)" };
            Regex::new(&format!("{flags}{pat}")).ok()
        });

        if let Some(re) = pattern {
            let count = re.find_iter(&corrected).count();
            if count > 0 {
                corrected = re.replace_all(&corrected, entry.replacement.as_str()).into_owned();
                total += count as u32;
                applied.push(AppliedCorrection {
                    find_text: entry.find_text.clone(),
                    replacement: entry.replacement.clone(),
                    category: entry.category.clone(),
                    count: count as u32,
                });
                debug!(
                    find = %entry.find_text,
                    replace = %entry.replacement,
                    count,
                    "Applied vocabulary correction"
                );
            }
        }
    }

    if total > 0 {
        info!(total_replacements = total, "Vocabulary corrections applied");
    }

    CorrectionResult {
        original_text: text.to_string(),
        corrected_text: corrected,
        corrections_applied: applied,
        total_replacements: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use medical_core::types::vocabulary::VocabularyCategory;
    use uuid::Uuid;

    fn entry(find: &str, replace: &str) -> VocabularyEntry {
        VocabularyEntry {
            id: Uuid::new_v4(),
            find_text: find.to_string(),
            replacement: replace.to_string(),
            category: VocabularyCategory::General,
            case_sensitive: false,
            priority: 0,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn empty_text_returns_unchanged() {
        let result = apply_corrections("", &[entry("htn", "hypertension")]);
        assert_eq!(result.corrected_text, "");
        assert_eq!(result.total_replacements, 0);
    }

    #[test]
    fn empty_entries_returns_unchanged() {
        let result = apply_corrections("patient has htn", &[]);
        assert_eq!(result.corrected_text, "patient has htn");
        assert_eq!(result.total_replacements, 0);
    }

    #[test]
    fn basic_replacement() {
        let entries = vec![entry("htn", "hypertension")];
        let result = apply_corrections("patient has htn", &entries);
        assert_eq!(result.corrected_text, "patient has hypertension");
        assert_eq!(result.total_replacements, 1);
        assert_eq!(result.corrections_applied.len(), 1);
        assert_eq!(result.corrections_applied[0].find_text, "htn");
        assert_eq!(result.corrections_applied[0].count, 1);
    }

    #[test]
    fn word_boundary_prevents_partial_match() {
        let entries = vec![entry("htn", "hypertension")];
        let result = apply_corrections("washington is a city", &entries);
        assert_eq!(result.corrected_text, "washington is a city");
        assert_eq!(result.total_replacements, 0);
    }

    #[test]
    fn case_insensitive_by_default() {
        let entries = vec![entry("htn", "hypertension")];
        let result = apply_corrections("patient has HTN", &entries);
        assert_eq!(result.corrected_text, "patient has hypertension");
        assert_eq!(result.total_replacements, 1);
    }

    #[test]
    fn case_sensitive_when_set() {
        let mut e = entry("HTN", "hypertension");
        e.case_sensitive = true;
        let entries = vec![e];

        let result = apply_corrections("patient has htn and HTN", &entries);
        assert_eq!(result.corrected_text, "patient has htn and hypertension");
        assert_eq!(result.total_replacements, 1);
    }

    #[test]
    fn higher_priority_applied_first() {
        let mut e1 = entry("cp", "chest pain");
        e1.priority = 0;
        let mut e2 = entry("sob", "shortness of breath");
        e2.priority = 10;

        let entries = vec![e1, e2];
        let result = apply_corrections("patient reports sob and cp", &entries);
        assert_eq!(result.corrected_text, "patient reports shortness of breath and chest pain");
        assert_eq!(result.total_replacements, 2);
        assert_eq!(result.corrections_applied[0].find_text, "sob");
        assert_eq!(result.corrections_applied[1].find_text, "cp");
    }

    #[test]
    fn longer_match_before_shorter_at_same_priority() {
        let e1 = entry("dm", "diabetes mellitus");
        let e2 = entry("dm type 2", "diabetes mellitus type 2");
        let entries = vec![e1, e2];
        let result = apply_corrections("patient has dm type 2", &entries);
        assert_eq!(result.corrected_text, "patient has diabetes mellitus type 2");
    }

    #[test]
    fn disabled_entries_skipped() {
        let mut e = entry("htn", "hypertension");
        e.enabled = false;
        let entries = vec![e];
        let result = apply_corrections("patient has htn", &entries);
        assert_eq!(result.corrected_text, "patient has htn");
        assert_eq!(result.total_replacements, 0);
    }

    #[test]
    fn multiple_occurrences_counted() {
        let entries = vec![entry("htn", "hypertension")];
        let result = apply_corrections("htn noted, also htn related issues", &entries);
        assert_eq!(result.corrected_text, "hypertension noted, also hypertension related issues");
        assert_eq!(result.total_replacements, 2);
        assert_eq!(result.corrections_applied[0].count, 2);
    }

    #[test]
    fn multiple_different_corrections() {
        let entries = vec![
            entry("htn", "hypertension"),
            entry("dm", "diabetes mellitus"),
        ];
        let result = apply_corrections("patient has htn and dm", &entries);
        assert_eq!(result.corrected_text, "patient has hypertension and diabetes mellitus");
        assert_eq!(result.total_replacements, 2);
        assert_eq!(result.corrections_applied.len(), 2);
    }

    #[test]
    fn preserves_original_text() {
        let entries = vec![entry("htn", "hypertension")];
        let result = apply_corrections("patient has htn", &entries);
        assert_eq!(result.original_text, "patient has htn");
        assert_eq!(result.corrected_text, "patient has hypertension");
    }
}
