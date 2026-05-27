//! FHIR R4 document export.
//!
//! Produces [FHIR R4](https://hl7.org/fhir/R4/) **document Bundles** as
//! pretty-printed JSON. A Bundle always includes `Patient`, `Practitioner`, and
//! `Encounter` resources, plus `DocumentReference` resources for the SOAP note
//! (LOINC 11506-3) and transcript (LOINC 11488-4) when present.
//!
//! Document text is base64-encoded (RFC 4648, standard alphabet with padding)
//! inside [`Attachment.data`][att] fields, as required by the FHIR spec.
//!
//! [att]: https://hl7.org/fhir/R4/datatypes.html#Attachment
//!
//! > **Note:** the crate produces structurally valid FHIR JSON but does *not*
//! > run a FHIR conformance validator. Resource IDs are random UUIDs and are
//! > not stable across repeated exports of the same recording.

use base64::Engine;
use chrono::Utc;
use medical_core::types::recording::Recording;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{ExportError, ExportResult};

// ── Data structures ──────────────────────────────────────────────────────────

/// A FHIR R4 [`Bundle`](https://hl7.org/fhir/R4/bundle.html) of type
/// `"document"`.
///
/// Serialises to / deserialises from standard FHIR JSON with `resourceType`,
/// `id`, `type`, `timestamp`, and `entry` fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FhirBundle {
    /// Always `"Bundle"`.
    #[serde(rename = "resourceType")]
    pub resource_type: String,
    /// UUID assigned at export time.
    pub id: String,
    /// Always `"document"` for this exporter.
    #[serde(rename = "type")]
    pub bundle_type: String,
    /// RFC 3339 timestamp of when the bundle was generated.
    pub timestamp: String,
    /// Ordered list of resources in the bundle.
    pub entry: Vec<BundleEntry>,
}

/// A single [`entry`](https://hl7.org/fhir/R4/bundle-definitions.html#Bundle.entry)
/// inside a FHIR Bundle, wrapping an arbitrary FHIR resource as a JSON value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleEntry {
    /// The FHIR resource (Patient, Practitioner, Encounter, DocumentReference, …).
    pub resource: Value,
}

/// Optional patient demographics merged into the `Patient` resource.
///
/// Any `None` field is omitted from the FHIR output. `name` falls back to
/// [`Recording::patient_name`] if not supplied.
#[derive(Debug, Clone, Default)]
pub struct PatientInfo {
    /// Patient display name (falls back to `Recording.patient_name`).
    pub name: Option<String>,
    /// Date of birth in `YYYY-MM-DD` format.
    pub birth_date: Option<String>,
    /// Administrative gender (`male`, `female`, `other`, `unknown`).
    pub gender: Option<String>,
    /// External patient identifier (e.g. MRN).
    pub identifier: Option<String>,
}

