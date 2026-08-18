//! Content-sync handlers for the `/v1/content/*` routes.
//!
//! Bidirectional content sync for recordings (transcript, SOAP, referral,
//! letter, etc.) operating at per-field granularity via last-write-wins.
//! Audio files are synced through dedicated GET/PUT endpoints (see
//! [`audio`]). All DB work runs inside `spawn_blocking`. No PHI
//! (transcript/SOAP/recording content) is ever logged — only counts, IDs,
//! and lengths.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::Query;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use chrono::Utc;
use futures_util::Stream;
use medical_core::types::recording::Recording;
use medical_db::content_sync::{
    ContentSyncRepo, FieldRevision, MergeConflict, PurgedRef, SyncFieldValue, SyncRecording,
};
use medical_db::recordings::RecordingsRepo;
use serde::Deserialize;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{ApiState, authorize};

// ── Content-sync DTOs ───────────────────────────────────────────────────
//
// Wire-format request/response types for the content-sync routes
// (/v1/content/*). These mirror the types in `medical_db::content_sync`
// but are kept here as thin serde wrappers so the handler signatures read
// clearly. No PHI appears in these types beyond what the client already
// sent us; logs emit counts and lengths only.

#[derive(Deserialize)]
pub(super) struct SyncSinceQuery {
    since: Option<String>,
    limit: Option<usize>,
}

#[derive(serde::Serialize)]
pub(super) struct ContentPullResponse {
    recordings: Vec<SyncRecording>,
    server_time: String,
    has_more: bool,
    /// Purge notifications: `purged_recordings` ledger entries newer than
    /// the client's `since` cursor (all entries when it sent none — a
    /// fresh client's first pull). Clients tombstone any local live copy
    /// so machines that missed a deletion converge. Wire-compatible both
    /// ways: `skip_serializing_if` keeps the payload byte-identical to the
    /// pre-purge format when empty, and older clients ignore the unknown
    /// field (no `deny_unknown_fields` anywhere on this path).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    purged: Vec<PurgedRef>,
}

#[derive(Deserialize)]
pub(super) struct ContentPushRequest {
    recordings: Vec<SyncRecording>,
}

#[derive(serde::Serialize)]
pub(super) struct ContentPushResponse {
    conflicts: Vec<MergeConflict>,
    server_time: String,
}

#[derive(serde::Serialize)]
pub(super) struct ContentMetaResponse {
    server_time: String,
    recording_count: i64,
    latest_updated_at: Option<String>,
}

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

    // Field wire timestamp = max(revision, row write) — the same rider as
    // the client-side builder in commands/content_sync.rs. Local writers
    // that bump only the row (transcription/generation completion) leave
    // stale revisions from a pre-edit sync round-trip; serving the stale
    // revision timestamp makes clients' merges tie and silently drop the
    // newer value. Parsed comparison (mixed timestamp formats compare
    // wrongly as strings). Row-derived stamps carry no origin device.
    let field_ts = |rev: Option<&FieldRevision>| -> (String, Option<String>) {
        match rev {
            Some(r) => {
                if medical_db::content_sync::cmp_lww_timestamps(&r.updated_at, &row_ts)
                    == std::cmp::Ordering::Less
                {
                    (row_ts.clone(), None)
                } else {
                    (r.updated_at.clone(), r.origin_device.clone())
                }
            }
            None => (row_ts.clone(), None),
        }
    };

    let mut fields: HashMap<String, SyncFieldValue> = HashMap::new();

    // Text columns: value is a JSON string when present.
    let mut push_text = |name: &str, val: Option<&str>| {
        if let Some(s) = val {
            let (ts, device) = field_ts(rev_map.get(name).copied());
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
            let (ts, device) = field_ts(rev_map.get(name).copied());
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
pub(super) async fn content_sync_pull_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Query(q): Query<SyncSinceQuery>,
) -> Result<Json<ContentPullResponse>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let limit = q.limit.unwrap_or(200).clamp(1, 500) as u32;
    let since = q.since;
    let db = Arc::clone(&state.db);

    let pulled = tokio::task::spawn_blocking(
        move || -> Result<(Vec<SyncRecording>, bool, Vec<PurgedRef>), medical_core::error::AppError>
        {
            let conn = db.conn()?;
            let (ids, has_more) = ContentSyncRepo::changed_since(&conn, since.as_deref(), limit)
                .map_err(medical_core::error::AppError::from)?;
            let recs = load_sync_recordings(&conn, &ids)?;
            // Purge notifications ride on the same response: ledger entries
            // newer than the client's cursor (all of them for a fresh
            // client). Best-effort by design — a ledger read failure must
            // not fail the pull; the client re-requests the same window on
            // its next sync cycle. The failure is still logged (error
            // only, no PHI) so a persistently broken ledger is diagnosable
            // instead of silently degrading convergence.
            let purged = match ContentSyncRepo::purged_since(&conn, since.as_deref()) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "content_sync pull: purge-ledger read failed; continuing without purge notifications");
                    Vec::new()
                }
            };
            Ok((recs, has_more, purged))
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("content_sync pull failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (recordings, has_more, purged) = pulled;

    info!(
        count = recordings.len(),
        has_more,
        purged_count = purged.len(),
        "content_sync: pull"
    );
    Ok(Json(ContentPullResponse {
        recordings,
        server_time: Utc::now().to_rfc3339(),
        has_more,
        purged,
    }))
}

