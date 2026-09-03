//! m019: expression index for the content-sync delta query. `changed_since`
//! filters and sorts on `julianday(updated_at)` (the julianday call is the
//! fix for mixed-format timestamps), which makes a plain index on
//! `updated_at` unusable — every delta pull full-scanned and sorted the
//! whole `recordings` table on both server and clients. This index makes
//! the hot sync path O(changed rows) again.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_recordings_updated_julianday
            ON recordings(julianday(updated_at))",
        [],
    )?;
    Ok(())
}
