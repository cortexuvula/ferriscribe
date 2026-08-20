//! Encrypted off-machine backup for FerriScribe.
//!
//! Design contract (see `.zcode/plans/plan-ferriscribe-off-machine-backup.md`):
//!
//! - **R1 Key escrow** — snapshots are encrypted under a *backup wrapping
//!   key* whose off-machine copies are two independent, individually
//!   verifiable escrow artifacts: a printable recovery sheet and an
//!   offline USB file ([`escrow`]). The SQLCipher DB key itself travels
//!   inside every snapshot, wrapped under the snapshot key, so ciphertext
//!   + escrow = recoverable.
//! - **R3 Append-only** — the target-side agent ([`agent`], added in a
//!   follow-up commit) exposes no delete/overwrite route to the push
//!   credential, so a compromised source cannot erase its own history.
//! - **R4 Tested restore** — [`drill`] restores the latest snapshot into a
//!   temp directory, opens the restored SQLCipher DB with the recovered
//!   key, decrypts a sample recording, and diffs record counts.
//! - **R5 Integrity + authenticity** — every snapshot carries an HMAC-SHA256
//!   tag over the receipt's canonical fields plus every payload hash;
//!   verification fails closed ([`snapshot::verify_snapshot`]).
//!
//! # PHI rules
//!
//! Snapshot filenames are opaque (`snap-<timestamp>-<rand>`, `fNNNNNN.bin`).
//! The plaintext receipt carries only counts, sizes, and the HMAC tag — no
//! paths, no patient data. Original relative paths (which may embed patient
//! names in recording filenames) live only inside the *encrypted* manifest.

pub mod drill;
pub mod escrow;
pub mod keys;
pub mod snapshot;

/// Errors surfaced by the backup tooling.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("keychain error: {0}")]
    Keychain(#[from] medical_security::keychain::KeychainError),
    #[error("crypto error: {0}")]
    Crypto(#[from] medical_security::file_crypto::FileCryptoError),
    #[error("database error: {0}")]
    Db(#[from] medical_db::DbError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("snapshot verification failed: {0}")]
    Verification(String),
    #[error("escrow error: {0}")]
    Escrow(String),
    #[error("missing key material: {0}")]
    MissingKey(String),
    #[error("snapshot format error: {0}")]
    Format(String),
}

/// Result alias for the backup crate.
pub type BackupResult<T> = Result<T, BackupError>;

/// Raw rusqlite errors (backup opens its own connections alongside the
/// pooled ones from `medical_db`).
impl From<rusqlite::Error> for BackupError {
    fn from(e: rusqlite::Error) -> Self {
        BackupError::Db(medical_db::DbError::Sqlite(e))
    }
}
