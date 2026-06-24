//! # medical-export
//!
//! Export clinical documents from FerriScribe [`Recording`]s to PDF, DOCX, and
//! FHIR R4 formats.
//!
//! Each sub-module exposes a stateless `*Exporter` struct with associated
//! functions that take a `&Recording` (from [`medical_core`]) and return
//! `ExportResult<Vec<u8>>` — raw bytes ready to be written to disk or streamed
//! over Tauri IPC.
//!
//! | Module | Format | Backend crate |
//! |--------|--------|---------------|
//! | [`pdf`]  | PDF  | `printpdf` 0.7 |
//! | [`docx`] | DOCX | `docx-rs` 0.4 |
//! | [`fhir`] | FHIR R4 JSON | `serde_json` |
//!
//! See the crate-level [README](../README.md) for an architectural overview,
//! FHIR Bundle layout, and format-specific gotchas.
//!
//! [`Recording`]: medical_core::types::recording::Recording

pub mod docx;
pub mod fhir;
pub mod pdf;

use thiserror::Error;

/// Errors that can occur during document export.
///
/// Each variant wraps a human-readable description produced by the underlying
/// library (`printpdf`, `docx-rs`, `serde_json`) — document *content* is never
/// included, in keeping with the project's no-PHI-in-logs rule.
#[derive(Debug, Error)]
pub enum ExportError {
    /// A PDF generation error from `printpdf`.
    #[error("PDF export error: {0}")]
    Pdf(String),
    /// A DOCX generation error from `docx-rs`.
    #[error("DOCX export error: {0}")]
    Docx(String),
    /// A FHIR serialisation error.
    #[error("FHIR export error: {0}")]
    Fhir(String),
    /// A general I/O error (unused by the crate itself but available to callers).
    #[error("IO error: {0}")]
    Io(String),
}

/// Convenience result type used throughout the crate.
pub type ExportResult<T> = Result<T, ExportError>;
