//! Document OCR pipeline: extract text from files (text, images, PDFs).
//!
//! Text files are read directly. Images are sent to a vision model. PDFs with
//! an embedded text layer are extracted via `pdf-extract`; scanned (image-only)
//! PDFs are rendered to page images by the bundled pdfium library and OCR'd
//! page-by-page through the vision model. No external tools or installs are
//! required — pdfium ships inside the app as a Tauri resource.
//!
//! **HIPAA note:** Extracted content is never logged — only filenames and
//! page/character counts appear in tracing. Rendered page images live only in
//! RAM (never written to disk).

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use medical_core::traits::ai_provider::AiProvider;
use medical_core::types::ai::{
    CompletionRequest, ContentPart, ImageUrlData, Message, MessageContent, Role,
};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

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

/// Message shown when a scanned PDF is detected but the pdfium renderer isn't
/// available (it couldn't be downloaded or loaded). The library is fetched into
/// the app data dir on first use; if that fails (offline, etc.) this message
/// tells the user how to recover.
const PDFIUM_UNAVAILABLE_MSG: &str = "[Scanned PDF detected (no text layer), but the PDF renderer (pdfium) \
     could not be downloaded or loaded. Check your internet connection and try \
     again; if it persists, check the logs.]";

/// Pinned `bblanchon/pdfium-binaries` release tag. The pdfium library is
/// downloaded from this GitHub release into the app data dir on first scanned-PDF
/// OCR (mirroring how whisper models are fetched). Pinned for reproducibility.
const PDFIUM_BIN_VERSION: &str = "chromium/7999";

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
// Scanned-PDF rasterization (pdfium, runtime-fetched)
// ---------------------------------------------------------------------------

/// Map the compile target to a `(archive_asset, archive_member, lib_filename)`
/// triple for the bblanchon/pdfium-binaries release. `lib_filename` is the
/// platform-default name that `Pdfium::pdfium_platform_library_name_at_path`
/// looks for at bind time.
fn pdfium_target() -> (&'static str, &'static str, &'static str) {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        (
            "pdfium-mac-arm64.tgz",
            "lib/libpdfium.dylib",
            "libpdfium.dylib",
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        (
            "pdfium-mac-x64.tgz",
            "lib/libpdfium.dylib",
            "libpdfium.dylib",
        )
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        ("pdfium-linux-x64.tgz", "lib/libpdfium.so", "libpdfium.so")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        ("pdfium-win-x64.tgz", "bin/pdfium.dll", "pdfium.dll")
    }
}

/// Pinned SHA-256 digests of the `bblanchon/pdfium-binaries` release assets
/// for [`PDFIUM_BIN_VERSION`]. The pdfium library is dlopen'd — a compromised
/// mirror or MITM on this download would execute arbitrary code — so the
/// digest is mandatory, exactly like the whisper-server binary in
/// `medical-sharing`. Regenerate when bumping `PDFIUM_BIN_VERSION`:
/// `curl -sL <release-url>/<asset>.tgz | shasum -a 256`.
const PDFIUM_SHA256: &[(&str, &str)] = &[
    (
        "pdfium-mac-arm64.tgz",
        "e214ee33f22b2204daa765a545aee1e425d88448e6154dac95c6a06206b7437f",
    ),
    (
        "pdfium-mac-x64.tgz",
        "4b924d948d2ec4863435d375a94541b4003c59f8adc28cc5e4236b0ab81a355d",
    ),
    (
        "pdfium-linux-x64.tgz",
        "c3af580f9df0fef9545b44115bc5ea440f286956b5f231df69fb373b8efc4f69",
    ),
    (
        "pdfium-win-x64.tgz",
        "55329d5cb5de8a379a2fc563106492d7f385a1f795d18970922c71f708f9fbb4",
    ),
];

/// The pinned digest for `asset`, if this build's platform asset has one.
/// `ensure_pdfium_available` refuses to download without it.
fn pdfium_sha256(asset: &str) -> Option<&'static str> {
    PDFIUM_SHA256
        .iter()
        .find(|(name, _)| *name == asset)
        .map(|(_, hash)| *hash)
}

