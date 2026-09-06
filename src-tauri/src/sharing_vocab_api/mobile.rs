//! Mobile-client API surface: recording ingestion, processing-job status,
//! per-doc-type generation triggers, document read/write, document export,
//! and device self-revocation.
//!
//! Routes (all bearer-gated against the same `TokenStore` as the rest of
//! the :11437 data API):
//!   POST   /v1/recordings                                — create a recording row
//!   GET    /v1/jobs/{recording_id}                       — current job snapshot (JSON)
//!   GET    /v1/jobs/{recording_id}/events                — SSE stage stream
//!   GET    /v1/recordings/{id}/documents/{doc_type}      — fetch one document field
//!   PUT    /v1/recordings/{id}/documents/{doc_type}      — save an edited document
//!   GET    /v1/recordings/{id}/export?format&doc_type    — PDF/DOCX bytes
//!   POST   /v1/devices/self                              — revoke the calling token
//!   POST   /v1/recordings/{id}/generate/{doc_type}       — Wry-only (see below)
//!
//! Audio ingestion intentionally reuses the existing
//! `PUT /v1/content/audio/{recording_id}` endpoint (plaintext bytes,
//! server-side FE1 at-rest encryption, first-write-wins 409) rather than
//! adding a multipart path — one battle-tested audio path, no duplication
//! of the encrypt-in-memory/atomic-rename machinery.
//!
//! Job model: an in-memory registry keyed by recording ID, fed by the SAME
//! `pipeline-progress` / `generation-progress` Tauri events the desktop
//! frontend consumes (attached via [`attach_event_forwarders`]). Stage
//! vocabulary: `queued → transcribing → generating_soap → completed|failed`
//! for the base pipeline; `generating_{referral|letter|synopsis|
//! peer_discussion}` for per-doc-type triggers. Snapshots are ephemeral —
//! they describe in-flight work only; durable state lives on the recording
//! row (and travels via content sync).
//!
//! PHI discipline: SSE payloads carry recording_id/stage/updated_at ONLY —
//! no error text, no names, no content (the mobile client refetches content
//! through the authenticated document endpoints). GET job snapshots include
//! the technical error string (authenticated request, aids debugging).
//! Logs carry IDs, doc types, stages, and byte counts only.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use futures_util::Stream;
use medical_core::error::AppError;
use medical_core::types::PatientContext;
use medical_core::types::recording::Recording;
use medical_db::recordings::RecordingsRepo;
use serde::Deserialize;
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use super::{ApiState, authorize};

// ---------------------------------------------------------------------------
// Doc types
// ---------------------------------------------------------------------------

/// The five document types the mobile app can read, write, generate, and
/// export. Mirrors the generation commands + exporters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DocType {
    Soap,
    Referral,
    Letter,
    Synopsis,
    PeerDiscussion,
}

impl DocType {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            DocType::Soap => "soap",
            DocType::Referral => "referral",
            DocType::Letter => "letter",
            DocType::Synopsis => "synopsis",
            DocType::PeerDiscussion => "peer_discussion",
        }
    }

    /// The recordings-table column (or metadata key for synopsis) this doc
    /// type persists to.
    fn field_name(self) -> &'static str {
        match self {
            DocType::Soap => "soap_note",
            DocType::Referral => "referral",
            DocType::Letter => "letter",
            DocType::Synopsis => "metadata", // synopsis rides metadata
            DocType::PeerDiscussion => "peer_discussion",
        }
    }
}

/// Accepted `doc_type` path-segment values.
pub(super) fn parse_doc_type(s: &str) -> Option<DocType> {
    match s {
        "soap" => Some(DocType::Soap),
        "referral" => Some(DocType::Referral),
        "letter" => Some(DocType::Letter),
        "synopsis" => Some(DocType::Synopsis),
        "peer_discussion" => Some(DocType::PeerDiscussion),
        _ => None,
    }
}

/// Character cap for document PUT bodies. Mirrors the desktop editor's
/// per-field caps (`commands::recordings_edit::max_chars_for_field`).
const MAX_DOC_CHARS: usize = 500_000;
/// Character cap for a recording filename on create.
const MAX_FILENAME_CHARS: usize = 255;

