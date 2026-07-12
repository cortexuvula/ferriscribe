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
fn max_chars_for_field(field: &str) -> usize {
    match field {
        "transcript" => 500_000,
        "soap_note" | "referral" | "letter" | "peer_discussion" | "chat" => 500_000,
        _ => 50_000,
    }
}

/// Save a clinician-edited text field on a recording.
///
/// Thinly wraps [`save_recording_field_inner`] so the inner logic can be
/// unit-tested without needing `tauri::State`.
#[tauri::command]
pub async fn save_recording_field(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    field: String,
    value: String,
) -> AppResult<()> {
    let db = state.db.clone();
    let cfg = {
        let conn = db.conn()?;
        medical_db::settings::SettingsRepo::load_config(&conn).unwrap_or_default()
    };
    let recording_id_inner = recording_id.clone();
    let field_inner = field.clone();
    let value_inner = value.clone();
    let capture = cfg.capture_for_training;
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        save_recording_field_inner(
            db,
            &conn,
            &recording_id_inner,
            &field_inner,
            &value_inner,
            capture,
        )
    })
    .await
    .map_err(crate::commands::join_err)??;

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
            crate::commands::content_sync::content_sync_target(&st)
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
        "transcript" => recording.transcript = owned_value,
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
    let _ = medical_db::ContentSyncRepo::upsert_revision(
        conn,
        &recording.id,
        field,
        &now,
        None, // origin_device — could add machine_id later
    );

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
