//! Generation freshness: authoritative, digest-based staleness verdicts for
//! the Generation tab, plus the Rust-owned effective-input digest builders
//! shared by the generation commands (write side) and the freshness read.
//!
//! Contract (spec 2026-09-11):
//! - **Rust owns the effective inputs.** The canonical `input_digest` is
//!   built here from live frontend values + current settings + resolved
//!   template/specialty + the source content the type is derived from —
//!   exactly what generation consumes. Generation commands persist the
//!   digest atomically with their output; the read side rebuilds it from
//!   the CURRENT row + CURRENT settings + the LIVE frontend values. The
//!   frontend never computes or caches digests, and a just-succeeded
//!   generation is never assumed current.
//! - **PHI-free, read-only.** `get_generation_freshness` returns per-type
//!   `{status: fresh|stale|unknown, reasons: [...]}` with non-content
//!   reason codes only. It takes no generation lock and writes nothing.
//! - **Missing provenance is never current.** A type with no provenance
//!   row (legacy outputs, generations pre-dating the table) is `unknown`
//!   with reason `missing_provenance`.
//! - **SOAP-derived types** (referral, letter) bind their SOURCE: fresh
//!   only while `source_digest == digest(recordings.soap_note)`. A
//!   hand-edited or regenerated SOAP note flags downstream outputs stale.
//! - **Peer mirrors the transcript** (its true input), not the SOAP note.
//! - **Output binding.** If the stored output no longer digests to the
//!   recorded `output_digest` (hand edit, editor autosave), the verdict is
//!   `unknown`/`output_modified` — provenance can no longer speak for an
//!   edited document.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use medical_core::error::{AppError, AppResult};
use medical_core::types::PatientContext;
use medical_core::types::recording::Recording;
use medical_core::types::settings::AppConfig;

use super::helpers::{fold_structured_context, patient_context_is_empty, resolve_soap_template};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Digest primitives
// ---------------------------------------------------------------------------

/// Canonical SHA-256 of a value's JSON serialization. Struct field order
/// is the canonical key order — the serialize side is plain data structs,
/// so this is stable across runs. One-way: a digest cannot leak content.
pub(super) fn digest_json(value: &impl Serialize) -> String {
    // Serialization of these plain data structs cannot fail; on the
    // impossible failure path an empty-byte digest is still deterministic.
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    format!("{:x}", hasher.finalize())
}

pub(super) fn digest_str(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Digest of a stored text column; trimmed-empty/absent → None (a missing
/// source contributes nothing to any input).
fn text_digest(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|t| !t.is_empty())
        .map(digest_str)
}

// ---------------------------------------------------------------------------
// Effective-input builders — THE shared truth for write and read sides
// ---------------------------------------------------------------------------

/// Settings that change generation output. Part of every type's effective
/// input — a settings change invalidates every proven output.
/// (Struct field order is the canonical JSON key order; never reorder.)
#[derive(Serialize)]
pub(super) struct EffectiveSettings<'a> {
    pub ai_model: &'a str,
    pub temperature: f32,
    pub soap_template: String,
    pub icd_version: &'a medical_core::types::settings::IcdVersion,
    pub specialty: Option<&'a str>,
    pub custom_prompt: Option<&'a str>,
}

impl<'a> EffectiveSettings<'a> {
    fn soap(config: &'a AppConfig, template_request: Option<&str>) -> Self {
        Self {
            ai_model: &config.ai_model,
            temperature: config.temperature,
            soap_template: format!("{:?}", resolve_soap_template(template_request, config)),
            icd_version: &config.icd_version,
            specialty: config.specialty.as_deref(),
            custom_prompt: config.custom_soap_prompt.as_deref(),
        }
    }

    fn simple(config: &'a AppConfig, custom_prompt: Option<&'a str>) -> Self {
        Self {
            ai_model: &config.ai_model,
            temperature: config.temperature,
            soap_template: String::new(),
            icd_version: &config.icd_version,
            specialty: config.specialty.as_deref(),
            custom_prompt,
        }
    }
}

