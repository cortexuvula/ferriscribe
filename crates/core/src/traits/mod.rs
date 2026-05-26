//! Provider trait interfaces implemented by workspace crates.
//!
//! Each trait defines the contract for a category of pluggable backend.
//! Provider crates (`ai-providers`, `stt-providers`, `tts-providers`,
//! etc.) implement these traits; consumer crates depend only on the
//! trait, enabling runtime provider selection.
//!
//! All provider traits are `Send + Sync` and use `async_trait` for
//! async method support.

pub mod ai_provider;
pub mod stt_provider;
pub mod tts_provider;
pub mod agent;
pub mod translation;
pub mod exporter;

pub use ai_provider::AiProvider;
pub use stt_provider::SttProvider;
pub use tts_provider::TtsProvider;
pub use agent::{Agent, Tool};
pub use translation::TranslationProvider;
pub use exporter::Exporter;
