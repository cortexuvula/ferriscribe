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
use std::sync::LazyLock;

use medical_core::icd9::{self, Icd9Entry};
use medical_core::types::PatientContext;

/// Maximum candidate codes to inject into the prompt (~1.5-2.5k tokens).
/// The baseline occupies guaranteed slots; the remainder are filled by
/// transcript-scored matches.
const MAX_CANDIDATES: usize = 40;

/// Compiled once — matches code-like substrings (dotted numeric or V/E codes)
/// in the lowercased source text. Previously recompiled on every SOAP generation.
static CODE_LIKE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?:\d{3}\.\d+[a-z]?|[ve]\d+\.\d+)").expect("code-like regex compiles")
});

/// Single alternation regex matching any clinical abbreviation with word
/// boundaries, case-insensitive — "mi", "MI", "Mi" all expand. One pass over
/// the source replaces all 20 abbreviations at once (the previous
/// per-abbreviation loop copied the full source once per pattern — ~20 ×
/// source-length allocations).
static ABBREV_ALTERNATION: LazyLock<regex::Regex> = LazyLock::new(|| {
    let alts: Vec<String> = CLINICAL_ABBREVIATIONS
        .iter()
        .map(|(abbr, _)| regex::escape(abbr))
        .collect();
    regex::Regex::new(&format!(r"(?i)\b({})\b", alts.join("|")))
        .expect("abbreviation alternation compiles")
});

/// Lowercased abbreviation → expansion lookup used by the alternation's
/// replacement closure.
static ABBREV_EXPANSIONS: LazyLock<HashMap<String, &'static str>> = LazyLock::new(|| {
    CLINICAL_ABBREVIATIONS
        .iter()
        .map(|(abbr, exp)| (abbr.to_lowercase(), *exp))
        .collect()
});

/// Clinical abbreviations expanded before tokenization so the selector can
/// match them against ICD-9 description tokens. These are common 1-2 letter
/// abbreviations that the <3-char token filter would otherwise drop.
const CLINICAL_ABBREVIATIONS: &[(&str, &str)] = &[
    ("MI", "myocardial infarction"),
    ("CP", "chest pain"),
    ("DM", "diabetes mellitus"),
    ("UTI", "urinary tract infection"),
    ("GERD", "gastroesophageal reflux"),
    ("COPD", "chronic obstructive pulmonary disease"),
    ("CHF", "congestive heart failure"),
    ("CKD", "chronic kidney disease"),
    ("AKI", "acute kidney injury"),
    ("AFib", "atrial fibrillation"),
    ("BPH", "benign prostatic hypertrophy"),
    ("DVT", "deep venous thrombosis"),
    ("PE", "pulmonary embolism"),
    ("TIA", "transient ischemic attack"),
    ("OA", "osteoarthritis"),
    ("MSK", "musculoskeletal"),
    ("URI", "upper respiratory infection"),
    ("PUD", "peptic ulcer disease"),
    ("IBS", "irritable bowel syndrome"),
    ("IBD", "inflammatory bowel disease"),
];

/// High-frequency BC primary-care codes, always included as a floor so
/// the selector never blanks out common presentations.
///
/// Verified against the bundled MSP list — these codes all exist. Where
/// the MSP list uses parent codes without trailing zeros (e.g. `786.5`
/// not `786.50`), the MSP form is used.
///
/// Kept to **29 entries** so the guaranteed slots never overflow
/// [`MAX_CANDIDATES`] and leave room for transcript-scored additions.
///
/// **Clinical review note:** curated for a BC family-practice context.
/// V70.0 (routine exam) is intentionally NOT in the baseline — MSP
/// prefers specific codes. Screening/preventive codes are included
/// instead so the selector surfaces them for wellness visits. Adjust
/// if the deployment context changes.
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
    "723.1", // Cervicalgia (neck pain)
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
    // Screening / preventive (V70.0 intentionally omitted — MSP prefers
    // specific codes; these surface for wellness/screening visits)
    "V77.0", // Special screening for thyroid disorders
    "V77.1", // Special screening for diabetes mellitus
    "V16.0", // Family history of malignant neoplasm, GI tract
    "V17.3", // Family history of ischaemic heart disease
];

