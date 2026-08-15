//! Pipeline configuration and step vocabulary for recording processing.
//!
//! These types describe *what* a processing run should do (which optional
//! steps are enabled, how steps are labelled, how progress events are
//! channelled). The actual per-recording orchestration — transcription,
//! document generation, RAG indexing — lives in the Tauri command layer
//! (`src-tauri/src/commands/`), which drives the workspace crates directly
//! and reports progress via `ProcessingEvent`.
//!
//! A previous `run_pipeline` function lived here that emitted
//! `TaskQueued → TaskStarted → TaskCompleted` events without performing any
//! of the underlying work. It was never wired into the app and has been
//! removed; do not reintroduce a scaffold that fakes step completion.

use medical_core::types::processing::ProcessingEvent;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Pipeline configuration
// ---------------------------------------------------------------------------

/// Controls which optional steps are executed during pipeline processing.
///
/// The pipeline always runs transcription and data extraction. The remaining
/// steps — SOAP generation, referral generation, letter generation, and RAG
/// indexing — are toggled by this config.
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
/// Used as a label/reporting vocabulary for progress UI. The always-run
/// steps are `Transcribing` and `ExtractingData`; the generation and
/// indexing steps are governed by [`PipelineConfig`]. A run terminates with
/// `Complete`.
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
