//! System and user prompt builders for peer-to-peer discussion note generation.
//!
//! The system prompt instructs the LLM to generate a structured peer discussion
//! note with sections: Header, Clinical Summary, Discussion Points, Assessment,
//! Recommendations, Action Items.
//!
//! # Module layout
//!
//! - `prompt_template` — the built-in default prompt and [`build_peer_discussion_prompt`].
//! - `user_prompt` — [`build_user_prompt`], plus the `sanitize_prompt` helper.

mod prompt_template;
mod user_prompt;

pub use prompt_template::{build_peer_discussion_prompt, default_peer_discussion_prompt};
pub use user_prompt::build_user_prompt;

/// Inputs to [`build_peer_discussion_prompt`].
#[derive(Debug, Clone)]
pub struct PeerDiscussionPromptConfig {
    /// Name of the physician being discussed with.
    pub physician_name: String,
    /// Specialty of the physician.
    pub specialty: String,
    /// Reason for the discussion.
    pub reason: String,
    /// User-supplied override for the entire system prompt.
    pub custom_prompt: Option<String>,
}
