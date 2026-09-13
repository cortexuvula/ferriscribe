//! Edit-save flow: persist user edits to a recording's text fields.
//!
//! Used by EditorTab when the clinician edits the SOAP / referral /
//! letter / transcript directly in the app. Each call also wires
//! into the training-corpus pipeline when the field is `soap_note`
//! and capture is enabled.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_db::Connection;
use medical_db::generations::GenerationsRepo;
use medical_db::recordings::RecordingsRepo;
use uuid::Uuid;

use crate::state::AppState;

// `Manager` provides `AppHandle::state::<T>()` used by the content-sync push.
use tauri::Manager;

/// Whitelist of fields that the frontend is allowed to edit. Anything
/// else returns an error. Keeps the surface tight; non-text fields like
/// patient_name, tags, metadata get their own commands.
const EDITABLE_FIELDS: &[&str] = &[
    "transcript",
    "soap_note",
    "referral",
    "letter",
    "peer_discussion",
    "chat",
];

/// Per-field character caps for edited content. Mirrors the generation
/// pipeline's `MAX_*_CHARS` bounds so a misbehaving/compromised frontend
/// can't store multi-megabyte strings that would later be re-fed to AI
/// providers or bloat the DB. Empty values (field-clear) bypass the cap.
/// Per-field character caps for edited content. Mirrors the generation
/// pipeline's `MAX_*_CHARS` bounds so a misbehaving/compromised frontend
/// can't store multi-megabyte strings that would later be re-fed to AI
/// providers or bloat the DB. Empty values (field-clear) bypass the cap.
///
/// The `_` arm is a fallback for forward-compat; the
/// `every_editable_field_has_explicit_cap` test guards that adding a field
/// to `EDITABLE_FIELDS` without an explicit cap here fails the test.
pub(crate) fn max_chars_for_field(field: &str) -> usize {
    match field {
        "transcript" => 500_000,
        "soap_note" | "referral" | "letter" | "peer_discussion" | "chat" => 500_000,
        _ => 50_000,
    }
}

/// Core save used by both the Tauri command and the restart-time pending
/// edit flush (`commands::restart`). Loads `capture_for_training` off the
/// async worker, then persists the edit. Content-sync push stays in the
/// command path only (it is a fire-and-forget debounced task — pointless
/// to spawn right before a restart exit).
pub(crate) async fn save_recording_field_core(
    db: &Arc<medical_db::Database>,
    recording_id: &str,
    field: &str,
    value: &str,
) -> AppResult<()> {
    // Load capture_for_training off the async worker — preserve the
    // `unwrap_or_default()` semantics so a settings load failure still lets
    // the edit go through without training capture. The JoinError itself is
    // surfaced as a real error (it indicates a panic).
    let capture = {
        let db_cfg = Arc::clone(db);
        tokio::task::spawn_blocking(move || -> bool {
            let conn = match db_cfg.conn() {
                Ok(c) => c,
                Err(_) => return false,
            };
            medical_db::settings::SettingsRepo::load_config(&conn)
                .unwrap_or_default()
                .capture_for_training
        })
        .await
        .map_err(crate::commands::join_err)?
    };
    let recording_id = recording_id.to_string();
    let field = field.to_string();
    let value = value.to_string();
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        save_recording_field_inner(
            Arc::clone(&db),
            &conn,
            &recording_id,
            &field,
            &value,
            capture,
        )
    })
    .await
    .map_err(crate::commands::join_err)?
}

/// Save a clinician-edited text field on a recording.
///
/// Thinly wraps [`save_recording_field_core`] + a best-effort content-sync
/// push so the inner logic can be unit-tested without `tauri::State`.
#[tauri::command]
pub async fn save_recording_field(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    field: String,
    value: String,
) -> AppResult<()> {
    let db = state.db.clone();
    save_recording_field_core(&db, &recording_id, &field, &value).await?;

    // Best-effort content sync push (fire-and-forget, debounced ~2s). The
    // debounce coaleses back-to-back edits (e.g. the frontend saving SOAP
    // then referral within the same second) into a single push batch. The
    // owned `PairedConnection` is moved into the task and `ContentRemote`
    // borrows it from within the task scope, mirroring the condition-chip
    // push pattern.
    let app_clone = app.clone();
    let rec_id = recording_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let st = app_clone.state::<AppState>();
        if let Some((conn, bearer, client)) =
            crate::commands::content_sync::content_sync_target(&st).await
            && let Some(remote) =
                crate::content_remote::ContentRemote::from(&conn, Some(bearer), client)
        {
            let db = st.db.clone();
            let rec_id_clone = rec_id.clone();
            let push_result = tokio::task::spawn_blocking(move || -> AppResult<_> {
                let c = db.conn()?;
                crate::commands::content_sync::build_sync_recording(&c, &rec_id_clone)
            })
            .await;
            if let Ok(Ok(sync_rec)) = push_result {
                let _ = remote.push(vec![sync_rec]).await;
            }
        }
    });

    Ok(())
}