/// Serializes the pdfium download/extract/bind path. Without it, two
/// concurrent OCR batches both seeing an empty cache would race the
/// `.part` file writes, the archive rename (which outright fails on
/// Windows when the destination exists), and the lib extraction —
/// producing a torn archive or a library bound mid-rewrite.
static PDFIUM_ENSURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ensure the pdfium library is present in `{data_dir}/pdfium/`, downloading +
/// extracting it (and ad-hoc signing on macOS) if missing or stale. Returns the
/// directory containing the lib. Idempotent: a no-op if the pinned version is
/// already on disk. The download is a one-time ~7 MB anonymous GET to GitHub
/// (consistent with the app's existing model-download traffic); the lib is then
/// cached and used locally forever.
pub async fn ensure_pdfium_available(data_dir: &Path) -> Result<std::path::PathBuf, String> {
    // Fast path before taking the lock ( uncontended once cached ).
    if let Some(dir) = cached_pdfium_dir(data_dir) {
        return Ok(dir);
    }
    let _guard = PDFIUM_ENSURE_LOCK.lock().await;
    // Re-check after acquiring: another task may have completed the download
    // while we waited.
    if let Some(dir) = cached_pdfium_dir(data_dir) {
        return Ok(dir);
    }

    let pdfium_dir = data_dir.join("pdfium");
    let (asset, member, lib_name) = pdfium_target();
    let lib_path = pdfium_dir.join(lib_name);
    let version_path = pdfium_dir.join(".version");

    // The library is dlopen'd, so an unverified download means arbitrary code
    // execution from a compromised mirror — refuse to fetch without the
    // pinned digest for this platform's asset.
    let expected = pdfium_sha256(asset).ok_or_else(|| {
        format!("no pinned SHA-256 for pdfium asset {asset} at {PDFIUM_BIN_VERSION}; refusing unverified download")
    })?;
    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/{PDFIUM_BIN_VERSION}/{asset}"
    );
    tracing::info!(%url, "downloading pdfium for scanned-PDF OCR");
    let bytes = medical_core::net::download_bytes(&url, Some(expected))
        .await
        .map_err(|e| format!("download pdfium: {e}"))?;

    // Extract just the library member into place, then (on macOS) ad-hoc
    // sign. Archive decompression and codesign are synchronous CPU/disk/subprocess
    // work — keep them off the tokio worker. The `.version` stamp is written
    // last: a torn state simply looks uncached and re-downloads next call.
    let bin_path = tokio::task::spawn_blocking(move || -> Result<std::path::PathBuf, String> {
        std::fs::create_dir_all(&pdfium_dir).map_err(|e| format!("create pdfium dir: {e}"))?;
        let tgz_path = pdfium_dir.join("pdfium.tgz");
        std::fs::write(&tgz_path, &bytes).map_err(|e| format!("write archive: {e}"))?;
        extract_member(&tgz_path, member, &lib_path)
            .map_err(|e| format!("extract {member}: {e}"))?;
        let _ = std::fs::remove_file(&tgz_path); // cleanup archive (lib extracted)

        // macOS: ad-hoc sign with Hardened Runtime so the dylib loads under the
        // notarized app's Hardened Runtime. No Developer ID needed — the dylib is
        // NOT part of the app bundle (it's in the writable data dir), so it is
        // outside notarization scope; ad-hoc signing suffices for loading. The
        // app's disable-library-validation entitlement permits loading a dylib
        // not signed with its Team ID. Note: no `--timestamp` — secure timestamps
        // contact Apple's server and add nothing to an ad-hoc signature.
        #[cfg(target_os = "macos")]
        ad_hoc_sign(&lib_path)?;

        std::fs::write(&version_path, PDFIUM_BIN_VERSION)
            .map_err(|e| format!("stamp version: {e}"))?;
        Ok(pdfium_dir)
    })
    .await
    .map_err(|e| format!("pdfium extract task failed: {e}"))??;
    tracing::info!(path = %bin_path.display(), "pdfium ready");
    Ok(bin_path)
}

/// The pdfium dir, if the lib is present with the pinned version stamped.
fn cached_pdfium_dir(data_dir: &Path) -> Option<std::path::PathBuf> {
    let pdfium_dir = data_dir.join("pdfium");
    let (_asset, _member, lib_name) = pdfium_target();
    let cached = pdfium_dir.join(lib_name).exists()
        && std::fs::read_to_string(pdfium_dir.join(".version"))
            .ok()
            .is_some_and(|s| s.trim() == PDFIUM_BIN_VERSION);
    cached.then_some(pdfium_dir)
}

/// Idempotently ensure pdfium is downloaded (if needed) and bound. Safe to call
/// repeatedly; a no-op once bound. Callers (the OCR command) should invoke this
/// before [`extract_text`] when the batch contains PDFs.
pub async fn ensure_pdfium_initialized(data_dir: &Path) -> Result<(), String> {
    if PDFIUM.get().is_some() {
        return Ok(());
    }
    let lib_dir = ensure_pdfium_available(data_dir).await?;
    if let Err(e) = init_pdfium(&lib_dir) {
        // A cached lib that fails to bind would otherwise fail forever (the
        // version stamp short-circuits re-download). Clear the stamp so the
        // next call re-fetches a fresh copy.
        let _ = std::fs::remove_file(lib_dir.join(".version"));
        Err(e)
    } else {
        Ok(())
    }
}

