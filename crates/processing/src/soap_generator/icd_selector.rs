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

use std::collections::{HashMap, HashSet};

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

    // 2. Score entries using the pre-computed inverted index (token → entry
    //    indices). Instead of iterating all 7,122 entries and re-tokenizing
    //    each description (the old O(n×tokens) approach), we look up each
    //    source token in the index and score only the entries it appears in.
    //    This reduces the scoring loop from ~7,122 iterations to typically
    //    ~50-200 candidate entries.
    let entries = icd9::entries();
    let desc_sets = icd9::desc_token_sets();
    let token_index = icd9::token_index();

    // Collect candidate entry indices: the union of all entries that share
    // at least one token with the source, PLUS any entries whose bare code
    // is mentioned verbatim in the source (the code-mention bonus path —
    // these may have no description-token overlap but should still score).
    let mut candidate_indices: HashSet<usize> = HashSet::new();
    for token in &source_set {
        if let Some(indices) = token_index.get(token) {
            candidate_indices.extend(indices.iter().copied());
        }
    }
    // Add entries whose code appears verbatim in the source. The
    // tokenization splits dotted codes (786.5 → "786" + "5"), so we scan
    // the source for code-like substrings directly and look them up in
    // the O(1) code index. Only distinctive codes (dotted or alpha) are
    // worth checking — bare 3-digit codes collide with lab values.
    let code_index = icd9::code_index();
    // Extract code-like tokens: substrings matching \d{3}\.\d or [VE]\d+\.\d
    // from the lowercased source. This is a lightweight scan, not the full
    // per-entry code_mentioned regex.
    for m in regex::Regex::new(r"(?P<code>(?:\d{3}\.\d+[a-z]?|[ve]\d+\.\d+))")
        .expect("code-like regex compiles")
        .find_iter(&source_lower)
    {
        let code = m.as_str().to_uppercase();
        if let Some(entry) = code_index.get(&code)
            && let Some(idx) = entries.iter().position(|e| std::ptr::eq(e, *entry))
        {
            candidate_indices.insert(idx);
        }
    }

    let mut scored: Vec<(usize, &'static Icd9Entry)> = Vec::new();
    for idx in candidate_indices {
        let entry = &entries[idx];
        let desc_set = &desc_sets[idx];

        let overlap = source_set.intersection(desc_set).count();
        let mut score = overlap;

        // Bonus: the bare code is mentioned verbatim in the source text.
        // Use a word-boundary match so bare 3-digit numeric codes (130, 250,
        // 401) don't false-positive on lab/dose values (e.g. "glucose 130").
        // Dotted codes (786.5) and V/alpha codes (V70.0, 01A) are
        // distinctive enough that a boundary match is reliable.
        if code_mentioned(&entry.code, &source_lower) {
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
    // Build a quick lookup from scored entries for baseline score retrieval.
    let scored_map: HashMap<&str, usize> =
        scored.iter().map(|(s, e)| (e.code.as_str(), *s)).collect();
    for code in PRIMARY_CARE_BASELINE {
        if let Some(entry) = icd9::find_by_code(code)
            && seen_codes.insert(entry.code.clone())
        {
            let score = scored_map
                .get(entry.code.as_str())
                .copied()
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

/// Tests whether the bare ICD-9 code appears as a word-boundary match in
/// the lowercased source text.
///
/// This guards against false positives where a bare 3-digit numeric code
/// (e.g. `130` = toxoplasmosis) would match lab/dose values like
/// "glucose 130" or "metformin 250 mg". Even with word boundaries, a
/// bare 3-digit number is indistinguishable from a clinical value, so
/// the bonus is only applied to codes that are **distinctive enough**
/// to be unambiguous when mentioned verbatim:
/// - ≥4 characters, OR
/// - contains a dot (`786.5`, `401.9`), OR
/// - contains a letter (`V70.0`, `01A`).
///
/// Bare 3-digit codes (130, 250, 401) never get the bonus — they rely
/// entirely on description-token overlap for scoring.
fn code_mentioned(code: &str, source_lower: &str) -> bool {
    if code.is_empty() {
        return false;
    }
    // Distinctiveness gate: skip bare short numeric codes that collide
    // with lab/dose values.
    let has_dot = code.contains('.');
    let has_letter = code.chars().any(|c| c.is_ascii_alphabetic());
    let long_enough = code.len() >= 4;
    if !(has_dot || has_letter || long_enough) {
        return false;
    }
    let code_lower = code.to_lowercase();
    // Word-boundary match without compiling a regex per call. Find the
    // code in the source, then check the chars before/after are
    // non-alphanumeric (or string boundaries).
    let mut search_from = 0;
    while let Some(pos) = source_lower[search_from..].find(&code_lower) {
        let abs = search_from + pos;
        let before_ok = abs == 0
            || !source_lower
                .as_bytes()
                .get(abs - 1)
                .is_some_and(is_word_char);
        let after = abs + code_lower.len();
        let after_ok = after >= source_lower.len()
            || !source_lower.as_bytes().get(after).is_some_and(is_word_char);
        if before_ok && after_ok {
            return true;
        }
        search_from = abs + 1;
    }
    false
}

/// Returns true if the byte is an alphanumeric "word" character (for the
/// word-boundary check in [`code_mentioned`]).
fn is_word_char(b: &u8) -> bool {
    b.is_ascii_alphanumeric()
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

    // ---- Numeric code-bonus false-positive guard (F2) ----

    #[test]
    fn numeric_lab_value_does_not_score_bare_numeric_code() {
        // "130" (toxoplasmosis) must NOT get the +3 bonus from a glucose
        // reading of 130 mg/dL. A bare-3-digit-code substring match would
        // have scored it before the word-boundary guard was added.
        let transcript = "Patient reports glucose 130 this morning, weight 250 pounds.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        // 130 (toxoplasmosis) is not in the baseline and should not be
        // scored in by the numeric value.
        assert!(
            !codes.contains(&"130"),
            "glucose 130 must not score toxoplasmosis (130): {codes:?}"
        );
    }

    // ---- Baseline-survives-cap guarantee ----

    #[test]
    fn baseline_codes_survive_cap_under_heavy_transcript_scoring() {
        // A transcript that scores many non-baseline entries must NOT push
        // baseline codes out of the 40-slot result. The baseline is the
        // billing-critical floor (V70.0 etc. must always be available).
        let transcript = "fever headache cough sore throat sinus pain congestion wheeze dyspnea chest pain abdominal pain nausea rash itching dizziness fatigue malaise myalgia arthralgia back pain";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: HashSet<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        // V70.0 is the last baseline entry by file order and the most likely
        // to be starved if the cap logic regresses.
        assert!(
            codes.contains("V70.0"),
            "V70.0 must survive the cap even under heavy scoring: {:?}",
            codes
        );
        // Spot-check a few other high-frequency baselines.
        assert!(codes.contains("401.9"), "HTN baseline survives: {codes:?}");
        assert!(
            codes.contains("784.0"),
            "headache baseline survives: {codes:?}"
        );
        assert_eq!(
            selected.len(),
            MAX_CANDIDATES,
            "heavy transcript fills all slots"
        );
    }

    // ---- code_mentioned V/alpha distinctiveness branch ----

    #[test]
    fn v_code_mentioned_verbatim_scores_via_bonus() {
        // V70.0 contains a letter → passes the distinctiveness gate → the
        // verbatim mention gets the +3 bonus. V70.0 is in the baseline so
        // it appears regardless; this test confirms the BONUS path works
        // by checking it ranks high (the bonus would be invisible in a
        // pure-membership check).
        // Use a non-baseline V-code to make the bonus contribution visible.
        // V72.0 (eye exam) — not in the baseline.
        let transcript = "Patient here for a V72.0 vision screening.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        // V72.0 should be selected via the verbatim-mention bonus (it has
        // no description-token overlap with "vision screening" because the
        // description is "EXAMINATION OF EYES AND VISION").
        assert!(
            codes.contains(&"V72.0"),
            "V72.0 mentioned verbatim should be selected via bonus: {codes:?}"
        );
    }

    // ---- Bare-3-digit code scored via overlap (not bonus) ----

    #[test]
    fn bare_three_digit_code_scored_via_overlap_not_bonus() {
        // "diagnosis 250" — 250 (DM) is in the baseline and its description
        // is "DIABETES MELLITUS". The transcript has "250" but F2 blocks
        // the bonus for bare 3-digit codes; 250.0 (in baseline) is still
        // selected via baseline membership. This confirms the F2 guard
        // doesn't accidentally drop valid baseline codes.
        let transcript = "Follow up on diagnosis 250, diabetes management.";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"250.0"),
            "250.0 (DM) should still be selected (via baseline/overlap): {codes:?}"
        );
    }

    // ---- Edge cases ----

    #[test]
    fn empty_conditions_in_patient_context_does_not_panic() {
        let pc = PatientContext {
            patient_name: None,
            prior_soap_notes: vec![],
            medications: vec![],
            conditions: vec![], // empty
            allergies: vec![],
        };
        let selected = select_icd9_candidates("routine visit", None, Some(&pc));
        assert!(
            !selected.is_empty(),
            "empty conditions should not panic or blank out"
        );
    }

    #[test]
    fn tokenize_handles_non_ascii_without_panic() {
        // Non-ASCII (accented patient name / clinical term) must tokenize
        // without panicking. char::is_alphanumeric is Unicode-aware so
        // "café" is one token; it won't match ASCII descriptions but
        // must not crash.
        let tokens = tokenize("Patient named café reports hypertension");
        assert!(
            tokens.iter().any(|t| t.contains("hypertension")),
            "ASCII terms kept"
        );
    }
}
