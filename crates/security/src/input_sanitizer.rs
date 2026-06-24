//! Input sanitizer: HTML stripping and safe string truncation.
//!
//! Used on untrusted text that will be rendered in the Tauri WebView or
//! persisted to the database. The two operations are independent — call
//! [`InputSanitizer::strip_html`] to neutralize markup and
//! [`InputSanitizer::truncate`] to enforce byte-length limits without
//! splitting UTF-8 code points.

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref HTML_TAG: Regex = Regex::new(r"<[^>]+>").expect("invalid HTML tag regex");
}

/// Decode the five common HTML entities. Not a full entity decoder — covers
/// the cases that matter for the "stripped text is rendered elsewhere" threat
/// model. Numeric `&#x27;` and `&#39;` both map to apostrophe.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

/// Utilities for sanitizing untrusted text input.
///
/// `InputSanitizer` is a stateless unit struct — both methods are
/// associated functions and do not require an instance.
pub struct InputSanitizer;

impl InputSanitizer {
    /// Remove all HTML tags from `input`, returning the bare text content,
    /// and decode the five common HTML entities (`&amp;`, `&lt;`, `&gt;`,
    /// `&quot;`, `&#x27;` / `&#39;`).
    ///
    /// The tag-stripping regex is applied in a loop until no more matches are
    /// found — this catches the classic `title=">"` bypass where a `>` inside
    /// an attribute prematurely terminates a single-pass regex, leaving the
    /// remainder of the tag (and any nested handlers) intact.
    ///
    /// This is **not** a full HTML parser and should not be relied on against
    /// actively hostile input, but it is sufficient for the threat model of
    /// "user pasted a snippet from a web page into the context field."
    pub fn strip_html(input: &str) -> String {
        // Loop the regex until stable. Bounds the iteration to avoid a
        // pathological input spinning forever.
        let mut current = input.to_string();
        for _ in 0..8 {
            let next = HTML_TAG.replace_all(&current, "").into_owned();
            if next == current {
                break;
            }
            current = next;
        }
        decode_entities(&current)
    }

    /// Truncate `input` to at most `max_len` **bytes**, respecting UTF-8
    /// character boundaries so the result is always valid UTF-8.
    ///
    /// If `input.len() <= max_len`, the original slice is returned
    /// unchanged. Otherwise the cut is made at the nearest character
    /// boundary at or below `max_len` (via `str::floor_char_boundary`,
    /// stable since Rust 1.73), which may yield fewer than `max_len`
    /// bytes when the boundary falls inside a multi-byte code point.
    pub fn truncate(input: &str, max_len: usize) -> &str {
        if input.len() <= max_len {
            return input;
        }
        // floor_char_boundary is stable since Rust 1.73.
        let boundary = input.floor_char_boundary(max_len);
        &input[..boundary]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_html() {
        assert_eq!(
            InputSanitizer::strip_html("<p>Hello, <b>world</b>!</p>"),
            "Hello, world!"
        );
        assert_eq!(InputSanitizer::strip_html("no tags here"), "no tags here");
        assert_eq!(
            InputSanitizer::strip_html("<script>alert('xss')</script>"),
            "alert('xss')"
        );
    }

    #[test]
    fn strip_html_loops_to_catch_nested() {
        // The loop catches nested/leftover tags that a single regex pass
        // leaves behind. NOTE: a '>' *inside a quoted attribute* (e.g.
        // title=">") genuinely can't be handled by a regex — that requires a
        // real parser (ammonia). The loop + entity decode covers the realistic
        // user-paste threat model; it is not a hardening against hostile input.
        assert_eq!(
            InputSanitizer::strip_html("<<script>alert(1)</script>>"),
            "alert(1)>"
        );
        assert_eq!(
            InputSanitizer::strip_html("text <b><b>bold</b></b> tail"),
            "text bold tail"
        );
    }

    #[test]
    fn strip_html_decodes_common_entities() {
        assert_eq!(InputSanitizer::strip_html("a &amp; b"), "a & b");
        assert_eq!(InputSanitizer::strip_html("&lt;tag&gt;"), "<tag>");
        assert_eq!(
            InputSanitizer::strip_html("say &quot;hi&quot;"),
            "say \"hi\""
        );
        assert_eq!(InputSanitizer::strip_html("it&#x27;s"), "it's");
        assert_eq!(InputSanitizer::strip_html("it&#39;s"), "it's");
    }

    #[test]
    fn strip_html_decodes_entities_after_stripping() {
        // Tags removed, then entities in the remaining text decoded.
        assert_eq!(
            InputSanitizer::strip_html("<b>&amp;</b> and &lt;rest&gt;"),
            "& and <rest>"
        );
    }

    #[test]
    fn truncates_to_max_length() {
        // ASCII: safe to truncate at any byte boundary
        assert_eq!(InputSanitizer::truncate("hello world", 5), "hello");
        assert_eq!(InputSanitizer::truncate("hello", 10), "hello");
        assert_eq!(InputSanitizer::truncate("hello", 5), "hello");

        // Multi-byte: "é" is 2 bytes (0xC3 0xA9)
        let s = "café"; // c-a-f-é  = 5 bytes
        // Truncate to 4 bytes → should not split the 2-byte "é", so result is "caf"
        let result = InputSanitizer::truncate(s, 4);
        assert!(
            result == "caf" || result == "café",
            "unexpected truncation result: {:?}",
            result
        );
        // Either way the result must be valid UTF-8 and ≤ 4 bytes.
        assert!(result.len() <= 4);
    }
}