/// English stopwords excluded from tokenization.
/// Note: "back", "side", "left", "right" are intentionally KEPT — they
/// carry clinical meaning (back pain, right-sided weakness, left arm).
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "that", "this", "with", "from", "have", "has", "was", "were", "are",
    "been", "not", "but", "his", "her", "she", "him", "you", "your", "they", "their", "will",
    "would", "could", "should", "about", "into", "over", "under", "than", "then", "them", "some",
    "any", "all", "can", "may", "did", "does", "done", "said", "says", "say", "one", "two",
    "three", "also", "just", "like", "what", "when", "where", "which", "how", "who", "whom",
    "patient", "doctor", "today", "visit", "come", "came", "going", "get", "got", "yes", "yeah",
    "okay", "ok", "well", "know", "think", "feel", "feeling", "really", "very", "kind", "sort",
    "bit", "lot", "stuff", "thing", "things", "here", "there",
    // "right", "left", "back", "side" intentionally KEPT — clinically meaningful
    "being", "had",
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

    // Single lowercased buffer feeds everything downstream: tokenization
    // (the abbreviation regex is (?i) and tokens are lowercased anyway),
    // the bounded-run set for code mentions, and the code-like scan. No
    // second full-source copy.
    let source_lower = source.to_lowercase();
    let source_set: HashSet<String> = tokenize(&source_lower).into_iter().collect();
    let mentioned_runs = extract_bounded_runs(&source_lower);

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
    // the O(1) code→index map.
    let code_to_idx = icd9::code_to_idx();
    for m in CODE_LIKE_RE.find_iter(&source_lower) {
        let code = m.as_str().to_uppercase();
        if let Some(&idx) = code_to_idx.get(&code) {
            candidate_indices.insert(idx);
        }
    }

    let mut scored: Vec<(usize, &'static Icd9Entry)> = Vec::new();
    for idx in candidate_indices {
        let entry = &entries[idx];
        let desc_set = &desc_sets[idx];

        let overlap = source_set.intersection(desc_set).count();
        let mut score = overlap as i32;

        // Bonus: the bare code is mentioned verbatim in the source text.
        // Use a word-boundary match so bare 3-digit numeric codes (130, 250,
        // 401) don't false-positive on lab/dose values (e.g. "glucose 130").
        // Dotted codes (786.5) and V/alpha codes (V70.0, 01A) are
        // distinctive enough that a boundary match is reliable.
        if code_mentioned(&entry.code, &mentioned_runs) {
            score += 3;
        }

        // Specificity adjustment: MSP billing prefers specific codes.
        // Boost specific codes (4-5 digit, V-screening) and penalize
        // non-specific codes (V70.x routine exam) so they rank lower.
        score += specificity_adjustment(&entry.code);

        if score > 0 {
            scored.push((score as usize, entry));
        }
    }

    // 3. Always-include baseline — these survive the cap regardless of
    //    score, so common presentations are never blanked out by
    //    incidental noise.
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
/// Before tokenizing, expands common clinical abbreviations (MI → myocardial
/// infarction, CP → chest pain, etc.) so 1-2 letter abbreviations that the
/// <3-char filter would drop can still match ICD-9 descriptions.
///
/// Returns owned lowercased strings so the resulting sets can be compared
/// case-insensitively — MSP descriptions are stored ALL UPPERCASE while
/// transcripts are mixed-case, so both sides must be normalized to match.
fn tokenize(text: &str) -> Vec<String> {
    // Expand clinical abbreviations in a single pass before tokenizing.
    // This replaces short tokens like "MI" with their full form so the
    // <3-char filter doesn't drop them and they can match description
    // tokens. The single-alternation regex borrows the input when nothing
    // matches — no per-pattern full-source copies.
    let expanded = ABBREV_ALTERNATION.replace_all(text, |caps: &regex::Captures| {
        // The alternation only matches listed abbreviations, so the lookup
        // always succeeds; the identity fallback is purely defensive.
        ABBREV_EXPANSIONS
            .get(caps[1].to_lowercase().as_str())
            .map(|expansion| expansion.to_string())
            .unwrap_or_else(|| caps[1].to_string())
    });

    expanded
        .split(|c: char| !c.is_alphanumeric())
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

/// Extract every maximal `[A-Za-z0-9.]+` run from the lowercased source,
/// with leading/trailing `.` trimmed, lowercased.
///
/// This is exactly the set of strings that can occur in the source with
/// non-word characters on both sides — i.e. everything the previous
/// per-candidate substring+boundary scan in [`code_mentioned`] could ever
/// find. Building it once per generation turns the candidate loop's
/// membership check from O(candidates × source_length) scans into O(1)
/// set lookups. Dots stay INSIDE runs (a dotted code like `786.5` must not
/// be split) but are trimmed at the edges, mirroring the old boundary rule
/// where `.` counts as a non-word character.
fn extract_bounded_runs(source_lower: &str) -> HashSet<String> {
    fn flush(current: &mut String, runs: &mut HashSet<String>) {
        let trimmed = current.trim_matches('.');
        if !trimmed.is_empty() {
            runs.insert(trimmed.to_string());
        }
        current.clear();
    }

    let mut runs = HashSet::new();
    let mut current = String::new();
    for ch in source_lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' {
            current.push(ch);
        } else if !current.is_empty() {
            flush(&mut current, &mut runs);
        }
    }
    if !current.is_empty() {
        flush(&mut current, &mut runs);
    }
    runs
}

