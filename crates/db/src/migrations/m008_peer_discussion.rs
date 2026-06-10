//! Add `peer_discussion` column to the `recordings` table.
//!
//! The column stores the AI-generated peer-to-peer discussion note text.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN peer_discussion TEXT;",
    )?;
    Ok(())
}
