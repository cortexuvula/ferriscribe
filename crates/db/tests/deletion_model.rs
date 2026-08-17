//! Integration tests for the timestamped-tombstone deletion model in
//! `ContentSyncRepo::merge_incoming`:
//!
//! - Remote tombstone vs local live row → LWW by timestamp (deletion wins ties)
//! - Remote live row vs local tombstone → LWW restore when remote is newer
//! - Purge-ledger refusal of stale re-inserts from machines that missed a
//!   practice-wide deletion
//!
//! **HIPAA note:** assertions touch only ids, counts, timestamps, and
//! visibility; fixture filenames and field payloads are synthetic non-PHI
//! filler.

use std::path::PathBuf;

use medical_core::types::recording::Recording;
use medical_db::Database;
use medical_db::content_sync::{ContentSyncRepo, MergeResult, SyncFieldValue, SyncRecording};
use medical_db::recordings::RecordingsRepo;
use uuid::Uuid;

/// Fixed, strictly-ordered timestamps: T0 < T1 < T2.
const T0: &str = "2026-08-10T00:00:00Z";
const T1: &str = "2026-08-11T00:00:00Z";
const T2: &str = "2026-08-12T00:00:00Z";

/// Insert a live, FTS-indexed recording row, returning the fixture.
fn seed(conn: &rusqlite::Connection, filename: &str) -> Recording {
    let rec = Recording::new(filename, PathBuf::from(format!("/audio/{filename}")));
    RecordingsRepo::insert(conn, &rec).expect("seed");
    rec
}

/// Build an incoming `SyncRecording`. `fields` are `(name, json string
/// payload, field timestamp)` tuples; `json_value` is wrapped with
/// `serde_json::json!` so text fields arrive as `Value::String`.
fn sync_rec(
    id: &Uuid,
    updated_at: &str,
    deleted_at: Option<&str>,
    fields: Vec<(&str, &str, &str)>,
) -> SyncRecording {
    SyncRecording {
        id: id.to_string(),
        filename: "incoming.wav".to_string(),
        created_at: T0.to_string(),
        updated_at: updated_at.to_string(),
        deleted_at: deleted_at.map(str::to_string),
        patient_name: None,
        duration_seconds: None,
        file_size_bytes: None,
        stt_provider: None,
        ai_provider: None,
        fields: fields
            .into_iter()
            .map(|(name, value, ts)| (name.to_string(), field_value(value, ts)))
            .collect(),
    }
}

/// Helper for `sync_rec`: build one `SyncFieldValue` from a string payload.
fn field_value(value: &str, ts: &str) -> SyncFieldValue {
    SyncFieldValue {
        value: serde_json::json!(value),
        updated_at: ts.to_string(),
        origin_device: Some("peer-device".to_string()),
    }
}

/// Raw `deleted_at` column read: `None` = live, `Some` = tombstoned.
fn deleted_at_raw(conn: &rusqlite::Connection, id: &Uuid) -> Option<String> {
    conn.query_row(
        "SELECT deleted_at FROM recordings WHERE id = ?1",
        [id.to_string()],
        |r| r.get(0),
    )
    .expect("query deleted_at")
}

/// Overwrite the row's `updated_at` while the row is live and indexed. The
/// FTS update trigger fires delete+insert with identical FTS column values
/// (only `updated_at` changed), so this is index-safe.
fn set_updated_at(conn: &rusqlite::Connection, id: &Uuid, ts: &str) {
    conn.execute(
        "UPDATE recordings SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![ts, id.to_string()],
    )
    .expect("set updated_at");
}

/// Put a live row into the tombstoned state with an exact `deleted_at`
/// timestamp, keeping the FTS index consistent. The UPDATE runs while the
/// row is still indexed (trigger does a same-values delete+insert), then the
/// row is explicitly de-indexed — mirroring what `soft_delete` would have
/// produced with a time-travelled clock.
fn soft_delete_at(conn: &rusqlite::Connection, id: &Uuid, ts: &str) {
    conn.execute(
        "UPDATE recordings SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
        rusqlite::params![ts, id.to_string()],
    )
    .expect("backdate tombstone");
    conn.execute(
        "INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
         SELECT 'delete', rowid, id, filename, transcript, soap_note, referral, letter, patient_name
         FROM recordings WHERE id = ?1",
        [id.to_string()],
    )
    .expect("de-index after backdating");
}

/// `recordings_fts` is an external-content FTS5 table: plain SELECTs read
/// from the content table and never observe de-indexing. Index membership
/// must be probed with a MATCH on a token unique to the fixture.
fn fts_row_present(conn: &rusqlite::Connection, filename_stem: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM recordings_fts WHERE recordings_fts MATCH ?1",
        [format!("filename:{filename_stem}")],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

/// Assert the external-content FTS index is internally consistent — a
/// 'delete' against absent index state corrupts it and only surfaces later
/// as SQLITE_CORRUPT on an unrelated query.
fn assert_fts_healthy(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO recordings_fts(recordings_fts) VALUES('integrity-check')",
        [],
    )
    .expect("FTS integrity-check must pass");
}

// -------------------------------------------------------------------------
// Test 1: a newer remote tombstone deletes the local live row.
// -------------------------------------------------------------------------

#[test]
fn remote_tombstone_newer_than_local_live_deletes() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let rec = seed(&conn, "lwwdel.wav");
    set_updated_at(&conn, &rec.id, T1);
    assert!(fts_row_present(&conn, "lwwdel"));

    let remote = sync_rec(&rec.id, T2, Some(T2), vec![]);
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert_eq!(
        deleted_at_raw(&conn, &rec.id).as_deref(),
        Some(T2),
        "newer remote tombstone must delete the local row"
    );
    assert!(
        !fts_row_present(&conn, "lwwdel"),
        "tombstoned row must leave the FTS index"
    );
    assert!(changed_recording_ids.contains(&rec.id.to_string()));
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 2: a local live edit newer than the remote tombstone wins.
// -------------------------------------------------------------------------

