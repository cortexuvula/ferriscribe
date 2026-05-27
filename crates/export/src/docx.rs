//! DOCX (Office Open XML) document export using
//! [docx-rs](https://docs.rs/docx-rs/0.4).
//!
//! Produces `.docx` files (which are ZIP archives — magic bytes `PK`) with a
//! simple hard-coded layout: title, date, and body. SOAP section headers are
//! rendered in bold.

use std::io::Cursor;

use docx_rs::*;
use medical_core::types::recording::Recording;

use crate::{ExportError, ExportResult};

// ── Exporter ─────────────────────────────────────────────────────────────────

/// Stateless DOCX exporter.
///
/// All methods are associated functions — construction is unnecessary.
///
/// # Errors
///
/// Returns [`ExportError::Docx`] when the recording is missing the required
/// content (e.g. no SOAP note for [`export_soap`](Self::export_soap)) or when
/// `docx-rs` fails during packing.
pub struct DocxExporter;

impl DocxExporter {
    /// Exports the SOAP note from a recording as a DOCX document.
    ///
    /// The SOAP text is rendered with bold section headers (`S:`, `O:`, `A:`,
    /// `P:`) and the recording date right-aligned beneath the title.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Docx`] if `recording.soap_note` is `None`.
    pub fn export_soap(recording: &Recording) -> ExportResult<Vec<u8>> {
        let soap = recording.soap_note.as_deref().ok_or_else(|| {
            ExportError::Docx("Recording has no SOAP note".to_string())
        })?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("SOAP Note", soap, &date)
    }

    /// Exports the referral letter from a recording as a DOCX document.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Docx`] if `recording.referral` is `None`.
    pub fn export_referral(recording: &Recording) -> ExportResult<Vec<u8>> {
        let referral = recording.referral.as_deref().ok_or_else(|| {
            ExportError::Docx("Recording has no referral letter".to_string())
        })?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("Referral Letter", referral, &date)
    }

    /// Exports the general patient letter from a recording as a DOCX document.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Docx`] if `recording.letter` is `None`.
    pub fn export_letter(recording: &Recording) -> ExportResult<Vec<u8>> {
        let letter = recording.letter.as_deref().ok_or_else(|| {
            ExportError::Docx("Recording has no letter".to_string())
        })?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("Letter", letter, &date)
    }
}

// ── Renderer ─────────────────────────────────────────────────────────────────

/// SOAP section header prefixes that should be rendered in bold.
const SOAP_HEADERS: &[&str] = &["S:", "O:", "A:", "P:"];

/// Renders a DOCX document with a title, date, and line-by-line body.
///
/// # Layout
///
/// - **Title**: centred, bold, size 32 half-points (16 pt).
/// - **Date**: right-aligned, gray `#888888`, size 20 half-points (10 pt).
/// - **Body**: one paragraph per line. Lines starting with `S:`, `O:`, `A:`,
///   or `P:` are bold at size 24 (12 pt); all others are size 22 (11 pt).
///
/// The resulting `Docx` is packed into a byte buffer via
/// [`Docx::build().pack(...)`](docx_rs::Docx).
///
/// # Errors
///
/// Returns [`ExportError::Docx`] if the DOCX cannot be packed to bytes.
pub fn render_document(title: &str, body: &str, date: &str) -> ExportResult<Vec<u8>> {
    let mut docx = Docx::new();

    // ── Title ────────────────────────────────────────────────────────────────
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(title).bold().size(32))
            .align(AlignmentType::Center),
    );

    // ── Date ─────────────────────────────────────────────────────────────────
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(date).size(20).color("888888"))
            .align(AlignmentType::Right),
    );

    // ── Body ─────────────────────────────────────────────────────────────────
    for line in body.lines() {
        let is_header = SOAP_HEADERS.iter().any(|&h| line.starts_with(h));
        let para = if is_header {
            Paragraph::new().add_run(Run::new().add_text(line).bold().size(24))
        } else {
            Paragraph::new().add_run(Run::new().add_text(line).size(22))
        };
        docx = docx.add_paragraph(para);
    }

    // ── Pack to bytes ────────────────────────────────────────────────────────
    let mut buf: Vec<u8> = Vec::new();
    docx.build()
        .pack(Cursor::new(&mut buf))
        .map_err(|e| ExportError::Docx(format!("DOCX pack error: {e}")))?;

    Ok(buf)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use medical_core::types::recording::Recording;

    fn recording_with_soap() -> Recording {
        let mut r = Recording::new("visit.wav", PathBuf::from("/tmp/visit.wav"));
        r.soap_note = Some(
            "S: Patient reports headache\nO: BP 120/80\nA: Tension headache\nP: Ibuprofen 400mg"
                .to_string(),
        );
        r
    }

    #[test]
    fn export_soap_produces_docx() {
        let recording = recording_with_soap();
        let bytes = DocxExporter::export_soap(&recording).expect("export OK");
        assert!(!bytes.is_empty());
        // DOCX files are ZIP archives — they start with the PK magic bytes (0x50 0x4B)
        assert!(
            bytes.starts_with(&[0x50, 0x4B]),
            "not a valid DOCX/ZIP (no PK magic)"
        );
    }

    #[test]
    fn export_without_note_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = DocxExporter::export_soap(&recording);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("SOAP note"));
    }

    #[test]
    fn export_referral_without_referral_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = DocxExporter::export_referral(&recording);
        assert!(result.is_err());
    }

    #[test]
    fn export_letter_without_letter_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = DocxExporter::export_letter(&recording);
        assert!(result.is_err());
    }
}
