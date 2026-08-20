//! Snapshot construction, verification (R5), and restore (R6).
//!
//! Layout of a snapshot directory:
//!
//! ```text
//! snap-<UTC timestamp>-<6 hex rand>/
//!   receipt.json        — plaintext, NO PHI: id, version, created_at,
//!                         counts, byte totals, HMAC tag
//!   manifest.json.enc   — FE1-encrypted manifest (paths + hashes)
//!   payload/
//!     f000000.bin       — VACUUM INTO copy of medical.db (SQLCipher)
//!     f000001.bin …     — recording ciphertext files (copied verbatim;
//!                         already FE1-encrypted under the DB-derived key)
//!     fDBKEY00.bin      — the DB key, FE1-encrypted under the snapshot
//!                         key (R1: escrow + snapshot = recoverable)
//! ```
//!
//! Integrity model: the receipt's HMAC tag covers the receipt's own
//! canonical fields plus every payload hash in manifest order. Any byte
//! changed anywhere (payload, manifest, or receipt) fails verification.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use medical_security::file_crypto;

use crate::keys;
use crate::{BackupError, BackupResult};

/// Snapshot format version (bump on breaking layout changes). v2: the
/// receipt HMAC now covers `relative_path` + `encrypted` per manifest
/// entry (v1 covered only name+hash) — v1 snapshots are refused with an
/// explicit "unsupported version" error rather than an HMAC mismatch.
pub const SNAPSHOT_VERSION: u32 = 2;
/// Plaintext receipt filename (the only file without the payload naming).
pub const RECEIPT_FILE: &str = "receipt.json";
/// Encrypted manifest filename.
pub const MANIFEST_FILE: &str = "manifest.json.enc";
/// Payload subdirectory.
pub const PAYLOAD_DIR: &str = "payload";
/// Manifest-relative path of the wrapped DB key entry.
pub const DB_KEY_ENTRY_PATH: &str = "backup/db-key.bin";

/// One payload file: its opaque on-disk name, its original relative path
/// (PHI-sensitive — lives only inside the encrypted manifest), the SHA-256
/// of its ciphertext bytes, and whether it is FE1-encrypted under the
/// snapshot key (`true`) or copied ciphertext as-is (`false`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub opaque_name: String,
    pub relative_path: String,
    pub sha256: String,
    pub encrypted: bool,
}

/// The encrypted manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub snapshot_id: String,
    pub created_at: DateTime<Utc>,
    pub entries: Vec<ManifestEntry>,
}

/// The plaintext receipt — counts, sizes, and the HMAC tag only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotReceipt {
    pub snapshot_id: String,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub file_count: u64,
    pub total_bytes: u64,
    pub recording_count: u64,
    pub hmac_tag: String,
}

/// Inputs to [`build_snapshot`].
pub struct BuildOptions {
    /// Path to the live `medical.db` (SQLCipher).
    pub db_path: PathBuf,
    /// Directory holding the (already-encrypted) recording files.
    pub recordings_dir: PathBuf,
    /// Optional keystore file (e.g. `config/keys.json`), copied verbatim.
    pub keystore_path: Option<PathBuf>,
    /// Destination directory; the snapshot lands in `<dest>/<snapshot-id>`.
    pub dest_dir: PathBuf,
    /// The live SQLCipher key (wrapped into the snapshot).
    pub db_key: [u8; 32],
    /// The escrowed backup wrapping key.
    pub wrapping_key: [u8; 32],
}

/// What [`verify_snapshot`] checked, for drill reports and UI.
#[derive(Debug, Clone)]
pub struct SnapshotSummary {
    pub receipt: SnapshotReceipt,
    pub files_checked: u64,
    pub total_bytes: u64,
}

/// What [`restore_snapshot`] recovered.
#[derive(Clone)]
pub struct RestoreReport {
    pub snapshot_id: String,
    pub files_restored: u64,
    pub recording_files: u64,
    pub db_key_recovered: bool,
    /// The recovered SQLCipher key — the secret that opens the restored
    /// database. Never logged or printed: `Debug` redacts it (same
    /// discipline as `RecordingSummary::patient_name`).
    pub db_key: [u8; 32],
    pub key_install: KeyInstallOutcome,
}

/// Manual Debug that redacts key material.
impl std::fmt::Debug for RestoreReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoreReport")
            .field("snapshot_id", &self.snapshot_id)
            .field("files_restored", &self.files_restored)
            .field("recording_files", &self.recording_files)
            .field("db_key_recovered", &self.db_key_recovered)
            .field("db_key", &"<redacted>")
            .field("key_install", &self.key_install)
            .finish()
    }
}

