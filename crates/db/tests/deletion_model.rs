//! Integration tests for the timestamped-tombstone deletion model in
//! `ContentSyncRepo::merge_incoming`:
//!
//! - Remote tombstone vs local live row → LWW by timestamp (deletion wins ties)
//! - Remote live row vs local tombstone → LWW restore when remote is newer
//! - Purge-ledger refusal of stale re-inserts from machines that missed a
//!   practice-wide deletion
//! - The server-side purge writing that ledger atomically with the row
//!   deletion (`purge_soft_deleted_with_ledger`)
//! - The same tombstone-propagation contract for condition chips
//!   (`ConditionChipsRepo::merge_incoming`) — pins the tie-break that stops
//!   a stale client's push from resurrecting a practice-wide chip deletion
//!
//! **HIPAA note:** assertions touch only ids, counts, timestamps, and
//! visibility; fixture filenames and field payloads are synthetic non-PHI
//! filler.

use std::path::PathBuf;

use medical_core::types::condition_chip::{ConditionChip, deterministic_id};
use medical_core::types::recording::Recording;
use medical_db::Database;
use medical_db::condition_chips::ConditionChipsRepo;
use medical_db::content_sync::{
    ContentSyncRepo, MergeResult, PurgedRef, SyncFieldValue, SyncRecording,
};
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
    sync_rec_as("incoming.wav", id, updated_at, deleted_at, fields)
}

/// `sync_rec` with an explicit incoming filename, for tests that probe FTS
/// index membership via the filename token.
fn sync_rec_as(
    filename: &str,
    id: &Uuid,
    updated_at: &str,
    deleted_at: Option<&str>,
    fields: Vec<(&str, &str, &str)>,
) -> SyncRecording {
    SyncRecording {
        id: id.to_string(),
        filename: filename.to_string(),
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
// Test 6: the purge ledger refuses re-insertion of a purged recording —
// id-only: ANY ledger hit refuses regardless of timestamps.
//
// A machine offline across the deletion can EDIT its stale copy (fresh
// updated_at, same UUID); since genuinely re-created content always gets a
// NEW UUID, same-UUID + a ledger hit is always a stale copy. The
// purged_at-vs-updated_at comparison this test used to pin was removed for
// exactly that reason.
// -------------------------------------------------------------------------

#[test]
fn purged_recording_refused_regardless_of_timestamps() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    // Piercing case: the stale copy was edited AFTER the purge
    // (incoming updated_at T2 > purged_at T0). A timestamp comparison
    // would let it through; id-only refusal must not.
    let edited = Uuid::new_v4();
    conn.execute(
        "INSERT INTO purged_recordings (id, purged_at) VALUES (?1, ?2)",
        rusqlite::params![edited.to_string(), T0],
    )
    .expect("seed ledger (purge before edit)");

    // Classic case: a pre-delete copy (incoming T1 < purged_at T2).
    let stale = Uuid::new_v4();
    conn.execute(
        "INSERT INTO purged_recordings (id, purged_at) VALUES (?1, ?2)",
        rusqlite::params![stale.to_string(), T2],
    )
    .expect("seed ledger (purge after edit)");

    let remotes = vec![
        sync_rec(
            &edited,
            T2,
            None,
            vec![("transcript", "edited stale copy", T2)],
        ),
        sync_rec(&stale, T1, None, vec![("transcript", "stale copy", T1)]),
    ];
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &remotes).expect("merge");

    for id in [&edited, &stale] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recordings WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            count, 0,
            "any ledger hit must refuse the insert, whatever the timestamps"
        );
    }
    assert!(
        changed_recording_ids.is_empty(),
        "refused inserts must not report changes"
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

// -------------------------------------------------------------------------
// Case C: insert-as-tombstone for an unknown recording must leave the row
// DE-INDEXED. The FTS-insert trigger fires unconditionally on INSERT, so a
// tombstone row inserted this way lands in the FTS index; a later
// sync_restore would re-index the already-indexed row and leave a duplicate
// posting (a single FTS 'delete' then only removes one copy).
// -------------------------------------------------------------------------

#[test]
fn insert_as_tombstone_deindexes_fts() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = Uuid::new_v4();

    let remote = sync_rec_as("ghstins.wav", &id, T2, Some(T2), vec![]);
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert!(changed_recording_ids.contains(&id.to_string()));
    assert_eq!(
        deleted_at_raw(&conn, &id).as_deref(),
        Some(T2),
        "tombstone row must be inserted so the deletion is durable"
    );
    assert!(
        !fts_row_present(&conn, "ghstins"),
        "a row inserted as a tombstone must NOT sit in the FTS index"
    );
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Case C, refused variant: with a ledger row the tombstone insert is
// refused outright (id-only refusal — the ledger already makes the
// deletion durable).
// -------------------------------------------------------------------------

#[test]
fn insert_as_tombstone_with_ledger_row_refused() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = Uuid::new_v4();

    conn.execute(
        "INSERT INTO purged_recordings (id, purged_at) VALUES (?1, ?2)",
        rusqlite::params![id.to_string(), T1],
    )
    .expect("seed ledger");

    let remote = sync_rec_as("ghstref.wav", &id, T2, Some(T2), vec![]);
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
    assert_eq!(count, 0, "ledgered id must not be re-inserted as tombstone");
    assert!(changed_recording_ids.is_empty());
}

