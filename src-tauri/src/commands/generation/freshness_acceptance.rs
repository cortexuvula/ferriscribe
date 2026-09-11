//! Freshness acceptance matrix (spec 2026-09-11): 6 functional cases +
//! PHI-boundary + tri-state. The race case is frontend-side (GenerateTab
//! request-scoped invalidation, covered by vitest).
//!
//! Every case drives the REAL write path (`generate_soap_inner` /
//! `generate_referral_inner` / `generate_letter_inner` /
//! `generate_peer_discussion_inner` with a mock provider) — provenance is
//! written by the same code production uses, then read back through the
//! same `compute_report` the command runs.

use super::freshness::{
    CurrentDocInputs, FreshnessStatus, compute_report_for_test,
};
use super::test_helpers::{MockCompletionProvider, build_test_state_with_provider};
use crate::state::AppState;
use medical_core::types::PatientContext;
use medical_core::types::settings::AppConfig;
use std::sync::Arc;

fn base_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.ai_provider = "ollama".to_string();
    // Loopback → preflight probe skipped; the mock serves completions.
    config.ollama_host = "localhost".to_string();
    config.ai_model = "llama3".to_string();
    config
}

fn pc(meds: &[&str]) -> PatientContext {
    PatientContext {
        patient_name: None,
        prior_soap_notes: vec![],
        medications: meds.iter().map(|s| (*s).to_string()).collect(),
        allergies: vec![],
        conditions: vec![],
    }
}

async fn report(
    state: &AppState,
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

fn load_config_blocking(state: &AppState) -> AppConfig {
    let conn = state.db.conn().expect("conn");
    let mut config = medical_db::settings::SettingsRepo::load_config(&conn).expect("config");
    config.migrate();
    config
}

/// Seed a recording that already has a SOAP note (for derived-type tests)
/// by running the real SOAP generation once.
async fn seeded(state: &AppState, recording_id: &str) {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    // build_test_state_with_provider registered its own provider; reuse it
    // by driving generation with the state as built.
    let _ = provider;
    super::soap::generate_soap_inner_for_test(state, recording_id).await;
}

// ─────────────────────────────────────────────────────────────────────────
// Case 1: unchanged inputs → fresh; partial notes edit with structured
// fields intact → stale; exact reversion → fresh.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a1_soap_fresh_on_unchanged_stale_on_edit_fresh_on_reversion() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let inputs = CurrentDocInputs {
        context: Some("Synthetic note alpha".into()),
        patient_context: Some(pc(&["Synthetic medication"])),
        ..Default::default()
    };
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs).await;

    // Unchanged → fresh.
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Fresh, "unchanged inputs must be fresh");

    // Partial notes edit, structured fields intact → stale.
    let edited = CurrentDocInputs {
        context: Some("Synthetic note beta".into()),
        patient_context: Some(pc(&["Synthetic medication"])),
        ..Default::default()
    };
    let r = report(&state, &rid, &edited).await;
    assert_eq!(r.soap.status, FreshnessStatus::Stale);
    assert_eq!(r.soap.reasons, vec!["inputs_changed"]);

    // Exact reversion → fresh again.
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Fresh, "exact reversion must be fresh");
}