/// Whether `restore_snapshot` may write the recovered DB key into this
/// machine's OS keychain (R6: a restored database must actually open).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInstall {
    /// Never touch the keychain — used by drills restoring into scratch
    /// dirs on the LIVE machine, where clobbering the real key would be
    /// catastrophic.
    Skip,
    /// Install only when the keychain has no key or the identical key
    /// (the default). A differing existing key refuses, so restoring an
    /// old snapshot cannot silently lock out the current database.
    IfAbsentOrEqual,
    /// Overwrite even a differing existing key (explicit `--force`).
    Overwrite,
}

/// Result of the key-installation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInstallOutcome {
    /// Not requested (`KeyInstall::Skip`) or nothing to install.
    Skipped,
    /// Key written to the keychain.
    Installed,
    /// Keychain already held the identical key — nothing to do.
    AlreadyPresent,
    /// Keychain holds a DIFFERENT key and the mode forbids overwriting.
    /// The files are restored, but the caller must resolve the conflict.
    RefusedExistingKeyDiffers,
}

/// Pure decision core for the guarded install (unit-tested without the
/// OS keychain; the EntryOnly keyring mock cannot exercise cross-call
/// persistence).
fn key_install_decision(
    existing: Option<[u8; 32]>,
    recovered: [u8; 32],
    mode: KeyInstall,
) -> KeyInstallOutcome {
    match mode {
        KeyInstall::Skip => KeyInstallOutcome::Skipped,
        KeyInstall::Overwrite => {
            if existing == Some(recovered) {
                KeyInstallOutcome::AlreadyPresent
            } else {
                KeyInstallOutcome::Installed
            }
        }
        KeyInstall::IfAbsentOrEqual => match existing {
            None => KeyInstallOutcome::Installed,
            Some(k) if k == recovered => KeyInstallOutcome::AlreadyPresent,
            Some(_) => KeyInstallOutcome::RefusedExistingKeyDiffers,
        },
    }
}

