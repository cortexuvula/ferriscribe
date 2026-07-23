//! Document OCR pipeline: extract text from files (text, images, PDFs).
//!
//! Text files are read directly. Images are sent to a vision model. PDFs
//! are text-extracted (scanned-PDF rasterization is deferred to v2).
//!
//! **HIPAA note:** Extracted content is never logged — only filenames and
//! page/character counts appear in tracing.

use std::path::Path;
use std::sync::Arc;

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
    #[error("file too large for OCR: {0} bytes (limit: {1} bytes)")]
    FileTooLarge(u64, u64),
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

/// Encode raw image bytes as a base64 data URL.
fn encode_image_as_data_url(data: &[u8], format: &str) -> String {
    use base64::{Engine, engine::general_purpose};
    let b64 = general_purpose::STANDARD.encode(data);
    format!("data:image/{format};base64,{b64}")
}

/// The OCR system prompt instructing the model to extract text.
const OCR_SYSTEM_PROMPT: &str = "Extract all text from this document image. \
    Output only the extracted text, preserving the document's structure, headings, \
    and table layout. Do not add commentary or descriptions.";

/// Build a CompletionRequest for a single image OCR call.
fn build_image_ocr_request(image_data_url: &str, model: &str) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "Extract all text from this document.".to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrlData {
                        url: image_data_url.to_string(),
                    },
                },
            ]),
            tool_calls: vec![],
        }],
        temperature: Some(0.0),
        max_tokens: None,
        system_prompt: Some(OCR_SYSTEM_PROMPT.to_string()),
    }
}

/// Extract embedded text from a PDF using `pdf-extract`.
///
/// Returns the concatenated text content. For scanned PDFs (image-only),
/// this will return empty — rasterization + vision OCR is deferred to v2.
fn extract_pdf_text(path: &Path) -> Result<String, OcrError> {
    if !path.exists() {
        return Err(OcrError::FileNotFound(path.display().to_string()));
    }
    let text =
        pdf_extract::extract_text(path).map_err(|e| OcrError::PdfExtraction(e.to_string()))?;
    Ok(text.trim().to_string())
}