// -------------------------------------------------------------------------
// Restore-direction tie: an incoming live row whose updated_at EQUALS the
// local deleted_at stays deleted (restore requires strictly newer).
// -------------------------------------------------------------------------

#[test]
fn restore_tie_stays_deleted() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let rec = seed(&conn, "lwwrtie.wav");
    soft_delete_at(&conn, &rec.id, T1);
    assert!(!fts_row_present(&conn, "lwwrtie"));

    let remote = sync_rec(&rec.id, T1, None, vec![("soap_note", "tie edit", T1)]);
    let MergeResult {
        changed_recording_ids,
        ..
    } = ContentSyncRepo::merge_incoming(&conn, &[remote]).expect("merge");

    assert_eq!(
        deleted_at_raw(&conn, &rec.id).as_deref(),
        Some(T1),
        "a live row merely TIED with the tombstone must not restore it"
    );
    assert!(
        !fts_row_present(&conn, "lwwrtie"),
        "row must stay out of the FTS index"
    );
    assert!(
        !changed_recording_ids.contains(&rec.id.to_string()),
        "nothing changed locally"
    );
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Repeat-merge idempotency: merging the same batch a second time reports
// no changes and leaves the FTS index healthy.
// -------------------------------------------------------------------------

#[test]
fn repeat_merge_is_idempotent() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let live = seed(&conn, "idemlive.wav");
    set_updated_at(&conn, &live.id, T1);
    let doomed = seed(&conn, "idemdoom.wav");
    set_updated_at(&conn, &doomed.id, T1);

    let build = || {
        vec![
            sync_rec(
                &live.id,
                T2,
                None,
                vec![("soap_note", "edited remotely", T2)],
            ),
            sync_rec(&doomed.id, T2, Some(T2), vec![]),
        ]
    };

    let first = ContentSyncRepo::merge_incoming(&conn, &build()).expect("first merge");
    assert!(
        first.changed_recording_ids.contains(&live.id.to_string())
            && first.changed_recording_ids.contains(&doomed.id.to_string()),
        "first merge must report both recordings changed"
    );

    let second = ContentSyncRepo::merge_incoming(&conn, &build()).expect("second merge");
    assert!(
        second.changed_recording_ids.is_empty(),
        "re-merging an already-applied batch must report no changes"
    );
    assert!(second.conflicts.is_empty());

    // Final states survive the second pass untouched.
    assert!(deleted_at_raw(&conn, &live.id).is_none());
    assert!(fts_row_present(&conn, "idemlive"));
    assert_eq!(
        deleted_at_raw(&conn, &doomed.id).as_deref(),
        Some(T2),
        "tombstone must survive the repeat merge"
    );
    assert!(!fts_row_present(&conn, "idemdoom"));
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// The purge side of the ledger: `purge_soft_deleted_with_ledger` must
// hard-delete the tombstoned row AND write its `purged_recordings` entry in
// the SAME transaction — the row deletion and the resurrection block become
// atomic, so a crash can never leave a purged row un-ledgered (or vice
// versa). Only ids actually purged are ledgered: a visible row passed by
// mistake must neither vanish nor acquire a ledger entry (which would
// over-block a later legitimate sync insert — see
// `non_ledgered_insert_unchanged`).
// -------------------------------------------------------------------------

