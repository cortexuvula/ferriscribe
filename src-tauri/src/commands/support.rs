//! Support bundle export — PHI-redacted log file concatenation for
//! troubleshooting. The user triggers this from Settings → About →
//! "Share logs for support".

use std::path::Path;

use medical_core::error::{AppError, AppResult};
use medical_security::phi_redactor::PhiRedactor;

/// Generate a PHI-redacted support bundle from all log files in `log_dir`.
///
/// Reads every `.log` file (sorted oldest-first by modified time),
/// concatenates them with `=== <filename> ===` separators, prepends a
/// bundle header with app version + timestamp, and runs the entire string
/// through [`PhiRedactor::redact`].
pub fn export_support_bundle_inner(log_dir: &Path) -> AppResult<String> {
    // 1. Collect all log files sorted by modified time (oldest first).
    // tracing-appender names files `ferri-scribe.log.2026-06-27` (prefix
    // + date), so we match filenames containing `.log` rather than checking
    // the final extension (which would be the date, not "log").
    let mut log_files: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    let entries = std::fs::read_dir(log_dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_log = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.contains(".log"));
        if is_log
            && let Ok(meta) = path.metadata()
            && let Ok(modified) = meta.modified()
        {
            log_files.push((modified, path));
        }
    }

    if log_files.is_empty() {
        return Err(AppError::Other(
            "No log files found in the log directory.".into(),
        ));
    }

    log_files.sort_by_key(|(time, _)| *time);

    // 2. Concatenate with file separators.
    let mut bundle = String::new();
    bundle.push_str("FerriScribe Support Bundle\n");
    bundle.push_str(&format!("Version: {}\n", env!("CARGO_PKG_VERSION")));
    bundle.push_str(&format!(
        "Generated: {}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    ));
    bundle.push_str(&format!("Log files: {}\n\n", log_files.len()));

    for (_, path) in &log_files {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        bundle.push_str(&format!("=== {filename} ===\n"));
        match std::fs::read_to_string(path) {
            Ok(content) => bundle.push_str(&content),
            Err(e) => bundle.push_str(&format!("[read error: {e}]\n")),
        }
        bundle.push('\n');
    }

    // 3. PHI-redact the entire bundle.
    Ok(PhiRedactor::redact(&bundle))
}

/// Export all app logs as a PHI-redacted plain-text file.
///
/// Reads every `.log` file from the log directory, concatenates them,
/// redacts PHI (phone numbers, SSNs, emails, DOBs, MRNs, addresses),
/// and writes the result to `file_path`. The user chooses the path via
/// a frontend save-file dialog.
#[tauri::command]
pub async fn export_support_bundle(file_path: String) -> AppResult<()> {
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rust-medical-assistant")
        .join("logs");

    // Both the log reading AND the file write happen on spawn_blocking so
    // neither blocks the Tauri IPC thread.
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let bundle = export_support_bundle_inner(&log_dir)?;
        std::fs::write(&file_path, &bundle)?;
        Ok(())
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn bundle_includes_header_and_file_separators() {
        let dir = TempDir::new().unwrap();
        // tracing-appender names files `ferri-scribe.log.2026-06-27` (the
        // .log prefix + date suffix), so the test must use that pattern.
        fs::write(
            dir.path().join("ferri-scribe.log.2026-06-26"),
            "2026-06-26 INFO Old log entry\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("ferri-scribe.log.2026-06-27"),
            "2026-06-27 INFO New log entry\n",
        )
        .unwrap();

        let bundle = export_support_bundle_inner(dir.path()).unwrap();

        assert!(
            bundle.contains("FerriScribe Support Bundle"),
            "must have bundle header"
        );
        assert!(bundle.contains("=== "), "must have file separators");
        assert!(
            bundle.contains("Old log entry"),
            "must include older file content"
        );
        assert!(
            bundle.contains("New log entry"),
            "must include newer file content"
        );
    }

    #[test]
    fn bundle_redacts_phi() {
        let dir = TempDir::new().unwrap();
        let phi_line = "2026-06-27 INFO Called patient at (604) 555-0199\n";
        fs::write(dir.path().join("ferri-scribe.log.2026-06-27"), phi_line).unwrap();

        let bundle = export_support_bundle_inner(dir.path()).unwrap();

        assert!(
            !bundle.contains("(604) 555-0199"),
            "phone number must be redacted: got:\n{bundle}"
        );
    }

    #[test]
    fn bundle_errors_when_no_logs() {
        let dir = TempDir::new().unwrap();
        let result = export_support_bundle_inner(dir.path());
        assert!(result.is_err(), "empty directory should error");
    }
}
