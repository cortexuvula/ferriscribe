//! Queue-task and batch-processing types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Relative priority for queue tasks.
///
/// Implements [`Ord`] so higher-priority tasks sort first when the
/// processing queue dequeues work. The [`as_i32`](Priority::as_i32)
/// method produces a signed integer suitable for SQL `ORDER BY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Priority {
    /// Below-normal priority — background / non-urgent work.
    Low,
    /// Default priority.
    #[default]
    Normal,
    /// Above-normal priority — user-facing / interactive work.
    High,
}


impl Priority {
    /// Returns a signed integer representation suitable for ordering
    /// queries (`Low = -1`, `Normal = 0`, `High = 1`).
    pub fn as_i32(self) -> i32 {
        match self {
            Priority::Low => -1,
            Priority::Normal => 0,
            Priority::High => 1,
        }
    }
}

/// The kind of work a queue task represents.
///
/// Each variant corresponds to a distinct processing pipeline (STT,
/// SOAP generation, etc.) in the `processing` crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// Speech-to-text transcription.
    Transcribe,
    /// SOAP note generation.
    GenerateSoap,
    /// Referral letter generation.
    GenerateReferral,
    /// Patient letter generation.
    GenerateLetter,
    /// Structured data extraction.
    ExtractData,
    /// RAG document indexing.
    IndexRag,
}

/// A single unit of work in the processing queue.
///
/// Stored in the `queue_tasks` table. The processing crate picks up
/// pending tasks, transitions them through
/// [`QueueTaskStatus::Processing`], and marks them
/// [`QueueTaskStatus::Completed`] or [`QueueTaskStatus::Failed`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueTask {
    /// Unique task identifier.
    pub id: Uuid,
    /// The recording this task operates on.
    pub recording_id: Uuid,
    /// What kind of work this task performs.
    pub task_type: TaskType,
    /// Scheduling priority.
    pub priority: Priority,
    /// Current lifecycle state.
    pub status: QueueTaskStatus,
    /// When the task was enqueued.
    pub created_at: DateTime<Utc>,
    /// Batch this task belongs to (if part of a batch job).
    pub batch_id: Option<Uuid>,
}

/// Lifecycle state of a queue task.
///
/// Tagged with `status` for JSON serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueueTaskStatus {
    /// Waiting to be picked up by a worker.
    Pending,
    /// Currently being executed by a worker.
    Processing {
        /// When processing started.
        started_at: DateTime<Utc>,
    },
    /// Completed successfully.
    Completed {
        /// When processing finished.
        completed_at: DateTime<Utc>,
        /// Optional result summary.
        result: Option<String>,
    },
    /// Failed — may be retried depending on `error_count`.
    Failed {
        /// Description of the failure.
        error: String,
        /// Number of consecutive failures.
        error_count: u32,
    },
}

/// Options controlling how a batch processing job behaves.
///
/// Passed to the batch-processing entry point in the `processing` crate.
/// Controls which document types to generate, concurrency, and error
/// handling strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchProcessingOptions {
    /// Whether to generate SOAP notes for each recording.
    pub generate_soap: bool,
    /// Whether to generate referral letters.
    pub generate_referral: bool,
    /// Whether to generate patient letters.
    pub generate_letter: bool,
    /// Skip recordings that already have the requested output.
    pub skip_existing: bool,
    /// Continue processing remaining items if one fails.
    pub continue_on_error: bool,
    /// Priority assigned to all tasks in the batch.
    pub priority: Priority,
    /// Maximum number of concurrent workers.
    pub max_concurrent: u32,
}

impl Default for BatchProcessingOptions {
    fn default() -> Self {
        Self {
            generate_soap: true,
            generate_referral: false,
            generate_letter: false,
            skip_existing: true,
            continue_on_error: true,
            priority: Priority::Normal,
            max_concurrent: 3,
        }
    }
}

