//! Post-processing pipeline applied to AI output: strip markdown markers and
//! ensure section headers are separated by blank lines.
//!
//! Two-stage cleanup:
//!
//! 1. **`clean_text`** — strips code blocks, inline code, markdown headings
//!    (`##`), bold/italic markers (`**`, `*`, `__`, `_`), and citation markers
//!    (`[1]`, `[2]`).
//! 2. **`format_soap_paragraphs`** — ensures each SOAP section header
//!    (Subjective, Objective, Assessment, Plan, etc.) appears on its own line
//!    preceded by a blank line, splits concatenated bullet points, and handles
//!    headers that appear mid-line.
//!
//! The combined pipeline is exposed via [`postprocess_soap`].

use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.+?```").unwrap());
static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`(.+?)`").unwrap());
static MARKDOWN_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*#+\s*").unwrap());
static BOLD_STAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.*?)\*\*").unwrap());
static BOLD_UNDER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"__(.*?)__").unwrap());
static ITALIC_STAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*([^*]+?)\*").unwrap());
static ITALIC_UNDER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b_([^_]+?)_\b").unwrap());
static CITATION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\[\d+\])+").unwrap());

/// Precomputed per-header regex triples: (mid-line-with-colon, header-at-end,
/// header-then-bullet). One triple per SECTION_HEADERS entry, same order.
///
/// The char captured before a promoted header excludes list markers
/// (`-`, `*`, `+`, `•`): with a bare `(\S)`, the dash of a bullet like
/// "- Follow up: return in two weeks" matched, and the replacement tore the
/// bullet into an orphaned `-` line plus the de-bulleted text.
static SECTION_HEADER_RES: LazyLock<Vec<(Regex, Regex, Regex)>> = LazyLock::new(|| {
    SECTION_HEADERS
        .iter()
        .map(|header| {
            let escaped = regex::escape(header);
            (
                Regex::new(&format!(r"(?i)([^\s\-*+•])\s+({escaped}:)")).unwrap(),
                Regex::new(&format!(r"(?im)([^\s\-*+•])\s+({escaped})\s*$")).unwrap(),
                Regex::new(&format!(r"(?i)({escaped}:)\s*(- )")).unwrap(),
            )
        })
        .collect()
});

static BULLET_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" (- [A-Z])").unwrap());

/// SOAP section headers (lowercase) that should be separated by blank lines.
const SECTION_HEADERS: &[&str] = &[
    "icd-9 code",
    "icd-10 code",
    "icd code",
    "subjective",
    "objective",
    "assessment",
    "differential diagnosis",
    "plan",
    "follow up",
    "follow-up",
    "clinical synopsis",
];

/// Remove markdown formatting and citation markers from AI output.
///
/// `pub(crate)` so the anti-drift contract suite in [`crate::markdown`]
/// can pin its behavior alongside `document_generator::strip_markdown`.
pub(crate) fn clean_text(text: &str) -> String {
    let mut result = CODE_BLOCK_RE.replace_all(text, "").into_owned();
    result = INLINE_CODE_RE.replace_all(&result, "$1").into_owned();
    result = MARKDOWN_HEADING_RE.replace_all(&result, "").into_owned();
    result = BOLD_STAR_RE.replace_all(&result, "$1").into_owned();
    result = BOLD_UNDER_RE.replace_all(&result, "$1").into_owned();
    result = ITALIC_STAR_RE.replace_all(&result, "$1").into_owned();
    result = ITALIC_UNDER_RE.replace_all(&result, "$1").into_owned();
    result = CITATION_RE.replace_all(&result, "").into_owned();
    result.trim().to_string()
}

/// Ensure proper paragraph separation between SOAP note sections.
///
/// - Splits section headers that appear mid-line onto their own line
/// - Ensures a blank line before each major section header
/// - Splits concatenated bullet points onto separate lines
fn format_soap_paragraphs(text: &str) -> String {
    let mut result = text.replace("\r\n", "\n").replace('\r', "\n");

    // Handle section headers that appear mid-line — split them onto their own line
    for (mid_colon, end_anchor, header_bullet) in SECTION_HEADER_RES.iter() {
        result = mid_colon.replace_all(&result, "$1\n$2").into_owned();
        result = end_anchor.replace_all(&result, "$1\n$2").into_owned();
        result = header_bullet.replace_all(&result, "$1\n$2").into_owned();
    }

    // Split concatenated bullet points: " - Text" where preceded by content
    result = BULLET_SPLIT_RE.replace_all(&result, "\n$1").into_owned();

    // Now ensure blank lines before each section header
    let lines: Vec<&str> = result.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 20);

    for (i, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        let stripped_no_bullet = stripped
            .trim_start_matches('-')
            .trim_start_matches('\u{2022}')
            .trim_start_matches('*')
            .trim();
        let lower = stripped_no_bullet.to_lowercase();

        let is_header = SECTION_HEADERS.iter().any(|h| {
            if let Some(rest) = lower.strip_prefix(h) {
                rest.is_empty() || rest.starts_with(':') || rest.starts_with(' ')
            } else {
                false
            }
        });

        // Insert blank line before header if previous line isn't blank
        if is_header
            && i > 0
            && let Some(last) = out.last()
            && !last.trim().is_empty()
        {
            out.push(String::new());
        }

        out.push(line.to_string());
    }

    out.join("\n")
}

/// Full post-processing pipeline: clean markdown, extract ICD codes, format
/// paragraphs.
///
/// Applies `clean_text` (strip code blocks, bold/italic markers, citation
/// markers, markdown headings), then [`extract_icd_codes`], then
/// `format_soap_paragraphs` (ensure blank lines before section headers,
/// split mid-line headers, split concatenated bullets).
///
/// # Why extraction runs twice
///
/// Codes are extracted BEFORE `format_soap_paragraphs` so the bullet
/// splitter (`" - Capitalized"` → newline) cannot tear a hyphen-separated
/// code line (`ICD-9 Code: 719.43 - Pain in ankle`) in two — extracting
/// only after formatting would lose the description and strand an orphan
/// `- Pain in ankle` bullet in the stored note. A second extraction pass
/// AFTER formatting then catches codes the model emitted mid-line: the
/// mid-line header split only promotes them onto their own line during
/// formatting. Captures from both passes are deduplicated by code, under
/// the same first-occurrence-wins rule [`extract_icd_codes`] applies
/// within a single pass (a code legitimately captured twice — once from a
/// standalone line in pass 1, once from a mid-line/bulleted occurrence
/// promoted by the formatter in pass 2 — must not be emitted twice into
/// `metadata.icd_codes`).
///
/// This is the final transformation step before the generated SOAP note is
/// persisted and displayed to the user; the returned codes go to
/// `recordings.metadata["icd_codes"]`.
pub fn postprocess_soap(raw: &str) -> (String, Vec<ExtractedIcdCode>) {
    let cleaned = clean_text(raw);
    let (stripped, mut codes) = extract_icd_codes(&cleaned);
    let formatted = format_soap_paragraphs(&stripped);
    let (note, late_codes) = extract_icd_codes(&formatted);
    for late in late_codes {
        match codes.iter_mut().find(|c| c.code == late.code) {
            Some(existing) => {
                if existing.description.is_none() {
                    existing.description = late.description;
                }
            }
            None => codes.push(late),
        }
    }
    (note, codes)
}

// ─── ICD code extraction ────────────────────────────────────────────────────

/// One ICD billing code captured from a generated SOAP note.
///
/// Serialized into `recordings.metadata["icd_codes"]` — the frontend's
/// billing-code list renders from there, keeping the note body itself
/// free of code lines (cleaner to read, copy, and export).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExtractedIcdCode {
    /// Bare code as the model emitted it (e.g. "847.2", "V70.0", "Z00.00").
    pub code: String,
    /// Model-written title from the note's " — <description>" suffix,
    /// when the line carried one.
    pub description: Option<String>,
    /// Which ICD revision the note carried the code as.
    pub kind: IcdKind,
}

/// ICD revision a note's code line was tagged with.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IcdKind {
    Icd9,
    Icd10,
}

/// Per-line ICD code entry: `ICD-9 Code: 847.2 — Sprain of lumbar`. The
/// description suffix is optional. Mirrors the frontend extractor's
/// regex (`src/lib/icd.ts`) — keep the two in sync. Tolerates the
/// prompt's documented variants: `ICD-9:`/`ICD9`/`ICD-9 Code:` prefixes,
/// em-dash/en-dash/hyphen/colon separators, and optional surrounding
/// whitespace. Stray prose ("ICD-9 codes were reviewed.") has no code
/// body after the separator and does not match.
static ICD_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)^[ \t]*ICD[-\s]?(?P<ver>9|10)(?:\s+Code)?[\s:—–-]+(?P<code>[A-Z]?[\d.]+[A-Z]?)(?:[ \t]*[—–-][ \t]*(?P<desc>.*))?[ \t\r]*$",
    )
    .unwrap()
});

/// Extract the per-line ICD codes from a generated SOAP note and return
/// the note with those lines removed.
///
/// The stored note carries no code lines — billing codes live in
/// `recordings.metadata["icd_codes"]` and render in the UI's
/// billing-code list. Removal also collapses the blank-line runs the
/// stripped block leaves behind and trims leading/trailing blanks. A
/// note with no matching lines is returned byte-for-byte unchanged.
pub fn extract_icd_codes(note: &str) -> (String, Vec<ExtractedIcdCode>) {
    if !ICD_LINE_RE.is_match(note) {
        return (note.to_string(), Vec::new());
    }

    let mut codes: Vec<ExtractedIcdCode> = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    for line in note.split('\n') {
        if let Some(caps) = ICD_LINE_RE.captures(line) {
            let kind = if &caps["ver"] == "10" {
                IcdKind::Icd10
            } else {
                IcdKind::Icd9
            };
            let description = caps
                .name("desc")
                .map(|d| d.as_str().trim().to_string())
                .filter(|d| !d.is_empty());
            let code = caps["code"].to_string();
            // Deduplicate by code (a model may repeat a line; the billing
            // list keys rows by code). First occurrence wins, but a later
            // duplicate's description is adopted when the first had none.
            if let Some(existing) = codes.iter_mut().find(|c| c.code == code) {
                if existing.description.is_none() {
                    existing.description = description;
                }
            } else {
                codes.push(ExtractedIcdCode {
                    code,
                    description,
                    kind,
                });
            }
        } else {
            kept.push(line);
        }
    }

    // Collapse consecutive blank lines (the removed block's separator
    // blanks) and trim blanks from both ends of the note.
    let mut collapsed: Vec<&str> = Vec::with_capacity(kept.len());
    let mut prev_blank = true;
    for line in kept {
        let blank = line.trim().is_empty();
        if blank {
            if !prev_blank {
                collapsed.push(line);
            }
        } else {
            collapsed.push(line);
        }
        prev_blank = blank;
    }
    while collapsed.last().is_some_and(|l| l.trim().is_empty()) {
        collapsed.pop();
    }

    (collapsed.join("\n"), codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_strips_markdown() {
        let input = "## Heading\n**bold** and *italic* text [1][2]";
        let result = clean_text(input);
        assert!(!result.contains("##"));
        assert!(!result.contains("**"));
        assert!(!result.contains("[1]"));
        assert!(result.contains("bold"));
        assert!(result.contains("italic"));
    }

    #[test]
    fn format_soap_paragraphs_adds_blank_lines() {
        let input = "Some intro\nSubjective:\n- Chief complaint\nObjective:\n- Vitals";
        let result = format_soap_paragraphs(input);
        // There should be a blank line before Objective
        assert!(result.contains("\n\nObjective:"));
    }

    #[test]
    fn format_splits_midline_headers() {
        let input = "some content Subjective: - Chief complaint: pain";
        let result = format_soap_paragraphs(input);
        assert!(result.contains("\nSubjective:\n- Chief complaint: pain"));
    }

    #[test]
    fn postprocess_full_pipeline() {
        let raw = "## Heading\n**Subjective:**\n- complaint\nObjective:\n- vitals [1]";
        let (result, codes) = postprocess_soap(raw);
        assert!(!result.contains("##"));
        assert!(!result.contains("**"));
        assert!(!result.contains("[1]"));
        assert!(result.contains("\n\nObjective:"));
        assert!(codes.is_empty(), "no ICD lines in this fixture");
    }

    #[test]
    fn postprocess_hyphen_description_survives_bullet_split() {
        // Regression (bug review 2026-08-31): format_soap_paragraphs' bullet
        // splitter tears " - Capitalized" onto a new line. Extracting only
        // after formatting lost the description and stranded an orphan
        // "- Pain in ankle" bullet in the note. The pre-format extraction
        // pass must capture code AND description, and the note must carry
        // neither the code line nor the orphaned bullet.
        let raw =
            "ICD-9 Code: 719.43 - Pain in ankle\n\nSubjective:\n- Chief complaint: ankle pain";
        let (note, codes) = postprocess_soap(raw);
        assert!(!note.contains("ICD-9"), "note must be code-free: {note}");
        assert!(
            !note.contains("- Pain in ankle"),
            "description bullet must not be orphaned: {note}"
        );
        assert!(note.contains("- Chief complaint: ankle pain"));
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].code, "719.43");
        assert_eq!(codes[0].description.as_deref(), Some("Pain in ankle"));
    }

    #[test]
    fn postprocess_extracts_midline_code_promoted_by_header_split() {
        // The model embedded a code mid-line; format_soap_paragraphs'
        // mid-line header split promotes it onto its own line only during
        // formatting, so the post-format extraction pass must catch it.
        let raw =
            "Assessment: strain improving ICD-9 Code: 847.2 — Sprain of lumbar\n\nPlan:\n- Rest";
        let (note, codes) = postprocess_soap(raw);
        assert!(!note.contains("ICD-9"), "note must be code-free: {note}");
        assert!(note.contains("Assessment"));
        assert!(note.contains("- Rest"));
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].code, "847.2");
        assert_eq!(codes[0].description.as_deref(), Some("Sprain of lumbar"));
    }

    #[test]
    fn postprocess_dedupes_code_captured_by_both_extraction_passes() {
        // Regression (SOAP pipeline review 2026-09-04): the same code emitted
        // both as a standalone line (captured by the pre-format pass) and
        // mid-line (promoted onto its own line by the formatter, captured by
        // the post-format pass) was extend()ed into metadata twice. The
        // merge must keep one entry, preserving the first pass's description.
        let raw = "ICD-9 Code: 847.2 — Sprain of lumbar\n\nAssessment: strain improving ICD-9 Code: 847.2\n\nPlan:\n- Rest";
        let (note, codes) = postprocess_soap(raw);
        assert!(!note.contains("ICD-9"), "note must be code-free: {note}");
        assert_eq!(codes.len(), 1, "duplicate code collapsed: {codes:?}");
        assert_eq!(codes[0].code, "847.2");
        assert_eq!(
            codes[0].description.as_deref(),
            Some("Sprain of lumbar"),
            "first pass's description preserved"
        );
    }

    #[test]
    fn postprocess_backfills_description_from_late_duplicate() {
        // Mirror of the dedup test with the description on the LATE capture
        // only: the standalone line carries no description, the mid-line
        // occurrence does — the merge adopts it (same backfill rule
        // extract_icd_codes applies to repeats within a single pass).
        let raw = "ICD-9 Code: 847.2\n\nAssessment: strain improving ICD-9 Code: 847.2 — Sprain of lumbar\n\nPlan:\n- Rest";
        let (_, codes) = postprocess_soap(raw);
        assert_eq!(codes.len(), 1, "duplicate code collapsed: {codes:?}");
        assert_eq!(
            codes[0].description.as_deref(),
            Some("Sprain of lumbar"),
            "late duplicate's description adopted"
        );
    }

    #[test]
    fn format_does_not_tear_bullet_starting_with_header_word() {
        // Regression (SOAP pipeline review 2026-09-04): the mid-line header
        // split matched the bullet dash of "- Follow up: …" itself, tearing
        // the line into an orphaned "-" plus the de-bulleted text. The char
        // before a promoted header must not be a list marker.
        let raw = "Follow up:\n- Follow up: return in two weeks\n- Return sooner if worsening";
        let (note, _) = postprocess_soap(raw);
        let lines: Vec<&str> = note.lines().collect();
        assert!(
            lines.contains(&"- Follow up: return in two weeks"),
            "bullet must stay intact on one line: {note}"
        );
        assert!(!lines.contains(&"-"), "no orphaned bullet dash: {note}");
        assert!(
            lines.contains(&"- Return sooner if worsening"),
            "sibling bullet untouched: {note}"
        );
    }

    #[test]
    fn extract_dedupes_repeated_codes_adopting_description() {
        let (_, codes) =
            extract_icd_codes("ICD-9 Code: 847.2\nICD-9 Code: 847.2 — Sprain of lumbar");
        assert_eq!(codes.len(), 1, "duplicate code collapsed");
        assert_eq!(
            codes[0].description.as_deref(),
            Some("Sprain of lumbar"),
            "later duplicate's description adopted"
        );
    }

    #[test]
    fn extract_strips_code_lines_and_captures_descriptions() {
        let note = "ICD-9 Code: 847.2 — Sprain of lumbar\nICD-9 Code: 724.5 — Lumbago\n\nSubjective:\n- Chief complaint: back pain";
        let (clean, codes) = extract_icd_codes(note);
        assert!(
            !clean.contains("ICD-9"),
            "note body must be code-free: {clean}"
        );
        assert!(
            clean.starts_with("Subjective:"),
            "leading blanks trimmed: {clean}"
        );
        assert!(clean.contains("- Chief complaint: back pain"));
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0].code, "847.2");
        assert_eq!(codes[0].description.as_deref(), Some("Sprain of lumbar"));
        assert_eq!(codes[0].kind, IcdKind::Icd9);
        assert_eq!(codes[1].code, "724.5");
    }

    #[test]
    fn extract_handles_descriptionless_and_icd10_lines() {
        let note = "ICD-9 Code: 401.9\nICD-10 Code: Z00.00 — Encounter for general exam";
        let (clean, codes) = extract_icd_codes(note);
        assert!(clean.is_empty(), "only code lines were present: {clean}");
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0].description, None);
        assert_eq!(codes[0].kind, IcdKind::Icd9);
        assert_eq!(codes[1].code, "Z00.00");
        assert_eq!(codes[1].kind, IcdKind::Icd10);
        assert_eq!(
            codes[1].description.as_deref(),
            Some("Encounter for general exam")
        );
    }

    #[test]
    fn extract_tolerates_prompt_separator_variants() {
        for note in [
            "ICD-9: 847.2 — Sprain of lumbar",
            "ICD9 847.2 — Sprain of lumbar",
            "ICD-9 - 847.2 - Sprain of lumbar",
            "icd-9 code: 847.2 — Sprain of lumbar",
        ] {
            let (clean, codes) = extract_icd_codes(note);
            assert!(clean.is_empty(), "line must be stripped: {note} -> {clean}");
            assert_eq!(codes.len(), 1, "line must be captured: {note}");
            assert_eq!(codes[0].code, "847.2", "code captured: {note}");
            assert_eq!(
                codes[0].description.as_deref(),
                Some("Sprain of lumbar"),
                "description captured: {note}"
            );
        }
    }

    #[test]
    fn extract_leaves_prose_and_code_free_notes_untouched() {
        // Stray prose mentioning ICD-9 without a code body must not match;
        // a note with no code lines is returned byte-for-byte identical.
        let note = "Assessment:\n- ICD-9 codes were reviewed.\n- Strain, improving.";
        let (clean, codes) = extract_icd_codes(note);
        assert!(codes.is_empty());
        assert_eq!(clean, note);
    }

    #[test]
    fn extract_collapses_blank_runs_left_by_mid_note_block() {
        let note = "Subjective:\n- pain\n\nICD-9 Code: 847.2 — Sprain\n\nAssessment:\n- strain";
        let (clean, codes) = extract_icd_codes(note);
        assert_eq!(codes.len(), 1);
        assert!(!clean.contains("ICD-9"));
        // The two blanks around the removed line collapse to one.
        assert!(
            clean.contains("- pain\n\nAssessment:"),
            "single blank preserved: {clean}"
        );
        assert!(!clean.contains("\n\n\n"));
    }

    #[test]
    fn extract_serializes_to_metadata_shape() {
        // Wire contract with the frontend's `metadata.icd_codes` reader
        // (src/lib/icd.ts) — pinned here so a rename fails loudly.
        let (_, codes) =
            extract_icd_codes("ICD-9 Code: 847.2 — Sprain of lumbar\nICD-10 Code: I10");
        let json = serde_json::to_value(&codes).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr[0]["code"], "847.2");
        assert_eq!(arr[0]["description"], "Sprain of lumbar");
        assert_eq!(arr[0]["kind"], "icd9");
        assert_eq!(arr[1]["kind"], "icd10");
        assert!(arr[1]["description"].is_null());
    }
}
