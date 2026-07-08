use rusqlite::Connection;

use crate::DbResult;

/// Add `encryption_pending` column to `recordings`.
///
/// Tracks whether a recording's WAV file is still being encrypted by a
/// background task spawned from `stop_recording`. A startup sweep reads
/// rows left at `1` (the app crashed or was hard-quit mid-encryption) and
/// encrypts them so no plaintext audio remains at rest.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN encryption_pending INTEGER NOT NULL DEFAULT 0;",
    )?;
    Ok(())
}
