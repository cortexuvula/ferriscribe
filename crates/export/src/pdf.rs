//! PDF document export using [printpdf](https://docs.rs/printpdf/0.7).
//!
//! Generates A4 PDFs with built-in Helvetica fonts (Windows-1252 only —
//! characters outside that repertoire are mapped to `?` with a counted
//! warning rather than silently dropped). Lines longer than the printable
//! width are wrapped at an approximate character budget; long SOAP notes
//! overflow onto additional pages automatically.

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

    /// Exports the synopsis from a recording's metadata as a PDF document.
    ///
    /// The synopsis has no dedicated column; it is stored under
    /// `metadata.synopsis` by `generate_synopsis`.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Pdf`] if the metadata carries no synopsis.
    pub fn export_synopsis(recording: &Recording) -> ExportResult<Vec<u8>> {
        let synopsis = crate::synopsis_text(recording)
            .ok_or_else(|| ExportError::Pdf("Recording has no synopsis".to_string()))?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("Synopsis", synopsis, &date)
    }

    /// Exports the peer-to-peer discussion note as a PDF document.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Pdf`] if `recording.peer_discussion` is `None`.
    pub fn export_peer_discussion(recording: &Recording) -> ExportResult<Vec<u8>> {
        let discussion = recording
            .peer_discussion
            .as_deref()
            .ok_or_else(|| ExportError::Pdf("Recording has no peer discussion note".to_string()))?;
        let date = recording.created_at.format("%Y-%m-%d").to_string();
        render_document("Peer Discussion", discussion, &date)
    }
}

// ── Renderer ─────────────────────────────────────────────────────────────────

/// SOAP section header prefixes that should be rendered in bold.
const SOAP_HEADERS: &[&str] = &["S:", "O:", "A:", "P:"];

// ── Layout constants ─────────────────────────────────────────────────────────

const A4_WIDTH: f32 = 210.0;
const A4_HEIGHT: f32 = 297.0;
const MARGIN_LEFT: f32 = 15.0;
const MARGIN_RIGHT: f32 = 15.0;
const MARGIN_TOP: f32 = 280.0;
const MARGIN_BOTTOM: f32 = 10.0;
const LINE_HEIGHT: f32 = 6.0;

const TITLE_FONT_SIZE: f32 = 16.0;
const HEADER_FONT_SIZE: f32 = 11.0;
const BODY_FONT_SIZE: f32 = 10.0;

/// PostScript points per millimetre (72 pt = 1 inch = 25.4 mm).
const PT_PER_MM: f32 = 72.0 / 25.4;

/// Conservative average glyph width for the built-in Helvetica faces, as a
/// fraction of the font size (mixed-case text averages ~0.5 em; 0.6 errs
/// toward wrapping early so a wrapped line never exceeds the printable
/// width).
const AVG_GLYPH_WIDTH_EM: f32 = 0.6;

/// Approximate characters that fit on one printed line at `font_size_pt`.
fn chars_per_line(font_size_pt: f32) -> usize {
    let printable_pt = (A4_WIDTH - MARGIN_LEFT - MARGIN_RIGHT) * PT_PER_MM;
    ((printable_pt / (font_size_pt * AVG_GLYPH_WIDTH_EM)) as usize).max(1)
}

// ── Text preparation ─────────────────────────────────────────────────────────

/// Is `c` inside the Windows-1252 (WinAnsi) repertoire the printpdf 0.7
/// built-in fonts encode? Anything else has no glyph and is dropped
/// silently by the encoder unless mapped first.
fn win_ansi_representable(c: char) -> bool {
    match c {
        '\t' => true,                // defined byte in cp1252
        ' '..='~' => true,           // printable ASCII
        '\u{A0}'..='\u{FF}' => true, // Latin-1 supplement
        // The Windows-1252 0x80–0x9F special block (defined code points
        // only): € ‚ ƒ „ … † ‡ ˆ ‰ Š ‹ Œ Ž ' ' " " • – — ˜ ™ š › œ ž Ÿ
        '\u{20AC}' | '\u{201A}' | '\u{0192}' | '\u{201E}' | '\u{2026}' | '\u{2020}'
        | '\u{2021}' | '\u{02C6}' | '\u{2030}' | '\u{0160}' | '\u{2039}' | '\u{0152}'
        | '\u{017D}' | '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '\u{2022}'
        | '\u{2013}' | '\u{2014}' | '\u{02DC}' | '\u{2122}' | '\u{0161}' | '\u{203A}'
        | '\u{0153}' | '\u{017E}' | '\u{0178}' => true,
        _ => false,
    }
}

