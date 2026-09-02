//! Recording and processing-status types.

use super::ai::UsageInfo;
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
///
/// # PHI note
///
/// Manual `Debug` impl redacts `patient_name`; the remaining fields are
/// structural metadata safe to log.
#[derive(Clone, Serialize, Deserialize)]
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
    /// Throughput (tokens/sec) of the most recent AI generation for this
    /// recording, from `metadata.generation_stats` — `None` when no
    /// generation has recorded stats.
    #[serde(default)]
    pub tokens_per_second: Option<f64>,
}

/// Manual Debug impl that redacts the `patient_name` field. All other
/// fields are structural metadata (no PHI) and are logged verbatim.
impl std::fmt::Debug for RecordingSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingSummary")
            .field("id", &self.id)
            .field("filename", &self.filename)
            .field("patient_name", &"<redacted>")
            .field("status", &self.status)
            .field("duration_seconds", &self.duration_seconds)
            .field("created_at", &self.created_at)
            .field("tags", &self.tags)
            .field("has_transcript", &self.has_transcript)
            .field("has_soap_note", &self.has_soap_note)
            .field("has_referral", &self.has_referral)
            .field("has_letter", &self.has_letter)
            .field("has_peer_discussion", &self.has_peer_discussion)
            .field("is_remote", &self.is_remote)
            .field("tokens_per_second", &self.tokens_per_second)
            .finish()
    }
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
            tokens_per_second: latest_tokens_per_second(&r.metadata),
        }
    }
}

/// Throughput metrics for a single LLM generation, persisted under
/// `recording.metadata["generation_stats"][doc_type]`.
///
/// Contains only counts, durations, and provider/model names — no PHI
/// (AGENTS.md: log counts and lengths, never content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationStat {
    /// Provider that produced the generation (e.g. `"ollama"`, `"lmstudio"`).
    pub provider: String,
    /// Model used for the generation.
    pub model: String,
    /// Tokens consumed by the prompt (input).
    pub prompt_tokens: u32,
    /// Tokens produced by the completion (output).
    pub completion_tokens: u32,
    /// Wall-clock duration of the completion call, in milliseconds (truncated;
    /// a sub-millisecond call records 0).
    pub duration_ms: u64,
    /// Effective throughput: the server-reported decode-phase rate when the
    /// server provides one (oMLX reports `generation_tokens_per_second`),
    /// else completion tokens divided by wall-clock seconds. The decode-phase
    /// rate excludes prompt evaluation and matches inference-server
    /// dashboards, so recorded stats agree with what the server shows.
    pub tokens_per_second: f64,
    /// When the generation completed.
    pub generated_at: DateTime<Utc>,
}

impl GenerationStat {
    /// Compute a stat from a completion response's usage plus the
    /// wall-clock time spent in `provider.complete()`.
    ///
    /// Returns `None` when no throughput can be derived (zero completion
    /// tokens or zero elapsed time) — nothing should be recorded then.
    pub fn from_completion(
        provider: &str,
        model: &str,
        usage: &UsageInfo,
        elapsed: std::time::Duration,
    ) -> Option<Self> {
        if usage.completion_tokens == 0 || elapsed.is_zero() {
            return None;
        }
        let seconds = elapsed.as_secs_f64();
        let tokens_per_second = match usage.decode_tokens_per_second {
            // Guard against malformed server reports (NaN / negative / inf).
            Some(r) if r.is_finite() && r > 0.0 => r,
            _ => usage.completion_tokens as f64 / seconds,
        };
        Some(Self {
            provider: provider.to_string(),
            model: model.to_string(),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            duration_ms: elapsed.as_millis() as u64,
            tokens_per_second,
            generated_at: Utc::now(),
        })
    }
}

/// Doc-type keys that may appear under `generation_stats`.
pub const GENERATION_STAT_DOC_TYPES: [&str; 5] =
    ["soap", "referral", "letter", "synopsis", "peer_discussion"];

