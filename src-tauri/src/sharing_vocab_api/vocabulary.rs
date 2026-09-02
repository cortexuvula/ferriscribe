//! Vocabulary rule CRUD handlers for the `/v1/vocabulary` routes.

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use medical_core::types::vocabulary::{VocabularyCategory, VocabularyEntry};
use medical_db::Database;
use medical_db::vocabulary::VocabularyRepo;
use serde::Deserialize;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{ApiState, authorize};

#[derive(Deserialize)]
pub(super) struct ListQuery {
    pub(super) category: Option<String>,
}

pub(super) async fn list_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<VocabularyEntry>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let entries = tokio::task::spawn_blocking(
        move || -> Result<Vec<VocabularyEntry>, medical_core::error::AppError> {
            let conn = db.conn()?;
            match q.category {
                Some(cat) => {
                    let cat = VocabularyCategory::from_str(&cat);
                    VocabularyRepo::list_by_category(&conn, &cat)
                        .map_err(medical_core::error::AppError::from)
                }
                None => {
                    VocabularyRepo::list_all(&conn).map_err(medical_core::error::AppError::from)
                }
            }
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("vocab_api list failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    debug!(count = entries.len(), "vocab_api: list");
    Ok(Json(entries))
}

pub(super) async fn count_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
) -> Result<Json<(u32, u32)>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db: Arc<Database> = Arc::clone(&state.db);
    let counts = tokio::task::spawn_blocking(
        move || -> Result<(u32, u32), medical_core::error::AppError> {
            let conn = db.conn()?;
            VocabularyRepo::count(&conn).map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(counts))
}

#[derive(Deserialize)]
pub(super) struct UpsertBody {
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

pub(super) async fn insert_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Json(body): Json<UpsertBody>,
) -> Result<Json<VocabularyEntry>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let now = Utc::now();
    let entry = VocabularyEntry {
        id: Uuid::new_v4(),
        find_text: body.find_text,
        replacement: body.replacement,
        category: VocabularyCategory::from_str(&body.category.unwrap_or_default()),
        case_sensitive: body.case_sensitive.unwrap_or(false),
        priority: body.priority.unwrap_or(0),
        enabled: body.enabled.unwrap_or(true),
        created_at: now,
        updated_at: now,
    };
    let db = Arc::clone(&state.db);
    let entry_clone = entry.clone();
    let insert_result = tokio::task::spawn_blocking(move || -> Result<(), medical_db::DbError> {
        let conn = db.conn()?;
        VocabularyRepo::insert(&conn, &entry_clone)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match insert_result {
        Ok(()) => {}
        // A duplicate find_text is a client-input problem, not a server
        // fault — 409 lets the client show its already-exists message
        // instead of a generic 500.
        Err(e) if e.is_unique_violation() => return Err(StatusCode::CONFLICT),
        Err(e) => {
            // Constraint text names the column, not the row value — safe to log.
            warn!("vocab_api insert failed: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    info!(
        find_len = entry.find_text.len(),
        "vocab_api: inserted entry"
    );
    Ok(Json(entry))
}

pub(super) async fn update_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpsertBody>,
) -> Result<Json<VocabularyEntry>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let uuid = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = Arc::clone(&state.db);
    let db2 = Arc::clone(&state.db);
    let existing = tokio::task::spawn_blocking(
        move || -> Result<VocabularyEntry, medical_core::error::AppError> {
            let conn = db.conn()?;
            VocabularyRepo::get_by_id(&conn, &uuid).map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| match e {
        // Only a genuinely missing row is a 404 — a transient DB failure
        // must surface as 500, not masquerade as "deleted on the server".
        medical_core::error::AppError::Database { .. } => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    })?;

    let entry = VocabularyEntry {
        id: existing.id,
        find_text: body.find_text,
        replacement: body.replacement,
        category: VocabularyCategory::from_str(
            &body
                .category
                .unwrap_or_else(|| existing.category.as_str().to_string()),
        ),
        case_sensitive: body.case_sensitive.unwrap_or(existing.case_sensitive),
        priority: body.priority.unwrap_or(existing.priority),
        enabled: body.enabled.unwrap_or(existing.enabled),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };
    let entry_clone = entry.clone();
    tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        let conn = db2.conn()?;
        VocabularyRepo::update(&conn, &entry_clone).map_err(medical_core::error::AppError::from)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(entry))
}

pub(super) async fn delete_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let uuid = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        let conn = db.conn()?;
        VocabularyRepo::delete(&conn, &uuid).map_err(medical_core::error::AppError::from)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete_all_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
) -> Result<Json<u32>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let n = tokio::task::spawn_blocking(move || -> Result<u32, medical_core::error::AppError> {
        let conn = db.conn()?;
        VocabularyRepo::delete_all(&conn).map_err(medical_core::error::AppError::from)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    info!(count = n, "vocab_api: deleted all entries");
    Ok(Json(n))
}
