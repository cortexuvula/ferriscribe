use rusqlite::Connection;

use crate::DbResult;

/// Add `sort_order` column for user-defined chip ordering + drag-and-drop.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE condition_chips ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;",
    )?;
    Ok(())
}