/// Optional practitioner metadata merged into the `Practitioner` resource.
///
/// Any `None` field is omitted from the FHIR output.
#[derive(Debug, Clone, Default)]
pub struct PractitionerInfo {
    /// Practitioner display name.
    pub name: Option<String>,
    /// External practitioner identifier (e.g. NPI).
    pub identifier: Option<String>,
    /// Clinical specialty, emitted as a `qualification`.
    pub specialty: Option<String>,
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// Base64-encodes a UTF-8 string using the standard alphabet (RFC 4648, with
/// padding) — the encoding required by FHIR `Attachment.data`.
pub fn base64_encode(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

// ── Exporter ─────────────────────────────────────────────────────────────────

/// Stateless FHIR R4 exporter.
///
/// All methods are associated functions — construction is unnecessary.
///
/// # Errors
///
/// Returns [`ExportError::Fhir`] if JSON serialisation fails (should not happen
/// in practice since inputs are always serialisable primitives).
pub struct FhirExporter;

impl FhirExporter {
    /// Builds a full FHIR R4 document Bundle from a recording.
    ///
    /// The bundle always contains `Patient`, `Practitioner`, and `Encounter`
    /// resources. Conditional resources:
    ///
    /// - **`DocumentReference` (LOINC 11506-3)** — added when
    ///   `recording.soap_note` is present ("Progress note").
    /// - **`DocumentReference` (LOINC 11488-4)** — added when
    ///   `recording.transcript` is present ("Consultation note").
    ///
    /// Resource IDs are freshly generated UUIDs; the encounter period starts
    /// at `recording.created_at`.
    pub fn export_bundle(
        recording: &Recording,
        patient: PatientInfo,
        practitioner: PractitionerInfo,
    ) -> ExportResult<Vec<u8>> {
        let now = Utc::now().to_rfc3339();
        let bundle_id = Uuid::new_v4().to_string();

        let patient_id = Uuid::new_v4().to_string();
        let practitioner_id = Uuid::new_v4().to_string();
        let encounter_id = Uuid::new_v4().to_string();

        let mut entries: Vec<BundleEntry> = Vec::new();

        // ── Patient ──────────────────────────────────────────────────────────
        let patient_name = patient
            .name
            .clone()
            .or_else(|| recording.patient_name.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        let mut patient_resource = json!({
            "resourceType": "Patient",
            "id": patient_id,
            "name": [{ "text": patient_name }]
        });

        if let Some(bd) = &patient.birth_date {
            patient_resource["birthDate"] = json!(bd);
        }
        if let Some(g) = &patient.gender {
            patient_resource["gender"] = json!(g);
        }
        if let Some(ident) = &patient.identifier {
            patient_resource["identifier"] = json!([{ "value": ident }]);
        }

        entries.push(BundleEntry { resource: patient_resource });

        // ── Practitioner ─────────────────────────────────────────────────────
        let prac_name = practitioner.name.clone().unwrap_or_else(|| "Unknown".to_string());
        let mut prac_resource = json!({
            "resourceType": "Practitioner",
            "id": practitioner_id,
            "name": [{ "text": prac_name }]
        });

        if let Some(ident) = &practitioner.identifier {
            prac_resource["identifier"] = json!([{ "value": ident }]);
        }
        if let Some(spec) = &practitioner.specialty {
            prac_resource["qualification"] = json!([{
                "code": { "text": spec }
            }]);
        }

        entries.push(BundleEntry { resource: prac_resource });

        // ── Encounter ────────────────────────────────────────────────────────
        let encounter_resource = json!({
            "resourceType": "Encounter",
            "id": encounter_id,
            "status": "finished",
            "class": {
                "system": "http://terminology.hl7.org/CodeSystem/v3-ActCode",
                "code": "AMB",
                "display": "ambulatory"
            },
            "subject": { "reference": format!("Patient/{}", patient_id) },
            "participant": [{
                "individual": { "reference": format!("Practitioner/{}", practitioner_id) }
            }],
            "period": { "start": recording.created_at.to_rfc3339() }
        });
        entries.push(BundleEntry { resource: encounter_resource });

        // ── SOAP DocumentReference (LOINC 11506-3) ───────────────────────────
        if let Some(soap) = &recording.soap_note {
            let doc_ref = Self::build_document_reference(
                &patient_id,
                "Progress note",
                "11506-3",
                soap,
                &recording.created_at.to_rfc3339(),
            );
            entries.push(BundleEntry { resource: doc_ref });
        }

        // ── Transcript DocumentReference (LOINC 11488-4) ────────────────────
        if let Some(transcript) = &recording.transcript {
            let doc_ref = Self::build_document_reference(
                &patient_id,
                "Consultation note",
                "11488-4",
                transcript,
                &recording.created_at.to_rfc3339(),
            );
            entries.push(BundleEntry { resource: doc_ref });
        }

        // ── Bundle ───────────────────────────────────────────────────────────
        let bundle = FhirBundle {
            resource_type: "Bundle".to_string(),
            id: bundle_id,
            bundle_type: "document".to_string(),
            timestamp: now,
            entry: entries,
        };

        serde_json::to_vec_pretty(&bundle)
            .map_err(|e| ExportError::Fhir(format!("JSON serialization failed: {e}")))
    }

    /// Exports a standalone FHIR `DocumentReference` (not wrapped in a Bundle).
    ///
    /// Uses the SOAP note if present, otherwise falls back to the transcript.
    /// The LOINC code is hard-coded to `11506-3` ("Progress note").
    ///
    /// This is useful for simpler integrations that do not need a full Bundle.
    pub fn export_document_reference(recording: &Recording, title: &str) -> ExportResult<Vec<u8>> {
        let content = recording
            .soap_note
            .as_deref()
            .or(recording.transcript.as_deref())
            .unwrap_or("");

        let doc_ref = Self::build_document_reference(
            &Uuid::new_v4().to_string(),
            title,
            "11506-3",
            content,
            &recording.created_at.to_rfc3339(),
        );

        serde_json::to_vec_pretty(&doc_ref)
            .map_err(|e| ExportError::Fhir(format!("JSON serialization failed: {e}")))
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn build_document_reference(
        patient_id: &str,
        title: &str,
        loinc_code: &str,
        content: &str,
        date: &str,
    ) -> Value {
        let encoded = base64_encode(content);
        json!({
            "resourceType": "DocumentReference",
            "id": Uuid::new_v4().to_string(),
            "status": "current",
            "type": {
                "coding": [{
                    "system": "http://loinc.org",
                    "code": loinc_code,
                    "display": title
                }]
            },
            "subject": { "reference": format!("Patient/{}", patient_id) },
            "date": date,
            "content": [{
                "attachment": {
                    "contentType": "text/plain",
                    "data": encoded,
                    "title": title
                }
            }]
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use medical_core::types::recording::Recording;

    fn make_recording() -> Recording {
        let mut r = Recording::new("test.wav", PathBuf::from("/tmp/test.wav"));
        r.soap_note = Some("S: patient complains\nO: normal\nA: healthy\nP: rest".to_string());
        r.transcript = Some("Doctor: how are you? Patient: fine.".to_string());
        r.patient_name = Some("Jane Doe".to_string());
        r
    }

    #[test]
    fn export_bundle_valid_json() {
        let recording = make_recording();
        let bytes = FhirExporter::export_bundle(
            &recording,
            PatientInfo::default(),
            PractitionerInfo::default(),
        )
        .unwrap();

        let json: Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(json["resourceType"], "Bundle");
        assert_eq!(json["type"], "document");
    }

    #[test]
    fn contains_patient_resource() {
        let recording = make_recording();
        let bytes = FhirExporter::export_bundle(
            &recording,
            PatientInfo::default(),
            PractitionerInfo::default(),
        )
        .unwrap();

        let bundle: FhirBundle = serde_json::from_slice(&bytes).unwrap();
        let has_patient = bundle
            .entry
            .iter()
            .any(|e| e.resource["resourceType"] == "Patient");
        assert!(has_patient);
    }

    #[test]
    fn contains_soap_doc_ref() {
        let recording = make_recording();
        let bytes = FhirExporter::export_bundle(
            &recording,
            PatientInfo::default(),
            PractitionerInfo::default(),
        )
        .unwrap();

        let bundle: FhirBundle = serde_json::from_slice(&bytes).unwrap();
        let has_doc_ref = bundle.entry.iter().any(|e| {
            e.resource["resourceType"] == "DocumentReference"
                && e.resource["type"]["coding"][0]["code"] == "11506-3"
        });
        assert!(has_doc_ref);
    }

    #[test]
    fn export_doc_reference() {
        let recording = make_recording();
        let bytes = FhirExporter::export_document_reference(&recording, "Progress note").unwrap();
        let json: Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(json["resourceType"], "DocumentReference");
        assert_eq!(json["status"], "current");
    }

    #[test]
    fn recording_without_soap_still_exports() {
        let mut recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        recording.transcript = Some("Some transcript".to_string());
        // No soap_note set

        let bytes = FhirExporter::export_bundle(
            &recording,
            PatientInfo::default(),
            PractitionerInfo::default(),
        )
        .unwrap();

        let bundle: FhirBundle = serde_json::from_slice(&bytes).unwrap();

        // Should have Patient, Practitioner, Encounter (no SOAP doc ref)
        let soap_doc_ref = bundle.entry.iter().any(|e| {
            e.resource["resourceType"] == "DocumentReference"
                && e.resource["type"]["coding"][0]["code"] == "11506-3"
        });
        assert!(!soap_doc_ref, "SOAP doc ref should not be present");

        // But transcript doc ref should be present
        let transcript_doc_ref = bundle.entry.iter().any(|e| {
            e.resource["resourceType"] == "DocumentReference"
                && e.resource["type"]["coding"][0]["code"] == "11488-4"
        });
        assert!(transcript_doc_ref, "Transcript doc ref should be present");
    }
}
