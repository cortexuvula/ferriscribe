//! Tauri commands for condition chips ("Known conditions" quick-add presets).
//!
//! Dispatch model: condition-chip operations check two gates before routing
//! through the office server's HTTP API:
//!
//! 1. The `sync_condition_chips` opt-in setting must be enabled.
//! 2. This client must be paired with an office server that advertised a
//!    `vocab` port (which also hosts `/v1/condition-chips`).
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

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tauri::Emitter;
use tracing::instrument;

use medical_core::error::{AppError, AppResult};
use medical_core::types::condition_chip::ConditionChip;

use crate::state::{self, AppState};

/// Returns `Some((conn, bearer))` when this client should route condition-chip
/// operations through the office server: the `sync_condition_chips` opt-in is
/// on AND the client is paired with a server that exposes the vocab/chips port.
///
/// Returns `None` otherwise, in which case commands fall back to the local
/// SQLite repo.
fn paired_conditions_target(
    state: &AppState,
) -> Option<(crate::commands::sharing::PairedConnection, String)> {
    // Gate 1: the user must opt in to condition-chip sync.
    let config = crate::commands::settings::load_config_sync(&state.db).ok()?;
    if !config.sync_condition_chips {
        return None;
    }
    // Gate 2: this client must be paired with a server that advertises the
    // vocab port (condition chips ride on the same port as the dictionary API).
    let conn = state::load_paired_connection()?;
    conn.ports.vocab?;
    let bearer = state::load_sharing_bearer()?;
    Some((conn, bearer))
}

/// ISO 8601 UTC timestamp with millisecond precision, matching the format used
/// by [`medical_db::condition_chips`] rows.
fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// List active condition chips.
///
/// When paired + sync enabled, pull from the server and merge into the local
/// store (last-write-wins) so both sides converge, then return the resulting
/// active list. A remote failure logs a warning and falls back to the local
/// active list so the UI keeps working offline.
#[tauri::command]
#[instrument(skip(state), name = "conditions::list")]
pub async fn list_condition_chips(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<ConditionChip>> {
    if let Some((conn, bearer)) = paired_conditions_target(&state)
        && let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        )
    {
        match remote.list().await {
            Ok(server_chips) => {
                let db = Arc::clone(&state.db);
                return tokio::task::spawn_blocking(move || {
                    let conn = db.conn()?;
                    medical_db::condition_chips::ConditionChipsRepo::merge_incoming(
                        &conn,
                        &server_chips,
                    )
                    .map_err(AppError::from)
                })
                .await
                .map_err(crate::commands::join_err)?;
            }
            Err(e) => {
                tracing::warn!(error = %e, "conditions remote list failed, using local");
                // Fall through to local fallback below.
            }
        }
    }
    // Local fallback (also reached when not paired / sync disabled, or when the
    // remote call failed).
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::list_active(&conn).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}

/// Add a condition chip.
///
/// Writes locally first (instant UI), then fires a non-blocking background
/// sync push of the resulting active list so the server converges. The push
/// is best-effort; a failure is retried on the next pull (list).
#[tauri::command]
#[instrument(skip(state, text), name = "conditions::add")]
pub async fn add_condition_chip(
    state: tauri::State<'_, AppState>,
    text: String,
) -> AppResult<Vec<ConditionChip>> {
    let now = now_iso();

    // 1. Update local DB immediately (instant UI feedback).
    let db = Arc::clone(&state.db);
    let local_list = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::add(&conn, &text, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)??;

    // 2. Best-effort background sync push (non-blocking). The owned
    //    `PairedConnection` is moved into the task and the `ConditionsRemote`
    //    borrows it from within the task's scope (it cannot borrow from this
    //    frame because `tokio::spawn` requires `'static`).
    if let Some((conn, bearer)) = paired_conditions_target(&state) {
        let http_client = state.http_client.clone();
        let chips_to_push = local_list.clone();
        tokio::spawn(async move {
            let remote = match crate::conditions_remote::ConditionsRemote::from(
                &conn,
                Some(bearer),
                http_client,
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("condition chip sync push target unavailable");
                    return;
                }
            };
            match remote.sync(chips_to_push).await {
                Ok(_) => tracing::debug!("condition chip sync push succeeded"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "condition chip sync push failed (will retry on next pull)"
                ),
            }
        });
    }

    Ok(local_list)
}