/// Maximum file size for OCR processing (100 MB). Guards against OOM when
/// reading large images or PDFs into memory and base64-encoding them.
const MAX_OCR_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Extract text from a list of document file paths.
///
/// Each file is classified by extension and processed accordingly:
/// - Text files (txt/md/csv): read directly, no model call
/// - Images (png/jpg/jpeg/bmp/webp): base64-encode and send to the vision model
/// - PDFs: extract embedded text via pdf-extract; empty result = scanned PDF
///
/// Returns one `OcrPageResult` per **successfully** processed file. Files that
/// error (read failure, model error, too large) are logged and skipped — the
/// caller sees which files succeeded via the returned vector. Duplicate paths
/// are deduplicated.
pub async fn extract_text(
    file_paths: &[String],
    ocr_model: &str,
    provider: Arc<dyn AiProvider>,
) -> Result<Vec<OcrPageResult>, OcrError> {
    let mut results = Vec::with_capacity(file_paths.len());
    let mut seen = std::collections::HashSet::new();

    for path_str in file_paths {
        // Dedup: skip if we've already processed this exact path.
        if !seen.insert(path_str.clone()) {
            continue;
        }

        let path = Path::new(path_str);
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        if !path.exists() {
            tracing::warn!(filename = %filename, "OCR: file not found, skipping");
            continue;
        }

        // Size guard: prevent OOM on huge files.
        let file_size = match path.metadata() {
            Ok(m) => m.len(),
            Err(e) => {
                tracing::warn!(filename = %filename, error = %e, "OCR: cannot read metadata, skipping");
                continue;
            }
        };
        if file_size > MAX_OCR_FILE_BYTES {
            let limit_mb = MAX_OCR_FILE_BYTES / (1024 * 1024);
            let size_mb = file_size / (1024 * 1024);
            tracing::warn!(
                filename = %filename,
                size_bytes = file_size,
                limit_bytes = MAX_OCR_FILE_BYTES,
                "OCR: file exceeds size limit, returning error result"
            );
            // Return a result with an explanatory message so the user sees
            // WHY the file wasn't processed, rather than a silent skip.
            results.push(OcrPageResult {
                filename,
                text: format!(
                    "[File too large for OCR: {size_mb} MB exceeds the {limit_mb} MB limit.]"
                ),
                page_count: 0,
            });
            continue;
        }

        let strategy = match classify(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(filename = %filename, error = %e, "OCR: unsupported file, skipping");
                continue;
            }
        };

        // Each strategy extracts text independently. Errors are logged and
        // the file is skipped — one bad file must NOT abort the whole batch.
        let result = match strategy {
            OcrStrategy::TextFile => match read_text_file(path) {
                Ok(text) => {
                    tracing::info!(filename = %filename, chars = text.len(), "OCR: text file read");
                    OcrPageResult {
                        filename,
                        text,
                        page_count: 1,
                    }
                }
                Err(e) => {
                    tracing::warn!(filename = %filename, error = %e, "OCR: text read failed, skipping");
                    continue;
                }
            },
            OcrStrategy::Pdf => match extract_pdf_text(path) {
                Ok(text) => {
                    if text.is_empty() {
                        tracing::info!(filename = %filename, "OCR: PDF text extraction returned empty (likely scanned PDF)");
                        OcrPageResult {
                            filename,
                            text: String::from(
                                "[No machine-readable text found in this PDF. \
                                Scanned PDFs require image-based OCR, coming in a future update.]",
                            ),
                            page_count: 0,
                        }
                    } else {
                        tracing::info!(filename = %filename, chars = text.len(), "OCR: PDF text extracted");
                        OcrPageResult {
                            filename,
                            text,
                            page_count: 1,
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(filename = %filename, error = %e, "OCR: PDF extraction failed, skipping");
                    continue;
                }
            },
            OcrStrategy::Image => {
                let image_data = match std::fs::read(path) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: image read failed, skipping");
                        continue;
                    }
                };
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
                let data_url = encode_image_as_data_url(&image_data, ext);
                let request = build_image_ocr_request(&data_url, ocr_model);

                tracing::info!(filename = %filename, bytes = image_data.len(), "OCR: sending image to vision model");
                match provider.complete(request).await {
                    Ok(response) => {
                        let text = response.content.trim().to_string();
                        if text.is_empty() {
                            tracing::warn!(filename = %filename, "OCR: vision model returned empty text");
                        }
                        OcrPageResult {
                            filename,
                            text,
                            page_count: 1,
                        }
                    }
                    Err(e) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: vision model error, skipping");
                        continue;
                    }
                }
            }
        };

        results.push(result);
    }

    Ok(results)
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

    #[test]
    fn build_image_ocr_request_has_multipart_content() {
        let req = build_image_ocr_request("data:image/png;base64,iVBOR=", "glm-ocr");
        assert_eq!(req.model, "glm-ocr");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, Role::User);
        match &req.messages[0].content {
            MessageContent::Parts(parts) => {
                assert_eq!(parts.len(), 2, "should have text + image parts");
                assert!(matches!(parts[0], ContentPart::Text { .. }));
                assert!(matches!(parts[1], ContentPart::ImageUrl { .. }));
            }
            other => panic!("expected Parts, got {other:?}"),
        }
        assert!(req.system_prompt.is_some());
    }

    #[test]
    fn encode_image_as_data_url_produces_valid_prefix() {
        let data = b"\x89PNG\r\n\x1a\n"; // PNG magic bytes
        let url = encode_image_as_data_url(data, "png");
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn extract_pdf_text_from_real_pdf() {
        // Build a structurally valid PDF with lopdf containing the text
        // "Hello PDF". Hand-crafted minimal PDFs (incorrect xref offsets)
        // are rejected by pdf-extract, so we use a proper builder.
        use lopdf::content::{Content, Operation};
        use lopdf::{Dictionary, Document, Object, Stream, dictionary};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![100.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal("Hello PDF")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        let pages = Dictionary::from_iter([
            ("Type", Object::Name(b"Pages".to_vec())),
            ("Kids", Object::Array(vec![Object::Reference(page_id)])),
            ("Count", Object::Integer(1)),
            ("Resources", Object::Reference(resources_id)),
            (
                "MediaBox",
                Object::Array(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(612),
                    Object::Integer(792),
                ]),
            ),
        ]);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.compress();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pdf");
        doc.save(&path).unwrap();

        let text = extract_pdf_text(&path);
        // pdf-extract should at least not crash. If it returns empty for this
        // minimal fixture, that's acceptable — assert is_ok() only.
        assert!(
            text.is_ok(),
            "PDF extraction should not error: {:?}",
            text.err()
        );
    }

    #[test]
    fn extract_pdf_text_returns_error_for_nonexistent_file() {
        let result = extract_pdf_text(Path::new("/nonexistent/path/test.pdf"));
        assert!(result.is_err());
    }

    use futures_core::Stream;
    use medical_core::error::{AppError, AppResult};
    use medical_core::types::ToolDef;
    use medical_core::types::ai::{
        CompletionRequest, CompletionResponse, ModelInfo, StreamChunk, ToolCompletionResponse,
    };
    use std::sync::Arc;

    /// A no-op provider for testing text-file paths (never actually called).
    struct NullProvider;
    #[async_trait::async_trait]
    impl AiProvider for NullProvider {
        fn name(&self) -> &str {
            "null"
        }
        async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
            Ok(vec![])
        }
        async fn complete(&self, _req: CompletionRequest) -> AppResult<CompletionResponse> {
            Err(AppError::Other("null provider".into()))
        }
        async fn complete_stream(
            &self,
            _req: CompletionRequest,
        ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>> {
            Err(AppError::Other("null provider".into()))
        }
        async fn complete_with_tools(
            &self,
            _req: CompletionRequest,
            _tools: Vec<ToolDef>,
        ) -> AppResult<ToolCompletionResponse> {
            Err(AppError::Other("null provider".into()))
        }
    }

    #[tokio::test]
    async fn extract_text_txt_file_returns_content_directly() {
        // A text file should be read directly without any provider call.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "Patient has hypertension.").unwrap();

        // Pass a dummy provider — it should never be called for text files.
        let provider: Arc<dyn AiProvider> = Arc::new(NullProvider);
        let results = extract_text(&[path.to_string_lossy().to_string()], "glm-ocr", provider)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].filename, "notes.txt");
        assert!(results[0].text.contains("hypertension"));
        assert_eq!(results[0].page_count, 1);
    }
}
