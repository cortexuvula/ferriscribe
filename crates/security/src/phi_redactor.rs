//! PHI/PII redactor using regular-expression pattern matching.
//!
//! `PhiRedactor::redact` replaces sensitive tokens with bracketed placeholders
//! such as `[SSN]`, `[PHONE]`, `[EMAIL]`, `[DOB]`, `[MRN]`, `[ADDRESS]`,
//! and `[ZIP]`.
//!
//! # Pattern ordering and false-positive avoidance
//!
//! Patterns are applied in a fixed order (SSN → PHONE → EMAIL → DOB → MRN →
//! ADDRESS → ZIP). The SSN, MRN, DOB, and ZIP patterns require a **keyword
//! prefix** (e.g. `SSN:`, `DOB:`, `zip code`) to avoid false positives on
//! lab values, reference numbers, and clinical fractions like `BP 120/80`.
//! Adding a new regex that matches bare 9-digit or 5-digit numbers will
//! redact legitimate clinical content — always require contextual keywords.
//!
//! # Extensions
//!
//! Per-recording patterns (patient name, datetime) are passed as
//! [`Extension`]s via [`PhiRedactor::redact_with`]. Extensions run
//! *before* the static patterns so a patient name like "John Smith" is
//! replaced with `[PT_NAME]` before the EMAIL regex could match an email
//! containing "smith". Use [`names::build_patient_name_extension`] and
//! [`datetime::build_datetime_extension`] rather than hand-rolling the
//! regex — they handle possessives, salutations, and date-format edge
//! cases.
//!
//! # Where it is used
//!
//! - [`crate::audit_logger::AuditLogger`] — redacts log payloads.
//! - `src-tauri/corpus_export` — scrubs transcripts before writing the
//!   training-corpus JSONL; emits manifest warnings on residual PHI.

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

/// A compiled extension pattern that can be added to a redaction pass.
///
/// Built per-export from the recording's patient name (via
/// [`names::build_patient_name_extension`]) and the datetime pattern (via
/// [`datetime::build_datetime_extension`]). Extensions run *before* the
/// static patterns in [`PhiRedactor::redact_with`] so they can catch
/// identifiers that would otherwise collide with a static regex.
///
/// `Extension` is `Clone` so a single datetime extension can be reused
/// across every row of a bulk export.
#[derive(Clone)]
pub struct Extension {
    /// Compiled regex. Must be constructed once and reused — regex
    /// compilation is expensive.
    pub regex: Regex,
    /// Replacement text (e.g. `"[PT_NAME]"`, `"[DATE]"`). Use bracketed
    /// uppercase names to match the static-pattern convention.
    pub placeholder: &'static str,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Redacts PHI/PII from text.
///
/// `PhiRedactor` is a stateless unit struct — all state lives in the
/// lazily-compiled static pattern table. Construct with `new()` or
/// `Default::default()`; all redaction methods are also available as
/// associated functions (no `self` required).
pub struct PhiRedactor;

impl PhiRedactor {
    /// Create a new redactor instance.
    ///
    /// Equivalent to `PhiRedactor::default()`. The instance itself carries
    /// no state — it exists only so callers that prefer method-call syntax
    /// (`PhiRedactor::new().redact(...)`) can use it.
    pub fn new() -> Self {
        Self
    }

    /// Replace all detected PHI tokens in `text` with bracketed placeholders.
    ///
    /// Patterns are applied in order (SSN → PHONE → EMAIL → DOB → MRN →
    /// ADDRESS → ZIP), so a more specific pattern wins if it matches first.
    /// Returns the input unchanged when no patterns match.
    ///
    /// Use [`PhiRedactor::redact_with`] instead when per-recording context
    /// (patient name, datetime) is available — those identifiers are not
    /// covered by the static pattern set.
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

    /// Returns `true` if `text` contains at least one PHI token matched by
    /// the static pattern set.
    ///
    /// Use [`PhiRedactor::contains_phi_with`] to also check per-recording
    /// extensions (patient name, datetime).
    pub fn contains_phi(text: &str) -> bool {
        PATTERNS.iter().any(|p| p.regex.is_match(text))
    }

