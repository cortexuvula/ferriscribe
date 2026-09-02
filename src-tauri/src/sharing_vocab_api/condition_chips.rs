//! Condition chips handlers for the `/v1/condition-chips` routes.
//!
//! Practice-wide quick-add condition presets stored in the dedicated
//! `condition_chips` table (not the settings blob). Reads/writes hit
//! `medical_db::condition_chips::ConditionChipsRepo` directly — the same
//! pattern as the vocabulary handlers above. Deletion is soft (tombstoned),
//! so a two-way merge can propagate add/remove across machines. List and
//! sync responses intentionally include tombstones (the FULL list) so a
//! deletion on one machine reaches every client — a stale client pushing
//! back an active copy cannot resurrect it (ties break toward the
//! tombstone in `merge_incoming`). Tombstones are pruned after 365 days,
//! long enough for every machine to have seen them. No PHI in logs; only
//! counts and lengths are logged.

use std::sync::Arc;

use axum::Json;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use super::{ApiState, authorize};

/// GET /v1/condition-chips — return ALL condition chips, including
/// tombstones. Tombstones travel intentionally: a client that never learned
/// of a deletion must receive the tombstone so its local copy converges to
/// deleted (a response of active-only chips would leave stale clients free
/// to resurrect deleted chips practice-wide). Clients filter locally via
/// `merge_incoming`, whose return value is the active list.
pub(super) async fn condition_chips_list_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
) -> Result<Json<Vec<medical_core::types::condition_chip::ConditionChip>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let chips = tokio::task::spawn_blocking(
        move || -> Result<Vec<medical_core::types::condition_chip::ConditionChip>, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::condition_chips::ConditionChipsRepo::list_all(&conn)
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
/// Returns: the merged FULL chip list (active chips + tombstones) after
/// applying last-write-wins. Tombstones travel intentionally so deletions
/// propagate to every client; each client merges the response locally via
/// `merge_incoming`, which applies the tombstones and returns the active
/// list for its UI.
pub(super) async fn condition_chips_sync_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Json(incoming): Json<Vec<medical_core::types::condition_chip::ConditionChip>>,
) -> Result<Json<Vec<medical_core::types::condition_chip::ConditionChip>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);

    let incoming_count = incoming.len();

    // Serialize concurrent merges (same discipline as the content-push
    // handler and the dictionary syncs).
    let merge_guard = state.merge_lock.lock().await;

    // Prune old tombstones opportunistically. Retention is 365 days: a
    // tombstone must outlive every stale client's next sync, or a machine
    // that syncs rarely (or was offline for weeks) would never learn of the
    // deletion and could resurrect the chip practice-wide. The condition
    // chips table is tiny, so a year of tombstones costs nothing. Best
    // effort — a prune failure must not fail the sync.
    let cutoff = chrono::Utc::now() - chrono::Duration::days(365);
    let cutoff_iso = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let merged = tokio::task::spawn_blocking(
        move || -> Result<Vec<medical_core::types::condition_chip::ConditionChip>, medical_core::error::AppError> {
            let conn = db.conn()?;
            // Merge the client's list in (LWW; ties break toward the
            // tombstone). The merge's return value is the ACTIVE list —
            // discard it and serve the FULL list so tombstones travel.
            medical_db::condition_chips::ConditionChipsRepo::merge_incoming(&conn, &incoming)
                .map_err(medical_core::error::AppError::from)?;
            // Best-effort prune — don't fail the sync if pruning errors.
            let _ = medical_db::condition_chips::ConditionChipsRepo::prune_tombstones(&conn, &cutoff_iso);
            medical_db::condition_chips::ConditionChipsRepo::list_all(&conn)
                .map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("condition_chips_api sync failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Release the merge lock before the SSE fan-out.
    drop(merge_guard);

    // Notify SSE subscribers that chips changed. Best-effort: no receivers is
    // not an error (send returns Err only when there are no active receivers,
    // which is the normal idle case).
    let _ = state.chips_changed_tx.send(());

    // Also notify THIS server's own webview so its chip tray refreshes when a
    // remote client pushed a change (add/remove/increment). The SSE broadcast
    // above only reaches *other* client machines; without this emit the server
    // UI would stay stale until restart — the bug behind chip reorder/use_count
    // not reflecting on the Mac when Windows pushed changes. The frontend's
    // ConditionChips.svelte already listens for `condition-chips-changed`.
    // Mirrors content_sync's `recording-updated` self-emit pattern.
    let _ = state.app_handle.emit("condition-chips-changed", ());

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
pub(super) async fn condition_chips_events_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
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
    // Keep-alive comments (`:\n\n` every 15s by default) prevent NAT / relay
    // idle timeouts from silently dropping the long-lived SSE stream — the
    // real risk the client's old 300s timeout was fumbling toward.
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// Re-export tauri::Emitter so the `state.app_handle.emit(...)` call in the
// sync handler compiles without a top-level `use` cluttering the module's
// public imports. Mirrors content_sync.rs. Kept private to this module.
use tauri::Emitter as _;

// Note on test coverage: the self-emit in condition_chips_sync_handler is not
// unit-tested. ApiState.app_handle is `tauri::AppHandle` (the default `Wry`
// runtime alias), and tauri::test::mock_app() yields `AppHandle<MockRuntime>` —
// incompatible types, so the handler can't be driven without a real running
// app. This mirrors the codebase's established precedent (content_sync's
// identical `recording-updated` self-emit is also untested; see
// transcription/inner.rs:766 for the documented "can't build AppHandle in
// unit tests" pattern). The regression is guarded manually: with Mac (server)
// + Windows (client), a chip add/increment on Windows should refresh the
// Mac's chip tray without restart.