/// Tests whether the bare ICD-9 code appears as a word-boundary match in
/// the source text, via the pre-extracted [`extract_bounded_runs`] set.
///
/// This guards against false positives where a bare 3-digit numeric code
/// (e.g. `130` = toxoplasmosis) would match lab/dose values like
/// "glucose 130" or "metformin 250 mg". Even with word boundaries, a
/// bare 3-digit number is indistinguishable from a clinical value, so the
/// bonus is only applied to codes that are **distinctive enough**
/// to be unambiguous when mentioned verbatim:
/// - ≥4 characters, OR
/// - contains a dot (`786.5`, `401.9`), OR
/// - contains a letter (`V70.0`, `01A`).
///
/// Bare 3-digit codes (130, 250, 401) never get the bonus — they rely
/// entirely on description-token overlap for scoring.
fn code_mentioned(code: &str, mentioned: &HashSet<String>) -> bool {
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
    mentioned.contains(&code.to_lowercase())
}

/// Specificity scoring adjustment for MSP billing preference.
///
/// MSP prefers specific codes over vague ones. This function returns a
/// small bonus or penalty:
/// - **+1** for 4-5 digit codes (e.g. 250.40, 401.9) — more specific
/// - **+1** for V-screening / family-history codes (V77.x, V16.x, V17.x,
///   V18.x, V20.x) — preferred for wellness visits over V70.x
/// - **-1** for V70.x codes — routine exam, MSP's least preferred
/// - **0** for everything else (3-digit unspecified, other V/E codes)
fn specificity_adjustment(code: &str) -> i32 {
    if code.starts_with("V70.") || code == "V70" {
        return -1; // Non-specific routine exam
    }
    // V-screening / family-history codes get a boost
    if code.starts_with("V81.")
        || code.starts_with("V77.")
        || code.starts_with("V16.")
        || code.starts_with("V17.")
        || code.starts_with("V18.")
        || code.starts_with("V20.")
    {
        return 1;
    }
    // 4-5 digit numeric codes (has a dot and at least 2 digits after)
    let dot_count = code.matches('.').count();
    if dot_count == 1 {
        let after_dot = code.split('.').nth(1).unwrap_or("");
        if after_dot.len() >= 2 {
            return 1; // e.g. 250.40, 401.90
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the exact boundary semantics of `code_mentioned` — written
    /// against the original substring-scan implementation before the
    /// HashSet rewrite; the rewrite must keep every row green.
    #[test]
    fn code_mentioned_boundary_semantics() {
        let cases: &[(&str, &str, bool)] = &[
            // Plain verbatim mention (dotted code).
            ("786.5", "chest pain 786.5 today", true),
            // Case-insensitive.
            ("V70.0", "routine v70.0 exam", true),
            // Adjacent punctuation is a valid boundary.
            ("786.5", "(786.5)", true),
            // Trailing period — still a mention ('.' is a non-word char).
            ("786.50", "code 786.50.", true),
            // Leading period — still a mention.
            ("786.5", ".786.5", true),
            // Longer run — NOT a mention (word char adjacent).
            ("786.5", "reading 786.50 today", false),
            // Substring of a longer alphanumeric run — not a mention.
            ("70.0", "code V70.0 here", false),
            // Double dot — not a mention.
            ("786.5", "786..5", false),
            // Bare 3-digit numeric codes are gate-blocked (lab/dose values).
            ("250", "glucose 250 mg", false),
            ("401", "BP 401?", false),
            // Dotted 3-digit root passes the gate and matches.
            ("250.0", "glucose 250.0 documented", true),
            // Short alpha code passes the gate via its letter.
            ("01A", "code 01A.", true),
            // Not present at all.
            ("786.5", "no codes here", false),
        ];
        for (code, source, expected) in cases {
            assert_eq!(
                code_mentioned(code, &extract_bounded_runs(&source.to_lowercase())),
                *expected,
                "code {code:?} in {source:?}"
            );
        }
    }

    /// The single-alternation abbreviation expansion is equivalent to the
    /// old sequential per-pattern replacement only while no expansion
    /// contains another abbreviation as a whole word (otherwise the old
    /// loop could cascade: expansion A's output re-matched by pattern B).
    /// Future edits to `CLINICAL_ABBREVIATIONS` must keep this property.
    #[test]
    fn clinical_abbreviations_have_no_cascading_expansions() {
        for (abbr, expansion) in CLINICAL_ABBREVIATIONS {
            let expansion_lower = expansion.to_lowercase();
            let expansion_words: std::collections::HashSet<&str> = expansion_lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            for (other, _) in CLINICAL_ABBREVIATIONS {
                assert!(
                    !expansion_words.contains(other.to_lowercase().as_str()),
                    "expansion of {abbr} ({expansion:?}) contains abbreviation {other} \
                     as a whole word — single-pass expansion would differ from sequential"
                );
            }
        }
    }

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
    fn empty_transcript_returns_baseline_only() {
        let selected = select_icd9_candidates("", None, None);
        // Baseline has 29 entries, all guaranteed slots under the 40 cap;
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
        // billing-critical floor (screening / family-history V-codes must
        // always be available).
        let transcript = "fever headache cough sore throat sinus pain congestion wheeze dyspnea chest pain abdominal pain nausea rash itching dizziness fatigue malaise myalgia arthralgia back pain";
        let selected = select_icd9_candidates(transcript, None, None);
        let codes: HashSet<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        // V16.0 (family hx GI cancer) and V77.0 (thyroid screening) are
        // late baseline entries and the most likely to be starved if the
        // cap logic regresses — they never match this acute-symptom
        // transcript, so they survive only via the baseline floor.
        assert!(
            codes.contains("V16.0"),
            "V16.0 must survive the cap even under heavy scoring: {:?}",
            codes
        );
        assert!(
            codes.contains("V77.0"),
            "V77.0 must survive the cap even under heavy scoring: {:?}",
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

    #[test]
    fn neck_pain_transcript_surfaces_cervicalgia() {
        // Regression: a "neck pain" transcript must surface 723.1
        // (CERVICALGIA) in the candidates. Previously this failed because
        // the description "CERVICALGIA" is a single token that doesn't
        // match "neck" or "pain" in the transcript.
        let selected = select_icd9_candidates(
            "Patient reports left-sided neck pain. Previously had right neck pain.",
            None,
            None,
        );
        let codes: Vec<&str> = selected.iter().map(|e| e.code.as_str()).collect();
        assert!(
            codes.contains(&"723.1"),
            "723.1 (CERVICALGIA) must be in candidates for a neck-pain transcript. Got: {codes:?}"
        );
    }
}
