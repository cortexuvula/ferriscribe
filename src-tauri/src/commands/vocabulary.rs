use std::sync::Arc;

use chrono::Utc;
use serde::Deserialize;
use tracing::{info, instrument};
use uuid::Uuid;

use medical_core::error::{AppError, AppResult};
use medical_core::types::vocabulary::{CorrectionResult, VocabularyCategory, VocabularyEntry};
use medical_db::vocabulary::VocabularyRepo;
use medical_processing::vocabulary_corrector;

use crate::state::{self, AppState};
use crate::vocab_remote::{UpsertBody as RemoteUpsert, VocabRemote};

/// Returns `Some((conn, bearer))` when this client is paired with an office
/// server that advertised a vocab CRUD API. Vocabulary commands route
/// through HTTP in that case; otherwise they operate on the local SQLite
/// repo.
fn paired_vocab_target() -> Option<(crate::commands::sharing::PairedConnection, String)> {
    let conn = state::load_paired_connection()?;
    conn.ports.vocab?;
    let bearer = state::load_sharing_bearer()?;
    Some((conn, bearer))
}

/// List vocabulary entries, optionally filtered by category.
///
/// When this machine is paired with an office server, routes through the
/// server's HTTP API instead of the local SQLite repo.
#[tauri::command]
pub async fn list_vocabulary_entries(
    state: tauri::State<'_, AppState>,
    category: Option<String>,
) -> AppResult<Vec<VocabularyEntry>> {
    if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        return remote.list(category.as_deref()).await;
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        match category {
            Some(cat) => {
                let cat = VocabularyCategory::from_str(&cat);
                VocabularyRepo::list_by_category(&conn, &cat)
                    .map_err(|e| AppError::Database(e.to_string()))
            }
            None => {
                VocabularyRepo::list_all(&conn).map_err(|e| AppError::Database(e.to_string()))
            }
        }
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Add a new vocabulary correction entry.
///
/// When paired with an office server, the entry is created on the server
/// (which becomes the canonical source of truth). Otherwise it's inserted
/// into the local SQLite database.
#[tauri::command]
pub async fn add_vocabulary_entry(
    state: tauri::State<'_, AppState>,
    find_text: String,
    replacement: String,
    category: Option<String>,
    case_sensitive: Option<bool>,
    priority: Option<i32>,
    enabled: Option<bool>,
) -> AppResult<VocabularyEntry> {
    if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        let body = RemoteUpsert {
            find_text,
            replacement,
            category,
            case_sensitive,
            priority,
            enabled,
        };
        let entry = remote.insert(&body).await?;
        info!("Vocabulary entry added (remote)");
        return Ok(entry);
    }
    let now = Utc::now();
    let entry = VocabularyEntry {
        id: Uuid::new_v4(),
        find_text,
        replacement,
        category: VocabularyCategory::from_str(&category.unwrap_or_default()),
        case_sensitive: case_sensitive.unwrap_or(false),
        priority: priority.unwrap_or(0),
        enabled: enabled.unwrap_or(true),
        created_at: now,
        updated_at: now,
    };
    let db = Arc::clone(&state.db);
    let entry_clone = entry.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        VocabularyRepo::insert(&conn, &entry_clone).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;
    info!(find = %entry.find_text, "Vocabulary entry added");
    Ok(entry)
}

