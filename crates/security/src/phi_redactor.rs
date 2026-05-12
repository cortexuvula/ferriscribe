//! PHI/PII redactor using regular-expression pattern matching.
//!
//! `PhiRedactor::redact` replaces sensitive tokens with bracketed placeholders
//! such as `[SSN]`, `[PHONE]`, `[EMAIL]`, etc.

use lazy_static::lazy_static;
use regex::Regex;

// ─── Pattern definitions ──────────────────────────────────────────────────────

struct RedactionPattern {
    regex: Regex,
    placeholder: &'static str,
}

lazy_static! {
    static ref PATTERNS: Vec<RedactionPattern> = {
        let defs: &[(&str, &'static str)] = &[
            // Social Security Number: require keyword prefix to avoid false positives
            // on lab values, reference numbers, and other 9-digit sequences.
            (r"(?i)(?:SSN|Social\s+Security(?:\s+Number)?|Social\s+Sec|SS#|SS\s+#)\s*:?\s*\d{3}-?\d{2}-?\d{4}", "[SSN]"),
            // Phone numbers (US-centric, optional country code)
            (
                r"\b(?:\+?1[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b",
                "[PHONE]",
            ),
            // E-mail addresses
            (
                r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b",
                "[EMAIL]",
            ),
            // Date of birth with keyword prefix
            (
                r"(?i)\b(?:DOB|Date\s+of\s+Birth|Born|D\.O\.B\.?)\s*:?\s*\d{1,2}[-/]\d{1,2}[-/]\d{2,4}\b",
                "[DOB]",
            ),
            // Medical record number with keyword prefix
            (
                r"(?i)\b(?:MRN|Medical\s+Record|Record\s*#?|Chart\s*#?)\s*:?\s*[A-Z0-9\-]{4,20}\b",
                "[MRN]",
            ),
            // Street addresses: "123 Main Street", "456 Oak Ave", etc.
            (
                r"\b\d{1,5}\s+[A-Za-z]+(?:\s+[A-Za-z]+)*\s+(?:St|Street|Ave|Avenue|Blvd|Boulevard|Dr|Drive|Ln|Lane|Rd|Road|Ct|Court|Way|Pl|Place)\.?\b",
                "[ADDRESS]",
            ),
            // US ZIP codes: require "zip"/"zip code" keyword or a two-UPPERCASE-letter
            // US state abbreviation (word-boundary anchored, case-sensitive match)
            // before the 5-digit code to avoid false positives on medical values.
            (r"(?:(?i)zip(?:\s+code)?|(?-i)\b[A-Z]{2}\b)\s+\d{5}(?:-\d{4})?", "[ZIP]"),
        ];

        // Skip patterns that fail to compile rather than panicking. A typo in
        // a hardcoded regex is caught by the unit tests below in CI; this
        // fallback means an unlikely runtime failure degrades gracefully
        // instead of killing the process on startup.
        defs.iter()
            .filter_map(|(pat, placeholder)| match Regex::new(pat) {
                Ok(regex) => Some(RedactionPattern { regex, placeholder }),
                Err(e) => {
                    tracing::error!("Invalid PHI regex `{pat}`: {e} — pattern skipped");
                    None
                }
            })
            .collect()
    };
}

// ─── Extension API ───────────────────────────────────────────────────────────

/// A compiled extension pattern that can be added to a redaction
/// pass. Built per-export from the recording's patient_name and any
/// other per-corpus identifiers.
#[derive(Clone)]
pub struct Extension {
    pub regex: Regex,
    pub placeholder: &'static str,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Redacts PHI/PII from text.
pub struct PhiRedactor;

impl PhiRedactor {
    /// Create a new redactor instance.
    pub fn new() -> Self {
        Self
    }

    /// Replace all detected PHI tokens in `text` with bracketed placeholders.
    ///
    /// Patterns are applied in order (SSN → PHONE → … → ZIP), so a more
    /// specific pattern wins if it matches first.
    pub fn redact(text: &str) -> String {
        let mut result = text.to_string();
        for pattern in PATTERNS.iter() {
            let replaced = pattern
                .regex
                .replace_all(&result, pattern.placeholder)
                .into_owned();
            result = replaced;
        }
        result
    }

    /// Returns `true` if `text` contains at least one PHI token.
    pub fn contains_phi(text: &str) -> bool {
        PATTERNS.iter().any(|p| p.regex.is_match(text))
    }

    /// Same as `redact`, but applies the given extensions first.
    /// Extensions run before the static patterns so a patient name
    /// like "John Smith" gets replaced with [PT_NAME] before the
    /// static EMAIL pattern could try to match an email containing
    /// "smith". (Defense-in-depth ordering.)
    pub fn redact_with(text: &str, extensions: &[Extension]) -> String {
        let mut result = text.to_string();
        for ext in extensions {
            result = ext.regex.replace_all(&result, ext.placeholder).into_owned();
        }
        for pattern in PATTERNS.iter() {
            result = pattern.regex.replace_all(&result, pattern.placeholder).into_owned();
        }
        result
    }

    /// Same predicate as `contains_phi`, but checks both static
    /// patterns and the supplied extensions.
    pub fn contains_phi_with(text: &str, extensions: &[Extension]) -> bool {
        extensions.iter().any(|e| e.regex.is_match(text))
            || PATTERNS.iter().any(|p| p.regex.is_match(text))
    }
}

impl Default for PhiRedactor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_ssn() {
        // Keyword-prefixed SSN patterns should be redacted.
        assert_eq!(PhiRedactor::redact("SSN: 123-45-6789"), "[SSN]");
        assert_eq!(PhiRedactor::redact("SSN 123-45-6789"), "[SSN]");
        assert_eq!(PhiRedactor::redact("Social Security Number: 123-45-6789"), "[SSN]");
        assert_eq!(PhiRedactor::redact("SS# 123456789"), "[SSN]");
    }

    #[test]
    fn does_not_redact_9_digit_numbers() {
        // 9-digit numbers without SSN keyword context must NOT be redacted.
        assert_eq!(PhiRedactor::redact("Lab value 123456789"), "Lab value 123456789");
        assert_eq!(PhiRedactor::redact("Reference #987654321"), "Reference #987654321");
        assert_eq!(PhiRedactor::redact("id 123456789 here"), "id 123456789 here");
    }

    #[test]
    fn redacts_phone() {
        let out = PhiRedactor::redact("Call me at 555-867-5309");
        assert!(out.contains("[PHONE]"), "got: {}", out);
        let out2 = PhiRedactor::redact("(800) 555-1234 is the number");
        assert!(out2.contains("[PHONE]"), "got: {}", out2);
    }

    #[test]
    fn redacts_email() {
        let out = PhiRedactor::redact("Contact john.doe@example.com for help");
        assert!(out.contains("[EMAIL]"), "got: {}", out);
        assert!(!out.contains("john.doe@example.com"), "got: {}", out);
    }

    #[test]
    fn redacts_dob() {
        let out = PhiRedactor::redact("DOB: 01/15/1985");
        assert!(out.contains("[DOB]"), "got: {}", out);
        let out2 = PhiRedactor::redact("Date of Birth: 3-22-1990");
        assert!(out2.contains("[DOB]"), "got: {}", out2);
    }

    #[test]
    fn redacts_mrn() {
        let out = PhiRedactor::redact("MRN: ABC1234567");
        assert!(out.contains("[MRN]"), "got: {}", out);
        let out2 = PhiRedactor::redact("Chart #: XYZ-9876");
        assert!(out2.contains("[MRN]"), "got: {}", out2);
    }

    #[test]
    fn redacts_address() {
        let out = PhiRedactor::redact("lives at 123 Main Street in the city");
        assert!(out.contains("[ADDRESS]"), "got: {}", out);
        let out2 = PhiRedactor::redact("Sent to 45 Oak Ave");
        assert!(out2.contains("[ADDRESS]"), "got: {}", out2);
    }

    #[test]
    fn contains_phi_detects() {
        assert!(PhiRedactor::contains_phi("SSN: 123-45-6789"));
        assert!(PhiRedactor::contains_phi("email: foo@bar.com"));
        assert!(!PhiRedactor::contains_phi("Hello, world!"));
    }

    #[test]
    fn preserves_non_phi() {
        let text = "The patient is feeling well today.";
        assert_eq!(PhiRedactor::redact(text), text);
    }

    #[test]
    fn handles_multiple_patterns() {
        let text = "Patient john@example.com, SSN 123-45-6789, DOB 01/01/1990";
        let out = PhiRedactor::redact(text);
        assert!(out.contains("[EMAIL]"), "got: {}", out);
        assert!(out.contains("[SSN]"), "got: {}", out);
        assert!(out.contains("[DOB]"), "got: {}", out);
        assert!(!out.contains("john@example.com"), "got: {}", out);
        assert!(!out.contains("123-45-6789"), "got: {}", out);
    }

    #[test]
    fn redacts_zip() {
        // ZIP with explicit keyword prefix.
        let out = PhiRedactor::redact("zip code 90210");
        assert!(out.contains("[ZIP]"), "got: {}", out);
        // ZIP with two-letter state abbreviation.
        let out2 = PhiRedactor::redact("Springfield IL 62701");
        assert!(out2.contains("[ZIP]"), "got: {}", out2);
    }

    #[test]
    fn does_not_redact_5_digit_numbers() {
        // 5-digit numbers without address/zip context must NOT be redacted.
        assert_eq!(PhiRedactor::redact("WBC count 15000"), "WBC count 15000");
        assert_eq!(PhiRedactor::redact("Dose 10000 units"), "Dose 10000 units");
        assert_eq!(PhiRedactor::redact("Platelet count 85000"), "Platelet count 85000");
    }

    #[test]
    fn redact_with_extensions_runs_extensions_first() {
        let ext = Extension {
            regex: Regex::new(r"(?i)\bJohn Smith\b").unwrap(),
            placeholder: "[PT_NAME]",
        };
        let input = "Mr. John Smith was seen for follow-up; reach him at john.smith@example.com.";
        let out = PhiRedactor::redact_with(input, &[ext]);
        assert!(out.contains("[PT_NAME]"), "name should be redacted: {out}");
        assert!(out.contains("[EMAIL]"), "email should be redacted: {out}");
        assert!(!out.contains("John Smith"), "raw name leaked: {out}");
    }

    #[test]
    fn redact_with_empty_extensions_matches_redact() {
        let input = "Call (555) 867-5309.";
        let a = PhiRedactor::redact(input);
        let b = PhiRedactor::redact_with(input, &[]);
        assert_eq!(a, b);
    }
}