    /// Same as [`PhiRedactor::redact`], but applies the given `extensions`
    /// **first**, then the static patterns.
    ///
    /// Extensions run before the static patterns so a patient name like
    /// "John Smith" gets replaced with `[PT_NAME]` before the EMAIL regex
    /// could try to match an email containing "smith". This ordering is
    /// deliberate and should be preserved when adding new entry points.
    pub fn redact_with(text: &str, extensions: &[Extension]) -> String {
        let mut result = text.to_string();
        for ext in extensions {
            result = ext.regex.replace_all(&result, ext.placeholder).into_owned();
        }
        for pattern in PATTERNS.iter() {
            result = pattern
                .regex
                .replace_all(&result, pattern.placeholder)
                .into_owned();
        }
        result
    }

    /// Same predicate as [`PhiRedactor::contains_phi`], but checks both
    /// the supplied extensions and the static pattern set.
    ///
    /// Used by `src-tauri/corpus_export` as a post-redaction sanity check:
    /// if this still returns `true` after `redact_with`, a manifest
    /// warning is emitted.
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

/// Datetime redaction extension.
///
/// Matches ISO datetimes/dates, US short dates with 4-digit years (avoids
/// clinical-fraction collision on `BP 120/80`), and long English dates
/// like "May 11, 2026".
pub mod datetime {
    use super::{Extension, Regex};

    /// Build a compiled [`Extension`] that replaces datetime tokens with
    /// `[DATE]`.
    ///
    /// Safe to call repeatedly — the regex is compiled fresh each time but
    /// is cheap enough for per-export construction. Clone the resulting
    /// extension if you need to reuse it across many rows.
    ///
    /// # Panics
    ///
    /// Panics if the hardcoded regex fails to compile. This is a bug —
    /// the pattern is tested in CI and should never fail at runtime.
    pub fn build_datetime_extension() -> Extension {
        // Conservative: match ISO datetime first, then specific
        // unambiguous date formats. Avoid bare MM/DD which collides
        // with clinical fractions.
        // - ISO datetime: 2026-05-11T14:30:00 or 2026-05-11 14:30:00
        // - ISO date alone: 2026-05-11 (requires 4-digit year)
        // - US short date: MM/DD/YYYY (requires 4-digit year)
        // - Long English: "May 11, 2026"
        let pat = r"(?ix)
            \b
            (?:
                \d{4}-\d{2}-\d{2}(?:[T\s]\d{2}:\d{2}(?::\d{2})?)?     # ISO date(+time)
                |
                \d{1,2}/\d{1,2}/\d{4}                                 # US short date with 4-digit year
                |
                (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2},?\s+\d{4}  # Long form
            )
            \b
        ";
        let regex = Regex::new(pat).expect("hardcoded datetime regex");
        Extension {
            regex,
            placeholder: "[DATE]",
        }
    }
}

/// Per-recording patient-name pattern construction.
///
/// Builds a single [`Extension`] that matches the full name, possessive
/// form ("Jane Smith's"), first name alone, and last name preceded by a
/// salutation ("Mr. Smith", "Dr. Smith"). Returns `None` if the input is
/// empty or whitespace-only.
///
/// The regex is assembled from escaped name tokens so special characters
/// in names (hyphens, apostrophes) are handled safely.
pub mod names {
    use super::{Extension, Regex};

