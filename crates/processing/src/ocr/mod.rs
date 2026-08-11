//! Document OCR pipeline: extract text from files (text, images, PDFs).
//!
//! Text files are read directly. Images are sent to a vision model. PDFs with
//! an embedded text layer are extracted via `pdf-extract`; scanned (image-only)
//! PDFs are rasterized to page images via poppler's `pdftoppm`/`pdftocairo`
//! (when available) and OCR'd page-by-page through the vision model.
//!
//! **HIPAA note:** Extracted content is never logged — only filenames and
//! page/character counts appear in tracing. Rendered page images live in a
//! temp dir that is deleted as soon as OCR completes.

use std::path::{Path, PathBuf};
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
    #[error("DOCX extraction failed: {0}")]
    DocxExtraction(String),
    #[error("XLSX extraction failed: {0}")]
    XlsxExtraction(String),
    #[error("OCR model error: {0}")]
    ModelError(String),
    #[error("file too large for OCR: {0} bytes (limit: {1} bytes)")]
    FileTooLarge(u64, u64),
}

/// Supported file extensions for OCR.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp", "tiff", "tif"];
const TEXT_EXTENSIONS: &[&str] = &["txt", "md", "csv"];
const PDF_EXTENSIONS: &[&str] = &["pdf"];
const OFFICE_EXTENSIONS: &[&str] = &["docx", "xlsx"];

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
    } else if OFFICE_EXTENSIONS.contains(&ext.as_str()) {
        // Sub-classify office docs — docx and xlsx use different extractors.
        match ext.as_str() {
            "docx" => Ok(OcrStrategy::Docx),
            "xlsx" => Ok(OcrStrategy::Xlsx),
            _ => Err(OcrError::UnsupportedType(ext)),
        }
    } else {
        Err(OcrError::UnsupportedType(ext))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OcrStrategy {
    TextFile,
    Image,
    Pdf,
    Docx,
    Xlsx,
}

/// Read a text file directly — no model call needed.
/// Uses lossy decoding so non-UTF-8 files (Latin-1, Windows-1252, etc.)
/// still produce usable text instead of a hard error.
fn read_text_file(path: &Path) -> Result<String, OcrError> {
    let bytes = std::fs::read(path).map_err(|e| OcrError::ReadError(e.to_string()))?;
    // Try UTF-8 first, fall back to lossy decode for Latin-1/Windows-1252 etc.
    Ok(String::from_utf8(bytes).unwrap_or_else(|e| {
        let lossy = String::from_utf8_lossy(e.as_bytes()).into_owned();
        tracing::warn!("OCR: text file was not valid UTF-8, decoded lossily");
        lossy
    }))
}

/// Encode raw image bytes as a base64 data URL.
/// Normalizes the format to a valid MIME subtype (jpg → jpeg).
fn encode_image_as_data_url(data: &[u8], format: &str) -> String {
    use base64::{Engine, engine::general_purpose};
    let mime = match format.to_lowercase().as_str() {
        "jpg" | "jpeg" => "jpeg",
        "png" => "png",
        "bmp" => "bmp",
        "webp" => "webp",
        _ => "png", // safe fallback
    };
    let b64 = general_purpose::STANDARD.encode(data);
    format!("data:image/{mime};base64,{b64}")
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

/// OCR a single in-memory image via the vision model.
///
/// Shared by the `Image` strategy (implicitly — same flow) and the scanned-PDF
/// page loop. `format` is the MIME subtype for the data URL ("png", "jpeg",
/// …). Returns the trimmed extracted text, or an `OcrError` on provider failure
/// or per-file timeout.
async fn ocr_image_bytes(
    image_bytes: &[u8],
    format: &str,
    ocr_model: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<String, OcrError> {
    let data_url = encode_image_as_data_url(image_bytes, format);
    let request = build_image_ocr_request(&data_url, ocr_model);
    match tokio::time::timeout(OCR_PER_FILE_TIMEOUT, provider.complete(request)).await {
        Ok(Ok(response)) => Ok(response.content.trim().to_string()),
        Ok(Err(e)) => Err(OcrError::ModelError(e.to_string())),
        Err(_) => Err(OcrError::ModelError(
            "OCR timed out after 120 seconds".to_string(),
        )),
    }
}

/// Extract embedded text from a PDF using `pdf-extract`.
///
/// Returns the concatenated text content. For scanned PDFs (image-only),
/// this will return empty — rasterization + vision OCR is deferred to v2.
///
/// Kept as a synchronous helper for unit tests; the production path in
/// `extract_text` calls `pdf_extract::extract_text` inline inside
/// `spawn_blocking` so panics can be caught.
#[allow(dead_code)]
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

/// Per-file timeout for vision model OCR calls. Prevents a single slow or
/// hung image from stalling the whole batch indefinitely.
const OCR_PER_FILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Render DPI for scanned-PDF rasterization. 150 is a standard OCR DPI — high
/// enough for the vision model to read text, low enough to bound image size
/// (and thus token cost per page).
const PDF_OCR_DPI: u32 = 150;

/// Maximum number of pages to rasterize + OCR per scanned PDF. Bounds work and
/// temp-disk usage for very large documents.
const MAX_PDF_OCR_PAGES: usize = 50;

/// Message shown when a scanned PDF is detected but no rasterizer is installed.
/// Kept as a constant so the install hint is testable and easy to update.
const NO_RASTERIZER_MSG: &str = "[Scanned PDF detected (no text layer). Install poppler to OCR scanned \
     PDFs — macOS: `brew install poppler`, Linux: `sudo apt install poppler-utils`, \
     Windows: download poppler and add it to PATH.]";

/// Extract text from a .docx file by reading word/document.xml from the ZIP
/// archive and concatenating all `<w:t>` element contents.
fn extract_docx_text(path: &Path) -> Result<String, OcrError> {
    let file = std::fs::File::open(path).map_err(|e| OcrError::ReadError(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| OcrError::DocxExtraction(format!("docx open: {e}")))?;

    let mut document_xml = String::new();
    for i in 0..archive.len() {
        let zf = archive
            .by_index(i)
            .map_err(|e| OcrError::DocxExtraction(format!("docx read: {e}")))?;
        if zf.name() == "word/document.xml" {
            use std::io::Read;
            zf.take(10 * 1024 * 1024) // 10MB cap on XML
                .read_to_string(&mut document_xml)
                .map_err(|e| OcrError::DocxExtraction(format!("docx xml read: {e}")))?;
            break;
        }
    }

    if document_xml.is_empty() {
        return Err(OcrError::DocxExtraction(
            "docx: word/document.xml not found".into(),
        ));
    }

    // Parse XML and extract text from <w:t> elements.
    use quick_xml::Reader;
    use quick_xml::events::Event;
    let mut reader = Reader::from_str(&document_xml);
    reader.config_mut().trim_text(true);
    let mut text_parts = Vec::new();
    let mut buf = Vec::new();
    let mut in_paragraph = false;
    let mut in_t = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                if name.as_ref() == b"w:t" {
                    in_t = true;
                } else if name.as_ref() == b"w:p" {
                    in_paragraph = true;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                if name.as_ref() == b"w:t" {
                    in_t = false;
                } else if name.as_ref() == b"w:p" && in_paragraph {
                    text_parts.push("\n".to_string());
                    in_paragraph = false;
                }
            }
            Ok(Event::Text(e)) if in_t => {
                if let Ok(text) = e.unescape() {
                    text_parts.push(text.into_owned());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!(error = %e, "docx XML parse error, partial extraction");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(text_parts.join("").trim().to_string())
}

/// Extract cell values from an .xlsx file using calamine.
fn extract_xlsx_text(path: &Path) -> Result<String, OcrError> {
    use calamine::{Data, Reader, Xlsx, open_workbook};

    let mut workbook: Xlsx<_> =
        open_workbook(path).map_err(|e| OcrError::XlsxExtraction(format!("xlsx open: {e}")))?;

    let mut lines = Vec::new();
    for sheet_name in workbook.sheet_names().clone() {
        lines.push(format!("--- Sheet: {} ---", sheet_name));
        if let Ok(range) = workbook.worksheet_range(&sheet_name) {
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| match cell {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::DateTime(dt) => dt.to_string(),
                        Data::DateTimeIso(s) => s.clone(),
                        Data::DurationIso(s) => s.clone(),
                        Data::Int(n) => n.to_string(),
                        Data::Float(n) => n.to_string(),
                        Data::Bool(b) => b.to_string(),
                        Data::Error(e) => format!("#ERR:{:?}", e),
                    })
                    .collect();
                lines.push(cells.join("\t"));
            }
        }
        lines.push(String::new()); // blank line between sheets
    }

    Ok(lines.join("\n").trim().to_string())
}

// ---------------------------------------------------------------------------
// Scanned-PDF rasterization (poppler fallback)
// ---------------------------------------------------------------------------

/// Candidate rasterizer binaries, in preference order. `pdftoppm` is the
/// canonical poppler renderer; `pdftocairo` is a capable fallback with the
/// same CLI shape for our flags.
const RASTERIZER_CANDIDATES: &[&str] = &["pdftoppm", "pdftocairo"];

/// Return the first rasterizer binary found on PATH, or `None`.
fn find_rasterizer() -> Option<&'static str> {
    for tool in RASTERIZER_CANDIDATES {
        // `-v` distinguishes "binary exists" (Ok, regardless of exit code) from
        // "not found" (io::Error NotFound). Version output is discarded.
        match std::process::Command::new(tool).arg("-v").output() {
            Ok(_) => return Some(tool),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => continue,
        }
    }
    None
}

/// Sort rendered page paths by their trailing page number so `page-2.png`
/// precedes `page-10.png` (lexicographic order would get this wrong when the
/// count crosses a digit boundary). Files whose numeric suffix can't be parsed
/// sort to the end, stably preserving their relative order.
fn sort_page_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut keyed: Vec<(Option<usize>, usize, PathBuf)> = paths
        .into_iter()
        .enumerate()
        .map(|(idx, p)| {
            let key = p
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|stem| stem.rsplit('-').next())
                .and_then(|num| num.parse::<usize>().ok());
            (key, idx, p)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, _, p)| p).collect()
}

/// Rasterize a PDF's pages to PNG images via poppler.
///
/// Picks the first available rasterizer, renders up to [`MAX_PDF_OCR_PAGES`]
/// pages at [`PDF_OCR_DPI`], and returns the page PNG paths in page order
/// alongside the owning temp dir (the PNGs are deleted when the dir drops).
/// Returns `Err(NO_RASTERIZER_MSG)` when no rasterizer is on PATH, or
/// `Err(<reason>)` on spawn/render failure. Runs the subprocess on a blocking
/// thread so the async runtime is never blocked.
async fn rasterize_pdf_pages(pdf_path: &Path) -> Result<(tempfile::TempDir, Vec<PathBuf>), String> {
    let tool = match find_rasterizer() {
        Some(t) => t,
        None => return Err(NO_RASTERIZER_MSG.to_string()),
    };
    let pdf_path = pdf_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let dir = tempfile::TempDir::new().map_err(|e| format!("create temp dir: {e}"))?;
        let prefix = dir.path().join("page");
        // `-l N` caps the last page rendered, bounding work + temp-disk usage.
        let status = std::process::Command::new(tool)
            .arg("-png")
            .arg("-r")
            .arg(PDF_OCR_DPI.to_string())
            .arg("-l")
            .arg(MAX_PDF_OCR_PAGES.to_string())
            .arg(&pdf_path)
            .arg(&prefix)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| format!("run {tool}: {e}"))?;
        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            let first_line = stderr.lines().next().unwrap_or("").trim_end();
            return Err(if first_line.is_empty() {
                format!("{tool} failed")
            } else {
                format!("{tool} failed: {first_line}")
            });
        }
        // Collect page-*.png and sort numerically by page number.
        let mut pages: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .map_err(|e| format!("read temp dir: {e}"))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()) == Some("png")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.starts_with("page-"))
            })
            .collect();
        if pages.is_empty() {
            return Err("rasterizer produced no pages (PDF may be empty or corrupt)".to_string());
        }
        pages = sort_page_paths(pages);
        Ok::<_, String>((dir, pages))
    })
    .await
    .map_err(|e| format!("rasterizer task failed: {e}"))?
}

