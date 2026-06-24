//! Tauri commands for the logging subsystem.
//!
//! Provides:
//! - `get_log_path`   — returns the log directory so the UI can offer "Open logs"
//! - `get_recent_logs` — returns the tail of the current log file for in-app viewing
//! - `frontend_log`   — bridge for the frontend to write structured log entries

use std::path::PathBuf;

use medical_core::error::{AppError, AppResult};
use medical_security::phi_redactor::PhiRedactor;

/// Return the path to the log directory.
#[tauri::command]
pub fn get_log_path() -> String {
    log_dir().display().to_string()
}

/// Return the last `lines` lines of today's log file.
///
/// Useful for an in-app log viewer or for attaching to bug reports.
#[tauri::command]
pub fn get_recent_logs(lines: Option<usize>) -> AppResult<String> {
    let max_lines = lines.unwrap_or(200);
    let dir = log_dir();

    // Find the most recently modified .log file
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(&dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        if let Ok(meta) = path.metadata()
            && let Ok(modified) = meta.modified()
            && newest.as_ref().is_none_or(|(t, _)| modified > *t)
        {
            newest = Some((modified, path));
        }
    }

    let log_path = newest
        .map(|(_, p)| p)
        .ok_or_else(|| AppError::Other("No log files found".to_string()))?;

    let content = std::fs::read_to_string(&log_path)?;

    let tail: Vec<&str> = content.lines().rev().take(max_lines).collect();
    let result: Vec<&str> = tail.into_iter().rev().collect();

    // Prepend source file name for context
    let filename = log_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    Ok(format!(
        "--- {filename} (last {max_lines} lines) ---\n{}",
        result.join("\n")
    ))
}

/// Maximum length of the `message` field accepted by `frontend_log`.
///
/// PHI guardrail: the frontend is trusted not to log patient content, but a
/// hard cap prevents accidental floods (e.g., a stringified transcript) from
/// reaching the on-disk log file. 1 KB is plenty for structured error
/// messages while still bounding worst-case damage.
const FRONTEND_LOG_MESSAGE_MAX: usize = 1000;

/// Maximum length of the `context` blob accepted by `frontend_log`.
///
/// Context is a JSON object stringified by the caller; 2 KB allows a few
/// nested fields without unbounded growth.
const FRONTEND_LOG_CONTEXT_MAX: usize = 2000;

/// PHI guardrail for frontend-supplied log text. The frontend is trusted not
/// to log patient content, but a bug like `invoke('frontend_log', { message:
/// transcript })` would land up to 1 KB of patient text in the rotating log
/// file (AGENTS.md line 6 forbids PHI in tracing::*). We run the static PHI
/// pattern redactor (SSN/phone/email/DOB/MRN/address/ZIP) on both the message
/// and the stringified context before emitting, so structured identifiers at
/// least are replaced with placeholders even if the frontend misbehaves.
/// Free-text names are NOT covered — that remains the frontend's responsibility.
fn redact_phi(s: &str) -> String {
    PhiRedactor::redact(s)
}

/// Truncate a string to at most `max` characters, appending an explicit
/// marker so log readers can tell the entry was capped.
fn truncate_for_log(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…[truncated, {len} chars total]")
}

/// Bridge for frontend JavaScript to log structured entries to the backend.
///
/// Call from the frontend as:
/// ```js
/// invoke('frontend_log', { level: 'error', message: 'Something failed', context: { component: 'RecordTab' } })
/// ```
///
/// `message` and the stringified `context` are length-capped (see
/// `FRONTEND_LOG_MESSAGE_MAX` / `FRONTEND_LOG_CONTEXT_MAX`) so an accidental
/// PHI flood cannot fill the log file.
#[tauri::command]
pub fn frontend_log(level: String, message: String, context: Option<serde_json::Value>) {
    // PHI scrubber runs BEFORE the length cap so a structured identifier
    // sitting just past the 1 KB boundary still gets redacted in the head.
    let message = truncate_for_log(&redact_phi(&message), FRONTEND_LOG_MESSAGE_MAX);
    let ctx = truncate_for_log(
        &redact_phi(&context.as_ref().map(|v| v.to_string()).unwrap_or_default()),
        FRONTEND_LOG_CONTEXT_MAX,
    );

    match level.to_lowercase().as_str() {
        "error" => tracing::error!(source = "frontend", context = %ctx, "{message}"),
        "warn" => tracing::warn!(source = "frontend", context = %ctx, "{message}"),
        "debug" => tracing::debug!(source = "frontend", context = %ctx, "{message}"),
        "trace" => tracing::trace!(source = "frontend", context = %ctx, "{message}"),
        _ => tracing::info!(source = "frontend", context = %ctx, "{message}"),
    }
}

fn log_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust-medical-assistant")
        .join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_log_passes_through_short_strings() {
        assert_eq!(truncate_for_log("hello", 100), "hello");
    }

    #[test]
    fn truncate_for_log_returns_input_at_exact_limit() {
        let s = "x".repeat(50);
        assert_eq!(truncate_for_log(&s, 50), s);
    }

    #[test]
    fn truncate_for_log_caps_and_annotates() {
        let s = "x".repeat(1500);
        let out = truncate_for_log(&s, 1000);
        assert!(out.starts_with(&"x".repeat(1000)));
        assert!(out.contains("[truncated, 1500 chars total]"));
        // The capped output is short: head (1000) + a small annotation tail.
        assert!(out.chars().count() < 1100);
    }

    #[test]
    fn truncate_for_log_counts_chars_not_bytes() {
        // Multi-byte UTF-8: 5 chars but 15 bytes. Should pass through under
        // a char-based cap of 10.
        let s = "héllo".repeat(5); // 25 chars
        let out = truncate_for_log(&s, 10);
        assert_eq!(out.chars().take(10).collect::<String>(), "héllohéllo");
        assert!(out.contains("[truncated, 25 chars total]"));
    }

    #[test]
    fn redact_phi_strips_phone_numbers() {
        let input = "Call patient at (555) 123-4567 about results";
        let out = redact_phi(input);
        assert!(
            !out.contains("555"),
            "phone digits must be redacted; got: {out}"
        );
        assert!(
            out.contains("[PHONE"),
            "expected [PHONE… placeholder; got: {out}"
        );
    }

    #[test]
    fn redact_phi_strips_keyword_prefixed_ssns() {
        // The SSN pattern requires a keyword prefix (SSN:/Social Security/etc.)
        // to avoid false positives — see phi_redactor.rs pattern order docs.
        let input = "SSN: 123-45-6789 on file";
        let out = redact_phi(input);
        assert!(
            !out.contains("6789"),
            "SSN tail must be redacted; got: {out}"
        );
        assert!(
            out.contains("[SSN]"),
            "expected [SSN] placeholder; got: {out}"
        );
    }

    #[test]
    fn redact_phi_leaves_diagnostic_text_intact() {
        // No keyword-prefixed identifiers → no redaction. Confirms the scrubber
        // doesn't mangle ordinary frontend error messages.
        let input = "Failed to load list (count=0)";
        assert_eq!(redact_phi(input), input);
    }
}
