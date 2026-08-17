pub mod audio;
pub mod chat;
pub mod conditions;
pub mod content_sync;
pub mod context_templates;
pub mod export;
pub mod generation;
pub mod icd;
pub mod letter_audiences;
pub mod logging;
pub mod models;
pub mod ocr;
pub mod pipeline;
pub mod providers;
pub mod recordings;
pub mod recordings_edit;
pub mod recovery;
pub mod settings;
pub mod sharing;
pub mod support;
pub mod training_corpus;
pub mod training_corpus_export;
pub mod transcription;
pub mod user_dictionary;
pub mod vocabulary;

use std::path::{Path, PathBuf};

use medical_core::error::{AppError, AppResult};
use medical_db::Database;

/// Convert a tokio JoinError into an AppError. Used by spawn_blocking call
/// sites to avoid repeating the format! boilerplate 36 times.
pub fn join_err(e: tokio::task::JoinError) -> AppError {
    AppError::Other(format!("Task join error: {e}"))
}

/// Validate that a user-supplied file path is safe to read from or write to.
/// Rejects paths that traverse outside the allowed directories.
/// For SAVE operations (export), the path should be under a user-chosen directory
/// (we can't sandbox to app-data because the user picks where to save via a dialog).
/// Instead, we validate it's an absolute path with no traversal components and
/// doesn't target sensitive system files.
pub fn validate_user_path(path: &str) -> AppResult<PathBuf> {
    let p = PathBuf::from(path);

    // Must be absolute (no relative traversal).
    if !p.is_absolute() {
        return Err(AppError::Other("Path must be absolute".into()));
    }

    // Normalize the path (resolves .. and .).
    let canonical = if p.exists() {
        p.canonicalize()
            .map_err(|e| AppError::Other(format!("Invalid path: {e}")))?
    } else {
        // For paths that don't exist yet (save targets), canonicalize the parent.
        let parent = p.parent().unwrap_or(&p);
        let canonical_parent = parent
            .canonicalize()
            .map_err(|e| AppError::Other(format!("Invalid directory: {e}")))?;
        canonical_parent.join(p.file_name().unwrap_or_default())
    };

    // Block known-dangerous paths.
    let dangerous = [
        "/etc", "/var", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys", "/root", "/lib",
    ];
    let canonical_str = canonical.to_string_lossy();
    for d in &dangerous {
        if canonical_str.starts_with(d) {
            return Err(AppError::Other(format!(
                "Access denied: path targets a system directory ({d})"
            )));
        }
    }

    // Windows: block system directories
    #[cfg(target_os = "windows")]
    {
        let win_dangerous = ["C:\\Windows", "C:\\Program Files", "C:\\ProgramData"];
        for d in &win_dangerous {
            if canonical_str.to_lowercase().starts_with(&d.to_lowercase()) {
                return Err(AppError::Other(format!(
                    "Access denied: path targets a system directory ({d})"
                )));
            }
        }
    }

    // Block hidden files that could be used for persistence (e.g., .bashrc, .ssh/authorized_keys).
    let file_name = canonical.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let blocked_files = [
        ".bashrc",
        ".bash_profile",
        ".profile",
        ".ssh",
        "authorized_keys",
    ];
    for f in &blocked_files {
        if file_name == *f || canonical_str.contains(&format!("/{f}")) {
            return Err(AppError::Other(format!(
                "Access denied: path targets a sensitive file ({f})"
            )));
        }
    }

    Ok(canonical)
}

/// Load the full `AppConfig` (with migrations applied) from the DB inside
/// `spawn_blocking`, so the SQLite pool checkout + JSON parse never stall the
/// Tokio async runtime worker. `context` names the calling feature for error
/// messages (e.g. "chat", "audio", "preflight").
///
/// Returns a hard error if the settings can't be read — a silent fallback to
/// defaults would route requests to the wrong provider/model. Best-effort
/// single-field reads (with explicit fallbacks) stay at their call sites.
pub async fn load_app_config(
    db: &std::sync::Arc<Database>,
    context: &str,
) -> AppResult<medical_core::types::settings::AppConfig> {
    let db = std::sync::Arc::clone(db);
    let context = context.to_string();
    tokio::task::spawn_blocking(
        move || -> AppResult<medical_core::types::settings::AppConfig> {
            let conn = db
                .conn()
                .map_err(|e| AppError::Config(format!("Failed to load {context} settings: {e}")))?;
            let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
                .map_err(|e| AppError::Config(format!("Failed to load {context} settings: {e}")))?;
            cfg.migrate();
            Ok(cfg)
        },
    )
    .await
    .map_err(join_err)?
}

