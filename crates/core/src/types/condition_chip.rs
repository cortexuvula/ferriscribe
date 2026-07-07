//! Condition chip type used by the condition-chip sync feature.
//!
//! A condition chip is a practice-wide quick-add preset shown under "Known
//! conditions" (e.g. "Hypertension"). Each chip has a deterministic ID derived
//! from its normalized text so that two machines independently adding the same
//! condition produce the same row — enabling per-item last-write-wins merge.

use serde::{Deserialize, Serialize};

/// Fixed namespace for UUID v5 generation of condition chip IDs.
/// Generated once and hardcoded — must never change (would break ID stability).
const CONDITION_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x4a, 0x3e, 0xc1, 0x07, 0x9b, 0x2d, 0x4f, 0x6a, 0xa1, 0x10, 0xd8, 0x4f, 0xa2, 0xb3, 0xc5, 0xe7,
]);

/// A condition chip entry with sync metadata.
///
/// - `id`: deterministic UUID v5 from `normalize_for_id(&text)`. Two machines
///   adding "Hypertension" produce the same id.
/// - `updated_at`: ISO 8601 UTC string — the last-write-wins clock.
/// - `deleted_at`: tombstone timestamp. `None` means active.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConditionChip {
    pub id: String,
    pub text: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
}

/// Normalize condition text for deterministic ID generation.
///
/// Lowercases and trims so "Hypertension", "hypertension ", and
/// "HYPERTENSION" all produce the same id.
pub fn normalize_for_id(text: &str) -> String {
    text.trim().to_lowercase()
}

/// Generate a deterministic UUID v5 from normalized condition text.
///
/// Same text always produces the same UUID, across machines and restarts.
pub fn deterministic_id(text: &str) -> String {
    uuid::Uuid::new_v5(&CONDITION_NAMESPACE, normalize_for_id(text).as_bytes()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_id_is_stable() {
        assert_eq!(
            deterministic_id("Hypertension"),
            deterministic_id("Hypertension")
        );
    }

    #[test]
    fn deterministic_id_is_case_insensitive() {
        assert_eq!(
            deterministic_id("Hypertension"),
            deterministic_id("hypertension")
        );
    }

    #[test]
    fn deterministic_id_ignores_whitespace() {
        assert_eq!(
            deterministic_id("Hypertension"),
            deterministic_id(" Hypertension ")
        );
    }

    #[test]
    fn different_conditions_have_different_ids() {
        assert_ne!(
            deterministic_id("Hypertension"),
            deterministic_id("Diabetes")
        );
    }
}
