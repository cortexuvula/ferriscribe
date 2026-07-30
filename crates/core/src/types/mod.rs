//! Shared domain types used across all workspace crates.
//!
//! This module re-exports every public type so consumers can write
//! `use medical_core::types::Recording` rather than reaching into
//! individual submodules. Submodules group types by subsystem:
//!
//! | Submodule | Contents |
//! |---|---|
//! | [`recording`] | [`recording::Recording`], [`recording::ProcessingStatus`], [`recording::RecordingSummary`] |
//! | [`processing`] | Queue tasks, batch processing, priority |
//! | [`agent`] | Agent context, tools, patient context |
//! | [`ai`] | Completion request/response, messages, streaming |
//! | [`condition_chip`] | [`condition_chip::ConditionChip`] with deterministic UUID v5 id |
//! | [`user_dict_entry`] | [`user_dict_entry::UserDictEntry`] with deterministic UUID v5 id |
//! | [`stt`] | Audio data, transcription config/results |
//! | [`tts`] | TTS config, voice info |
//! | [`rag`] | RAG results, search config, knowledge graph types |
//! | [`settings`] | [`settings::AppConfig`] and related enums |
//! | [`vocabulary`] | Vocabulary entries and corrections |
//! | [`endpoint`] | [`endpoint::RemoteEndpoint`] with LAN/Tailscale resolution |
//! | [`letter_audience`] | [`letter_audience::LetterAudience`] for generated letters |

pub mod agent;
pub mod ai;
pub mod condition_chip;
pub mod endpoint;
pub mod letter_audience;
pub mod processing;
pub mod rag;
pub mod recording;
pub mod settings;
pub mod stt;
pub mod tts;
pub mod user_dict_entry;
pub mod vocabulary;

pub use agent::*;
pub use ai::*;
pub use condition_chip::*;
pub use endpoint::*;
pub use letter_audience::LetterAudience;
pub use processing::*;
pub use rag::*;
pub use recording::*;
pub use settings::*;
pub use stt::*;
pub use tts::*;
// Re-export only the struct from `user_dict_entry` (not the `deterministic_id`
// / `normalize_for_id` helpers) to avoid an ambiguous-glob clash with
// `condition_chip::*`, which exports identically-named helpers. Callers that
// need the dict-helpers reach them via the fully-qualified path
// `medical_core::types::user_dict_entry::deterministic_id`.
pub use user_dict_entry::UserDictEntry;
pub use vocabulary::*;
