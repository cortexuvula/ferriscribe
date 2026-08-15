//! At-rest file encryption for patient artifacts (audio recordings,
//! orphaned transcripts) using AES-256-GCM.
//!
//! The key is the existing SQLCipher database key from the OS keychain —
//! reused via a single-pass domain-separated SHA-256 derivation so there
//! is one secret to manage and a fresh keychain round-trip isn't needed
//! per file. A random 12-byte nonce is generated per encryption; the
//! on-disk format is:
//!
//! ```text
//! [magic "FE1" (3 bytes)] [nonce (12 bytes)] [ciphertext (rest)]
//! ```
//!
//! The magic lets readers distinguish encrypted files from legacy plaintext
//! ones (e.g. WAVs written before this feature shipped) and decrypt only
//! when needed.
//!
//! # PHI context
//!
//! These functions handle the most sensitive artifacts (patient voice,
//! raw transcripts). The key never leaves the keychain; nonce/ciphertext
//! are the only on-disk artifacts. Decryption is in-memory and the plaintext
//! is never logged.

use std::path::Path;

use aes_gcm::aead::consts::U12;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::keychain;

/// On-disk magic prefix identifying an encrypted file: "FE1" (FerriScribe
/// Encrypted, version 1).
pub const MAGIC: &[u8; 3] = b"FE1";
/// AES-256-GCM nonce length (12 bytes).
const NONCE_LEN: usize = 12;

/// Errors returned by file encryption/decryption.
#[derive(Debug, thiserror::Error)]
pub enum FileCryptoError {
    #[error("keychain error: {0}")]
    Keychain(#[from] keychain::KeychainError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encryption failed: {0}")]
    Encrypt(String),
    #[error("decryption failed: {0}")]
    Decrypt(String),
    /// The file does not start with the encryption magic — it's plaintext
    /// (legacy) or not a FerriScribe-encrypted file.
    #[error("file is not encrypted (no magic prefix)")]
    NotEncrypted,
}

/// Derive a 32-byte AES-256-GCM key from the database key.
///
/// Reusing the DB key avoids a second keychain secret. A single-pass
/// SHA-256 of the root key concatenated with a domain-separation tag
/// produces a distinct, fixed-length key for file encryption. (This is a
/// KDF on a high-entropy 32-byte root key, so a single hash round is
/// sufficient — unlike a password, the input already has full entropy.)
fn derive_file_key(db_key: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(db_key);
    hasher.update(b"ferriescribe-file-v1");
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Obtain the AES-256-GCM cipher, deriving the key from the keychain DB key.
fn cipher() -> Result<Aes256Gcm, FileCryptoError> {
    let db_key = keychain::get_or_create_db_key()?;
    let file_key = derive_file_key(&db_key);
    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&file_key)))
}

/// Encrypt `plaintext` with a given derived file key (internal — used by
/// `encrypt_file` and by tests to avoid concurrent keychain access).
fn encrypt_with_key(cipher: &Aes256Gcm, plaintext: &[u8]) -> Result<Vec<u8>, FileCryptoError> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| FileCryptoError::Encrypt(e.to_string()))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Encrypt plaintext bytes in memory and return the ciphertext blob
/// (magic + nonce + ciphertext). Use when you have the plaintext already
/// in memory and want to write it without ever persisting plaintext to disk.
pub fn encrypt_bytes_in_memory(plaintext: &[u8]) -> Result<Vec<u8>, FileCryptoError> {
    let cipher = cipher()?;
    encrypt_with_key(&cipher, plaintext)
}

/// Encrypt bytes and write to `path` with the magic + nonce + ciphertext
/// format. **Atomic over in-place encryption**: writes to a sibling temp
/// file, fsyncs, then renames over the original. The original file is
/// never partially written — if encryption or I/O fails, the plaintext
/// source is left intact.
pub fn encrypt_file(path: &Path, plaintext: &[u8]) -> Result<(), FileCryptoError> {
    encrypt_file_with(path, &cipher()?, plaintext)
}

