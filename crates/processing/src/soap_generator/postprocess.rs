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
static SECTION_HEADER_RES: LazyLock<Vec<(Regex, Regex, Regex)>> = LazyLock::new(|| {
    SECTION_HEADERS
        .iter()
        .map(|header| {
            let escaped = regex::escape(header);
            (
                Regex::new(&format!(r"(?i)(\S)\s+({escaped}:)")).unwrap(),
                Regex::new(&format!(r"(?im)(\S)\s+({escaped})\s*$")).unwrap(),
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
fn clean_text(text: &str) -> String {
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

/// Full post-processing pipeline: clean markdown, then format paragraphs.
///
/// Applies `clean_text` (strip code blocks, bold/italic markers, citation
/// markers, markdown headings) followed by `format_soap_paragraphs` (ensure
/// blank lines before section headers, split mid-line headers, split
/// concatenated bullets).
///
/// This is the final transformation step before the generated SOAP note is
/// persisted and displayed to the user.
pub fn postprocess_soap(raw: &str) -> String {
    let cleaned = clean_text(raw);
    format_soap_paragraphs(&cleaned)
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
        let result = postprocess_soap(raw);
        assert!(!result.contains("##"));
        assert!(!result.contains("**"));
        assert!(!result.contains("[1]"));
        assert!(result.contains("\n\nObjective:"));
    }
}
