//! Tauri commands for the logging subsystem.
//!
//! Provides:
//! - `get_log_path`   — returns the log directory so the UI can offer "Open logs"
//! - `get_recent_logs` — returns the tail of the current log file for in-app viewing
//! - `frontend_log`   — bridge for the frontend to write structured log entries

use std::path::PathBuf;

use medical_core::error::{AppError, AppResult};

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
                && newest.as_ref().is_none_or(|(t, _)| modified > *t) {
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
    Ok(format!("--- {filename} (last {max_lines} lines) ---\n{}", result.join("\n")))
}

/// Bridge for frontend JavaScript to log structured entries to the backend.
///
/// Call from the frontend as:
/// ```js
/// invoke('frontend_log', { level: 'error', message: 'Something failed', context: { component: 'RecordTab' } })
/// ```
#[tauri::command]
pub fn frontend_log(level: String, message: String, context: Option<serde_json::Value>) {
    let ctx = context
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_default();

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
