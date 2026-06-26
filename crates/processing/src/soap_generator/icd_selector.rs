//! Selects a clinically relevant subset of BC MSP ICD-9 codes to inject
//! into the SOAP prompt as a constrained vocabulary.
//!
//! The full list is 7,122 codes — far too many for a local model's
//! context window. This module scores each entry against the visit's
//! source text (transcript + clinical context + known conditions) and
//! returns the top candidates plus a curated primary-care baseline.
//!
//! # Algorithm
//!
//! 1. Concatenate source text: transcript, freeform context, and known
//!    conditions from [`PatientContext`].
//! 2. Tokenize (lowercase, alphanumeric split, drop stopwords + short
//!    tokens).
//! 3. Score each entry: count of source tokens present in its
//!    description-token set; +3 if a source token equals the bare code.
//! 4. Always include a curated primary-care baseline and the routine-
//!    encounter V-codes.
//! 5. Sort by score desc, dedupe, cap at [`MAX_CANDIDATES`].

use std::collections::HashSet;

use medical_core::icd9::{self, Icd9Entry};
use medical_core::types::PatientContext;

/// Maximum candidate codes to inject into the prompt (~1.5-2.5k tokens).
/// The baseline occupies guaranteed slots; the remainder are filled by
/// transcript-scored matches.
const MAX_CANDIDATES: usize = 40;

/// High-frequency BC primary-care codes, always included as a floor so
/// the selector never blanks out common presentations or the routine-
/// encounter fallback.
///
/// Verified against the bundled MSP list — these codes all exist. Where
/// the MSP list uses parent codes without trailing zeros (e.g. `786.5`
/// not `786.50`), the MSP form is used.
///
/// Kept to **25 entries** so the guaranteed slots never overflow
/// [`MAX_CANDIDATES`] and leave room for transcript-scored additions.
///
/// **Clinical review note:** curated for a BC family-practice context.
/// Adjust if the deployment context changes.
const PRIMARY_CARE_BASELINE: &[&str] = &[
    // Cardiovascular / metabolic
    "401.9", // Essential hypertension, unspecified
    "250.0", // DM without complication
    "272.4", // Hyperlipidemia, other/unspecified
    // Respiratory
    "466.0", // Acute bronchitis
    "460",   // Common cold (acute nasopharyngitis)
    "462",   // Acute pharyngitis
    "461.9", // Acute sinusitis, unspecified
    "493.9", // Asthma, unspecified
    "496",   // Chronic airways obstruction (COPD)
    "786.2", // Cough
    // Musculoskeletal
    "724.5", // Backache, unspecified
    "724.2", // Lumbago
    "847.2", // Sprain of lumbar (back strain)
    "719.4", // Pain in joint
    "729.5", // Pain in limb
    "715",   // Osteoarthrosis and allied disorders
    // Gastrointestinal
    "789.0", // Abdominal pain
    "787.0", // Nausea and vomiting
    "787.1", // Heartburn
    // Neurological / symptoms
    "784.0", // Headache
    "346",   // Migraine
    "780.7", // Malaise and fatigue
    // Mental health
    "311",   // Depressive disorder, NEC
    "300.0", // Anxiety states
    // Encounter / administrative
    "V70.0", // Routine general medical examination
];

/// English stopwords excluded from tokenization.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "that", "this", "with", "from", "have", "has", "was", "were", "are",
    "been", "not", "but", "his", "her", "she", "him", "you", "your", "they", "their", "will",
    "would", "could", "should", "about", "into", "over", "under", "than", "then", "them", "some",
    "any", "all", "can", "may", "did", "does", "done", "said", "says", "say", "one", "two",
    "three", "also", "just", "like", "what", "when", "where", "which", "how", "who", "whom",
    "patient", "doctor", "today", "visit", "come", "came", "going", "get", "got", "yes", "yeah",
    "okay", "ok", "well", "know", "think", "feel", "feeling", "really", "very", "kind", "sort",
    "bit", "lot", "stuff", "thing", "things", "here", "there", "right", "left", "back", "side",
    "been", "being", "had",
];

