//! m018: purge ledger. Records the ids of recordings permanently purged by
//! the office server's tombstone sweeper so `merge_incoming` can refuse
//! re-insertion of a stale live copy pushed by a machine that missed the
//! deletion. Id-only — no PHI.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS purged_recordings (
            id TEXT PRIMARY KEY,
            purged_at TEXT NOT NULL
        )",
        [],
    )?;
    Ok(())
}
