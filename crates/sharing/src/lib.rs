//! # medical-sharing
//!
//! LAN and Tailscale sharing for FerriScribe -- run AI inference and
//! speech-to-text on a powerful office server while clinicians connect from
//! laptops over the local network.
//!
//! ## Subsystems
//!
//! | Module | Role |
//! |---|---|
//! | [`orchestrator`] | [`SharingService`] -- top-level start/stop/status |
//! | [`auth_proxy`] | Bearer-validated reverse proxy (Ollama, whisper, LM Studio) |
//! | [`pairing`] | One-shot 6-digit enrollment codes |
//! | [`token_store`] | SQLCipher-encrypted per-client token CRUD |
//! | [`mdns`] | mDNS advertiser and browser (`_ferriscribe._tcp.local.`) |
//! | [`qr`] | `ferriscribe://pair?...` URL codec |
//! | [`service_installer`] | Persistent Ollama service (launchd / systemd / schtasks) |
//! | [`whisper_supervisor`] | whisper-server binary download + process supervision |
//! | [`tailscale`] | `tailscale status --json` parser |
//! | [`suggested_label`] | Sanitised OS hostname for default client labels |
//!
//! ## PHI safety
//!
//! No patient data ever crosses these modules. Audio bytes pass through the
//! auth proxy as opaque body bytes. Nothing in this crate writes transcripts,
//! SOAP notes, medications, or allergies to logs or stdout.
//!
//! ## Quick start
//!
//! ```rust,ignore
//! use medical_sharing::{SharingService, SharingConfig};
//!
//! let config = SharingConfig::default();
//! let svc = SharingService::new(config)?;
//! svc.start().await?;
//! // ... server is now broadcasting mDNS, accepting pairing requests, and
//! //     proxying authenticated STT/AI traffic.
//! svc.stop().await?;
//! # Ok::<(), medical_sharing::SharingError>(())
//! ```

pub mod auth_proxy;
pub mod mdns;
pub mod orchestrator;
pub mod pairing;
pub mod qr;
pub mod service_installer;
pub mod suggested_label;
pub mod tailscale;
pub mod token_store;
pub mod upstream;
pub mod whisper_supervisor;

pub use orchestrator::{SharingConfig, SharingService, SharingStatus};

/// Unified error type for all sharing subsystems.
///
/// Each variant wraps a subsystem-specific error message. The
/// [`InvalidPath`](SharingError::InvalidPath) variant is used by the service
/// installer when a filesystem path has no parent directory.
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

/// Convenience alias for `Result<T, SharingError>`.
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
