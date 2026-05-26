//! Document export trait and format types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;

/// The output format for a document export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// PDF document.
    Pdf,
    /// Microsoft Word document.
    Docx,
    /// FHIR Bundle (interoperability standard).
    FhirBundle,
}

/// Options controlling how a document is exported.
///
/// Passed to [`Exporter::export`]. Includes format selection, metadata
/// inclusion, optional watermark, and page size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportConfig {
    /// Target output format.
    pub format: ExportFormat,
    /// Whether to include document metadata (dates, provider info).
    pub include_metadata: bool,
    /// Optional watermark text overlaid on each page.
    pub watermark: Option<String>,
    /// Page size (e.g. `"A4"`, `"Letter"`).
    pub page_size: String,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            format: ExportFormat::Pdf,
            include_metadata: true,
            watermark: None,
            page_size: "A4".into(),
        }
    }
}

/// Abstraction over document exporters.
///
/// Implemented by the `export` crate. Each exporter produces documents
/// in a single [`ExportFormat`].
#[async_trait]
pub trait Exporter: Send + Sync {
    /// The format this exporter produces.
    fn format(&self) -> ExportFormat;

    /// Export the given content using the provided configuration,
    /// returning the raw bytes of the exported document.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Export`](crate::error::AppError::Export) on
    /// export failure.
    async fn export(&self, content: &str, config: ExportConfig) -> AppResult<Vec<u8>>;
}
