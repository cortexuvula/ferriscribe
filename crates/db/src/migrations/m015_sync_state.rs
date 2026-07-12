use rusqlite::Connection;

use crate::DbResult;

/// Create `sync_state` table for content sync cursor persistence.
///
/// Stores the client's last-seen server `updated_at` cursor so delta
/// pulls resume from the right position after restarts.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (
            key   TEXT PRIMARY KEY,
            value TEXT
        );
        INSERT OR IGNORE INTO sync_state (key, value) VALUES
            ('content_sync_cursor', NULL),
            ('content_sync_last_pull', NULL),
            ('pending_audio_uploads', '[]');",
    )?;
    Ok(())
}