/// Update an existing vocabulary entry by UUID.
///
/// When paired with an office server, routes through the server's HTTP API.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri IPC: each field is named on the JS side.
pub async fn update_vocabulary_entry(
    state: tauri::State<'_, AppState>,
    id: String,
    find_text: String,
    replacement: String,
    category: Option<String>,
    case_sensitive: Option<bool>,
    priority: Option<i32>,
    enabled: Option<bool>,
) -> AppResult<VocabularyEntry> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| AppError::Other(format!("Invalid ID: {e}")))?;
    if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        let body = RemoteUpsert {
            find_text,
            replacement,
            category,
            case_sensitive,
            priority,
            enabled,
        };
        return remote.update(uuid, &body).await;
    }
    let db = Arc::clone(&state.db);
    let db2 = Arc::clone(&state.db);

    let existing = tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        VocabularyRepo::get_by_id(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    let entry = VocabularyEntry {
        id: existing.id,
        find_text,
        replacement,
        category: VocabularyCategory::from_str(
            &category.unwrap_or_else(|| existing.category.as_str().to_string()),
        ),
        case_sensitive: case_sensitive.unwrap_or(existing.case_sensitive),
        priority: priority.unwrap_or(existing.priority),
        enabled: enabled.unwrap_or(existing.enabled),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    let entry_clone = entry.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db2.conn().map_err(|e| AppError::Database(e.to_string()))?;
        VocabularyRepo::update(&conn, &entry_clone).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;
    Ok(entry)
}

/// Delete a single vocabulary entry by UUID.
#[tauri::command]
pub async fn delete_vocabulary_entry(
    state: tauri::State<'_, AppState>,
    id: String,
) -> AppResult<()> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| AppError::Other(format!("Invalid ID: {e}")))?;
    if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        return remote.delete(uuid).await;
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        VocabularyRepo::delete(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Delete all vocabulary entries.
#[tauri::command]
pub async fn delete_all_vocabulary_entries(
    state: tauri::State<'_, AppState>,
) -> AppResult<u32> {
    if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        return remote.delete_all().await;
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        VocabularyRepo::delete_all(&conn).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Get vocabulary counts as `(total, enabled)`.
#[tauri::command]
pub async fn get_vocabulary_count(
    state: tauri::State<'_, AppState>,
) -> AppResult<(u32, u32)> {
    if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        return remote.count().await;
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        VocabularyRepo::count(&conn).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Import vocabulary entries from a JSON file.
///
/// Accepts either a bare array or a `{ "corrections": [...] }` wrapper.
/// Returns the number of entries imported.
#[tauri::command]
#[instrument(skip(state))]
pub async fn import_vocabulary_json(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> AppResult<u32> {
    let content = tokio::fs::read_to_string(&file_path).await?;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ImportFile {
        Wrapped { corrections: Vec<ImportEntry> },
        Bare(Vec<ImportEntry>),
    }
    #[derive(Deserialize)]
    struct ImportEntry {
        find_text: String,
        replacement: String,
        #[serde(default)]
        category: Option<String>,
        #[serde(default)]
        case_sensitive: Option<bool>,
        #[serde(default)]
        priority: Option<i32>,
        #[serde(default)]
        enabled: Option<bool>,
    }

    let data: ImportFile = serde_json::from_str(&content)?;
    let corrections = match data {
        ImportFile::Wrapped { corrections } => corrections,
        ImportFile::Bare(list) => list,
    };

    let now = Utc::now();
    let entries: Vec<VocabularyEntry> = corrections
        .into_iter()
        .filter(|e| !e.find_text.trim().is_empty() && !e.replacement.trim().is_empty())
        .map(|e| VocabularyEntry {
            id: Uuid::new_v4(),
            find_text: e.find_text.trim().to_string(),
            replacement: e.replacement.trim().to_string(),
            category: VocabularyCategory::from_str(&e.category.unwrap_or_default()),
            case_sensitive: e.case_sensitive.unwrap_or(false),
            priority: e.priority.unwrap_or(0),
            enabled: e.enabled.unwrap_or(true),
            created_at: now,
            updated_at: now,
        })
        .collect();

    let count = entries.len() as u32;
    if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        for entry in &entries {
            let body = RemoteUpsert {
                find_text: entry.find_text.clone(),
                replacement: entry.replacement.clone(),
                category: Some(entry.category.as_str().to_string()),
                case_sensitive: Some(entry.case_sensitive),
                priority: Some(entry.priority),
                enabled: Some(entry.enabled),
            };
            remote.insert(&body).await?;
        }
        info!(count, path = %file_path, "Imported vocabulary entries (remote)");
        return Ok(count);
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        for entry in &entries {
            VocabularyRepo::upsert(&conn, entry).map_err(|e| AppError::Database(e.to_string()))?;
        }
        Ok::<_, AppError>(())
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    info!(count, path = %file_path, "Imported vocabulary entries");
    Ok(count)
}

/// Export all vocabulary entries to a JSON file.
///
/// Writes a `{ "version": "1.0", "corrections": [...] }` document.
/// Returns the number of entries exported.
#[tauri::command]
#[instrument(skip(state))]
pub async fn export_vocabulary_json(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> AppResult<u32> {
    let entries: Vec<VocabularyEntry> = if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        remote.list(None).await?
    } else {
        let db = Arc::clone(&state.db);
        tokio::task::spawn_blocking(move || {
            let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
            VocabularyRepo::list_all(&conn).map_err(|e| AppError::Database(e.to_string()))
        })
        .await
        .map_err(|e| AppError::Other(format!("Task join error: {e}")))??
    };

    let count = entries.len() as u32;
    let export = serde_json::json!({
        "version": "1.0",
        "corrections": entries.iter().map(|e| serde_json::json!({
            "find_text": e.find_text,
            "replacement": e.replacement,
            "category": e.category.as_str(),
            "case_sensitive": e.case_sensitive,
            "priority": e.priority,
            "enabled": e.enabled,
        })).collect::<Vec<_>>()
    });

    let json = serde_json::to_string_pretty(&export)?;
    tokio::fs::write(&file_path, json).await?;

    info!(count, path = %file_path, "Exported vocabulary entries");
    Ok(count)
}

/// Apply all enabled vocabulary corrections to the given text.
///
/// Returns a `CorrectionResult` with the corrected text and per-entry
/// substitution counts. Used by the frontend's "test correction" feature.
#[tauri::command]
pub async fn test_vocabulary_correction(
    state: tauri::State<'_, AppState>,
    text: String,
) -> AppResult<CorrectionResult> {
    let entries: Vec<VocabularyEntry> = if let Some((conn, bearer)) = paired_vocab_target() {
        let remote = VocabRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired vocab target unavailable".into()))?;
        remote
            .list(None)
            .await?
            .into_iter()
            .filter(|e| e.enabled)
            .collect()
    } else {
        let db = Arc::clone(&state.db);
        tokio::task::spawn_blocking(move || {
            let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
            VocabularyRepo::list_enabled(&conn).map_err(|e| AppError::Database(e.to_string()))
        })
        .await
        .map_err(|e| AppError::Other(format!("Task join error: {e}")))??
    };

    Ok(vocabulary_corrector::apply_corrections(&text, &entries))
}
