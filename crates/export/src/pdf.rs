//! PDF document export using [printpdf](https://docs.rs/printpdf/0.7).
//!
//! Generates A4 PDFs with built-in Helvetica fonts (Latin-1 only). Long SOAP
//! notes overflow onto additional pages automatically.

use medical_core::types::recording::Recording;
use printpdf::*;

use crate::{ExportError, ExportResult};

// ── Exporter ─────────────────────────────────────────────────────────────────

/// Stateless PDF exporter.
///
/// All methods are associated functions — construct is unnecessary.
///
/// # Errors
///
/// Returns [`ExportError::Pdf`] when the recording is missing the required
/// content (e.g. no SOAP note for [`export_soap`](Self::export_soap)) or when
/// `printpdf` fails during font loading or serialisation.
pub struct PdfExporter;

impl PdfExporter {
    /// Exports the SOAP note from a recording as a PDF document.
    ///
    /// The SOAP text is rendered with bold section headers (`S:`, `O:`, `A:`,
    /// `P:`) and the recording date as a subtitle.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Pdf`] if `recording.soap_note` is `None`.
    pub fn export_soap(recording: &Recording) -> ExportResult<Vec<u8>> {
        let soap = recording
            .soap_note
            .as_deref()
            .ok_or_else(|| ExportError::Pdf("Recording has no SOAP note".to_string()))?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("SOAP Note", soap, &date)
    }

    /// Exports the referral letter from a recording as a PDF document.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Pdf`] if `recording.referral` is `None`.
    pub fn export_referral(recording: &Recording) -> ExportResult<Vec<u8>> {
        let referral = recording
            .referral
            .as_deref()
            .ok_or_else(|| ExportError::Pdf("Recording has no referral letter".to_string()))?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("Referral Letter", referral, &date)
    }

    /// Exports the general patient letter from a recording as a PDF document.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Pdf`] if `recording.letter` is `None`.
    pub fn export_letter(recording: &Recording) -> ExportResult<Vec<u8>> {
        let letter = recording
            .letter
            .as_deref()
            .ok_or_else(|| ExportError::Pdf("Recording has no letter".to_string()))?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("Letter", letter, &date)
    }
}

// ── Renderer ─────────────────────────────────────────────────────────────────

/// SOAP section header prefixes that should be rendered in bold.
const SOAP_HEADERS: &[&str] = &["S:", "O:", "A:", "P:"];

/// Renders an A4 PDF with a title, date subtitle, and line-by-line body.
///
/// # Layout
///
/// - **Title**: 16 pt Helvetica-Bold, left-aligned.
/// - **Date**: 10 pt Helvetica, left-aligned below the title.
/// - **Body**: one paragraph per line. Lines starting with `S:`, `O:`, `A:`,
///   or `P:` are rendered in 11 pt Helvetica-Bold; all others in 10 pt
///   Helvetica.
/// - When the y-cursor falls below the 10 mm bottom margin a new A4 page is
///   appended and rendering continues from the top.
///
/// # Errors
///
/// Returns [`ExportError::Pdf`] if the built-in fonts cannot be loaded or the
/// document cannot be serialised.
pub fn render_document(title: &str, body: &str, date: &str) -> ExportResult<Vec<u8>> {
    const A4_WIDTH: f32 = 210.0;
    const A4_HEIGHT: f32 = 297.0;
    const MARGIN_LEFT: f32 = 15.0;
    const MARGIN_TOP: f32 = 280.0;
    const MARGIN_BOTTOM: f32 = 10.0;
    const LINE_HEIGHT: f32 = 6.0;

    let (doc, page1, layer1) = PdfDocument::new(title, Mm(A4_WIDTH), Mm(A4_HEIGHT), "Main Layer");

    let font = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .map_err(|e| ExportError::Pdf(format!("Font load error: {e}")))?;
    let font_bold = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .map_err(|e| ExportError::Pdf(format!("Bold font load error: {e}")))?;

    let mut current_layer = doc.get_page(page1).get_layer(layer1);
    let mut y = MARGIN_TOP;

    // Title
    current_layer.use_text(title, 16.0, Mm(MARGIN_LEFT), Mm(y), &font_bold);
    y -= LINE_HEIGHT * 1.5;

    // Date
    current_layer.use_text(date, 10.0, Mm(MARGIN_LEFT), Mm(y), &font);
    y -= LINE_HEIGHT * 2.0;

    // Body — line by line
    for line in body.lines() {
        if y < MARGIN_BOTTOM {
            // Page overflow — create a new page and continue.
            let (new_page, new_layer) = doc.add_page(Mm(A4_WIDTH), Mm(A4_HEIGHT), "Main Layer");
            current_layer = doc.get_page(new_page).get_layer(new_layer);
            y = MARGIN_TOP;
        }
        let is_header = SOAP_HEADERS.iter().any(|&h| line.starts_with(h));
        if is_header {
            current_layer.use_text(line, 11.0, Mm(MARGIN_LEFT), Mm(y), &font_bold);
        } else {
            current_layer.use_text(line, 10.0, Mm(MARGIN_LEFT), Mm(y), &font);
        }
        y -= LINE_HEIGHT;
    }

    let mut buf: Vec<u8> = Vec::new();
    doc.save(&mut std::io::BufWriter::new(&mut buf))
        .map_err(|e| ExportError::Pdf(format!("PDF save error: {e}")))?;

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
    fn export_soap_produces_pdf() {
        let recording = recording_with_soap();
        let bytes = PdfExporter::export_soap(&recording).expect("export OK");
        assert!(!bytes.is_empty());
        // PDF files start with the %PDF- magic bytes
        assert!(bytes.starts_with(b"%PDF-"), "not a valid PDF");
    }

    #[test]
    fn export_without_note_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = PdfExporter::export_soap(&recording);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("SOAP note"));
    }

    #[test]
    fn export_referral_without_referral_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = PdfExporter::export_referral(&recording);
        assert!(result.is_err());
    }

    #[test]
    fn export_letter_without_letter_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = PdfExporter::export_letter(&recording);
        assert!(result.is_err());
    }

    #[test]
    fn export_long_soap_creates_multi_page_pdf() {
        let mut rec = Recording::new("test.wav", PathBuf::from("/tmp/test.wav"));
        rec.soap_note = Some(
            (0..200)
                .map(|i| format!("Line {i}: Patient presents with symptoms"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        rec.patient_name = Some("Test Patient".into());
        let bytes = PdfExporter::export_soap(&rec).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..5], b"%PDF-");
    }
}
