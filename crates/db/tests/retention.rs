//! Integration tests for the per-machine recordings retention sweep
//! (`RecordingsRepo::retention_soft_delete_older_than`) and the
//! restore-time `retention_exempt` metadata stamp.
//!
//! **HIPAA note:** No transcript or SOAP content is logged. Assertions
//! check ids, counts, and visibility; fixture filenames are synthetic.

use std::path::PathBuf;

use medical_core::types::recording::Recording;
use medical_db::Database;
use medical_db::recordings::RecordingsRepo;
use uuid::Uuid;

/// Insert a recording whose `created_at` is `days` days in the past,
/// returning the inserted fixture.
fn seed_days_old(conn: &rusqlite::Connection, days: i64, filename: &str) -> Recording {
    let mut rec = Recording::new(filename, PathBuf::from(format!("/audio/{filename}")));
    rec.created_at = chrono::Utc::now() - chrono::TimeDelta::days(days);
    RecordingsRepo::insert(conn, &rec).expect("insert fixture recording");
    rec
}

/// Read the raw `deleted_at` column: `None` = visible, `Some` = trashed.
///
/// `get_by_id` deliberately returns soft-deleted rows (no `deleted_at`
/// filter), so visibility has to be asserted at the column level or via
/// `list_all`.
fn deleted_at_raw(conn: &rusqlite::Connection, id: Uuid) -> Option<String> {
    conn.query_row(
        "SELECT deleted_at FROM recordings WHERE id = ?1",
        [id.to_string()],
        |row| row.get::<_, Option<String>>(0),
    )
    .expect("query deleted_at")
}

/// Ids of all visible recordings (list_all filters `deleted_at IS NULL`).
fn visible_ids(conn: &rusqlite::Connection) -> Vec<Uuid> {
    RecordingsRepo::list_all(conn, 100, 0)
        .expect("list_all")
        .into_iter()
        .map(|summary| summary.id)
        .collect()
}

// -------------------------------------------------------------------------
// Test 1: only old, visible recordings are swept.
// -------------------------------------------------------------------------

#[test]
fn retention_soft_deletes_only_old_visible_recordings() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let old = seed_days_old(&conn, 100, "old-visit.wav");
    let fresh = seed_days_old(&conn, 10, "fresh-visit.wav");

    let trashed = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("retention sweep");

    assert_eq!(
        trashed,
        vec![old.id],
        "sweep must trash exactly the 100-day-old recording"
    );
    assert!(
        deleted_at_raw(&conn, old.id).is_some(),
        "old recording must be soft-deleted"
    );
    assert!(
        deleted_at_raw(&conn, fresh.id).is_none(),
        "fresh recording must be untouched"
    );

    let visible = visible_ids(&conn);
    assert!(!visible.contains(&old.id), "old recording must be hidden");
    assert!(visible.contains(&fresh.id), "fresh recording stays visible");
}

// -------------------------------------------------------------------------
// Test 2: restoring a swept recording exempts it from future sweeps.
// -------------------------------------------------------------------------

#[test]
fn retention_respects_restore_exemption() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let rec = seed_days_old(&conn, 400, "restored-visit.wav");

    let first = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("first sweep");
    assert_eq!(first, vec![rec.id], "400-day-old recording is trashed");

    RecordingsRepo::restore(&conn, &rec.id).expect("restore");

    // Restore must stamp the exemption in metadata.
    let fetched = RecordingsRepo::get_by_id(&conn, &rec.id).expect("get_by_id");
    assert_eq!(
        fetched
            .metadata
            .get("retention_exempt")
            .and_then(serde_json::Value::as_bool),
        Some(true),
        "restore must set retention_exempt = true in metadata"
    );

    let second = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("second sweep");
    assert!(
        second.is_empty(),
        "exempt recording must not be re-trashed by a later sweep"
    );

    assert!(deleted_at_raw(&conn, rec.id).is_none(), "still visible");
    assert!(
        visible_ids(&conn).contains(&rec.id),
        "restored recording is listed again"
    );
}

// -------------------------------------------------------------------------
// Test 3: the sweep never touches rows already in the trash.
// -------------------------------------------------------------------------

#[test]
fn retention_is_idempotent_for_already_deleted() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let rec = seed_days_old(&conn, 100, "already-trashed.wav");
    RecordingsRepo::soft_delete(&conn, &rec.id).expect("manual soft delete");

    let trashed = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("retention sweep");

    assert!(
        trashed.is_empty(),
        "already-deleted rows must never be returned or touched"
    );
    assert!(
        deleted_at_raw(&conn, rec.id).is_some(),
        "recording stays in trash"
    );
}

// -------------------------------------------------------------------------
// Tombstone purge (30-day durable deletion)
// -------------------------------------------------------------------------

