//! Shared types, traits, and error handling for the FerriScribe workspace.
//!
//! This crate is the foundation leaf — every workspace crate depends on it,
//! but it depends on none of them. It provides:
//!
//! - **[`AppError`] / [`AppResult`]** — the single error type propagated across
//!   all crate boundaries, with variants for every subsystem (database, AI
//!   provider, STT, audio, export, etc.) and a custom [`serde::Serialize`] impl
//!   that produces machine-readable JSON for the Tauri frontend.
//! - **Domain types** — [`Recording`](types::Recording), [`AppConfig`](types::AppConfig),
//!   [`CompletionRequest`](types::CompletionRequest), [`Transcript`](types::Transcript),
//!   and the full set of structs/enums shared across crate boundaries.
//! - **Provider traits** — [`AiProvider`](traits::AiProvider),
//!   [`SttProvider`](traits::SttProvider), [`TtsProvider`](traits::TtsProvider),
//!   [`Agent`](traits::Agent), [`Tool`](traits::Tool), [`Exporter`](traits::Exporter),
//!   and [`TranslationProvider`](traits::TranslationProvider) — the interfaces that
//!   provider crates implement.
//! - **Endpoint policy** — static (no-DNS) classification of host strings to
//!   enforce the local-only AI/STT constraint at settings-save time.
//! - **Preflight probes** — short-timeout connectivity checks run before
//!   expensive commands to surface offline endpoints early.
//!
//! # Crate convention
//!
//! The package name in `Cargo.toml` is `medical-core`; other crates import it
//! as `use medical_core::…`.

pub mod endpoint_policy;
pub mod error;
pub mod http_error_body;
pub mod preflight;
pub mod traits;
pub mod types;

pub use error::{AppError, AppResult, ErrorContext, ErrorSeverity};
