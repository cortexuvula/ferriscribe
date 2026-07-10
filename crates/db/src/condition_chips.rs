//! CRUD + sync-merge operations for the `condition_chips` table.
//!
//! A condition chip is a practice-wide quick-add preset shown under "Known
//! conditions" (e.g. "Hypertension"). Each chip has a deterministic ID derived
//! from its normalized text, enabling per-item last-write-wins merge across
//! machines.
//!
//! Deletion is soft: a tombstone timestamp is written to `deleted_at` and the
//! row is retained so the deletion can propagate to other machines during sync.
//! Tombstones older than a cutoff can eventually be pruned via
//! [`ConditionChipsRepo::prune_tombstones`].

use rusqlite::{Connection, Row, params};

use medical_core::types::condition_chip::{ConditionChip, deterministic_id};

use crate::{DbError, DbResult};

/// Repository for the `condition_chips` table.
///
/// All methods are associated functions taking a `&Connection` as the first
/// argument, following the same pattern as [`crate::RecordingsRepo`] and
/// [`crate::UserDictionaryRepo`].
pub struct ConditionChipsRepo;

impl ConditionChipsRepo {
    /// List active (non-deleted) chips, ordered by sort_order then text
    /// case-insensitively.
    pub fn list_active(conn: &Connection) -> DbResult<Vec<ConditionChip>> {
        let mut stmt = conn.prepare(
            "SELECT id, text, updated_at, deleted_at, sort_order
             FROM condition_chips
             WHERE deleted_at IS NULL
             ORDER BY sort_order, LOWER(text)",
        )?;
        let chips = stmt
            .query_map([], Self::row_to_chip)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(chips)
    }

    /// List all chips including tombstones (for sync).
    pub fn list_all(conn: &Connection) -> DbResult<Vec<ConditionChip>> {
        let mut stmt = conn.prepare(
            "SELECT id, text, updated_at, deleted_at, sort_order
             FROM condition_chips
             ORDER BY LOWER(text)",
        )?;
        let chips = stmt
            .query_map([], Self::row_to_chip)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(chips)
    }

    /// Insert or replace a chip by id (`ON CONFLICT DO UPDATE`).
    ///
    /// Used both for direct writes and as the primitive underlying the merge.
    pub fn upsert(conn: &Connection, chip: &ConditionChip) -> DbResult<()> {
        conn.execute(
            "INSERT INTO condition_chips (id, text, updated_at, deleted_at, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 text = excluded.text,
                 updated_at = excluded.updated_at,
                 deleted_at = excluded.deleted_at,
                 sort_order = excluded.sort_order",
            params![
                chip.id,
                chip.text,
                chip.updated_at,
                chip.deleted_at,
                chip.sort_order
            ],
        )?;
        Ok(())
    }

    /// Soft-delete a chip by id: set `deleted_at` and `updated_at` to `now_iso`.
    pub fn soft_delete(conn: &Connection, id: &str, now_iso: &str) -> DbResult<()> {
        conn.execute(
            "UPDATE condition_chips
             SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![now_iso, id],
        )?;
        Ok(())
    }