/// Merge `stat` into `metadata["generation_stats"][doc_type]`, creating the
/// nested object when absent. Never touches any other metadata key.
pub fn merge_generation_stat(
    metadata: &mut serde_json::Value,
    doc_type: &str,
    stat: GenerationStat,
) {
    debug_assert!(
        GENERATION_STAT_DOC_TYPES.contains(&doc_type),
        "unknown generation-stats doc type: {doc_type}"
    );
    if !metadata.is_object() {
        *metadata = serde_json::json!({});
    }
    let obj = metadata
        .as_object_mut()
        .expect("replaced with an object above");
    let stats = obj
        .entry("generation_stats".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !stats.is_object() {
        *stats = serde_json::json!({});
    }
    let stats_obj = stats
        .as_object_mut()
        .expect("replaced with an object above");
    stats_obj.insert(
        doc_type.to_string(),
        serde_json::to_value(stat).expect("GenerationStat serializes infallibly"),
    );
}

/// The `tokens_per_second` of the most recent generation across doc types
/// (newest `generated_at`), or `None` when no valid stats are recorded.
/// Entries that fail to deserialize as [`GenerationStat`] are skipped.
pub fn latest_tokens_per_second(metadata: &serde_json::Value) -> Option<f64> {
    let stats = metadata.get("generation_stats")?;
    let mut best: Option<(DateTime<Utc>, f64)> = None;
    for key in GENERATION_STAT_DOC_TYPES {
        let Some(raw) = stats.get(key) else { continue };
        let Ok(stat) = serde_json::from_value::<GenerationStat>(raw.clone()) else {
            continue;
        };
        if best.is_none_or(|(best_at, _)| stat.generated_at >= best_at) {
            best = Some((stat.generated_at, stat.tokens_per_second));
        }
    }
    best.map(|(_, tokens_per_second)| tokens_per_second)
}

/// Record a completion's throughput stat into `metadata` under `doc_type`:
/// derive the [`GenerationStat`] (a no-op when no throughput can be
/// computed), log it at debug level (counts and durations only — never
/// content), and merge it. Best-effort by construction — never fails.
pub fn record_completion_stat(
    metadata: &mut serde_json::Value,
    doc_type: &'static str,
    provider: &str,
    model: &str,
    usage: &UsageInfo,
    elapsed: std::time::Duration,
) {
    debug_assert!(
        GENERATION_STAT_DOC_TYPES.contains(&doc_type),
        "unknown generation-stats doc type: {doc_type}"
    );
    let Some(stat) = GenerationStat::from_completion(provider, model, usage, elapsed) else {
        return;
    };
    tracing::debug!(
        doc_type,
        tokens_per_second = stat.tokens_per_second,
        completion_tokens = stat.completion_tokens,
        duration_ms = stat.duration_ms,
        "generation throughput recorded"
    );
    merge_generation_stat(metadata, doc_type, stat);
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
    fn generation_stat_from_completion_computes_throughput() {
        let usage = UsageInfo {
            prompt_tokens: 1000,
            completion_tokens: 200,
            total_tokens: 1200,
            decode_tokens_per_second: None,
        };
        let stat = GenerationStat::from_completion(
            "ollama",
            "llama3",
            &usage,
            std::time::Duration::from_millis(4000),
        )
        .expect("throughput is computable");
        assert_eq!(stat.provider, "ollama");
        assert_eq!(stat.model, "llama3");
        assert_eq!(stat.prompt_tokens, 1000);
        assert_eq!(stat.completion_tokens, 200);
        assert_eq!(stat.duration_ms, 4000);
        assert_eq!(stat.tokens_per_second, 50.0);
    }

    // oMLX reports its decode-phase rate in the usage event; the recorded
    // stat must match that (dashboard parity) instead of diluting it with
    // prompt-eval wall time.
    #[test]
    fn generation_stat_prefers_server_decode_rate() {
        let usage = UsageInfo {
            prompt_tokens: 6000,
            completion_tokens: 6800,
            total_tokens: 12800,
            decode_tokens_per_second: Some(84.0),
        };
        let stat = GenerationStat::from_completion(
            "omlx",
            "Ornith-1.5-35B",
            &usage,
            std::time::Duration::from_millis(101_000),
        )
        .expect("throughput is computable");
        assert_eq!(stat.tokens_per_second, 84.0);
        assert_eq!(stat.completion_tokens, 6800);
        assert_eq!(stat.duration_ms, 101_000, "duration stays wall-clock");
    }

    // A malformed server report (zero/NaN/negative) must not corrupt the
    // stat — fall back to the wall-clock computation.
    #[test]
    fn generation_stat_ignores_malformed_decode_rate() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let usage = UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 100,
                total_tokens: 110,
                decode_tokens_per_second: Some(bad),
            };
            let stat = GenerationStat::from_completion(
                "omlx",
                "m",
                &usage,
                std::time::Duration::from_secs(2),
            )
            .expect("throughput is computable");
            assert_eq!(stat.tokens_per_second, 50.0, "fallback for {bad}");
        }
    }

    #[test]
    fn generation_stat_from_completion_rejects_zero_completion_tokens() {
        let usage = UsageInfo::default(); // completion_tokens == 0
        assert!(
            GenerationStat::from_completion(
                "ollama",
                "llama3",
                &usage,
                std::time::Duration::from_secs(1)
            )
            .is_none()
        );
    }

    #[test]
    fn generation_stat_from_completion_rejects_zero_elapsed() {
        let usage = UsageInfo {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            decode_tokens_per_second: None,
        };
        assert!(
            GenerationStat::from_completion("ollama", "llama3", &usage, std::time::Duration::ZERO)
                .is_none()
        );
    }

    fn stat(tokens_per_second: f64, generated_at: chrono::DateTime<Utc>) -> GenerationStat {
        GenerationStat {
            provider: "ollama".to_string(),
            model: "llama3".to_string(),
            prompt_tokens: 10,
            completion_tokens: 100,
            duration_ms: 1000,
            tokens_per_second,
            generated_at,
        }
    }

    #[test]
    fn merge_generation_stat_overwrites_own_slot_only() {
        let mut metadata = serde_json::json!({ "context": "visit notes" });
        merge_generation_stat(&mut metadata, "soap", stat(20.0, Utc::now()));

        assert_eq!(metadata["context"], serde_json::json!("visit notes"));
        assert_eq!(
            metadata["generation_stats"]["soap"]["tokens_per_second"],
            serde_json::json!(20.0)
        );

        merge_generation_stat(&mut metadata, "referral", stat(150.0, Utc::now()));
        merge_generation_stat(&mut metadata, "soap", stat(75.5, Utc::now()));

        // soap slot overwritten by its newest write; referral slot preserved;
        // unrelated metadata keys untouched.
        assert_eq!(
            metadata["generation_stats"]["soap"]["tokens_per_second"],
            serde_json::json!(75.5)
        );
        assert_eq!(
            metadata["generation_stats"]["referral"]["tokens_per_second"],
            serde_json::json!(150.0)
        );
        assert_eq!(metadata["context"], serde_json::json!("visit notes"));
    }

    #[test]
    fn merge_generation_stat_initializes_null_metadata() {
        let mut metadata = serde_json::Value::Null;
        merge_generation_stat(&mut metadata, "soap", stat(20.0, Utc::now()));
        assert!(metadata["generation_stats"]["soap"].is_object());
    }

    #[test]
    fn latest_tokens_per_second_picks_newest_generated_at() {
        let older_at = Utc::now() - chrono::TimeDelta::hours(2);
        let mut metadata = serde_json::json!({});
        merge_generation_stat(&mut metadata, "soap", stat(20.0, older_at));
        merge_generation_stat(&mut metadata, "letter", stat(75.5, Utc::now()));

        assert_eq!(latest_tokens_per_second(&metadata), Some(75.5));
    }

    #[test]
    fn latest_tokens_per_second_none_without_stats() {
        assert_eq!(latest_tokens_per_second(&serde_json::Value::Null), None);
        assert_eq!(
            latest_tokens_per_second(&serde_json::json!({ "context": "x" })),
            None
        );
    }

    #[test]
    fn record_completion_stat_writes_throughput() {
        let mut metadata = serde_json::json!({ "context": "visit notes" });
        let usage = UsageInfo {
            prompt_tokens: 10,
            completion_tokens: 50,
            total_tokens: 60,
            decode_tokens_per_second: None,
        };
        record_completion_stat(
            &mut metadata,
            "soap",
            "ollama",
            "llama3",
            &usage,
            std::time::Duration::from_millis(1000),
        );
        assert_eq!(
            metadata["generation_stats"]["soap"]["tokens_per_second"],
            serde_json::json!(50.0)
        );
        assert_eq!(metadata["context"], serde_json::json!("visit notes"));
    }

    #[test]
    fn record_completion_stat_skips_zero_token_completions() {
        let mut metadata = serde_json::json!({});
        let usage = UsageInfo::default();
        record_completion_stat(
            &mut metadata,
            "soap",
            "ollama",
            "llama3",
            &usage,
            std::time::Duration::from_secs(1),
        );
        assert!(metadata.get("generation_stats").is_none());
    }

    #[test]
    fn latest_tokens_per_second_skips_malformed_entries() {
        let metadata = serde_json::json!({
            "generation_stats": {
                "soap": { "tokens_per_second": 99.0 },
                "referral": stat(40.0, Utc::now())
            }
        });
        // "soap" is missing required fields → skipped; the valid referral
        // entry wins despite the lower value.
        assert_eq!(latest_tokens_per_second(&metadata), Some(40.0));
    }

    #[test]
    fn summary_tokens_per_second_from_metadata() {
        let mut rec = Recording::new("visit.wav", PathBuf::from("/audio/visit.wav"));
        rec.transcript = Some("Hello".into());

        // No stats recorded yet → None.
        assert_eq!(RecordingSummary::from(&rec).tokens_per_second, None);

        let older_at = Utc::now() - chrono::TimeDelta::hours(1);
        merge_generation_stat(&mut rec.metadata, "soap", stat(50.0, older_at));
        merge_generation_stat(&mut rec.metadata, "referral", stat(100.0, Utc::now()));

        let summary = RecordingSummary::from(&rec);
        assert_eq!(summary.tokens_per_second, Some(100.0));
    }

    #[test]
    fn merge_generation_stat_resets_corrupt_stats_blob() {
        let mut metadata = serde_json::json!({ "generation_stats": "corrupt" });
        merge_generation_stat(&mut metadata, "soap", stat(20.0, Utc::now()));
        assert_eq!(
            metadata["generation_stats"]["soap"]["tokens_per_second"],
            serde_json::json!(20.0)
        );
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