/// Read the stored synopsis off a recording's metadata.
///
/// Mirrors the medical-export crate's reader: the synopsis has no dedicated
/// column; `generate_synopsis` persists it as a plain string under
/// `metadata.synopsis`. An empty string counts as absent.
fn synopsis_of(rec: &Recording) -> Option<&str> {
    rec.metadata
        .get("synopsis")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Extract the current content for a doc type from a recording row.
fn document_content(rec: &Recording, doc: DocType) -> Option<&str> {
    match doc {
        DocType::Soap => rec.soap_note.as_deref(),
        DocType::Referral => rec.referral.as_deref(),
        DocType::Letter => rec.letter.as_deref(),
        DocType::Synopsis => synopsis_of(rec),
        DocType::PeerDiscussion => rec.peer_discussion.as_deref(),
    }
}

// ---------------------------------------------------------------------------
// Job registry
// ---------------------------------------------------------------------------

/// A single job's current state. `stage` uses the fixed vocabulary described
/// in the module docs; `error` carries the technical failure text (GET only,
/// never SSE).
#[derive(Debug, Clone, Serialize)]
pub(super) struct JobSnapshot {
    pub(super) recording_id: String,
    pub(super) stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    pub(super) updated_at: String,
}

/// Wire shape of a `pipeline-progress` event (see `commands::pipeline`).
#[derive(Deserialize)]
struct PipelineProgressWire {
    recording_id: String,
    stage: String,
    error: Option<String>,
}

/// Wire shape of a `generation-progress` event (see
/// `commands::generation::helpers::run_generation_command`). `progress`
/// (live streaming stats) is deliberately ignored — stage labels only.
#[derive(Deserialize)]
struct GenerationProgressWire {
    #[serde(rename = "type")]
    doc_type: String,
    status: String,
    recording_id: String,
}

/// In-memory job registry keyed by recording ID.
///
/// Updated from two sources:
/// 1. Live Tauri events (pipeline-progress / generation-progress) via
///    [`attach_event_forwarders`] — authoritative while a command runs.
/// 2. The generation HTTP handler: `queued` on accept, plus a safety-net
///    terminal state applied only when no live event fired (a command that
///    bails before its first emit would otherwise strand the job at
///    `queued` forever).
///
/// Sequence numbers make the safety net race-free: a terminal mark captured
/// at queue time is dropped if anything newer was recorded since (a newer
/// job for the same recording must not be clobbered by a slow predecessor).
pub(crate) struct JobRegistry {
    inner: std::sync::Mutex<HashMap<String, (u64, JobSnapshot)>>,
    next_seq: std::sync::atomic::AtomicU64,
    changed: tokio::sync::broadcast::Sender<String>,
}

impl JobRegistry {
    pub(super) fn new() -> Self {
        let (changed, _) = tokio::sync::broadcast::channel::<String>(64);
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
            next_seq: std::sync::atomic::AtomicU64::new(1),
            changed,
        }
    }

    /// Record a stage transition unconditionally. Returns the sequence
    /// number for later [`Self::mark_if_current`] calls.
    pub(super) fn mark(&self, recording_id: &str, stage: &str, error: Option<String>) -> u64 {
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.store(recording_id, stage, error, None, seq);
        seq
    }

    /// Apply a terminal update only when nothing newer was recorded since
    /// `seq` — i.e. the live events stayed silent and the queued snapshot is
    /// still the latest word on this job.
    pub(super) fn mark_if_current(
        &self,
        recording_id: &str,
        seq: u64,
        stage: &str,
        error: Option<String>,
    ) {
        self.store(recording_id, stage, error, Some(seq), seq);
    }

    /// Single-lock insert. `expect = Some(seq)` gates on the current entry's
    /// sequence; `None` inserts unconditionally. A poisoned lock is
    /// recovered rather than dropped — progress events must not vanish.
    fn store(
        &self,
        recording_id: &str,
        stage: &str,
        error: Option<String>,
        expect: Option<u64>,
        seq: u64,
    ) {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(expect) = expect
            && match guard.get(recording_id) {
                Some((current, _)) => *current != expect,
                None => true,
            }
        {
            return; // superseded by a newer mark
        }
        guard.insert(
            recording_id.to_string(),
            (
                seq,
                JobSnapshot {
                    recording_id: recording_id.to_string(),
                    stage: stage.to_string(),
                    error,
                    updated_at: Utc::now().to_rfc3339(),
                },
            ),
        );
        drop(guard);
        // Best-effort: no SSE subscribers is not an error.
        let _ = self.changed.send(recording_id.to_string());
    }

    pub(super) fn get(&self, recording_id: &str) -> Option<JobSnapshot> {
        let guard = self.inner.lock().ok()?;
        guard.get(recording_id).map(|(_, snap)| snap.clone())
    }

    pub(super) fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.changed.subscribe()
    }
}

