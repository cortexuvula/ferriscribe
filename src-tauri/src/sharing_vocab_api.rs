//! Server-canonical sync APIs for paired clients (vocabulary + context
//! templates). Lives on the office server alongside the sharing service.
//! Bearer-validated against the same `TokenStore` the auth proxy uses,
//! so any client whose pairing has been revoked is rejected here too.
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
//!
//! Wire formats reuse the existing `VocabularyEntry` and `ContextTemplate`
//! serde definitions, so clients deserialize directly into the same types
//! used for the local DB.
//!
//! No PHI passes through these routes — vocabulary is correction terms
//! and templates are visit-style boilerplate; neither carries patient
//! content — so logging request paths and status codes is safe.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Query, State as AxumState},
    http::{HeaderMap, StatusCode},
    routing::{get, put},
    Json,
};
use chrono::Utc;
use medical_core::types::settings::ContextTemplate;
use medical_core::types::vocabulary::{VocabularyCategory, VocabularyEntry};
use medical_db::{Database, settings::SettingsRepo, vocabulary::VocabularyRepo};
use medical_sharing::token_store::TokenStore;
use serde::Deserialize;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Clone)]
struct ApiState {
    db: Arc<Database>,
    tokens: Arc<TokenStore>,
}

/// Bind on `127.0.0.1:port` and 0.0.0.0:port? — we listen on 0.0.0.0 because
/// clients reach this from the LAN / Tailscale, just like the auth proxies.
pub async fn spawn(
    db: Arc<Database>,
    tokens: Arc<TokenStore>,
    port: u16,
) -> Result<JoinHandle<()>, String> {
    let state = ApiState { db, tokens };
    let app = Router::new()
        .route("/v1/vocabulary", get(list_handler).post(insert_handler).delete(delete_all_handler))
        .route("/v1/vocabulary/count", get(count_handler))
        .route("/v1/vocabulary/:id", put(update_handler).delete(delete_handler))
        .route("/v1/context-templates", get(templates_list_handler))
        .route("/v1/context-templates/upsert", axum::routing::post(templates_upsert_handler))
        .route("/v1/context-templates/rename", axum::routing::post(templates_rename_handler))
        .route("/v1/context-templates/delete", axum::routing::post(templates_delete_handler))
        .with_state(state);

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()
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
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
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
    let entries = tokio::task::spawn_blocking(move || -> Result<Vec<VocabularyEntry>, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        match q.category {
            Some(cat) => {
                let cat = VocabularyCategory::from_str(&cat);
                VocabularyRepo::list_by_category(&conn, &cat).map_err(|e| e.to_string())
            }
            None => VocabularyRepo::list_all(&conn).map_err(|e| e.to_string()),
        }
    })
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
    let counts = tokio::task::spawn_blocking(move || -> Result<(u32, u32), String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        VocabularyRepo::count(&conn).map_err(|e| e.to_string())
    })
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
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        VocabularyRepo::insert(&conn, &entry_clone).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("vocab_api insert failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    info!(find_len = entry.find_text.len(), "vocab_api: inserted entry");
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
    let existing = tokio::task::spawn_blocking(move || -> Result<VocabularyEntry, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        VocabularyRepo::get_by_id(&conn, &uuid).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::NOT_FOUND)?;

    let entry = VocabularyEntry {
        id: existing.id,
        find_text: body.find_text,
        replacement: body.replacement,
        category: VocabularyCategory::from_str(
            &body.category.unwrap_or_else(|| existing.category.as_str().to_string()),
        ),
        case_sensitive: body.case_sensitive.unwrap_or(existing.case_sensitive),
        priority: body.priority.unwrap_or(existing.priority),
        enabled: body.enabled.unwrap_or(existing.enabled),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };
    let entry_clone = entry.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = db2.conn().map_err(|e| e.to_string())?;
        VocabularyRepo::update(&conn, &entry_clone).map_err(|e| e.to_string())
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
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        VocabularyRepo::delete(&conn, &uuid).map_err(|e| e.to_string())
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
    let n = tokio::task::spawn_blocking(move || -> Result<u32, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        VocabularyRepo::delete_all(&conn).map_err(|e| e.to_string())
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

fn ctx_templates_load_sorted(db: &Database) -> Result<Vec<ContextTemplate>, String> {
    let conn = db.conn().map_err(|e| e.to_string())?;
    let mut cfg = SettingsRepo::load_config(&conn).map_err(|e| e.to_string())?;
    cfg.migrate();
    let mut t = cfg.custom_context_templates;
    t.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
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
    let entry = tokio::task::spawn_blocking(move || -> Result<ContextTemplate, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        let mut cfg = SettingsRepo::load_config(&conn).map_err(|e| e.to_string())?;
        cfg.migrate();
        let entry = ContextTemplate { name: name.clone(), body: body_text.clone() };
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
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        SettingsRepo::save_config(&conn, &cfg).map_err(|e| e.to_string())?;
        Ok(entry)
    })
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
    let entry = tokio::task::spawn_blocking(move || -> Result<ContextTemplate, (StatusCode, String)> {
        let conn = db
            .conn()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let mut cfg = SettingsRepo::load_config(&conn)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        cfg.migrate();
        if old_name == new_name {
            return cfg
                .custom_context_templates
                .iter()
                .find(|t| t.name == old_name)
                .cloned()
                .ok_or((StatusCode::NOT_FOUND, format!("'{old_name}' not found")));
        }
        if cfg.custom_context_templates.iter().any(|t| t.name == new_name) {
            return Err((StatusCode::CONFLICT, format!("'{new_name}' already exists")));
        }
        let idx = cfg
            .custom_context_templates
            .iter()
            .position(|t| t.name == old_name)
            .ok_or((StatusCode::NOT_FOUND, format!("'{old_name}' not found")))?;
        cfg.custom_context_templates[idx].name = new_name.clone();
        let renamed = cfg.custom_context_templates[idx].clone();
        cfg.custom_context_templates
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        SettingsRepo::save_config(&conn, &cfg)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(renamed)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|(code, msg)| {
        warn!("templates rename: {msg}");
        code
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
    tokio::task::spawn_blocking(move || -> Result<(), (StatusCode, String)> {
        let conn = db
            .conn()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let mut cfg = SettingsRepo::load_config(&conn)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        cfg.migrate();
        let idx = cfg
            .custom_context_templates
            .iter()
            .position(|t| t.name == name)
            .ok_or((StatusCode::NOT_FOUND, format!("'{name}' not found")))?;
        cfg.custom_context_templates.remove(idx);
        SettingsRepo::save_config(&conn, &cfg)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|(code, msg)| {
        warn!("templates delete: {msg}");
        code
    })?;
    Ok(StatusCode::NO_CONTENT)
}

