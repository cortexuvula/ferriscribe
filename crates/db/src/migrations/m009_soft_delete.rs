use rusqlite::Connection;

use crate::DbResult;

/// Add `deleted_at` column for soft-delete / undo support.
///
/// When a recording is deleted from the UI, it's marked `deleted_at = now()`
/// instead of being hard-deleted. The frontend shows an "Undo" toast for 8
/// seconds. If the user clicks Undo, `deleted_at` is cleared. A future purge
/// sweeper will permanently delete recordings older than 30 days.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN deleted_at TEXT;
         CREATE INDEX IF NOT EXISTS idx_recordings_deleted_at ON recordings(deleted_at);",
    )?;
    Ok(())
}