/// Translate a `generation-progress` status into a job stage.
///
/// `started` maps to `generating_{doc_type}` (matching the pipeline's
/// `generating_soap` vocabulary); `completed` passes through; `failed: …`
/// (from `format_progress_error`) becomes stage `failed` with the message
/// detached. Any other status (live streaming stats) is ignored.
fn map_generation_event(wire: &GenerationProgressWire) -> Option<(String, Option<String>)> {
    match wire.status.as_str() {
        "started" => Some((format!("generating_{}", wire.doc_type), None)),
        "completed" => Some(("completed".to_string(), None)),
        s if s.starts_with("failed") => {
            let msg = s.strip_prefix("failed:").unwrap_or(s).trim();
            Some((
                "failed".to_string(),
                (!msg.is_empty()).then(|| msg.to_string()),
            ))
        }
        _ => None,
    }
}

/// Bridge the desktop's Tauri progress events into the job registry.
///
/// Registered once per vocab-API lifetime (server start) and mirrored in
/// route tests. Listening to the same events the desktop frontend consumes
/// means HTTP-triggered and desktop-triggered work are indistinguishable to
/// the registry — one job vocabulary, both entry points.
pub(super) fn attach_event_forwarders<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    jobs: &Arc<JobRegistry>,
) {
    use tauri::Listener as _;

    let j = Arc::clone(jobs);
    app_handle.listen_any("pipeline-progress", move |event| {
        let Ok(wire) = serde_json::from_str::<PipelineProgressWire>(event.payload()) else {
            return;
        };
        j.mark(&wire.recording_id, &wire.stage, wire.error);
    });

    let j = Arc::clone(jobs);
    app_handle.listen_any("generation-progress", move |event| {
        let Ok(wire) = serde_json::from_str::<GenerationProgressWire>(event.payload()) else {
            return;
        };
        if let Some((stage, error)) = map_generation_event(&wire) {
            j.mark(&wire.recording_id, &stage, error);
        }
    });
}

// ---------------------------------------------------------------------------
// POST /v1/recordings — create a recording row
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct CreateRecordingRequest {
    /// Optional client-minted UUID (the mobile app creates recordings
    /// offline-first). Server-generated when absent.
    id: Option<String>,
    filename: String,
    duration_seconds: Option<f64>,
    patient_name: Option<String>,
}

#[derive(Serialize)]
pub(super) struct CreateRecordingResponse {
    id: String,
    created_at: String,
}

