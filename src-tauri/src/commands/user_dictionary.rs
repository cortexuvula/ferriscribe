//! Tauri commands for the per-user spellcheck dictionary.
//!
//! Dispatch model mirrors `commands::conditions`: dictionary operations check
//! two gates before routing through the office server's HTTP API:
//!
//! 1. The `sync_user_dictionary` opt-in setting must be enabled.
//! 2. This client must be paired with an office server that advertised a
//!    `vocab` port (which also hosts `/v1/user-dictionary`).
//!
//! When both hold, list pulls from the server and merges locally, and
//! add/remove push the local state to the server in the background. When
//! either gate fails, operations run against the local SQLite repo only —
//! the feature is fully usable offline or unpaired.
//!
//! Writes (add/remove) always update the local DB first for instant UI
//! feedback, then fire a best-effort background sync push that does not
//! block the command's return. A failed push is retried implicitly on the
//! next `list` (which always pulls + merges when paired).
//!
//! No word values are emitted to logs — the dictionary may contain
//! patient-context-specific terms (PHI). Only counts and lengths are logged.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tauri::Emitter;
use tracing::instrument;

use medical_core::error::{AppError, AppResult};

use crate::state::{self, AppState};

/// Returns `Some((conn, bearer))` when this client should route user-dictionary
/// operations through the office server: the `sync_user_dictionary` opt-in is
/// on AND the client is paired with a server that exposes the vocab/dict port.
///
/// Returns `None` otherwise, in which case commands fall back to the local
/// SQLite repo.
fn paired_dict_target(
    state: &AppState,
) -> Option<(crate::commands::sharing::PairedConnection, String)> {
    // Gate 1: the user must opt in to dictionary sync.
    let config = crate::commands::settings::load_config_sync(&state.db).ok()?;
    if !config.sync_user_dictionary {
        return None;
    }
    // Gate 2: this client must be paired with a server that advertises the
    // vocab port (the dictionary API rides on the same port as the vocab API).
    let conn = state::load_paired_connection()?;
    conn.ports.vocab?;
    let bearer = state::load_sharing_bearer()?;
    Some((conn, bearer))
}

/// ISO 8601 UTC timestamp with millisecond precision, matching the format used
/// by dictionary rows.
fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// List active dictionary words.
///
/// When paired + sync enabled, push the local full list (active + tombstones)
/// to the server and return the server's post-merge active word list. The
/// push converges both sides via the server's last-write-wins merge, so the
/// returned list reflects the union across machines. A remote failure logs a
/// warning and falls back to the local active list so the UI keeps working
/// offline.
#[tauri::command]
#[instrument(skip(state), name = "user_dict::list")]
pub async fn user_dict_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<String>> {
    if let Some((conn, bearer)) = paired_dict_target(&state)
        && let Some(remote) = crate::user_dict_remote::UserDictRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        )
    {
        match remote.sync(load_all_local(&state.db).await?).await {
            Ok(server_words) => return Ok(server_words),
            Err(e) => {
                tracing::warn!(error = %e, "dict remote sync failed, using local");
                // Fall through to local fallback below.
            }
        }
    }
    // Local fallback (also reached when not paired / sync disabled, or when the
    // remote call failed).
    load_active_local(&state.db).await
}

/// Add a word to the dictionary.
///
/// Writes locally first (instant UI), then fires a non-blocking background
/// sync push of the resulting full list (active + tombstones) so the server
/// converges. The push is best-effort; a failure is retried on the next pull
/// (list). Returns `true` if a new row was inserted or a tombstone resurrected.
#[tauri::command]
#[instrument(skip(state, word), name = "user_dict::add")]
pub async fn user_dict_add(state: tauri::State<'_, AppState>, word: String) -> AppResult<bool> {
    let now = now_iso();
    let word_len = word.len();

    // 1. Update local DB immediately (instant UI feedback).
    let db = Arc::clone(&state.db);
    let added = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)??;

    // 2. Best-effort background sync push (non-blocking). The owned
    //    `PairedConnection` is moved into the task and `UserDictRemote`
    //    borrows it from within the task's scope (it cannot borrow from this
    //    frame because `tokio::spawn` requires `'static`).
    if let Some((conn, bearer)) = paired_dict_target(&state) {
        let http_client = state.http_client.clone();
        let db2 = Arc::clone(&state.db);
        tokio::spawn(async move {
            let all_entries = match load_all_local_blocking(db2).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to load dict for sync push (add)"
                    );
                    return;
                }
            };
            let remote = match crate::user_dict_remote::UserDictRemote::from(
                &conn,
                Some(bearer),
                http_client,
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("dict sync push (add) target unavailable");
                    return;
                }
            };
            match remote.sync(all_entries).await {
                Ok(_) => tracing::debug!("dict sync push (add) succeeded"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "dict sync push (add) failed (will retry on next pull)"
                ),
            }
        });
    }

    let _ = word_len; // available for debug logging if ever needed.
    Ok(added)
}