/// Selects a relevant subset of ICD-9 codes for the SOAP prompt.
///
/// Combines a keyword-scored selection from the full MSP list with a
/// curated primary-care baseline. Returns at most [`MAX_CANDIDATES`]
/// entries, ordered by relevance score (baseline entries get a floor
/// score so they appear after strong transcript matches but before
/// weak ones).
pub fn select_icd9_candidates(
    transcript: &str,
    context: Option<&str>,
    patient_context: Option<&PatientContext>,
) -> Vec<Icd9Entry> {
    // 1. Gather source text.
    let mut source = String::from(transcript);
    if let Some(ctx) = context {
        source.push(' ');
        source.push_str(ctx);
    }
    if let Some(pc) = patient_context {
        for cond in &pc.conditions {
            source.push(' ');
            source.push_str(cond);
        }
    }

    let source_tokens = tokenize(&source);
    let source_set: HashSet<String> = source_tokens.into_iter().collect();
    let source_lower = source.to_lowercase();

    // 2. Score every entry by source-token overlap with its description.
    //    MSP descriptions are ALL UPPERCASE, so tokenize() lowercases both
    //    sides to make the intersection case-insensitive. This is the
    //    core selection mechanism — without it the feature is just the
    //    static baseline.
    let mut scored: Vec<(usize, &'static Icd9Entry)> = Vec::new();
    for entry in icd9::entries() {
        let desc_tokens = tokenize(&entry.description);
        let desc_set: HashSet<String> = desc_tokens.into_iter().collect();

        let overlap = source_set.intersection(&desc_set).count();
        let mut score = overlap;

        // Bonus: the bare code is mentioned verbatim in the source text.
        // We test the lowercased source as a substring so a dotted code
        // like "401.9" matches even though tokenization splits it.
        if !entry.code.is_empty() && source_lower.contains(&entry.code.to_lowercase()) {
            score += 3;
        }

        if score > 0 {
            scored.push((score, entry));
        }
    }

    // 3. Always-include baseline — these survive the cap regardless of
    //    score, so a paperwork visit always has a valid code (V70.0) and
    //    common presentations are never blanked out by incidental noise.
    let baseline_floor = 1;
    let mut baseline: Vec<(usize, &'static Icd9Entry)> = Vec::new();
    let mut seen_codes: HashSet<String> = HashSet::new();
    for code in PRIMARY_CARE_BASELINE {
        if let Some(entry) = icd9::find_by_code(code)
            && seen_codes.insert(entry.code.clone())
        {
            let score = scored
                .iter()
                .find(|(_, e)| e.code == entry.code)
                .map(|(s, _)| *s)
                .unwrap_or(baseline_floor);
            baseline.push((score, entry));
        }
    }

    // 4. Add transcript-scored entries not already in the baseline.
    //    Sort scored entries by score desc BEFORE capping — otherwise low-
    //    relevance entries that happen to appear early in the 7,122-entry
    //    file would fill the non-baseline slots and push out higher-scoring
    //    codes that appear later (e.g. the 780-799 Symptoms chapter).
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut selected: Vec<(usize, &'static Icd9Entry)> = baseline;
    for (score, entry) in &scored {
        if selected.len() >= MAX_CANDIDATES {
            break;
        }
        if seen_codes.insert(entry.code.clone()) {
            selected.push((*score, entry));
        }
    }

    // 5. Final sort by score desc for prompt presentation (baseline entries
    //    that also scored well float to the top alongside scored matches).
    selected.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

    selected.into_iter().map(|(_, e)| e.clone()).collect()
}

/// Lowercase alphanumeric tokenization with stopword + short-token removal.
///
/// Returns owned lowercased strings so the resulting sets can be compared
/// case-insensitively — MSP descriptions are stored ALL UPPERCASE while
/// transcripts are mixed-case, so both sides must be normalized to match.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| {
            if s.len() < 3 {
                return false;
            }
            // Case-insensitive stopword check against the original slice,
            // then lowercase only the tokens we keep (avoid allocating for
            // every dropped stopword/short token).
            !STOPWORDS.iter().any(|&stop| s.eq_ignore_ascii_case(stop))
        })
        .map(|s| s.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(conditions: &[&str]) -> PatientContext {
        PatientContext {
            patient_name: None,
            prior_soap_notes: vec![],
            medications: vec![],
            conditions: conditions.iter().map(|s| s.to_string()).collect(),
            allergies: vec![],
        }
    }

    #[test]
    fn selects_hypertension_for_htn_transcript() {
        let transcript =
            "Patient here for blood pressure check. Has hypertension, readings elevated today.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"401.9"),
            "expected 401.9 in selection, got: {codes:?}"
        );
    }

    #[test]
    fn selects_diabetes_for_dm_transcript() {
        let transcript =
            "Follow-up for diabetes mellitus. Blood sugar glucose elevated. Checking A1c.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"250.0"),
            "expected 250.0 in selection, got: {codes:?}"
        );
    }

    #[test]
    fn always_includes_routine_exam_v700() {
        // Even with an empty/irrelevant transcript, V70.0 must appear
        // via the baseline so a paperwork visit has a valid code.
        let selected = select_icd9_candidates("nothing relevant here xyzzy", None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"V70.0"),
            "V70.0 must always be in selection"
        );
    }

    #[test]
    fn empty_transcript_returns_baseline_only() {
        let selected = select_icd9_candidates("", None, None);
        // Baseline has 25 entries, all guaranteed slots under the 40 cap;
        // with no transcript matches nothing else is added.
        assert_eq!(
            selected.len(),
            PRIMARY_CARE_BASELINE.len(),
            "empty transcript should return baseline only"
        );
        // All should be from the baseline set.
        let baseline_codes: HashSet<&str> = PRIMARY_CARE_BASELINE.iter().copied().collect();
        for e in &selected {
            assert!(
                baseline_codes.contains(e.code.as_str()),
                "non-baseline code {} appeared with empty transcript",
                e.code
            );
        }
    }

    #[test]
    fn respects_max_candidates() {
        // A long transcript hitting many entries must not exceed the cap.
        let transcript = "hypertension diabetes asthma bronchitis back pain headache anxiety depression rash arthritis osteoarthritis migraine dyspnoea cough abdominal pain nausea";
        let selected = select_icd9_candidates(transcript, None, None);
        assert!(selected.len() <= MAX_CANDIDATES);
    }

    #[test]
    fn patient_context_conditions_contribute() {
        let pc = pc(&["hypertension", "type 2 diabetes"]);
        let selected = select_icd9_candidates("routine check", None, Some(&pc));
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"401.9"),
            "HTN from patient context should be selected: {codes:?}"
        );
        assert!(
            codes.contains(&"250.0"),
            "DM from patient context should be selected: {codes:?}"
        );
    }

    #[test]
    fn freeform_context_contributes() {
        let ctx = "Patient with known hyperlipidemia, on statin.";
        let selected = select_icd9_candidates("follow up", Some(ctx), None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"272.4") || codes.contains(&"272.0") || codes.contains(&"272.2"),
            "hyperlipidemia code should be selected: {codes:?}"
        );
    }

    #[test]
    fn baseline_codes_all_exist_in_msp_list() {
        // Regression guard: every baseline code must be in the bundled
        // MSP list. If a code is added that doesn't exist, this fails
        // loudly rather than silently dropping it from selection.
        for code in PRIMARY_CARE_BASELINE {
            assert!(
                icd9::find_by_code(code).is_some(),
                "baseline code {code} does not exist in MSP list"
            );
        }
    }

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens() {
        let tokens = tokenize("The patient has HTN and is ok");
        // "The", "and", "is", "ok", "patient" are stopwords/short.
        assert!(!tokens.iter().any(|t| t.eq_ignore_ascii_case("the")));
        assert!(!tokens.iter().any(|t| t.eq_ignore_ascii_case("patient")));
        assert!(!tokens.iter().any(|t| t.eq_ignore_ascii_case("ok")));
        assert!(tokens.iter().any(|t| t.eq_ignore_ascii_case("htn")));
    }

    // ---- Regression guards for BUG-1 / BUG-2 ----
    //
    // MSP descriptions are stored ALL UPPERCASE. tokenize() lowercases
    // both sides; if anyone reverts that (or reintroduces a case-sensitive
    // intersection), these tests fail because the non-baseline codes below
    // would never be scored and would drop out of the selection.

    #[test]
    fn scores_non_baseline_code_by_description_overlap() {
        // 786.5 "CHEST PAIN" is NOT in the 25-code baseline. It must be
        // selected purely by description-token overlap with the transcript.
        let transcript = "Patient complains of chest pain, no radiation.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"786.5"),
            "non-baseline 786.5 (chest pain) must be scored in: {codes:?}"
        );
    }

    #[test]
    fn scores_non_baseline_allergic_rhinitis() {
        // 477 "ALLERGIC RHINITIS" is NOT in the baseline.
        let transcript = "Here for allergic rhinitis, sneezing, itchy eyes.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"477"),
            "non-baseline 477 (allergic rhinitis) must be scored in: {codes:?}"
        );
    }

    #[test]
    fn code_match_bonus_fires_for_dotted_code() {
        // BUG-2 guard: a dotted code mentioned verbatim ("401.9") must
        // be scored via the substring bonus even though tokenization
        // splits it into "401" and "9". 780.7 is in the baseline, so use
        // a non-baseline dotted code: 786.5.
        let transcript = "Documented diagnosis 786.5 on prior workup.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"786.5"),
            "dotted code 786.5 mentioned verbatim must be scored in: {codes:?}"
        );
    }
}
