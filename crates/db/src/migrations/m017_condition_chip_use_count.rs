use rusqlite::Connection;

use crate::DbResult;

/// Add `use_count` column for frequency-based chip ordering (most-used first).
///
/// Existing chips default to 0; ties break to `LOWER(text)` ascending, so the
/// tray only reorders as chips are actually used. See `ConditionChipsRepo`.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE condition_chips ADD COLUMN use_count INTEGER NOT NULL DEFAULT 0;",
    )?;
    Ok(())
}
