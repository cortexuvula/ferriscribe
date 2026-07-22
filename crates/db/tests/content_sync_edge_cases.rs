//! Edge-case tests for `ContentSyncRepo` — covering the specific bugs fixed
//! in v0.30.28 and related scenarios:
//!
//! - Separate push cursor (round-trip + v3 reset sentinel)
//! - `apply_field` validates `processing_status` before writing (H1)
//! - `insert_remote_recording` uses remote's real status (H1)
//! - `stamp_synced_origin` merges into non-object metadata without data loss (M6)
//! - New recording insert stamps `synced_from` metadata for cloud badge
//!
//! **HIPAA note:** All sample text is synthetic non-PHI filler. Assertions
//! check field names, timestamps, and JSON structure — never patient content.

use std::collections::HashMap;
use std::path::PathBuf;

use medical_core::types::recording::Recording;
use medical_db::Database;
use medical_db::content_sync::{ContentSyncRepo, MergeResult, SyncFieldValue, SyncRecording};
use medical_db::recordings::RecordingsRepo;
use uuid::Uuid;

/// ISO 8601 timestamp offset from a fixed base epoch (seconds).
fn now(offset_secs: i64) -> String {
    let base = chrono::DateTime::parse_from_rfc3339("2026-07-15T10:00:00Z").unwrap();
    let t = base + chrono::Duration::seconds(offset_secs);
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Insert a bare recording row so it exists locally, returning the id.
fn seed_recording(conn: &rusqlite::Connection) -> Uuid {
    let mut rec = Recording::new("visit.wav", PathBuf::from("/audio/visit.wav"));
    rec.id = Uuid::new_v4();
    RecordingsRepo::insert(conn, &rec).expect("insert seed recording");
    rec.id
}

/// Build a `(field, SyncFieldValue)` pair for insertion into a fields map.
fn field_value(field: &str, value: serde_json::Value, offset: i64) -> (String, SyncFieldValue) {
    (
        field.to_string(),
        SyncFieldValue {
            value,
            updated_at: now(offset),
            origin_device: Some("remote-machine".to_string()),
        },
    )
}

/// Convenience: build a single-field map.
fn single_field(field: &str, value: serde_json::Value, offset: i64) -> HashMap<String, SyncFieldValue> {
    let mut m = HashMap::new();
    let (k, v) = field_value(field, value, offset);
    m.insert(k, v);
    m
}

/// Build a `SyncRecording` referring to a recording id.
fn remote_for(id: Uuid, fields: HashMap<String, SyncFieldValue>) -> SyncRecording {
    SyncRecording {
        id: id.to_string(),
        filename: "visit.wav".to_string(),
        created_at: now(0),
        updated_at: now(0),
        deleted_at: None,
        patient_name: None,
        duration_seconds: None,
        file_size_bytes: None,
        stt_provider: None,
        ai_provider: None,
        fields,
    }
}

// -------------------------------------------------------------------------
// Push cursor: round-trip through sync_state.
// -------------------------------------------------------------------------

#[test]
fn push_cursor_round_trips() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    // First call triggers the v3 reset (returns None), so call it once
    // to consume the one-time sentinel before testing round-trip.
    let first = ContentSyncRepo::get_push_cursor(&conn).expect("first get");
    assert!(first.is_none(), "first get should be None after v3 reset");

    ContentSyncRepo::set_push_cursor(&conn, "2026-07-15T12:00:00Z").expect("set");

    let after = ContentSyncRepo::get_push_cursor(&conn).expect("get after set");
    assert_eq!(after.as_deref(), Some("2026-07-15T12:00:00Z"));
}

// -------------------------------------------------------------------------
// Push cursor: v3 reset sentinel fires only once.
// -------------------------------------------------------------------------

#[test]
fn push_cursor_v3_reset_is_one_time() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    // First call: sentinel absent → resets push cursor → returns None.
    let first = ContentSyncRepo::get_push_cursor(&conn).expect("first get");
    assert!(first.is_none(), "first call should trigger reset");

    // Set a cursor.
    ContentSyncRepo::set_push_cursor(&conn, "2026-07-15T12:00:00Z").expect("set");

    // Second call: sentinel present → should return the persisted cursor,
    // not trigger another reset.
    let second = ContentSyncRepo::get_push_cursor(&conn).expect("second get");
    assert_eq!(
        second.as_deref(),
        Some("2026-07-15T12:00:00Z"),
        "second call must not re-reset"
    );
}

// -------------------------------------------------------------------------
// H1: insert_remote_recording uses remote's actual processing_status.
//
// Before the fix, the insert hardcoded {"status":"pending"}, silently
// downgrading a Completed recording. After the fix, the remote's validated
// status is used.
// -------------------------------------------------------------------------

