//! User dictionary entry type used by the bidirectional dictionary sync feature.
//!
//! A user dictionary entry is one accepted spelling for the in-app
//! spellchecker (e.g. "Lisinopril"). Each entry has a deterministic ID derived
//! from its normalized word so that two machines independently adding the same
//! word produce the same row — enabling per-item last-write-wins merge. This
//! mirrors [`crate::types::condition_chip::ConditionChip`].

use serde::{Deserialize, Serialize};

/// Fixed namespace for UUID v5 generation of user dictionary IDs.
/// Generated once and hardcoded — must never change (would break ID stability
/// and split a word's history across two ids).
const DICT_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0xd1, 0xc7, 0x4a, 0x2b, 0x8e, 0xf6, 0x4d, 0xa2, 0x91, 0x3e, 0x5f, 0x71, 0xa3, 0x28, 0xc9, 0x0b,
]);

/// A user dictionary entry with sync metadata.
///
/// - `id`: deterministic UUID v5 from [`normalize_for_id`] of `word`. Two
///   machines adding "Lisinopril" produce the same id.
/// - `updated_at`: ISO 8601 UTC string — the last-write-wins clock.
/// - `deleted_at`: tombstone timestamp. `None` means active.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserDictEntry {
    pub id: String,
    pub word: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// Normalize a word for deterministic ID generation.
///
/// Lowercases and trims so "Lisinopril", "lisinopril ", and "LISINOPRIL" all
/// produce the same id (matching the case-insensitive uniqueness the
/// `user_dictionary` table has always enforced).
pub fn normalize_for_id(word: &str) -> String {
    word.trim().to_lowercase()
}

/// Generate a deterministic UUID v5 from the normalized word.
///
/// Same word always produces the same UUID, across machines and restarts.
/// Uses a distinct namespace from condition chips so a word and a condition
/// with the same text never collide.
pub fn deterministic_id(word: &str) -> String {
    uuid::Uuid::new_v5(&DICT_NAMESPACE, normalize_for_id(word).as_bytes()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_id_is_stable() {
        assert_eq!(
            deterministic_id("Lisinopril"),
            deterministic_id("Lisinopril")
        );
    }

    #[test]
    fn deterministic_id_is_case_insensitive() {
        assert_eq!(
            deterministic_id("Lisinopril"),
            deterministic_id("lisinopril")
        );
    }

    #[test]
    fn deterministic_id_ignores_whitespace() {
        assert_eq!(
            deterministic_id("Lisinopril"),
            deterministic_id(" Lisinopril ")
        );
    }

    #[test]
    fn different_words_have_different_ids() {
        assert_ne!(deterministic_id("Lisinopril"), deterministic_id("Atenolol"));
    }

    #[test]
    fn dict_id_differs_from_condition_chip_id() {
        // Same text, different namespaces → different ids. Sanity check that
        // the dict namespace really is distinct from the condition-chip one.
        use crate::types::condition_chip::deterministic_id as chip_id;
        assert_ne!(deterministic_id("Hypertension"), chip_id("Hypertension"));
    }
}