#[test]
fn purge_records_ledger_entries_transactionally() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    // A tombstone aged 40d past the 30-day window, seeded the same way the
    // retention sweeper test in src-tauri does: one UPDATE on the still-live
    // row (FTS columns unchanged, so the update trigger is a no-op
    // same-values delete+insert), then the explicit de-index.
    let rec = seed(&conn, "ldgpurge.wav");
    let aged = (chrono::Utc::now() - chrono::TimeDelta::days(40)).to_rfc3339();
    soft_delete_at(&conn, &rec.id, &aged);

    // Control: a visible row must never be purged or ledgered.
    let visible = seed(&conn, "ldgkeep.wav");

    let before = chrono::Utc::now();
    let purged = RecordingsRepo::purge_soft_deleted_with_ledger(&conn, &[rec.id, visible.id])
        .expect("purge with ledger");
    assert_eq!(
        purged,
        vec![rec.id],
        "only the tombstoned id is reported purged"
    );

    // The tombstoned row is gone from `recordings`…
    let gone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            [rec.id.to_string()],
            |r| r.get(0),
        )
        .expect("count purged row");
    assert_eq!(gone, 0, "purged row must be hard-deleted");

    // …and present in the ledger with a fresh, parseable `purged_at`.
    let purged_at: String = conn
        .query_row(
            "SELECT purged_at FROM purged_recordings WHERE id = ?1",
            [rec.id.to_string()],
            |r| r.get(0),
        )
        .expect("ledger row must exist for the purged id");
    let parsed = chrono::DateTime::parse_from_rfc3339(&purged_at)
        .expect("purged_at must be RFC3339")
        .with_timezone(&chrono::Utc);
    assert!(
        parsed >= before,
        "purged_at must be stamped by this purge call, got {purged_at}"
    );

    // The visible row survives un-ledgered.
    assert_eq!(
        deleted_at_raw(&conn, &visible.id).as_deref(),
        None,
        "visible row must stay live"
    );
    let ledgered_visible: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM purged_recordings WHERE id = ?1",
            [visible.id.to_string()],
            |r| r.get(0),
        )
        .expect("count ledger for visible id");
    assert_eq!(
        ledgered_visible, 0,
        "an id that was not purged must never be ledgered"
    );

    // Re-purging the same id is a harmless no-op (no row, no ledger churn).
    let again = RecordingsRepo::purge_soft_deleted_with_ledger(&conn, &[rec.id])
        .expect("re-purge must not fail");
    assert!(again.is_empty(), "already-purged id reports nothing");
    let still_ledgered: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM purged_recordings WHERE id = ?1",
            [rec.id.to_string()],
            |r| r.get(0),
        )
        .expect("count ledger after re-purge");
    assert_eq!(still_ledgered, 1, "ledger entry is durable");

    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Atomicity by fault injection: the row deletion must never survive a
// ledger-write failure. Dropping `purged_recordings` makes the in-transaction
// INSERT fail; the whole transaction must roll back, leaving the tombstoned
// row (and its FTS de-indexed state) exactly as before. Without the shared
// transaction a partial commit would purge the row un-ledgered — a durable
// deletion with no resurrection block.
// -------------------------------------------------------------------------