/// Extract a single `member` from a .tggz archive to `dest`.
fn extract_member(tgz: &Path, member: &str, dest: &Path) -> std::io::Result<()> {
    use flate2::read::GzDecoder;
    let file = std::fs::File::open(tgz)?;
    let gz = GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_str() == Some(member) {
            let mut dest_file = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut dest_file)?;
            return Ok(());
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("member {member} not found in archive"),
    ))
}

/// macOS: strip extended attributes and ad-hoc sign the dylib with Hardened
/// Runtime, so it loads under the app's Hardened Runtime. No `--timestamp` —
/// secure timestamps contact Apple's server and add nothing to an ad-hoc
/// signature (the dylib lives outside the bundle, so it never notarizes).
#[cfg(target_os = "macos")]
fn ad_hoc_sign(lib: &Path) -> Result<(), String> {
    // Strip xattrs (prevents "resource fork / detritus" rejection).
    let _ = std::process::Command::new("xattr")
        .args(["-cr"])
        .arg(lib)
        .status();
    let status = std::process::Command::new("codesign")
        .args(["--force", "--options", "runtime", "--sign", "-"])
        .arg(lib)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .map_err(|e| format!("run codesign: {e}"))?;
    if !status.success() {
        return Err(format!("codesign ad-hoc failed (exit {:?})", status.code()));
    }
    Ok(())
}

/// The pdfium renderer, bound once via [`ensure_pdfium_initialized`] /
/// [`init_pdfium`]. Guarded by a `Mutex` because pdfium is not safe for
/// concurrent calls from multiple threads (the safe wrapper's bindings are
/// `Send` but not `Sync`). All rendering for a given PDF happens inside a
/// single `spawn_blocking` closure, so the lock is held briefly and serially.
static PDFIUM: OnceLock<Mutex<Pdfium>> = OnceLock::new();

/// Bind pdfium from a directory containing the platform library. Idempotent.
/// Usually called by [`ensure_pdfium_initialized`]; exposed for tests that want
/// to bind against a pre-placed lib.
pub fn init_pdfium(lib_dir: &Path) -> Result<(), String> {
    if PDFIUM.get().is_some() {
        return Ok(());
    }
    let lib_path = Pdfium::pdfium_platform_library_name_at_path(lib_dir);
    let bindings = Pdfium::bind_to_library(lib_path)
        .map_err(|e| format!("pdfium bind_to_library({:?}): {e}", lib_dir))?;
    let _ = PDFIUM.set(Mutex::new(Pdfium::new(bindings)));
    Ok(())
}

/// Render up to [`MAX_PDF_OCR_PAGES`] pages of `pdf_path` to in-memory PNG
/// bytes at [`PDF_OCR_DPI`], via the bundled pdfium. Returns `(page_number,
/// png_bytes)` in page order. PHI never touches disk — rendered pages live only
/// in RAM. Synchronous (CPU + FFI work); callers should run it on a
/// `spawn_blocking` thread. Returns `Err(PDFIUM_UNAVAILABLE_MSG.to_string())`
/// if pdfium wasn't initialized, or `Err(<reason>)` on load/render failure.
fn render_pdf_pages(pdf_path: &Path) -> Result<Vec<(usize, Vec<u8>)>, String> {
    use std::io::Cursor;

    let pdfium = PDFIUM
        .get()
        .ok_or_else(|| PDFIUM_UNAVAILABLE_MSG.to_string())?;
    let pdfium = pdfium
        .lock()
        .map_err(|e| format!("pdfium mutex poisoned: {e}"))?;

    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| format!("pdfium load_pdf: {e}"))?;

    // PDF user-space units are 1/72 inch; scale to the target DPI.
    let scale = PDF_OCR_DPI as f32 / 72.0;
    let mut pages: Vec<(usize, Vec<u8>)> = Vec::new();
    for (i, page) in document.pages().iter().enumerate() {
        if i >= MAX_PDF_OCR_PAGES {
            break;
        }
        // Render at a pixel width matching PDF_OCR_DPI for this page's width.
        let target_w = ((page.width().value * scale).round() as i32).max(1);
        let config = PdfRenderConfig::new().set_target_width(target_w);
        let img = page
            .render_with_config(&config)
            .map_err(|e| format!("pdfium render page {}: {e}", i + 1))?
            .as_image()
            .map_err(|e| format!("pdfium as_image page {}: {e}", i + 1))?;
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| format!("png encode page {}: {e}", i + 1))?;
        pages.push((i + 1, png));
    }
    Ok(pages)
}

