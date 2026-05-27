//! Saved recipients store.
//!
//! Stub implementation -- full CRUD is planned for a future iteration.
//! The `saved_recipients` table is created by migration `m001`.

/// Repository for saved letter/recipient contacts.
///
/// Currently a stub with no methods. The `saved_recipients` table exists
/// in the schema (created by `m001_initial`) and will be populated by a
/// future implementation.
pub struct RecipientsRepo;

impl RecipientsRepo {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RecipientsRepo {
    fn default() -> Self {
        Self::new()
    }
}