#[test]
fn ledger_write_failure_rolls_back_row_deletion() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let rec = seed(&conn, "ldgfail.wav");
    let aged = (chrono::Utc::now() - chrono::TimeDelta::days(40)).to_rfc3339();
    soft_delete_at(&conn, &rec.id, &aged);

    // Fault injection: make the ledger INSERT fail mid-transaction.
    conn.execute("DROP TABLE purged_recordings", [])
        .expect("drop ledger table");

    let result = RecordingsRepo::purge_soft_deleted_with_ledger(&conn, &[rec.id]);
    assert!(
        result.is_err(),
        "a failing ledger write must fail the whole purge"
    );

    // Rollback proof: the row must still exist, still tombstoned.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            [rec.id.to_string()],
            |r| r.get(0),
        )
        .expect("count rows");
    assert_eq!(
        count, 1,
        "the row deletion must roll back with the ledger failure"
    );
    assert_eq!(
        deleted_at_raw(&conn, &rec.id).as_deref(),
        Some(aged.as_str()),
        "the tombstone must be untouched by the rolled-back purge"
    );

    // Even the intermediate FTS re-index write rolled back: the row stays
    // de-indexed (as `soft_delete_at` left it) and the index stays healthy.
    assert!(
        !fts_row_present(&conn, "ldgfail"),
        "rolled-back purge must leave no FTS residue"
    );
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Purged ids travelling on the pull response. `purged_since` selects the
// ledger entries a client with the given cursor has not seen yet (all
// entries for a fresh client); `apply_purged_refs` tombstones stale LOCAL
// LIVE copies so a machine that missed the practice-wide deletion
// converges.
// -------------------------------------------------------------------------

#[test]
fn purged_since_filters_by_cutoff() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    conn.execute(
        "INSERT INTO purged_recordings (id, purged_at) VALUES (?1, ?2), (?3, ?4)",
        rusqlite::params![a.to_string(), T1, b.to_string(), T2],
    )
    .expect("seed ledger");

    // A cursor between T1 and T2: only the T2 entry is new to this client.
    let cutoff = "2026-08-11T12:00:00Z";
    let refs = ContentSyncRepo::purged_since(&conn, Some(cutoff)).expect("purged_since");
    assert_eq!(
        refs,
        vec![PurgedRef {
            id: b.to_string(),
            purged_at: T2.to_string(),
        }],
        "only ledger entries with purged_at strictly newer than the cursor are returned"
    );

    // A fresh client (no cursor) sees every entry, ordered by purged_at.
    let all = ContentSyncRepo::purged_since(&conn, None).expect("purged_since");
    assert_eq!(
        all,
        vec![
            PurgedRef {
                id: a.to_string(),
                purged_at: T1.to_string(),
            },
            PurgedRef {
                id: b.to_string(),
                purged_at: T2.to_string(),
            },
        ],
        "a fresh client's first pull carries the entire ledger, ordered by purged_at"
    );
}

#[test]
fn apply_purged_refs_tombstones_live_copies_only() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    // Two stale LIVE copies (this machine missed the deletion), one row the
    // machine had already tombstoned itself, and one id it has never seen.
    let live1 = seed(&conn, "prglive1.wav");
    let live2 = seed(&conn, "prglive2.wav");
    let gone = seed(&conn, "prggone.wav");
    soft_delete_at(&conn, &gone.id, T1);
    let unknown = Uuid::new_v4();
    assert!(fts_row_present(&conn, "prglive1"));
    assert!(fts_row_present(&conn, "prglive2"));

    let refs = vec![
        PurgedRef {
            id: live1.id.to_string(),
            purged_at: T2.to_string(),
        },
        PurgedRef {
            id: live2.id.to_string(),
            purged_at: T2.to_string(),
        },
        PurgedRef {
            id: gone.id.to_string(),
            purged_at: T2.to_string(),
        },
        PurgedRef {
            id: unknown.to_string(),
            purged_at: T2.to_string(),
        },
    ];
    ContentSyncRepo::apply_purged_refs(&conn, &refs).expect("apply_purged_refs");

    // The stale live copies are tombstoned (FTS-safe).
    assert_eq!(
        deleted_at_raw(&conn, &live1.id).as_deref(),
        Some(T2),
        "stale live copy must be tombstoned at the purge timestamp"
    );
    assert_eq!(
        deleted_at_raw(&conn, &live2.id).as_deref(),
        Some(T2),
        "stale live copy must be tombstoned at the purge timestamp"
    );
    assert!(
        !fts_row_present(&conn, "prglive1"),
        "tombstoned copy must leave the FTS index"
    );
    assert!(
        !fts_row_present(&conn, "prglive2"),
        "tombstoned copy must leave the FTS index"
    );

    // The already-tombstoned row keeps its original deleted_at (a later
    // re-tombstone would be a no-op at best and an FTS hazard at worst).
    assert_eq!(
        deleted_at_raw(&conn, &gone.id).as_deref(),
        Some(T1),
        "already-tombstoned row must keep its original deleted_at"
    );

    // The unknown id stays unknown — the local sweeper, not the sync
    // layer, owns inserting durable tombstone rows for unseen ids.
    let unknown_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            [unknown.to_string()],
            |r| r.get(0),
        )
        .expect("count unknown id");
    assert_eq!(unknown_rows, 0, "unknown id must not be inserted");

    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Condition chips: the tombstone-propagation half of the chips deletion
