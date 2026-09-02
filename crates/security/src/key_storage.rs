//! AES-256-GCM key storage with PBKDF2 key derivation.
//!
//! API keys are encrypted with a per-entry random 12-byte nonce and stored in
//! a JSON file (`keys.json`). The master cipher key is derived from either
//! the `MEDICAL_ASSISTANT_MASTER_KEY` environment variable or the machine ID
//! using PBKDF2-HMAC-SHA256 with 600,000 iterations over a 32-byte salt
//! persisted to `salt.bin` next to the key file.
//!
//! # Lifecycle
//!
//! 1. `KeyStorage::open(config_dir)` loads or creates `salt.bin`, derives
//!    the 32-byte cipher key, and constructs an `Aes256Gcm` instance.
//! 2. `store_key(provider, api_key)` generates a fresh 12-byte nonce via
//!    `OsRng`, encrypts, and writes `base64(nonce || ciphertext)` to
//!    `keys.json` alongside an ISO-8601 timestamp and an 8-char hex prefix
//!    of SHA-256(plaintext) used as an integrity hint.
//! 3. `get_key(provider)` decodes, splits nonce from ciphertext, and
//!    decrypts. Returns `Ok(None)` when the provider slot is empty.
//!
//! # Gotchas
//!
//! - The same plaintext stored twice yields different ciphertexts because
//!   each `store_key` call draws a fresh nonce. Equality of ciphertexts
//!   therefore does *not* leak equality of keys.
//! - Deleting `salt.bin` invalidates every stored key: a new salt produces
//!   a different PBKDF2 output, so AES-GCM's auth tag fails on decrypt.
//! - The internal `file_lock` mutex serializes writes; reads are lock-free.
//!   A poisoned mutex surfaces as `SecurityError::Other`.

// Two `RngCore` traits coexist in the dep graph: rand 0.9 (rand_core 0.9)
// and aes_gcm's re-exported rand_core 0.6. We use `aes_gcm::aead::OsRng`
// (0.6) for nonces, but `rand::rng()` (0.9) for salt generation, so both
// traits must be in scope. The 0.6 trait is aliased to disambiguate.
use aes_gcm::aead::rand_core::RngCore as AeadRngCore;
use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng},
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};

use crate::machine_id::get_machine_id;
use crate::{SecurityError, SecurityResult};

// ─── Constants ────────────────────────────────────────────────────────────────

const SALT_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
const PBKDF2_ITERATIONS: u32 = 600_000;
const KEY_FILE_NAME: &str = "keys.json";
const SALT_FILE_NAME: &str = "salt.bin";

// ─── Data structures persisted to disk ────────────────────────────────────────

/// One encrypted entry in the key file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredKey {
    /// Base64-encoded nonce (12 bytes) concatenated with ciphertext.
    encrypted: String,
    /// ISO 8601 timestamp of when this key was stored.
    stored_at: String,
    /// First 8 hex chars of SHA-256(plaintext) for quick integrity hints.
    key_hash: String,
}

/// The on-disk JSON structure: a map of provider name -> StoredKey.
#[derive(Debug, Default, Serialize, Deserialize)]
struct KeyFile {
    keys: HashMap<String, StoredKey>,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Manages encrypted storage of API keys for named providers.
///
/// Keys are encrypted with AES-256-GCM and persisted as JSON in a
/// caller-supplied config directory. The cipher key is derived once at
/// construction time via PBKDF2-HMAC-SHA256; subsequent `store_key` and
/// `get_key` calls reuse the same cipher instance.
///
/// Instances are cheap to construct but not `Sync` (the internal mutex is
/// `std::sync::Mutex` — wrap in `Arc` and share across threads as needed,
/// which is how `src-tauri` holds it in `AppState::keys`).
pub struct KeyStorage {
    cipher: Aes256Gcm,
    storage_path: PathBuf,
    file_lock: std::sync::Mutex<()>,
}

impl KeyStorage {
    /// Open (or create) the key store located inside `config_dir`.
    ///
    /// Creates `config_dir` if it does not exist, loads or generates
    /// `salt.bin`, and derives the master cipher key. This is the only
    /// constructor; there is no in-memory-only variant because the salt
    /// must persist alongside the ciphertext.
    ///
    /// # Errors
    ///
    /// - [`SecurityError::Io`] if the directory cannot be created or the
    ///   salt file cannot be read/written.
    /// - [`SecurityError::MasterKeyUnavailable`] if neither the
    ///   `MEDICAL_ASSISTANT_MASTER_KEY` env var nor the machine-id lookup
    ///   yields a non-empty password.
    pub fn open(config_dir: &Path) -> SecurityResult<Self> {
        std::fs::create_dir_all(config_dir)?;

        let salt = load_or_create_salt(config_dir)?;
        let master_key = derive_master_key(&salt)?;
        let key = Key::<Aes256Gcm>::from_slice(&master_key);
        let cipher = Aes256Gcm::new(key);

        Ok(Self {
            cipher,
            storage_path: config_dir.join(KEY_FILE_NAME),
            file_lock: std::sync::Mutex::new(()),
        })
    }

