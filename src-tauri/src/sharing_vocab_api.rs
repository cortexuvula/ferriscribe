//! Server-canonical sync APIs for paired clients (vocabulary, context
//! templates, and the per-user spellcheck dictionary). Lives on the
//! office server alongside the sharing service. Bearer-validated against
//! the same `TokenStore` the auth proxy uses, so any client whose
//! pairing has been revoked is rejected here too.
//!
//! Routes:
//!   /v1/vocabulary
//!     GET    /                       — list (or by ?category=)
//!     GET    /count                  — (total, enabled)
//!     POST   /                       — insert
//!     PUT    /:id                    — replace by uuid
//!     DELETE /:id                    — delete one
//!     DELETE /                       — delete all
//!   /v1/context-templates
//!     GET    /                       — list (sorted by name)
//!     POST   /upsert                 — { name, body }
//!     POST   /rename                 — { old_name, new_name }
//!     POST   /delete                 — { name }
//!   /v1/user-dictionary
//!     GET    /                       — list all words
//!     POST   /                       — add word { word }
//!     DELETE /{word}                 — remove word
//!   /v1/condition-chips
//!     GET    /                       — list active chips
//!     POST   /sync                   — two-way merge (client → server)
//!
//! Wire formats reuse the existing `VocabularyEntry` and `ContextTemplate`
//! serde definitions, so clients deserialize directly into the same types
//! used for the local DB. The dictionary uses `Vec<String>` (list) and
//! `bool` (add/remove outcome).
//!
//! Vocabulary and context templates carry no patient content. Dictionary
//! words MAY include patient-context-specific terms a clinician added,
//! so the dictionary handlers log only word lengths and boolean outcomes
//! — never the word value. Request paths and status codes remain safe to
//! log for all three route groups.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State as AxumState},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post, put},
};
use chrono::Utc;
use futures_util::Stream;
use medical_core::types::recording::Recording;
use medical_core::types::settings::ContextTemplate;
use medical_core::types::vocabulary::{VocabularyCategory, VocabularyEntry};
use medical_db::content_sync::{
    ContentSyncRepo, FieldRevision, MergeConflict, SyncFieldValue, SyncRecording,
};
use medical_db::{
    Database, recordings::RecordingsRepo, settings::SettingsRepo, vocabulary::VocabularyRepo,
};
use medical_security::file_crypto;
use medical_sharing::token_store::TokenStore;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Internal Axum state shared across all vocab/template/dictionary route
/// handlers. Holds the database and token store needed to validate bearer
/// tokens and read/write data.
#[derive(Clone)]
struct ApiState {
    db: Arc<Database>,
    tokens: Arc<TokenStore>,
    /// Broadcasts `()` whenever condition chips change on the server so SSE
    /// subscribers can push realtime notifications to their clients.
    chips_changed_tx: broadcast::Sender<()>,
    /// Broadcasts a recording ID (or `"*"` for all) whenever content-sync
    /// push merges new data, so SSE subscribers can refresh in near-realtime.
    content_changed_tx: broadcast::Sender<String>,
    /// App data dir; used to resolve the recordings directory for audio
    /// upload/download.
    data_dir: PathBuf,
    /// Tauri app handle for emitting events to THIS machine's own frontend.
    /// When a remote client pushes recordings into the server's DB, the
    /// server's webview would otherwise never learn about the new rows
    /// (the SSE channel only notifies *other* clients). We emit a
    /// `recording-updated` Tauri event per changed ID so the server's own
    /// Recordings view reloads — mirroring what the client does on pull.
    app_handle: AppHandle,
}