/// The freeform-context component shared by the derived types: the F2 Rust
/// fold (structured fields + freeform context), identical on the write
/// side (generation commands) and the read side (freshness).
fn folded_context(
    patient_context: Option<&PatientContext>,
    context: Option<&str>,
) -> Option<String> {
    fold_structured_context(
        patient_context.filter(|pc| !patient_context_is_empty(pc)),
        context.map(str::trim).filter(|c| !c.is_empty()),
    )
}

/// Effective-input digest for the SOAP note.
///
/// `template_request` is the explicit template override (None from the
/// Generate tab — same as `generate_soap` receives).
pub(super) fn soap_input_digest(
    recording: &Recording,
    config: &AppConfig,
    template_request: Option<&str>,
    context: Option<&str>,
    patient_context: Option<&PatientContext>,
) -> String {
    let input = serde_json::json!({
        "v": 1,
        "transcript": text_digest(recording.transcript.as_deref()),
        // Post-F1: SOAP's freeform context is notes + OCR only; structured
        // fields travel exclusively via patient_context below.
        "context": context.map(str::trim).filter(|c| !c.is_empty()),
        "patient_context": patient_context.filter(|pc| !patient_context_is_empty(pc)),
        "settings": EffectiveSettings::soap(config, template_request),
    });
    digest_json(&input)
}

/// Effective-input digest for the referral letter.
pub(super) fn referral_input_digest(
    context_folded: Option<&str>,
    recipient_type: Option<&str>,
    urgency: Option<&str>,
    config: &AppConfig,
) -> String {
    let input = serde_json::json!({
        "v": 1,
        "context": context_folded,
        "doc": {
            "recipient_type": recipient_type.unwrap_or("Specialist"),
            "urgency": urgency.unwrap_or("routine"),
        },
        "settings": EffectiveSettings::simple(config, config.custom_referral_prompt.as_deref()),
    });
    digest_json(&input)
}

/// Effective-input digest for the patient letter.
pub(super) fn letter_input_digest(
    context_folded: Option<&str>,
    letter_type: Option<&str>,
    audience_id: Option<Uuid>,
    config: &AppConfig,
) -> String {
    let input = serde_json::json!({
        "v": 1,
        "context": context_folded,
        "doc": {
            "letter_type": letter_type.unwrap_or("follow-up"),
            "audience_id": audience_id,
        },
        "settings": EffectiveSettings::simple(config, config.custom_letter_prompt.as_deref()),
    });
    digest_json(&input)
}

/// Effective-input digest for the peer discussion note (transcript-based).
pub(super) fn peer_input_digest(
    recording: &Recording,
    context_folded: Option<&str>,
    physician_name: &str,
    specialty: &str,
    reason: &str,
    config: &AppConfig,
) -> String {
    let input = serde_json::json!({
        "v": 1,
        "transcript": text_digest(recording.transcript.as_deref()),
        "context": context_folded,
        "doc": {
            "physician_name": physician_name,
            "specialty": specialty,
            "reason": reason,
        },
        "settings": EffectiveSettings::simple(
            config,
            config.custom_peer_discussion_prompt.as_deref(),
        ),
    });
    digest_json(&input)
}

// ---------------------------------------------------------------------------
// Provenance write helper (called by generation commands, inside their lock)
// ---------------------------------------------------------------------------

/// Persist provenance for one output. Best-effort per spec — the spec's
/// freshness guarantees are read-side; a failed provenance write must never
/// fail an otherwise-successful generation, it just leaves that type
/// `unknown` until the next regeneration.
#[allow(clippy::too_many_arguments)] // flat args mirror the generation call sites
pub(super) fn record_provenance(
    conn: &rusqlite::Connection,
    recording_id: Uuid,
    doc_type: &'static str,
    provider_name: &str,
    model_name: &str,
    input_digest: String,
    output_text: &str,
    source_digest: Option<String>,
) {
    let insert = medical_db::generation_provenance::ProvenanceInsert {
        recording_id,
        doc_type,
        ai_provider: provider_name.to_string(),
        ai_model: model_name.to_string(),
        input_digest,
        output_digest: digest_str(output_text),
        source_digest,
    };
    if let Err(e) =
        medical_db::generation_provenance::GenerationProvenanceRepo::upsert(conn, &insert)
    {
        tracing::warn!(
            error = %e,
            recording_id = %recording_id,
            doc_type,
            "provenance write failed; freshness for this type will read unknown until regeneration"
        );
    }
}