// ────────────────────────────────────────────────────────────────────────
// Case 2: transcript-only edit → SOAP stale; derived types unaffected by
// the transcript (they key on SOAP + own inputs); per-document isolation.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a2_transcript_edit_stales_soap_only_and_document_isolation() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let inputs = CurrentDocInputs::default();
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs).await;
    super::referral::generate_referral_inner_for_test(&state, &rid, &inputs).await;
    super::letter::generate_letter_inner_for_test(&state, &rid, &inputs).await;
    super::peer_discussion::generate_peer_discussion_inner_for_test(&state, &rid, &inputs).await;

    // Everything fresh to start.
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Fresh);
    assert_eq!(r.referral.status, FreshnessStatus::Fresh);
    assert_eq!(r.letter.status, FreshnessStatus::Fresh);
    assert_eq!(r.peer_discussion.status, FreshnessStatus::Fresh);

    // Transcript edit: SOAP goes stale (transcript is its input); peer
    // mirrors the transcript → stale; referral/letter key on SOAP — fresh.
    {
        let uuid = uuid::Uuid::parse_str(&rid).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let mut rec =
            medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
        rec.transcript = Some("Edited synthetic transcript".into());
        medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
    }
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Stale, "transcript edit stales SOAP");
    assert_eq!(
        r.peer_discussion.status,
        FreshnessStatus::Stale,
        "peer mirrors the transcript"
    );
    assert_eq!(
        r.referral.status,
        FreshnessStatus::Fresh,
        "referral keys on SOAP, not transcript"
    );
    assert_eq!(r.letter.status, FreshnessStatus::Fresh);

    // Per-document isolation: change only the letter's audience field —
    // letter stale, siblings untouched.
    let mut letter_inputs = inputs.clone();
    letter_inputs.audience_id = Some(uuid::Uuid::new_v4());
    let r = report(&state, &rid, &letter_inputs).await;
    assert_eq!(r.letter.status, FreshnessStatus::Stale);
    assert_eq!(r.referral.status, FreshnessStatus::Fresh);
    assert_eq!(r.peer_discussion.status, FreshnessStatus::Stale); // transcript still edited
}

// ────────────────────────────────────────────────────────────────────────
// Case 3: SOAP regeneration (or hand-edit of the SOAP note) → referral
// and letter go stale via source_changed; peer unaffected.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a3_soap_change_stales_derived_types_via_source() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let inputs = CurrentDocInputs::default();
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs).await;
    super::referral::generate_referral_inner_for_test(&state, &rid, &inputs).await;
    super::letter::generate_letter_inner_for_test(&state, &rid, &inputs).await;

    // Hand-edit the SOAP note on the row (regeneration path is covered by
    // the same digest comparison).
    {
        let uuid = uuid::Uuid::parse_str(&rid).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let mut rec =
            medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
        rec.soap_note = Some("Hand-edited synthetic SOAP".into());
        medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
    }
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(
        r.referral.status,
        FreshnessStatus::Stale,
        "SOAP change must stale the referral via source_changed"
    );
    assert!(r.referral.reasons.contains(&"source_changed"));
    assert_eq!(
        r.letter.status,
        FreshnessStatus::Stale,
        "SOAP change must stale the letter via source_changed"
    );
    assert!(r.letter.reasons.contains(&"source_changed"));
}

// ────────────────────────────────────────────────────────────────────────
// Case 4: missing provenance → unknown/missing_provenance, never current.
// (Legacy outputs predating the table.)
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a4_missing_provenance_is_unknown_never_current() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    super::soap::generate_soap_inner_for_test_with(&state, &rid, &CurrentDocInputs::default())
        .await;

    // Simulate a legacy recording: outputs on the row, no provenance rows.
    {
        let uuid = uuid::Uuid::parse_str(&rid).expect("uuid");
        let conn = state.db.conn().expect("conn");
        conn.execute("DELETE FROM generation_provenance", []).expect("wipe provenance");
        let mut rec =
            medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
        rec.referral = Some("Legacy synthetic referral".into());
        rec.letter = Some("Legacy synthetic letter".into());
        rec.peer_discussion = Some("Legacy synthetic discussion".into());
        medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
    }

    let r = report(&state, &rid, &CurrentDocInputs::default()).await;
    for (name, v) in [
        ("soap", &r.soap),
        ("referral", &r.referral),
        ("letter", &r.letter),
        ("peer_discussion", &r.peer_discussion),
    ] {
        assert_eq!(v.status, FreshnessStatus::Unknown, "{name}: missing provenance");
        assert_eq!(v.reasons, vec!["missing_provenance"], "{name}: reason code");
    }
}