/// Keychain-free core of [`encrypt_file`] — the cipher is supplied by the
/// caller. Internal seam so tests can exercise the atomic-write mechanics
/// with a fixed key instead of the OS keychain (see the test module docs).
fn encrypt_file_with(
    path: &Path,
    cipher: &Aes256Gcm,
    plaintext: &[u8],
) -> Result<(), FileCryptoError> {
    let out = encrypt_with_key(cipher, plaintext)?;
    // Write to a temp sibling, fsync, then atomic rename. This avoids the
    // truncate-then-write race that could destroy the original on crash.
    let tmp_path = path.with_extension("enc.tmp");
    {
        let mut file = std::fs::File::create(&tmp_path)?;
        use std::io::Write;
        file.write_all(&out)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Read the plaintext file at `path`, encrypt it in place (atomic temp +
/// rename), and return. Use this when the caller already has a plaintext
/// file on disk (e.g. a freshly-captured WAV) and wants it encrypted.
///
/// Propagates read errors rather than defaulting to empty — a failed read
/// must NOT produce an empty encrypted file that destroys the original.
pub fn encrypt_file_in_place(path: &Path) -> Result<(), FileCryptoError> {
    encrypt_file_in_place_with(path, &cipher()?)
}

/// Keychain-free core of [`encrypt_file_in_place`].
fn encrypt_file_in_place_with(path: &Path, cipher: &Aes256Gcm) -> Result<(), FileCryptoError> {
    let plaintext = std::fs::read(path)?;
    encrypt_file_with(path, cipher, &plaintext)
}

/// Read `path` and decrypt. Returns the plaintext bytes.
///
/// Returns `NotEncrypted` if the file lacks the magic prefix (legacy
/// plaintext file) — callers decide whether to read it as-is or migrate.
pub fn decrypt_file(path: &Path) -> Result<Vec<u8>, FileCryptoError> {
    let bytes = std::fs::read(path)?;
    decrypt_bytes(&bytes)
}

/// Keychain-free core of [`decrypt_file`] — the cipher is supplied by the
/// caller. Internal seam for hermetic tests.
#[cfg(test)]
fn decrypt_file_with(path: &Path, cipher: &Aes256Gcm) -> Result<Vec<u8>, FileCryptoError> {
    let bytes = std::fs::read(path)?;
    decrypt_bytes_with(&bytes, cipher)
}

/// Decrypt an in-memory buffer (magic + nonce + ciphertext).
pub fn decrypt_bytes(bytes: &[u8]) -> Result<Vec<u8>, FileCryptoError> {
    // Validate the header before the keychain round-trip so malformed and
    // legacy-plaintext inputs are rejected even on machines with no
    // keychain available.
    split_header(bytes)?;
    decrypt_bytes_with(bytes, &cipher()?)
}

/// Keychain-free core of [`decrypt_bytes`].
fn decrypt_bytes_with(bytes: &[u8], cipher: &Aes256Gcm) -> Result<Vec<u8>, FileCryptoError> {
    let (nonce, ciphertext) = split_header(bytes)?;
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| FileCryptoError::Decrypt(e.to_string()))
}

/// Validate the magic + nonce header and split it from the ciphertext.
fn split_header(bytes: &[u8]) -> Result<(&Nonce<U12>, &[u8]), FileCryptoError> {
    let header = MAGIC.len() + NONCE_LEN;
    if bytes.len() < header {
        return Err(FileCryptoError::Decrypt("file too short for header".into()));
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(FileCryptoError::NotEncrypted);
    }
    let nonce = Nonce::from_slice(&bytes[MAGIC.len()..header]);
    Ok((nonce, &bytes[header..]))
}

/// Returns true if the file at `path` begins with the encryption magic.
///
/// Useful for deciding whether to decrypt a legacy file or read it as
/// plaintext during a migration. Reads only the first 3 bytes (the magic
/// prefix) rather than slurping the entire file — important for large
/// audio recordings checked during the startup encryption sweep.
pub fn is_encrypted(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 3];
    let ok = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    ok && &buf == MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A deterministic cipher for exercising the file-mechanics paths.
    ///
    /// The public wrappers source their key from the real OS keychain via
    /// [`cipher`], which is untestable in-process: the keyring mock
    /// installed by the `keychain` tests is process-global and EntryOnly
    /// (a fresh empty credential per `Entry::new`), so depending on test
    /// ordering the encrypt and decrypt halves of a roundtrip can end up
    /// with different keys and fail spuriously. The `_with` seams take the
    /// cipher as a parameter, so these tests are hermetic and deterministic
    /// on every machine.
    fn fixed_test_cipher() -> Aes256Gcm {
        let raw_key = [42u8; 32];
        let file_key = derive_file_key(&raw_key);
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&file_key))
    }

    #[test]
    fn encrypt_then_decrypt_roundtrips() {
        let cipher = fixed_test_cipher();

        let original = b"patient audio bytes - sensitive PHI";
        let encrypted = encrypt_with_key(&cipher, original).expect("encryption must succeed");
        assert!(
            encrypted.starts_with(MAGIC),
            "encrypted output has magic prefix"
        );

        // Decrypt with the same key — simulate decrypt_bytes but with our
        // fixed cipher rather than the keychain-sourced one.
        let header = MAGIC.len() + NONCE_LEN;
        assert!(encrypted.len() > header);
        assert_eq!(&encrypted[..MAGIC.len()], MAGIC);
        let nonce = Nonce::from_slice(&encrypted[MAGIC.len()..header]);
        let ciphertext = &encrypted[header..];
        let decrypted = cipher
            .decrypt(nonce, ciphertext)
            .expect("decryption must roundtrip");
        assert_eq!(decrypted, original);
    }

    #[test]
    fn decrypt_bytes_rejects_truncated_header() {
        // Input shorter than the magic+nonce header must return a Decrypt
        // error, not panic on slicing.
        let truncated = b"FE1";
        let result = decrypt_bytes(truncated);
        assert!(
            matches!(result, Err(FileCryptoError::Decrypt(_))),
            "truncated header should error, got {result:?}"
        );
    }

    #[test]
    fn decrypt_bytes_rejects_wrong_magic() {
        // Input with a non-matching prefix should return NotEncrypted,
        // not attempt decryption.
        let not_ours = b"RIFF\x00\x00\x00\x00WAVEfmt ";
        let result = decrypt_bytes(not_ours);
        assert!(
            matches!(result, Err(FileCryptoError::NotEncrypted)),
            "non-FE1 prefix should return NotEncrypted, got {result:?}"
        );
    }

    #[test]
    fn decrypt_plain_file_returns_not_encrypted() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("plain.wav");
        std::fs::write(&path, b"RIFF....plaintext wave data").unwrap();
        let result = decrypt_file(&path);
        assert!(matches!(result, Err(FileCryptoError::NotEncrypted)));
        assert!(!is_encrypted(&path));
    }

    #[test]
    fn magic_is_distinct_and_short() {
        // 3 bytes — minimal overhead, not a common file header.
        assert_eq!(MAGIC.len(), 3);
        assert_ne!(MAGIC.as_slice(), b"RIFF"); // not confused with WAV
        assert_ne!(MAGIC.as_slice(), b"ID3"); // not confused with MP3
    }

    // ---- encrypt_file_in_place (the production PHI-at-rest path) ----

    #[test]
    fn encrypt_file_in_place_roundtrips() {
        // This is the exact path called by audio capture + import:
        // read a plaintext file, encrypt it atomically (temp+rename),
        // then decrypt to verify the round-trip. Driven with a fixed
        // cipher (see `fixed_test_cipher`) so it never touches the
        // keychain.
        let cipher = fixed_test_cipher();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("recording.wav");
        let original = b"RIFF\x00\x00\x00\x00WAVEfmt fake-audio-data-PHI";
        std::fs::write(&path, original).unwrap();

        encrypt_file_in_place_with(&path, &cipher).expect("in-place encryption");

        // The file should now have the FE1 magic (encrypted).
        assert!(
            is_encrypted(&path),
            "file should be encrypted after in-place"
        );
        // Decrypting should recover the original plaintext.
        let decrypted = decrypt_file_with(&path, &cipher).expect("decryption must roundtrip");
        assert_eq!(decrypted, original);
    }

    #[test]
    fn encrypt_file_in_place_preserves_original_on_read_failure() {
        // encrypt_file_in_place reads the file first. If the read fails
        // (e.g. file doesn't exist), the original must NOT be destroyed —
        // the function should return an error and leave nothing behind.
        let cipher = fixed_test_cipher();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.wav");
        let result = encrypt_file_in_place_with(&path, &cipher);
        assert!(
            result.is_err(),
            "encrypting a nonexistent file should error"
        );
        // No temp file should be left behind.
        assert!(
            !dir.path().join("nonexistent.wav.enc.tmp").exists(),
            "temp file should be cleaned up on failure"
        );
    }
}
