//! Backup wrapping key: generation, keychain persistence, and the
//! domain-separated derivations used by snapshots and escrow artifacts.
//!
//! The wrapping key is an independent 32-byte root secret — NOT derived
//! from the DB key — so a compromised or wiped machine #1 keychain cannot
//! decrypt snapshots, and the escrowed wrapping key alone (recovery sheet
//! or USB) can. Derivations reuse the single-pass SHA-256 + domain-tag
//! pattern from `medical_security::file_crypto::derive_file_key` (valid
//! because the input is a full-entropy 32-byte key, not a password).

use sha2::{Digest, Sha256};

use medical_security::keychain;

use crate::BackupResult;

/// Domain tag for the AES key that encrypts snapshot payloads (manifest,
/// wrapped DB key).
const DOMAIN_SNAPSHOT_AES: &[u8] = b"ferriescribe-backup-snapshot-v1";
/// Domain tag for the HMAC key that authenticates snapshot receipts.
const DOMAIN_SNAPSHOT_HMAC: &[u8] = b"ferriescribe-backup-hmac-v1";
/// Canary message HMAC'd under the HMAC key and embedded in every escrow
/// artifact, so an artifact can be verified WITHOUT any other secret
/// (R1: "each independently verifiable").
const ESCROW_CANARY_MESSAGE: &[u8] = b"ferriescribe-backup-escrow-verify-v1";

/// Generate a fresh random 32-byte wrapping key.
pub fn generate_wrapping_key() -> [u8; 32] {
    use rand::RngCore;
    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);
    key
}

/// Load the wrapping key from the OS keychain, or generate and store one
/// on first use. The keychain copy exists for unattended backups and
/// drills; the off-machine escrow copies (sheet + USB) are what survive
/// machine #1's death.
pub fn load_or_create_wrapping_key() -> BackupResult<[u8; 32]> {
    Ok(keychain::get_or_create_secret(
        keychain::KEYCHAIN_BACKUP_KEY_ACCOUNT,
    )?)
}

/// Load the wrapping key from the OS keychain, erroring if absent
/// (restore-on-clean-machine paths take an escrow artifact instead).
pub fn load_wrapping_key() -> BackupResult<[u8; 32]> {
    keychain::get_secret(keychain::KEYCHAIN_BACKUP_KEY_ACCOUNT)?.ok_or_else(|| {
        crate::BackupError::MissingKey(
            "no backup wrapping key in keychain — provide an escrow artifact".into(),
        )
    })
}

/// AES-256 key for snapshot payload encryption, domain-separated from the
/// wrapping key (and from the HMAC key) by a single SHA-256 pass.
pub fn snapshot_aes_key(wrapping: &[u8; 32]) -> [u8; 32] {
    derive(wrapping, DOMAIN_SNAPSHOT_AES)
}

/// HMAC-SHA256 key for snapshot receipt authentication.
pub fn snapshot_hmac_key(wrapping: &[u8; 32]) -> [u8; 32] {
    derive(wrapping, DOMAIN_SNAPSHOT_HMAC)
}

/// Tag embedded in escrow artifacts: HMAC(hmac_key, canary message).
/// Recovering the key from any artifact is accepted only when the key
/// reproduces this tag, which simultaneously validates the transcription
/// (sheet) and the bytes (USB).
pub fn escrow_canary_tag(wrapping: &[u8; 32]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<Sha256>::new_from_slice(&snapshot_hmac_key(wrapping))
        .expect("32-byte HMAC key is always accepted");
    mac.update(ESCROW_CANARY_MESSAGE);
    let mut out = [0u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

fn derive(root: &[u8; 32], domain: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(root);
    hasher.update(domain);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivations_are_deterministic_and_domain_separated() {
        let wrapping = [5u8; 32];
        let aes = snapshot_aes_key(&wrapping);
        let hmac_key = snapshot_hmac_key(&wrapping);
        assert_eq!(aes, snapshot_aes_key(&wrapping), "deterministic");
        assert_ne!(aes, hmac_key, "domains must not collide");
        assert_ne!(aes, wrapping, "derived != root");
        // Distinct wrapping keys derive distinct keys.
        assert_ne!(aes, snapshot_aes_key(&[6u8; 32]));
    }

    #[test]
    fn escrow_canary_is_deterministic_and_key_sensitive() {
        let t1 = escrow_canary_tag(&[1u8; 32]);
        let t2 = escrow_canary_tag(&[1u8; 32]);
        let t3 = escrow_canary_tag(&[2u8; 32]);
        assert_eq!(t1, t2);
        assert_ne!(t1, t3);
    }

    #[test]
    fn generated_keys_are_random() {
        let a = generate_wrapping_key();
        let b = generate_wrapping_key();
        assert_ne!(a, b, "two generated wrapping keys must differ");
    }
}