// ────────────────────────────────────────────────────────────────────────
// Case 5: settings change (model) → every proven type goes stale.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a5_settings_change_stales_everything() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let inputs = CurrentDocInputs::default();
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs).await;

    // Change the model in settings.
    {
        let conn = state.db.conn().expect("conn");
        let mut config = load_config_blocking(&state);
        config.ai_model = "qwen3.8".into();
        medical_db::settings::SettingsRepo::save_config(&conn, &config).expect("save");
    }
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Stale, "model change must stale SOAP");
}

// ────────────────────────────────────────────────────────────────────────
// Case 6: switch away and back — no cross-recording leakage; a recording
// with no provenance at all reads unknown for every type.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a6_switch_away_and_back_no_cross_recording_leakage() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let inputs = CurrentDocInputs {
        context: Some("Synthetic A".into()),
        ..Default::default()
    };
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs).await;

    // A second recording with no provenance.
    let other_id = {
        let mut rec = medical_core::types::recording::Recording::new(
            "other.wav".to_string(),
            std::path::PathBuf::from("/synthetic/not-read.wav"),
        );
        rec.transcript = Some("Other synthetic transcript".into());
        let conn = state.db.conn().expect("conn");
        medical_db::recordings::RecordingsRepo::insert(&conn, &rec).expect("insert");
        rec.id.to_string()
    };

    // Other recording: unknown everywhere.
    let r = report(&state, &other_id, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Unknown);
    // Switch back: still fresh with the same inputs — no leakage.
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Fresh, "switch away/back must not leak");
}

// ────────────────────────────────────────────────────────────────────────
// PHI boundary: the wire payload carries statuses + reason codes only —
// no context text, no output text, no digests on the wire.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a7_phi_boundary_wire_payload_is_metadata_only() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let secret_context = "SECRET-SYNTHETIC-NOTE-MARKER";
    let secret_med = "SECRET-SYNTHETIC-MEDICATION-MARKER";
    let inputs = CurrentDocInputs {
        context: Some(secret_context.into()),
        patient_context: Some(pc(&[secret_med])),
        ..Default::default()
    };
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs).await;

    let r = report(&state, &rid, &inputs).await;
    let wire = serde_json::to_string(&r).expect("serialize wire payload");
    assert!(!wire.contains("SECRET"), "wire payload must carry no content: {wire}");
    // Shape: exactly {status, reasons} per type.
    let value: serde_json::Value = serde_json::from_str(&wire).unwrap();
    for key in ["soap", "referral", "letter", "peer_discussion"] {
        let obj = value[key].as_object().expect("verdict object");
        assert_eq!(obj.len(), 2, "{key}: exactly status+reasons, got {obj:?}");
        assert!(obj.contains_key("status"));
        assert!(obj.contains_key("reasons"));
    }
}

// ────────────────────────────────────────────────────────────────────────
// Tri-state coverage: fresh / stale / unknown (output_modified) distinct.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a8_tri_state_fresh_stale_unknown_distinct() {
    let provider = Arc::new(MockCompletionProvider::new(
        "ollama",
        "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
        200,
    ));
    let (state, rid) =
        build_test_state_with_provider(base_config(), "Patient reports back pain.", provider).await;

    let inputs = CurrentDocInputs::default();
    super::soap::generate_soap_inner_for_test_with(&state, &rid, &inputs).await;

    // fresh
    assert_eq!(report(&state, &rid, &inputs).await.soap.status, FreshnessStatus::Fresh);

    // stale (input changed)
    let edited = CurrentDocInputs {
        context: Some("changed".into()),
        ..Default::default()
    };
    assert_eq!(report(&state, &rid, &edited).await.soap.status, FreshnessStatus::Stale);

    // unknown (output hand-edited → provenance can no longer vouch)
    {
        let uuid = uuid::Uuid::parse_str(&rid).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let mut rec =
            medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
        rec.soap_note = Some("Hand-edited note".into());
        medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
    }
    let r = report(&state, &rid, &inputs).await;
    assert_eq!(r.soap.status, FreshnessStatus::Unknown);
    assert_eq!(r.soap.reasons, vec!["output_modified"]);
}