    /// Merge a batch of remote chips into the local store using
    /// last-write-wins semantics.
    ///
    /// # Algorithm
    ///
    /// For each remote chip `R`:
    /// - If no local chip with the same id exists → **insert** `R` as-is
    ///   (it is new — an addition or a tombstone from the other side).
    /// - If `R.updated_at > local.updated_at` → **replace** local with `R`.
    /// - If `R.updated_at < local.updated_at` → local wins, **do nothing**.
    /// - If timestamps are equal (tie) → the **tombstone wins**: if `R` is a
    ///   tombstone (`deleted_at` is `Some`) replace local, otherwise keep local.
    ///
    /// The tie-break rule (deleted wins on exact timestamp equality) is
    /// conservative — it avoids ghost reappearance of a condition that one side
    /// deleted.
    ///
    /// Timestamps are ISO 8601 UTC strings, which compare chronologically under
    /// lexicographic ordering as long as the format/timezone is consistent.
    ///
    /// Returns the active chip list after merging.
    pub fn merge_incoming(
        conn: &Connection,
        remote_chips: &[ConditionChip],
    ) -> DbResult<Vec<ConditionChip>> {
        // Load all local chips once and index by id for O(1) lookup.
        let local_all = Self::list_all(conn)?;
        let local_map: std::collections::HashMap<&str, &ConditionChip> =
            local_all.iter().map(|c| (c.id.as_str(), c)).collect();

        // Wrap the upsert loop in a transaction so a mid-merge failure
        // rolls back all prior writes — otherwise a partial merge leaves
        // the local store inconsistent with the remote side.
        conn.execute_batch("BEGIN")?;
        let result = (|| {
            for remote in remote_chips {
                match local_map.get(remote.id.as_str()) {
                    None => {
                        // New chip — insert as-is (addition or tombstone).
                        Self::upsert(conn, remote)?;
                    }
                    Some(local) => match remote.updated_at.cmp(&local.updated_at) {
                        std::cmp::Ordering::Greater => {
                            // Remote is newer — remote wins.
                            Self::upsert(conn, remote)?;
                        }
                        std::cmp::Ordering::Less => {
                            // Local is newer — local wins, do nothing.
                        }
                        std::cmp::Ordering::Equal => {
                            // Tie — tombstone wins to avoid ghost reappearance.
                            if remote.deleted_at.is_some() {
                                Self::upsert(conn, remote)?;
                            }
                        }
                    },
                }
            }
            Ok::<(), DbError>(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }

        Self::list_active(conn)
    }

    /// Permanently delete tombstones whose `deleted_at` is older than
    /// `cutoff_iso`. Returns the number of rows removed.
    ///
    /// Active chips are never touched.
    pub fn prune_tombstones(conn: &Connection, cutoff_iso: &str) -> DbResult<usize> {
        let removed = conn.execute(
            "DELETE FROM condition_chips
             WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            params![cutoff_iso],
        )?;
        Ok(removed)
    }

    /// Add a new chip with the given text. The id is derived deterministically
    /// from the normalized text. Returns the active chip list afterwards.
    ///
    /// The MAX(sort_order) read and the subsequent insert run inside a
    /// transaction so a concurrent writer can't slip in between them and
    /// steal the same sort_order slot.
    pub fn add(conn: &Connection, text: &str, now_iso: &str) -> DbResult<Vec<ConditionChip>> {
        conn.execute_batch("BEGIN")?;
        let result = (|| {
            let max_order: i32 = conn.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM condition_chips WHERE deleted_at IS NULL",
                [],
                |row| row.get(0),
            )?;
            let chip = ConditionChip {
                id: deterministic_id(text),
                text: text.trim().to_string(),
                updated_at: now_iso.to_string(),
                deleted_at: None,
                sort_order: max_order + 1,
            };
            Self::upsert(conn, &chip)?;
            Ok::<(), DbError>(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Self::list_active(conn)
    }

    /// Soft-delete the chip whose text matches (case-insensitively, via the
    /// deterministic id). Returns the active chip list afterwards.
    pub fn remove_by_text(
        conn: &Connection,
        text: &str,
        now_iso: &str,
    ) -> DbResult<Vec<ConditionChip>> {
        let id = deterministic_id(text);
        Self::soft_delete(conn, &id, now_iso)?;
        Self::list_active(conn)
    }

    /// Reorder chips to match the given ordered list of IDs.
    /// Sets sort_order = index for each, bumps updated_at on all listed rows.
    /// Chips not in the list keep their existing sort_order.
    /// Returns the active list in the new order.
    ///
    /// The UPDATE loop runs inside a transaction so a mid-loop failure
    /// (e.g. DB lock) rolls back all prior updates — without this, a partial
    /// failure would leave chips with inconsistent sort_order values.
    pub fn reorder(
        conn: &Connection,
        ordered_ids: &[String],
        now_iso: &str,
    ) -> DbResult<Vec<ConditionChip>> {
        conn.execute_batch("BEGIN")?;
        let result = (|| {
            for (index, id) in ordered_ids.iter().enumerate() {
                conn.execute(
                    "UPDATE condition_chips
                     SET sort_order = ?1, updated_at = ?2
                     WHERE id = ?3",
                    params![index as i32, now_iso, id],
                )?;
            }
            Ok::<(), DbError>(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Self::list_active(conn)
    }

    /// Map a `rusqlite::Row` (columns: id, text, updated_at, deleted_at,
    /// sort_order) to a [`ConditionChip`].
    fn row_to_chip(row: &Row) -> rusqlite::Result<ConditionChip> {
        Ok(ConditionChip {
            id: row.get(0)?,
            text: row.get(1)?,
            updated_at: row.get(2)?,
            deleted_at: row.get(3)?,
            sort_order: row.get(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    /// Create an in-memory database with the `condition_chips` table.
    /// The table migration is Task 3, so tests create it manually here.
    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory");
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT,
                sort_order INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create condition_chips table");
        conn
    }

    /// ISO 8601 timestamp offset from a fixed base epoch.
    fn now(offset_secs: i64) -> String {
        let base = chrono::DateTime::parse_from_rfc3339("2026-07-03T10:00:00Z").unwrap();
        let t = base + chrono::Duration::seconds(offset_secs);
        t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    /// Build a chip with a deterministic id for the given text.
    fn chip(text: &str, updated_offset: i64, deleted: bool) -> ConditionChip {
        ConditionChip {
            id: deterministic_id(text),
            text: text.to_string(),
            updated_at: now(updated_offset),
            deleted_at: if deleted {
                Some(now(updated_offset))
            } else {
                None
            },
            sort_order: 0,
        }
    }

    #[test]
    fn merge_inserts_new_remote_chip() {
        let conn = fresh();
        let remote = vec![chip("Hypertension", 0, false)];

        let result = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("merge");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "Hypertension");
        assert!(result[0].deleted_at.is_none());
    }

    #[test]
    fn merge_remote_newer_wins() {
        let conn = fresh();
        // Local chip at t=0.
        ConditionChipsRepo::upsert(&conn, &chip("Hypertension", 0, false)).expect("upsert local");
        // Remote chip at t=300 — should win.
        let remote = vec![chip("Hypertension", 300, false)];

        let result = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("merge");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].updated_at, now(300));
    }

    #[test]
    fn merge_local_newer_wins() {
        let conn = fresh();
        // Local chip at t=300.
        ConditionChipsRepo::upsert(&conn, &chip("Hypertension", 300, false)).expect("upsert local");
        // Remote chip at t=0 — local should win.
        let remote = vec![chip("Hypertension", 0, false)];

        let result = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("merge");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].updated_at, now(300));
    }

    #[test]
    fn merge_tombstone_wins_over_older_active() {
        let conn = fresh();
        // Local active chip at t=0.
        ConditionChipsRepo::upsert(&conn, &chip("Hypertension", 0, false)).expect("upsert local");
        // Remote tombstone at t=600 — newer, so tombstone wins.
        let remote = vec![chip("Hypertension", 600, true)];

        let result = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("merge");

        assert!(
            result.is_empty(),
            "active list should be empty after tombstone merge"
        );
    }

    #[test]
    fn merge_re_add_after_tombstone() {
        let conn = fresh();
        // Local tombstone at t=600.
        ConditionChipsRepo::upsert(&conn, &chip("Hypertension", 600, true))
            .expect("upsert tombstone");
        // Remote active at t=1200 — newer, so the chip is resurrected.
        let remote = vec![chip("Hypertension", 1200, false)];

        let result = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("merge");

        assert_eq!(result.len(), 1, "chip should be resurrected");
        assert!(result[0].deleted_at.is_none());
    }

    #[test]
    fn merge_tie_deleted_wins() {
        let conn = fresh();
        // Local active chip at t=500.
        ConditionChipsRepo::upsert(&conn, &chip("Hypertension", 500, false)).expect("upsert local");
        // Remote tombstone at the SAME t=500 — tie, tombstone wins.
        let remote = vec![chip("Hypertension", 500, true)];

        let result = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("merge");

        assert!(result.is_empty(), "on tie the tombstone should win");
    }

    #[test]
    fn merge_is_idempotent() {
        let conn = fresh();
        let remote = vec![
            chip("Hypertension", 100, false),
            chip("Diabetes", 200, false),
        ];

        let first = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("first merge");
        let second = ConditionChipsRepo::merge_incoming(&conn, &remote).expect("second merge");

        assert_eq!(
            first, second,
            "merging the same list twice must yield the same result"
        );
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn prune_tombstones_removes_old_only() {
        let conn = fresh();
        // Old tombstone at t=0.
        ConditionChipsRepo::upsert(&conn, &chip("Old Condition", 0, true))
            .expect("upsert old tombstone");
        // Recent tombstone at t=900.
        ConditionChipsRepo::upsert(&conn, &chip("Recent Condition", 900, true))
            .expect("upsert recent tombstone");
        // Active chip — must be untouched.
        ConditionChipsRepo::upsert(&conn, &chip("Active Condition", 300, false))
            .expect("upsert active");

        // Prune tombstones older than t=500.
        let removed = ConditionChipsRepo::prune_tombstones(&conn, &now(500)).expect("prune");

        assert_eq!(removed, 1, "only the old tombstone should be pruned");

        let all = ConditionChipsRepo::list_all(&conn).expect("list_all");
        assert_eq!(all.len(), 2, "recent tombstone + active chip should remain");
        let active = ConditionChipsRepo::list_active(&conn).expect("list_active");
        assert_eq!(active.len(), 1, "active chip should still be present");
        assert_eq!(active[0].text, "Active Condition");
    }

    #[test]
    fn add_and_remove_by_text() {
        let conn = fresh();

        // Add returns a single active chip.
        let after_add = ConditionChipsRepo::add(&conn, "Hypertension", &now(100)).expect("add");
        assert_eq!(after_add.len(), 1);
        assert_eq!(after_add[0].text, "Hypertension");

        // Remove returns an empty active list.
        let after_remove =
            ConditionChipsRepo::remove_by_text(&conn, "Hypertension", &now(200)).expect("remove");
        assert!(
            after_remove.is_empty(),
            "active list should be empty after remove"
        );

        // The tombstone should still exist in list_all.
        let all = ConditionChipsRepo::list_all(&conn).expect("list_all");
        assert_eq!(all.len(), 1, "tombstone should be retained");
        assert!(all[0].deleted_at.is_some(), "chip should be a tombstone");
    }

    #[test]
    fn reorder_updates_sort_order() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
        ConditionChipsRepo::add(&conn, "Beta", &now(1)).unwrap();
        ConditionChipsRepo::add(&conn, "Gamma", &now(2)).unwrap();

        // Reorder: Gamma first, Alpha second, Beta third.
        let gamma_id = deterministic_id("Gamma");
        let alpha_id = deterministic_id("Alpha");
        let beta_id = deterministic_id("Beta");
        let reordered =
            ConditionChipsRepo::reorder(&conn, &[gamma_id, alpha_id, beta_id], &now(100)).unwrap();
        assert_eq!(
            reordered
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Gamma", "Alpha", "Beta"],
            "list_active should reflect new sort_order"
        );
    }

    #[test]
    fn reorder_bumps_updated_at() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
        ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();

        let alpha_id = deterministic_id("Alpha");
        let beta_id = deterministic_id("Beta");
        ConditionChipsRepo::reorder(&conn, &[beta_id, alpha_id], &now(100)).unwrap();

        let all = ConditionChipsRepo::list_all(&conn).unwrap();
        for chip in &all {
            assert_eq!(
                chip.updated_at,
                now(100),
                "updated_at should be bumped by reorder"
            );
        }
    }

    #[test]
    fn reorder_partial_list_keeps_unlisted_positions() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
        ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();
        ConditionChipsRepo::add(&conn, "Gamma", &now(0)).unwrap();

        let alpha_id = deterministic_id("Alpha");
        let beta_id = deterministic_id("Beta");
        let reordered =
            ConditionChipsRepo::reorder(&conn, &[beta_id, alpha_id], &now(100)).unwrap();

        assert_eq!(
            reordered
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Beta", "Alpha", "Gamma"]
        );
    }

    #[test]
    fn merge_propagates_order() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
        ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();

        let remote = vec![
            ConditionChip {
                id: deterministic_id("Beta"),
                text: "Beta".into(),
                updated_at: now(100),
                deleted_at: None,
                sort_order: 0,
            },
            ConditionChip {
                id: deterministic_id("Alpha"),
                text: "Alpha".into(),
                updated_at: now(100),
                deleted_at: None,
                sort_order: 1,
            },
        ];
        let merged = ConditionChipsRepo::merge_incoming(&conn, &remote).unwrap();

        assert_eq!(
            merged.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            vec!["Beta", "Alpha"],
            "merge should propagate remote's ordering"
        );
    }

    #[test]
    fn add_appends_to_end_of_sorted_list() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
        ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();
        let after_gamma = ConditionChipsRepo::add(&conn, "Gamma", &now(0)).unwrap();

        assert_eq!(after_gamma.last().unwrap().text, "Gamma");
    }
}