/// Overall status of a batch job.
///
/// Aggregates progress across all tasks in a batch for UI display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchStatus {
    /// The batch identifier (shared by all [`QueueTask::batch_id`]s).
    pub batch_id: Uuid,
    /// High-level batch lifecycle state.
    pub state: BatchState,
    /// Total number of tasks in the batch.
    pub total: u32,
    /// Number of tasks that completed successfully.
    pub completed: u32,
    /// Number of tasks that failed.
    pub failed: u32,
    /// When the batch was created.
    pub created_at: DateTime<Utc>,
}

/// High-level lifecycle state of a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchState {
    /// All tasks are enqueued, no worker has started yet.
    Queued,
    /// At least one task is being processed.
    Running,
    /// All tasks completed (some may have failed).
    Completed,
    /// The batch failed catastrophically.
    Failed,
    /// The batch was cancelled by the user.
    Cancelled,
}

/// Events emitted during processing for progress tracking.
///
/// Tagged with `event` for JSON serialization. The Tauri frontend
/// subscribes to these via event listeners to update progress UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ProcessingEvent {
    /// A task was added to the queue.
    TaskQueued {
        /// The queued task's ID.
        task_id: Uuid,
        /// The recording the task operates on.
        recording_id: Uuid,
        /// What kind of work the task performs.
        task_type: TaskType,
    },
    /// A worker picked up a task.
    TaskStarted {
        /// The task's ID.
        task_id: Uuid,
        /// The recording being processed.
        recording_id: Uuid,
    },
    /// A task completed successfully.
    TaskCompleted {
        /// The task's ID.
        task_id: Uuid,
        /// The recording that was processed.
        recording_id: Uuid,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },
    /// A task failed.
    TaskFailed {
        /// The task's ID.
        task_id: Uuid,
        /// The recording that failed.
        recording_id: Uuid,
        /// Error description.
        error: String,
    },
    /// All tasks in a batch have finished.
    BatchCompleted {
        /// The batch ID.
        batch_id: Uuid,
        /// Total tasks in the batch.
        total: u32,
        /// Number of tasks that failed.
        failed: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert_eq!(Priority::Low.as_i32(), -1);
        assert_eq!(Priority::Normal.as_i32(), 0);
        assert_eq!(Priority::High.as_i32(), 1);
        assert_eq!(Priority::default(), Priority::Normal);
    }

    #[test]
    fn batch_options_defaults() {
        let opts = BatchProcessingOptions::default();
        assert!(opts.generate_soap);
        assert!(!opts.generate_referral);
        assert!(!opts.generate_letter);
        assert!(opts.skip_existing);
        assert!(opts.continue_on_error);
        assert_eq!(opts.priority, Priority::Normal);
        assert_eq!(opts.max_concurrent, 3);
    }

    #[test]
    fn queue_task_status_serializes() {
        let status = QueueTaskStatus::Pending;
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["status"], "pending");

        let failed = QueueTaskStatus::Failed {
            error: "network".into(),
            error_count: 2,
        };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["error_count"], 2);

        let completed = QueueTaskStatus::Completed {
            completed_at: Utc::now(),
            result: Some("done".into()),
        };
        let json = serde_json::to_value(&completed).unwrap();
        assert_eq!(json["status"], "completed");
        assert_eq!(json["result"], "done");
    }

    #[test]
    fn processing_event_serializes() {
        let id = Uuid::new_v4();
        let rec_id = Uuid::new_v4();
        let event = ProcessingEvent::TaskStarted {
            task_id: id,
            recording_id: rec_id,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "task_started");

        let batch_event = ProcessingEvent::BatchCompleted {
            batch_id: Uuid::new_v4(),
            total: 10,
            failed: 1,
        };
        let json = serde_json::to_value(&batch_event).unwrap();
        assert_eq!(json["event"], "batch_completed");
        assert_eq!(json["total"], 10);
    }
}
