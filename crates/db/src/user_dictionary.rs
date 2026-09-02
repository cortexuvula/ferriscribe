//! Per-user dictionary of accepted spellings.
//!
//! Backs the in-app spellchecker: words on this list are never flagged as
//! misspelled. Storage is case-insensitive via a `UNIQUE INDEX` on
//! `LOWER(word)` (active rows only), so "Lisinopril" and "lisinopril" are the
//! same entry.
//!
//! # Sync
//!
//! The table carries sync metadata (`sync_id`, `updated_at`, `deleted_at`)
//! added by migration m016, enabling two-way last-write-wins merge across
//! paired machines — mirroring `condition_chips`. Deletion is soft
//! (tombstoned) so a removal propagates to other machines during sync instead
//! of ghost-resurfacing. See [`UserDictionaryRepo::merge_incoming`].

use medical_core::types::user_dict_entry::{UserDictEntry, deterministic_id};
use rusqlite::{Connection, Row, params};

use crate::DbResult;

pub struct UserDictionaryRepo;

impl UserDictionaryRepo {
    /// List all active (non-tombstoned) words, sorted case-insensitively.
    ///
    /// Legacy shape (`Vec<String>`) kept for the spellchecker load path and
    /// any callers that only want word values.
    pub fn list(conn: &Connection) -> DbResult<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT word FROM user_dictionary
             WHERE deleted_at IS NULL
             ORDER BY LOWER(word)",
        )?;
        let words = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(words)
    }

    /// List active (non-deleted) entries as `Vec<String>` of words.
    /// Excludes tombstones. (Alias of [`Self::list`], named to match the
    /// condition-chips sync surface.)
    pub fn list_active(conn: &Connection) -> DbResult<Vec<String>> {
        Self::list(conn)
    }

    /// List all entries including tombstones, as full [`UserDictEntry`] rows
    /// (for sync push/pull). Ordered by `LOWER(word)`.
    pub fn list_all(conn: &Connection) -> DbResult<Vec<UserDictEntry>> {
        let mut stmt = conn.prepare(
            "SELECT sync_id, word, updated_at, deleted_at
             FROM user_dictionary
             ORDER BY LOWER(word)",
        )?;
        let entries = stmt
            .query_map([], Self::row_to_entry)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    /// Insert or replace an entry by `sync_id` (`ON CONFLICT DO UPDATE`).
    ///
    /// Used both for direct writes and as the primitive underlying the merge.
    pub fn upsert(conn: &Connection, entry: &UserDictEntry) -> DbResult<()> {
        conn.execute(
            "INSERT INTO user_dictionary (sync_id, word, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(sync_id) DO UPDATE SET
                 word = excluded.word,
                 updated_at = excluded.updated_at,
                 deleted_at = excluded.deleted_at",
            params![entry.id, entry.word, entry.updated_at, entry.deleted_at],
        )?;
        Ok(())
    }

    /// Soft-delete an entry by `sync_id`: set `deleted_at` and `updated_at`
    /// to `now_iso`. Idempotent (a no-op if the row is already tombstoned,
    /// though `updated_at` is still bumped to record the touch).
    pub fn soft_delete(conn: &Connection, sync_id: &str, now_iso: &str) -> DbResult<()> {
        conn.execute(
            "UPDATE user_dictionary
             SET deleted_at = ?1, updated_at = ?1
             WHERE sync_id = ?2",
            params![now_iso, sync_id],
        )?;
        Ok(())
    }

    /// Merge a batch of remote entries into the local store using
    /// last-write-wins semantics — the exact same algorithm as
    /// [`crate::condition_chips::ConditionChipsRepo::merge_incoming`].
    ///
    /// # Algorithm
    ///
    /// For each remote entry `R`:
    /// - If no local entry with the same `sync_id` exists → **insert** `R`
    ///   as-is (it is new — an addition or a tombstone from the other side).
    /// - If `R.updated_at > local.updated_at` → **replace** local with `R`.
    /// - If `R.updated_at < local.updated_at` → local wins, **do nothing**.
    /// - If timestamps are equal (tie) → the **tombstone wins**: if `R` is a
    ///   tombstone (`deleted_at` is `Some`) replace local, otherwise keep local.
    ///
    /// The tie-break rule (deleted wins on exact timestamp equality) is
    /// conservative — it avoids ghost reappearance of a word one side deleted.
    ///
    /// Returns the active word list (`Vec<String>`) after merging.
    pub fn merge_incoming(conn: &Connection, remote: &[UserDictEntry]) -> DbResult<Vec<String>> {
        // Load all local entries once and index by sync_id for O(1) lookup.
        // The load runs INSIDE the transaction: two concurrent sync rounds
        // reading the same snapshot outside it could apply the older entry
        // on top of the newer one, and nothing re-delivers the loser.
        let tx = conn.unchecked_transaction()?;
        let local_all = Self::list_all(&tx)?;
        let local_map: std::collections::HashMap<&str, &UserDictEntry> =
            local_all.iter().map(|e| (e.id.as_str(), e)).collect();

        for remote_entry in remote {
            match local_map.get(remote_entry.id.as_str()) {
                None => {
                    // New entry — insert as-is (addition or tombstone).
                    Self::upsert(&tx, remote_entry)?;
                }
                Some(local) => match remote_entry.updated_at.cmp(&local.updated_at) {
                    std::cmp::Ordering::Greater => {
                        // Remote is newer — remote wins.
                        Self::upsert(&tx, remote_entry)?;
                    }
                    std::cmp::Ordering::Less => {
                        // Local is newer — local wins, do nothing.
                    }
                    std::cmp::Ordering::Equal => {
                        // Tie — tombstone wins to avoid ghost reappearance.
                        if remote_entry.deleted_at.is_some() {
                            Self::upsert(&tx, remote_entry)?;
                        }
                    }
                },
            }
        }
        tx.commit()?;

        Self::list_active(conn)
    }

    /// Permanently delete tombstones whose `deleted_at` is older than
    /// `cutoff_iso`. Returns the number of rows removed.
    ///
    /// Active entries are never touched.
    pub fn prune_tombstones(conn: &Connection, cutoff_iso: &str) -> DbResult<usize> {
        let removed = conn.execute(
            "DELETE FROM user_dictionary
             WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            params![cutoff_iso],
        )?;
        Ok(removed)
    }

    /// Add a word to the dictionary. Whitespace is trimmed; empty input is a
    /// no-op. The sync id is derived deterministically from the normalized
    /// word; `updated_at` is set to `now_iso`. Resurrects a previously
    /// tombstoned entry (clears `deleted_at`). Returns `true` if a new row
    /// was inserted or a tombstone was resurrected, `false` if the word was
    /// already active or input was empty.
    ///
    /// (Legacy signature kept for the existing server/client command surface;
    /// now writes through the sync columns so local writes participate in
    /// sync immediately.)
    pub fn add(conn: &Connection, word: &str, now_iso: &str) -> DbResult<bool> {
        let trimmed = word.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let entry = UserDictEntry {
            id: deterministic_id(trimmed),
            word: trimmed.to_string(),
            updated_at: now_iso.to_string(),
            deleted_at: None,
        };
        let rows_before = conn.query_row(
            "SELECT COUNT(*) FROM user_dictionary WHERE sync_id = ?1 AND deleted_at IS NULL",
            params![entry.id],
            |row| row.get::<_, i64>(0),
        )?;
        Self::upsert(conn, &entry)?;
        let rows_after = conn.query_row(
            "SELECT COUNT(*) FROM user_dictionary WHERE sync_id = ?1 AND deleted_at IS NULL",
            params![entry.id],
            |row| row.get::<_, i64>(0),
        )?;
        // Newly inserted or resurrected → counts went 0 → 1.
        Ok(rows_after > rows_before)
    }

    /// Remove (soft-delete) a word (case-insensitive match). Writes a
    /// tombstone so the deletion can propagate during sync. Returns `true` if
    /// an active row was tombstoned, `false` if no active matching word
    /// existed.
    ///
    /// (Legacy signature kept for the existing server/client command surface;
    /// now writes a tombstone through the sync columns.)
    pub fn remove(conn: &Connection, word: &str, now_iso: &str) -> DbResult<bool> {
        let sync_id = deterministic_id(word);
        let active_before = conn.query_row(
            "SELECT COUNT(*) FROM user_dictionary
             WHERE sync_id = ?1 AND deleted_at IS NULL",
            params![sync_id],
            |row| row.get::<_, i64>(0),
        )?;
        if active_before == 0 {
            return Ok(false);
        }
        Self::soft_delete(conn, &sync_id, now_iso)?;
        Ok(true)
    }

    /// Check whether a word is present and active (case-insensitive).
    pub fn contains(conn: &Connection, word: &str) -> DbResult<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM user_dictionary
             WHERE LOWER(word) = LOWER(?1) AND deleted_at IS NULL",
            params![word],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Map a `rusqlite::Row` (columns: sync_id, word, updated_at, deleted_at)
    /// to a [`UserDictEntry`].
    fn row_to_entry(row: &Row) -> rusqlite::Result<UserDictEntry> {
        Ok(UserDictEntry {
            id: row.get(0)?,
            word: row.get(1)?,
            updated_at: row.get(2)?,
            deleted_at: row.get(3)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).expect("migrate");
        conn
    }

    /// ISO 8601 timestamp offset from a fixed base epoch (matches the
    /// condition-chips test helper format).
    fn now(offset_secs: i64) -> String {
        let base = chrono::DateTime::parse_from_rfc3339("2026-07-03T10:00:00Z").unwrap();
        let t = base + chrono::Duration::seconds(offset_secs);
        t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    /// Build an entry with a deterministic id for the given word.
    fn entry(word: &str, updated_offset: i64, deleted: bool) -> UserDictEntry {
        UserDictEntry {
            id: deterministic_id(word),
            word: word.to_string(),
            updated_at: now(updated_offset),
            deleted_at: if deleted {
                Some(now(updated_offset))
            } else {
                None
            },
        }
    }

    #[test]
    fn add_then_list_returns_word() {
        let conn = fresh();
        assert!(UserDictionaryRepo::add(&conn, "atenolol", &now(0)).unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap(), vec!["atenolol"]);
    }

    #[test]
    fn add_is_idempotent_case_insensitive() {
        let conn = fresh();
        assert!(UserDictionaryRepo::add(&conn, "Lisinopril", &now(0)).unwrap());
        // Same word, different case — the row already exists & is active.
        assert!(!UserDictionaryRepo::add(&conn, "lisinopril", &now(1)).unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn add_resurrects_tombstone() {
        let conn = fresh();
        UserDictionaryRepo::add(&conn, "metformin", &now(0)).unwrap();
        UserDictionaryRepo::remove(&conn, "metformin", &now(10)).unwrap();
        // Re-add clears the tombstone.
        assert!(UserDictionaryRepo::add(&conn, "metformin", &now(20)).unwrap());
        assert!(UserDictionaryRepo::contains(&conn, "metformin").unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap(), vec!["metformin"]);
    }

    #[test]
    fn contains_is_case_insensitive() {
        let conn = fresh();
        UserDictionaryRepo::add(&conn, "metformin", &now(0)).unwrap();
        assert!(UserDictionaryRepo::contains(&conn, "METFORMIN").unwrap());
        assert!(UserDictionaryRepo::contains(&conn, "metformin").unwrap());
        assert!(!UserDictionaryRepo::contains(&conn, "unknown").unwrap());
    }

    #[test]
    fn remove_returns_false_when_absent() {
        let conn = fresh();
        assert!(!UserDictionaryRepo::remove(&conn, "ghost", &now(0)).unwrap());
    }

    #[test]
    fn remove_tombstones_and_keeps_row_for_sync() {
        let conn = fresh();
        UserDictionaryRepo::add(&conn, "metformin", &now(0)).unwrap();
        assert!(UserDictionaryRepo::remove(&conn, "metformin", &now(10)).unwrap());

        // Active list is empty, but the tombstone row remains for sync.
        assert!(UserDictionaryRepo::list(&conn).is_ok());
        assert!(UserDictionaryRepo::list(&conn).unwrap().is_empty());
        let all = UserDictionaryRepo::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].deleted_at.is_some());
        assert!(!UserDictionaryRepo::contains(&conn, "metformin").unwrap());
    }

    #[test]
    fn add_strips_whitespace_and_skips_empty() {
        let conn = fresh();
        assert!(!UserDictionaryRepo::add(&conn, "   ", &now(0)).unwrap());
        assert!(UserDictionaryRepo::add(&conn, "  word  ", &now(0)).unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap(), vec!["word"]);
    }

    // --- merge_incoming tests (mirror condition_chips coverage) ---

    #[test]
    fn merge_inserts_new_remote_entry() {
        let conn = fresh();
        let remote = vec![entry("Atenolol", 0, false)];

        let result = UserDictionaryRepo::merge_incoming(&conn, &remote).expect("merge");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "Atenolol");
    }

    #[test]
    fn merge_remote_newer_wins() {
        let conn = fresh();
        UserDictionaryRepo::upsert(&conn, &entry("Atenolol", 0, false)).expect("upsert local");
        // Remote tombstone at t=300 — newer, so tombstone wins.
        let remote = vec![entry("Atenolol", 300, true)];

        let result = UserDictionaryRepo::merge_incoming(&conn, &remote).expect("merge");

        assert!(
            result.is_empty(),
            "active list should be empty after tombstone merge"
        );
    }

    #[test]
    fn merge_local_newer_wins() {
        let conn = fresh();
        UserDictionaryRepo::upsert(&conn, &entry("Atenolol", 300, false)).expect("upsert local");
        // Remote tombstone at t=0 — older, so local active wins.
        let remote = vec![entry("Atenolol", 0, true)];

        let result = UserDictionaryRepo::merge_incoming(&conn, &remote).expect("merge");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "Atenolol");
    }

    #[test]
    fn merge_re_add_after_tombstone() {
        let conn = fresh();
        // Local tombstone at t=600.
        UserDictionaryRepo::upsert(&conn, &entry("Atenolol", 600, true)).expect("upsert tombstone");
        // Remote active at t=1200 — newer, so the word is resurrected.
        let remote = vec![entry("Atenolol", 1200, false)];

        let result = UserDictionaryRepo::merge_incoming(&conn, &remote).expect("merge");

        assert_eq!(result.len(), 1, "word should be resurrected");
        assert_eq!(result[0], "Atenolol");
    }

    #[test]
    fn merge_tie_deleted_wins() {
        let conn = fresh();
        // Local active word at t=500.
        UserDictionaryRepo::upsert(&conn, &entry("Atenolol", 500, false)).expect("upsert local");
        // Remote tombstone at the SAME t=500 — tie, tombstone wins.
        let remote = vec![entry("Atenolol", 500, true)];

        let result = UserDictionaryRepo::merge_incoming(&conn, &remote).expect("merge");

        assert!(result.is_empty(), "on tie the tombstone should win");
    }

    #[test]
    fn merge_is_idempotent() {
        let conn = fresh();
        let remote = vec![
            entry("Atenolol", 100, false),
            entry("Lisinopril", 200, false),
        ];

        let first = UserDictionaryRepo::merge_incoming(&conn, &remote).expect("first merge");
        let second = UserDictionaryRepo::merge_incoming(&conn, &remote).expect("second merge");

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
        UserDictionaryRepo::upsert(&conn, &entry("oldword", 0, true))
            .expect("upsert old tombstone");
        // Recent tombstone at t=900.
        UserDictionaryRepo::upsert(&conn, &entry("recentword", 900, true))
            .expect("upsert recent tombstone");
        // Active word — must be untouched.
        UserDictionaryRepo::upsert(&conn, &entry("activeword", 300, false)).expect("upsert active");

        let removed = UserDictionaryRepo::prune_tombstones(&conn, &now(500)).expect("prune");

        assert_eq!(removed, 1, "only the old tombstone should be pruned");
        let all = UserDictionaryRepo::list_all(&conn).expect("list_all");
        assert_eq!(all.len(), 2, "recent tombstone + active word should remain");
        let active = UserDictionaryRepo::list_active(&conn).expect("list_active");
        assert_eq!(active.len(), 1, "active word should still be present");
        assert_eq!(active[0], "activeword");
    }
}
