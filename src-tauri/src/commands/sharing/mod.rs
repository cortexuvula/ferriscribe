//! Tauri commands for the sharing/pairing subsystem.
//!
//! Submodules:
//! - [`lifecycle`] — start/stop/status of the office-server side
//! - [`pairing`] — pairing-QR generation, client list/revoke, and the
//!   client-side pair/unpair flow against an office server
//! - [`discovery`] — mDNS + Tailscale-aware peer discovery
//!
//! This file holds the cross-cutting DTOs and persistence helpers that all
//! three submodules touch.

use medical_core::error::{AppError, AppResult};
use medical_sharing::{SharingStatus, qr::PairPorts};
use serde::{Deserialize, Serialize};

pub mod discovery;
pub mod lifecycle;
pub mod pairing;
pub mod settings_helpers;

// `start_sharing_inner` is a regular async function (not a Tauri command), so
// re-exporting via `pub use` is fine — only `#[tauri::command]` items have the
// macro-generated sibling symbols that don't survive a re-export.
pub use lifecycle::{start_sharing_inner, stop_sharing_inner};

#[derive(Debug, Serialize)]
pub struct SharingStatusDto {
    pub enabled: bool,
    pub ollama_ok: bool,
    pub whisper_ok: bool,
    pub lmstudio_ok: bool,
    pub mdns_ok: bool,
    pub pairing_ok: bool,
    pub paired_clients: u32,
}

impl From<SharingStatus> for SharingStatusDto {
    fn from(s: SharingStatus) -> Self {
        Self {
            enabled: s.enabled,
            ollama_ok: s.ollama_ok,
            whisper_ok: s.whisper_ok,
            lmstudio_ok: s.lmstudio_ok,
            mdns_ok: s.mdns_ok,
            pairing_ok: s.pairing_ok,
            paired_clients: s.paired_clients,
        }
    }
}

/// Non-secret connection metadata persisted across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedConnection {
    pub lan: Option<String>,
    pub tailscale: Option<String>,
    pub ports: PairPorts,
    pub label: String,
}

pub(super) fn paired_connection_path() -> AppResult<std::path::PathBuf> {
    let app_data = dirs::data_dir()
        .ok_or_else(|| AppError::Other("no app data dir".into()))?
        .join("rust-medical-assistant");
    std::fs::create_dir_all(&app_data)?;
    Ok(app_data.join("sharing-paired.json"))
}

/// Persisted "this machine is the office server" config. Written when the
/// user clicks Start sharing, removed when they Stop sharing. The presence
/// of this file at app startup is what triggers auto-resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Schema version. Bumped if/when fields are added so older installs
    /// can choose to ignore unrecognised configs rather than panic.
    #[serde(default = "default_server_config_version")]
    pub version: u32,
    pub friendly_name: String,
}

fn default_server_config_version() -> u32 {
    1
}

pub fn server_config_path() -> AppResult<std::path::PathBuf> {
    let app_data = dirs::data_dir()
        .ok_or_else(|| AppError::Other("no app data dir".into()))?
        .join("rust-medical-assistant");
    std::fs::create_dir_all(&app_data)?;
    Ok(app_data.join("sharing-server.json"))
}

pub(super) fn write_server_config(cfg: &ServerConfig) -> AppResult<()> {
    let path = server_config_path()?;
    let json = serde_json::to_string(cfg)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Idempotently delete the persisted server config. Missing file is not an
/// error — Stop sharing should always succeed in clearing the auto-resume.
pub(super) fn delete_server_config() {
    if let Ok(path) = server_config_path() {
        let _ = std::fs::remove_file(&path);
    }
}

#[derive(Debug, Serialize)]
pub struct ClientDto {
    pub id: i64,
    pub label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_round_trips_through_json() {
        let cfg = ServerConfig {
            version: 1,
            friendly_name: "Clinic Server".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: ServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, 1);
        assert_eq!(back.friendly_name, "Clinic Server");
    }

    #[test]
    fn server_config_defaults_version_when_missing() {
        // An older install (or hand-edited file) might lack `version`. We
        // accept it and default to 1 so we don't reject our own writes.
        let json = r#"{"friendly_name":"Old Format"}"#;
        let back: ServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(back.version, 1);
        assert_eq!(back.friendly_name, "Old Format");
    }

    #[test]
    fn write_then_delete_server_config_is_idempotent() {
        // Writes the real config file (whatever dirs::data_dir() points at).
        // We snapshot whatever was there first and restore it afterwards so we
        // don't clobber a developer's actual paired install state.
        let path = match server_config_path() {
            Ok(p) => p,
            Err(_) => return, // headless / sandboxed env without data_dir — nothing to test
        };
        let saved = std::fs::read(&path).ok();

        // Ensure clean slate.
        let _ = std::fs::remove_file(&path);
        delete_server_config(); // idempotent — file already missing

        // Write, confirm, then delete twice.
        write_server_config(&ServerConfig {
            version: 1,
            friendly_name: "Test".into(),
        })
        .expect("write should succeed");
        assert!(path.exists(), "config should exist after write");
        delete_server_config();
        assert!(!path.exists(), "config should be gone after delete");
        delete_server_config(); // second delete is a no-op

        // Restore prior state if any.
        if let Some(bytes) = saved {
            std::fs::write(&path, bytes).ok();
        }
    }
}