// ---------------------------------------------------------------------------
// Freshness read: wire types
// ---------------------------------------------------------------------------

/// Freshness verdict for one document type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessStatus {
    Fresh,
    Stale,
    Unknown,
}

/// Per-type result. `reasons` are non-content reason codes (stable strings
/// the frontend may group on); never clinical text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FreshnessVerdict {
    pub status: FreshnessStatus,
    pub reasons: Vec<&'static str>,
}

impl FreshnessVerdict {
    fn fresh() -> Self {
        Self {
            status: FreshnessStatus::Fresh,
            reasons: vec![],
        }
    }
    fn with_reasons(status: FreshnessStatus, reasons: Vec<&'static str>) -> Self {
        Self { status, reasons }
    }
}

/// Wire shape of the freshness command: one entry per surfaced document
/// type, keyed by the same snake_case names the frontend uses.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FreshnessReport {
    pub soap: FreshnessVerdict,
    pub referral: FreshnessVerdict,
    pub letter: FreshnessVerdict,
    pub peer_discussion: FreshnessVerdict,
}

/// Live document inputs from the Generation tab, mirroring the generate
/// commands' parameters. Fields a type doesn't use are ignored when
/// building its effective input. CamelCase on the wire (Tauri convention).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentDocInputs {
    /// Freeform supporting-context text (post-F1: for SOAP this carries
    /// only notes + OCR — never structured fields).
    pub context: Option<String>,
    pub patient_context: Option<PatientContext>,
    /// Explicit SOAP template request (Generate tab always passes null;
    /// kept for parity with `generate_soap`).
    pub template: Option<String>,
    pub letter_type: Option<String>,
    pub audience_id: Option<Uuid>,
    pub recipient_type: Option<String>,
    pub urgency: Option<String>,
    pub physician_name: Option<String>,
    pub specialty: Option<String>,
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Freshness read: command
// ---------------------------------------------------------------------------

/// `get_generation_freshness` — read-only freshness verdicts for the four
/// surfaced document types.
///
/// Takes NO generation lock and writes nothing; generation commands own
/// provenance writes (inside their lock, atomically with the output).
#[tauri::command]
pub async fn get_generation_freshness(
    state: tauri::State<'_, AppState>,
    recording_id: String,
    inputs: CurrentDocInputs,
) -> AppResult<FreshnessReport> {
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::InvalidInput(format!("Invalid recording ID: {e}")))?;

    // Single blocking task, single pooled connection: recording + settings
    // load, provenance reads, and verdict computation (in-memory test
    // pools are max_size=1 — two tasks would deadlock on the pool).
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<FreshnessReport> {
        let conn = db.conn()?;
        // Same semantics as the generation loader: active-only recording
        // lookup (tombstoned rows are invisible), migrated config.
        let recording = medical_db::recordings::RecordingsRepo::get_by_id_active(&conn, &uuid)?;
        let mut config = medical_db::settings::SettingsRepo::load_config(&conn)?;
        config.migrate();
        Ok(compute_report(&conn, &recording, &config, &inputs))
    })
    .await
    .map_err(crate::commands::join_err)?
}

// ---------------------------------------------------------------------------
// Freshness read: verdict computation (pure over its inputs — test seam)
// ---------------------------------------------------------------------------

