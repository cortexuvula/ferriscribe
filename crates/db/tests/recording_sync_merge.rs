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
use medical_db::content_sync::{
    ContentSyncRepo, MergeResult, SyncFieldValue, SyncRecording,
};
use medical_db::recordings::RecordingsRepo;
use medical_db::Database;
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
    conn.query_row(&sql, [id.to_string()], |row| row.get::<_, Option<String>>(0))
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

    assert!(conflicts.is_empty(), "no conflict expected when remote newer");
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

    assert!(conflicts.is_empty(), "deletion should not produce conflicts");

    let deleted_at: Option<String> = conn
        .query_row(
            "SELECT deleted_at FROM recordings WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("query deleted_at");
    assert!(deleted_at.is_some(), "recording must be soft-deleted after merge");
}
