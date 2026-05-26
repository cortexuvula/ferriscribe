//! medical-sharing — LAN/Tailscale "office server" sharing layer.
//!
//! Exposes:
//! - [`SharingService`] — orchestrates the sharing subsystems (auth proxy,
//!   mDNS, pairing service, whisper-cpp supervisor) based on a [`SharingConfig`].
//! - Per-module APIs for unit tests and Tauri command wiring.
//!
//! No PHI ever crosses these modules. Audio bytes pass through the auth
//! proxy as opaque body bytes.

pub mod auth_proxy;
pub mod mdns;
pub mod orchestrator;
pub mod pairing;
pub mod qr;
pub mod service_installer;
pub mod suggested_label;
pub mod tailscale;
pub mod token_store;
pub mod whisper_supervisor;

pub use orchestrator::{SharingConfig, SharingService, SharingStatus};

#[derive(Debug, thiserror::Error)]
pub enum SharingError {
    #[error("token store: {0}")]
    TokenStore(String),
    #[error("pairing: {0}")]
    Pairing(String),
    #[error("auth proxy: {0}")]
    AuthProxy(String),
    #[error("mdns: {0}")]
    Mdns(String),
    #[error("whisper supervisor: {0}")]
    WhisperSupervisor(String),
    #[error("service installer: {0}")]
    ServiceInstaller(String),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, SharingError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_path_variant_includes_underlying_reason_and_path() {
        let err = SharingError::InvalidPath("no parent dir: /".to_string());
        let msg = err.to_string();
        assert!(
            msg.contains("no parent dir"),
            "expected message to include the underlying reason, got: {msg}"
        );
        assert!(
            msg.contains('/'),
            "expected message to include the offending path '/', got: {msg}"
        );
    }
}
