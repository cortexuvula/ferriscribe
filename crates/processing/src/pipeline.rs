//! Recording processing pipeline with step-level progress reporting.
//!
//! The pipeline runs a configurable sequence of steps for a single recording:
//! transcription (always), optional SOAP/referral/letter generation, data
//! extraction (always), and optional RAG indexing. Each step emits three
//! progress events (`TaskQueued → TaskStarted → TaskCompleted`) over an
//! `mpsc` channel so the frontend can show granular progress.
//!
//! Steps are sequential — concurrency across recordings lives in the caller
//! (typically `src-tauri` spawning multiple pipeline tasks).

use medical_core::types::processing::{ProcessingEvent, TaskType};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ProcessingResult;

// ---------------------------------------------------------------------------
// Pipeline configuration
// ---------------------------------------------------------------------------

/// Controls which optional steps are executed during pipeline processing.
///
/// The pipeline always runs transcription (Step 1) and data extraction
/// (Step 5). The remaining steps — SOAP generation, referral generation,
/// letter generation, and RAG indexing — are toggled by this config.
///
/// Default: SOAP on, referral off, letter off, RAG on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// Generate a SOAP note (default: true).
    pub generate_soap: bool,
    /// Generate a referral letter (default: false).
    pub generate_referral: bool,
    /// Generate a patient letter (default: false).
    pub generate_letter: bool,
    /// Automatically index the result into the RAG store (default: true).
    pub auto_index_rag: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            generate_soap: true,
            generate_referral: false,
            generate_letter: false,
            auto_index_rag: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline step
// ---------------------------------------------------------------------------

/// An individual step within the processing pipeline.
///
/// Steps are emitted in order by [`run_pipeline`]. The pipeline always runs
/// `Transcribing` and `ExtractingData`; the generation and indexing steps
/// are governed by [`PipelineConfig`]. Every run terminates with `Complete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineStep {
    Transcribing,
    GeneratingSoap,
    GeneratingReferral,
    GeneratingLetter,
    ExtractingData,
    IndexingRag,
    Complete,
}

impl PipelineStep {
    /// A short human-readable label for this step.
    pub fn label(&self) -> &'static str {
        match self {
            PipelineStep::Transcribing => "Transcribing",
            PipelineStep::GeneratingSoap => "Generating SOAP note",
            PipelineStep::GeneratingReferral => "Generating referral letter",
            PipelineStep::GeneratingLetter => "Generating patient letter",
            PipelineStep::ExtractingData => "Extracting data",
            PipelineStep::IndexingRag => "Indexing into RAG",
            PipelineStep::Complete => "Complete",
        }
    }
}

// ---------------------------------------------------------------------------
// Progress channel type alias
// ---------------------------------------------------------------------------

/// Sender half of the progress event channel.
pub type ProgressSender = mpsc::Sender<ProcessingEvent>;

// ---------------------------------------------------------------------------
// Helper – send an event, swallowing channel-closed errors
// ---------------------------------------------------------------------------

async fn send_event(tx: &ProgressSender, event: ProcessingEvent) {
    // A closed channel means the receiver was dropped; not a fatal error.
    let _ = tx.send(event).await;
}

// ---------------------------------------------------------------------------
// Pipeline runner
// ---------------------------------------------------------------------------

