//! Batch processing job tracker for multi-recording runs.
//!
//! Tracks progress counts, completion status, and derived [`BatchState`]
//! across a set of recordings processed with the same [`PipelineConfig`].
//! Callers invoke [`BatchJob::record_success`] / [`BatchJob::record_failure`]
//! after each recording and poll [`BatchJob::is_done`] /
//! [`BatchJob::progress_percent`] for UI progress reporting.
//!
//! Note: there is no `PartiallyCompleted` variant in `BatchState` — mixed
//! outcomes (some succeeded, some failed) are reported as `Failed`. Callers
//! must inspect `completed_count` vs `failed_count` to distinguish total
//! from partial failure.

use chrono::{DateTime, Utc};
use medical_core::types::processing::BatchState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pipeline::PipelineConfig;

// ---------------------------------------------------------------------------
// BatchJob
// ---------------------------------------------------------------------------

/// Tracks the state of a multi-recording batch processing job.
///
/// Created in the `Queued` state via [`BatchJob::new`]. As each recording
/// completes (success or failure), the caller invokes [`record_success`] or
/// [`record_failure`], which update counts and recompute [`status`].
///
/// The batch transitions to `Running` on the first recorded outcome, then to
/// `Completed` (all succeeded), `Failed` (any failures), once [`is_done`]
/// returns true.
///
/// [`record_success`]: BatchJob::record_success
/// [`record_failure`]: BatchJob::record_failure
/// [`status`]: BatchJob::status
/// [`is_done`]: BatchJob::is_done
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchJob {
    pub id: Uuid,
    pub recording_ids: Vec<Uuid>,
    pub config: PipelineConfig,
    pub status: BatchState,
    pub total_count: usize,
    pub completed_count: usize,
    pub failed_count: usize,
    pub created_at: DateTime<Utc>,
}

impl BatchJob {
    /// Create a new batch job in the `Queued` (pending) state.
    pub fn new(recording_ids: Vec<Uuid>, config: PipelineConfig) -> Self {
        let total_count = recording_ids.len();
        Self {
            id: Uuid::new_v4(),
            recording_ids,
            config,
            status: BatchState::Queued,
            total_count,
            completed_count: 0,
            failed_count: 0,
            created_at: Utc::now(),
        }
    }

    // ------------------------------------------------------------------
    // Progress mutators
    // ------------------------------------------------------------------

    /// Record one successful recording.
    pub fn record_success(&mut self) {
        self.completed_count += 1;
        self.update_status();
    }

    /// Record one failed recording.
    pub fn record_failure(&mut self) {
        self.failed_count += 1;
        self.update_status();
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Returns `true` when every recording has been processed (success or failure).
    pub fn is_done(&self) -> bool {
        self.completed_count + self.failed_count >= self.total_count
    }

    /// Returns progress as a fraction in `[0.0, 1.0]`.
    pub fn progress_percent(&self) -> f64 {
        if self.total_count == 0 {
            return 1.0;
        }
        (self.completed_count + self.failed_count) as f64 / self.total_count as f64
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Recompute `status` based on current counts.
    fn update_status(&mut self) {
        if !self.is_done() {
            self.status = BatchState::Running;
        } else if self.failed_count == 0 {
            self.status = BatchState::Completed;
        } else if self.completed_count == 0 {
            // All recordings failed.
            self.status = BatchState::Failed;
        } else {
            // Mixed: some succeeded, some failed — treated as Failed with partial data.
            // (The processing type set does not define a PartiallyCompleted variant.)
            self.status = BatchState::Failed;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    #[test]
    fn new_batch_job() {
        let ids = make_ids(3);
        let job = BatchJob::new(ids.clone(), PipelineConfig::default());

        assert_eq!(job.total_count, 3);
        assert_eq!(job.completed_count, 0);
        assert_eq!(job.failed_count, 0);
        assert_eq!(job.status, BatchState::Queued);
        assert_eq!(job.recording_ids, ids);
        assert!(!job.is_done());
        assert!((job.progress_percent() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tracks_progress() {
        let mut job = BatchJob::new(make_ids(4), PipelineConfig::default());

        job.record_success();
        assert_eq!(job.completed_count, 1);
        assert_eq!(job.status, BatchState::Running);
        assert!(!job.is_done());
        assert!((job.progress_percent() - 0.25).abs() < 1e-9);

        job.record_success();
        job.record_success();
        assert!(!job.is_done());

        job.record_success();
        assert!(job.is_done());
        assert_eq!(job.status, BatchState::Completed);
        assert!((job.progress_percent() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn partial_failure() {
        let mut job = BatchJob::new(make_ids(3), PipelineConfig::default());

        job.record_success();
        job.record_failure();
        job.record_success();

        assert!(job.is_done());
        assert_eq!(job.completed_count, 2);
        assert_eq!(job.failed_count, 1);
        // Mixed outcome — reported as Failed (no PartiallyCompleted variant available).
        assert_eq!(job.status, BatchState::Failed);
    }

    #[test]
    fn total_failure() {
        let mut job = BatchJob::new(make_ids(2), PipelineConfig::default());

        job.record_failure();
        job.record_failure();

        assert!(job.is_done());
        assert_eq!(job.completed_count, 0);
        assert_eq!(job.failed_count, 2);
        assert_eq!(job.status, BatchState::Failed);
        assert!((job.progress_percent() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_batch() {
        let job = BatchJob::new(vec![], PipelineConfig::default());

        assert_eq!(job.total_count, 0);
        assert!(job.is_done());
        // Empty batch is trivially complete.
        assert!((job.progress_percent() - 1.0).abs() < f64::EPSILON);
        // Status starts as Queued; update_status is not called until record_* is invoked.
        assert_eq!(job.status, BatchState::Queued);
    }
}
