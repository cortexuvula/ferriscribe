//! Recording and processing-status types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// A recorded consultation with optional transcript and generated documents.
///
/// This is the central domain entity — every other subsystem (STT, SOAP
/// generation, referral, letter, chat) reads from and writes to fields
/// on this struct. Stored in the `recordings` table via the `db` crate.
///
/// The `metadata` field is a freeform JSON blob that holds both
/// freeform `context` (string) and structured `patient_context`
/// ([`PatientContext`](super::agent::PatientContext) shape). New metadata
/// keys are non-breaking.
///
/// # PHI note
///
/// Manual `Debug` impl redacts transcript, SOAP note, referral, letter,
/// peer discussion, chat, and patient name — these are PHI and must never
/// appear in logs or panic backtraces.
#[derive(Clone, Serialize, Deserialize)]
pub struct Recording {
    /// Unique identifier (UUIDv4, assigned at creation).
    pub id: Uuid,
    /// Original filename of the audio file.
    pub filename: String,
    /// Transcribed text (populated after STT).
    pub transcript: Option<String>,
    /// Generated SOAP note (populated after AI generation).
    pub soap_note: Option<String>,
    /// Generated referral letter.
    pub referral: Option<String>,
    /// Generated patient letter.
    pub letter: Option<String>,
    /// Generated peer-to-peer discussion note.
    pub peer_discussion: Option<String>,
    /// Interactive chat transcript.
    pub chat: Option<String>,
    /// Patient name (if known).
    pub patient_name: Option<String>,
    /// Path to the audio file on disk.
    pub audio_path: PathBuf,
    /// Duration of the audio in seconds.
    pub duration_seconds: Option<f64>,
    /// Size of the audio file in bytes.
    pub file_size_bytes: Option<u64>,
    /// Which STT provider produced the transcript.
    pub stt_provider: Option<String>,
    /// Which AI provider produced the SOAP note.
    pub ai_provider: Option<String>,
    /// User-assigned tags for filtering.
    pub tags: Vec<String>,
    /// Processing lifecycle state.
    pub status: ProcessingStatus,
    /// When the recording was created.
    pub created_at: DateTime<Utc>,
    /// Freeform JSON metadata (see module docs for known keys).
    pub metadata: serde_json::Value,
    /// Last modification timestamp (any field). Drives content-sync
    /// delta filtering. Set to `created_at` on insert, bumped on every update.
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

/// Manual Debug impl that redacts PHI-bearing fields. Only logs structural
/// metadata (id, filename, status, timestamps, provider names, sizes) —
/// never transcript, SOAP note, referral, letter, chat, or patient name.
impl std::fmt::Debug for Recording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recording")
            .field("id", &self.id)
            .field("filename", &self.filename)
            .field(
                "transcript",
                &self
                    .transcript
                    .as_ref()
                    .map(|t| format!("<{} chars>", t.len())),
            )
            .field(
                "soap_note",
                &self
                    .soap_note
                    .as_ref()
                    .map(|t| format!("<{} chars>", t.len())),
            )
            .field(
                "referral",
                &self
                    .referral
                    .as_ref()
                    .map(|t| format!("<{} chars>", t.len())),
            )
            .field(
                "letter",
                &self.letter.as_ref().map(|t| format!("<{} chars>", t.len())),
            )
            .field(
                "peer_discussion",
                &self
                    .peer_discussion
                    .as_ref()
                    .map(|t| format!("<{} chars>", t.len())),
            )
            .field(
                "chat",
                &self.chat.as_ref().map(|t| format!("<{} chars>", t.len())),
            )
            .field("patient_name", &"<redacted>")
            .field("audio_path", &self.audio_path)
            .field("duration_seconds", &self.duration_seconds)
            .field("file_size_bytes", &self.file_size_bytes)
            .field("stt_provider", &self.stt_provider)
            .field("ai_provider", &self.ai_provider)
            .field("tags", &self.tags)
            .field("status", &self.status)
            .field("created_at", &self.created_at)
            .field("metadata", &"<redacted>")
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl Recording {
    /// Create a new recording in the [`Pending`](ProcessingStatus::Pending) state.
    ///
    /// Generates a new UUIDv4 and sets `created_at` to now. All optional
    /// fields (transcript, soap_note, etc.) start as `None`.
    pub fn new(filename: impl Into<String>, audio_path: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            filename: filename.into(),
            transcript: None,
            soap_note: None,
            referral: None,
            letter: None,
            peer_discussion: None,
            chat: None,
            patient_name: None,
            audio_path,
            duration_seconds: None,
            file_size_bytes: None,
            stt_provider: None,
            ai_provider: None,
            tags: Vec::new(),
            status: ProcessingStatus::Pending,
            created_at: Utc::now(),
            metadata: serde_json::Value::Null,
            updated_at: None,
        }
    }

