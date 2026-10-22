use rusqlite::Connection;

use crate::DbResult;

/// Local-only undo clock for received tombstones. NULL falls back to deleted_at
/// for legacy rows and locally initiated deletions. Never transmitted over sync:
/// deleted_at and updated_at retain the original peer's LWW timestamps.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch("ALTER TABLE recordings ADD COLUMN local_deleted_at TEXT;")?;
    Ok(())
}
