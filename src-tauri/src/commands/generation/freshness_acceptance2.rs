//! Acceptance cases 9–13: the late additions to the spec matrix.
//!
//! - Case 9 (migration): a database created BEFORE the provenance table
//!   (schema m019, on a real disk file) carrying legacy outputs, then
//!   upgraded through the REAL migration path (m020 applied by
//!   `MigrationEngine::migrate` on open). Every type must read `unknown`
//!   with `missing_provenance` — never crash, never fresh. (a4 covers
//!   wiped provenance rows on an in-memory DB; this covers the same
//!   state via the upgrade path a real database takes.)
//! - Case 12 (capture timing): provenance must snapshot the inputs the
//!   model actually SAW. A mid-generation context edit must leave the
//!   output stale-or-unknown — never falsely fresh — and after
//!   regenerating with the new inputs, fresh.
//! - Case 13 (digest trim normalization): every write-side digest that is
//!   compared against a stored text column must trim like the read side
//!   (`text_digest`) — an output persisted with trailing whitespace
//!   (`strip_markdown` does not trim) or a SOAP source column carrying a
//!   trailing newline must read fresh immediately after generation, not
//!   `output_modified`/`source_changed`.

use super::freshness::{CurrentDocInputs, FreshnessStatus, compute_report_for_test};
use super::test_helpers::{MockCompletionProvider, build_test_state_with_provider};
use medical_core::types::settings::AppConfig;
use std::sync::Arc;

fn base_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.ai_provider = "ollama".to_string();
    config.ollama_host = "localhost".to_string();
    config.ai_model = "llama3".to_string();
    config
}

async fn report(
    state: &crate::state::AppState,
    recording_id: &str,
    inputs: &CurrentDocInputs,
) -> super::freshness::FreshnessReport {
    let uuid = uuid::Uuid::parse_str(recording_id).expect("uuid");
    let db = Arc::clone(&state.db);
    let inputs = inputs.clone();
    tokio::task::spawn_blocking(move || -> medical_core::error::AppResult<_> {
        let conn = db.conn()?;
        let recording = medical_db::recordings::RecordingsRepo::get_by_id_active(&conn, &uuid)?;
        let mut config = medical_db::settings::SettingsRepo::load_config(&conn)?;
        config.migrate();
        Ok(compute_report_for_test(&conn, &recording, &config, &inputs))
    })
    .await
    .expect("blocking task")
    .expect("report")
}