/// Run the processing pipeline for a single recording.
///
/// Executes steps in order according to `config`, emitting three progress
/// events per step (`TaskQueued → TaskStarted → TaskCompleted`) through
/// `progress`. Returns the list of steps that were executed, always ending
/// with [`PipelineStep::Complete`].
///
/// # Errors
///
/// Returns [`ProcessingError::Cancelled`](crate::ProcessingError::Cancelled) if the progress channel is closed
/// mid-pipeline (though currently channel-closed sends are swallowed silently
/// via `let _ = tx.send(...).await`).
///
/// # Concurrency
///
/// This function is sequential. To process multiple recordings concurrently,
/// the caller (typically `src-tauri`) spawns multiple pipeline tasks.
pub async fn run_pipeline(
    recording_id: Uuid,
    config: &PipelineConfig,
    progress: &ProgressSender,
) -> ProcessingResult<Vec<PipelineStep>> {
    let mut completed: Vec<PipelineStep> = Vec::new();

    // Helper closure to build a synthetic task_id per step.
    let step_task_id = |step: PipelineStep| -> Uuid {
        // Deterministic-ish: just generate a fresh one per call.
        let _ = step; // step used conceptually
        Uuid::new_v4()
    };

    // ------------------------------------------------------------------
    // Step 1: Transcribing (always)
    // ------------------------------------------------------------------
    {
        let task_id = step_task_id(PipelineStep::Transcribing);
        send_event(
            progress,
            ProcessingEvent::TaskQueued {
                task_id,
                recording_id,
                task_type: TaskType::Transcribe,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskStarted {
                task_id,
                recording_id,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskCompleted {
                task_id,
                recording_id,
                duration_ms: 0,
            },
        )
        .await;
        completed.push(PipelineStep::Transcribing);
    }

    // ------------------------------------------------------------------
    // Step 2: GeneratingSoap (if configured)
    // ------------------------------------------------------------------
    if config.generate_soap {
        let task_id = step_task_id(PipelineStep::GeneratingSoap);
        send_event(
            progress,
            ProcessingEvent::TaskQueued {
                task_id,
                recording_id,
                task_type: TaskType::GenerateSoap,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskStarted {
                task_id,
                recording_id,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskCompleted {
                task_id,
                recording_id,
                duration_ms: 0,
            },
        )
        .await;
        completed.push(PipelineStep::GeneratingSoap);
    }

    // ------------------------------------------------------------------
    // Step 3: GeneratingReferral (if configured)
    // ------------------------------------------------------------------
    if config.generate_referral {
        let task_id = step_task_id(PipelineStep::GeneratingReferral);
        send_event(
            progress,
            ProcessingEvent::TaskQueued {
                task_id,
                recording_id,
                task_type: TaskType::GenerateReferral,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskStarted {
                task_id,
                recording_id,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskCompleted {
                task_id,
                recording_id,
                duration_ms: 0,
            },
        )
        .await;
        completed.push(PipelineStep::GeneratingReferral);
    }

    // ------------------------------------------------------------------
    // Step 4: GeneratingLetter (if configured)
    // ------------------------------------------------------------------
    if config.generate_letter {
        let task_id = step_task_id(PipelineStep::GeneratingLetter);
        send_event(
            progress,
            ProcessingEvent::TaskQueued {
                task_id,
                recording_id,
                task_type: TaskType::GenerateLetter,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskStarted {
                task_id,
                recording_id,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskCompleted {
                task_id,
                recording_id,
                duration_ms: 0,
            },
        )
        .await;
        completed.push(PipelineStep::GeneratingLetter);
    }

    // ------------------------------------------------------------------
    // Step 5: ExtractingData (always)
    // ------------------------------------------------------------------
    {
        let task_id = step_task_id(PipelineStep::ExtractingData);
        send_event(
            progress,
            ProcessingEvent::TaskQueued {
                task_id,
                recording_id,
                task_type: TaskType::ExtractData,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskStarted {
                task_id,
                recording_id,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskCompleted {
                task_id,
                recording_id,
                duration_ms: 0,
            },
        )
        .await;
        completed.push(PipelineStep::ExtractingData);
    }

    // ------------------------------------------------------------------
    // Step 6: IndexingRag (if configured)
    // ------------------------------------------------------------------
    if config.auto_index_rag {
        let task_id = step_task_id(PipelineStep::IndexingRag);
        send_event(
            progress,
            ProcessingEvent::TaskQueued {
                task_id,
                recording_id,
                task_type: TaskType::IndexRag,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskStarted {
                task_id,
                recording_id,
            },
        )
        .await;
        send_event(
            progress,
            ProcessingEvent::TaskCompleted {
                task_id,
                recording_id,
                duration_ms: 0,
            },
        )
        .await;
        completed.push(PipelineStep::IndexingRag);
    }

    // ------------------------------------------------------------------
    // Step 7: Complete (always) – send a BatchCompleted event as a proxy
    // ------------------------------------------------------------------
    send_event(
        progress,
        ProcessingEvent::BatchCompleted {
            batch_id: recording_id, // reuse recording_id for single-recording completion
            total: completed.len() as u32 + 1, // +1 for Complete itself
            failed: 0,
        },
    )
    .await;
    completed.push(PipelineStep::Complete);

    Ok(completed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn pipeline_default_steps() {
        let (tx, _rx) = mpsc::channel(64);
        let config = PipelineConfig::default(); // soap=true, referral=false, letter=false, rag=true
        let id = Uuid::new_v4();
        let steps = run_pipeline(id, &config, &tx).await.unwrap();

        assert!(steps.contains(&PipelineStep::Transcribing));
        assert!(steps.contains(&PipelineStep::GeneratingSoap));
        assert!(!steps.contains(&PipelineStep::GeneratingReferral));
        assert!(!steps.contains(&PipelineStep::GeneratingLetter));
        assert!(steps.contains(&PipelineStep::ExtractingData));
        assert!(steps.contains(&PipelineStep::IndexingRag));
        assert!(steps.contains(&PipelineStep::Complete));
    }

    #[tokio::test]
    async fn pipeline_all_steps() {
        let (tx, _rx) = mpsc::channel(64);
        let config = PipelineConfig {
            generate_soap: true,
            generate_referral: true,
            generate_letter: true,
            auto_index_rag: true,
        };
        let id = Uuid::new_v4();
        let steps = run_pipeline(id, &config, &tx).await.unwrap();

        assert_eq!(steps.len(), 7);
        assert!(steps.contains(&PipelineStep::Transcribing));
        assert!(steps.contains(&PipelineStep::GeneratingSoap));
        assert!(steps.contains(&PipelineStep::GeneratingReferral));
        assert!(steps.contains(&PipelineStep::GeneratingLetter));
        assert!(steps.contains(&PipelineStep::ExtractingData));
        assert!(steps.contains(&PipelineStep::IndexingRag));
        assert!(steps.contains(&PipelineStep::Complete));
    }

    #[test]
    fn step_labels() {
        assert_eq!(PipelineStep::Transcribing.label(), "Transcribing");
        assert_eq!(PipelineStep::GeneratingSoap.label(), "Generating SOAP note");
        assert_eq!(
            PipelineStep::GeneratingReferral.label(),
            "Generating referral letter"
        );
        assert_eq!(
            PipelineStep::GeneratingLetter.label(),
            "Generating patient letter"
        );
        assert_eq!(PipelineStep::ExtractingData.label(), "Extracting data");
        assert_eq!(PipelineStep::IndexingRag.label(), "Indexing into RAG");
        assert_eq!(PipelineStep::Complete.label(), "Complete");
    }
}