/// Remove (soft-delete) a word.
///
/// Writes the tombstone locally first, then pushes the FULL list (including
/// tombstones) so the server sees the deletion. Using the full list (not the
/// active list) is essential — otherwise the tombstone would never reach the
/// server and the word would ghost-resurface on other machines.
#[tauri::command]
#[instrument(skip(state, word), name = "user_dict::remove")]
pub async fn user_dict_remove(state: tauri::State<'_, AppState>, word: String) -> AppResult<bool> {
    let now = now_iso();

    // 1. Soft-delete locally.
    let db = Arc::clone(&state.db);
    let removed = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)??;

    // 2. Best-effort background sync — push ALL entries (including tombstones)
    //    so the server records the deletion. The owned `PairedConnection` is
    //    moved into the task and `UserDictRemote` borrows it there.
    if let Some((conn, bearer)) = paired_dict_target(&state) {
        let http_client = state.http_client.clone();
        let db2 = Arc::clone(&state.db);
        tokio::spawn(async move {
            let all_entries = match load_all_local_blocking(db2).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to load dict for sync push (remove)"
                    );
                    return;
                }
            };
            let remote = match crate::user_dict_remote::UserDictRemote::from(
                &conn,
                Some(bearer),
                http_client,
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("dict sync push (remove) target unavailable");
                    return;
                }
            };
            match remote.sync(all_entries).await {
                Ok(_) => tracing::debug!("dict sync push (remove) succeeded"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "dict sync push (remove) failed (will retry on next pull)"
                ),
            }
        });
    }

    Ok(removed)
}

/// Manually trigger a full bidirectional user-dictionary sync.
///
/// Pushes the local full list (including tombstones) to the server, receives
/// the server's merged active word list, and returns it. Used when the user
/// toggles `sync_user_dictionary` on or reconnects after being offline. When
/// not paired / sync disabled, it simply returns the local active list.
#[tauri::command]
#[instrument(skip(state), name = "user_dict::sync")]
pub async fn sync_user_dictionary_cmd(state: tauri::State<'_, AppState>) -> AppResult<Vec<String>> {
    let local_all = load_all_local(&state.db).await?;

    if let Some((conn, bearer)) = paired_dict_target(&state)
        && let Some(remote) = crate::user_dict_remote::UserDictRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        )
    {
        let merged_words = remote.sync(local_all).await?;
        // The server's response is its post-merge active word list. The push
        // already converged the server with us; return that list.
        return Ok(merged_words);
    }

    // Not paired / sync disabled — return the local active list.
    load_active_local(&state.db).await
}

/// Start a long-lived SSE subscription to the office server's user-dictionary
/// change notifications.
///
/// Spawns a background task that connects to `/v1/user-dictionary/events` and
/// emits a `user-dictionary-changed` Tauri event for each server-pushed
/// "changed" notification. The frontend listens for this event and reloads
/// the user dictionary for near-realtime sync across machines. The task runs
/// for the lifetime of the app and reconnects with exponential backoff
/// (capped at 30s) when the stream ends or errors.
///
/// Returns `Ok(())` immediately when not paired / sync disabled (no task is
/// spawned). This command is safe to call repeatedly; each call spawns an
/// independent task. In practice the frontend calls it once on mount.
#[tauri::command]
#[instrument(skip(app, state), name = "user_dict::subscribe")]
pub async fn subscribe_user_dictionary(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<()> {
    // Gate the same way the other dict commands do: only subscribe when
    // paired + sync enabled. When not paired, return quietly.
    let Some((conn, bearer)) = paired_dict_target(&state) else {
        return Ok(());
    };
    let http_client = state.http_client.clone();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(5);
        loop {
            // `conn` and `bearer` are owned by this task; `UserDictRemote`
            // borrows `conn` from within the task scope (cannot borrow from the
            // calling frame because `tokio::spawn` requires `'static`).
            let remote = match crate::user_dict_remote::UserDictRemote::from(
                &conn,
                Some(bearer.clone()),
                http_client.clone(),
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("dict SSE subscription target unavailable, retrying");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            match remote.subscribe_events().await {
                Ok(stream) => {
                    tracing::info!("dict SSE subscription connected");
                    backoff = Duration::from_secs(5);
                    // The stream from `filter_map` is `!Unpin`; pin it on the
                    // stack so `StreamExt::next` can borrow it mutably.
                    tokio::pin!(stream);
                    while let Some(()) = stream.next().await {
                        let _ = app.emit("user-dictionary-changed", ());
                    }
                    tracing::info!("dict SSE stream ended, reconnecting");
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "dict SSE subscription failed, reconnecting"
                ),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
    Ok(())
}

// --- small async helpers that run on the blocking pool ---

/// Load the active word list from the local DB.
async fn load_active_local(db: &Arc<medical_db::Database>) -> AppResult<Vec<String>> {
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::user_dictionary::UserDictionaryRepo::list_active(&conn).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}

/// Load the full entry list (active + tombstones) from the local DB.
async fn load_all_local(
    db: &Arc<medical_db::Database>,
) -> AppResult<Vec<medical_core::types::user_dict_entry::UserDictEntry>> {
    load_all_local_blocking(Arc::clone(db)).await
}

/// Blocking-pool variant of [`load_all_local`] for reuse inside `tokio::spawn`
/// tasks (where we already own a fresh `Arc<Database>` and want to avoid the
/// extra async indirection on the calling frame).
async fn load_all_local_blocking(
    db: Arc<medical_db::Database>,
) -> AppResult<Vec<medical_core::types::user_dict_entry::UserDictEntry>> {
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::user_dictionary::UserDictionaryRepo::list_all(&conn).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}
