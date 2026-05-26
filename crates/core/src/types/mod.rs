//! Shared domain types used across all workspace crates.
//!
//! This module re-exports every public type so consumers can write
//! `use medical_core::types::Recording` rather than reaching into
//! individual submodules. Submodules group types by subsystem:
//!
//! | Submodule | Contents |
//! |---|---|
//! | [`recording`] | [`Recording`](recording::Recording), [`ProcessingStatus`](recording::ProcessingStatus), [`RecordingSummary`](recording::RecordingSummary) |
//! | [`processing`] | Queue tasks, batch processing, priority |
//! | [`agent`] | Agent context, tools, patient context |
//! | [`ai`] | Completion request/response, messages, streaming |
//! | [`stt`] | Audio data, transcription config/results |
//! | [`tts`] | TTS config, voice info |
//! | [`rag`] | RAG results, search config, knowledge graph types |
//! | [`settings`] | [`AppConfig`](settings::AppConfig) and related enums |
//! | [`vocabulary`] | Vocabulary entries and corrections |
//! | [`endpoint`] | [`RemoteEndpoint`](endpoint::RemoteEndpoint) with LAN/Tailscale resolution |
//! | [`letter_audience`] | [`LetterAudience`](letter_audience::LetterAudience) for generated letters |

pub mod recording;
pub mod processing;
pub mod agent;
pub mod ai;
pub mod stt;
pub mod tts;
pub mod rag;
pub mod settings;
pub mod vocabulary;
pub mod endpoint;
pub mod letter_audience;

pub use recording::*;
pub use processing::*;
pub use agent::*;
pub use ai::*;
pub use stt::*;
pub use tts::*;
pub use rag::*;
pub use settings::*;
pub use vocabulary::*;
pub use endpoint::*;
pub use letter_audience::LetterAudience;
