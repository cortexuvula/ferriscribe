use rusqlite::Connection;

use crate::DbResult;

/// Add `updated_at` column to `recordings`.
///
/// Tracks the last modification time of any field on a recording row.
/// Drives delta filtering for content sync — the server answers
/// "give me everything where `updated_at > cursor`". Existing rows are
/// backfilled to `created_at` (they have never been "modified").
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN updated_at TEXT;
         UPDATE recordings SET updated_at = created_at WHERE updated_at IS NULL;",
    )?;
    Ok(())
}
