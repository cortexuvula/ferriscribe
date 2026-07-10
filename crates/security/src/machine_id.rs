//! Cross-platform machine ID derivation.
//!
//! Returns a stable 64-character lowercase hex SHA-256 string derived from a
//! platform-specific hardware/OS identifier. Used as the PBKDF2 password
//! when the `MEDICAL_ASSISTANT_MASTER_KEY` environment variable is not set,
//! which is the common case for production installations.
//!
//! # Platform sources (in order of preference)
//!
//! | Platform | Source |
//! |---|---|
//! | Linux | `/etc/machine-id`, then `/var/lib/dbus/machine-id` |
//! | macOS | `IOPlatformUUID` via `ioreg -rd1 -c IOPlatformExpertDevice` |
//! | Windows | `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` |
//! | Other / fallback | Persistent random ID in the app data dir |
//!
//! # Stability
//!
//! The returned hash is stable for a given machine as long as the underlying
//! OS identifier does not change. It will change across:
//! - OS reinstalls / VM clones (new `machine-id` or IOPlatformUUID)
//! - Hardware changes that alter the platform UUID
//! - CI runners with ephemeral identities (use the env-var override there)

use sha2::{Digest, Sha256};
use std::fmt::Write as FmtWrite;

use crate::SecurityResult;

/// Returns a stable 64-character lowercase hex SHA-256 string that
/// uniquely identifies this machine.
///
/// The value is derived from a platform-specific hardware identifier (see
/// the module docs for the source on each OS). Callers should treat the
/// result as opaque — its only guaranteed property is stability across
/// calls on the same machine.
///
/// # Errors
///
/// Returns [`crate::SecurityError::Io`] only if the platform-specific
/// lookup and the [`fallback_id`] both fail, which in practice requires
/// the `USER`/`HOME` environment variables to be unset *and* the platform
/// identifier to be unreadable.
pub fn get_machine_id() -> SecurityResult<String> {
    let raw = raw_machine_id()?;
    Ok(sha256_hex(raw.as_bytes()))
}

/// Hashes arbitrary bytes and returns the lowercase hex string.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result {
        write!(hex, "{:02x}", byte).expect("write to String is infallible");
    }
    hex
}

/// Reads the raw (un-hashed) platform identifier.
#[cfg(target_os = "linux")]
fn raw_machine_id() -> SecurityResult<String> {
    // Try /etc/machine-id first, then DBus path, then fallback.
    for path in &["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }
    }
    Ok(fallback_id())
}

#[cfg(target_os = "macos")]
fn raw_machine_id() -> SecurityResult<String> {
    use std::process::Command;

    // ioreg -rd1 -c IOPlatformExpertDevice | grep IOPlatformUUID
    if let Ok(output) = Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        && let Ok(text) = std::str::from_utf8(&output.stdout)
    {
        for line in text.lines() {
            if line.contains("IOPlatformUUID") {
                // The line looks like: `... "IOPlatformUUID" = "XXXX-...-XXXX"`
                // Split on double-quotes and take the last quoted value.
                let parts: Vec<&str> = line.split('"').collect();
                if parts.len() >= 4 {
                    let uuid = parts[parts.len() - 2];
                    if !uuid.is_empty() {
                        return Ok(uuid.to_string());
                    }
                }
            }
        }
    }
    Ok(fallback_id())
}

#[cfg(target_os = "windows")]
fn raw_machine_id() -> SecurityResult<String> {
    use std::process::Command;

    // Query registry: HKLM\SOFTWARE\Microsoft\Cryptography  /v MachineGuid
    if let Ok(output) = Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
    {
        if let Ok(text) = std::str::from_utf8(&output.stdout) {
            for line in text.lines() {
                if line.contains("MachineGuid") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if let Some(guid) = parts.last() {
                        return Ok(guid.to_string());
                    }
                }
            }
        }
    }
    Ok(fallback_id())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn raw_machine_id() -> SecurityResult<String> {
    Ok(fallback_id())
}

/// Generate a fallback machine ID when hardware ID lookup fails.
///
/// Uses a persistent random ID stored in the app data directory so it's
/// stable across restarts but not guessable. If the persistent file can't
/// be created, includes a process-random component as a last resort.
///
/// This is **public** so integration tests can verify that hashing the
/// fallback still produces a valid 64-char hex machine ID. Production
/// callers should always use [`get_machine_id`] instead.
pub fn fallback_id() -> String {
    // Try to read/create a persistent random ID file.
    if let Some(data_dir) = dirs::data_dir() {
        let id_file = data_dir.join("ferriescribe").join(".machine-id");
        if let Ok(existing) = std::fs::read_to_string(&id_file) {
            let trimmed = existing.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        // Generate a new random ID and persist it.
        let random_id = uuid::Uuid::new_v4().to_string();
        if std::fs::create_dir_all(id_file.parent().unwrap()).is_ok() {
            let _ = std::fs::write(&id_file, &random_id);
        }
        return random_id;
    }

    // Absolute last resort: random (not persisted, changes per restart)
    // but at least not guessable.
    tracing::warn!("Using non-persistent random machine ID fallback");
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_64_char_hex() {
        let id = get_machine_id().expect("get_machine_id failed");
        assert_eq!(id.len(), 64, "Expected 64 hex chars, got: {}", id);
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "Expected only hex chars, got: {}",
            id
        );
    }

    #[test]
    fn is_stable() {
        let first = get_machine_id().expect("first call failed");
        let second = get_machine_id().expect("second call failed");
        assert_eq!(first, second, "machine_id should be stable across calls");
    }

    #[test]
    fn fallback_works() {
        let id = fallback_id();
        // New fallback is a UUID v4 string (persistent or random), so it
        // must NOT be empty and must contain the UUID hyphen separators.
        assert!(!id.is_empty(), "fallback_id must not be empty");
        assert!(
            id.contains('-'),
            "fallback_id should be a UUID, got: {}",
            id
        );
        // Hashing the fallback should also produce a 64-char hex
        let hashed = sha256_hex(id.as_bytes());
        assert_eq!(hashed.len(), 64);
    }
}