/// Inner logic — testable without `tauri::State`.
///
/// Steps:
///  1. Validate `field` against the whitelist.
///  2. Parse `recording_id` as UUID.
///  3. Load the recording, mutate the requested field, persist.
///  4. If `field == "soap_note"` and `capture_enabled`, update the
///     matching generations row's `final_text` and spawn the
///     background edit-distance task.
pub fn save_recording_field_inner(
    db: Arc<medical_db::Database>,
    conn: &Connection,
    recording_id: &str,
    field: &str,
    value: &str,
    capture_enabled: bool,
) -> AppResult<()> {
    if !EDITABLE_FIELDS.contains(&field) {
        return Err(AppError::Other(format!(
            "field '{field}' is not editable; allowed: {EDITABLE_FIELDS:?}"
        )));
    }

    // Length cap: defend against unbounded text (defense-in-depth — the
    // frontend shouldn't send megabytes, but don't trust it). Empty values
    // (clearing a field) are allowed through.
    let max_chars = max_chars_for_field(field);
    if !value.is_empty() && value.chars().count() > max_chars {
        return Err(AppError::Other(format!(
            "field '{field}' value exceeds {max_chars} character limit (got {})",
            value.chars().count()
        )));
    }

    let id = Uuid::parse_str(recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;

    // Load → mutate → persist.
    let mut recording = RecordingsRepo::get_by_id(conn, &id)?;

    // Empty string means "clear the field".
    let owned_value = if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    };

    match field {
        // TRANSCRIPT SEGMENT INVALIDATION (review contract line C,
        // docs/reviews/transcript-render-2026-09-13, finding 2): saving an
        // edited transcript must drop the stale `transcript_segments`
        // metadata in the SAME transaction as the text save. Segments take
        // precedence over text parsing in the rich view, so retaining them
        // hid saved corrections behind the pre-edit words (a corrected dose
        // or negation appeared to revert despite a successful save). The
        // text (with its `[Speaker unassigned]` markers / speaker labels)
        // becomes the single source of truth until the next
        // retranscription re-persists fresh segments. Cleared by KEY
        // REMOVAL, not null-write: the frontend's shape validator treats a
        // null value the same as an absent key, but removal is the honest
        // state — there ARE no segments for this text.
        // DIARIZATION FOLD EVIDENCE (hard-block design, legacy folded
        // transcripts): whether the cleared segments COULD have been folded
        // by the old `seg.speaker.or(last_speaker)` formatter is only
        // observable here — once the segments are removed, a folded span is
        // indistinguishable from the speaker's own speech in stored text.
        // Record it BEFORE the removal, in the same pass, as a closed
        // vocabulary value (content-free by construction: no span text, no
        // speaker labels, no counts). Three states, and unknown must stay
        // distinguishable from "no":
        //   "fold_possible"  — an unlabelled segment FOLLOWED a labelled one
        //                      (the only ordering the inheritance could
        //                      corrupt).
        //   "none_observed"  — segments were present and parseable, and
        //                      showed no such ordering.
        //   key ABSENT       — segments missing/unparseable at clear time:
        //                      UNKNOWN. Never guessed, never defaulted to
        //                      "none_observed" (stale/pre-flag recordings
        //                      must not silently read as clean). An existing
        //                      historical value is likewise left untouched —
        //                      the evidence was recorded when it existed.
        "transcript" => {
            recording.transcript = owned_value;
            if let Some(obj) = recording.metadata.as_object_mut() {
                let fold_evidence = obj
                    .get("transcript_segments")
                    .and_then(diarization_fold_evidence);
                obj.remove("transcript_segments");
                if let Some(value) = fold_evidence {
                    obj.insert(
                        "diarization_fold_evidence".into(),
                        serde_json::Value::String(value.as_str().to_owned()),
                    );
                }
            }
        }
        "soap_note" => recording.soap_note = owned_value,
        "referral" => recording.referral = owned_value,
        "letter" => recording.letter = owned_value,
        "peer_discussion" => recording.peer_discussion = owned_value,
        "chat" => recording.chat = owned_value,
        _ => {
            // The whitelist check above makes this branch unreachable in
            // practice. Use an explicit Err rather than unreachable!() to
            // satisfy conservative lint configurations.
            return Err(AppError::Other(format!("unexpected field: {field}")));
        }
    }

    RecordingsRepo::update(conn, &recording)?;

    // Bump updated_at + field revision for content sync. The recording row's
    // `updated_at` drives the changed-since delta query, and the per-field
    // revision gives the merge a precise LWW timestamp for this exact field.
    // Best-effort: a failure here must not turn a successful edit-save into
    // an error (the user's edit is already persisted above).
    let now = chrono::Utc::now().to_rfc3339();
    let _ = conn.execute(
        "UPDATE recordings SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, recording.id.to_string()],
    );
    // Best-effort, but visible: a dropped revision stamp costs this edit
    // its LWW sync priority with no other signal. Field name only — no PHI.
    if let Err(e) = medical_db::ContentSyncRepo::upsert_revision(
        conn,
        &recording.id,
        field,
        &now,
        None, // origin_device — could add machine_id later
    ) {
        tracing::warn!(error = %e, field, "edit saved without a field revision stamp");
    }

    // Training-corpus finalize hook. Only applies to soap_note (v1 captures
    // only SOAP). Best-effort — failures are logged but never returned to
    // the caller.
    if field == "soap_note" && capture_enabled {
        match GenerationsRepo::update_final_text(conn, id, "soap", value) {
            Ok(Some(g)) => {
                tracing::debug!(generation_id = %g.id, "updated final_text via edit-save");
                crate::commands::generation::soap::spawn_edit_distance_task(
                    db,
                    g.id,
                    g.draft_text.clone(),
                    value.to_owned(),
                );
            }
            Ok(None) => {
                // No generation row for this recording — capture was off when
                // the SOAP was generated. Nothing to update.
            }
            Err(e) => {
                tracing::warn!(error = %e, field = %field, "edit-save finalize failed");
            }
        }
    }

    Ok(())
}