    /// Encrypt `api_key` and store it under `provider`.
    ///
    /// Generates a fresh 12-byte nonce via `OsRng` so that storing the
    /// same plaintext twice yields different ciphertexts. Overwrites any
    /// existing entry for `provider` atomically (read-modify-write under
    /// the file lock).
    ///
    /// # Errors
    ///
    /// - [`SecurityError::Encryption`] if AES-GCM rejects the input.
    /// - [`SecurityError::Io`] on filesystem failure.
    /// - [`SecurityError::Other`] if the internal mutex is poisoned.
    pub fn store_key(&self, provider: &str, api_key: &str) -> SecurityResult<()> {
        let _lock = self
            .file_lock
            .lock()
            .map_err(|e| SecurityError::Other(format!("mutex poisoned in store_key: {e}")))?;
        let mut nonce_bytes = [0u8; NONCE_LENGTH];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, api_key.as_bytes())
            .map_err(|e| SecurityError::Encryption(e.to_string()))?;

        // Store nonce || ciphertext as a single base64 blob.
        let mut combined = Vec::with_capacity(NONCE_LENGTH + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);

        let entry = StoredKey {
            encrypted: STANDARD.encode(&combined),
            stored_at: Utc::now().to_rfc3339(),
            key_hash: key_hash_hex(api_key.as_bytes()),
        };

        let mut file = self.load_file()?;
        file.keys.insert(provider.to_string(), entry);
        self.save_file(&file)
    }

    /// Decrypt and return the key for `provider`, or `Ok(None)` if no
    /// entry exists under that name.
    ///
    /// # Errors
    ///
    /// - [`SecurityError::Decryption`] if the ciphertext cannot be
    ///   base64-decoded, the nonce is present but AES-GCM's auth tag
    ///   rejects (wrong master key, tampered blob, or stale `salt.bin`),
    ///   or the plaintext is not valid UTF-8.
    /// - [`SecurityError::InvalidFormat`] if the stored blob is shorter
    ///   than the 12-byte nonce prefix.
    /// - [`SecurityError::Io`] if `keys.json` cannot be read.
    pub fn get_key(&self, provider: &str) -> SecurityResult<Option<String>> {
        let file = self.load_file()?;
        let Some(entry) = file.keys.get(provider) else {
            return Ok(None);
        };

        let combined = STANDARD
            .decode(&entry.encrypted)
            .map_err(|e| SecurityError::Decryption(e.to_string()))?;

        if combined.len() < NONCE_LENGTH {
            return Err(SecurityError::InvalidFormat);
        }

        let (nonce_bytes, ciphertext) = combined.split_at(NONCE_LENGTH);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| SecurityError::Decryption(e.to_string()))?;

