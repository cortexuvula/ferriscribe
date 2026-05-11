pub mod key_storage;
pub mod keychain;
pub mod machine_id;
pub mod phi_redactor;
pub mod audit_logger;
pub mod input_sanitizer;
pub mod rate_limiter;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SecurityError {
    #[error("Decryption error: {0}")]
    Decryption(String),
    #[error("Encryption error: {0}")]
    Encryption(String),
    #[error("Invalid key format")]
    InvalidFormat,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Key not found: {0}")]
    KeyNotFound(String),
    #[error("master key unavailable: {reason}")]
    MasterKeyUnavailable { reason: String },
}

pub type SecurityResult<T> = Result<T, SecurityError>;