/// Remove (soft-delete) a condition chip by text.
///
/// Writes the tombstone locally first, then pushes the FULL list (including
/// tombstones) so the server sees the deletion. Using `list_all` (not
/// `list_active`) is essential — otherwise the tombstone would never reach the
/// server and the chip would ghost-resurface on other machines.
#[tauri::command]
#[instrument(skip(state, text), name = "conditions::remove")]
pub async fn remove_condition_chip(
    state: tauri::State<'_, AppState>,
    text: String,
) -> AppResult<Vec<ConditionChip>> {
    let now = now_iso();

    // 1. Soft-delete locally (returns the active list, sans the removed chip).
    let db = Arc::clone(&state.db);
    let local_list = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::remove_by_text(&conn, &text, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)??;

    // 2. Best-effort background sync — push ALL chips (including tombstones)
    //    so the server records the deletion. The owned `PairedConnection` is
    //    moved into the task and `ConditionsRemote` borrows it there.
    if let Some((conn, bearer)) = paired_conditions_target(&state) {
        let http_client = state.http_client.clone();
        let db2 = Arc::clone(&state.db);
        tokio::spawn(async move {
            // Load the full local list (incl. tombstones) on the blocking pool.
            let all_chips = match tokio::task::spawn_blocking(move || {
                let conn = db2.conn()?;
                medical_db::condition_chips::ConditionChipsRepo::list_all(&conn)
                    .map_err(AppError::from)
            })
            .await
            {
                Ok(Ok(chips)) => chips,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "failed to load chips for sync push (remove)");
                    return;
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "task join error loading chips for sync push (remove)"
                    );
                    return;
                }
            };
            let remote = match crate::conditions_remote::ConditionsRemote::from(
                &conn,
                Some(bearer),
                http_client,
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("condition chip sync push (remove) target unavailable");
                    return;
                }
            };
            match remote.sync(all_chips).await {
                Ok(_) => tracing::debug!("condition chip sync push (remove) succeeded"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "condition chip sync push (remove) failed (will retry on next pull)"
                ),
            }
        });
    }

    Ok(local_list)
}

/// Manually trigger a full bidirectional condition-chip sync.
///
/// Pushes the local full list (including tombstones) to the server, receives
/// the server's merged result, and merges that back into the local store.
/// Returns the active list afterwards.
///
/// Used when the user toggles `sync_condition_chips` on or reconnects after
/// being offline. When not paired / sync disabled, it simply returns the local
/// active list.
#[tauri::command]
#[instrument(skip(state), name = "conditions::sync")]
pub async fn sync_condition_chips_cmd(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<ConditionChip>> {
    // Load the local full list (including tombstones) to push.
    let db = Arc::clone(&state.db);
    let local_all = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::list_all(&conn).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)??;

    if let Some((conn, bearer)) = paired_conditions_target(&state)
        && let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        )
    {
        let merged = remote.sync(local_all).await?;
        let db = Arc::clone(&state.db);
        return tokio::task::spawn_blocking(move || {
            let conn = db.conn()?;
            medical_db::condition_chips::ConditionChipsRepo::merge_incoming(&conn, &merged)
                .map_err(AppError::from)
        })
        .await
        .map_err(crate::commands::join_err)?;
    }

    // Not paired / sync disabled — return the local active list.
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::list_active(&conn).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}