/// Spawn the vocab/templates/dictionary HTTP API server on `0.0.0.0:{port}`.
///
/// Returns a `JoinHandle` for the server task. The server runs until the
/// handle is dropped (which happens when sharing is stopped). Bearer tokens
/// are validated against the same `TokenStore` the auth proxy uses.
pub async fn spawn(
    db: Arc<Database>,
    tokens: Arc<TokenStore>,
    port: u16,
    data_dir: PathBuf,
    app_handle: AppHandle,
) -> Result<JoinHandle<()>, medical_core::error::AppError> {
    let (chips_changed_tx, _) = broadcast::channel::<()>(16);
    let (content_changed_tx, _) = broadcast::channel::<String>(32);
    let state = ApiState {
        db,
        tokens,
        chips_changed_tx,
        content_changed_tx,
        data_dir,
        app_handle,
    };
    let app = Router::new()
        .route(
            "/v1/vocabulary",
            get(list_handler)
                .post(insert_handler)
                .delete(delete_all_handler),
        )
        .route("/v1/vocabulary/count", get(count_handler))
        .route(
            "/v1/vocabulary/{id}",
            put(update_handler).delete(delete_handler),
        )
        .route("/v1/context-templates", get(templates_list_handler))
        .route(
            "/v1/context-templates/upsert",
            axum::routing::post(templates_upsert_handler),
        )
        .route(
            "/v1/context-templates/rename",
            axum::routing::post(templates_rename_handler),
        )
        .route(
            "/v1/context-templates/delete",
            axum::routing::post(templates_delete_handler),
        )
        .route(
            "/v1/user-dictionary",
            get(dict_list_handler).post(dict_add_handler),
        )
        .route(
            "/v1/user-dictionary/{word}",
            axum::routing::delete(dict_remove_handler),
        )
        .route("/v1/condition-chips", get(condition_chips_list_handler))
        .route(
            "/v1/condition-chips/sync",
            post(condition_chips_sync_handler),
        )
        .route(
            "/v1/condition-chips/events",
            get(condition_chips_events_handler),
        )
        .route(
            "/v1/content/sync",
            get(content_sync_pull_handler).post(content_sync_push_handler),
        )
        .route("/v1/content/sync/meta", get(content_sync_meta_handler))
        .route("/v1/content/events", get(content_events_handler))
        .route(
            "/v1/content/audio/{recording_id}",
            get(content_audio_get_handler).put(content_audio_put_handler),
        )
        // Allow large bodies (up to 256 MiB) for audio upload/download.
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .with_state(state);

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| format!("vocab_api bind addr parse: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("vocab_api bind {addr}: {e}"))?;
    info!(port, "vocab API listening");
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!("vocab_api serve exited: {e}");
        }
    }))
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    v.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

