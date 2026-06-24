//! CRUD operations for the `processing_queue` table.

use rusqlite::Connection;
use uuid::Uuid;

use crate::{DbError, DbResult};

/// Lightweight row struct returned by queue queries.
///
/// Uses simple string fields for `task_type` and `status` to keep the DB layer
/// decoupled from the richer domain enums in `medical_core::types::processing`.
#[derive(Debug, Clone)]
pub struct QueueTask {
    pub id: Uuid,
    pub recording_id: Uuid,
    pub task_type: String,
    pub priority: i32,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub error_count: i32,
    pub last_error: Option<String>,
    pub result: Option<String>,
    pub batch_id: Option<String>,
}

/// Repository for the `processing_queue` table.
///
/// Manages a priority queue of background tasks (transcription, SOAP
/// generation, etc.) associated with recordings. Tasks flow through
/// `pending` -> `processing` -> `completed`/`failed` states.
pub struct ProcessingQueueRepo;

impl ProcessingQueueRepo {
    pub fn new() -> Self {
        Self
    }

    /// Insert a new task into the processing queue and return its ID.
    ///
    /// The task starts in `pending` status. Use [`dequeue`](Self::dequeue)
    /// to atomically claim the highest-priority task.
    pub fn enqueue(
        conn: &Connection,
        recording_id: &Uuid,
        task_type: &str,
        priority: i32,
    ) -> DbResult<Uuid> {
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO processing_queue (id, recording_id, task_type, priority, status)
             VALUES (?1, ?2, ?3, ?4, 'pending')",
            rusqlite::params![
                id.to_string(),
                recording_id.to_string(),
                task_type,
                priority,
            ],
        )?;
        Ok(id)
    }

    /// Atomically fetch the highest-priority pending task and mark it as
    /// `processing`.  Returns `None` when the queue is empty.
    ///
    /// Ordering: priority DESC (higher number = more urgent), then created_at
    /// ASC (older tasks first within the same priority).
    pub fn dequeue(conn: &Connection) -> DbResult<Option<QueueTask>> {
        // Use a CTE to atomically SELECT + UPDATE in one statement so that
        // concurrent workers don't pick up the same task.
        let mut stmt = conn.prepare(
            "UPDATE processing_queue
             SET status = 'processing',
                 started_at = datetime('now')
             WHERE id = (
                 SELECT id FROM processing_queue
                 WHERE status = 'pending'
                 ORDER BY priority DESC, created_at ASC
                 LIMIT 1
             )
             RETURNING id, recording_id, task_type, priority, status,
                       created_at, started_at, completed_at, error_count,
                       last_error, result, batch_id",
        )?;

        let mut rows = stmt.query_map([], Self::row_to_task)?;
        match rows.next() {
            Some(Ok(task)) => Ok(Some(task)),
            Some(Err(e)) => Err(DbError::Sqlite(e)),
            None => Ok(None),
        }
    }

    /// Update the status of a task.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if no task with the given ID exists.
    pub fn update_status(conn: &Connection, task_id: &Uuid, status: &str) -> DbResult<()> {
        let rows = conn.execute(
            "UPDATE processing_queue SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, task_id.to_string()],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("queue task {task_id}")));
        }
        Ok(())
    }

    /// Retrieve all tasks associated with a given recording, ordered by
    /// `created_at ASC`.
    pub fn get_by_recording(conn: &Connection, recording_id: &Uuid) -> DbResult<Vec<QueueTask>> {
        let mut stmt = conn.prepare(
            "SELECT id, recording_id, task_type, priority, status,
                    created_at, started_at, completed_at, error_count,
                    last_error, result, batch_id
             FROM processing_queue
             WHERE recording_id = ?1
             ORDER BY created_at ASC",
        )?;

        let tasks = stmt
            .query_map([recording_id.to_string()], Self::row_to_task)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    /// Count tasks that are still in `pending` status.
    pub fn count_pending(conn: &Connection) -> DbResult<u64> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM processing_queue WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )?;
        Ok(count as u64)
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueTask> {
        let id_str: String = row.get(0)?;
        let rec_id_str: String = row.get(1)?;
        Ok(QueueTask {
            id: Uuid::parse_str(&id_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            recording_id: Uuid::parse_str(&rec_id_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            task_type: row.get(2)?,
            priority: row.get(3)?,
            status: row.get(4)?,
            created_at: row.get(5)?,
            started_at: row.get(6)?,
            completed_at: row.get(7)?,
            error_count: row.get(8)?,
            last_error: row.get(9)?,
            result: row.get(10)?,
            batch_id: row.get(11)?,
        })
    }
}

impl Default for ProcessingQueueRepo {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;

    /// Create an in-memory DB with all migrations applied and a dummy recording
    /// row (required for the FK on `processing_queue.recording_id`).
    fn setup() -> (rusqlite::Connection, Uuid) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrations::MigrationEngine::migrate(&conn).unwrap();

        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status) VALUES (?1, ?2, ?3)",
            rusqlite::params![rec_id.to_string(), "test.wav", "\"pending\""],
        )
        .unwrap();
        (conn, rec_id)
    }

    #[test]
    fn enqueue_returns_id() {
        let (conn, rec_id) = setup();
        let task_id = ProcessingQueueRepo::enqueue(&conn, &rec_id, "transcribe", 0).unwrap();
        // Should be a valid UUID
        assert_ne!(task_id, Uuid::nil());
    }

    #[test]
    fn dequeue_returns_highest_priority_first() {
        let (conn, rec_id) = setup();

        // Enqueue three tasks with different priorities
        let _low = ProcessingQueueRepo::enqueue(&conn, &rec_id, "low_task", -1).unwrap();
        let _normal = ProcessingQueueRepo::enqueue(&conn, &rec_id, "normal_task", 0).unwrap();
        let high = ProcessingQueueRepo::enqueue(&conn, &rec_id, "high_task", 1).unwrap();

        // Dequeue should return the high-priority task
        let task = ProcessingQueueRepo::dequeue(&conn)
            .unwrap()
            .expect("should return a task");
        assert_eq!(task.id, high);
        assert_eq!(task.task_type, "high_task");
        assert_eq!(task.status, "processing");

        // Dequeue again — should return normal
        let task2 = ProcessingQueueRepo::dequeue(&conn)
            .unwrap()
            .expect("should return a task");
        assert_eq!(task2.task_type, "normal_task");

        // And then low
        let task3 = ProcessingQueueRepo::dequeue(&conn)
            .unwrap()
            .expect("should return a task");
        assert_eq!(task3.task_type, "low_task");

        // Queue should now be empty
        assert!(ProcessingQueueRepo::dequeue(&conn).unwrap().is_none());
    }

    #[test]
    fn dequeue_respects_created_at_within_same_priority() {
        let (conn, rec_id) = setup();

        let first = ProcessingQueueRepo::enqueue(&conn, &rec_id, "first", 0).unwrap();
        let _second = ProcessingQueueRepo::enqueue(&conn, &rec_id, "second", 0).unwrap();

        let task = ProcessingQueueRepo::dequeue(&conn).unwrap().expect("task");
        assert_eq!(task.id, first);
    }

    #[test]
    fn dequeue_on_empty_queue_returns_none() {
        let (conn, _) = setup();
        assert!(ProcessingQueueRepo::dequeue(&conn).unwrap().is_none());
    }

    #[test]
    fn update_status() {
        let (conn, rec_id) = setup();
        let task_id = ProcessingQueueRepo::enqueue(&conn, &rec_id, "transcribe", 0).unwrap();

        ProcessingQueueRepo::update_status(&conn, &task_id, "completed").unwrap();

        // The task should no longer be dequeue-able (not pending)
        assert!(ProcessingQueueRepo::dequeue(&conn).unwrap().is_none());

        // Verify status via get_by_recording
        let tasks = ProcessingQueueRepo::get_by_recording(&conn, &rec_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "completed");
    }

    #[test]
    fn update_status_not_found() {
        let (conn, _) = setup();
        let bogus = Uuid::new_v4();
        let result = ProcessingQueueRepo::update_status(&conn, &bogus, "done");
        assert!(result.is_err());
    }

    #[test]
    fn get_by_recording() {
        let (conn, rec_id) = setup();

        ProcessingQueueRepo::enqueue(&conn, &rec_id, "transcribe", 0).unwrap();
        ProcessingQueueRepo::enqueue(&conn, &rec_id, "generate_soap", 1).unwrap();

        let tasks = ProcessingQueueRepo::get_by_recording(&conn, &rec_id).unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn get_by_recording_empty() {
        let (conn, _) = setup();
        let other_id = Uuid::new_v4();
        let tasks = ProcessingQueueRepo::get_by_recording(&conn, &other_id).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn count_pending() {
        let (conn, rec_id) = setup();

        assert_eq!(ProcessingQueueRepo::count_pending(&conn).unwrap(), 0);

        ProcessingQueueRepo::enqueue(&conn, &rec_id, "a", 0).unwrap();
        ProcessingQueueRepo::enqueue(&conn, &rec_id, "b", 0).unwrap();
        assert_eq!(ProcessingQueueRepo::count_pending(&conn).unwrap(), 2);

        // Dequeue one — should go to processing, not pending
        ProcessingQueueRepo::dequeue(&conn).unwrap();
        assert_eq!(ProcessingQueueRepo::count_pending(&conn).unwrap(), 1);
    }
}
