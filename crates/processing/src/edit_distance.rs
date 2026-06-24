//! Word-level Levenshtein for the training-corpus edit-distance signal.
//!
//! Operates on whitespace-split tokens, not characters — the curate
//! UI surfaces "you changed 20% of the words," which is more
//! intuitive for clinicians than character delta. ~O(m·n) where m
//! and n are token counts; for typical 200-800-word SOAPs that's a
//! sub-millisecond computation.
//!
//! # Use Case
//!
//! After a physician edits an AI-generated SOAP note, the edit distance
//! ratio indicates how much was changed. A low ratio (< 0.1) suggests the
//! AI output was close; a high ratio (> 0.5) suggests significant
//! hallucination or misalignment. This signal feeds the training-corpus
//! quality filter.

/// Word-level Levenshtein distance and ratio.
///
/// Splits both inputs on whitespace, then computes the standard Levenshtein
/// edit distance over the resulting token sequences using a two-row DP
/// algorithm (O(min(m,n)) memory).
///
/// # Returns
///
/// `(distance, ratio)` where:
/// - `distance` is the number of single-token insertions, deletions, and
///   substitutions needed to transform `a` into `b`.
/// - `ratio = distance / max(a_words, b_words)`, clamped to `[0.0, 1.0]`.
///
/// Empty inputs both return `(0, 0.0)`.
///
/// # Performance
///
/// For typical 200–800-word SOAP notes, computation is sub-millisecond.
pub fn word_edit_distance(a: &str, b: &str) -> (usize, f64) {
    let a_words: Vec<&str> = a.split_whitespace().collect();
    let b_words: Vec<&str> = b.split_whitespace().collect();

    let m = a_words.len();
    let n = b_words.len();
    if m == 0 && n == 0 {
        return (0, 0.0);
    }

    // Two-row DP, O(min(m,n)) memory after the swap.
    let (short, long) = if m <= n {
        (&a_words, &b_words)
    } else {
        (&b_words, &a_words)
    };
    let s_len = short.len();
    let l_len = long.len();

    let mut prev: Vec<usize> = (0..=s_len).collect();
    let mut curr: Vec<usize> = vec![0; s_len + 1];

    for i in 1..=l_len {
        curr[0] = i;
        for j in 1..=s_len {
            let cost = if long[i - 1] == short[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1) // insertion
                .min(prev[j] + 1) // deletion
                .min(prev[j - 1] + cost); // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    let distance = prev[s_len];
    let denom = m.max(n) as f64;
    let ratio = (distance as f64 / denom).clamp(0.0, 1.0);
    (distance, ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_strings_return_zero() {
        let (d, r) = word_edit_distance("hello world", "hello world");
        assert_eq!(d, 0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn empty_strings_return_zero() {
        let (d, r) = word_edit_distance("", "");
        assert_eq!(d, 0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn single_word_substitution() {
        let (d, r) = word_edit_distance("hello world", "hello there");
        assert_eq!(d, 1);
        assert!((r - 0.5).abs() < 1e-9, "expected ratio 0.5, got {r}");
    }

    #[test]
    fn complete_replacement_returns_max_ratio() {
        let (d, r) = word_edit_distance("a b c", "d e f");
        assert_eq!(d, 3);
        assert!((r - 1.0).abs() < 1e-9);
    }

    #[test]
    fn insertion_at_end() {
        let (d, _) = word_edit_distance("a b", "a b c d");
        assert_eq!(d, 2);
    }

    #[test]
    fn deletion_at_start() {
        let (d, _) = word_edit_distance("a b c d", "c d");
        assert_eq!(d, 2);
    }

    #[test]
    fn typical_soap_edit_is_moderate_ratio() {
        let draft = "S: Patient reports cough. O: temp 98.6. A: viral URI. P: rest.";
        let edited = "S: Patient reports productive cough. O: temp 99.1, mild rhonchi. A: viral URI. P: rest, fluids.";
        let (_d, r) = word_edit_distance(draft, edited);
        assert!(r > 0.1 && r < 0.6, "expected moderate edit ratio, got {r}");
    }
}