        String::from_utf8(plaintext)
            .map(Some)
            .map_err(|e| SecurityError::Decryption(e.to_string()))
    }

    /// Remove the key for `provider`. Returns `true` if an entry existed
    /// and was deleted, `false` if the slot was already empty.
    ///
    /// # Errors
    ///
    /// - [`SecurityError::Io`] on filesystem failure.
    /// - [`SecurityError::Other`] if the internal mutex is poisoned.
    pub fn remove_key(&self, provider: &str) -> SecurityResult<bool> {
        let _lock = self
            .file_lock
            .lock()
            .map_err(|e| SecurityError::Other(format!("mutex poisoned in remove_key: {e}")))?;
        let mut file = self.load_file()?;
        let existed = file.keys.remove(provider).is_some();
        if existed {
            self.save_file(&file)?;
        }
        Ok(existed)
    }

    /// List all stored provider names (unordered).
    ///
    /// # Errors
    ///
    /// - [`SecurityError::Io`] if `keys.json` cannot be read.
    pub fn list_providers(&self) -> SecurityResult<Vec<String>> {
        let file = self.load_file()?;
        Ok(file.keys.keys().cloned().collect())
    }

    // ─── Internal helpers ─────────────────────────────────────────────────────

    fn load_file(&self) -> SecurityResult<KeyFile> {
        if !self.storage_path.exists() {
            return Ok(KeyFile::default());
        }
        let bytes = std::fs::read(&self.storage_path)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| SecurityError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
    }

    fn save_file(&self, key_file: &KeyFile) -> SecurityResult<()> {
        let json = serde_json::to_vec_pretty(key_file)
            .map_err(|e| SecurityError::Io(std::io::Error::other(e)))?;
        atomic_write(&self.storage_path, &json)?;
        Ok(())
    }
}

// ─── Key derivation ───────────────────────────────────────────────────────────

/// Derives the 32-byte master key from the salt using PBKDF2-HMAC-SHA256.
///
/// The password is taken from the `MEDICAL_ASSISTANT_MASTER_KEY` env var when
/// set, otherwise the machine ID is used.  Returns `Err(MasterKeyUnavailable)`
/// when neither source yields a non-empty value — never falls back to a
/// guessable constant.
fn derive_master_key(salt: &[u8]) -> SecurityResult<[u8; 32]> {
    derive_master_key_from(
        salt,
        || std::env::var("MEDICAL_ASSISTANT_MASTER_KEY").ok(),
        get_machine_id,
    )
}

/// Testable core of `derive_master_key`: accepts injectable env-var and
/// machine-id resolvers so the two error paths can be exercised without
/// touching process state or OS calls.
fn derive_master_key_from<E, M>(salt: &[u8], env_var: E, machine_id: M) -> SecurityResult<[u8; 32]>
where
    E: FnOnce() -> Option<String>,
    M: FnOnce() -> SecurityResult<String>,
{
    let password = match env_var() {
        Some(v) if !v.is_empty() => v,
        _ => machine_id().map_err(|e| SecurityError::MasterKeyUnavailable {
            reason: format!(
                "MEDICAL_ASSISTANT_MASTER_KEY not set and machine id lookup failed: {e}"
            ),
        })?,
    };

    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, PBKDF2_ITERATIONS, &mut key);
    Ok(key)
}

/// Load an existing salt or create and persist a new random one.
///
/// Creation is exclusive (`create_new`): if two first-time opens race,
/// exactly one wins and the loser must ADOPT the winner's salt. A
/// last-writer-wins overwrite would leave the loser deriving keys from a
/// salt that no longer matches the one on disk — permanently
/// undecryptable data.
fn load_or_create_salt(config_dir: &Path) -> SecurityResult<Vec<u8>> {
    let path = config_dir.join(SALT_FILE_NAME);
    // Fast path: the salt has existed since a previous run. The length
    // check is load-bearing: a concurrent first-open racer may have
    // CREATED the file but not yet written/synced its bytes — reading at
    // that instant yields a short (often empty) salt, and adopting it
    // silently derives keys nobody else can reproduce (the CI flake this
    // fixes). Fall through to the waiting read instead.
    if path.exists() {
        if let Ok(bytes) = std::fs::read(&path)
            && bytes.len() == SALT_LENGTH
        {
            return Ok(bytes);
        }
        return read_complete_salt_or_wait(&path);
    }
    let mut salt = vec![0u8; SALT_LENGTH];
    rand::rng().fill_bytes(&mut salt);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            std::io::Write::write_all(&mut file, &salt)?;
            // Durable + visible before anyone can observe the file via
            // the AlreadyExists branch below.
            file.sync_all()?;
            Ok(salt)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Lost a concurrent first-create (or the salt appeared since
            // the exists() check): adopt the on-disk salt.
            read_complete_salt_or_wait(&path)
        }
        Err(e) => Err(SecurityError::Io(e)),
    }
}

