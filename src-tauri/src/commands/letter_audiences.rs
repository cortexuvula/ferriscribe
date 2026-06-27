use std::sync::Arc;

use chrono::Utc;
use tracing::info;
use uuid::Uuid;

use medical_core::error::{AppError, AppResult};
use medical_core::types::LetterAudience;
use medical_db::LetterAudiencesRepo;

use crate::state::AppState;

#[tauri::command]
pub async fn list_letter_audiences(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<LetterAudience>> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        LetterAudiencesRepo::list_all(&conn).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn upsert_letter_audience(
    state: tauri::State<'_, AppState>,
    mut audience: LetterAudience,
) -> AppResult<LetterAudience> {
    // Prevent creating new audiences with is_builtin=true
    if audience.id == Uuid::nil() && audience.is_builtin {
        return Err(AppError::Other(
            "Cannot create new audiences with is_builtin=true".into(),
        ));
    }

    let now = Utc::now();
    if audience.id == Uuid::nil() {
        audience.id = Uuid::new_v4();
        audience.created_at = now;
    }
    audience.updated_at = now;

    let db = Arc::clone(&state.db);
    let audience_clone = audience.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        LetterAudiencesRepo::upsert(&conn, &audience_clone)
            .map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;
    // PHI guardrail: audience names are physician-authored free text that
    // could embed a recipient name; log the id + name length only.
    info!(
        id = %audience.id,
        name_len = audience.name.chars().count(),
        "Upserted letter audience"
    );
    Ok(audience)
}

#[tauri::command]
pub async fn delete_letter_audience(state: tauri::State<'_, AppState>, id: Uuid) -> AppResult<()> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        LetterAudiencesRepo::delete(&conn, &id).map_err(|e| match e {
            medical_db::DbError::Constraint(msg) => AppError::Other(msg),
            medical_db::DbError::NotFound(msg) => AppError::Other(msg),
            other => AppError::Database(other.to_string()),
        })
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;
    info!(%id, "Deleted letter audience");
    Ok(())
}
