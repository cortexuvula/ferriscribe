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

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use medical_db::Database;
use medical_sharing::token_store::TokenStore;
use std::path::PathBuf;
use tauri::AppHandle;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub(super) mod audio;
pub(super) mod condition_chips;
pub(super) mod content_sync;
pub(super) mod context_templates;
pub(super) mod user_dictionary;
pub(super) mod vocabulary;

/// Internal Axum state shared across all vocab/template/dictionary route
/// handlers. Holds the database and token store needed to validate bearer
/// tokens and read/write data.
#[derive(Clone)]
pub(super) struct ApiState {
    pub(super) db: Arc<Database>,
    pub(super) tokens: Arc<TokenStore>,
    /// Broadcasts `()` whenever condition chips change on the server so SSE
    /// subscribers can push realtime notifications to their clients.
    pub(super) chips_changed_tx: tokio::sync::broadcast::Sender<()>,
    /// Broadcasts a recording ID (or `"*"` for all) whenever content-sync
    /// push merges new data, so SSE subscribers can refresh in near-realtime.
    pub(super) content_changed_tx: tokio::sync::broadcast::Sender<String>,
    /// App data dir; used to resolve the recordings directory for audio
    /// upload/download.
    pub(super) data_dir: PathBuf,
    /// Tauri app handle for emitting events to THIS machine's own frontend.
    /// When a remote client pushes recordings into the server's DB, the
    /// server's webview would otherwise never learn about the new rows
    /// (the SSE channel only notifies *other* clients). We emit a
    /// `recording-updated` Tauri event per changed ID so the server's own
    /// Recordings view reloads — mirroring what the client does on pull.
    pub(super) app_handle: AppHandle,
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
    let (chips_changed_tx, _) = tokio::sync::broadcast::channel::<()>(16);
    let (content_changed_tx, _) = tokio::sync::broadcast::channel::<String>(32);
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
            get(vocabulary::list_handler)
                .post(vocabulary::insert_handler)
                .delete(vocabulary::delete_all_handler),
        )
        .route("/v1/vocabulary/count", get(vocabulary::count_handler))
        .route(
            "/v1/vocabulary/{id}",
            put(vocabulary::update_handler).delete(vocabulary::delete_handler),
        )
        .route(
            "/v1/context-templates",
            get(context_templates::templates_list_handler),
        )
        .route(
            "/v1/context-templates/upsert",
            axum::routing::post(context_templates::templates_upsert_handler),
        )
        .route(
            "/v1/context-templates/rename",
            axum::routing::post(context_templates::templates_rename_handler),
        )
        .route(
            "/v1/context-templates/delete",
            axum::routing::post(context_templates::templates_delete_handler),
        )
        .route(
            "/v1/user-dictionary",
            get(user_dictionary::dict_list_handler).post(user_dictionary::dict_add_handler),
        )
        .route(
            "/v1/user-dictionary/{word}",
            axum::routing::delete(user_dictionary::dict_remove_handler),
        )
        .route(
            "/v1/condition-chips",
            get(condition_chips::condition_chips_list_handler),
        )
        .route(
            "/v1/condition-chips/sync",
            post(condition_chips::condition_chips_sync_handler),
        )
        .route(
            "/v1/condition-chips/events",
            get(condition_chips::condition_chips_events_handler),
        )
        .route(
            "/v1/content/sync",
            get(content_sync::content_sync_pull_handler)
                .post(content_sync::content_sync_push_handler),
        )
        .route(
            "/v1/content/sync/meta",
            get(content_sync::content_sync_meta_handler),
        )
        .route(
            "/v1/content/events",
            get(content_sync::content_events_handler),
        )
        .route(
            "/v1/content/audio/{recording_id}",
            get(audio::content_audio_get_handler).put(audio::content_audio_put_handler),
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

pub(super) fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    v.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

pub(super) fn authorize(state: &ApiState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    let token = extract_bearer(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let row = state
        .tokens
        .validate(&token)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let _ = state.tokens.touch(row.id);
    Ok(row.id)
}
