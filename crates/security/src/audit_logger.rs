//! Audit logger — redacts PHI from log details before they are written.
//!
//! Wraps [`crate::phi_redactor::PhiRedactor`] so callers that need to log
//! user-supplied or patient-adjacent text have a one-line safe path.
//! Intentionally uses the static pattern set only (no per-call extensions)
//! — if you need per-recording identifiers in a log line, call
//! [`PhiRedactor::redact_with`](crate::phi_redactor::PhiRedactor::redact_with)
//! directly.

use crate::phi_redactor::PhiRedactor;

/// Wraps the audit-logging concern with automatic PHI redaction.
///
/// `AuditLogger` is a stateless unit struct — the only method,
/// [`AuditLogger::redact_for_log`], is an associated function that
/// delegates to [`PhiRedactor::redact`]. It exists as a named type so
/// call-sites read as intent ("this string is being sanitized for the
/// log") rather than as an opaque PHI-redactor call.
pub struct AuditLogger;

impl AuditLogger {
    /// Create a new `AuditLogger`.
    ///
    /// Equivalent to `AuditLogger::default()`. The instance carries no
    /// state.
    pub fn new() -> Self {
        Self
    }

    /// Redact any PHI/PII found in `details` so it is safe to write to
    /// a log sink.
    ///
    /// Uses the static pattern set (SSN, PHONE, EMAIL, DOB, MRN, ADDRESS,
    /// ZIP). Per-recording identifiers such as patient names are **not**
    /// redacted — construct a [`crate::phi_redactor::Extension`] and call
    /// [`PhiRedactor::redact_with`] directly if those must be scrubbed.
    pub fn redact_for_log(details: &str) -> String {
        PhiRedactor::redact(details)
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_phi_in_log_details() {
        let input = "User action: looked up SSN 123-45-6789 for patient john@example.com";
        let output = AuditLogger::redact_for_log(input);
        assert!(!output.contains("123-45-6789"), "SSN should be redacted: {}", output);
        assert!(!output.contains("john@example.com"), "email should be redacted: {}", output);
        assert!(output.contains("[SSN]"), "expected [SSN] placeholder: {}", output);
        assert!(output.contains("[EMAIL]"), "expected [EMAIL] placeholder: {}", output);
    }
}
