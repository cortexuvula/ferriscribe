//! Migration 016: add sync metadata to the `user_dictionary` table.
//!
//! Adds three columns used by the bidirectional dictionary sync feature:
//! - `sync_id TEXT` — deterministic UUID v5 id (the per-item merge key)
//! - `updated_at TEXT` — ISO 8601 timestamp (the last-write-wins clock)
//! - `deleted_at TEXT` — tombstone timestamp (`NULL` means active)
//!
//! ## Why a table rebuild instead of `ALTER TABLE ADD COLUMN`
//!
//! The original m005 schema already has an `id INTEGER PRIMARY KEY` column,
//! so a literal `ALTER TABLE user_dictionary ADD COLUMN id TEXT` would fail
//! with a duplicate-column error. SQLite additionally forbids adding a column
//! with a `UNIQUE`/`PRIMARY KEY` constraint via `ALTER TABLE`. The standard
//! SQLite workaround for both is the table-rebuild idiom: create the new
//! table under a temp name, copy rows over, drop the old table, and rename.
//!
//! Existing rows are backfilled with deterministic `sync_id`s (derived from
//! the normalized word) and `updated_at` copied from `added_at`, so they
//! participate in sync immediately without resurfacing as conflicts.
//!
//! The lowercase-word unique index from m005 is recreated on the rebuilt
//! table but widened to allow duplicate (LOWER(word), NULL-deleted_at) only
//! for tombstones — in practice the sync layer upserts by `sync_id`, so the
//! case-insensitive index stays as a defence-in-depth uniqueness guard on
//! active rows. To permit a word to be re-added after deletion (tombstone →
//! active resurrection), the unique constraint applies only to rows where
//! `deleted_at IS NULL` (a partial index).

use rusqlite::Connection;

use crate::DbResult;

/// Rebuild `user_dictionary` with sync metadata columns and backfill existing
/// rows with deterministic `sync_id`s.
pub fn up(conn: &Connection) -> DbResult<()> {
    // Build the new shape under a temp name. `sync_id` is the merge key (NOT
    // the rowid-style PRIMARY KEY — we keep an INTEGER rowid alias for
    // convenience but the sync layer keys everything on sync_id).
    conn.execute_batch(
        "CREATE TABLE user_dictionary_new (
            sync_id     TEXT PRIMARY KEY,
            word        TEXT NOT NULL,
            added_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL,
            deleted_at  TEXT
        );",
    )?;

    // Backfill: copy existing rows, deriving sync_id deterministically and
    // seeding updated_at from added_at. Backfill runs in Rust so we can call
    // the shared deterministic_id helper (kept in sync with the type module).
    backfill_existing_rows(conn)?;

    // Swap the rebuilt table in and recreate the case-insensitive uniqueness
    // guard on ACTIVE rows only (partial index: a tombstoned word can later
    // be re-added/resurrected under the same sync_id without colliding).
    conn.execute_batch(
        "DROP TABLE user_dictionary;
         ALTER TABLE user_dictionary_new RENAME TO user_dictionary;

         CREATE UNIQUE INDEX IF NOT EXISTS idx_user_dictionary_word_nocase
             ON user_dictionary (LOWER(word))
             WHERE deleted_at IS NULL;",
    )?;

    Ok(())
}

