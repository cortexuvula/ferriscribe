use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A target audience for generated letters.
///
/// Built-in audiences ship with the app (e.g. "Referral Letter to Specialist",
/// "Patient Discharge Summary"). Custom audiences are created by the user and
/// can have their own system prompts and templates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LetterAudience {
    pub id: Uuid,
    pub name: String,
    pub system_prompt: String,
    pub user_template: Option<String>,
    pub is_builtin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LetterAudience {
    /// Create a new custom letter audience.
    ///
    /// Generates a new UUID, sets `is_builtin` to false, and timestamps to now.
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
    /// Uses the provided UUID, sets `is_builtin` to true, and timestamps to now.
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
