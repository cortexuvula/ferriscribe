use rusqlite::Connection;

use crate::DbResult;

/// Create `recording_field_revisions` table for per-field LWW sync.
///
/// Each syncable text field has its own `updated_at` timestamp and
/// `origin_device` (machine_id of the editor). During merge, incoming
/// revisions are compared field-by-field to resolve conflicts.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS recording_field_revisions (
            recording_id  TEXT NOT NULL,
            field         TEXT NOT NULL,
            updated_at    TEXT NOT NULL,
            origin_device TEXT,
            PRIMARY KEY (recording_id, field),
            FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_revisions_updated_at
            ON recording_field_revisions(updated_at);",
    )?;
    Ok(())
}