/// Increment the use count of a condition chip (frequency tracking).
///
/// Called when a user adds the condition to a note via the chip. Writes
/// locally first (instant UI + reorder), then fires a best-effort background
/// sync push so the count propagates. Cross-machine reconciliation to `MAX`
/// happens in `merge_incoming`, so a count bump here can never clobber a
/// larger count on another machine. The push is best-effort; a failure is
/// retried on the next pull (list).
#[tauri::command]
#[instrument(skip(state, text), name = "conditions::increment_use")]
pub async fn increment_condition_chip_use(
    state: tauri::State<'_, AppState>,
    text: String,
) -> AppResult<Vec<ConditionChip>> {
    let now = now_iso();

    // 1. Increment locally (returns the active list, now reordered by use_count).
    //    `increment_use` upserts-on-miss, so this also self-heals fresh installs
    //    whose default chips were never seeded.
    let db = Arc::clone(&state.db);
    let local_list = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::increment_use(&conn, &text, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)??;

    // 2. Best-effort background sync push (non-blocking). The increment only
    //    touches an active chip, so pushing the active list is sufficient.
    if let Some((conn, bearer)) = paired_conditions_target(&state) {
        let http_client = state.http_client.clone();
        let chips_to_push = local_list.clone();
        tokio::spawn(async move {
            let remote = match crate::conditions_remote::ConditionsRemote::from(
                &conn,
                Some(bearer),
                http_client,
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("condition chip sync push (increment_use) target unavailable");
                    return;
                }
            };
            match remote.sync(chips_to_push).await {
                Ok(_) => tracing::debug!("condition chip sync push (increment_use) succeeded"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "condition chip sync push (increment_use) failed (will retry on next pull)"
                ),
            }
        });
    }

    Ok(local_list)
}

/// Start a long-lived SSE subscription to the office server's condition-chip
/// change notifications.
///
/// Spawns a background task that connects to `/v1/condition-chips/events` and
/// emits a `condition-chips-changed` Tauri event for each server-pushed
/// "changed" notification. The frontend listens for this event and calls
/// `refreshChips()` for near-realtime sync across machines. The task runs for
/// the lifetime of the app and reconnects with exponential backoff (capped at
/// 30s) when the stream ends or errors.
///
/// This is a complement to, not a replacement for, the 30s poll — the poll
/// remains as a safety net in case SSE delivery fails silently.
///
/// Returns `Ok(())` immediately when not paired / sync disabled (no task is
/// spawned). This command is safe to call repeatedly; each call spawns an
/// independent task. In practice the frontend calls it once on mount.
#[tauri::command]
#[instrument(skip(app, state), name = "conditions::subscribe")]
pub async fn subscribe_condition_chips(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<()> {
    // Gate the same way the other condition commands do: only subscribe when
    // paired + sync enabled. When not paired, also cancel any existing
    // subscriber — the user may have just unpaired, and the old task must
    // not keep reconnecting with stale credentials.
    let Some((conn, bearer)) = paired_conditions_target(&state) else {
        return crate::commands::swap_sse_cancel_token(
            &state.condition_sse_cancel,
            "condition_sse_cancel",
            None,
        );
    };

    // Replace any previous subscriber: the frontend subscribes on every
    // mount of ConditionChips, so without this each mount leaks an eternal
    // reconnect loop holding its own SSE connection.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    crate::commands::swap_sse_cancel_token(
        &state.condition_sse_cancel,
        "condition_sse_cancel",
        Some(cancel_token.clone()),
    )?;

    let http_client = state.http_client.clone();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(5);
        loop {
            if cancel_token.is_cancelled() {
                break;
            }
            // `conn` and `bearer` are owned by this task; `ConditionsRemote`
            // borrows `conn` from within the task scope (cannot borrow from the
            // calling frame because `tokio::spawn` requires `'static`).
            let remote = match crate::conditions_remote::ConditionsRemote::from(
                &conn,
                Some(bearer.clone()),
                http_client.clone(),
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("condition chip SSE subscription target unavailable, retrying");
                    tokio::select! {
                        _ = cancel_token.cancelled() => break,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            match remote.subscribe_events().await {
                Ok(stream) => {
                    tracing::info!("condition chip SSE subscription connected");
                    backoff = Duration::from_secs(5);
                    // The stream from `filter_map` is `!Unpin`; pin it on the
                    // stack so `StreamExt::next` can borrow it mutably.
                    tokio::pin!(stream);
                    loop {
                        // Cancellation must interrupt a healthy stream too —
                        // an SSE connection stays open indefinitely, so a
                        // top-of-loop check alone never fires.
                        tokio::select! {
                            _ = cancel_token.cancelled() => break,
                            item = stream.next() => match item {
                                Some(()) => {
                                    let _ = app.emit("condition-chips-changed", ());
                                }
                                None => break,
                            },
                        }
                    }
                    tracing::info!("condition chip SSE stream ended, reconnecting");
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "condition chip SSE subscription failed, reconnecting"
                ),
            }
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
    Ok(())
}