/// Read every existing `user_dictionary` row and re-insert it into
/// `user_dictionary_new` with a deterministic `sync_id` (UUID v5 of the
/// normalized word) and `updated_at` copied from `added_at`.
///
/// Rows whose word normalizes to the same id (e.g. "Lisinopril" and
/// "lisinopril", which the old case-insensitive index prevented but could not
/// guarantee under direct inserts) are de-duplicated by `INSERT OR IGNORE`,
/// keeping the first-seen row.
fn backfill_existing_rows(conn: &Connection) -> DbResult<()> {
    use medical_core::types::user_dict_entry::deterministic_id;

    let mut stmt = conn.prepare("SELECT word, added_at FROM user_dictionary")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for (word, added_at) in rows {
        let sync_id = deterministic_id(&word);
        // INSERT OR IGNORE: the PRIMARY KEY (sync_id) dedupes any case-variant
        // collisions. updated_at seeds from added_at so pre-existing words
        // take part in LWW merge without artificially recent timestamps.
        let _ = conn.execute(
            "INSERT OR IGNORE INTO user_dictionary_new
                 (sync_id, word, added_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?3, NULL)",
            rusqlite::params![sync_id, word, added_at],
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).expect("migrate");
        conn
    }

    #[test]
    fn fresh_table_has_sync_columns() {
        let conn = fresh();
        // All three sync columns must exist and be usable.
        let (sync_id, updated_at, deleted_at): (String, String, Option<String>) = conn
            .query_row(
                "SELECT sync_id, updated_at, deleted_at FROM user_dictionary LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap_or_else(|_| {
                // Empty table is fine — just verify the columns are queryable.
                conn.execute(
                    "INSERT INTO user_dictionary (sync_id, word, added_at, updated_at, deleted_at)
                     VALUES ('test-id', 'word', '2026-01-01', '2026-01-01', NULL)",
                    [],
                )
                .unwrap();
                conn.query_row(
                    "SELECT sync_id, updated_at, deleted_at FROM user_dictionary LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
            });
        assert_eq!(sync_id, "test-id".to_string());
        assert_eq!(updated_at, "2026-01-01");
        assert!(deleted_at.is_none());
    }

    #[test]
    fn backfill_preserves_existing_words() {
        // Create a DB at an OLD schema (pre-m016), insert words, then apply
        // m016 specifically and verify backfill. The full MigrationEngine path
        // is covered by `fresh()` above; this test isolates the m016 upgrade
        // from a realistic pre-m016 starting point.
        let conn = Connection::open_in_memory().unwrap();
        // Create the schema_version bookkeeping table (normally created by
        // MigrationEngine::migrate) so we can record applied versions.
        conn.execute_batch(
            "CREATE TABLE schema_version (
                version INTEGER NOT NULL,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        // Apply only up to m015 to simulate an upgrade.
        for m in crate::migrations::all_migrations() {
            if m.version > 15 {
                continue;
            }
            conn.execute_batch("BEGIN").unwrap();
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
                rusqlite::params![m.version, m.name],
            )
            .unwrap();
            conn.execute_batch("COMMIT").unwrap();
        }
        conn.execute(
            "INSERT INTO user_dictionary (word, added_at) VALUES ('Lisinopril', '2026-06-01')",
            [],
        )
        .unwrap();

        // Now apply m016 specifically.
        conn.execute_batch("BEGIN").unwrap();
        up(&conn).expect("m016 up");
        conn.execute(
            "INSERT INTO schema_version (version, name) VALUES (16, 'user_dictionary_sync')",
            [],
        )
        .unwrap();
        conn.execute_batch("COMMIT").unwrap();

        // The word survived, got a deterministic sync_id, and updated_at = added_at.
        let (word, updated_at, deleted_at): (String, String, Option<String>) = conn
            .query_row(
                "SELECT word, updated_at, deleted_at FROM user_dictionary",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(word, "Lisinopril");
        assert_eq!(updated_at, "2026-06-01");
        assert!(deleted_at.is_none());

        let sync_id: String = conn
            .query_row("SELECT sync_id FROM user_dictionary", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            sync_id,
            medical_core::types::user_dict_entry::deterministic_id("Lisinopril")
        );
    }

    #[test]
    fn fresh_full_migration_is_idempotent_via_engine() {
        // The engine records m016 as applied; running again must not re-apply.
        let conn = fresh();
        let again = MigrationEngine::migrate(&conn).expect("migrate again");
        assert_eq!(again, 0, "no migrations should be applied the second time");
    }
}