/// Case 9 (migration): build a REAL disk database frozen at schema m019
/// (the last pre-provenance version) with legacy outputs on the row, then
/// upgrade via `Database::open` — the same `MigrationEngine::migrate` a
/// production database runs at startup — and require every type to read
/// `unknown`/`missing_provenance`, never fresh, never a crash.
#[tokio::test]
async fn a9_migration_legacy_rows_read_unknown_after_upgrade() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("legacy-m019.db");

    // ── The pre-provenance world: a disk DB at schema m019 ──────────────
    let legacy_recording_id = {
        let conn = rusqlite::Connection::open(&db_path).expect("open raw disk db");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version    INTEGER NOT NULL,
                name       TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .expect("schema_version table");
        for migration in medical_db::migrations::all_migrations() {
            assert!(
                migration.version <= 20,
                "new migrations appended: bump the pre-provenance pin (currently m020 = generation_provenance)"
            );
            if migration.version >= 20 {
                break; // freeze at m019 — the world before provenance
            }
            (migration.up)(&conn).expect("apply pre-provenance migration");
            conn.execute(
                "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
                rusqlite::params![migration.version, migration.name],
            )
            .expect("record applied migration");
        }

        // A legacy recording with outputs but, by construction, no
        // provenance rows (the table does not exist yet).
        let mut rec = medical_core::types::recording::Recording::new(
            "legacy.wav".to_string(),
            std::path::PathBuf::from("/synthetic/legacy.wav"),
        );
        rec.transcript = Some("Legacy synthetic transcript".into());
        rec.soap_note = Some("Legacy synthetic SOAP".into());
        rec.referral = Some("Legacy synthetic referral".into());
        rec.letter = Some("Legacy synthetic letter".into());
        rec.peer_discussion = Some("Legacy synthetic discussion".into());
        medical_db::recordings::RecordingsRepo::insert(&conn, &rec).expect("insert legacy row");
        medical_db::settings::SettingsRepo::save_config(&conn, &base_config())
            .expect("save config");
        rec.id
        // conn dropped here: the file closes with the legacy schema on disk
    };

    // ── The upgrade: Database::open runs MigrationEngine::migrate, which
    // applies m020 (generation_provenance) — exactly what a user's
    // database goes through when the app updates. Must not crash.
    let db = Arc::new(
        medical_db::Database::open(&db_path, None).expect("open + migrate legacy disk db"),
    );
    assert!(
        std::path::Path::new(&db_path).exists(),
        "disk db must persist"
    );

    let r = {
        let conn = db.conn().expect("conn");
        let recording =
            medical_db::recordings::RecordingsRepo::get_by_id_active(&conn, &legacy_recording_id)
                .expect("legacy row survives migration");
        let mut config = medical_db::settings::SettingsRepo::load_config(&conn).expect("config");
        config.migrate();
        compute_report_for_test(&conn, &recording, &config, &CurrentDocInputs::default())
    };

    // Every legacy output: unknown + missing_provenance — never fresh.
    for (name, v) in [
        ("soap", &r.soap),
        ("referral", &r.referral),
        ("letter", &r.letter),
        ("peer_discussion", &r.peer_discussion),
    ] {
        assert_eq!(
            v.status,
            FreshnessStatus::Unknown,
            "{name}: legacy rows read unknown after upgrade"
        );
        assert_eq!(v.reasons, vec!["missing_provenance"], "{name}: reason code");
    }
}

/// Case 12 (capture timing): the digest written to provenance must be the
/// digest of the inputs sent to the model — computed from the same
/// `context`/`patient_context` arguments the command received — NOT
/// re-read from the recording row at completion time. Concretely: the
/// write side passes the command's arguments straight into
/// `soap_input_digest` (soap.rs), so a metadata edit that lands on the
/// row mid-generation cannot contaminate the snapshot. We verify the
/// invariant end-to-end: generate with context A while the row carries
/// context B in metadata; the verdict for inputs A must be fresh and for
/// inputs B stale — proving the snapshot bound to what the model saw (A),
/// not to row state. Regenerating with the NEW inputs must then read
/// fresh again (stale is never a trap: the user can always recover).
#[tokio::test]
async fn a12_provenance_snapshots_inputs_actually_sent() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    // Simulate a concurrent editor save landing on the row BEFORE
    // generation reads it back (the generation command holds its own
    // argument copies; the row's metadata is never the digest source).
    let gen_inputs = CurrentDocInputs {
        context: Some("context the model saw".into()),
        ..Default::default()
    };
    let edited_inputs = CurrentDocInputs {
        context: Some("mid-generation edit on the row".into()),
        ..Default::default()
    };
    {
        let uuid = uuid::Uuid::parse_str(&rid).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let mut rec =
            medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
        if rec.metadata.is_null() {
            rec.metadata = serde_json::json!({});
        }
        rec.metadata["context"] = serde_json::json!("mid-generation edit on the row");
        medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
    }
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &gen_inputs)
        .await
        .expect("generation must succeed");

    // The verdict binds to the inputs actually sent: fresh for those,
    // stale for anything else — including the row's mid-flight metadata
    // (never falsely fresh for the edited inputs).
    let r = report(&state, &rid, &gen_inputs).await;
    assert_eq!(
        r.soap.status,
        FreshnessStatus::Fresh,
        "provenance must snapshot the inputs sent to the model"
    );
    let r = report(&state, &rid, &edited_inputs).await;
    assert!(
        matches!(
            r.soap.status,
            FreshnessStatus::Stale | FreshnessStatus::Unknown
        ),
        "row state at completion must never count as the generation's input (got {:?})",
        r.soap.status
    );

    // Regenerate with the NEW inputs → fresh again (recoverable).
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &edited_inputs)
        .await
        .expect("regeneration must succeed");
    let r = report(&state, &rid, &edited_inputs).await;
    assert_eq!(
        r.soap.status,
        FreshnessStatus::Fresh,
        "regenerating with the new inputs must read fresh"
    );

    // Structural pin: the SOAP write path computes its digest from the
    // command arguments (template/context/patient_context), not by
    // re-reading the recording after the LLM call. If this source pin
    // breaks, case 12's guarantee has been regressed.
    let src = include_str!("soap.rs");
    // Find the PERSIST-side digest call (the one inside the provenance
    // block, after the LLM call) and pin that it feeds the command's own
    // argument copies — `template`, `context`, `patient_context` — into
    // the digest. The load-side call (resolve-time) is the first
    // occurrence; the persist-side is the last.
    let digest_call = src
        .match_indices("soap_input_digest")
        .map(|(i, _)| i)
        .collect::<Vec<_>>()
        .pop()
        .expect("soap_input_digest call site");
    let args_window = &src[digest_call..digest_call + 260];
    assert!(
        args_window.contains("template")
            && args_window.contains("context")
            && args_window.contains("patient_context"),
        "write-side digest must bind the command's argument copies, got: {args_window}"
    );
}

