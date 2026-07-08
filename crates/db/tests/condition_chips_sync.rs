//! Integration test: condition chip sync round-trip between two independent
//! databases (simulating two machines).

use medical_core::types::condition_chip::{ConditionChip, deterministic_id};
use medical_db::condition_chips::ConditionChipsRepo;
use medical_db::Database;

/// ISO 8601 timestamp offset from a fixed base epoch.
fn now(offset_secs: i64) -> String {
    let base = chrono::DateTime::parse_from_rfc3339("2026-07-08T10:00:00Z").unwrap();
    let t = base + chrono::Duration::seconds(offset_secs);
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Simulate a sync round-trip: A pushes its full list, server (B) merges,
/// returns active list, A merges the result back.
///
/// Both sides push `list_all` (including tombstones) so tombstones propagate.
fn sync_roundtrip(
    conn_a: &rusqlite::Connection,
    conn_b: &rusqlite::Connection,
) -> (Vec<ConditionChip>, Vec<ConditionChip>) {
    let a_all = ConditionChipsRepo::list_all(conn_a).unwrap();
    let _b_result = ConditionChipsRepo::merge_incoming(conn_b, &a_all).unwrap();
    let b_all = ConditionChipsRepo::list_all(conn_b).unwrap();
    let a_result = ConditionChipsRepo::merge_incoming(conn_a, &b_all).unwrap();
    let b_result = ConditionChipsRepo::list_active(conn_b).unwrap();
    (a_result, b_result)
}

/// Collect the active chip texts in their displayed sort order.
fn texts(chips: &[ConditionChip]) -> Vec<String> {
    chips.iter().map(|c| c.text.clone()).collect()
}

#[test]
fn both_add_unique_chips_converge() {
    // A adds "Hypertension", B adds "Diabetes". After a round-trip both
    // machines must hold both chips.
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    ConditionChipsRepo::add(&conn_a, "Hypertension", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "Diabetes", &now(0)).unwrap();

    let (a_after, b_after) = sync_roundtrip(&conn_a, &conn_b);

    assert_eq!(a_after.len(), 2, "A should have both chips");
    assert_eq!(b_after.len(), 2, "B should have both chips");

    let mut a_texts = texts(&a_after);
    let mut b_texts = texts(&b_after);
    a_texts.sort();
    b_texts.sort();
    assert_eq!(a_texts, vec!["Diabetes".to_string(), "Hypertension".to_string()]);
    assert_eq!(b_texts, vec!["Diabetes".to_string(), "Hypertension".to_string()]);
}

#[test]
fn both_add_same_chip_converge_to_one() {
    // A adds "Asthma" at t=0, B adds "Asthma" at t=1. Both produce the same
    // deterministic id, so after a round-trip there must be exactly one
    // Asthma chip on each side (no duplicate).
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    ConditionChipsRepo::add(&conn_a, "Asthma", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "Asthma", &now(1)).unwrap();

    let (a_after, b_after) = sync_roundtrip(&conn_a, &conn_b);

    assert_eq!(a_after.len(), 1, "A should have exactly one Asthma chip");
    assert_eq!(b_after.len(), 1, "B should have exactly one Asthma chip");
    assert_eq!(a_after[0].text, "Asthma");
    assert_eq!(b_after[0].text, "Asthma");
    // The newer timestamp (t=1) should have won on both sides.
    assert_eq!(a_after[0].updated_at, now(1));
    assert_eq!(b_after[0].updated_at, now(1));
}

#[test]
fn tombstone_propagates_across_roundtrip() {
    // Both start with "COPD". A removes it (tombstone at t=10). After a
    // round-trip both sides must have zero active chips.
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    ConditionChipsRepo::add(&conn_a, "COPD", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "COPD", &now(0)).unwrap();

    // A soft-deletes COPD at t=10.
    ConditionChipsRepo::remove_by_text(&conn_a, "COPD", &now(10)).unwrap();

    let (a_after, b_after) = sync_roundtrip(&conn_a, &conn_b);

    assert!(a_after.is_empty(), "A should have no active chips");
    assert!(b_after.is_empty(), "B should have no active chips (tombstone propagated)");

    // The tombstone itself should be present in list_all on both sides.
    let a_all = ConditionChipsRepo::list_all(&conn_a).unwrap();
    let b_all = ConditionChipsRepo::list_all(&conn_b).unwrap();
    assert_eq!(a_all.len(), 1, "A should retain the tombstone");
    assert_eq!(b_all.len(), 1, "B should retain the tombstone");
    assert!(a_all[0].deleted_at.is_some(), "A row must be a tombstone");
    assert!(b_all[0].deleted_at.is_some(), "B row must be a tombstone");
}

#[test]
fn reorder_propagates_across_roundtrip() {
    // Both start with Alpha, Beta. Sync the initial state so B has both.
    // A then reorders to [Beta, Alpha]. After a round-trip both sides must
    // display [Beta, Alpha].
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    ConditionChipsRepo::add(&conn_a, "Alpha", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_a, "Beta", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "Alpha", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "Beta", &now(0)).unwrap();

    // Sync initial state so both converge on the same ordering.
    let _ = sync_roundtrip(&conn_a, &conn_b);

    // A reorders to [Beta, Alpha] at t=10.
    let beta_id = deterministic_id("Beta");
    let alpha_id = deterministic_id("Alpha");
    ConditionChipsRepo::reorder(&conn_a, &[beta_id, alpha_id], &now(10)).unwrap();

    let (a_after, b_after) = sync_roundtrip(&conn_a, &conn_b);

    assert_eq!(
        texts(&a_after),
        vec!["Beta".to_string(), "Alpha".to_string()],
        "A should display [Beta, Alpha]"
    );
    assert_eq!(
        texts(&b_after),
        vec!["Beta".to_string(), "Alpha".to_string()],
        "B should display [Beta, Alpha] after reorder propagates"
    );
}