/// Create a recording metadata row (audio arrives separately via the
/// existing `PUT /v1/content/audio/{id}`).
///
/// 201 with the row's id/created_at on success; 400 on validation failure;
/// 409 when a client-supplied id already exists (including soft-deleted
/// rows — a resurrected id would corrupt sync tombstone semantics).
pub(super) async fn create_recording_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Json(req): Json<CreateRecordingRequest>,
) -> Result<(StatusCode, Json<CreateRecordingResponse>), StatusCode> {
    let _ = authorize(&state, &headers)?;

    let filename = req.filename.trim().to_string();
    if filename.is_empty() || filename.chars().count() > MAX_FILENAME_CHARS {
        warn!("mobile: create recording rejected, invalid filename length");
        return Err(StatusCode::BAD_REQUEST);
    }
    if let Some(d) = req.duration_seconds
        && !(d.is_finite() && d >= 0.0)
    {
        warn!("mobile: create recording rejected, invalid duration");
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = match req.id.as_deref() {
        None => Uuid::new_v4(),
        Some(s) => Uuid::parse_str(s).map_err(|_| {
            warn!("mobile: create recording rejected, malformed id");
            StatusCode::BAD_REQUEST
        })?,
    };

    let db = Arc::clone(&state.db);
    let rec = tokio::task::spawn_blocking(move || -> Result<Recording, StatusCode> {
        let conn = db.conn().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let existing: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recordings WHERE id = ?1",
                [&id.to_string()],
                |row| row.get(0),
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if existing > 0 {
            return Err(StatusCode::CONFLICT);
        }
        let mut rec = Recording::new(filename, std::path::PathBuf::new());
        rec.id = id;
        rec.duration_seconds = req.duration_seconds;
        rec.patient_name = req.patient_name;
        RecordingsRepo::insert(&conn, &rec).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(rec)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // Let other clients (SSE) and this server's own Recordings view know a
    // row landed — same fan-out the audio PUT performs.
    let _ = state.content_changed_tx.send(rec.id.to_string());
    use tauri::Emitter as _;
    let _ = state.app_handle.emit(
        "recording-updated",
        serde_json::json!({ "id": rec.id.to_string() }),
    );

    info!(recording_id = %rec.id, "mobile: recording created");
    Ok((
        StatusCode::CREATED,
        Json(CreateRecordingResponse {
            id: rec.id.to_string(),
            created_at: rec.created_at.to_rfc3339(),
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET /v1/jobs/{recording_id} + /events — job status
// ---------------------------------------------------------------------------

/// Current job snapshot as JSON. 404 when no job is known for the recording
/// (never started, or server restarted — durable state is the recording
/// row, not this registry).
pub(super) async fn job_status_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Path(recording_id): Path<String>,
) -> Result<Json<JobSnapshot>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    match state.jobs.get(&recording_id) {
        Some(snap) => {
            info!(recording_id = %recording_id, stage = %snap.stage, "mobile: job status");
            Ok(Json(snap))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// SSE stream of job-stage changes for one recording.
///
/// Payloads are IDs + stage labels + timestamps ONLY — no error text, no
/// content (module PHI discipline). The initial event carries the current
/// snapshot (or `unknown` when nothing is known yet) so a freshly connected
/// client doesn't wait a round-trip for state.
pub(super) async fn job_events_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Path(recording_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let mut rx = state.jobs.subscribe();
    let jobs = Arc::clone(&state.jobs);
    let rid = recording_id.clone();

    let stream = async_stream::stream! {
        match jobs.get(&rid) {
            Some(snap) => yield Ok(job_event(&snap)),
            None => yield Ok(Event::default().data(
                serde_json::json!({ "recording_id": rid, "stage": "unknown" }).to_string(),
            )),
        }
        loop {
            match rx.recv().await {
                Ok(id) if id == rid => {
                    if let Some(snap) = jobs.get(&rid) {
                        yield Ok(job_event(&snap));
                    }
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// SSE event payload — recording_id, stage, updated_at only. No error field.
fn job_event(snap: &JobSnapshot) -> Event {
    Event::default().data(
        serde_json::json!({
            "recording_id": snap.recording_id,
            "stage": snap.stage,
            "updated_at": snap.updated_at,
        })
        .to_string(),
    )
}

// ---------------------------------------------------------------------------
// GET/PUT /v1/recordings/{id}/documents/{doc_type}
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub(super) struct DocumentResponse {
    doc_type: String,
    content: Option<String>,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct DocumentPutRequest {
    content: String,
}

/// Fetch one document field. 404 for unknown recordings; `content` is null
/// when the document has not been generated yet.
pub(super) async fn document_get_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Path((recording_id, doc_type)): Path<(String, String)>,
) -> Result<Json<DocumentResponse>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let doc = parse_doc_type(&doc_type).ok_or(StatusCode::BAD_REQUEST)?;
    let uuid = Uuid::parse_str(&recording_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = Arc::clone(&state.db);

    let out = tokio::task::spawn_blocking(move || -> Result<DocumentResponse, StatusCode> {
        let conn = db.conn().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let rec = RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| match AppError::from(e) {
            AppError::Database { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
        let content = document_content(&rec, doc).map(|s| s.to_string());
        // Field revision when one exists (per-field LWW stamp), else the
        // row stamp — same precedence the sync wire builder uses.
        let updated_at = medical_db::ContentSyncRepo::revisions_for(&conn, &uuid)
            .ok()
            .and_then(|revs| {
                revs.iter()
                    .find(|r| r.field == doc.field_name())
                    .map(|r| r.updated_at.clone())
            })
            .or_else(|| rec.updated_at.map(|dt| dt.to_rfc3339()));
        Ok(DocumentResponse {
            doc_type: doc.as_str().to_string(),
            content,
            updated_at,
        })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    info!(recording_id = %recording_id, doc_type = doc.as_str(), "mobile: document get");
    Ok(Json(out))
}

/// Save an edited document.
///
/// Column-backed doc types reuse `save_recording_field_inner` — the exact
/// desktop editor path (whitelist, caps, targeted column update, `updated_at`
/// bump, per-field sync revision, training-corpus hook) so mobile edits and
/// desktop edits converge under the same LWW semantics. Synopsis (metadata-
/// backed) persists via a producer patch on `metadata.synopsis` plus a
/// `metadata` field revision.
pub(super) async fn document_put_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Path((recording_id, doc_type)): Path<(String, String)>,
    Json(req): Json<DocumentPutRequest>,
) -> Result<StatusCode, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let doc = parse_doc_type(&doc_type).ok_or(StatusCode::BAD_REQUEST)?;
    Uuid::parse_str(&recording_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    if !req.content.is_empty() && req.content.chars().count() > MAX_DOC_CHARS {
        warn!("mobile: document put rejected, over cap");
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let db = Arc::clone(&state.db);
    let field = doc.field_name().to_string();
    let value = req.content;
    let rid = recording_id.clone();
    tokio::task::spawn_blocking(move || -> Result<(), StatusCode> {
        let conn = db.conn().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let result = if doc == DocType::Synopsis {
            persist_synopsis(&db, &conn, &rid, &value)
        } else {
            let capture = medical_db::settings::SettingsRepo::load_config(&conn)
                .unwrap_or_default()
                .capture_for_training;
            crate::commands::recordings_edit::save_recording_field_inner(
                Arc::clone(&db),
                &conn,
                &rid,
                &field,
                &value,
                capture,
            )
        };
        result.map_err(|e| match e {
            AppError::Database { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // Same fan-out the sync push performs: other clients + this server's
    // own Recordings/Editor views.
    let _ = state.content_changed_tx.send(recording_id.clone());
    use tauri::Emitter as _;
    let _ = state.app_handle.emit(
        "recording-updated",
        serde_json::json!({ "id": recording_id }),
    );

    info!(recording_id = %recording_id, doc_type = doc.as_str(), "mobile: document put");
    Ok(StatusCode::NO_CONTENT)
}

/// Persist a synopsis edit: merge `metadata.synopsis` into the row's CURRENT
/// metadata (producer patch — one-level merge preserves sibling keys like
/// generation_stats), then stamp a `metadata` field revision for sync.
fn persist_synopsis(
    db: &Arc<medical_db::Database>,
    conn: &medical_db::Connection,
    recording_id: &str,
    value: &str,
) -> Result<(), AppError> {
    let uuid = Uuid::parse_str(recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let patch = if value.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(value.to_string())
    };
    RecordingsRepo::persist_producer_update(
        conn,
        &uuid,
        &medical_db::recordings::ProducerPersist {
            metadata_patch: vec![("synopsis".to_string(), patch)],
            ..Default::default()
        },
    )
    .map_err(AppError::from)?;
    let now = Utc::now().to_rfc3339();
    let _ = medical_db::ContentSyncRepo::upsert_revision(conn, &uuid, "metadata", &now, None);
    let _ = db; // reserved for symmetry with save_recording_field_inner
    Ok(())
}

// ---------------------------------------------------------------------------
// GET /v1/recordings/{id}/export — PDF/DOCX bytes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct ExportQuery {
    format: String,
    doc_type: String,
}

/// Stream an exported document. `format` ∈ {pdf, docx}; `doc_type` ∈ the
/// five doc types (synopsis + peer_discussion use the net-new exporters).
/// 404 when the recording (or the requested document content) is absent —
/// checked before dispatch so the exporters' internal missing-content
/// errors can't surface as 500s.
pub(super) async fn export_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Path(recording_id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let doc = parse_doc_type(&q.doc_type).ok_or(StatusCode::BAD_REQUEST)?;
    let pdf = match q.format.as_str() {
        "pdf" => true,
        "docx" => false,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let uuid = Uuid::parse_str(&recording_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = Arc::clone(&state.db);

    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, StatusCode> {
        let conn = db.conn().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let rec = RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| match AppError::from(e) {
            AppError::Database { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
        if document_content(&rec, doc).is_none() {
            return Err(StatusCode::NOT_FOUND);
        }
        let res = match (pdf, doc) {
            (true, DocType::Soap) => medical_export::pdf::PdfExporter::export_soap(&rec),
            (true, DocType::Referral) => medical_export::pdf::PdfExporter::export_referral(&rec),
            (true, DocType::Letter) => medical_export::pdf::PdfExporter::export_letter(&rec),
            (true, DocType::Synopsis) => medical_export::pdf::PdfExporter::export_synopsis(&rec),
            (true, DocType::PeerDiscussion) => {
                medical_export::pdf::PdfExporter::export_peer_discussion(&rec)
            }
            (false, DocType::Soap) => medical_export::docx::DocxExporter::export_soap(&rec),
            (false, DocType::Referral) => medical_export::docx::DocxExporter::export_referral(&rec),
            (false, DocType::Letter) => medical_export::docx::DocxExporter::export_letter(&rec),
            (false, DocType::Synopsis) => medical_export::docx::DocxExporter::export_synopsis(&rec),
            (false, DocType::PeerDiscussion) => {
                medical_export::docx::DocxExporter::export_peer_discussion(&rec)
            }
        };
        res.map_err(|e| {
            warn!(error = %e, "mobile: export render failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    let byte_count = bytes.len();
    let ext = if pdf { "pdf" } else { "docx" };
    let short_id: String = recording_id.chars().take(8).collect();
    let mut resp = bytes.into_response();
    let headers = resp.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(if pdf {
            "application/pdf"
        } else {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }),
    );
    let disposition = format!("attachment; filename=\"{}-{short_id}.{ext}\"", doc.as_str());
    if let Ok(v) = axum::http::HeaderValue::from_str(&disposition) {
        headers.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    info!(recording_id = %recording_id, doc_type = doc.as_str(), format = ext, byte_count, "mobile: export");
    Ok(resp)
}

// ---------------------------------------------------------------------------
// POST /v1/devices/self — self-revocation
// ---------------------------------------------------------------------------

/// Revoke the calling device's own token.
///
/// `POST /pair/revoke/:id` on :11436 is loopback-gated (desktop-admin only);
/// a lost phone cannot reach loopback. This endpoint lets ANY authenticated
/// client revoke exactly itself — the bearer token identifies the row, no
/// id is trusted from the request. 204 on success; the token 401s on every
/// subsequent request.
pub(super) async fn devices_self_revoke_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let client_id = authorize(&state, &headers)?;
    state.tokens.revoke(client_id).map_err(|e| {
        warn!(error = %e, "mobile: self-revoke failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    info!(client_id, "mobile: device self-revoked");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /v1/recordings/{id}/generate/{doc_type} — Wry-only route
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct GenerateRequest {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    patient_context: Option<PatientContext>,
    #[serde(default)]
    recipient_type: Option<String>,
    #[serde(default)]
    urgency: Option<String>,
    #[serde(default)]
    letter_type: Option<String>,
    #[serde(default)]
    audience_id: Option<Uuid>,
    #[serde(default)]
    physician_name: Option<String>,
    #[serde(default)]
    specialty: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// Runtime-independent validation for a generate request: doc-type parse,
/// peer-discussion required fields, recording existence/visibility, and
/// (for soap) audio presence. Returns the parsed doc type.
///
/// Factored out of the Wry-typed handler so the checks are route-testable
/// on MockRuntime.
async fn validate_generate_request<R: tauri::Runtime>(
    state: &ApiState<R>,
    recording_id: &str,
    doc_type: &str,
    req: &GenerateRequest,
) -> Result<DocType, StatusCode> {
    let doc = parse_doc_type(doc_type).ok_or(StatusCode::BAD_REQUEST)?;
    if doc == DocType::PeerDiscussion {
        let filled =
            |v: &Option<String>| v.as_deref().map(str::trim).is_some_and(|s| !s.is_empty());
        if !(filled(&req.physician_name) && filled(&req.specialty) && filled(&req.reason)) {
            warn!("mobile: peer_discussion generate rejected, missing required fields");
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    let uuid = Uuid::parse_str(recording_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> Result<(), StatusCode> {
        let conn = db.conn().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let visible: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recordings WHERE id = ?1 AND deleted_at IS NULL",
                [&uuid.to_string()],
                |row| row.get(0),
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if visible == 0 {
            return Err(StatusCode::NOT_FOUND);
        }
        if doc == DocType::Soap {
            // process_recording transcribes first — without audio it can
            // only fail. Reject with 409 before queueing.
            let rec = RecordingsRepo::get_by_id(&conn, &uuid)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if rec.audio_path.as_os_str().is_empty() || !rec.audio_path.exists() {
                warn!("mobile: soap generate rejected, no audio uploaded (409)");
                return Err(StatusCode::CONFLICT);
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    Ok(doc)
}

/// Register the generate route on the concrete (Wry) runtime.
///
/// The generation commands (`process_recording`, `generate_*`) are typed
/// `AppHandle<Wry>`, so only this handler needs the concrete runtime — it
/// is merged into the router in `spawn()` and excluded from the
/// MockRuntime route tests (its validation core is
/// [`validate_generate_request`], which IS route-tested).
pub(super) fn generate_route(state: ApiState<tauri::Wry>) -> axum::Router {
    axum::Router::new()
        .route(
            "/v1/recordings/{id}/generate/{doc_type}",
            axum::routing::post(generate_handler),
        )
        .with_state(state)
}

/// Queue a generation. 202 immediately; progress via `/v1/jobs`.
///
/// soap runs the full transcribe→SOAP pipeline (`process_recording`); the
/// other four call their generation commands directly. All of them report
/// through the same event-driven job registry.
async fn generate_handler(
    AxumState(state): AxumState<ApiState<tauri::Wry>>,
    headers: HeaderMap,
    Path((recording_id, doc_type)): Path<(String, String)>,
    Json(req): Json<GenerateRequest>,
) -> Result<StatusCode, StatusCode> {
    use tauri::Manager as _;

    let _ = authorize(&state, &headers)?;
    let doc = validate_generate_request(&state, &recording_id, &doc_type, &req).await?;

    // AppState is managed whenever the sharing lifecycle (which owns this
    // API) could have started it; guard anyway so a misordered startup
    // degrades to 503 instead of panicking inside `state()`.
    if state
        .app_handle
        .try_state::<crate::state::AppState>()
        .is_none()
    {
        warn!("mobile: generate rejected, AppState not managed (503)");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let seq = state.jobs.mark(&recording_id, "queued", None);
    let app = state.app_handle.clone();
    let jobs = Arc::clone(&state.jobs);
    let rid = recording_id.clone();

    tokio::spawn(async move {
        // State borrowed from the owned handle keeps the future 'static.
        let st = app.state::<crate::state::AppState>();
        let result = match doc {
            DocType::Soap => {
                crate::commands::pipeline::process_recording(
                    app.clone(),
                    st,
                    rid.clone(),
                    req.context.clone(),
                    req.template.clone(),
                    req.patient_context.clone(),
                )
                .await
            }
            DocType::Referral => {
                crate::commands::generation::referral::generate_referral(
                    app.clone(),
                    st,
                    rid.clone(),
                    req.recipient_type.clone(),
                    req.urgency.clone(),
                    req.context.clone(),
                )
                .await
            }
            DocType::Letter => {
                crate::commands::generation::letter::generate_letter(
                    app.clone(),
                    st,
                    rid.clone(),
                    req.letter_type.clone(),
                    req.audience_id,
                    req.context.clone(),
                )
                .await
            }
            DocType::Synopsis => {
                crate::commands::generation::synopsis::generate_synopsis(
                    app.clone(),
                    st,
                    rid.clone(),
                )
                .await
            }
            DocType::PeerDiscussion => {
                let physician = req.physician_name.clone().unwrap_or_default();
                let specialty = req.specialty.clone().unwrap_or_default();
                let reason = req.reason.clone().unwrap_or_default();
                crate::commands::generation::peer_discussion::generate_peer_discussion(
                    app.clone(),
                    st,
                    rid.clone(),
                    physician,
                    specialty,
                    reason,
                    req.context.clone(),
                )
                .await
            }
        };
        // Safety net: the in-command events normally set the terminal stage
        // (advancing the sequence, which suppresses this). This only lands
        // when no event fired since queue time — e.g. the generation lock
        // was already held and the command bailed before its first emit.
        match result {
            Ok(_) => jobs.mark_if_current(&rid, seq, "completed", None),
            Err(e) => jobs.mark_if_current(&rid, seq, "failed", Some(e.to_string())),
        }
    });

    info!(recording_id = %recording_id, doc_type = doc.as_str(), "mobile: generation queued");
    Ok(StatusCode::ACCEPTED)
}