/// Soft-delete a recording with `deleted_at` set to `days` days ago.
///
/// `soft_delete` stamps `deleted_at = now` and immediately de-indexes the
/// row from `recordings_fts`. A plain backdating UPDATE *after* that would
/// fire `recordings_fts_update`'s 'delete' command against values that are
/// no longer in the index — the same SQLITE_CORRUPT class of failure the
/// purge is being tested against. So: run the real `soft_delete`, re-index
/// (the same command `restore` uses), backdate the tombstone, then de-index
/// again (the same command `soft_delete` uses). The net state is exactly
/// what `soft_delete` would have produced with a time-travelled clock.
fn soft_delete_days_ago(conn: &rusqlite::Connection, id: Uuid, days: i64) {
    RecordingsRepo::soft_delete(conn, &id).expect("soft delete");

    const FTS_COLS: &str =
        "rowid, id, filename, transcript, soap_note, referral, letter, patient_name";
    conn.execute(
        &format!(
            "INSERT INTO recordings_fts({FTS_COLS}) SELECT {FTS_COLS} FROM recordings WHERE id = ?1"
        ),
        [id.to_string()],
    )
    .expect("re-index for backdating");

    let ts = (chrono::Utc::now() - chrono::TimeDelta::days(days)).to_rfc3339();
    conn.execute(
        "UPDATE recordings SET deleted_at = ?1 WHERE id = ?2",
        rusqlite::params![ts, id.to_string()],
    )
    .expect("backdate tombstone");

    conn.execute(
        &format!(
            "INSERT INTO recordings_fts(recordings_fts, {FTS_COLS})
             SELECT 'delete', {FTS_COLS} FROM recordings WHERE id = ?1"
        ),
        [id.to_string()],
    )
    .expect("de-index after backdating");
}

/// Assert the external-content FTS index is internally consistent.
fn assert_fts_healthy(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO recordings_fts(recordings_fts) VALUES('integrity-check')",
        [],
    )
    .expect("FTS integrity-check must pass");
}

/// Raw row count for one recording id — distinguishes "hidden" (soft-deleted)
/// from "gone" (purged), which `get_by_id` alone can't express.
fn row_count(conn: &rusqlite::Connection, id: Uuid) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM recordings WHERE id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )
    .expect("count rows")
}

// -------------------------------------------------------------------------
// Test 4: purge hard-deletes a soft-deleted row without corrupting FTS.
// -------------------------------------------------------------------------

#[test]
fn purge_completes_after_soft_delete() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let rec = seed_days_old(&conn, 100, "purge-me.wav");
    soft_delete_days_ago(&conn, rec.id, 100);

    // The listing the sweeper consumes finds the 100-day-old tombstone.
    let listed = RecordingsRepo::list_soft_deleted_older_than(&conn, 30, chrono::Utc::now())
        .expect("list soft-deleted");
    assert!(
        listed.iter().any(|(id, _)| *id == rec.id),
        "100-day-old tombstone must be listed"
    );

    // The purge must succeed — the pre-fix code died here with
    // SQLITE_CORRUPT because the delete trigger supplied values for a
    // rowid no longer in the index.
    let purged =
        RecordingsRepo::purge_soft_deleted(&conn, &[rec.id]).expect("purge must not corrupt FTS");
    assert_eq!(purged, vec![rec.id], "purged ids must be reported");

    // The row is GONE from the table — not merely hidden.
    assert_eq!(row_count(&conn, rec.id), 0, "row must be hard-deleted");
    assert!(matches!(
        RecordingsRepo::get_by_id(&conn, &rec.id),
        Err(medical_db::DbError::NotFound(_))
    ));

    // The FTS index survived the hard DELETE intact.
    assert_fts_healthy(&conn);

    // The index is not wedged: a fresh soft_delete + restore cycle still
    // works on a new row.
    let fresh = seed_days_old(&conn, 0, "post-purge-cycle.wav");
    RecordingsRepo::soft_delete(&conn, &fresh.id).expect("cycle soft delete");
    assert_fts_healthy(&conn);
    RecordingsRepo::restore(&conn, &fresh.id).expect("cycle restore");
    assert!(deleted_at_raw(&conn, fresh.id).is_none());
    assert!(visible_ids(&conn).contains(&fresh.id));
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 5: purge never touches visible rows.
// -------------------------------------------------------------------------

#[test]
fn purge_skips_visible_rows() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let rec = seed_days_old(&conn, 100, "still-visible.wav");

    let purged = RecordingsRepo::purge_soft_deleted(&conn, &[rec.id]).expect("purge visible id");
    assert!(
        purged.is_empty(),
        "a visible recording must never be purged"
    );

    assert_eq!(row_count(&conn, rec.id), 1, "row must still exist");
    assert!(deleted_at_raw(&conn, rec.id).is_none(), "still visible");
    assert!(visible_ids(&conn).contains(&rec.id));
    assert_fts_healthy(&conn);
}

// -------------------------------------------------------------------------
// Test 6: the purge listing respects the age cutoff.
// -------------------------------------------------------------------------

#[test]
fn list_respects_cutoff() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");

    let older = seed_days_old(&conn, 200, "older-tombstone.wav");
    let newer = seed_days_old(&conn, 50, "newer-tombstone.wav");
    soft_delete_days_ago(&conn, older.id, 40);
    soft_delete_days_ago(&conn, newer.id, 10);

    let listed = RecordingsRepo::list_soft_deleted_older_than(&conn, 30, chrono::Utc::now())
        .expect("list soft-deleted");

    let ids: Vec<Uuid> = listed.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![older.id], "only the 40-day-old tombstone is due");
}
