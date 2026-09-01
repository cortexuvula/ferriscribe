//! Markdown-stripper contracts: documentation + anti-drift test suite.
//!
//! FerriScribe has two markdown strippers with intentionally different
//! contracts:
//!
//! | Behavior | [`strip_markdown`] (documents) | [`clean_text`] (SOAP) |
//! |---|---|---|
//! | `## Heading` | heading text **uppercased** | only the `#` markers removed |
//! | `-` / `*` bullets | converted to `• ` | left as-is |
//! | `[x](url)` links | stripped, text kept | left as-is |
//! | `---` horizontal rules | removed | left as-is |
//! | `[1]` citation markers | left as-is | removed |
//! | fenced ` ``` ` code blocks | NOT handled (inline code only) | removed entirely |
//! | `__bold__` | NOT handled (manual `_…_` scan skips it) | stripped |
//! | 3+ blank lines | collapsed to 2 | left as-is |
//! | leading/trailing whitespace | left as-is | trimmed |
//!
//! ## Why they are deliberately NOT merged
//!
//! Although the two share ~70% of their *conceptual* surface, every
//! nominally-shared regex differs in a quantifier (e.g. bold is
//! ``\*\*(.+?)\*\*`` in one and ``\*\*(.*?)\*\*`` in the other — different
//! results on empty delimiters like `****`), and the step ORDER is
//! load-bearing differently: `clean_text` strips inline code BEFORE heading
//! markers (so a line-initial `` `## x` `` loses its markers), while
//! `strip_markdown` uppercases headings BEFORE inline code (so the same
//! input keeps them). Any single fused pipeline would change one wrapper's
//! byte output. The divergence table below is therefore pinned by tests
//! instead: if a future edit to either stripper drifts from its documented
//! contract, the [`anti_drift`] suite fails here.
//!
//! [`strip_markdown`]: crate::document_generator::strip_markdown
//! [`clean_text`]: crate::soap_generator::postprocess::clean_text

#[cfg(test)]
mod tests {
    use crate::document_generator::strip_markdown;
    use crate::soap_generator::clean_text;

    /// Inputs exercising every documented divergence. `(input,
    /// strip_markdown output, clean_text output)`.
    fn divergence_table() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            // ── Shared constructs: both wrappers must agree ──────────────
            ("**bold** text", "bold text", "bold text"),
            ("*italic* text", "italic text", "italic text"),
            ("`code` span", "code span", "code span"),
            // snake_case underscores survive in both (intraword)
            ("dosage_var_1", "dosage_var_1", "dosage_var_1"),
            // ── Headings: uppercase vs marker-only ───────────────────────
            ("## Assessment", "ASSESSMENT", "Assessment"),
            // ── Bullets: • conversion vs as-is ───────────────────────────
            ("- nitroglycerin", "• nitroglycerin", "- nitroglycerin"),
            // ── Links: stripped vs as-is ─────────────────────────────────
            (
                "see [guidance](http://example.com) here",
                "see guidance here",
                "see [guidance](http://example.com) here",
            ),
            // ── Horizontal rules: removed vs as-is ───────────────────────
            (
                "before\n---\nafter",
                "before\n\nafter",
                "before\n---\nafter",
            ),
            // ── Citations: as-is vs removed ──────────────────────────────
            ("chest pain [1][2]", "chest pain [1][2]", "chest pain"),
            // ── Fenced code blocks: kept vs removed ──────────────────────
            (
                "start\n```\ncode line\n```\nend",
                // strip_markdown has no fence handling; its inline-code
                // regex consumes one backtick from each fence (leaving
                // "``" lines that the HR rule's {3,} then misses). Pinned
                // as-is: changing this would alter the documents pipeline.
                "start\n``\ncode line\n``\nend",
                "start\n\nend",
            ),
            // ── Bold underscores: untouched vs stripped ──────────────────
            ("__underline__", "__underline__", "underline"),
            // ── Blank-line collapse vs as-is ─────────────────────────────
            ("a\n\n\n\nb", "a\n\nb", "a\n\n\n\nb"),
            // ── Trim vs as-is ────────────────────────────────────────────
            ("  padded  ", "  padded  ", "padded"),
        ]
    }

    #[test]
    fn anti_drift() {
        for (input, want_strip, want_clean) in divergence_table() {
            assert_eq!(
                strip_markdown(input),
                want_strip,
                "strip_markdown drifted on {input:?}"
            );
            assert_eq!(
                clean_text(input),
                want_clean,
                "clean_text drifted on {input:?}"
            );
        }
    }

    /// The italic-underscore constructs the two wrappers AGREE on: simple
    /// `_emphasis_` is stripped by both (manual scanner vs regex), and
    /// intraword underscores survive both (pinned above via snake_case).
    #[test]
    fn italic_underscores_agree_on_simple_emphasis() {
        assert_eq!(strip_markdown("take _with_ food"), "take with food");
        assert_eq!(clean_text("take _with_ food"), "take with food");
    }
}
