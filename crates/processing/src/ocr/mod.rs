//! Document OCR pipeline: extract text from files (text, images, PDFs).
//!
//! Text files are read directly. Images are sent to a vision model. PDFs
//! are text-extracted (scanned-PDF rasterization is deferred to v2).
//!
//! **HIPAA note:** Extracted content is never logged — only filenames and
//! page/character counts appear in tracing.

use std::path::Path;

use medical_core::traits::ai_provider::AiProvider;
use medical_core::types::ai::{
    CompletionRequest, ContentPart, ImageUrlData, Message, MessageContent, Role,
};

/// Result of extracting text from one file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OcrPageResult {
    pub filename: String,
    pub text: String,
    pub page_count: usize,
}

/// Error from the OCR pipeline.
#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("file not found: {0}")]
    FileNotFound(String),
    #[error("unsupported file type: {0}")]
    UnsupportedType(String),
    #[error("failed to read file: {0}")]
    ReadError(String),
    #[error("PDF text extraction failed: {0}")]
    PdfExtraction(String),
    #[error("OCR model error: {0}")]
    ModelError(String),
    #[error("no text extracted from: {0}")]
    EmptyExtraction(String),
}

/// Supported file extensions for OCR.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp"];
const TEXT_EXTENSIONS: &[&str] = &["txt", "md", "csv"];
const PDF_EXTENSIONS: &[&str] = &["pdf"];

/// Classify a file by its extension into an OCR strategy.
fn classify(path: &Path) -> Result<OcrStrategy, OcrError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if TEXT_EXTENSIONS.contains(&ext.as_str()) {
        Ok(OcrStrategy::TextFile)
    } else if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Ok(OcrStrategy::Image)
    } else if PDF_EXTENSIONS.contains(&ext.as_str()) {
        Ok(OcrStrategy::Pdf)
    } else {
        Err(OcrError::UnsupportedType(ext))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OcrStrategy {
    TextFile,
    Image,
    Pdf,
}

/// Read a text file directly — no model call needed.
fn read_text_file(path: &Path) -> Result<String, OcrError> {
    std::fs::read_to_string(path).map_err(|e| OcrError::ReadError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_file_passthrough_reads_content() {
        let dir = tempfile::NamedTempFile::new().unwrap();
        let path = dir.path().with_extension("txt");
        std::fs::write(&path, "Lab Results\nHbA1c: 7.2%").unwrap();
        let text = read_text_file(&path).unwrap();
        assert!(text.contains("HbA1c: 7.2%"));
    }

    #[test]
    fn classify_txt_is_text_file() {
        let path = Path::new("report.txt");
        assert_eq!(classify(path).unwrap(), OcrStrategy::TextFile);
    }

    #[test]
    fn classify_png_is_image() {
        let path = Path::new("scan.png");
        assert_eq!(classify(path).unwrap(), OcrStrategy::Image);
    }

    #[test]
    fn classify_pdf_is_pdf() {
        let path = Path::new("doc.pdf");
        assert_eq!(classify(path).unwrap(), OcrStrategy::Pdf);
    }

    #[test]
    fn classify_docx_is_unsupported() {
        let path = Path::new("doc.docx");
        assert!(matches!(classify(path), Err(OcrError::UnsupportedType(_))));
    }
}