    /// Build a compiled [`Extension`] that replaces occurrences of
    /// `patient_name` with `[PT_NAME]`.
    ///
    /// Returns `None` when `patient_name` is empty or whitespace-only —
    /// callers should skip adding the extension in that case rather than
    /// passing a dummy regex.
    ///
    /// The returned regex matches (case-insensitively):
    /// 1. The full name, with optional possessive `'s`
    /// 2. The first name alone (word-boundary anchored)
    /// 3. Salutation + last name: `(Mr|Mrs|Ms|Miss|Dr) Last`
    pub fn build_patient_name_extension(patient_name: &str) -> Option<Extension> {
        let trimmed = patient_name.trim();
        if trimmed.is_empty() {
            return None;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        // Escape each token, then assemble three alternatives:
        //   1) Full name (with possessive): "First Last('s)?"
        //   2) First alone (word-boundary)
        //   3) Salutation Last: "(Mr|Mrs|Ms|Dr|Miss) Last"
        let escape = |s: &str| regex::escape(s);
        let mut alts: Vec<String> = Vec::new();

        let full = parts
            .iter()
            .map(|p| escape(p))
            .collect::<Vec<_>>()
            .join(r"\s+");
        alts.push(format!(r"\b{full}(?:'s)?\b"));

        alts.push(format!(r"\b{}(?:'s)?\b", escape(parts[0])));

        if parts.len() >= 2 {
            let last = parts.last().unwrap();
            alts.push(format!(
                r"\b(?:Mr|Mrs|Ms|Miss|Dr)\.?\s+{}(?:'s)?\b",
                escape(last)
            ));
        }

        let combined = format!(r"(?i)(?:{})", alts.join("|"));
        let regex = Regex::new(&combined).ok()?;
        Some(Extension {
            regex,
            placeholder: "[PT_NAME]",
        })
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
        assert_eq!(
            PhiRedactor::redact("Social Security Number: 123-45-6789"),
            "[SSN]"
        );
        assert_eq!(PhiRedactor::redact("SS# 123456789"), "[SSN]");
    }

    #[test]
    fn does_not_redact_9_digit_numbers() {
        // 9-digit numbers without SSN keyword context must NOT be redacted.
        assert_eq!(
            PhiRedactor::redact("Lab value 123456789"),
            "Lab value 123456789"
        );
        assert_eq!(
            PhiRedactor::redact("Reference #987654321"),
            "Reference #987654321"
        );
        assert_eq!(
            PhiRedactor::redact("id 123456789 here"),
            "id 123456789 here"
        );
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
        assert_eq!(
            PhiRedactor::redact("Platelet count 85000"),
            "Platelet count 85000"
        );
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

#[cfg(test)]
mod datetime_tests {
    use super::*;

    #[test]
    fn datetime_extension_redacts_iso_format() {
        let ext = datetime::build_datetime_extension();
        let out = PhiRedactor::redact_with("Visit on 2026-05-11 14:30:00.", &[ext]);
        assert!(out.contains("[DATE]"), "{out}");
    }

    #[test]
    fn datetime_extension_redacts_us_short_date() {
        let ext = datetime::build_datetime_extension();
        let out = PhiRedactor::redact_with("Surgery scheduled 05/15/2026.", &[ext]);
        assert!(out.contains("[DATE]"), "{out}");
    }

    #[test]
    fn datetime_extension_does_not_redact_clinical_numbers() {
        let ext = datetime::build_datetime_extension();
        let cases = ["BP 120/80", "98.6 F", "Lab 5/15 reactive"];
        for c in cases {
            let out = PhiRedactor::redact_with(c, std::slice::from_ref(&ext));
            assert_eq!(out, c, "clinical number wrongly redacted: {c} -> {out}");
        }
    }
}

#[cfg(test)]
mod names_tests {
    use super::*;

    #[test]
    fn build_patient_name_extension_handles_full_name() {
        let ext =
            names::build_patient_name_extension("Jane Smith").expect("should build extension");
        let out = PhiRedactor::redact_with("Jane Smith presents with cough.", &[ext]);
        assert!(out.contains("[PT_NAME]"));
        assert!(!out.contains("Jane Smith"));
    }

    #[test]
    fn build_patient_name_extension_handles_possessive() {
        let ext = names::build_patient_name_extension("Jane Smith").unwrap();
        let out = PhiRedactor::redact_with("Reviewed Jane Smith's results today.", &[ext]);
        assert!(out.contains("[PT_NAME]"), "{out}");
    }

    #[test]
    fn build_patient_name_extension_handles_first_only() {
        let ext = names::build_patient_name_extension("Jane Smith").unwrap();
        let out = PhiRedactor::redact_with("Jane is doing well.", &[ext]);
        // First-name-only should still match.
        assert!(out.contains("[PT_NAME]"), "{out}");
    }

    #[test]
    fn build_patient_name_extension_handles_last_only_with_title() {
        let ext = names::build_patient_name_extension("Jane Smith").unwrap();
        let out = PhiRedactor::redact_with("Mrs. Smith returns for follow-up.", &[ext]);
        assert!(out.contains("[PT_NAME]"), "{out}");
    }

    #[test]
    fn build_patient_name_extension_returns_none_for_empty() {
        assert!(names::build_patient_name_extension("").is_none());
        assert!(names::build_patient_name_extension("   ").is_none());
    }

    #[test]
    fn build_patient_name_extension_does_not_match_unrelated_text() {
        let ext = names::build_patient_name_extension("Jane Smith").unwrap();
        let out = PhiRedactor::redact_with("Patient denied chest pain.", &[ext]);
        assert_eq!(out, "Patient denied chest pain.");
    }
}