fn compute_report(
    conn: &rusqlite::Connection,
    recording: &Recording,
    config: &AppConfig,
    inputs: &CurrentDocInputs,
) -> FreshnessReport {
    use medical_db::generation_provenance::GenerationProvenanceRepo as Repo;

    let _t_digest = text_digest(recording.transcript.as_deref());
    let s_digest = text_digest(recording.soap_note.as_deref());

    // ── SOAP ────────────────────────────────────────────────────────────
    let soap_now = soap_input_digest(
        recording,
        config,
        inputs.template.as_deref(),
        inputs.context.as_deref(),
        inputs.patient_context.as_ref(),
    );
    let soap_verdict = match Repo::get(conn, recording.id, "soap") {
        Ok(None) | Err(_) => unknown_read("missing_provenance"),
        Ok(Some(p)) => {
            if s_digest.as_deref() != Some(p.output_digest.as_str()) {
                FreshnessVerdict::with_reasons(FreshnessStatus::Unknown, vec!["output_modified"])
            } else if p.input_digest != soap_now {
                FreshnessVerdict::with_reasons(FreshnessStatus::Stale, vec!["inputs_changed"])
            } else {
                FreshnessVerdict::fresh()
            }
        }
    };

    // ── Referral (SOAP-derived) ─────────────────────────────────────────
    let folded = folded_context(inputs.patient_context.as_ref(), inputs.context.as_deref());
    let referral_now = referral_input_digest(
        folded.as_deref(),
        inputs.recipient_type.as_deref(),
        inputs.urgency.as_deref(),
        config,
    );
    let referral_verdict = derived_verdict(
        Repo::get(conn, recording.id, "referral"),
        &referral_now,
        recording.referral.as_deref(),
        s_digest.as_deref(),
    );

    // ── Letter (SOAP-derived) ───────────────────────────────────────────
    let letter_now = letter_input_digest(
        folded.as_deref(),
        inputs.letter_type.as_deref(),
        inputs.audience_id,
        config,
    );
    let letter_verdict = derived_verdict(
        Repo::get(conn, recording.id, "letter"),
        &letter_now,
        recording.letter.as_deref(),
        s_digest.as_deref(),
    );

    // ── Peer discussion (transcript-mirroring) ──────────────────────────
    let peer_now = peer_input_digest(
        recording,
        folded.as_deref(),
        inputs.physician_name.as_deref().unwrap_or(""),
        inputs.specialty.as_deref().unwrap_or(""),
        inputs.reason.as_deref().unwrap_or(""),
        config,
    );
    let peer_verdict = derived_verdict(
        Repo::get(conn, recording.id, "peer_discussion"),
        &peer_now,
        recording.peer_discussion.as_deref(),
        None, // transcript-based: no SOAP source binding
    );

    FreshnessReport {
        soap: soap_verdict,
        referral: referral_verdict,
        letter: letter_verdict,
        peer_discussion: peer_verdict,
    }
}

/// Verdict for a derived type: output binding + own-input + source checks.
fn derived_verdict(
    provenance: Result<
        Option<medical_db::generation_provenance::GenerationProvenance>,
        medical_db::DbError,
    >,
    input_digest_now: &str,
    stored_output: Option<&str>,
    source_digest_now: Option<&str>,
) -> FreshnessVerdict {
    let p = match provenance {
        Ok(None) | Err(_) => return unknown_read("missing_provenance"),
        Ok(Some(p)) => p,
    };

    let mut reasons: Vec<&'static str> = Vec::new();
    let out_now = text_digest(stored_output);
    if out_now.as_deref() != Some(p.output_digest.as_str()) {
        reasons.push("output_modified");
    }
    if p.input_digest != input_digest_now {
        reasons.push("inputs_changed");
    }
    if source_digest_now.is_some() && p.source_digest.as_deref() != source_digest_now {
        reasons.push("source_changed");
    }

    if reasons.is_empty() {
        FreshnessVerdict::fresh()
    } else if reasons.contains(&"output_modified") && reasons.len() == 1 {
        FreshnessVerdict::with_reasons(FreshnessStatus::Unknown, reasons)
    } else {
        // Any input-side change is stale (the user can regenerate); a lone
        // output_modified is the only unknown (provenance cannot vouch).
        FreshnessVerdict::with_reasons(FreshnessStatus::Stale, reasons)
    }
}

/// Unknown verdict on a failed provenance read — never claim freshness
/// from a DB error.
fn unknown_read(reason: &'static str) -> FreshnessVerdict {
    FreshnessVerdict::with_reasons(FreshnessStatus::Unknown, vec![reason])
}

#[cfg(test)]
pub(crate) fn compute_report_for_test(
    conn: &rusqlite::Connection,
    recording: &Recording,
    config: &AppConfig,
    inputs: &CurrentDocInputs,
) -> FreshnessReport {
    compute_report(conn, recording, config, inputs)
}

#[cfg(test)]
mod tests {
    // The 9-case acceptance matrix lives in freshness_acceptance.rs.
}