    /// Returns `true` if processing has completed successfully.
    pub fn is_processed(&self) -> bool {
        matches!(self.status, ProcessingStatus::Completed { .. })
    }

    /// Returns `true` if a transcript is present.
    pub fn has_transcript(&self) -> bool {
        self.transcript.is_some()
    }

    /// Returns `true` if a SOAP note is present.
    pub fn has_soap_note(&self) -> bool {
        self.soap_note.is_some()
    }
}

/// Processing lifecycle of a recording.
///
/// Tagged with `status` for JSON serialization so the frontend can
/// dispatch on state. Transitions: `Pending` → `Processing` →
/// `Completed` or `Failed`. Failed tasks with `retry_count < 3`
/// may be retried.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProcessingStatus {
    /// Waiting to be processed.
    Pending,
    /// Currently being processed.
    Processing {
        /// When processing started.
        started_at: DateTime<Utc>,
    },
    /// Processing completed successfully.
    Completed {
        /// When processing finished.
        completed_at: DateTime<Utc>,
    },
    /// Processing failed (may be retried if `retry_count < 3`).
    Failed {
        /// Description of the failure.
        error: String,
        /// Number of times this task has been retried.
        retry_count: u32,
    },
}

impl ProcessingStatus {
    /// Returns `true` if no further automatic transitions are expected.
    ///
    /// `Completed` is always terminal. `Failed` is terminal once
    /// `retry_count >= 3`.
    pub fn is_terminal(&self) -> bool {
        match self {
            ProcessingStatus::Completed { .. } => true,
            ProcessingStatus::Failed { retry_count, .. } => *retry_count >= 3,
            _ => false,
        }
    }

    /// Returns `true` if the task can be retried (failed fewer than 3 times).
    pub fn can_retry(&self) -> bool {
        match self {
            ProcessingStatus::Failed { retry_count, .. } => *retry_count < 3,
            _ => false,
        }
    }

    /// A human-readable label for the status (e.g. `"Pending"`, `"Failed"`).
    pub fn status_label(&self) -> &'static str {
        match self {
            ProcessingStatus::Pending => "Pending",
            ProcessingStatus::Processing { .. } => "Processing",
            ProcessingStatus::Completed { .. } => "Completed",
            ProcessingStatus::Failed { .. } => "Failed",
        }
    }
}

/// Lightweight summary of a recording suitable for list views.
///
/// Avoids loading full transcript/SOAP content. Use
/// `RecordingSummary::from(&recording)` to derive from a full
/// [`Recording`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSummary {
    /// Recording UUID.
    pub id: Uuid,
    /// Original filename.
    pub filename: String,
    /// Patient name (if known).
    pub patient_name: Option<String>,
    /// Current processing state.
    pub status: ProcessingStatus,
    /// Audio duration in seconds.
    pub duration_seconds: Option<f64>,
    /// When the recording was created.
    pub created_at: DateTime<Utc>,
    /// User-assigned tags.
    pub tags: Vec<String>,
    /// Whether a transcript exists (without loading it).
    pub has_transcript: bool,
    /// Whether a SOAP note exists (without loading it).
    pub has_soap_note: bool,
    /// Whether a referral letter exists (without loading it).
    pub has_referral: bool,
    /// Whether a patient letter exists (without loading it).
    pub has_letter: bool,
    /// Whether a peer discussion note exists (without loading it).
    pub has_peer_discussion: bool,
    /// True if this recording was synced from a remote machine (metadata
    /// contains a `synced_from` key).
    pub is_remote: bool,
}

