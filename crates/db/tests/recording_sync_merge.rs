//! Integration tests for `ContentSyncRepo` per-field last-write-wins merge.
//!
//! These tests verify the four core merge scenarios described in the task:
//! remote-wins-when-newer, local-wins-conflict, different-fields-both-win,
//! and deletion-propagation.
//!
//! **HIPAA note:** No transcript or SOAP content is logged. Assertions check
//! lengths and timestamps only; sample text is synthetic non-PHI filler.

use std::collections::HashMap;
use std::path::PathBuf;

use medical_core::types::recording::Recording;
use medical_db::Database;
use medical_db::content_sync::{ContentSyncRepo, MergeResult, SyncFieldValue, SyncRecording};
use medical_db::recordings::RecordingsRepo;
use uuid::Uuid;

/// ISO 8601 timestamp offset from a fixed base epoch (milliseconds).
fn now(offset_secs: i64) -> String {
    let base = chrono::DateTime::parse_from_rfc3339("2026-07-10T10:00:00Z").unwrap();
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

/// Build a sparse field-value map containing a single field.
fn field_value(field: &str, value: &str, offset: i64) -> (String, SyncFieldValue) {
    (
        field.to_string(),
        SyncFieldValue {
            value: serde_json::Value::String(value.to_string()),
            updated_at: now(offset),
            origin_device: Some("remote-machine".to_string()),
        },
    )
}

/// Build a `SyncRecording` referring to an existing local recording id,
/// with the given sparse fields map.
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

/// Fetch the local recording's column value for a text field.
fn get_text_column(conn: &rusqlite::Connection, id: Uuid, column: &str) -> Option<String> {
    let sql = format!("SELECT {column} FROM recordings WHERE id = ?1");
    conn.query_row(&sql, [id.to_string()], |row| {
        row.get::<_, Option<String>>(0)
    })
    .expect("query column")
}

// -------------------------------------------------------------------------
// Test 1: remote wins when its field revision is newer.
// -------------------------------------------------------------------------

#[test]
fn merge_remote_wins_when_newer() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);

    // Local revision: soap_note at T1.
    ContentSyncRepo::upsert_revision(&conn, &id, "soap_note", &now(1), None)
        .expect("upsert local revision");
    // Set local soap_note so we can confirm it gets overwritten.
    {
        let mut r = RecordingsRepo::get_by_id(&conn, &id).expect("get");
        r.soap_note = Some("local older content".to_string());
        RecordingsRepo::update(&conn, &r).expect("set local soap_note");
    }

    // Remote: soap_note at T2 (newer), different content.
    let mut fields = HashMap::new();
    let (k, v) = field_value("soap_note", "remote newer content", 2);
    fields.insert(k, v);
    let remote = remote_for(id, fields);

    let MergeResult {
        conflicts,
        changed_recording_ids,
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(
        conflicts.is_empty(),
        "no conflict expected when remote newer"
    );
    assert!(
        changed_recording_ids.contains(&id.to_string()),
        "recording should be marked changed"
    );

    let soap = get_text_column(&conn, id, "soap_note");
    assert_eq!(soap.as_deref(), Some("remote newer content"));
}

// -------------------------------------------------------------------------
// Test 2: local wins, conflict is reported.
// -------------------------------------------------------------------------

#[test]
fn merge_local_wins_conflict_reported() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);

    // Local revision: soap_note at T2 (later).
    ContentSyncRepo::upsert_revision(&conn, &id, "soap_note", &now(2), None)
        .expect("upsert local revision");
    {
        let mut r = RecordingsRepo::get_by_id(&conn, &id).expect("get");
        r.soap_note = Some("local newer content".to_string());
        RecordingsRepo::update(&conn, &r).expect("set local soap_note");
    }

    // Remote: soap_note at T1 (older), different content.
    let mut fields = HashMap::new();
    let (k, v) = field_value("soap_note", "remote older content", 1);
    fields.insert(k, v);
    let remote = remote_for(id, fields);

    let MergeResult { conflicts, .. } =
        ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert_eq!(conflicts.len(), 1, "expected one conflict for soap_note");
    assert_eq!(conflicts[0].field, "soap_note");

    let soap = get_text_column(&conn, id, "soap_note");
    assert_eq!(
        soap.as_deref(),
        Some("local newer content"),
        "local value must be preserved"
    );
}

// -------------------------------------------------------------------------
// Test 3: different fields from each side both win (no conflict).
// -------------------------------------------------------------------------

#[test]
fn merge_different_fields_both_win() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);

    // Local has transcript revision at T1.
    ContentSyncRepo::upsert_revision(&conn, &id, "transcript", &now(1), None)
        .expect("upsert local transcript revision");
    {
        let mut r = RecordingsRepo::get_by_id(&conn, &id).expect("get");
        r.transcript = Some("local transcript".to_string());
        RecordingsRepo::update(&conn, &r).expect("set local transcript");
    }

    // Remote brings soap_note at T2 (different field).
    let mut fields = HashMap::new();
    let (k, v) = field_value("soap_note", "remote soap", 2);
    fields.insert(k, v);
    let remote = remote_for(id, fields);

    let MergeResult { conflicts, .. } =
        ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(conflicts.is_empty(), "different fields must not conflict");

    // Both values present locally.
    assert_eq!(
        get_text_column(&conn, id, "transcript").as_deref(),
        Some("local transcript")
    );
    assert_eq!(
        get_text_column(&conn, id, "soap_note").as_deref(),
        Some("remote soap")
    );
}