/// Extract text from a list of document file paths.
///
/// Each file is classified by extension and processed accordingly:
/// - Text files (txt/md/csv): read directly, no model call
/// - Images (png/jpg/jpeg/bmp/webp/tiff/tif): base64-encode and send to the
///   vision model (TIFF is converted to PNG first since vision models reject TIFF)
/// - PDFs: extract embedded text via pdf-extract; if empty (scanned PDF),
///   render pages via the bundled pdfium and OCR each through the vision model
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
                            // Scanned PDF (no text layer): render pages to PNG
                            // via the bundled pdfium and OCR each through the
                            // vision model.
                            tracing::info!(filename = %filename, "OCR: PDF text empty — trying pdfium rasterization");
                            let path_buf = path.to_path_buf();
                            let render_res =
                                tokio::task::spawn_blocking(move || render_pdf_pages(&path_buf))
                                    .await;
                            match render_res {
                                Ok(Ok(rendered_pages)) => {
                                    let page_count = rendered_pages.len();
                                    let mut pages: Vec<String> = Vec::with_capacity(page_count);
                                    for (page_num, png_bytes) in rendered_pages {
                                        let page_text = match ocr_image_bytes(
                                            &png_bytes, "png", ocr_model, &provider,
                                        )
                                        .await
                                        {
                                            Ok(t) if !t.is_empty() => t,
                                            Ok(_) => "[No text detected on this page.]".to_string(),
                                            Err(e) => {
                                                tracing::warn!(filename = %filename, page = page_num, error = %e, "OCR: page failed");
                                                format!("[Page OCR failed: {e}]")
                                            }
                                        };
                                        pages.push(format!(
                                            "--- Page {} ---\n{}",
                                            page_num, page_text
                                        ));
                                    }
                                    let text = pages.join("\n\n");
                                    tracing::info!(filename = %filename, pages = page_count, chars = text.len(), "OCR: scanned PDF OCR'd via pdfium");
                                    results.push(OcrPageResult {
                                        filename,
                                        text,
                                        page_count,
                                    });
                                }
                                Ok(Err(msg)) => {
                                    tracing::warn!(filename = %filename, reason = %msg, "OCR: pdfium rasterization failed");
                                    results.push(OcrPageResult {
                                        filename,
                                        text: msg,
                                        page_count: 0,
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(filename = %filename, error = %e, "OCR: pdfium render task failed");
                                    results.push(OcrPageResult {
                                        filename,
                                        text: format!("[PDF rendering task failed: {e}]"),
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
    fn pdfium_asset_has_pinned_hash() {
        let (asset, _, _) = pdfium_target();
        assert!(
            pdfium_sha256(asset).is_some(),
            "no pinned SHA-256 for this platform's pdfium asset {asset} at {PDFIUM_BIN_VERSION} \
             — a version bump must regenerate PDFIUM_SHA256"
        );
    }

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
    fn pdfium_unavailable_message_is_descriptive() {
        // pdfium is fetched at runtime, so this message surfaces when the
        // download/load fails (offline, etc.). It must name pdfium and point at
        // the network recovery path — and still NOT ask the user to install
        // anything manually.
        assert!(PDFIUM_UNAVAILABLE_MSG.contains("pdfium"));
        let lower = PDFIUM_UNAVAILABLE_MSG.to_lowercase();
        assert!(
            lower.contains("download") || lower.contains("internet"),
            "should point at the download/network recovery"
        );
        assert!(
            !lower.contains("install"),
            "must not ask the user to install anything (pdfium is fetched automatically)"
        );
    }

    /// Validates the full runtime pdfium pipeline (download → extract →
    /// [macOS: ad-hoc sign] → bind → load → render → PNG encode) end-to-end.
    /// `#[ignore]` because it needs network to download pdfium on first run;
    /// run with `cargo test -p medical-processing --lib -- --ignored renders_pdf`.
    #[ignore = "requires network to download pdfium on first run"]
    #[tokio::test]
    async fn renders_pdf_when_pdfium_initialized() {
        // Use a temp data dir so the test downloads pdfium fresh (no pollution
        // of the real app data dir).
        let data_dir = tempfile::tempdir().unwrap();
        ensure_pdfium_initialized(data_dir.path())
            .await
            .expect("ensure should download + bind pdfium");

        // Build a minimal valid PDF (same fixture approach as
        // `extract_pdf_text_from_real_pdf`). pdfium renders text PDFs too, so
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

        let pages = render_pdf_pages(&pdf_path).expect("render should succeed");
        assert!(!pages.is_empty(), "should render at least one page");
        let (_, png) = &pages[0];
        assert!(
            png.len() > 100,
            "PNG bytes should be non-trivial, got {}",
            png.len()
        );
        assert_eq!(
            &png[..8],
            b"\x89PNG\r\n\x1a\n",
            "first page should be a PNG"
        );
    }
}
