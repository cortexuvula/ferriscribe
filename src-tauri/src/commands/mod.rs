pub mod audio;
pub mod chat;
pub mod context_templates;
pub mod export;
pub mod generation;
pub mod logging;
pub mod models;
pub mod pipeline;
pub mod providers;
pub mod rag;
pub mod recordings;
pub mod recovery;
pub mod settings;
pub mod sharing;
pub mod transcription;
pub mod vocabulary;

use std::path::{Path, PathBuf};

use medical_core::error::{AppError, AppResult};
use medical_db::Database;

/// Resolve the recordings directory from settings.
///
/// If the user has configured a custom `storage_path`, use it.
/// Otherwise fall back to `{data_dir}/recordings`.
pub fn resolve_recordings_dir(db: &Database, data_dir: &Path) -> AppResult<PathBuf> {
    let dir = if let Ok(conn) = db.conn() {
        medical_db::settings::SettingsRepo::load_config(&conn)
            .ok()
            .map(|mut c| { c.migrate(); c })
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
        AppError::Database(s)
        | AppError::Security(s)
        | AppError::Audio(s)
        | AppError::AiProvider(s)
        | AppError::SttProvider(s)
        | AppError::TtsProvider(s)
        | AppError::Agent(s)
        | AppError::Rag(s)
        | AppError::Processing(s)
        | AppError::Export(s)
        | AppError::Translation(s)
        | AppError::Config(s)
        | AppError::Other(s) => s,
        AppError::Io(e) => e.to_string(),
        AppError::Serialization(e) => e.to_string(),
        AppError::Cancelled => "Cancelled".to_string(),
    }
}

/// Borrowing variant of [`unwrap_app_error_message`] for sites that only
/// hold a reference (e.g., progress-emit strings inspecting a `&AppError`
/// from a `Result<_, AppError>`). `AppError` does not derive `Clone`
/// (because `std::io::Error` is not `Clone`), so we avoid moving/cloning
/// the error and return an owned `String` from the borrowed variants.
pub(super) fn unwrap_app_error_message_ref(err: &AppError) -> String {
    match err {
        AppError::Database(s)
        | AppError::Security(s)
        | AppError::Audio(s)
        | AppError::AiProvider(s)
        | AppError::SttProvider(s)
        | AppError::TtsProvider(s)
        | AppError::Agent(s)
        | AppError::Rag(s)
        | AppError::Processing(s)
        | AppError::Export(s)
        | AppError::Translation(s)
        | AppError::Config(s)
        | AppError::Other(s) => s.clone(),
        AppError::Io(e) => e.to_string(),
        AppError::Serialization(e) => e.to_string(),
        AppError::Cancelled => "Cancelled".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_app_error_message_strips_all_category_prefixes() {
        assert_eq!(
            unwrap_app_error_message(AppError::AiProvider("bad key".to_string())),
            "bad key"
        );
        assert_eq!(
            unwrap_app_error_message(AppError::Database("db down".to_string())),
            "db down"
        );
        assert_eq!(
            unwrap_app_error_message(AppError::Cancelled),
            "Cancelled"
        );
    }

    #[test]
    fn unwrap_app_error_message_ref_strips_all_category_prefixes() {
        assert_eq!(
            unwrap_app_error_message_ref(&AppError::AiProvider("bad key".to_string())),
            "bad key"
        );
        assert_eq!(
            unwrap_app_error_message_ref(&AppError::Database("db down".to_string())),
            "db down"
        );
        assert_eq!(
            unwrap_app_error_message_ref(&AppError::Cancelled),
            "Cancelled"
        );
    }
}