/// Build a new snapshot under `opts.dest_dir` and return its receipt.
pub fn build_snapshot(opts: &BuildOptions) -> BackupResult<SnapshotReceipt> {
    let aes_key = keys::snapshot_aes_key(&opts.wrapping_key);
    let snapshot_id = new_snapshot_id();
    let snap_dir = opts.dest_dir.join(&snapshot_id);
    let payload_dir = snap_dir.join(PAYLOAD_DIR);
    fs::create_dir_all(&payload_dir)?;

    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut total_bytes: u64 = 0;

    // 1. Consistent DB copy (VACUUM INTO — safe while the app is open).
    let db_copy = payload_dir.join("f000000.bin");
    medical_db::snapshot_db_to(&opts.db_path, opts.db_key, &db_copy)?;
    total_bytes += push_entry(&mut entries, &db_copy, "medical.db", false)?;

    // 2. Recording files (already encrypted at rest; copied verbatim).
    let mut recording_files: u64 = 0;
    if opts.recordings_dir.is_dir() {
        let mut paths: Vec<PathBuf> = fs::read_dir(&opts.recordings_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();
        paths.sort();
        for path in paths {
            let name = format!("f{:06}.bin", entries.len() + 1);
            let dest = payload_dir.join(&name);
            fs::copy(&path, &dest)?;
            let rel = format!(
                "recordings/{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.clone())
            );
            total_bytes += push_entry(&mut entries, &dest, &rel, false)?;
            recording_files += 1;
        }
    }

    // 3. Keystore (optional) — copied verbatim (encrypted at rest already).
    if let Some(ks) = &opts.keystore_path
        && ks.is_file()
    {
        let name = format!("f{:06}.bin", entries.len() + 1);
        let dest = payload_dir.join(&name);
        fs::copy(ks, &dest)?;
        total_bytes += push_entry(&mut entries, &dest, "config/keys.json", false)?;
    }

    // 4. Wrap the DB key under the snapshot key (R1).
    let wrapped = file_crypto::encrypt_bytes_with_key(&aes_key, &opts.db_key)?;
    let key_name = format!("f{:06}.bin", entries.len() + 1);
    let key_dest = payload_dir.join(&key_name);
    fs::write(&key_dest, &wrapped)?;
    total_bytes += push_entry(&mut entries, &key_dest, DB_KEY_ENTRY_PATH, true)?;

    // 5. Record count from the live DB (the drill diffs against this).
    let recording_count = count_recordings(&opts.db_path, opts.db_key)?;

    // 6. Encrypted manifest (holds the PHI-sensitive relative paths).
    let created_at = Utc::now();
    let manifest = SnapshotManifest {
        snapshot_id: snapshot_id.clone(),
        created_at,
        entries: entries.clone(),
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let manifest_enc = file_crypto::encrypt_bytes_with_key(&aes_key, &manifest_bytes)?;
    fs::write(snap_dir.join(MANIFEST_FILE), &manifest_enc)?;

    // 7. Receipt with the HMAC over receipt fields + every payload hash.
    let tag = compute_tag(
        &opts.wrapping_key,
        &snapshot_id,
        SNAPSHOT_VERSION,
        &created_at,
        recording_count,
        &entries,
    );
    let receipt = SnapshotReceipt {
        snapshot_id,
        version: SNAPSHOT_VERSION,
        created_at,
        file_count: entries.len() as u64,
        total_bytes,
        recording_count,
        hmac_tag: tag,
    };
    fs::write(
        snap_dir.join(RECEIPT_FILE),
        serde_json::to_vec_pretty(&receipt)?,
    )?;

    tracing::info!(
        snapshot_id = %receipt.snapshot_id,
        files = receipt.file_count,
        recording_files = recording_files,
        recording_rows = recording_count,
        total_bytes = total_bytes,
        "snapshot built"
    );
    Ok(receipt)
}

/// Verify a snapshot directory end-to-end: receipt parses, manifest
/// decrypts, every payload file exists with the recorded SHA-256, no
/// unlisted payload files, counts/bytes agree, and the HMAC tag
/// reproduces. Fails closed on any mismatch (R5).
pub fn verify_snapshot(dir: &Path, wrapping_key: &[u8; 32]) -> BackupResult<SnapshotSummary> {
    let receipt: SnapshotReceipt = read_json(&dir.join(RECEIPT_FILE))?;
    if receipt.version != SNAPSHOT_VERSION {
        return Err(BackupError::Format(format!(
            "unsupported snapshot version {} (expected {SNAPSHOT_VERSION})",
            receipt.version
        )));
    }
    let manifest = load_manifest(dir, wrapping_key)?;
    if manifest.snapshot_id != receipt.snapshot_id {
        return Err(BackupError::Verification(
            "manifest snapshot id does not match receipt".into(),
        ));
    }

    let payload_dir = dir.join(PAYLOAD_DIR);
    let mut listed: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 0;
    for entry in &manifest.entries {
        let path = payload_dir.join(&entry.opaque_name);
        let bytes = fs::read(&path).map_err(|_| {
            BackupError::Verification(format!("payload file missing: {}", entry.opaque_name))
        })?;
        let sha = sha256_hex(&bytes);
        if sha != entry.sha256 {
            return Err(BackupError::Verification(format!(
                "payload hash mismatch on {} — corrupted or tampered",
                entry.opaque_name
            )));
        }
        total_bytes += bytes.len() as u64;
        listed.push(entry.opaque_name.clone());
    }

    // No unlisted payload files (an attacker must not be able to smuggle
    // content past the manifest).
    let mut on_disk: Vec<String> = fs::read_dir(&payload_dir)?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    on_disk.sort();
    let mut listed_sorted = listed.clone();
    listed_sorted.sort();
    if on_disk != listed_sorted {
        return Err(BackupError::Verification(
            "payload directory contains unlisted files".into(),
        ));
    }

    if receipt.file_count != manifest.entries.len() as u64 {
        return Err(BackupError::Verification("file count mismatch".into()));
    }
    if receipt.total_bytes != total_bytes {
        return Err(BackupError::Verification("total bytes mismatch".into()));
    }

    let tag = compute_tag(
        wrapping_key,
        &receipt.snapshot_id,
        receipt.version,
        &receipt.created_at,
        receipt.recording_count,
        &manifest.entries,
    );
    if !tag.eq_ignore_ascii_case(&receipt.hmac_tag) {
        return Err(BackupError::Verification(
            "HMAC tag mismatch — snapshot corrupted or tampered".into(),
        ));
    }

    Ok(SnapshotSummary {
        receipt,
        files_checked: manifest.entries.len() as u64,
        total_bytes,
    })
}

/// Restore a verified snapshot into `dest_data_dir`, reconstructing the
/// original layout (medical.db, recordings/, config/) and recovering the
/// DB key. Depending on `key_install`, the recovered key is written to
/// this machine's keychain so the restored database actually opens (R6).
/// Returns the report; logs counts only (no PHI, no key material).
pub fn restore_snapshot(
    dir: &Path,
    wrapping_key: &[u8; 32],
    dest_data_dir: &Path,
    key_install: KeyInstall,
) -> BackupResult<RestoreReport> {
    // Fail closed: never write anything from an unverified snapshot.
    let _summary = verify_snapshot(dir, wrapping_key)?;
    let manifest = load_manifest(dir, wrapping_key)?;
    let aes_key = keys::snapshot_aes_key(wrapping_key);
    let payload_dir = dir.join(PAYLOAD_DIR);

    let mut files_restored: u64 = 0;
    let mut recording_files: u64 = 0;
    let mut db_key: Option<[u8; 32]> = None;

    for entry in &manifest.entries {
        if entry.relative_path == DB_KEY_ENTRY_PATH {
            let blob = fs::read(payload_dir.join(&entry.opaque_name))?;
            let plain = file_crypto::decrypt_bytes_with_key(&aes_key, &blob)?;
            let mut key = [0u8; 32];
            key.copy_from_slice(&plain);
            db_key = Some(key);
            continue;
        }
        let dest = safe_join(dest_data_dir, &entry.relative_path)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(payload_dir.join(&entry.opaque_name), &dest)?;
        files_restored += 1;
        if entry.relative_path.starts_with("recordings/") {
            recording_files += 1;
        }
    }

    // R6: persist the recovered key so the restored DB opens on this
    // machine. Guarded — never clobber a differing live key without force.
    let key_install_outcome = match db_key {
        None => KeyInstallOutcome::Skipped,
        Some(recovered) => {
            let existing = medical_security::keychain::get_secret(
                medical_security::keychain::KEYCHAIN_DB_KEY_ACCOUNT,
            )?;
            match key_install_decision(existing, recovered, key_install) {
                KeyInstallOutcome::Installed => {
                    medical_security::keychain::set_secret(
                        medical_security::keychain::KEYCHAIN_DB_KEY_ACCOUNT,
                        recovered,
                    )?;
                    KeyInstallOutcome::Installed
                }
                other => other,
            }
        }
    };

    tracing::info!(
        snapshot_id = %manifest.snapshot_id,
        files_restored,
        recording_files,
        db_key_recovered = db_key.is_some(),
        key_install = ?key_install_outcome,
        "snapshot restored"
    );
    Ok(RestoreReport {
        snapshot_id: manifest.snapshot_id,
        files_restored,
        recording_files,
        db_key_recovered: db_key.is_some(),
        db_key: db_key.unwrap_or([0u8; 32]),
        key_install: key_install_outcome,
    })
}

/// Decrypt + parse the manifest (shared by verify/restore).
fn load_manifest(dir: &Path, wrapping_key: &[u8; 32]) -> BackupResult<SnapshotManifest> {
    let blob = fs::read(dir.join(MANIFEST_FILE))?;
    let aes_key = keys::snapshot_aes_key(wrapping_key);
    let plain = file_crypto::decrypt_bytes_with_key(&aes_key, &blob)
        .map_err(|e| BackupError::Verification(format!("manifest decrypt failed: {e}")))?;
    serde_json::from_slice(&plain)
        .map_err(|e| BackupError::Format(format!("manifest parse failed: {e}")))
}

/// Recover the wrapped DB key from a snapshot (used by the drill to open
/// the restored database). Verifies the snapshot first.
pub fn recover_db_key(dir: &Path, wrapping_key: &[u8; 32]) -> BackupResult<[u8; 32]> {
    verify_snapshot(dir, wrapping_key)?;
    let manifest = load_manifest(dir, wrapping_key)?;
    let entry = manifest
        .entries
        .iter()
        .find(|e| e.relative_path == DB_KEY_ENTRY_PATH)
        .ok_or_else(|| BackupError::Format("snapshot has no wrapped DB key".into()))?;
    let blob = fs::read(dir.join(PAYLOAD_DIR).join(&entry.opaque_name))?;
    let aes_key = keys::snapshot_aes_key(wrapping_key);
    let plain = file_crypto::decrypt_bytes_with_key(&aes_key, &blob)?;
    let mut db_key = [0u8; 32];
    db_key.copy_from_slice(&plain);
    Ok(db_key)
}

/// HMAC over the receipt's canonical fields plus every payload hash, in
/// manifest order. Keyed with the wrapping-key-derived HMAC key (R5).
fn compute_tag(
    wrapping_key: &[u8; 32],
    snapshot_id: &str,
    version: u32,
    created_at: &DateTime<Utc>,
    recording_count: u64,
    entries: &[ManifestEntry],
) -> String {
    // Length-prefixed canonical buffer: no delimiter can be forged by a
    // value containing it (recording filenames may legally contain `|`
    // and `:`). Every restore-affecting field is covered (finding 4).
    fn put(buf: &mut Vec<u8>, bytes: &[u8]) {
        buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        buf.extend_from_slice(bytes);
    }
    let mut buf = Vec::new();
    buf.extend_from_slice(b"FBTAG1");
    put(&mut buf, version.to_string().as_bytes());
    put(&mut buf, snapshot_id.as_bytes());
    put(&mut buf, created_at.to_rfc3339().as_bytes());
    buf.extend_from_slice(&recording_count.to_le_bytes());
    for entry in entries {
        put(&mut buf, entry.opaque_name.as_bytes());
        put(&mut buf, entry.sha256.as_bytes());
        put(&mut buf, entry.relative_path.as_bytes());
        buf.push(u8::from(entry.encrypted));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(&keys::snapshot_hmac_key(wrapping_key))
        .expect("32-byte HMAC key is always accepted");
    mac.update(&buf);
    hex::encode(mac.finalize().into_bytes())
}

/// Local retention (finding 6): remove the OLDEST local snapshot dirs
/// beyond `keep`, judged by receipt `created_at`. Purely local
/// housekeeping — the target's admin credential is never involved.
/// Dirs without a parseable receipt are left alone (never delete what
/// can't be dated). Returns the removed snapshot ids.
pub fn prune_local_snapshots(backups_dir: &Path, keep: usize) -> Vec<String> {
    let mut dated: Vec<(chrono::DateTime<Utc>, PathBuf, String)> = Vec::new();
    let Ok(rd) = fs::read_dir(backups_dir) else {
        return Vec::new();
    };
    for entry in rd.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(bytes) = fs::read(dir.join(RECEIPT_FILE)) else {
            continue;
        };
        let Ok(receipt) = serde_json::from_slice::<SnapshotReceipt>(&bytes) else {
            continue;
        };
        dated.push((receipt.created_at, dir, receipt.snapshot_id));
    }
    dated.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    let mut removed = Vec::new();
    for (_, dir, id) in dated.into_iter().skip(keep) {
        if fs::remove_dir_all(&dir).is_ok() {
            removed.push(id);
        }
    }
    removed
}

/// Opaque snapshot id: UTC timestamp + random hex. No hostnames, no
/// patient-identifying material (PHI rule).
fn new_snapshot_id() -> String {
    use rand::RngCore;
    let mut rand_bytes = [0u8; 3];
    rand::rng().fill_bytes(&mut rand_bytes);
    format!(
        "snap-{}-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        hex::encode(rand_bytes)
    )
}

/// NOTE (finding 7): whole files are read into memory to hash them. For
/// very large recording libraries (tens of GB) run the backup on a
/// machine with ample RAM; a streaming hash/transfer refactor must
/// preserve the HMAC-over-full-bytes model — do not half-fix it.
fn push_entry(
    entries: &mut Vec<ManifestEntry>,
    path: &Path,
    relative_path: &str,
    encrypted: bool,
) -> BackupResult<u64> {
    let bytes = fs::read(path)?;
    let len = bytes.len() as u64;
    entries.push(ManifestEntry {
        opaque_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        relative_path: relative_path.to_string(),
        sha256: sha256_hex(&bytes),
        encrypted,
    });
    Ok(len)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn count_recordings(db_path: &Path, db_key: [u8; 32]) -> BackupResult<u64> {
    let conn = rusqlite::Connection::open(db_path)?;
    medical_db::encryption::apply_pragma_key(&conn, &db_key)?;
    let count: i64 = conn.query_row("SELECT count(*) FROM recordings", [], |r| r.get(0))?;
    Ok(count as u64)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> BackupResult<T> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

/// Join `dest` with a manifest-relative path, rejecting traversal — a
/// tampered manifest must never be able to write outside the destination.
fn safe_join(dest: &Path, relative: &str) -> BackupResult<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute() || rel.components().any(|c| c.as_os_str() == "..") {
        return Err(BackupError::Format(format!(
            "manifest path escapes destination: {relative}"
        )));
    }
    Ok(dest.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::recording::Recording;
    use medical_db::recordings::RecordingsRepo;

    /// Build a realistic source: SQLCipher DB with N recording rows plus a
    /// recordings dir holding an FE1-encrypted fake WAV.
    fn fixture_source(dir: &Path, db_key: [u8; 32]) -> (PathBuf, PathBuf) {
        let db_path = dir.join("medical.db");
        let database = medical_db::Database::open(&db_path, Some(db_key)).expect("open db");
        {
            let conn = database.conn().expect("conn");
            for i in 0..3 {
                let rec =
                    Recording::new(format!("visit-{i}.enc"), dir.join(format!("visit-{i}.enc")));
                RecordingsRepo::insert(&conn, &rec).expect("insert");
            }
        }
        drop(database);

        let recordings = dir.join("recordings");
        fs::create_dir_all(&recordings).expect("mkdir");
        let wav_key = file_crypto::derive_file_key(&db_key);
        let blob = file_crypto::encrypt_bytes_with_key(&wav_key, b"RIFF fake-wav patient audio")
            .expect("encrypt wav");
        fs::write(recordings.join("visit-0.enc"), &blob).expect("write wav");
        (db_path, recordings)
    }

    fn fixture_opts(
        src: &Path,
        db_key: [u8; 32],
        wrapping: [u8; 32],
    ) -> (tempfile::TempDir, BuildOptions) {
        let dest = tempfile::tempdir().expect("dest");
        let (db_path, recordings) = fixture_source(src, db_key);
        let opts = BuildOptions {
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            dest_dir: dest.path().to_path_buf(),
            db_key,
            wrapping_key: wrapping,
        };
        (dest, opts)
    }

    fn snapshot_dir(dest: &Path, receipt: &SnapshotReceipt) -> PathBuf {
        dest.join(&receipt.snapshot_id)
    }

    #[test]
    fn build_verify_restore_roundtrip() {
        let src = tempfile::tempdir().expect("src");
        let db_key = [0xA1u8; 32];
        let wrapping = [0xB2u8; 32];
        let (dest, opts) = fixture_opts(src.path(), db_key, wrapping);

        let receipt = build_snapshot(&opts).expect("build");
        assert_eq!(receipt.version, SNAPSHOT_VERSION);
        assert_eq!(receipt.recording_count, 3, "3 fixture rows");
        assert!(receipt.snapshot_id.starts_with("snap-"));
        // receipt.json must not contain any recording filenames (PHI).
        let receipt_text =
            fs::read_to_string(snapshot_dir(dest.path(), &receipt).join(RECEIPT_FILE)).unwrap();
        assert!(!receipt_text.contains("visit-"), "no filenames in receipt");

        let summary =
            verify_snapshot(&snapshot_dir(dest.path(), &receipt), &wrapping).expect("verify");
        assert_eq!(summary.receipt.snapshot_id, receipt.snapshot_id);

        let restore_dest = tempfile::tempdir().expect("restore");
        let report = restore_snapshot(
            &snapshot_dir(dest.path(), &receipt),
            &wrapping,
            restore_dest.path(),
            // Keychain-free test context: skip the install, prove the key
            // still reaches the caller via the report.
            KeyInstall::Skip,
        )
        .expect("restore");
        assert!(report.db_key_recovered);
        assert_eq!(report.key_install, KeyInstallOutcome::Skipped);
        assert!(report.debug_string_has_no_key()); // see helper below
        assert_eq!(report.recording_files, 1);
        assert!(restore_dest.path().join("medical.db").exists());
        assert!(restore_dest.path().join("recordings/visit-0.enc").exists());

        // The restored DB opens with the key returned in the report —
        // the exact credential a clean-machine restore would install.
        let reopened = medical_db::Database::open(
            &restore_dest.path().join("medical.db"),
            Some(report.db_key),
        );
        assert!(reopened.is_ok(), "restored DB must open with recovered key");
    }

    #[test]
    fn key_install_decision_guards_overwrite() {
        use super::{KeyInstall, KeyInstallOutcome as O, key_install_decision};
        let a = [1u8; 32];
        let b = [2u8; 32];
        // Skip never installs.
        assert_eq!(key_install_decision(None, a, KeyInstall::Skip), O::Skipped);
        assert_eq!(
            key_install_decision(Some(b), a, KeyInstall::Skip),
            O::Skipped
        );
        // Default mode: install when absent, no-op when equal, refuse when
        // a DIFFERENT live key exists (restoring an old snapshot must not
        // lock out the current database).
        assert_eq!(
            key_install_decision(None, a, KeyInstall::IfAbsentOrEqual),
            O::Installed
        );
        assert_eq!(
            key_install_decision(Some(a), a, KeyInstall::IfAbsentOrEqual),
            O::AlreadyPresent
        );
        assert_eq!(
            key_install_decision(Some(b), a, KeyInstall::IfAbsentOrEqual),
            O::RefusedExistingKeyDiffers
        );
        // Force overwrites a differing key.
        assert_eq!(
            key_install_decision(Some(b), a, KeyInstall::Overwrite),
            O::Installed
        );
        assert_eq!(
            key_install_decision(Some(a), a, KeyInstall::Overwrite),
            O::AlreadyPresent
        );
    }

    #[test]
    fn restore_installs_recovered_key_when_keychain_absent() {
        // Restore on a machine with no db-key entry installs the snapshot's
        // key (R6). Uses the EntryOnly keyring mock — cross-call persistence
        // is not observable, so we assert the decision outcome + the
        // returned key, per the keychain module's documented test limits.
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());

        let src = tempfile::tempdir().expect("src");
        let db_key = [0x71u8; 32];
        let wrapping = [0x82u8; 32];
        let (dest, opts) = fixture_opts(src.path(), db_key, wrapping);
        let receipt = build_snapshot(&opts).expect("build");
        let restore_dest = tempfile::tempdir().expect("restore");
        let report = restore_snapshot(
            &snapshot_dir(dest.path(), &receipt),
            &wrapping,
            restore_dest.path(),
            KeyInstall::IfAbsentOrEqual,
        )
        .expect("restore");
        assert_eq!(report.key_install, KeyInstallOutcome::Installed);
        // And the key in the report opens the restored DB.
        assert!(
            medical_db::Database::open(
                &restore_dest.path().join("medical.db"),
                Some(report.db_key)
            )
            .is_ok()
        );
    }

    impl RestoreReport {
        /// Test helper: the redacted Debug output must not contain the key
        /// in hex form (any prefix of it).
        fn debug_string_has_no_key(&self) -> bool {
            let dbg = format!("{self:?}");
            let key_hex = hex::encode(self.db_key);
            let prefix = &key_hex[..16];
            !dbg.contains(prefix)
        }
    }

    #[test]
    fn verify_fails_on_tampered_payload() {
        let src = tempfile::tempdir().expect("src");
        let (dest, opts) = fixture_opts(src.path(), [0xC3u8; 32], [0xD4u8; 32]);
        let receipt = build_snapshot(&opts).expect("build");
        let dir = snapshot_dir(dest.path(), &receipt);

        // Flip one byte in the first payload file.
        let payload = dir.join(PAYLOAD_DIR).join("f000000.bin");
        let mut bytes = fs::read(&payload).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        fs::write(&payload, bytes).unwrap();

        let result = verify_snapshot(&dir, &[0xD4u8; 32]);
        assert!(matches!(result, Err(BackupError::Verification(_))));
    }

    #[test]
    fn verify_fails_on_edited_receipt() {
        let src = tempfile::tempdir().expect("src");
        let (dest, opts) = fixture_opts(src.path(), [0xE5u8; 32], [0xF6u8; 32]);
        let receipt = build_snapshot(&opts).expect("build");
        let dir = snapshot_dir(dest.path(), &receipt);

        // Claim a different recording count without re-tagging.
        let mut tampered = receipt.clone();
        tampered.recording_count += 1;
        fs::write(
            dir.join(RECEIPT_FILE),
            serde_json::to_vec_pretty(&tampered).unwrap(),
        )
        .unwrap();

        let result = verify_snapshot(&dir, &[0xF6u8; 32]);
        assert!(matches!(result, Err(BackupError::Verification(_))));
    }

    #[test]
    fn verify_fails_with_wrong_wrapping_key() {
        let src = tempfile::tempdir().expect("src");
        let (dest, opts) = fixture_opts(src.path(), [0x17u8; 32], [0x28u8; 32]);
        let receipt = build_snapshot(&opts).expect("build");
        let result = verify_snapshot(&snapshot_dir(dest.path(), &receipt), &[0x39u8; 32]);
        assert!(
            result.is_err(),
            "wrong wrapping key must not verify (manifest undecryptable)"
        );
    }

    #[test]
    fn verify_rejects_unlisted_payload_file() {
        let src = tempfile::tempdir().expect("src");
        let (dest, opts) = fixture_opts(src.path(), [0x4Au8; 32], [0x5Bu8; 32]);
        let receipt = build_snapshot(&opts).expect("build");
        let dir = snapshot_dir(dest.path(), &receipt);
        fs::write(dir.join(PAYLOAD_DIR).join("f999999.bin"), b"smuggled").unwrap();
        let result = verify_snapshot(&dir, &[0x5Bu8; 32]);
        assert!(matches!(result, Err(BackupError::Verification(_))));
    }

    #[test]
    fn safe_join_rejects_traversal() {
        assert!(safe_join(Path::new("/tmp/x"), "recordings/a.enc").is_ok());
        assert!(safe_join(Path::new("/tmp/x"), "../escape").is_err());
        assert!(safe_join(Path::new("/tmp/x"), "/absolute").is_err());
    }

    /// Finding 4: a manifest whose GCM layer is VALID (re-encrypted under
    /// the real snapshot key by an attacker who holds the wrapping key)
    /// but whose relative_path was altered must still fail verification
    /// via the independent HMAC.
    #[test]
    fn verify_fails_on_tampered_manifest_path() {
        let src = tempfile::tempdir().expect("src");
        let db_key = [0xC7u8; 32];
        let wrapping = [0xD8u8; 32];
        let (dest, opts) = fixture_opts(src.path(), db_key, wrapping);
        let receipt = build_snapshot(&opts).expect("build");
        let dir = snapshot_dir(dest.path(), &receipt);
        let aes_key = keys::snapshot_aes_key(&wrapping);

        // Rewrite the manifest with a tampered path, re-encrypted so the
        // GCM layer itself is perfectly valid.
        let blob = fs::read(dir.join(MANIFEST_FILE)).unwrap();
        let plain = file_crypto::decrypt_bytes_with_key(&aes_key, &blob).unwrap();
        let mut manifest: SnapshotManifest = serde_json::from_slice(&plain).unwrap();
        let entry = manifest
            .entries
            .iter_mut()
            .find(|e| e.relative_path.starts_with("recordings/"))
            .expect("recording entry");
        entry.relative_path = "recordings/tampered-name.enc".into();
        let re_enc =
            file_crypto::encrypt_bytes_with_key(&aes_key, &serde_json::to_vec(&manifest).unwrap())
                .unwrap();
        fs::write(dir.join(MANIFEST_FILE), re_enc).unwrap();

        // Receipt untouched → the HMAC (now covering relative_path) must
        // catch what the payload hashes alone never would.
        let result = verify_snapshot(&dir, &wrapping);
        assert!(
            matches!(result, Err(BackupError::Verification(ref msg))
                if msg.contains("HMAC")),
            "expected HMAC mismatch, got: {result:?}"
        );
    }

    /// v1 snapshots (old tag format) are refused with an explicit version
    /// error, not a confusing HMAC mismatch.
    #[test]
    fn verify_rejects_v1_snapshots_with_version_error() {
        let src = tempfile::tempdir().expect("src");
        let (dest, opts) = fixture_opts(src.path(), [0xE9u8; 32], [0xFAu8; 32]);
        let mut receipt = build_snapshot(&opts).expect("build");
        let dir = snapshot_dir(dest.path(), &receipt);
        receipt.version = 1;
        fs::write(
            dir.join(RECEIPT_FILE),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let result = verify_snapshot(&dir, &[0xFAu8; 32]);
        assert!(
            matches!(result, Err(BackupError::Format(ref msg)) if msg.contains("unsupported")),
            "expected unsupported-version error, got: {result:?}"
        );
    }
    /// Finding 6: local retention keeps the NEWEST N snapshot dirs by
    /// receipt created_at; undated dirs are never touched.
    #[test]
    fn prune_local_snapshots_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        let base = chrono::Utc::now();
        for i in 0..3 {
            let snap = dir.path().join(format!("snap-{i}"));
            fs::create_dir_all(&snap).unwrap();
            let receipt = SnapshotReceipt {
                snapshot_id: format!("snap-{i}"),
                version: SNAPSHOT_VERSION,
                created_at: base - chrono::Duration::hours(i as i64), // 0 newest
                file_count: 1,
                total_bytes: 1,
                recording_count: 0,
                hmac_tag: "x".into(),
            };
            fs::write(
                snap.join(RECEIPT_FILE),
                serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap();
        }
        // A dir with no receipt — undatable, must survive.
        fs::create_dir_all(dir.path().join("snap-undated")).unwrap();

        let removed = prune_local_snapshots(dir.path(), 1);
        assert_eq!(removed.len(), 2, "removed the two oldest");
        // Newest + undated survive.
        assert!(dir.path().join("snap-0").join(RECEIPT_FILE).exists());
        assert!(dir.path().join("snap-undated").exists());
        assert!(!dir.path().join("snap-1").exists());
        assert!(!dir.path().join("snap-2").exists());
    }
}