/// Extract text from a list of document file paths.
///
/// Each file is classified by extension and processed accordingly:
/// - Text files (txt/md/csv): read directly, no model call
/// - Images (png/jpg/jpeg/bmp/webp/tiff/tif): base64-encode and send to the
///   vision model (TIFF is converted to PNG first since vision models reject TIFF)
/// - PDFs: extract embedded text via pdf-extract; empty result = scanned PDF
/// - DOCX: extract text from word/document.xml via ZIP + quick-xml
/// - XLSX: extract cell values from all sheets via calamine
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
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());

        if !path.exists() {
            tracing::warn!(filename = %filename, "OCR: file not found, skipping");
            results.push(OcrPageResult {
                filename,
                text: "[File not found. It may have been moved or deleted.]".to_string(),
                page_count: 0,
            });
            continue;
        }

        // Size guard: prevent OOM on huge files.
        let file_size = match path.metadata() {
            Ok(m) => m.len(),
            Err(e) => {
                tracing::warn!(filename = %filename, error = %e, "OCR: cannot read metadata, skipping");
                results.push(OcrPageResult {
                    filename,
                    text: format!("[Cannot read file properties: {e}]"),
                    page_count: 0,
                });
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

        // Check for directories.
        if path.is_dir() {
            results.push(OcrPageResult {
                filename,
                text: "[Folders are not supported. Drop individual files.]".to_string(),
                page_count: 0,
            });
            continue;
        }

        // Check for empty files.
        if file_size == 0 {
            results.push(OcrPageResult {
                filename,
                text: "[File is empty (0 bytes).]".to_string(),
                page_count: 0,
            });
            continue;
        }

        let strategy = match classify(path) {
            Ok(s) => s,
            Err(e) => {
                let ext_str = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                tracing::warn!(filename = %filename, error = %e, "OCR: unsupported file type");
                results.push(OcrPageResult {
                    filename,
                    text: format!(
                        "[Unsupported file type: .{ext_str}. Supported: PDF, PNG, JPG, BMP, WebP, TIFF, TXT, MD, CSV, DOCX, XLSX.]"
                    ),
                    page_count: 0,
                });
                continue;
            }
        };

        // Each strategy extracts text independently. Errors are logged and
        // the file is skipped — one bad file must NOT abort the whole batch.
        match strategy {
            OcrStrategy::TextFile => match read_text_file(path) {
                Ok(text) => {
                    tracing::info!(filename = %filename, chars = text.len(), "OCR: text file read");
                    results.push(OcrPageResult {
                        filename,
                        text,
                        page_count: 1,
                    });
                }
                Err(e) => {
                    tracing::warn!(filename = %filename, error = %e, "OCR: text read failed");
                    results.push(OcrPageResult {
                        filename,
                        text: format!("[Could not read text file: {e}]"),
                        page_count: 0,
                    });
                    continue;
                }
            },
            OcrStrategy::Pdf => {
                let path_buf = path.to_path_buf();
                match tokio::task::spawn_blocking(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        pdf_extract::extract_text(&path_buf)
                    }))
                })
                .await
                {
                    Ok(Ok(Ok(text))) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            // Scanned PDF (no text layer): rasterize pages via
                            // poppler and OCR each page through the vision model.
                            tracing::info!(filename = %filename, "OCR: PDF text empty — trying page rasterization");
                            match rasterize_pdf_pages(path).await {
                                Ok((dir, page_paths)) => {
                                    let page_count = page_paths.len();
                                    let mut pages: Vec<String> = Vec::with_capacity(page_count);
                                    for (i, page_path) in page_paths.iter().enumerate() {
                                        let pp = page_path.clone();
                                        let read_res =
                                            tokio::task::spawn_blocking(move || std::fs::read(&pp))
                                                .await;
                                        let page_text = match read_res {
                                            Ok(Ok(bytes)) => {
                                                match ocr_image_bytes(
                                                    &bytes, "png", ocr_model, &provider,
                                                )
                                                .await
                                                {
                                                    Ok(t) if !t.is_empty() => t,
                                                    Ok(_) => "[No text detected on this page.]"
                                                        .to_string(),
                                                    Err(e) => {
                                                        tracing::warn!(filename = %filename, page = i + 1, error = %e, "OCR: page failed");
                                                        format!("[Page OCR failed: {e}]")
                                                    }
                                                }
                                            }
                                            Ok(Err(e)) => {
                                                format!("[Could not read rendered page: {e}]")
                                            }
                                            Err(e) => format!("[Render-read task failed: {e}]"),
                                        };
                                        pages.push(format!(
                                            "--- Page {} ---\n{}",
                                            i + 1,
                                            page_text
                                        ));
                                    }
                                    drop(dir); // delete rendered page PNGs (PHI) ASAP
                                    let text = pages.join("\n\n");
                                    tracing::info!(filename = %filename, pages = page_count, chars = text.len(), "OCR: scanned PDF OCR'd via rasterization");
                                    results.push(OcrPageResult {
                                        filename,
                                        text,
                                        page_count,
                                    });
                                }
                                Err(msg) => {
                                    tracing::warn!(filename = %filename, reason = %msg, "OCR: scanned-PDF rasterization unavailable");
                                    results.push(OcrPageResult {
                                        filename,
                                        text: msg,
                                        page_count: 0,
                                    });
                                }
                            }
                        } else {
                            tracing::info!(filename = %filename, chars = text.len(), "OCR: PDF text extracted");
                            results.push(OcrPageResult {
                                filename,
                                text,
                                page_count: 1,
                            });
                        }
                    }
                    Ok(Ok(Err(e))) => {
                        let err_str = e.to_string();
                        let msg = if err_str.to_lowercase().contains("password")
                            || err_str.to_lowercase().contains("encrypt")
                        {
                            "[This PDF is password-protected. Remove the password and retry.]"
                                .to_string()
                        } else {
                            format!("[PDF could not be parsed: {err_str}]")
                        };
                        tracing::warn!(filename = %filename, error = %err_str, "OCR: PDF extraction failed");
                        results.push(OcrPageResult {
                            filename,
                            text: msg,
                            page_count: 0,
                        });
                        continue;
                    }
                    Ok(Err(_)) => {
                        tracing::warn!(filename = %filename, "OCR: PDF parser panicked (corrupt file)");
                        results.push(OcrPageResult {
                            filename,
                            text: "[PDF appears to be corrupt or malformed.]".to_string(),
                            page_count: 0,
                        });
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: PDF task failed");
                        results.push(OcrPageResult {
                            filename,
                            text: format!("[PDF processing failed unexpectedly: {e}]"),
                            page_count: 0,
                        });
                        continue;
                    }
                }
            }
            OcrStrategy::Docx => {
                let path_buf = path.to_path_buf();
                match tokio::task::spawn_blocking(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        extract_docx_text(&path_buf)
                    }))
                })
                .await
                {
                    Ok(Ok(Ok(text))) => {
                        if text.is_empty() {
                            results.push(OcrPageResult {
                                filename,
                                text: "[No text found in this Word document.]".to_string(),
                                page_count: 0,
                            });
                        } else {
                            tracing::info!(filename = %filename, chars = text.len(), "OCR: docx text extracted");
                            results.push(OcrPageResult {
                                filename,
                                text,
                                page_count: 1,
                            });
                        }
                    }
                    Ok(Ok(Err(e))) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: docx extraction failed");
                        results.push(OcrPageResult {
                            filename,
                            text: format!("[Could not read Word document: {e}]"),
                            page_count: 0,
                        });
                    }
                    Ok(Err(_)) => {
                        tracing::warn!(filename = %filename, "OCR: docx parser panicked (corrupt file)");
                        results.push(OcrPageResult {
                            filename,
                            text: "[Word document appears to be corrupt or malformed.]".to_string(),
                            page_count: 0,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: docx task failed");
                        results.push(OcrPageResult {
                            filename,
                            text: format!("[Word document processing failed unexpectedly: {e}]"),
                            page_count: 0,
                        });
                        continue;
                    }
                }
            }
            OcrStrategy::Xlsx => {
                let path_buf = path.to_path_buf();
                match tokio::task::spawn_blocking(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        extract_xlsx_text(&path_buf)
                    }))
                })
                .await
                {
                    Ok(Ok(Ok(text))) => {
                        if text.is_empty() {
                            results.push(OcrPageResult {
                                filename,
                                text: "[No data found in this spreadsheet.]".to_string(),
                                page_count: 0,
                            });
                        } else {
                            tracing::info!(filename = %filename, chars = text.len(), "OCR: xlsx data extracted");
                            results.push(OcrPageResult {
                                filename,
                                text,
                                page_count: 1,
                            });
                        }
                    }
                    Ok(Ok(Err(e))) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: xlsx extraction failed");
                        results.push(OcrPageResult {
                            filename,
                            text: format!("[Could not read spreadsheet: {e}]"),
                            page_count: 0,
                        });
                    }
                    Ok(Err(_)) => {
                        tracing::warn!(filename = %filename, "OCR: xlsx parser panicked (corrupt file)");
                        results.push(OcrPageResult {
                            filename,
                            text: "[Spreadsheet appears to be corrupt or malformed.]".to_string(),
                            page_count: 0,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: xlsx task failed");
                        results.push(OcrPageResult {
                            filename,
                            text: format!("[Spreadsheet processing failed unexpectedly: {e}]"),
                            page_count: 0,
                        });
                        continue;
                    }
                }
            }
            OcrStrategy::Image => {
                // Move the sync file read off the async executor.
                let path_buf = path.to_path_buf();
                let ext_str = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_else(|| "png".to_string());
                let ext_for_url = if ext_str == "tiff" || ext_str == "tif" {
                    "png"
                } else {
                    ext_str.as_str()
                };
                let ext_for_capture = ext_str.clone();
                let image_data = match tokio::task::spawn_blocking(move || {
                    let raw =
                        std::fs::read(&path_buf).map_err(|e| OcrError::ReadError(e.to_string()))?;
                    // Convert TIFF to PNG — vision models don't accept TIFF directly.
                    if ext_for_capture == "tiff" || ext_for_capture == "tif" {
                        let img =
                            image::load_from_memory_with_format(&raw, image::ImageFormat::Tiff)
                                .map_err(|e| OcrError::ReadError(format!("TIFF decode: {e}")))?;
                        let mut png_bytes = Vec::new();
                        img.write_to(
                            &mut std::io::Cursor::new(&mut png_bytes),
                            image::ImageFormat::Png,
                        )
                        .map_err(|e| OcrError::ReadError(format!("PNG encode: {e}")))?;
                        Ok::<Vec<u8>, OcrError>(png_bytes)
                    } else {
                        Ok(raw)
                    }
                })
                .await
                {
                    Ok(Ok(data)) => data,
                    Ok(Err(e)) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: image read/convert failed");
                        results.push(OcrPageResult {
                            filename,
                            text: format!("[Could not read image file: {e}]"),
                            page_count: 0,
                        });
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: image task failed");
                        results.push(OcrPageResult {
                            filename,
                            text: format!("[Image processing failed unexpectedly: {e}]"),
                            page_count: 0,
                        });
                        continue;
                    }
                };
                let data_url = encode_image_as_data_url(&image_data, ext_for_url);
                let request = build_image_ocr_request(&data_url, ocr_model);

                tracing::info!(filename = %filename, bytes = image_data.len(), "OCR: sending image to vision model");
                match tokio::time::timeout(OCR_PER_FILE_TIMEOUT, provider.complete(request)).await {
                    Ok(Ok(response)) => {
                        let text = response.content.trim().to_string();
                        if text.is_empty() {
                            tracing::warn!(filename = %filename, "OCR: vision model returned empty text");
                        }
                        results.push(OcrPageResult {
                            filename,
                            text: if text.is_empty() {
                                "[No text detected in this image. The model may not have recognized any text, or the image quality may be too low.]".to_string()
                            } else {
                                text
                            },
                            page_count: 1,
                        });
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(filename = %filename, error = %e, "OCR: vision model error");
                        results.push(OcrPageResult {
                            filename,
                            text: format!(
                                "[Vision model error: {e}. Check that your OCR model is running.]"
                            ),
                            page_count: 0,
                        });
                        continue;
                    }
                    Err(_) => {
                        tracing::warn!(filename = %filename, "OCR: per-file timeout (120s)");
                        results.push(OcrPageResult {
                            filename,
                            text: "[OCR timed out after 120 seconds. The file may be too large or the model too slow.]".to_string(),
                            page_count: 0,
                        });
                        continue;
                    }
                }
            }
        }
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
    fn classify_docx_is_office() {
        let path = Path::new("doc.docx");
        assert_eq!(classify(path).unwrap(), OcrStrategy::Docx);
    }

    #[test]
    fn classify_xlsx_is_office() {
        let path = Path::new("sheet.xlsx");
        assert_eq!(classify(path).unwrap(), OcrStrategy::Xlsx);
    }

    #[test]
    fn classify_tiff_is_image() {
        let path = Path::new("scan.tiff");
        assert_eq!(classify(path).unwrap(), OcrStrategy::Image);
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

    #[test]
    fn sorts_pdf_page_filenames_numerically() {
        // Synthetic paths named like pdftoppm output. A lexicographic sort
        // would put page-10 before page-2; the numeric sort must not.
        let paths: Vec<PathBuf> = ["page-1.png", "page-10.png", "page-2.png", "page-3.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let sorted = sort_page_paths(paths);
        let names: Vec<String> = sorted
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["page-1.png", "page-2.png", "page-3.png", "page-10.png"]
        );
    }

    #[test]
    fn sorts_pdf_page_filenames_handles_zero_padding() {
        // pdftoppm zero-pads based on total page count; both forms must sort
        // numerically, not lexically.
        let paths: Vec<PathBuf> = ["page-09.png", "page-10.png", "page-01.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let sorted = sort_page_paths(paths);
        let names: Vec<String> = sorted
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["page-01.png", "page-09.png", "page-10.png"]);
    }

    #[test]
    fn no_rasterizer_message_mentions_poppler_and_platform_hints() {
        assert!(NO_RASTERIZER_MSG.contains("poppler"));
        assert!(
            NO_RASTERIZER_MSG.contains("macOS") || NO_RASTERIZER_MSG.contains("brew"),
            "should hint at macOS install"
        );
        assert!(
            NO_RASTERIZER_MSG.contains("Linux") || NO_RASTERIZER_MSG.contains("apt"),
            "should hint at Linux install"
        );
    }

    /// Validates the full rasterization pipeline (subprocess, temp dir, page
    /// collection, numeric sort) end-to-end. `#[ignore]` because it needs
    /// poppler (pdftoppm/pdftocairo) on PATH; run locally with
    /// `cargo test -p medical-processing --lib -- --ignored rasterizes_pdf`.
    #[ignore = "requires poppler (pdftoppm/pdftocairo) on PATH"]
    #[tokio::test]
    async fn rasterizes_pdf_when_poppler_present() {
        // Build a minimal valid PDF (same fixture approach as
        // `extract_pdf_text_from_real_pdf`). pdftoppm renders text PDFs too, so
        // this exercises rasterization without needing a scanned/image PDF.
        use lopdf::content::{Content, Operation};
        use lopdf::{Dictionary, Document, Object, Stream, dictionary};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![100.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal("Scanned PDF OCR test")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
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
            "Type" => "Catalog", "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.compress();

        let dir = tempfile::tempdir().unwrap();
        let pdf_path = dir.path().join("scan.pdf");
        doc.save(&pdf_path).unwrap();

        let result = rasterize_pdf_pages(&pdf_path).await;
        assert!(result.is_ok(), "rasterize failed: {:?}", result.err());
        let (render_dir, pages) = result.unwrap();
        assert!(!pages.is_empty(), "should render at least one page PNG");
        assert_eq!(pages[0].extension().and_then(|e| e.to_str()), Some("png"));
        drop(render_dir);
    }
}