/// Map `input` onto the Windows-1252 repertoire: characters outside it
/// become `?` (counted) so the export proceeds without silently losing
/// bytes — the built-in Helvetica faces cannot render them.
fn to_win_ansi(input: &str) -> (String, usize) {
    let mut replaced = 0usize;
    let mapped = input
        .chars()
        .map(|c| {
            if win_ansi_representable(c) {
                c
            } else {
                replaced += 1;
                '?'
            }
        })
        .collect();
    (mapped, replaced)
}

/// One rendered body line: the (possibly wrapped) text plus whether it is
/// a bold SOAP section header.
struct BodyLine {
    text: String,
    header: bool,
}

/// Split `body` into rendered lines: each source line becomes one or more
/// output lines wrapped at the approximate character budget for its font
/// size (no silent clipping past the printable width). Empty source lines
/// are preserved as empty rendered lines (they consume a line of vertical
/// space, exactly as before wrapping existed).
fn layout_body_lines(body: &str) -> Vec<BodyLine> {
    let mut out = Vec::new();
    for line in body.lines() {
        let header = SOAP_HEADERS.iter().any(|&h| line.starts_with(h));
        let budget = chars_per_line(if header {
            HEADER_FONT_SIZE
        } else {
            BODY_FONT_SIZE
        });
        for text in wrap_line(line, budget) {
            out.push(BodyLine { text, header });
        }
    }
    out
}

/// Greedy word-wrap at an approximate character budget. A single word
/// longer than the budget is split at the budget (nothing is dropped);
/// an empty/blank line yields one empty output line.
fn wrap_line(line: &str, budget: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        // A word on its own longer than the budget: hard-split it.
        let mut word_chars = word.chars().collect::<Vec<_>>();
        while word_chars.len() > budget {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let head: String = word_chars.drain(..budget).collect();
            out.push(head);
        }
        let word: String = word_chars.into_iter().collect();
        let separator = if current.is_empty() { 0 } else { 1 };
        if current.chars().count() + separator + word.chars().count() > budget
            && !current.is_empty()
        {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&word);
    }
    out.push(current);
    out
}

// ── Document assembly ────────────────────────────────────────────────────────