/// Read the salt file, tolerating a concurrent winner that has created
/// but not yet finished writing it. A file that stays incomplete is
/// corruption — deriving keys from a wrong-length salt would silently
/// break every existing key, so surface it as an error instead.
fn read_complete_salt_or_wait(path: &Path) -> SecurityResult<Vec<u8>> {
    const RETRIES: u8 = 50;
    const SLEEP: std::time::Duration = std::time::Duration::from_millis(2);
    for _ in 0..RETRIES {
        let bytes = std::fs::read(path)?;
        if bytes.len() == SALT_LENGTH {
            return Ok(bytes);
        }
        std::thread::sleep(SLEEP);
    }
    Err(SecurityError::Other(format!(
        "salt file is corrupt (expected {SALT_LENGTH} bytes)"
    )))
}

/// Write `content` to `path` atomically by first writing a temp file in
/// the same directory and then renaming it over the target.
///
/// Atomicity guarantees:
/// - On POSIX, `rename(2)` is atomic: a crash mid-rename never leaves a
///   partially-written target file. Readers either see the old file or the
///   new file in its entirety.
/// - On NTFS (same volume), `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`
///   is likewise atomic.
///
/// The temp file lives next to the target (`.tmp` extension) so the
/// rename is intra-directory and therefore same-volume — required for
/// atomicity on both platforms. On any error after the temp file is
/// created, we attempt to remove it so a crash-later retry doesn't pick
/// up stale bytes.
fn atomic_write(path: &Path, content: &[u8]) -> SecurityResult<()> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, content)?;
    if let Err(e) = std::fs::rename(&temp, path) {
        // Cleanup the temp file on rename failure so it doesn't linger.
        let _ = std::fs::remove_file(&temp);
        return Err(SecurityError::Io(e));
    }
    Ok(())
}

// ─── Utility ──────────────────────────────────────────────────────────────────

