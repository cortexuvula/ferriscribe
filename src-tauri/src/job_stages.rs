//! The job-stage vocabulary shared by the progress emitters (the pipeline
//! and generation commands) and the mobile API's `JobRegistry`.
//!
//! These labels cross THREE consumers that used to share them only as
//! copy-pasted string literals: the Tauri events the desktop frontend
//! renders, the `JobRegistry` stage snapshots served over
//! `/v1/jobs/{id}`, and the queue-time safety net in the mobile generate
//! handler. One home so an edit on the emitter side cannot silently break
//! the consumer side (values are pinned by tests — do not change them
//! casually; the frontend matches them too).

/// Job accepted, command not yet started (mobile generate queue-time mark).
pub(crate) const QUEUED: &str = "queued";
/// STT in flight (pipeline stage 1).
pub(crate) const TRANSCRIBING: &str = "transcribing";
/// SOAP generation in flight (the pipeline's fixed second stage).
pub(crate) const GENERATING_SOAP: &str = "generating_soap";
/// Terminal success (also the `generation-progress` success status).
pub(crate) const COMPLETED: &str = "completed";
/// Terminal failure. As a `generation-progress` status it carries a
/// `": message"` suffix (see `commands::generation::format_progress_error`).
pub(crate) const FAILED: &str = "failed";
/// `generation-progress` status meaning the command started.
pub(crate) const STATUS_STARTED: &str = "started";

/// The stage label for a document generation: `generating_{doc_type}` —
/// the mobile registry derives its stage from the event's doc type.
pub(crate) fn generating(doc_type: &str) -> String {
    format!("generating_{doc_type}")
}

/// Whether a stage is terminal — a job in one of these states never
/// changes again.
pub(crate) fn is_terminal(stage: &str) -> bool {
    stage == COMPLETED || stage == FAILED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generating_composes_the_doc_prefixed_stage() {
        assert_eq!(generating("synopsis"), "generating_synopsis");
        assert_eq!(generating("peer_discussion"), "generating_peer_discussion");
    }

    #[test]
    fn terminal_stages_are_exactly_completed_and_failed() {
        assert!(is_terminal(COMPLETED));
        assert!(is_terminal(FAILED));
        for live in [
            QUEUED,
            TRANSCRIBING,
            GENERATING_SOAP,
            "generating_soap_extra",
        ] {
            assert!(!is_terminal(live), "{live} must not be terminal");
        }
    }
}
