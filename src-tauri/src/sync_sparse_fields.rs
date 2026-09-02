//! Shared builder for the sparse `fields` map of a wire `SyncRecording`.
//!
//! Single source of truth for BOTH sync directions — the client's push path
//! (`commands/content_sync.rs`) and the server's pull responses
//! (`sharing_vocab_api/content_sync.rs`). The two used to carry ~92-line
//! near-identical copies that had already diverged in signature; a comparator
//! change on one side would silently desync per-field LWW, so the logic lives
//! here once.

use std::collections::HashMap;

use medical_core::types::recording::Recording;
use medical_db::content_sync::{FieldRevision, SyncFieldValue};

/// Build the sparse field map for a recording.
///
/// For each syncable field that has content, look up its revision (if any)
/// to get the precise `updated_at` + `origin_device`; otherwise fall back to
/// the recording's row-level `updated_at`. Only fields with content are
/// included — the map is sparse by design so absent fields don't participate
/// in the merge.
///
/// `synced_from` is stripped from `metadata` on BOTH sides: it is a
/// local-only marker that must never round-trip to its origin machine.
pub(crate) fn build_sparse_fields(
    rec: &Recording,
    revisions: &[FieldRevision],
) -> HashMap<String, SyncFieldValue> {
    let mut fields: HashMap<String, SyncFieldValue> = HashMap::new();
    let rev_map: HashMap<&str, &FieldRevision> =
        revisions.iter().map(|r| (r.field.as_str(), r)).collect();

    let row_ts = rec
        .updated_at
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| rec.created_at.to_rfc3339());

    // Field wire timestamp = max(revision, row write). Writers that bump
    // only the row (transcription/generation completion via
    // `RecordingsRepo::update`) leave stale revisions from a pre-edit sync
    // round-trip; shipping the stale revision timestamp ties against the
    // peer's copy and the merge's Equal arm silently drops the newer
    // value. Parsed comparison — string comparison is wrong across the two
    // stored timestamp formats. When the row is newer the origin device is
    // unknown (the row bump doesn't carry one).
    let field_ts = |rev: Option<&FieldRevision>| -> (String, Option<String>) {
        match rev {
            Some(r) => {
                if medical_db::content_sync::cmp_lww_timestamps(&r.updated_at, &row_ts)
                    == std::cmp::Ordering::Less
                {
                    (row_ts.clone(), None)
                } else {
                    (r.updated_at.clone(), r.origin_device.clone())
                }
            }
            None => (row_ts.clone(), None),
        }
    };

    let mut push_text = |name: &str, val: Option<&str>| {
        if let Some(s) = val {
            let (ts, device) = field_ts(rev_map.get(name).copied());
            fields.insert(
                name.to_string(),
                SyncFieldValue {
                    value: serde_json::Value::String(s.to_string()),
                    updated_at: ts,
                    origin_device: device,
                },
            );
        }
    };

    push_text("transcript", rec.transcript.as_deref());
    push_text("soap_note", rec.soap_note.as_deref());
    push_text("referral", rec.referral.as_deref());
    push_text("letter", rec.letter.as_deref());
    push_text("peer_discussion", rec.peer_discussion.as_deref());
    push_text("chat", rec.chat.as_deref());
    push_text("patient_name", rec.patient_name.as_deref());

    let mut push_json = |name: &str, val: &serde_json::Value| {
        if !val.is_null() {
            let (ts, device) = field_ts(rev_map.get(name).copied());
            fields.insert(
                name.to_string(),
                SyncFieldValue {
                    value: val.clone(),
                    updated_at: ts,
                    origin_device: device,
                },
            );
        }
    };

    // tags is a Vec<String> on the struct; serialize to JSON.
    if let Ok(tags_json) = serde_json::to_value(&rec.tags) {
        push_json("tags", &tags_json);
    }
    // synced_from is a local-only marker — strip before it can round-trip.
    let mut metadata_clean = rec.metadata.clone();
    if let Some(obj) = metadata_clean.as_object_mut() {
        obj.remove("synced_from");
    }
    push_json("metadata", &metadata_clean);
    let status_json = serde_json::to_value(&rec.status).unwrap_or(serde_json::Value::Null);
    push_json("processing_status", &status_json);

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec_with_metadata(metadata: serde_json::Value) -> Recording {
        let mut rec = Recording::new("visit.wav", "/tmp/visit.wav".into());
        rec.transcript = Some("transcript text".into());
        rec.patient_name = Some("Doe, Jane".into());
        rec.tags = vec!["follow-up".into()];
        rec.metadata = metadata;
        rec
    }

    fn rev(field: &str, updated_at: &str) -> FieldRevision {
        FieldRevision {
            field: field.into(),
            updated_at: updated_at.into(),
            origin_device: Some("device-a".into()),
        }
    }

    #[test]
    fn includes_present_fields_and_skips_absent_ones() {
        let fields = build_sparse_fields(&rec_with_metadata(serde_json::json!({})), &[]);
        assert!(fields.contains_key("transcript"));
        assert!(fields.contains_key("patient_name"));
        assert!(fields.contains_key("tags"));
        assert!(fields.contains_key("processing_status"));
        assert!(
            !fields.contains_key("soap_note"),
            "sparse: absent stays out"
        );
    }

    #[test]
    fn strips_synced_from_marker_from_metadata() {
        let fields = build_sparse_fields(
            &rec_with_metadata(serde_json::json!({
                "synced_from": "server-row-id",
                "context": "visit notes"
            })),
            &[],
        );
        let meta = fields.get("metadata").expect("metadata present");
        let obj = meta.value.as_object().expect("object");
        assert!(
            obj.get("synced_from").is_none(),
            "marker must not round-trip"
        );
        assert_eq!(
            obj.get("context").and_then(|v| v.as_str()),
            Some("visit notes")
        );
    }

    #[test]
    fn row_timestamp_wins_over_stale_revision() {
        // Revision older than the row write → the row stamp ships, with no
        // origin device (the row bump doesn't carry one).
        let mut r = rec_with_metadata(serde_json::json!({}));
        let now = chrono::Utc::now();
        r.updated_at = Some(now);
        let stale = "2020-01-01T00:00:00+00:00";
        let fields = build_sparse_fields(&r, &[rev("transcript", stale)]);
        let t = fields.get("transcript").expect("present");
        assert_eq!(t.origin_device, None);
        assert_ne!(t.updated_at, stale);
    }

    #[test]
    fn fresh_revision_wins_with_its_origin_device() {
        let fields = build_sparse_fields(
            &rec_with_metadata(serde_json::json!({})),
            &[rev("transcript", "2999-01-01T00:00:00+00:00")],
        );
        let t = fields.get("transcript").expect("present");
        assert_eq!(t.origin_device.as_deref(), Some("device-a"));
        assert_eq!(t.updated_at, "2999-01-01T00:00:00+00:00");
    }
}
