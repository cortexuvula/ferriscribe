# medical-export

Export clinical documents from FerriScribe recordings to **PDF**, **DOCX**, and **FHIR R4** formats.

## How It Fits

```
medical-core (types::Recording)
    |
    v
medical-export  <-- you are here
    |
    v
src-tauri (Tauri commands: export_pdf, export_docx, export_fhir)
```

The crate consumes `medical_core::types::Recording` values and produces raw byte
buffers (`Vec<u8>`) that the Tauri layer writes to disk or streams back to the
frontend. It has no knowledge of the filesystem, UI, or IPC — it is a pure
data-in / bytes-out library.

## Key Types

| Type | Module | Role |
|------|--------|------|
| `PdfExporter` | `pdf` | Stateless builder for PDF documents (printpdf) |
| `DocxExporter` | `docx` | Stateless builder for DOCX documents (docx-rs) |
| `FhirExporter` | `fhir` | Builds FHIR R4 Bundles and DocumentReferences |
| `FhirBundle` | `fhir` | Serializable FHIR Bundle (document type) |
| `BundleEntry` | `fhir` | Single resource entry inside a Bundle |
| `PatientInfo` | `fhir` | Optional demographics merged into the Patient resource |
| `PractitionerInfo` | `fhir` | Optional clinician metadata merged into the Practitioner resource |
| `ExportError` | root | Unified error enum (`Pdf` / `Docx` / `Fhir` / `Io`) |
| `ExportResult<T>` | root | Convenience alias for `Result<T, ExportError>` |

## How Each Format Works

### PDF (`pdf.rs`)

Uses **printpdf 0.7** with built-in Helvetica / Helvetica-Bold fonts.

1. Create an A4 (210 x 297 mm) document.
2. Render a bold 16 pt title, a 10 pt date line, then body lines at 10 pt.
3. Lines starting with `S:`, `O:`, `A:`, or `P:` are rendered in bold 11 pt.
4. When the y-cursor drops below the bottom margin a new page is appended
   automatically — long SOAP notes are multi-page.
5. The document is serialised into a `Vec<u8>` via `PdfDocument::save`.

### DOCX (`docx.rs`)

Uses **docx-rs 0.4** to produce an Office Open XML `.docx` (a ZIP archive).

1. Title paragraph: centred, bold, size 32 half-points (16 pt).
2. Date paragraph: right-aligned, gray `#888888`, size 20 (10 pt).
3. Body paragraphs: SOAP section headers bold at size 24 (12 pt), regular
   lines at size 22 (11 pt).
4. The `Docx` is packed into a `Cursor<Vec<u8>>` via `docx.build().pack(...)`.

### FHIR R4 (`fhir.rs`)

Produces a **FHIR R4 document Bundle** as pretty-printed JSON.

Bundle contents (in order):

| Resource | Notes |
|----------|-------|
| `Patient` | Merges `PatientInfo` fields with `Recording.patient_name` fallback |
| `Practitioner` | Merges `PractitionerInfo` fields |
| `Encounter` | Status `finished`, class `AMB` (ambulatory), references Patient + Practitioner |
| `DocumentReference` (LOINC 11506-3) | SOAP note — only if present. Text is base64-encoded |
| `DocumentReference` (LOINC 11488-4) | Transcript — only if present. Text is base64-encoded |

`export_document_reference` produces a standalone `DocumentReference` resource
(not wrapped in a Bundle) for simpler integrations.

## Public API Quick Reference

```rust
// PDF
let bytes: Vec<u8> = PdfExporter::export_soap(&recording)?;
let bytes: Vec<u8> = PdfExporter::export_referral(&recording)?;
let bytes: Vec<u8> = PdfExporter::export_letter(&recording)?;
let bytes: Vec<u8> = pdf::render_document("Title", "body", "2026-05-26")?;

// DOCX
let bytes: Vec<u8> = DocxExporter::export_soap(&recording)?;
let bytes: Vec<u8> = DocxExporter::export_referral(&recording)?;
let bytes: Vec<u8> = DocxExporter::export_letter(&recording)?;
let bytes: Vec<u8> = docx::render_document("Title", "body", "2026-05-26")?;

// FHIR
let bytes: Vec<u8> = FhirExporter::export_bundle(
    &recording, PatientInfo::default(), PractitionerInfo::default(),
)?;
let bytes: Vec<u8> = FhirExporter::export_document_reference(&recording, "Progress note")?;

// Utility
let encoded: String = fhir::base64_encode("plain text");
```

All exporter methods return `ExportResult<Vec<u8>>`. Errors are non-recoverable
(missing content or serialisation failure) — the caller decides how to surface
them to the user.

## Gotchas

1. **FHIR is *structural*, not validated.** The crate produces well-formed FHIR
   JSON but does **not** run a FHIR validator. If you need strict conformance
   (e.g. for submission to an EHR), run the output through a validator such as
   the HL7 FHIR CLI tool. Resource IDs are random UUIDs — not stable across
   repeated exports of the same recording.

2. **PDF fonts are built-in only.** `printpdf`'s `BuiltinFont::Helvetica` family
   covers Latin-1. Non-Latin characters (CJK, Arabic, etc.) will render as
   missing glyphs. Adding external TTF fonts is possible but not yet wired up.

3. **DOCX has no template support.** The layout is hard-coded. If you need
   letterhead, logos, or footer disclaimers you will need to extend
   `render_document` or switch to a template-based approach.

4. **No PHI in logs.** Per project-wide HIPAA constraints, this crate never logs
   document content. Errors carry descriptive messages (`"Recording has no SOAP
   note"`) but never the recording's text fields.

5. **Base64 encoding in FHIR** uses the standard alphabet (RFC 4648 with
   padding), which is what the FHIR spec requires for `Attachment.data`.

6. **Missing content returns an error, not empty output.** `export_soap` on a
   recording without a SOAP note, or `export_referral` without a referral, will
   return `Err(ExportError::Pdf/Docx(...))` rather than a blank document. The
   FHIR exporter is more lenient — it simply omits the corresponding
   `DocumentReference` from the bundle.