/// Resolve the recordings directory from settings.
///
/// If the user has configured a custom `storage_path`, use it.
/// Otherwise fall back to `{data_dir}/recordings`.
pub fn resolve_recordings_dir(db: &Database, data_dir: &Path) -> AppResult<PathBuf> {
    let dir = if let Ok(conn) = db.conn() {
        medical_db::settings::SettingsRepo::load_config(&conn)
            .ok()
            .map(|mut c| {
                c.migrate();
                c
            })
            .and_then(|cfg| cfg.storage_path.filter(|s| !s.is_empty()))
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("recordings"))
    } else {
        data_dir.join("recordings")
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Extract the inner payload from an `AppError`, avoiding thiserror's
/// category-prefix (e.g., `"Processing error: "`). Used when re-wrapping
/// an existing `AppError` so we don't double-prefix the stored/emitted message.
pub(super) fn unwrap_app_error_message(err: AppError) -> String {
    match err {
        AppError::Database { message: s, .. }
        | AppError::Security { message: s, .. }
        | AppError::Audio { message: s, .. }
        | AppError::AiProvider { message: s, .. }
        | AppError::SttProvider { message: s, .. }
        | AppError::TtsProvider { message: s, .. }
        | AppError::Agent { message: s, .. }
        | AppError::Rag { message: s, .. }
        | AppError::Processing { message: s, .. }
        | AppError::Export { message: s, .. }
        | AppError::Translation { message: s, .. }
        | AppError::Config(s)
        | AppError::InvalidInput(s)
        | AppError::MutexPoisoned(s)
        | AppError::HttpClient(s)
        | AppError::Other(s) => s,
        AppError::Io(e) => e.to_string(),
        AppError::Serialization(e) => e.to_string(),
        AppError::Cancelled => "Cancelled".to_string(),
        AppError::EndpointOffline { .. } => err.to_string(),
        AppError::InvalidEndpoint { .. } => err.to_string(),
    }
}

/// Borrowing variant of [`unwrap_app_error_message`] for sites that only
/// hold a reference (e.g., progress-emit strings inspecting a `&AppError`
/// from a `Result<_, AppError>`). `AppError` does not derive `Clone`
/// (because `std::io::Error` is not `Clone`), so we avoid moving/cloning
/// the error and return an owned `String` from the borrowed variants.
pub(super) fn unwrap_app_error_message_ref(err: &AppError) -> String {
    match err {
        AppError::Database { message: s, .. }
        | AppError::Security { message: s, .. }
        | AppError::Audio { message: s, .. }
        | AppError::AiProvider { message: s, .. }
        | AppError::SttProvider { message: s, .. }
        | AppError::TtsProvider { message: s, .. }
        | AppError::Agent { message: s, .. }
        | AppError::Rag { message: s, .. }
        | AppError::Processing { message: s, .. }
        | AppError::Export { message: s, .. }
        | AppError::Translation { message: s, .. }
        | AppError::Config(s)
        | AppError::InvalidInput(s)
        | AppError::MutexPoisoned(s)
        | AppError::HttpClient(s)
        | AppError::Other(s) => s.clone(),
        AppError::Io(e) => e.to_string(),
        AppError::Serialization(e) => e.to_string(),
        AppError::Cancelled => "Cancelled".to_string(),
        AppError::EndpointOffline { .. } => err.to_string(),
        AppError::InvalidEndpoint { .. } => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_app_error_message_strips_all_category_prefixes() {
        assert_eq!(
            unwrap_app_error_message(AppError::ai_provider("bad key".to_string())),
            "bad key"
        );
        assert_eq!(
            unwrap_app_error_message(AppError::database("db down")),
            "db down"
        );
        assert_eq!(unwrap_app_error_message(AppError::Cancelled), "Cancelled");
    }

    #[test]
    fn unwrap_app_error_message_ref_strips_all_category_prefixes() {
        assert_eq!(
            unwrap_app_error_message_ref(&AppError::ai_provider("bad key".to_string())),
            "bad key"
        );
        assert_eq!(
            unwrap_app_error_message_ref(&AppError::database("db down")),
            "db down"
        );
        assert_eq!(
            unwrap_app_error_message_ref(&AppError::Cancelled),
            "Cancelled"
        );
    }
}