#[test]
fn insert_uses_remote_processing_status() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let remote_id = Uuid::new_v4();

    // Remote sends a completed status.
    let fields = single_field(
        "processing_status",
        serde_json::json!({"status":"completed","completed_at":"2026-07-15T10:05:00Z"}),
        1,
    );
    let remote = remote_for(remote_id, fields);

    ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    // Read the stored processing_status column.
    let stored: String = conn
        .query_row(
            "SELECT processing_status FROM recordings WHERE id = ?1",
            [remote_id.to_string()],
            |row| row.get(0),
        )
        .expect("query status");

    let parsed: serde_json::Value = serde_json::from_str(&stored).expect("valid json");
    assert_eq!(
        parsed["status"].as_str(),
        Some("completed"),
        "remote completed status must be preserved, not downgraded to pending"
    );
}

// -------------------------------------------------------------------------
// H1: apply_field skips invalid processing_status values.
//
// A malformed wire value would be stored verbatim and silently deserialize
// to Pending. After the fix, apply_field validates and skips bad values.
// -------------------------------------------------------------------------

#[test]
fn apply_field_skips_invalid_processing_status() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);

    // Set a known-good local status first.
    conn.execute(
        "UPDATE recordings SET processing_status = ?1 WHERE id = ?2",
        rusqlite::params![
            serde_json::json!({"status":"completed"}).to_string(),
            id.to_string()
        ],
    )
    .expect("set local status");

    // Remote sends an invalid processing_status (garbage value).
    let fields = single_field(
        "processing_status",
        serde_json::json!({"status":"totally-bogus-status"}),
        99, // newer timestamp so remote would "win" without the guard
    );
    let remote = remote_for(id, fields);

    ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    // The invalid value must NOT have been written — local completed status preserved.
    let stored: String = conn
        .query_row(
            "SELECT processing_status FROM recordings WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("query status");

    let parsed: serde_json::Value = serde_json::from_str(&stored).expect("valid json");
    assert_eq!(
        parsed["status"].as_str(),
        Some("completed"),
        "invalid processing_status must be rejected, preserving local value"
    );
}

// -------------------------------------------------------------------------
// M6 / cloud badge: new recording insert stamps synced_from in metadata.
// -------------------------------------------------------------------------

#[test]
fn insert_stamps_synced_from_metadata() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let remote_id = Uuid::new_v4();
    let fields = single_field("transcript", serde_json::json!("some text content"), 1);
    let remote = remote_for(remote_id, fields);

    ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    let metadata: String = conn
        .query_row(
            "SELECT metadata FROM recordings WHERE id = ?1",
            [remote_id.to_string()],
            |row| row.get(0),
        )
        .expect("query metadata");

    let meta: serde_json::Value = serde_json::from_str(&metadata).expect("valid json");
    assert!(
        meta.get("synced_from").is_some(),
        "metadata must contain synced_from marker for cloud badge: {metadata}"
    );
}

// -------------------------------------------------------------------------
// stamp_synced_origin: non-object metadata is wrapped, not discarded.
//
// If a peer sends a metadata field that is a bare string or number (not a
// JSON object), stamp_synced_origin wraps it as {"original": <value>} and
// then adds synced_from. The original value must survive.
// -------------------------------------------------------------------------

#[test]
fn stamp_synced_origin_wraps_non_object_metadata() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let remote_id = Uuid::new_v4();

    // Remote sends metadata as a JSON number (valid JSON but not an object).
    // stamp_synced_origin should wrap it under "original" rather than discard it.
    let fields = single_field("metadata", serde_json::json!(42), 1);
    let remote = remote_for(remote_id, fields);

    ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    let metadata: String = conn
        .query_row(
            "SELECT metadata FROM recordings WHERE id = ?1",
            [remote_id.to_string()],
            |row| row.get(0),
        )
        .expect("query metadata");

    let meta: serde_json::Value = serde_json::from_str(&metadata).expect("valid json");

    // The original value should be preserved under "original".
    assert_eq!(
        meta.get("original").and_then(|v| v.as_i64()),
        Some(42),
        "non-object metadata must be wrapped under 'original', not discarded: {metadata}"
    );
    // synced_from must also be present.
    assert!(
        meta.get("synced_from").is_some(),
        "synced_from must be stamped even when metadata was non-object: {metadata}"
    );
}

// -------------------------------------------------------------------------
// Merge: atomic batch — one bad UUID in the middle rolls back all writes.
//
// This is the transaction-integrity property the C2/C3 cursor-advance fix
// in content_sync.rs relies on: merge_incoming either fully merges a batch
// or rolls back entirely.
// -------------------------------------------------------------------------

