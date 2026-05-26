//! `medical-security` — encrypted key storage, PHI redaction, and safety
//! primitives for FerriScribe.
//!
//! This crate is the HIPAA-compliance backstop: it owns every operation that
//! touches secrets or patient-identifiable information at the boundary
//! between trusted in-process memory and anything persistent or observable
//! (disk files, log sinks, export archives).
//!
//! # Modules at a glance
//!
//! | Module | Purpose |
//! |---|---|
//! | [`key_storage`] | AES-256-GCM encrypted API-key store (PBKDF2 master key) |
//! | [`keychain`] | Cross-platform OS keychain wrapper for the SQLCipher DB key |
//! | [`machine_id`] | Stable per-machine identifier for key derivation |
//! | [`phi_redactor`] | Regex-based PHI/PII redaction with per-recording extensions |
//! | [`audit_logger`] | PHI-redacting wrapper for log payloads |
//! | [`input_sanitizer`] | HTML stripping and UTF-8-safe truncation |
//! | [`rate_limiter`] | In-process token-bucket limiter |
//!
//! # Master-key derivation
//!
//! [`key_storage::KeyStorage`] derives its cipher key from the
//! `MEDICAL_ASSISTANT_MASTER_KEY` environment variable when set, otherwise
//! from [`machine_id::get_machine_id`], via PBKDF2-HMAC-SHA256 with 600,000
//! iterations over a 32-byte salt persisted next to the key file. See the
//! crate README for the full derivation flow and the "losing the master key
//! is unrecoverable" gotcha.
//!
//! # Cross-crate contracts
//!
//! The sharing auth proxy (`crates/sharing/src/auth_proxy.rs`) and the STT
//! client (`crates/stt-providers/src/client.rs`) coordinate on the
//! `x-auth-reason: unknown-token` HTTP header — do not rename the header on
//! one side without the other. See the README "Cross-Crate Contracts"
//! section for the full list.

pub mod key_storage;
pub mod keychain;
pub mod machine_id;
pub mod phi_redactor;
pub mod audit_logger;
pub mod input_sanitizer;
pub mod rate_limiter;

use thiserror::Error;

/// Errors returned by the security crate's public APIs.
///
/// Most variants carry a human-readable message rather than a structured
/// payload — they are intended for logging and UI surfacing, not for
/// programmatic branching. The exceptions are:
///
/// - [`SecurityError::KeyNotFound`] — callers may treat this as "not yet
///   configured" and prompt the user.
/// - [`SecurityError::MasterKeyUnavailable`] — signals that neither the
///   `MEDICAL_ASSISTANT_MASTER_KEY` env var nor the machine-id lookup
///   produced a usable password; the keystore cannot bootstrap.
/// - [`SecurityError::InvalidFormat`] — the on-disk ciphertext is shorter
///   than a nonce; the file is corrupted or from an incompatible version.
#[derive(Error, Debug)]
pub enum SecurityError {
    /// AES-GCM authentication-tag mismatch — wrong key, tampered ciphertext,
    /// or a salt/nonce that no longer matches the stored blob.
    #[error("Decryption error: {0}")]
    Decryption(String),

    /// AES-GCM `encrypt` call failed (typically an internal invariant
    /// violation; the underlying crate only errors on overflow-sized input).
    #[error("Encryption error: {0}")]
    Encryption(String),

    /// Stored ciphertext is shorter than the 12-byte nonce prefix.
    #[error("Invalid key format")]
    InvalidFormat,

    /// Filesystem failure while reading or writing `keys.json` / `salt.bin`.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// No entry under the requested provider name — not an error in the
    /// failure sense; callers typically treat this as "not configured yet".
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    /// Bootstrap failure: neither `MEDICAL_ASSISTANT_MASTER_KEY` nor the
    /// machine-id lookup produced a usable password. The keystore cannot
    /// open until one of these sources is restored.
    #[error("master key unavailable: {reason}")]
    MasterKeyUnavailable { reason: String },

    /// Catch-all for errors that do not fit the other variants (e.g. mutex
    /// poison on the internal file lock).
    #[error("{0}")]
    Other(String),
}

/// Crate-wide result alias.
pub type SecurityResult<T> = Result<T, SecurityError>;
