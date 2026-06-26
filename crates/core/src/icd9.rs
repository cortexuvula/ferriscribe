//! BC MSP ICD-9 diagnostic code list, bundled at compile time.
//!
//! The canonical list of ICD-9 diagnostic codes accepted by BC's Medical
//! Services Plan for physician-claim billing. The data is embedded via
//! [`include_str!`] from `crates/core/icd9_codes.json` (sourced from the
//! Province of British Columbia) and parsed once into a [`LazyLock`].
//!
//! # Why this exists
//!
//! BC MSP still uses ICD-9 (not ICD-10) diagnostic codes for physician
//! billing. The SOAP generator constrains the LLM to this accepted list so
//! it selects from valid codes rather than fabricating from parametric
//! memory. The frontend separately validates any emitted code against the
//! same set and flags unknowns.
//!
//! # Layout
//!
//! The JSON keeps a `metadata` block (source attribution, Teleplan
//! submission note) for the compliance trail; only the `codes` array is
//! modeled at runtime.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde::Deserialize;

/// Bundled ICD-9 JSON ( Province of British Columbia, Medical Services Plan).
const ICD9_JSON: &str = include_str!("../icd9_codes.json");

/// A single ICD-9 diagnostic code entry.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Icd9Entry {
    /// Bare ICD-9 code as published (e.g. `"847.2"`, `"V70.0"`, `"01A"`).
    pub code: String,
    /// Official MSP description, uppercased in the source.
    pub description: String,
    /// ICD-9 chapter / MSP-additional group label.
    pub category: String,
}

#[derive(Debug, Deserialize)]
struct Icd9File {
    codes: Vec<Icd9Entry>,
}

/// All 7,122 ICD-9 entries, parsed once.
///
/// The MSP source uses hierarchical descriptions: child codes (e.g.
/// `847.2`) often carry only a terse fragment ("LUMBAR") that is only
/// meaningful alongside the parent code (`847` = "SPRAIN OF NECK AND
/// BACK"). At parse time each child's `description` is enriched to
/// `"<parent description>: <fragment>"` so keyword search and the prompt
/// both surface the full clinical meaning.
static ENTRIES: LazyLock<Vec<Icd9Entry>> = LazyLock::new(|| {
    let file: Icd9File = serde_json::from_str(ICD9_JSON).expect("icd9_codes.json must parse");
    enrich_descriptions(file.codes)
});

/// Merges terse child descriptions with their parent (the 3-digit code
/// immediately above, e.g. `847.2` → parent `847`).
///
/// Codes whose integer part has no 3-digit parent in the list, and V/E
/// codes, are left unchanged.
fn enrich_descriptions(mut codes: Vec<Icd9Entry>) -> Vec<Icd9Entry> {
    use std::collections::HashMap;
    // Collect parent descriptions into owned strings so we can mutate
    // `codes` afterward.
    let parent_desc: HashMap<String, String> = codes
        .iter()
        .filter(|e| {
            // Parents are bare 3-digit codes or V/E codes with no dot.
            !e.code.contains('.') && e.code.len() >= 3
        })
        .map(|e| (e.code.clone(), e.description.clone()))
        .collect();

    for entry in codes.iter_mut() {
        if let Some(dot) = entry.code.find('.') {
            let parent_code = &entry.code[..dot];
            if let Some(parent) = parent_desc.get(parent_code) {
                // Only enrich if the child has a terse fragment — skip if
                // it already reads as a complete description.
                if is_terse_fragment(&entry.description) {
                    entry.description = format!("{}: {}", parent, entry.description);
                }
            }
        }
    }
    codes
}

/// Heuristic: MSP fragments like "LUMBAR", "ACUTE", "UNSPECIFIED" are
/// short (1-2 words); enriched descriptions read as full phrases.
fn is_terse_fragment(desc: &str) -> bool {
    desc.split_whitespace().count() <= 2
}

/// Set of bare codes for O(1) membership tests. Uses the code string
/// verbatim — callers must normalize before lookup if they are stripping
/// leading zeros (see [`normalize_code`]).
static CODE_SET: LazyLock<BTreeSet<String>> =
    LazyLock::new(|| ENTRIES.iter().map(|e| e.code.clone()).collect());

/// Returns all ICD-9 entries in source order.
pub fn entries() -> &'static [Icd9Entry] {
    &ENTRIES
}

/// Returns the set of all bare codes (for membership tests).
pub fn code_set() -> &'static BTreeSet<String> {
    &CODE_SET
}

/// Looks up a single entry by exact bare-code match.
pub fn find_by_code(code: &str) -> Option<&'static Icd9Entry> {
    ENTRIES.iter().find(|e| e.code == code)
}

/// Normalizes a bare ICD-9 code for membership comparison.
///
/// The MSP list uses zero-padded numeric codes (`"001.0"`, `"042"`), but a
/// model may emit a trimmed form (`"1.0"`, `"42"`). Alpha-suffixed
/// MSP-additional codes (`"01A"`) and V-codes (`"V70.0"`) are returned
/// unchanged — only the integer portion before the first `.` is re-padded
/// to three digits for pure-numeric codes.
///
/// Returns the set of candidate forms to try (original + zero-padded) so
/// callers can test membership with any of them.
pub fn normalized_forms(code: &str) -> Vec<String> {
    let trimmed = code.trim();
    let mut forms = vec![trimmed.to_string()];
    // Pure-numeric codes: zero-pad the integer part to 3 digits.
    if let Some(dot) = trimmed.find('.') {
        let (int_part, rest) = trimmed.split_at(dot);
        if let Ok(n) = int_part.parse::<u32>() {
            forms.push(format!("{n:03}{rest}"));
        }
    } else if let Ok(n) = trimmed.parse::<u32>() {
        forms.push(format!("{n:03}"));
    }
    forms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_entries() {
        assert_eq!(entries().len(), 7122, "expected 7122 BC MSP ICD-9 codes");
    }

    #[test]
    fn code_set_matches_entries_count() {
        assert_eq!(code_set().len(), 7122);
    }

    #[test]
    fn finds_known_code() {
        let e = find_by_code("847.2").expect("847.2 must exist");
        // After enrichment: parent "SPRAIN OF NECK AND BACK: LUMBAR".
        assert!(e.description.contains("LUMBAR"));
        assert!(e.description.contains("SPRAIN"));
    }

    #[test]
    fn finds_v_code() {
        // V70.0 = routine general medical examination (the prompt's
        // routine-encounter fallback).
        assert!(find_by_code("V70.0").is_some(), "V70.0 must exist");
    }

    #[test]
    fn finds_alpha_suffix_code() {
        // 01A is a BC MSP-additional code (dizziness/vertigo/insomnia).
        assert!(find_by_code("01A").is_some(), "01A must exist");
    }

    #[test]
    fn missing_code_returns_none() {
        assert!(find_by_code("999.999").is_none());
    }

    #[test]
    fn normalized_forms_zero_pads_numeric() {
        let forms = normalized_forms("1.0");
        assert!(forms.contains(&"001.0".to_string()));
    }

    #[test]
    fn normalized_forms_preserves_alpha_suffix() {
        let forms = normalized_forms("01A");
        assert!(forms.contains(&"01A".to_string()));
        assert_eq!(forms.len(), 1, "alpha codes should not be re-padded");
    }

    #[test]
    fn normalized_forms_preserves_v_code() {
        let forms = normalized_forms("V70.0");
        assert!(forms.contains(&"V70.0".to_string()));
        assert_eq!(forms.len(), 1);
    }
}