#[test]
fn merge_atomic_batch_rolls_back_on_bad_uuid() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let good_id = Uuid::new_v4();

    let good_fields = single_field("transcript", serde_json::json!("good content"), 1);
    let good_remote = remote_for(good_id, good_fields);

    // Bad recording with an unparseable UUID.
    let bad_fields = single_field("transcript", serde_json::json!("bad content"), 1);
    let mut bad_remote = remote_for(Uuid::new_v4(), bad_fields);
    bad_remote.id = "not-a-uuid".to_string();

    let result = ContentSyncRepo::merge_incoming(&conn, &[good_remote, bad_remote]);

    assert!(result.is_err(), "batch with bad UUID must fail");

    // The good recording must NOT have been partially inserted (transaction rollback).
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            [good_id.to_string()],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(
        count, 0,
        "good recording must not persist when batch rolled back"
    );
}

// -------------------------------------------------------------------------
// Merge: remote wins for a field with no local revision.
//
// When the local recording exists but has no revision row for a given
// field, the remote value is applied unconditionally.
// -------------------------------------------------------------------------

#[test]
fn merge_remote_wins_when_no_local_revision() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);

    let fields = single_field("transcript", serde_json::json!("remote transcript text"), 1);
    let remote = remote_for(id, fields);

    let MergeResult {
        conflicts,
        changed_recording_ids,
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(conflicts.is_empty(), "no conflict when no local revision");
    assert!(
        changed_recording_ids.contains(&id.to_string()),
        "recording should be marked changed"
    );

    let transcript: Option<String> = conn
        .query_row(
            "SELECT transcript FROM recordings WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("query transcript");
    assert_eq!(
        transcript.as_deref(),
        Some("remote transcript text"),
        "remote value must be applied"
    );
}

// -------------------------------------------------------------------------
// Merge: equal timestamps keep local value (no conflict).
// -------------------------------------------------------------------------

#[test]
fn merge_equal_timestamps_keeps_local() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);

    // Local revision at T5.
    ContentSyncRepo::upsert_revision(&conn, &id, "soap_note", &now(5), None)
        .expect("upsert local");
    {
        let mut r = RecordingsRepo::get_by_id(&conn, &id).expect("get");
        r.soap_note = Some("local content".to_string());
        RecordingsRepo::update(&conn, &r).expect("update");
    }

    // Remote at same timestamp T5.
    let fields = single_field("soap_note", serde_json::json!("remote content at same time"), 5);
    let remote = remote_for(id, fields);

    let MergeResult { conflicts, .. } =
        ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(conflicts.is_empty(), "equal timestamps must not conflict");

    let soap: Option<String> = conn
        .query_row(
            "SELECT soap_note FROM recordings WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("query");
    assert_eq!(
        soap.as_deref(),
        Some("local content"),
        "local value preserved on timestamp tie"
    );
}

// -------------------------------------------------------------------------
// Merge: unknown field names are silently ignored (forward compatibility).
// -------------------------------------------------------------------------

#[test]
fn merge_ignores_unknown_fields() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);

    let fields = single_field("future_field_name", serde_json::json!("some value"), 1);
    let remote = remote_for(id, fields);

    let result = ContentSyncRepo::merge_incoming(&conn, &[remote]);
    assert!(result.is_ok(), "unknown fields must not cause errors");
}

// -------------------------------------------------------------------------
// Merge: deletion tombstone for a recording we don't have locally.
//
// The tombstone is inserted so the deletion is durable and doesn't
// re-appear on the next pull.
// -------------------------------------------------------------------------

#[test]
fn merge_inserts_tombstone_for_unknown_deleted_recording() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let ghost_id = Uuid::new_v4();
    let remote = SyncRecording {
        id: ghost_id.to_string(),
        filename: "ghost.wav".to_string(),
        created_at: now(0),
        updated_at: now(5),
        deleted_at: Some(now(5)),
        patient_name: None,
        duration_seconds: None,
        file_size_bytes: None,
        stt_provider: None,
        ai_provider: None,
        fields: HashMap::new(),
    };

    let MergeResult {
        conflicts,
        changed_recording_ids,
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(conflicts.is_empty());
    assert!(changed_recording_ids.contains(&ghost_id.to_string()));

    // Verify the tombstone row exists with deleted_at set.
    let deleted_at: Option<String> = conn
        .query_row(
            "SELECT deleted_at FROM recordings WHERE id = ?1",
            [ghost_id.to_string()],
            |row| row.get(0),
        )
        .expect("query");
    assert!(deleted_at.is_some(), "tombstone row must be inserted");
}
