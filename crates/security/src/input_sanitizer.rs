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

/// Utilities for sanitizing untrusted text input.
///
/// `InputSanitizer` is a stateless unit struct — both methods are
/// associated functions and do not require an instance.
pub struct InputSanitizer;

impl InputSanitizer {
    /// Remove all HTML tags from `input`, returning the bare text content.
    ///
    /// Uses a simple `<[^>]+>` regex that strips any `<...>` sequence.
    /// This is **not** a full HTML parser — it will not decode entities
    /// (`&amp;` stays as-is) and will not handle malformed markup the way
    /// a browser would. It is sufficient for the threat model of "user
    /// pasted a snippet from a web page into the context field" but
    /// should not be relied on against actively hostile input.
    pub fn strip_html(input: &str) -> String {
        HTML_TAG.replace_all(input, "").into_owned()
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
        assert_eq!(
            InputSanitizer::strip_html("no tags here"),
            "no tags here"
        );
        assert_eq!(
            InputSanitizer::strip_html("<script>alert('xss')</script>"),
            "alert('xss')"
        );
    }

    #[test]
    fn truncates_to_max_length() {
        // ASCII: safe to truncate at any byte boundary
        assert_eq!(InputSanitizer::truncate("hello world", 5), "hello");
        assert_eq!(InputSanitizer::truncate("hello", 10), "hello");
        assert_eq!(InputSanitizer::truncate("hello", 5), "hello");

        // Multi-byte: "é" is 2 bytes (0xC3 0xA9)
        let s = "café";  // c-a-f-é  = 5 bytes
        // Truncate to 4 bytes → should not split the 2-byte "é", so result is "caf"
        let result = InputSanitizer::truncate(s, 4);
        assert!(
            result == "caf" || result == "café",
            "unexpected truncation result: {:?}", result
        );
        // Either way the result must be valid UTF-8 and ≤ 4 bytes.
        assert!(result.len() <= 4);
    }
}