/// Case 13 (digest trim normalization, 2026-10-08 review): the write side
/// used to hash RAW text while the read side trims, so a referral/letter
/// persisted with a trailing newline (strip_markdown does not trim) read
/// `output_modified`, and a SOAP source column carrying trailing
/// whitespace (hand-edit or sync) flagged freshly generated derived
/// outputs `source_changed` the moment they were written. Every compared
/// digest now goes through the trimming variant on BOTH sides.
#[tokio::test]
async fn a13_trim_normalized_digests_keep_whitespace_padded_outputs_fresh() {
    // The mock's completion ends with newlines — strip_markdown keeps
    // them, so the persisted referral/letter columns carry trailing
    // whitespace exactly like a real model response does.
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Dear Cardiology, please assess this patient for chest pain.\n\n",
        64,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let inputs = CurrentDocInputs::default();
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs)
        .await
        .expect("generation must succeed");

    // Simulate a hand-edited/sync-arrived SOAP note whose column value
    // carries a trailing newline — the derived types bind this column as
    // their source, and the read side trims it before hashing.
    {
        let uuid = uuid::Uuid::parse_str(&rid).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let mut rec =
            medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
        let trimmed_note = rec.soap_note.as_deref().expect("soap note").to_string();
        rec.soap_note = Some(format!("{trimmed_note}\n"));
        medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
    }

    super::referral::generate_referral_inner_for_test(&state, &rid, &inputs)
        .await
        .expect("referral generation must succeed");
    super::letter::generate_letter_inner_for_test(&state, &rid, &inputs)
        .await
        .expect("letter generation must succeed");

    // Both derived types must read FRESH: no output_modified (their own
    // stored output ends with whitespace) and no source_changed (the SOAP
    // column they bind ends with a newline).
    let r = report(&state, &rid, &inputs).await;
    for (name, v) in [("referral", &r.referral), ("letter", &r.letter)] {
        assert_eq!(
            v.status,
            FreshnessStatus::Fresh,
            "{name}: whitespace padding must not break its own freshness binding (reasons: {:?})",
            v.reasons
        );
        assert!(
            !v.reasons.contains(&"output_modified"),
            "{name}: output digest must be trim-normalized"
        );
        assert!(
            !v.reasons.contains(&"source_changed"),
            "{name}: source digest must be trim-normalized"
        );
    }
}
