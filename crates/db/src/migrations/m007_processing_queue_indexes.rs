//! Migration 7: Add indexes to `processing_queue` for dequeue performance.
//!
//! The dequeue query filters on `status = 'pending' ORDER BY priority DESC,
//! created_at ASC` — without indexes this is a full table scan that grows
//! linearly as completed/failed tasks accumulate.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "
        -- Composite index for the dequeue CTE:
        -- WHERE status = 'pending' ORDER BY priority DESC, created_at ASC
        CREATE INDEX IF NOT EXISTS idx_pq_status_priority
            ON processing_queue(status, priority DESC, created_at ASC);

        -- Index for get_by_recording lookups
        CREATE INDEX IF NOT EXISTS idx_pq_recording
            ON processing_queue(recording_id);
        ",
    )?;
    Ok(())
}