/// POST /v1/content/sync — push (two-way merge).
///
/// Body: `ContentPushRequest` with the client's changed recordings. The
/// server merges each field via last-write-wins and returns the fields
/// where the server's local copy won (conflicts). After a successful merge
/// the server broadcasts on `content_changed_tx` so other SSE-connected
/// clients refresh.
pub(super) async fn content_sync_push_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(req): Json<ContentPushRequest>,
) -> Result<Json<ContentPushResponse>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let incoming_count = req.recordings.len();

    // Acquire the merge lock to serialize concurrent pushes. Without this,
    // two clients pushing simultaneously could interleave their
    // read-modify-write cycles on field revisions, losing data. The lock is
    // held across the spawn_blocking merge call and released before SSE
    // events are sent so a slow broadcast never blocks other merges.
    let merge_guard = state.merge_lock.lock().await;

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

    // Release the merge lock before fanning out notifications so a slow
    // broadcast/SSE never blocks the next concurrent push merge.
    drop(merge_guard);

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
pub(super) async fn content_sync_meta_handler(
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
pub(super) async fn content_events_handler(
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
    // Keep-alive comments (`:\n\n` every 15s by default) prevent NAT / relay
    // idle timeouts from silently dropping the long-lived SSE stream.
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// Re-export tauri::Emitter so the inline `state.app_handle.emit(...)` call
// above compiles without a top-level `use` cluttering the module's public
// imports. Kept private to this module.
use tauri::Emitter as _;

#[cfg(test)]
mod sparse_fields_tests {
    // Pins the max(revision, row) stamp on the SERVER-side builder — the
    // pull responses clients consume. A stale revision must not mask a
    // newer row-level write or clients' Equal-tie merges silently drop the
    // newer value (final whole-feature review, HIGH-1).
    use super::*;
    use medical_db::Database;

    #[test]
    fn server_builder_row_timestamp_wins_over_stale_revision() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let mut rec = Recording::new("srv-rider.wav", std::path::PathBuf::from("/audio/srv.wav"));
        rec.soap_note = Some("regenerated soap".to_string());
        let row_time = chrono::Utc::now();
        rec.updated_at = Some(row_time);
        RecordingsRepo::insert(&conn, &rec).expect("insert");
        ContentSyncRepo::upsert_revision(&conn, &rec.id, "soap_note", "2020-01-01T00:00:00Z", None)
            .expect("stale revision");

        let revisions = ContentSyncRepo::revisions_for(&conn, &rec.id).expect("revisions");
        let fields = build_sparse_fields(&rec, Some(&revisions));
        let soap = &fields["soap_note"];
        assert_ne!(soap.updated_at, "2020-01-01T00:00:00Z");
        assert_eq!(soap.updated_at, row_time.to_rfc3339());
    }
}
