//! Letter audience types — defines the target audience and prompt template
//! for generated patient letters.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A target audience for generated letters.
///
/// Built-in audiences ship with the app (e.g. "Referral Letter to
/// Specialist", "Patient Discharge Summary"). Custom audiences are
/// created by the user and can have their own system prompts and
/// templates.
///
/// Each audience carries its own `system_prompt` that instructs the AI
/// model how to tailor the letter's tone, structure, and clinical
/// detail level for the target reader.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LetterAudience {
    /// Unique identifier.
    pub id: Uuid,
    /// Human-readable audience name.
    pub name: String,
    /// System prompt for letter generation.
    pub system_prompt: String,
    /// Optional user-defined template text.
    pub user_template: Option<String>,
    /// Whether this is a built-in audience (cannot be deleted).
    pub is_builtin: bool,
    /// When the audience was created.
    pub created_at: DateTime<Utc>,
    /// When the audience was last modified.
    pub updated_at: DateTime<Utc>,
}

impl LetterAudience {
    /// Create a new custom letter audience.
    ///
    /// Generates a new UUIDv4, sets `is_builtin` to `false`, and
    /// timestamps to now.
    pub fn new(
        name: impl Into<String>,
        system_prompt: impl Into<String>,
        user_template: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            system_prompt: system_prompt.into(),
            user_template,
            is_builtin: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a built-in letter audience.
    ///
    /// Uses the provided UUID, sets `is_builtin` to `true`, and
    /// timestamps to now. Built-in audiences cannot be deleted by the
    /// user.
    pub fn builtin(
        id: Uuid,
        name: impl Into<String>,
        system_prompt: impl Into<String>,
        user_template: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            name: name.into(),
            system_prompt: system_prompt.into(),
            user_template,
            is_builtin: true,
            created_at: now,
            updated_at: now,
        }
    }
}
