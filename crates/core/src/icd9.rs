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

use std::collections::{BTreeSet, HashMap, HashSet};
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

/// All 7,122 ICD-9 entries, parsed once from the bundled JSON.
///
/// The MSP source uses hierarchical descriptions: child codes (e.g.
/// `847.2`) often carry only a terse fragment ("LUMBAR") that is only
/// meaningful alongside the parent code (`847` = "SPRAINS AND STRAINS
/// OF OTHER AND UNSPECIFIED PARTS OF BACK"). These fragments are stored
/// verbatim — an earlier version prepended the parent description, but
/// that pushed the clinically-meaningful fragment past the prompt's
/// truncation budget (see [`ENTRIES`] for the full rationale).
/// All 7,122 ICD-9 entries, parsed once from the bundled JSON.
///
/// Descriptions are stored verbatim from the MSP source. The MSP file
/// uses hierarchical fragments for child codes (e.g. `847.2` carries
/// only `"LUMBAR"`, meaningful alongside parent `847` = "SPRAINS AND
/// STRAINS..."). An earlier version prepended the parent description,
/// but that pushed the clinically-meaningful fragment past the prompt's
/// 57-char truncation (e.g. `041.2` "PNEUMOCOCCUS" became "BACTERIAL
/// INFECTION IN CONDITIONS CLASSIFIED ELSEWHERE AN…"), corrupting 880
/// codes. The raw fragments are kept — they are terse but never
/// truncated, and the LLM generally knows ICD-9 structure well enough
/// to select from them.
static ENTRIES: LazyLock<Vec<Icd9Entry>> = LazyLock::new(|| {
    let file: Icd9File = serde_json::from_str(ICD9_JSON).expect("icd9_codes.json must parse");
    file.codes
});

/// Set of bare codes for O(1) membership tests. Uses the code string
/// verbatim — callers must normalize before lookup if they are stripping
/// leading zeros (see [`normalize_code`]).
static CODE_SET: LazyLock<BTreeSet<String>> =
    LazyLock::new(|| ENTRIES.iter().map(|e| e.code.clone()).collect());

/// HashMap index: code → entry, for O(1) [`find_by_code`] lookups.
/// Replaces the previous O(n) linear scan that ran per baseline code
/// and per frontend-validated code.
static CODE_INDEX: LazyLock<HashMap<String, &'static Icd9Entry>> =
    LazyLock::new(|| ENTRIES.iter().map(|e| (e.code.clone(), e)).collect());

/// Pre-tokenized (lowercased) description token sets, one per entry.
/// Avoids re-tokenizing all 7,122 descriptions on every SOAP generation.
/// Allocated once; the selector reads these without allocation.
static DESC_TOKEN_SETS: LazyLock<Vec<HashSet<String>>> = LazyLock::new(|| {
    ENTRIES
        .iter()
        .map(|e| tokenize_desc(&e.description))
        .collect()
});

/// Inverted index: lowercased description token → indices of entries
/// whose description contains that token. Lets the selector score only
/// the entries that share at least one token with the source text,
/// instead of iterating all 7,122. ~100x reduction in scoring work.
static TOKEN_INDEX: LazyLock<HashMap<String, Vec<usize>>> = LazyLock::new(|| {
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, tokens) in DESC_TOKEN_SETS.iter().enumerate() {
        for token in tokens {
            map.entry(token.clone()).or_default().push(i);
        }
    }
    map
});

/// Returns all ICD-9 entries in source order.
pub fn entries() -> &'static [Icd9Entry] {
    &ENTRIES
}

/// Returns the set of all bare codes (for membership tests).
pub fn code_set() -> &'static BTreeSet<String> {
    &CODE_SET
}

/// Returns the code→entry HashMap index for O(1) lookups.
pub fn code_index() -> &'static HashMap<String, &'static Icd9Entry> {
    &CODE_INDEX
}

/// Returns the pre-tokenized description token sets (one per entry,
/// indexed in parallel with [`entries()`]).
pub fn desc_token_sets() -> &'static [HashSet<String>] {
    &DESC_TOKEN_SETS
}

/// Returns the inverted index (token → entry indices).
pub fn token_index() -> &'static HashMap<String, Vec<usize>> {
    &TOKEN_INDEX
}

/// Tokenize a description into a set of lowercased tokens (no stopwords,
/// no tokens <3 chars). Public so the selector reuses it for source text.
pub fn tokenize_desc(desc: &str) -> HashSet<String> {
    desc.split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 3)
        .map(|s| s.to_lowercase())
        .collect()
}

/// Looks up a single entry by exact bare-code match (O(1) via the
/// [`CODE_INDEX`] HashMap).
pub fn find_by_code(code: &str) -> Option<&'static Icd9Entry> {
    CODE_INDEX.get(code).copied()
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
    let padded = if let Some(dot) = trimmed.find('.') {
        let (int_part, rest) = trimmed.split_at(dot);
        int_part
            .parse::<u32>()
            .map(|n| format!("{n:03}{rest}"))
            .ok()
    } else {
        trimmed.parse::<u32>().map(|n| format!("{n:03}")).ok()
    };
    // Only push the padded form when it actually differs — avoids
    // duplicate entries like ["780","780"] for already-3-digit codes.
    if let Some(p) = padded
        && p != trimmed
    {
        forms.push(p);
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
        // Raw MSP fragment — enrichment was removed (it corrupted 880 codes
        // by pushing the fragment past prompt truncation).
        assert_eq!(e.description, "LUMBAR");
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
    fn normalized_forms_zero_pads_integer_no_dot() {
        // The else branch: a pure-integer code with no decimal point.
        // "42" → ["42", "042"]. This mirrors the TS port's behavior.
        let forms = normalized_forms("42");
        assert!(
            forms.contains(&"042".to_string()),
            "42 must zero-pad to 042: {forms:?}"
        );
        assert!(forms.contains(&"42".to_string()), "original form retained");
    }

    #[test]
    fn normalized_forms_dedups_already_padded_codes() {
        // "780" is already 3 digits → the padded form equals the original
        // → no duplicate entry (F3 dedup guard).
        let forms = normalized_forms("780");
        assert_eq!(
            forms.len(),
            1,
            "already-3-digit code must not produce a duplicate: {forms:?}"
        );
        assert_eq!(forms[0], "780");
    }

    #[test]
    fn normalized_forms_handles_empty_and_whitespace() {
        assert_eq!(normalized_forms(""), vec!["".to_string()]);
        assert_eq!(normalized_forms("   "), vec!["".to_string()]);
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
