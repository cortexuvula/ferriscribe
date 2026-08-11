//! Tauri commands for AI-powered document generation (SOAP, referral, letter, synopsis).
//!
//! Each command lives in its own submodule; helpers shared between them sit in
//! [`helpers`]. Tauri command names and the public path
//! `commands::generation::*` are unchanged from the pre-split layout.

use serde::Serialize;

use medical_core::error::AppError;

mod helpers;
pub mod letter;
pub mod letter_writer;
pub mod peer_discussion;
pub mod referral;
pub mod soap;
pub mod synopsis;
#[cfg(test)]
pub(super) mod test_helpers;

// Re-exposed for `commands::pipeline`, which validates the same payload before
// kicking off its own generation flow.
pub(super) use helpers::validate_patient_context;

// Re-exposed for `commands::ocr`, which needs to resolve the configured AI
// provider before calling the OCR pipeline.
pub(super) use helpers::resolve_provider;

// ---------------------------------------------------------------------------
// Input size bounds
// ---------------------------------------------------------------------------

/// Maximum size of a user-supplied `context` string (roughly ~12k tokens).
/// Prevents pasting multi-megabyte documents that would blow past provider
/// token limits or cause excessive memory usage.
pub(super) const MAX_CONTEXT_CHARS: usize = 50_000;

/// Per-list item count cap on `PatientContext`. Generous against realistic
/// clinical input; exists to reject pathological payloads.
pub(super) const PATIENT_CTX_MAX_ITEMS_PER_LIST: usize = 50;

/// Per-item character cap on `PatientContext` entries. A single med string
/// like "Lisinopril 10mg PO daily once in the morning with food" is well
/// under this; an entry over 500 chars is malformed input.
pub(super) const PATIENT_CTX_MAX_ITEM_CHARS: usize = 500;

/// Maximum size of a recording transcript we will send to a provider.
/// A 2-hour transcript at ~150 wpm is ~180k chars; 500k gives comfortable
/// headroom while still catching obviously-corrupt or runaway inputs.
pub(super) const MAX_TRANSCRIPT_CHARS: usize = 500_000;

/// Maximum size of a SOAP note re-fed into downstream document generation
/// (referral / letter / synopsis). SOAP notes are AI-generated, so this is
/// a sanity upper bound rather than an expected boundary.
pub(super) const MAX_SOAP_NOTE_CHARS: usize = 500_000;

/// Maximum size of a source document (e.g. OCR'd text) accepted by the
/// standalone Letter Writer. Multi-page scans can run long, so this matches
/// the SOAP/transcript headroom; it exists to reject pathological input.
pub(super) const MAX_DOCUMENT_CHARS: usize = 500_000;

// ---------------------------------------------------------------------------
// Progress event payload
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(super) struct GenerationProgress {
    #[serde(rename = "type")]
    pub doc_type: String,
    pub status: String,
    pub recording_id: String,
}

/// Format an error for a `generation-progress` "failed" event. Falls back to a
/// kind-tagged placeholder when `unwrap_app_error_message_ref` returns an empty
/// or whitespace-only string, so the frontend never sees just `"failed: "`.
pub(super) fn format_progress_error(err: &AppError) -> String {
    let msg = crate::commands::unwrap_app_error_message_ref(err);
    let trimmed = msg.trim();
    if trimmed.is_empty() {
        format!("failed: unknown error ({})", err.kind_str())
    } else {
        format!("failed: {}", trimmed)
    }
}