/// Closed vocabulary for the persisted `diarization_fold_evidence` metadata
/// key. Written ONLY at transcript-segment clear time (see the transcript
/// arm above); never a third value — absence of the key is the unknown
/// state and is meaningful on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiarizationFoldEvidence {
    /// An unlabelled segment followed a labelled one — the ordering the old
    /// formatter's `last_speaker` inheritance could fold into the wrong
    /// speaker.
    FoldPossible,
    /// Segments were present and parseable, but never showed a labelled →
    /// unlabelled ordering.
    NoneObserved,
}

impl DiarizationFoldEvidence {
    fn as_str(self) -> &'static str {
        match self {
            Self::FoldPossible => "fold_possible",
            Self::NoneObserved => "none_observed",
        }
    }
}

/// Classify a `transcript_segments` metadata value for fold evidence.
///
/// Returns `Some` only when the value is a parseable segment array — the
/// caller then persists exactly that verdict and REMOVES the segments. Any
/// other shape (missing key, non-array, elements that aren't objects with
/// an inspectable `speaker` field) returns `None`: the evidence is UNKNOWN
/// and the key must be left absent rather than guessed (a malformed store
/// or a pre-flag recording is not "no fold").
pub(crate) fn diarization_fold_evidence(
    segments: &serde_json::Value,
) -> Option<DiarizationFoldEvidence> {
    let arr = segments.as_array()?;
    let mut saw_labelled = false;
    for seg in arr {
        let speaker = seg.as_object()?.get("speaker")?;
        if speaker.is_string() {
            saw_labelled = true;
        } else if saw_labelled {
            // Unlabelled (or null) after a labelled span — the exact
            // ordering the old formatter's inheritance corrupted.
            return Some(DiarizationFoldEvidence::FoldPossible);
        }
    }
    Some(DiarizationFoldEvidence::NoneObserved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::recording::{ProcessingStatus, Recording};
    use medical_db::Connection;
    use medical_db::generations::{GenerationInsert, GenerationsRepo};
    use medical_db::migrations::MigrationEngine;
    use medical_db::recordings::RecordingsRepo;
    use std::path::PathBuf;

    fn in_memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    fn insert_recording(conn: &Connection) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        let mut rec = Recording::new(
            format!("{}.wav", id),
            PathBuf::from(format!("/tmp/{}.wav", id)),
        );
        rec.id = id;
        rec.status = ProcessingStatus::Pending;
        rec.soap_note = Some("Original SOAP text.".into());
        RecordingsRepo::insert(conn, &rec).unwrap();
        id
    }

    fn insert_generation(conn: &Connection, recording_id: uuid::Uuid) -> uuid::Uuid {
        let g = GenerationsRepo::record_generation(
            conn,
            GenerationInsert {
                recording_id,
                output_type: "soap",
                ai_provider: "ollama",
                ai_model: "llama3",
                prompt_template_name: None,
                input_transcript: "Patient reports headache.",
                input_context_json: None,
                draft_text: "Original SOAP text.",
            },
        )
        .unwrap();
        g.id
    }

    /// Review contract line C (finding 2): saving an edited transcript must
    /// clear `transcript_segments` in the SAME transaction — stale segments
    /// take precedence in the rich view and hid saved corrections.
    #[test]
    fn saving_transcript_clears_stale_segments_in_same_save() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        // Give the recording the producer-shaped metadata: segments the
        // diarizer emitted for the ORIGINAL text, plus the outcome keys.
        {
            let mut rec = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
            rec.metadata = serde_json::json!({
                "transcript_segments": [
                    {"speaker": "Speaker 1", "text": "Original wording.", "start": 0.0, "end": 1.0}
                ],
                "diarization_outcome": "completed",
                "diarization_reason": null,
            });
            rec.transcript =
                Some("00:00:00,000 --> 00:00:01,000 [Speaker 1] \nOriginal wording.".into());
            RecordingsRepo::update(&conn, &rec).unwrap();
        }
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        save_recording_field_inner(
            db,
            &conn,
            &rec_id.to_string(),
            "transcript",
            "00:00:00,000 --> 00:00:01,000 [Speaker 1] \nCorrected wording.",
            false,
        )
        .unwrap();

        let after = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
        assert_eq!(
            after.transcript.as_deref(),
            Some("00:00:00,000 --> 00:00:01,000 [Speaker 1] \nCorrected wording."),
            "edited text saved"
        );
        // The stale segments are GONE (key removed, not null) — the saved
        // text is now the single source of truth for rendering.
        assert!(
            after.metadata.get("transcript_segments").is_none(),
            "stale transcript_segments must not survive a transcript edit; metadata: {}",
            after.metadata
        );
        // Other metadata keys are untouched — the save is surgical.
        assert_eq!(after.metadata["diarization_outcome"], "completed");
    }

    /// Clearing the transcript (empty value) also drops the segments — an
    /// empty transcript with live segments is the same stale-precedence
    /// hazard in reverse.
    #[test]
    fn clearing_transcript_also_clears_segments() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        {
            let mut rec = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
            rec.metadata = serde_json::json!({
                "transcript_segments": [
                    {"speaker": "Speaker 1", "text": "Original wording.", "start": 0.0, "end": 1.0}
                ]
            });
            rec.transcript = Some("Original wording.".into());
            RecordingsRepo::update(&conn, &rec).unwrap();
        }
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        save_recording_field_inner(db, &conn, &rec_id.to_string(), "transcript", "", false)
            .unwrap();

        let after = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
        assert_eq!(after.transcript, None);
        assert!(after.metadata.get("transcript_segments").is_none());
    }

    /// Helper: seed a recording with the given metadata + transcript, then
    /// save an edited transcript through the full inner path.
    fn save_transcript_edit_with_metadata(
        conn: &Connection,
        metadata: serde_json::Value,
    ) -> medical_core::types::recording::Recording {
        let rec_id = insert_recording(conn);
        {
            let mut rec = RecordingsRepo::get_by_id(conn, &rec_id).unwrap();
            rec.metadata = metadata;
            rec.transcript = Some("Original wording.".into());
            RecordingsRepo::update(conn, &rec).unwrap();
        }
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        save_recording_field_inner(
            db,
            conn,
            &rec_id.to_string(),
            "transcript",
            "Edited wording.",
            false,
        )
        .unwrap();
        RecordingsRepo::get_by_id(conn, &rec_id).unwrap()
    }

    /// Fold evidence, labelled → unlabelled: a null-speaker segment FOLLOWING
    /// a labelled one is the only ordering the old formatter's
    /// `last_speaker` inheritance could corrupt — must record
    /// `fold_possible`.
    #[test]
    fn fold_evidence_labelled_then_unlabelled_yields_fold_possible() {
        let conn = in_memory_db();
        let after = save_transcript_edit_with_metadata(
            &conn,
            serde_json::json!({
                "transcript_segments": [
                    {"speaker": "Speaker 1", "text": "Labelled.", "start": 0.0, "end": 1.0},
                    {"speaker": null, "text": "Unlabelled after.", "start": 1.0, "end": 2.0}
                ],
                "diarization_outcome": "completed"
            }),
        );
        assert_eq!(
            after.metadata.get("diarization_fold_evidence"),
            Some(&serde_json::json!("fold_possible")),
            "labelled→unlabelled ordering must record fold_possible; metadata: {}",
            after.metadata
        );
    }

    /// Fold evidence, leading-unlabelled only: unlabelled spans with no
    /// labelled span before them were never inheritable — `none_observed`.
    #[test]
    fn fold_evidence_leading_unlabelled_only_yields_none_observed() {
        let conn = in_memory_db();
        let after = save_transcript_edit_with_metadata(
            &conn,
            serde_json::json!({
                "transcript_segments": [
                    {"speaker": null, "text": "Unlabelled first.", "start": 0.0, "end": 1.0},
                    {"speaker": null, "text": "Still unlabelled.", "start": 1.0, "end": 2.0}
                ]
            }),
        );
        assert_eq!(
            after.metadata.get("diarization_fold_evidence"),
            Some(&serde_json::json!("none_observed")),
            "leading-unlabelled-only must record none_observed; metadata: {}",
            after.metadata
        );
    }

    /// Fold evidence, no segments at clear time: the key must be ABSENT —
    /// unknown is a distinct third state and must never be silently written
    /// as `none_observed`.
    #[test]
    fn fold_evidence_absent_segments_leaves_key_absent() {
        let conn = in_memory_db();
        let after = save_transcript_edit_with_metadata(
            &conn,
            serde_json::json!({"diarization_outcome": "completed"}),
        );
        assert!(
            after.metadata.get("diarization_fold_evidence").is_none(),
            "no segments at clear time means UNKNOWN — key must be absent, not none_observed; metadata: {}",
            after.metadata
        );
    }

    /// Fold evidence survives the segment clear alongside the outcome keys:
    /// the flag, `diarization_outcome`, and `diarization_reason` all
    /// outlive the removal of `transcript_segments` in the same save.
    #[test]
    fn fold_evidence_survives_segment_clear_alongside_outcome_keys() {
        let conn = in_memory_db();
        let after = save_transcript_edit_with_metadata(
            &conn,
            serde_json::json!({
                "transcript_segments": [
                    {"speaker": "Speaker 2", "text": "Labelled.", "start": 0.0, "end": 1.0},
                    {"speaker": null, "text": "Folded candidate.", "start": 1.0, "end": 2.0}
                ],
                "diarization_outcome": "completed",
                "diarization_reason": null
            }),
        );
        assert!(after.metadata.get("transcript_segments").is_none());
        assert_eq!(
            after.metadata.get("diarization_fold_evidence"),
            Some(&serde_json::json!("fold_possible"))
        );
        assert_eq!(after.metadata["diarization_outcome"], "completed");
        assert!(after.metadata.get("diarization_reason").is_some());
    }

    /// Fold evidence is historical: a SECOND transcript edit that has no
    /// segments to inspect (they were cleared by the first) must not
    /// overwrite the recorded verdict — `fold_possible` stays even though
    /// this clear observed nothing.
    #[test]
    fn fold_evidence_not_overwritten_by_later_edit_without_segments() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        {
            let mut rec = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
            rec.metadata = serde_json::json!({
                "transcript_segments": [
                    {"speaker": "Speaker 1", "text": "Labelled.", "start": 0.0, "end": 1.0},
                    {"speaker": null, "text": "Unlabelled.", "start": 1.0, "end": 2.0}
                ]
            });
            rec.transcript = Some("Original wording.".into());
            RecordingsRepo::update(&conn, &rec).unwrap();
        }
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        // First edit: segments present → fold_possible recorded, segments
        // cleared.
        save_recording_field_inner(
            std::sync::Arc::clone(&db),
            &conn,
            &rec_id.to_string(),
            "transcript",
            "First edit.",
            false,
        )
        .unwrap();
        let first = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
        assert_eq!(
            first.metadata.get("diarization_fold_evidence"),
            Some(&serde_json::json!("fold_possible"))
        );

        // Second edit: no segments left to inspect — the historical verdict
        // must survive untouched.
        save_recording_field_inner(
            db,
            &conn,
            &rec_id.to_string(),
            "transcript",
            "Second edit.",
            false,
        )
        .unwrap();
        let second = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
        assert_eq!(
            second.metadata.get("diarization_fold_evidence"),
            Some(&serde_json::json!("fold_possible")),
            "a later segment-free edit must not rewrite historical fold evidence; metadata: {}",
            second.metadata
        );
        assert_eq!(second.transcript.as_deref(), Some("Second edit."));
    }

    /// Editing a NON-transcript field must not touch transcript segments —
    /// a SOAP edit invalidates nothing about the transcript's metadata.
    #[test]
    fn saving_other_field_leaves_transcript_segments_alone() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        {
            let mut rec = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
            rec.metadata = serde_json::json!({
                "transcript_segments": [
                    {"speaker": "Speaker 1", "text": "Original wording.", "start": 0.0, "end": 1.0}
                ]
            });
            RecordingsRepo::update(&conn, &rec).unwrap();
        }
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        save_recording_field_inner(
            db,
            &conn,
            &rec_id.to_string(),
            "soap_note",
            "Amended assessment.",
            false,
        )
        .unwrap();

        let after = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
        assert!(
            after.metadata.get("transcript_segments").is_some(),
            "a non-transcript edit must not invalidate transcript segments"
        );
    }

    #[test]
    fn rejects_non_whitelisted_field() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        let result = save_recording_field_inner(
            db,
            &conn,
            &rec_id.to_string(),
            "patient_name",
            "Dr Smith",
            false,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("patient_name"));
    }

    #[test]
    fn rejects_value_exceeding_length_cap() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        // 500_001 chars — one over the cap.
        let oversized = "x".repeat(500_001);
        let result = save_recording_field_inner(
            db,
            &conn,
            &rec_id.to_string(),
            "soap_note",
            &oversized,
            false,
        );
        assert!(result.is_err(), "oversized value should be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("character limit"),
            "error should mention limit: {msg}"
        );
    }

    #[test]
    fn accepts_value_at_length_cap_boundary() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());
        // Exactly 500_000 chars — the cap (should pass).
        let at_cap = "x".repeat(500_000);
        save_recording_field_inner(db, &conn, &rec_id.to_string(), "soap_note", &at_cap, false)
            .expect("value at the cap boundary should be accepted");
    }

    #[test]
    fn updates_soap_note_field_in_recording() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());

        save_recording_field_inner(
            db,
            &conn,
            &rec_id.to_string(),
            "soap_note",
            "Edited SOAP note.",
            false, // capture off — skip generation hook
        )
        .unwrap();

        let refreshed = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
        assert_eq!(refreshed.soap_note.as_deref(), Some("Edited SOAP note."));
    }

    #[tokio::test]
    async fn updates_final_text_in_generations_when_capture_enabled() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        let gen_id = insert_generation(&conn, rec_id);

        // Use a dummy Arc<Database> — the edit-distance spawn will fail
        // to open a connection on the in-memory db copy, but the core
        // update_final_text path runs synchronously on `conn`.
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());

        save_recording_field_inner(
            db,
            &conn,
            &rec_id.to_string(),
            "soap_note",
            "Clinician edited version.",
            true, // capture enabled
        )
        .unwrap();

        // The generations row's final_text should now be the new value.
        let g = GenerationsRepo::get_by_id(&conn, gen_id).unwrap();
        assert_eq!(
            g.final_text.as_deref(),
            Some("Clinician edited version."),
            "final_text should be updated by edit-save hook"
        );
    }

    #[test]
    fn clears_field_when_empty_value_given() {
        let conn = in_memory_db();
        let rec_id = insert_recording(&conn);
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().unwrap());

        save_recording_field_inner(db, &conn, &rec_id.to_string(), "soap_note", "", false).unwrap();

        let refreshed = RecordingsRepo::get_by_id(&conn, &rec_id).unwrap();
        assert!(
            refreshed.soap_note.is_none(),
            "empty value should clear the field"
        );
    }

    #[test]
    fn every_editable_field_has_explicit_cap() {
        // Guard: if a field is added to EDITABLE_FIELDS without an explicit
        // arm in max_chars_for_field, it silently falls into the _ => 50_000
        // fallback. This test catches that by asserting every whitelisted
        // field gets the intended 500_000 cap (the large-doc limit matching
        // the generation pipeline). A new field must be added to both the
        // match and this assertion.
        for field in EDITABLE_FIELDS {
            assert_eq!(
                max_chars_for_field(field),
                500_000,
                "field '{field}' is in EDITABLE_FIELDS but max_chars_for_field returned the 50_000 fallback — add an explicit cap arm"
            );
        }
    }
}