/// Renders an A4 PDF with a title, date subtitle, and line-by-line body.
///
/// # Layout
///
/// - **Title**: 16 pt Helvetica-Bold, left-aligned.
/// - **Date**: 10 pt Helvetica, left-aligned below the title.
/// - **Body**: one paragraph per line. Lines starting with `S:`, `O:`, `A:`,
///   or `P:` are rendered in 11 pt Helvetica-Bold; all others in 10 pt
///   Helvetica. Lines longer than the printable width are wrapped at an
///   approximate character budget.
/// - Characters outside the Windows-1252 repertoire of the built-in fonts
///   are mapped to `?`; when any were replaced, a single counted warning
///   is emitted (no content) and the export proceeds.
/// - When the y-cursor falls below the 10 mm bottom margin a new A4 page is
///   appended and rendering continues from the top.
///
/// # Errors
///
/// Returns [`ExportError::Pdf`] if the built-in fonts cannot be loaded or the
/// document cannot be serialised.
pub fn render_document(title: &str, body: &str, date: &str) -> ExportResult<Vec<u8>> {
    let (mapped_body, replaced) = to_win_ansi(body);
    if replaced > 0 {
        // Count only — never the text (PHI).
        tracing::warn!(
            count = replaced,
            "PDF export: replaced characters outside the built-in fonts' Windows-1252 repertoire with '?'"
        );
    }

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
    current_layer.use_text(title, TITLE_FONT_SIZE, Mm(MARGIN_LEFT), Mm(y), &font_bold);
    y -= LINE_HEIGHT * 1.5;

    // Date
    current_layer.use_text(date, BODY_FONT_SIZE, Mm(MARGIN_LEFT), Mm(y), &font);
    y -= LINE_HEIGHT * 2.0;

    // Body — line by line (wrapped), one page overflow check per rendered
    // line so wrapped tails also paginate.
    for BodyLine { text, header } in layout_body_lines(&mapped_body) {
        if y < MARGIN_BOTTOM {
            // Page overflow — create a new page and continue.
            let (new_page, new_layer) = doc.add_page(Mm(A4_WIDTH), Mm(A4_HEIGHT), "Main Layer");
            current_layer = doc.get_page(new_page).get_layer(new_layer);
            y = MARGIN_TOP;
        }
        if header {
            current_layer.use_text(
                text.as_str(),
                HEADER_FONT_SIZE,
                Mm(MARGIN_LEFT),
                Mm(y),
                &font_bold,
            );
        } else {
            current_layer.use_text(text.as_str(), BODY_FONT_SIZE, Mm(MARGIN_LEFT), Mm(y), &font);
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

    #[test]
    fn export_synopsis_produces_pdf() {
        let mut rec = Recording::new("visit.wav", PathBuf::from("/tmp/visit.wav"));
        rec.metadata = serde_json::json!({ "synopsis": "Brief synopsis: tension headache." });
        let bytes = PdfExporter::export_synopsis(&rec).expect("export OK");
        assert!(bytes.starts_with(b"%PDF-"), "not a valid PDF");
    }

    #[test]
    fn export_synopsis_without_synopsis_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = PdfExporter::export_synopsis(&recording);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("synopsis"));
    }

    #[test]
    fn export_synopsis_ignores_empty_string() {
        let mut rec = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        rec.metadata = serde_json::json!({ "synopsis": "" });
        let result = PdfExporter::export_synopsis(&rec);
        assert!(result.is_err());
    }

    #[test]
    fn export_peer_discussion_produces_pdf() {
        let mut rec = Recording::new("visit.wav", PathBuf::from("/tmp/visit.wav"));
        rec.peer_discussion = Some("Discussion: recommend MRI to rule out pathology.".to_string());
        let bytes = PdfExporter::export_peer_discussion(&rec).expect("export OK");
        assert!(bytes.starts_with(b"%PDF-"), "not a valid PDF");
    }

    #[test]
    fn export_peer_discussion_without_note_errors() {
        let recording = Recording::new("empty.wav", PathBuf::from("/tmp/empty.wav"));
        let result = PdfExporter::export_peer_discussion(&recording);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("peer discussion"));
    }

    // ── Wrap + WinAnsi mapping (2026-10-08 review) ─────────────────────────

    #[test]
    fn layout_wraps_lines_longer_than_the_page_width() {
        // One source line far past the printable width (~85 chars at 10 pt).
        let long = "word ".repeat(200).trim_end().to_string();
        let lines = layout_body_lines(&long);
        assert!(
            lines.len() > 1,
            "a line longer than the page width must be wrapped, got {} line",
            lines.len()
        );
        for line in &lines {
            assert!(
                line.text.chars().count() <= chars_per_line(BODY_FONT_SIZE),
                "wrapped line exceeds the budget: {}",
                line.text.chars().count()
            );
        }
        // Nothing lost: every source word appears in the wrapped output.
        let joined = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for word in long.split_whitespace() {
            assert!(joined.contains(word), "word lost to wrapping: {word}");
        }
    }

    #[test]
    fn layout_wraps_overlong_header_lines_at_the_header_budget() {
        let header = format!("S: {}", "detail ".repeat(100));
        let lines = layout_body_lines(header.trim_end());
        assert!(lines.len() > 1, "header line must wrap");
        assert!(lines[0].header, "wrapped tail keeps the header flag");
        for line in &lines {
            assert!(line.text.chars().count() <= chars_per_line(HEADER_FONT_SIZE));
        }
    }

    #[test]
    fn layout_preserves_short_lines_and_empty_lines() {
        let lines = layout_body_lines("S: Patient reports headache\n\nO: BP 120/80");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].header);
        assert_eq!(lines[1].text, "", "empty line keeps its vertical slot");
        assert!(lines[2].header);
        assert_eq!(lines[0].text, "S: Patient reports headache");
    }

    #[test]
    fn wrap_line_hard_splits_an_overlong_single_word() {
        let wrapped = wrap_line("abcdefghij", 4);
        assert_eq!(wrapped, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn win_ansi_mapping_replaces_out_of_repertoire_chars() {
        // CJK has no WinAnsi byte — replaced, counted.
        let (mapped, count) = to_win_ansi("血压 blood pressure");
        assert_eq!(count, 2);
        assert!(mapped.starts_with("??"));
        assert!(mapped.contains("blood pressure"));

        // In-repertoire characters pass through untouched: em dash, é, •.
        let (mapped, count) = to_win_ansi("pain — café • bullets");
        assert_eq!(count, 0);
        assert_eq!(mapped, "pain — café • bullets");
    }

    #[test]
    fn export_with_cjk_characters_replaces_and_does_not_panic() {
        let mut rec = recording_with_soap();
        rec.soap_note = Some("S: 血压偏高\nA: 头痛\nP: 继续观察".to_string());
        let bytes = PdfExporter::export_soap(&rec).expect("export with replacements succeeds");
        assert!(bytes.starts_with(b"%PDF-"), "still a valid PDF");
    }
}
