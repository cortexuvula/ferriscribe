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