// model. The office server serves FULL chip lists (active + tombstones) so
// deletions travel to every client; this test pins the `merge_incoming`
// semantics that make that safe:
//
// 1. a newer remote tombstone removes the chip from the local active list
//    (and is retained as a durable tombstone row), and
// 2. a stale client that missed the deletion pushes its ACTIVE copy back
//    with the SAME updated_at — the tie must keep the tombstone, so the
//    chip cannot ghost-resurrect practice-wide. Only a strictly newer
//    active row (a genuine re-add) may resurrect it.
//
// Chip text here is a synthetic generic label, matching the repo's own
// unit-test fixtures — assertions touch only id, deleted_at, and counts.
// -------------------------------------------------------------------------

#[test]
fn chips_merge_applies_remote_tombstone() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    // Local: an active chip, seeded through the real write path.
    ConditionChipsRepo::add(&conn, "Hypertension", T1).expect("seed local active chip");
    let id = deterministic_id("Hypertension");
    assert_eq!(
        ConditionChipsRepo::list_active(&conn)
            .expect("list_active")
            .len(),
        1,
        "fixture: chip starts active"
    );

    // The same chip as a remote tombstone: newer updated_at, deleted_at set.
    let tombstone = ConditionChip {
        id: id.clone(),
        text: "Hypertension".to_string(),
        updated_at: T2.to_string(),
        deleted_at: Some(T2.to_string()),
        sort_order: 0,
        use_count: 0,
    };
    let merged = ConditionChipsRepo::merge_incoming(&conn, std::slice::from_ref(&tombstone))
        .expect("merge tombstone");
    assert!(
        merged.is_empty(),
        "newer remote tombstone must drop the chip from the active list"
    );
    let all = ConditionChipsRepo::list_all(&conn).expect("list_all");
    assert_eq!(all.len(), 1, "the tombstone row itself must be retained");
    assert_eq!(
        all[0].deleted_at.as_deref(),
        Some(T2),
        "row must carry the tombstone timestamp"
    );

    // A stale client that never saw the deletion pushes its ACTIVE copy
    // back, with updated_at TIED to the tombstone — the tie must keep the
    // tombstone (deletions win ties).
    let stale_active = ConditionChip {
        deleted_at: None,
        ..tombstone.clone()
    };
    let merged_again =
        ConditionChipsRepo::merge_incoming(&conn, &[stale_active]).expect("merge stale active");
    assert!(
        merged_again.is_empty(),
        "tie on updated_at must not resurrect the tombstoned chip"
    );
    assert_eq!(
        ConditionChipsRepo::list_all(&conn).expect("list_all")[0]
            .deleted_at
            .as_deref(),
        Some(T2),
        "tombstone must survive the stale client's push"
    );

    // Only a strictly newer ACTIVE row may resurrect — re-add semantics.
    let newer_active = ConditionChip {
        updated_at: "2026-08-13T00:00:00Z".to_string(),
        deleted_at: None,
        ..tombstone
    };
    let resurrected =
        ConditionChipsRepo::merge_incoming(&conn, &[newer_active]).expect("merge newer active");
    assert_eq!(resurrected.len(), 1, "a strictly newer active row re-adds");
    assert!(
        resurrected[0].deleted_at.is_none(),
        "re-added chip must be active"
    );
}