// -------------------------------------------------------------------------
// Test 4: deletion propagates from remote.
// -------------------------------------------------------------------------

#[test]
fn merge_deletion_propagates() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);
    let id_str = id.to_string();

    // Remote arrives with a deleted_at set; local is not deleted.
    let remote = SyncRecording {
        id: id_str,
        filename: "visit.wav".to_string(),
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

    let MergeResult { conflicts, .. } =
        ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(
        conflicts.is_empty(),
        "deletion should not produce conflicts"
    );

    let deleted_at: Option<String> = conn
        .query_row(
            "SELECT deleted_at FROM recordings WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("query deleted_at");
    assert!(
        deleted_at.is_some(),
        "recording must be soft-deleted after merge"
    );
}

// -------------------------------------------------------------------------
// FTS-safety helpers for the soft-delete regression tests below.
// -------------------------------------------------------------------------

/// Assert the external-content FTS index is internally consistent.
fn assert_fts_healthy(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO recordings_fts(recordings_fts) VALUES('integrity-check')",
        [],
    )
    .expect("FTS integrity-check must pass");
}

// -------------------------------------------------------------------------
// Test 5: a remote field edit on a locally-trashed recording is a safe no-op.
// -------------------------------------------------------------------------

#[test]
fn merge_field_edit_into_locally_trashed_recording_is_fts_safe() {
    // Regression: a remote field edit landing on a locally soft-deleted
    // (FTS-de-indexed) recording fired the update trigger against absent
    // index state → SQLITE_CORRUPT. A local tombstone wins over peer field
    // edits; the merge itself must succeed so the sync cursor advances.
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);
    RecordingsRepo::soft_delete(&conn, &id).expect("local soft delete");

    // Remote edits the transcript; no local revision exists, so on a live
    // row the remote value would win — on a trashed row it must not land.
    let mut fields = HashMap::new();
    let (k, v) = field_value("transcript", "remote edit for trashed row", 10);
    fields.insert(k, v);
    let remote = remote_for(id, fields);

    // Pre-fix this failed inside the transaction with SQLITE_CORRUPT.
    let MergeResult { conflicts, .. } =
        ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge must not corrupt FTS");
    assert!(conflicts.is_empty(), "tombstone-wins is not a conflict");

    // The row stays trashed and the edit did not land.
    assert!(
        get_text_column(&conn, id, "deleted_at").is_some(),
        "local tombstone must win"
    );
    assert!(
        get_text_column(&conn, id, "transcript").is_none(),
        "remote field edit must not be applied to a trashed row"
    );

    assert_fts_healthy(&conn);

    // The trashed row can still be purged without corrupting FTS.
    let purged = RecordingsRepo::purge_soft_deleted(&conn, &[id]).expect("purge");
    assert_eq!(purged, vec![id], "trashed row must still be purgeable");
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 6: both-deleted earliest-wins reconciliation skips trashed rows.
// -------------------------------------------------------------------------

#[test]
fn merge_both_deleted_reconciliation_is_fts_safe() {
    // Regression: when both sides hold a tombstone and the remote one is
    // earlier, the merge used to UPDATE the locally-trashed (de-indexed)
    // row's deleted_at → the FTS update trigger fired against absent index
    // state → SQLITE_CORRUPT. The cosmetic timestamp reconciliation is
    // skipped instead; keeping the later local tombstone is conservative
    // (purge simply waits a little longer).
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = seed_recording(&conn);
    RecordingsRepo::soft_delete(&conn, &id).expect("local soft delete");
    let local_ts = get_text_column(&conn, id, "deleted_at").expect("local tombstone");

    // Remote tombstone dated earlier than the local one (the test clock's
    // base epoch precedes real `now` by over a month).
    let remote = SyncRecording {
        id: id.to_string(),
        filename: "visit.wav".to_string(),
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
    assert!(
        remote.deleted_at.as_deref().unwrap() < local_ts.as_str(),
        "fixture requires an earlier remote tombstone"
    );

    // Pre-fix this failed inside the transaction with SQLITE_CORRUPT.
    let MergeResult {
        conflicts,
        changed_recording_ids,
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge must not corrupt FTS");
    assert!(conflicts.is_empty());
    assert!(
        changed_recording_ids.is_empty(),
        "skipping cosmetic reconciliation must not report a change"
    );

    // The local tombstone (later timestamp) is preserved untouched.
    assert_eq!(
        get_text_column(&conn, id, "deleted_at").as_deref(),
        Some(local_ts.as_str()),
        "local deleted_at must be untouched"
    );
    assert_fts_healthy(&conn);

    // The row can still be purged cleanly.
    let purged = RecordingsRepo::purge_soft_deleted(&conn, &[id]).expect("purge");
    assert_eq!(purged, vec![id], "trashed row must still be purgeable");
    assert_fts_healthy(&conn);
}