impl From<&Recording> for RecordingSummary {
    fn from(r: &Recording) -> Self {
        Self {
            id: r.id,
            filename: r.filename.clone(),
            patient_name: r.patient_name.clone(),
            status: r.status.clone(),
            duration_seconds: r.duration_seconds,
            created_at: r.created_at,
            tags: r.tags.clone(),
            has_transcript: r.transcript.is_some(),
            has_soap_note: r.soap_note.is_some(),
            has_referral: r.referral.is_some(),
            has_letter: r.letter.is_some(),
            has_peer_discussion: r.peer_discussion.is_some(),
            is_remote: r.metadata.get("synced_from").is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_recording_starts_pending() {
        let rec = Recording::new("test.wav", PathBuf::from("/audio/test.wav"));
        assert!(matches!(rec.status, ProcessingStatus::Pending));
        assert!(!rec.is_processed());
        assert!(!rec.has_transcript());
        assert!(!rec.has_soap_note());
    }

    #[test]
    fn processing_status_terminal_states() {
        let completed = ProcessingStatus::Completed {
            completed_at: Utc::now(),
        };
        assert!(completed.is_terminal());

        let failed_max = ProcessingStatus::Failed {
            error: "boom".into(),
            retry_count: 3,
        };
        assert!(failed_max.is_terminal());

        let failed_once = ProcessingStatus::Failed {
            error: "boom".into(),
            retry_count: 1,
        };
        assert!(!failed_once.is_terminal());

        let pending = ProcessingStatus::Pending;
        assert!(!pending.is_terminal());
    }

    #[test]
    fn retry_logic() {
        let retryable = ProcessingStatus::Failed {
            error: "err".into(),
            retry_count: 2,
        };
        assert!(retryable.can_retry());

        let exhausted = ProcessingStatus::Failed {
            error: "err".into(),
            retry_count: 3,
        };
        assert!(!exhausted.can_retry());

        assert!(!ProcessingStatus::Pending.can_retry());
        assert!(
            !ProcessingStatus::Completed {
                completed_at: Utc::now()
            }
            .can_retry()
        );
    }

    #[test]
    fn serializes_with_tag() {
        let status = ProcessingStatus::Pending;
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["status"], "pending");

        let processing = ProcessingStatus::Processing {
            started_at: Utc::now(),
        };
        let json = serde_json::to_value(&processing).unwrap();
        assert_eq!(json["status"], "processing");
        assert!(json["started_at"].is_string());

        let failed = ProcessingStatus::Failed {
            error: "oops".into(),
            retry_count: 1,
        };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["error"], "oops");
        assert_eq!(json["retry_count"], 1);
    }

    #[test]
    fn summary_from_recording() {
        let mut rec = Recording::new("visit.wav", PathBuf::from("/audio/visit.wav"));
        rec.transcript = Some("Hello".into());
        rec.soap_note = Some("S: ....".into());
        rec.patient_name = Some("Jane Doe".into());

        let summary = RecordingSummary::from(&rec);
        assert_eq!(summary.filename, "visit.wav");
        assert!(summary.has_transcript);
        assert!(summary.has_soap_note);
        assert!(!summary.has_referral);
        assert!(!summary.has_letter);
        assert_eq!(summary.patient_name.as_deref(), Some("Jane Doe"));
    }

    #[test]
    fn status_labels() {
        assert_eq!(ProcessingStatus::Pending.status_label(), "Pending");
        assert_eq!(
            ProcessingStatus::Processing {
                started_at: Utc::now()
            }
            .status_label(),
            "Processing"
        );
        assert_eq!(
            ProcessingStatus::Completed {
                completed_at: Utc::now()
            }
            .status_label(),
            "Completed"
        );
        assert_eq!(
            ProcessingStatus::Failed {
                error: "e".into(),
                retry_count: 0
            }
            .status_label(),
            "Failed"
        );
    }
}