/// Returns the first 8 hex characters of SHA-256(data).
fn key_hash_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(8);
    for byte in &result[..4] {
        // fmt::Write for String is infallible: it appends to the buffer and
        // never returns Err. The `let _ =` explicitly discards the Ok(()) value.
        let _ = write!(hex, "{:02x}", byte);
    }
    hex
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_temp_store() -> (TempDir, KeyStorage) {
        let dir = TempDir::new().unwrap();
        let ks = KeyStorage::open(dir.path()).unwrap();
        (dir, ks)
    }

    #[test]
    fn store_and_retrieve() {
        let (_dir, ks) = open_temp_store();
        ks.store_key("openai", "sk-test-1234").unwrap();
        let val = ks.get_key("openai").unwrap();
        assert_eq!(val, Some("sk-test-1234".to_string()));
    }

    #[test]
    fn get_nonexistent_none() {
        let (_dir, ks) = open_temp_store();
        assert_eq!(ks.get_key("does_not_exist").unwrap(), None);
    }

    #[test]
    fn overwrite_key() {
        let (_dir, ks) = open_temp_store();
        ks.store_key("provider", "first_key").unwrap();
        ks.store_key("provider", "second_key").unwrap();
        assert_eq!(
            ks.get_key("provider").unwrap(),
            Some("second_key".to_string())
        );
    }

    #[test]
    fn remove_key() {
        let (_dir, ks) = open_temp_store();
        ks.store_key("anthropic", "sk-ant-123").unwrap();
        assert!(ks.remove_key("anthropic").unwrap());
        assert_eq!(ks.get_key("anthropic").unwrap(), None);
    }

    #[test]
    fn remove_nonexistent_false() {
        let (_dir, ks) = open_temp_store();
        assert!(!ks.remove_key("ghost").unwrap());
    }

    #[test]
    fn list_providers() {
        let (_dir, ks) = open_temp_store();
        ks.store_key("openai", "a").unwrap();
        ks.store_key("anthropic", "b").unwrap();
        let mut providers = ks.list_providers().unwrap();
        providers.sort();
        assert_eq!(providers, vec!["anthropic", "openai"]);
    }

    #[test]
    fn salt_persists_across_instances() {
        let dir = TempDir::new().unwrap();
        {
            let ks = KeyStorage::open(dir.path()).unwrap();
            ks.store_key("svc", "my_secret").unwrap();
        }
        {
            let ks2 = KeyStorage::open(dir.path()).unwrap();
            let val = ks2.get_key("svc").unwrap();
            assert_eq!(val, Some("my_secret".to_string()));
        }
    }

    #[test]
    fn different_nonces_per_encryption() {
        let (_dir, ks) = open_temp_store();
        ks.store_key("p1", "same_value").unwrap();
        ks.store_key("p2", "same_value").unwrap();

        let file = ks.load_file().unwrap();
        let enc1 = &file.keys["p1"].encrypted;
        let enc2 = &file.keys["p2"].encrypted;
        // Even for the same plaintext the nonces differ, so ciphertexts differ.
        assert_ne!(enc1, enc2);
    }

    #[test]
    fn derive_master_key_from_errors_when_env_empty_and_machine_id_fails() {
        let result = derive_master_key_from(
            &[0u8; 16],
            || None,
            || Err(SecurityError::Io(std::io::Error::other("simulated"))),
        );
        assert!(matches!(
            result,
            Err(SecurityError::MasterKeyUnavailable { .. })
        ));
    }

    #[test]
    fn derive_master_key_from_uses_env_when_present() {
        let result = derive_master_key_from(
            &[0u8; 16],
            || Some("explicit".to_string()),
            || panic!("machine id should not be called"),
        );
        assert!(result.is_ok());
    }

    /// Poison the file_lock mutex so subsequent lock() calls return an error.
    fn poison_mutex(ks: &KeyStorage) {
        let lock = ks.file_lock.lock().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock;
            panic!("intentional panic to poison mutex");
        }));
        assert!(result.is_err());
    }

    #[test]
    fn store_key_returns_error_when_mutex_poisoned() {
        let (_dir, ks) = open_temp_store();
        poison_mutex(&ks);
        let result = ks.store_key("provider", "key");
        assert!(
            result.is_err(),
            "store_key should return Err on poisoned mutex"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("mutex poisoned") || err_msg.contains("poisoned"),
            "error should mention mutex poison, got: {err_msg}"
        );
    }

    #[test]
    fn remove_key_returns_error_when_mutex_poisoned() {
        let (_dir, ks) = open_temp_store();
        ks.store_key("provider", "key").unwrap();
        poison_mutex(&ks);
        let result = ks.remove_key("provider");
        assert!(
            result.is_err(),
            "remove_key should return Err on poisoned mutex"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("mutex poisoned") || err_msg.contains("poisoned"),
            "error should mention mutex poison, got: {err_msg}"
        );
    }

    #[test]
    fn key_hash_hex_produces_correct_output() {
        // SHA-256 of empty slice, first 4 bytes -> 8 hex chars
        let hash = key_hash_hex(b"");
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        // Known value: SHA-256("") starts with e3b0c442
        assert_eq!(hash, "e3b0c442");
    }

    /// Regression for the salt TOCTOU race: concurrent first-time opens
    /// must all converge on ONE salt. With the old exists()-then-write
    /// sequence, the loser kept its own in-memory salt while the winner's
    /// landed on disk — every key the loser encrypted became permanently
    /// undecryptable.
    #[test]
    fn concurrent_first_open_adopts_single_salt() {
        use std::collections::HashSet;
        use std::sync::Arc;
        use std::sync::Barrier;

        const RACERS: usize = 8;
        let dir = TempDir::new().unwrap();
        let config_dir = Arc::new(dir.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(RACERS));

        let handles: Vec<_> = (0..RACERS)
            .map(|_| {
                let config_dir = Arc::clone(&config_dir);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    load_or_create_salt(&config_dir)
                })
            })
            .collect();

        let salts: HashSet<Vec<u8>> = handles
            .into_iter()
            .map(|h| h.join().expect("racer thread"))
            .map(|r| r.expect("load_or_create_salt"))
            .collect();
        assert_eq!(salts.len(), 1, "all racers must adopt the same salt");
        // And the disk holds that same salt for the next open.
        let on_disk = load_or_create_salt(&config_dir).unwrap();
        assert!(salts.contains(&on_disk));
    }
}