#[test]
fn local_live_newer_than_remote_tombstone_wins() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let rec = seed(&conn, "lwwlive.wav");
    set_updated_at(&conn, &rec.id, T2);
    assert!(fts_row_present(&conn, "lwwlive"));

    // A machine that hasn't seen the local T2 edit pushes an older T1 delete.
    let remote = sync_rec(&rec.id, T1, Some(T1), vec![]);
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(
        deleted_at_raw(&conn, &rec.id).is_none(),
        "stale remote tombstone must not delete a newer local live row"
    );
    assert!(
        fts_row_present(&conn, "lwwlive"),
        "live row must keep its FTS entry"
    );
    assert!(
        !changed_recording_ids.contains(&rec.id.to_string()),
        "nothing changed locally"
    );
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 3: timestamp tie between local live row and remote tombstone →
// the tombstone wins (deletes win ties).
// -------------------------------------------------------------------------

#[test]
fn tie_tombstone_wins() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let rec = seed(&conn, "lwwtie.wav");
    set_updated_at(&conn, &rec.id, T1);
    assert!(fts_row_present(&conn, "lwwtie"));

    // deleted_at exactly equals the local updated_at.
    let remote = sync_rec(&rec.id, T1, Some(T1), vec![]);
    ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert_eq!(
        deleted_at_raw(&conn, &rec.id).as_deref(),
        Some(T1),
        "on a tie the deletion must win"
    );
    assert!(
        !fts_row_present(&conn, "lwwtie"),
        "tied tombstone must still de-index the row"
    );
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 4: a newer remote live row restores a locally tombstoned row and
// its field edits are applied.
// -------------------------------------------------------------------------

#[test]
fn remote_live_newer_than_local_tombstone_restores() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let rec = seed(&conn, "lwwres.wav");
    soft_delete_at(&conn, &rec.id, T1);
    assert!(!fts_row_present(&conn, "lwwres"));

    // A peer restored + edited the recording after our local delete.
    let remote = sync_rec(
        &rec.id,
        T2,
        None,
        vec![("soap_note", "edited after restore", T2)],
    );
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(
        deleted_at_raw(&conn, &rec.id).is_none(),
        "newer remote live row must restore the local tombstone"
    );
    let soap: Option<String> = conn
        .query_row(
            "SELECT soap_note FROM recordings WHERE id = ?1",
            [rec.id.to_string()],
            |r| r.get(0),
        )
        .expect("query soap_note");
    assert_eq!(
        soap.as_deref(),
        Some("edited after restore"),
        "fields carried by the newer live row must be applied post-restore"
    );
    assert!(
        fts_row_present(&conn, "lwwres"),
        "restored row must be searchable again"
    );
    assert!(changed_recording_ids.contains(&rec.id.to_string()));
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 5: an older remote live row does NOT restore a local tombstone,
// and its field edits stay guarded.
// -------------------------------------------------------------------------

#[test]
fn remote_live_older_than_local_tombstone_stays_deleted() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let rec = seed(&conn, "lwwstays.wav");
    soft_delete_at(&conn, &rec.id, T1);
    assert!(!fts_row_present(&conn, "lwwstays"));

    // A stale machine pushes a pre-delete live copy with an old edit.
    let remote = sync_rec(
        &rec.id,
        T0,
        None,
        vec![("soap_note", "stale pre-delete edit", T0)],
    );
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert_eq!(
        deleted_at_raw(&conn, &rec.id).as_deref(),
        Some(T1),
        "older remote live row must not resurrect the local tombstone"
    );
    let soap: Option<String> = conn
        .query_row(
            "SELECT soap_note FROM recordings WHERE id = ?1",
            [rec.id.to_string()],
            |r| r.get(0),
        )
        .expect("query soap_note");
    assert!(
        soap.is_none(),
        "fields from a pre-delete copy must not be applied"
    );
    assert!(
        !fts_row_present(&conn, "lwwstays"),
        "tombstoned row must stay out of the FTS index"
    );
    assert!(
        !changed_recording_ids.contains(&rec.id.to_string()),
        "tombstone standing means nothing changed locally"
    );
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 6: the purge ledger refuses re-insertion of a purged recording.
// -------------------------------------------------------------------------

#[test]
fn purged_recording_refused_on_insert() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = Uuid::new_v4();

    // The office server purged this recording at T2.
    conn.execute(
        "INSERT INTO purged_recordings (id, purged_at) VALUES (?1, ?2)",
        rusqlite::params![id.to_string(), T2],
    )
    .expect("seed purge ledger");

    // A machine that missed the deletion pushes its stale T1 live copy.
    let remote = sync_rec(&id, T1, None, vec![("transcript", "stale copy", T1)]);
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(count, 0, "purged recording must not be re-inserted");
    assert!(
        changed_recording_ids.is_empty(),
        "refused insert must not report changes"
    );
}

// -------------------------------------------------------------------------
// Test 7: without a ledger entry the insert proceeds (guards against
// over-blocking).
// -------------------------------------------------------------------------

#[test]
fn non_ledgered_insert_unchanged() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = Uuid::new_v4();

    let remote = sync_rec(&id, T1, None, vec![("transcript", "fresh copy", T1)]);
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(count, 1, "non-ledgered recording must be inserted");
    assert!(changed_recording_ids.contains(&id.to_string()));
}