fn authorize(state: &ApiState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    let token = extract_bearer(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let row = state
        .tokens
        .validate(&token)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let _ = state.tokens.touch(row.id);
    Ok(row.id)
}

// ── Content-sync DTOs ───────────────────────────────────────────────────
//
// Wire-format request/response types for the content-sync routes
// (/v1/content/*). These mirror the types in `medical_db::content_sync`
// but are kept here as thin serde wrappers so the handler signatures read
// clearly. No PHI appears in these types beyond what the client already
// sent us; logs emit counts and lengths only.

#[derive(Deserialize)]
struct SyncSinceQuery {
    since: Option<String>,
    limit: Option<usize>,
}

#[derive(serde::Serialize)]
struct ContentPullResponse {
    recordings: Vec<SyncRecording>,
    server_time: String,
    has_more: bool,
}

#[derive(Deserialize)]
struct ContentPushRequest {
    recordings: Vec<SyncRecording>,
}

#[derive(serde::Serialize)]
struct ContentPushResponse {
    conflicts: Vec<MergeConflict>,
    server_time: String,
}

#[derive(serde::Serialize)]
struct ContentMetaResponse {
    server_time: String,
    recording_count: i64,
    latest_updated_at: Option<String>,
}

#[derive(Deserialize)]
struct ListQuery {
    category: Option<String>,
}

async fn list_handler(
    AxumState(state): AxumState<ApiState>,
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

async fn count_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<(u32, u32)>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
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
struct UpsertBody {
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

async fn insert_handler(
    AxumState(state): AxumState<ApiState>,
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
    tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        let conn = db.conn()?;
        VocabularyRepo::insert(&conn, &entry_clone).map_err(medical_core::error::AppError::from)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("vocab_api insert failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    info!(
        find_len = entry.find_text.len(),
        "vocab_api: inserted entry"
    );
    Ok(Json(entry))
}

async fn update_handler(
    AxumState(state): AxumState<ApiState>,
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
    .map_err(|_| StatusCode::NOT_FOUND)?;

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

async fn delete_handler(
    AxumState(state): AxumState<ApiState>,
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

async fn delete_all_handler(
    AxumState(state): AxumState<ApiState>,
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

// ── Context templates handlers ───────────────────────────────────────────
//
// Templates live inside AppConfig.custom_context_templates (Vec<ContextTemplate>)
// in the SQLCipher settings row, so each mutation is read-modify-write of
// the whole settings blob. SettingsRepo is sync; we wrap in spawn_blocking
// to keep the axum runtime free.

#[derive(Deserialize)]
struct TemplateUpsertBody {
    name: String,
    body: String,
}

#[derive(Deserialize)]
struct TemplateRenameBody {
    old_name: String,
    new_name: String,
}

#[derive(Deserialize)]
struct TemplateDeleteBody {
    name: String,
}

fn ctx_templates_load_sorted(
    db: &Database,
) -> Result<Vec<ContextTemplate>, medical_core::error::AppError> {
    let conn = db.conn()?;
    let mut cfg = SettingsRepo::load_config(&conn)?;
    cfg.migrate();
    let mut t = cfg.custom_context_templates;
    t.sort_by_key(|a| a.name.to_lowercase());
    Ok(t)
}

async fn templates_list_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ContextTemplate>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let list = tokio::task::spawn_blocking(move || ctx_templates_load_sorted(&db))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("templates list failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    debug!(count = list.len(), "vocab_api: templates list");
    Ok(Json(list))
}

async fn templates_upsert_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<TemplateUpsertBody>,
) -> Result<Json<ContextTemplate>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let name = body.name.trim().to_string();
    let body_text = body.body.trim().to_string();
    if name.is_empty() || body_text.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = Arc::clone(&state.db);
    let entry = tokio::task::spawn_blocking(
        move || -> Result<ContextTemplate, medical_core::error::AppError> {
            let conn = db.conn()?;
            let mut cfg = SettingsRepo::load_config(&conn)?;
            cfg.migrate();
            let entry = ContextTemplate {
                name: name.clone(),
                body: body_text.clone(),
            };
            if let Some(existing) = cfg
                .custom_context_templates
                .iter_mut()
                .find(|t| t.name == name)
            {
                existing.body = body_text;
            } else {
                cfg.custom_context_templates.push(entry.clone());
            }
            cfg.custom_context_templates
                .sort_by_key(|a| a.name.to_lowercase());
            SettingsRepo::save_config(&conn, &cfg)?;
            Ok(entry)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("templates upsert failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    info!(name_len = entry.name.len(), "vocab_api: template upserted");
    Ok(Json(entry))
}

async fn templates_rename_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<TemplateRenameBody>,
) -> Result<Json<ContextTemplate>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let new_name = body.new_name.trim().to_string();
    if new_name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let old_name = body.old_name;
    let db = Arc::clone(&state.db);
    let entry = tokio::task::spawn_blocking(
        move || -> Result<ContextTemplate, medical_core::error::AppError> {
            let conn = db.conn()?;
            let mut cfg = SettingsRepo::load_config(&conn)?;
            cfg.migrate();
            if old_name == new_name {
                return cfg
                    .custom_context_templates
                    .iter()
                    .find(|t| t.name == old_name)
                    .cloned()
                    .ok_or(medical_core::error::AppError::Other(format!(
                        "'{old_name}' not found"
                    )));
            }
            if cfg
                .custom_context_templates
                .iter()
                .any(|t| t.name == new_name)
            {
                return Err(medical_core::error::AppError::Other(format!(
                    "'{new_name}' already exists"
                )));
            }
            let idx = cfg
                .custom_context_templates
                .iter()
                .position(|t| t.name == old_name)
                .ok_or(medical_core::error::AppError::Other(format!(
                    "'{old_name}' not found"
                )))?;
            cfg.custom_context_templates[idx].name = new_name.clone();
            let renamed = cfg.custom_context_templates[idx].clone();
            cfg.custom_context_templates
                .sort_by_key(|a| a.name.to_lowercase());
            SettingsRepo::save_config(&conn, &cfg)?;
            Ok(renamed)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("templates rename: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(entry))
}

async fn templates_delete_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<TemplateDeleteBody>,
) -> Result<StatusCode, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let name = body.name;
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        let conn = db.conn()?;
        let mut cfg = SettingsRepo::load_config(&conn)?;
        cfg.migrate();
        let idx = cfg
            .custom_context_templates
            .iter()
            .position(|t| t.name == name)
            .ok_or(medical_core::error::AppError::Other(format!(
                "'{name}' not found"
            )))?;
        cfg.custom_context_templates.remove(idx);
        SettingsRepo::save_config(&conn, &cfg)?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("templates delete: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

// ── User dictionary handlers ────────────────────────────────────────────
//
// Per-user spellcheck wordlist. Reads/writes hit
// `medical_db::user_dictionary::UserDictionaryRepo` against the office
// server's local SQLite DB. Same bearer auth + spawn_blocking pattern as
// the vocab handlers above. No PHI in logs.

#[derive(Deserialize)]
struct DictAddBody {
    word: String,
}

async fn dict_list_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let words = tokio::task::spawn_blocking(
        move || -> Result<Vec<String>, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::list(&conn)
                .map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("dict_api list failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    debug!(count = words.len(), "dict_api: list");
    Ok(Json(words))
}

async fn dict_add_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<DictAddBody>,
) -> Result<Json<bool>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let word = body.word;
    let word_len = word.len();
    let added =
        tokio::task::spawn_blocking(move || -> Result<bool, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word)
                .map_err(medical_core::error::AppError::from)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("dict_api add failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!(word_len, added, "dict_api: add");
    Ok(Json(added))
}

async fn dict_remove_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Path(word): Path<String>,
) -> Result<Json<bool>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let word_len = word.len();
    let removed =
        tokio::task::spawn_blocking(move || -> Result<bool, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word)
                .map_err(medical_core::error::AppError::from)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("dict_api remove failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!(word_len, removed, "dict_api: remove");
    Ok(Json(removed))
}

// ── Condition chips handlers ────────────────────────────────────────────
//
// Practice-wide quick-add condition presets stored in the dedicated
// `condition_chips` table (not the settings blob). Reads/writes hit
// `medical_db::condition_chips::ConditionChipsRepo` directly — the same
// pattern as the vocabulary handlers above. Deletion is soft (tombstoned),
// so a two-way merge can propagate add/remove across machines. No PHI in
// logs; only counts and lengths are logged.

/// GET /v1/condition-chips — return all active condition chips.
async fn condition_chips_list_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<medical_core::types::condition_chip::ConditionChip>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let chips = tokio::task::spawn_blocking(
        move || -> Result<Vec<medical_core::types::condition_chip::ConditionChip>, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::condition_chips::ConditionChipsRepo::list_active(&conn)
                .map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("condition_chips_api list failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    debug!(count = chips.len(), "condition_chips_api: list");
    Ok(Json(chips))
}

/// POST /v1/condition-chips/sync — two-way merge.
///
/// Body: the client's full chip list (active chips + tombstones).
/// Returns: the merged active chip list after applying last-write-wins.
async fn condition_chips_sync_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(incoming): Json<Vec<medical_core::types::condition_chip::ConditionChip>>,
) -> Result<Json<Vec<medical_core::types::condition_chip::ConditionChip>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);

    let incoming_count = incoming.len();

    // Prune old tombstones opportunistically (30 days). Best-effort — a
    // prune failure must not fail the sync.
    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    let cutoff_iso = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let merged = tokio::task::spawn_blocking(
        move || -> Result<Vec<medical_core::types::condition_chip::ConditionChip>, medical_core::error::AppError> {
            let conn = db.conn()?;
            let result = medical_db::condition_chips::ConditionChipsRepo::merge_incoming(&conn, &incoming)
                .map_err(medical_core::error::AppError::from)?;
            // Best-effort prune — don't fail the sync if pruning errors.
            let _ = medical_db::condition_chips::ConditionChipsRepo::prune_tombstones(&conn, &cutoff_iso);
            Ok(result)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("condition_chips_api sync failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Notify SSE subscribers that chips changed. Best-effort: no receivers is
    // not an error (send returns Err only when there are no active receivers,
    // which is the normal idle case).
    let _ = state.chips_changed_tx.send(());

    info!(
        incoming_count,
        result_count = merged.len(),
        "condition_chips_api: sync"
    );
    Ok(Json(merged))
}

/// GET /v1/condition-chips/events — Server-Sent Events stream.
///
/// Pushes a `data: connected` event immediately on connection, then a
/// `data: changed` event each time a condition-chips sync completes on the
/// server. Clients use this to refresh their local chip list in near-realtime
/// instead of waiting for the 30s poll. The stream stays open until the client
/// disconnects or the server shuts down.
async fn condition_chips_events_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let mut rx = state.chips_changed_tx.subscribe();
    let stream = async_stream::stream! {
        yield Ok(Event::default().data("connected"));
        loop {
            match rx.recv().await {
                Ok(()) => yield Ok(Event::default().data("changed")),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream))
}

// ── Content-sync handlers ───────────────────────────────────────────────
//
// Bidirectional content sync for recordings (transcript, SOAP, referral,
// letter, etc.) operating at per-field granularity via last-write-wins.
// Audio files are synced through dedicated GET/PUT endpoints. All DB work
// runs inside `spawn_blocking`. No PHI (transcript/SOAP/recording content)
// is ever logged — only counts, IDs, and lengths.

/// Build the sparse `fields` map for a `SyncRecording` from a `Recording`
/// row plus its optional field revisions.
///
/// For each syncable text field that has content on the recording row, we
/// look up the matching revision (if any) to get the precise `updated_at`
/// and `origin_device`; otherwise we fall back to the recording's row-level
/// `updated_at`. Only fields with content are included — the map is sparse
/// by design so absent fields don't participate in the merge.
fn build_sparse_fields(
    rec: &Recording,
    revisions: Option<&Vec<FieldRevision>>,
) -> HashMap<String, SyncFieldValue> {
    let rev_map: HashMap<&str, &FieldRevision> = revisions
        .map(|v| v.iter().map(|r| (r.field.as_str(), r)).collect())
        .unwrap_or_default();

    let row_ts = rec
        .updated_at
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| rec.created_at.to_rfc3339());

    let mut fields: HashMap<String, SyncFieldValue> = HashMap::new();

    // Text columns: value is a JSON string when present.
    let mut push_text = |name: &str, val: Option<&str>| {
        if let Some(s) = val {
            let (ts, device) = rev_map
                .get(name)
                .map(|r| (r.updated_at.clone(), r.origin_device.clone()))
                .unwrap_or_else(|| (row_ts.clone(), None));
            fields.insert(
                name.to_string(),
                SyncFieldValue {
                    value: serde_json::Value::String(s.to_string()),
                    updated_at: ts,
                    origin_device: device,
                },
            );
        }
    };

    push_text("transcript", rec.transcript.as_deref());
    push_text("soap_note", rec.soap_note.as_deref());
    push_text("referral", rec.referral.as_deref());
    push_text("letter", rec.letter.as_deref());
    push_text("peer_discussion", rec.peer_discussion.as_deref());
    push_text("chat", rec.chat.as_deref());
    push_text("patient_name", rec.patient_name.as_deref());

    // JSON columns: tags, metadata, processing_status. These store the
    // serialized JSON value directly.
    let mut push_json = |name: &str, val: &serde_json::Value| {
        if !val.is_null() {
            let (ts, device) = rev_map
                .get(name)
                .map(|r| (r.updated_at.clone(), r.origin_device.clone()))
                .unwrap_or_else(|| (row_ts.clone(), None));
            fields.insert(
                name.to_string(),
                SyncFieldValue {
                    value: val.clone(),
                    updated_at: ts,
                    origin_device: device,
                },
            );
        }
    };

    // tags is a Vec<String> on the struct; serialize to JSON.
    if let Ok(tags_json) = serde_json::to_value(&rec.tags) {
        push_json("tags", &tags_json);
    }
    // Strip synced_from from metadata before transmitting — it's a local-only
    // marker that must not round-trip back to the origin machine.
    let mut metadata_clean = rec.metadata.clone();
    if let Some(obj) = metadata_clean.as_object_mut() {
        obj.remove("synced_from");
    }
    push_json("metadata", &metadata_clean);
    let status_json = serde_json::to_value(&rec.status).unwrap_or(serde_json::Value::Null);
    push_json("processing_status", &status_json);

    fields
}

/// Convert a `Recording` row into a wire-format `SyncRecording`.
///
/// `deleted_at` is read separately (it's not on the `Recording` struct).
fn recording_to_sync(
    rec: &Recording,
    deleted_at: Option<String>,
    revisions: Option<&Vec<FieldRevision>>,
) -> SyncRecording {
    SyncRecording {
        id: rec.id.to_string(),
        filename: rec.filename.clone(),
        created_at: rec.created_at.to_rfc3339(),
        updated_at: rec
            .updated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| rec.created_at.to_rfc3339()),
        deleted_at,
        patient_name: rec.patient_name.clone(),
        duration_seconds: rec.duration_seconds,
        file_size_bytes: rec.file_size_bytes,
        stt_provider: rec.stt_provider.clone(),
        ai_provider: rec.ai_provider.clone(),
        fields: build_sparse_fields(rec, revisions),
    }
}

/// Load a batch of recordings (including soft-deleted ones) as wire-format
/// `SyncRecording`s, with their field revisions attached.
///
/// `get_many` on the repo filters out deleted rows, so we run a custom
/// query here that includes the `deleted_at` column. Returns at most the
/// number of IDs supplied.
fn load_sync_recordings(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<Vec<SyncRecording>, medical_core::error::AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT id, filename, transcript, soap_note, referral, letter, peer_discussion, chat,
                patient_name, audio_path, duration_seconds, file_size_bytes,
                stt_provider, ai_provider, tags, processing_status, created_at, metadata,
                updated_at, deleted_at
         FROM recordings
         WHERE id IN ({placeholders})"
    );
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| medical_core::error::AppError::from(medical_db::DbError::from(e)))?;
    let rows: Vec<(Recording, Option<String>)> = stmt
        .query_map(params.as_slice(), |row| {
            let rec = RecordingsRepo::row_to_recording(row)?;
            let deleted_at: Option<String> = row.get(19)?;
            Ok((rec, deleted_at))
        })
        .map_err(|e| medical_core::error::AppError::from(medical_db::DbError::from(e)))?
        .filter_map(|r| {
            r.map_err(|e| warn!(error = %e, "dropping unreadable sync row"))
                .ok()
        })
        .collect();

    // Bulk-load revisions for the batch.
    let rec_ids: Vec<Uuid> = rows.iter().map(|(r, _)| r.id).collect();
    let rev_map = ContentSyncRepo::revisions_for_batch(conn, &rec_ids)
        .map_err(medical_core::error::AppError::from)?;

    let out = rows
        .into_iter()
        .map(|(rec, deleted_at)| {
            let id_str = rec.id.to_string();
            let revs = rev_map.get(&id_str);
            recording_to_sync(&rec, deleted_at, revs)
        })
        .collect();
    Ok(out)
}

/// GET /v1/content/sync — incremental delta pull.
///
/// Query params: `since` (RFC 3339 watermark, omit for initial full pull),
/// `limit` (default 200, max 500). Returns changed recordings ordered by
/// `updated_at` ascending so the client can page through with the last
/// item's `updated_at` as the next cursor.
async fn content_sync_pull_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Query(q): Query<SyncSinceQuery>,
) -> Result<Json<ContentPullResponse>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let limit = q.limit.unwrap_or(200).clamp(1, 500) as u32;
    let since = q.since;
    let db = Arc::clone(&state.db);

    let recordings = tokio::task::spawn_blocking(
        move || -> Result<(Vec<SyncRecording>, bool), medical_core::error::AppError> {
            let conn = db.conn()?;
            let (ids, has_more) = ContentSyncRepo::changed_since(&conn, since.as_deref(), limit)
                .map_err(medical_core::error::AppError::from)?;
            let recs = load_sync_recordings(&conn, &ids)?;
            Ok((recs, has_more))
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("content_sync pull failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (recordings, has_more) = recordings;

    info!(count = recordings.len(), has_more, "content_sync: pull");
    Ok(Json(ContentPullResponse {
        recordings,
        server_time: Utc::now().to_rfc3339(),
        has_more,
    }))
}

/// POST /v1/content/sync — push (two-way merge).
///
/// Body: `ContentPushRequest` with the client's changed recordings. The
/// server merges each field via last-write-wins and returns the fields
/// where the server's local copy won (conflicts). After a successful merge
/// the server broadcasts on `content_changed_tx` so other SSE-connected
/// clients refresh.
async fn content_sync_push_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(req): Json<ContentPushRequest>,
) -> Result<Json<ContentPushResponse>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let incoming_count = req.recordings.len();

    let result = tokio::task::spawn_blocking(
        move || -> Result<medical_db::content_sync::MergeResult, medical_core::error::AppError> {
            let conn = db.conn()?;
            ContentSyncRepo::merge_incoming(&conn, &req.recordings)
                .map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("content_sync push failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Notify SSE subscribers that content changed. Best-effort: no
    // receivers is not an error.
    let _ = state.content_changed_tx.send("*".to_string());

    // Emit a `recording-updated` Tauri event for each changed recording so
    // THIS server's own webview refreshes. Without this, the server's
    // Recordings view never learns about rows a remote client just pushed
    // (the broadcast above only notifies *other* clients over SSE). Mirrors
    // what the client does after a pull in content_sync.rs.
    for id in &result.changed_recording_ids {
        let _ = state
            .app_handle
            .emit("recording-updated", serde_json::json!({ "id": id }));
    }

    // Emit `content-sync-complete` so the server's ContentSync settings panel
    // updates its "Last synced" timestamp. Without this, the server's panel
    // always shows "never" because run_sync only runs on clients.
    let _ = state.app_handle.emit(
        "content-sync-complete",
        serde_json::json!({
            "pulled": 0,
            "pushed": incoming_count,
            "merge_conflicts": result.conflicts.len(),
            "push_conflicts": 0,
        }),
    );

    info!(
        incoming_count,
        conflict_count = result.conflicts.len(),
        changed_count = result.changed_recording_ids.len(),
        "content_sync: push"
    );
    Ok(Json(ContentPushResponse {
        conflicts: result.conflicts,
        server_time: Utc::now().to_rfc3339(),
    }))
}

/// GET /v1/content/sync/meta — server diagnostics for sync clients.
///
/// Returns the current recording count (non-deleted), the latest
/// `updated_at` watermark, and the server time. Clients use this to decide
/// whether a full re-pull is warranted.
async fn content_sync_meta_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<ContentMetaResponse>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);

    let (count, latest) = tokio::task::spawn_blocking(
        move || -> Result<(i64, Option<String>), medical_core::error::AppError> {
            let conn = db.conn()?;
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM recordings WHERE deleted_at IS NULL",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| medical_core::error::AppError::from(medical_db::DbError::from(e)))?;
            let latest: Option<String> = conn
                .query_row("SELECT MAX(updated_at) FROM recordings", [], |row| {
                    row.get(0)
                })
                .ok()
                .flatten();
            Ok((count, latest))
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("content_sync meta failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    debug!(count, "content_sync: meta");
    Ok(Json(ContentMetaResponse {
        server_time: Utc::now().to_rfc3339(),
        recording_count: count,
        latest_updated_at: latest,
    }))
}

/// GET /v1/content/events — Server-Sent Events stream for content changes.
///
/// Pushes `data: connected` on connect, then `data: changed` for each
/// broadcast on `content_changed_tx` (triggered by a push merge). Mirrors
/// the condition-chips events handler.
async fn content_events_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let mut rx = state.content_changed_tx.subscribe();
    let stream = async_stream::stream! {
        yield Ok(Event::default().data("connected"));
        loop {
            match rx.recv().await {
                Ok(_id) => yield Ok(Event::default().data("changed")),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream))
}

/// GET /v1/content/audio/{recording_id} — download decrypted audio bytes.
///
/// Loads the recording row, resolves its `audio_path`, and decrypts the
/// file (falling back to plaintext read for legacy unencrypted files). The
/// raw bytes are returned as the response body. Only the ID length and byte
/// count are logged — never the content.
async fn content_audio_get_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Path(recording_id): Path<String>,
) -> Result<Response, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let id_len = recording_id.len();
    let db = Arc::clone(&state.db);
    let data_dir = state.data_dir.clone();

    let bytes =
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, medical_core::error::AppError> {
            let conn = db.conn()?;
            let uuid = Uuid::parse_str(&recording_id)
                .map_err(|_| medical_core::error::AppError::Other("invalid recording id".into()))?;
            let rec = RecordingsRepo::get_by_id(&conn, &uuid)
                .map_err(medical_core::error::AppError::from)?;
            let path = &rec.audio_path;
            if path.as_os_str().is_empty() || !path.exists() {
                return Err(medical_core::error::AppError::Other(
                    "audio file not found".into(),
                ));
            }
            // Containment check: verify the audio path is within the
            // recordings directory. This prevents a malicious DB value
            // from causing the server to decrypt/read arbitrary files.
            let recordings_dir = crate::commands::resolve_recordings_dir(&db, &data_dir)?;
            let canonical_path = path.canonicalize().map_err(|e| {
                medical_core::error::AppError::Other(format!("path canonicalize failed: {e}"))
            })?;
            let canonical_dir = recordings_dir.canonicalize().map_err(|e| {
                medical_core::error::AppError::Other(format!(
                    "recordings dir canonicalize failed: {e}"
                ))
            })?;
            if !canonical_path.starts_with(&canonical_dir) {
                tracing::warn!(
                    id_len,
                    "content_audio: path outside recordings dir — rejected"
                );
                return Err(medical_core::error::AppError::Other(
                    "audio path not allowed".into(),
                ));
            }
            match file_crypto::decrypt_file(path) {
                Ok(plaintext) => Ok(plaintext),
                Err(file_crypto::FileCryptoError::NotEncrypted) => {
                    // Legacy plaintext file — read as-is.
                    std::fs::read(path).map_err(|e| {
                        medical_core::error::AppError::Other(format!("audio read failed: {e}"))
                    })
                }
                Err(e) => Err(medical_core::error::AppError::Security(format!(
                    "audio decrypt failed: {e}"
                ))),
            }
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!(id_len, error = %e, "content_audio: get failed");
            if matches!(
                e,
                medical_core::error::AppError::Other(ref s)
                    if s.contains("not found")
            ) {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    let byte_count = bytes.len();
    info!(id_len, byte_count, "content_audio: get");
    Ok(bytes.into_response())
}

/// PUT /v1/content/audio/{recording_id} — upload audio bytes (first-write-wins).
///
/// Receives the raw audio bytes in the request body, writes them to a temp
/// file under the recordings dir, encrypts in place (atomic rename), then
/// updates the recording's `audio_path`. If a file already exists for this
/// recording, returns 409 Conflict (first-write-wins). Returns 201 Created
/// on success. Only the ID length and byte count are logged.
async fn content_audio_put_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Path(recording_id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let id_len = recording_id.len();
    let byte_count = body.len();

    // Validate recording exists before writing anything to disk.
    let db = Arc::clone(&state.db);
    let uuid = Uuid::parse_str(&recording_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let exists = tokio::task::spawn_blocking({
        let db = Arc::clone(&db);
        move || -> Result<bool, StatusCode> {
            let conn = db.conn().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            match RecordingsRepo::get_by_id(&conn, &uuid) {
                Ok(_) => Ok(true),
                Err(medical_db::DbError::NotFound(_)) => Ok(false),
                Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let data_dir = state.data_dir.clone();
    let target_path = {
        // Resolve the recordings dir synchronously (cheap: reads settings).
        let recordings_dir =
            crate::commands::resolve_recordings_dir(&db, &data_dir).map_err(|e| {
                warn!(id_len, error = %e, "content_audio: resolve dir failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        recordings_dir.join(format!("{recording_id}.enc"))
    };

    // First-write-wins: if the target already exists, refuse.
    if target_path.exists() {
        info!(id_len, "content_audio: put rejected, file exists (409)");
        return Err(StatusCode::CONFLICT);
    }

    // Encrypt in memory first, then write ciphertext directly to a unique temp
    // file and atomically rename. This avoids ever writing plaintext PHI to
    // disk — critical for HIPAA at-rest guarantees (H9 fix).
    let db2 = Arc::clone(&db);
    let path_for_db = target_path.clone();
    let body_vec = body.to_vec();
    tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        // Encrypt the plaintext bytes in memory (no disk I/O).
        let ciphertext = file_crypto::encrypt_bytes_in_memory(&body_vec).map_err(|e| {
            medical_core::error::AppError::Security(format!("audio encrypt failed: {e}"))
        })?;

        // Write ciphertext to a unique temp file, then atomic rename.
        let tmp_path = target_path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp_path, &ciphertext)?;
        if let Err(e) = std::fs::rename(&tmp_path, &target_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(medical_core::error::AppError::Io(e));
        }

        // Update audio_path on the recording row.
        let conn = db2.conn()?;
        let mut rec =
            RecordingsRepo::get_by_id(&conn, &uuid).map_err(medical_core::error::AppError::from)?;
        rec.audio_path = path_for_db;
        rec.file_size_bytes = Some(body_vec.len() as u64);
        RecordingsRepo::update(&conn, &rec).map_err(medical_core::error::AppError::from)?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!(id_len, error = %e, "content_audio: put failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(id_len, byte_count, "content_audio: put (201)");
    Ok(StatusCode::CREATED)
}
