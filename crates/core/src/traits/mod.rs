//! Provider trait interfaces implemented by workspace crates.
//!
//! Each trait defines the contract for a category of pluggable backend.
//! Provider crates (`ai-providers`, `stt-providers`, `tts-providers`,
//! etc.) implement these traits; consumer crates depend only on the
//! trait, enabling runtime provider selection.
//!
//! All provider traits are `Send + Sync` and use `async_trait` for
//! async method support.

pub mod agent;
pub mod ai_provider;
pub mod exporter;
pub mod stt_provider;
pub mod translation;
pub mod tts_provider;

pub use agent::{Agent, Tool};
pub use ai_provider::AiProvider;
pub use exporter::Exporter;
pub use stt_provider::SttProvider;
pub use translation::TranslationProvider;
pub use tts_provider::TtsProvider;
