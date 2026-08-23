//! Filesystem backup destination: a plain folder (USB drive, network
//! share, or a cloud-synced directory) holding the SAME store layout the
//! HTTP agent serves (`serve --root DIR`) — shared content-addressed
//! blobs under `blobs/<xx>/<hash>` plus committed `<snap-id>/` dirs with
//! `receipt.json`, `manifest.json.enc`, `blobs.idx`, and `.committed`.
//! Layout parity means assemble/verify/restore work against either kind
//! of store unchanged, and no PHI ever appears in a filename (hashes,
//! sizes, and opaque ids only).
//!
//! Unlike the append-only agent, the source machine OWNS this folder and
//! can rewrite it — retention pruning is the writer's job here.

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::BackupResult;
use crate::agent::{self, BLOBS_DIR, BLOBS_IDX_FILE, COMMITTED_MARKER};
use crate::client::PushStats;
use crate::snapshot::{
    self, MANIFEST_FILE, PAYLOAD_DIR, RECEIPT_FILE, SnapshotManifest, SnapshotReceipt,
};

/// Default snapshot retention on a folder destination.
pub const DEFAULT_FOLDER_KEEP: usize = 30;

/// Push a built v3 snapshot into the folder store at `store_root`,
/// mirroring the agent's CAS commit: every unique blob is copied at most
/// once and hash-checked while copying (a corrupt source can never poison
/// the store), then the snapshot dir is committed with receipt, encrypted
/// manifest, and blob index. Stream-staged recordings (no local payload
/// file) are copied from `recordings_dir`, exactly like the HTTP client's
/// CAS push. Returns the receipt and transfer stats.
pub fn push_to_folder(
    snapshot_dir: &Path,
    store_root: &Path,
    recordings_dir: Option<&Path>,
    wrapping_key: &[u8; 32],
) -> BackupResult<(SnapshotReceipt, PushStats)> {
    let receipt: SnapshotReceipt =
        serde_json::from_slice(&std::fs::read(snapshot_dir.join(RECEIPT_FILE))?)?;
    if receipt.version < 3 {
        return Err(crate::BackupError::Format(
            "folder destinations support v3 (CAS) snapshots only".into(),
        ));
    }
    let manifest_bytes = std::fs::read(snapshot_dir.join(MANIFEST_FILE))?;
    let manifest = decrypt_manifest(&manifest_bytes, wrapping_key)?;

    // The store root must already exist: never CREATE the destination
    // itself. If the drive was unplugged between the job's pre-flight
    // check and here, a blind create_dir_all would silently re-create
    // the (now empty) mount point on the PARENT volume and write backups
    // to the wrong disk.
    if !store_root.is_dir() {
        return Err(crate::BackupError::Setup(format!(
            "backup destination not available: {}",
            store_root.display()
        )));
    }
    std::fs::create_dir_all(store_root.join(BLOBS_DIR))?;
    let payload_dir = snapshot_dir.join(PAYLOAD_DIR);
    let mut stats = PushStats::default();
    let mut seen = std::collections::HashSet::new();
    for entry in &manifest.entries {
        if !seen.insert(entry.sha256.clone()) {
            continue; // two manifest entries may share one blob
        }
        validate_blob_hash(&entry.sha256)?;
        let dest = agent::blob_path(store_root, &entry.sha256);
        if dest.is_file() {
            stats.skipped += 1;
            continue;
        }
        // Staged locally, or stream from the recordings source? (Same
        // resolution as the HTTP client's CAS push.)
        let staged = payload_dir.join(&entry.sha256);
        let source: PathBuf = if staged.is_file() {
            staged
        } else if let Some(dir) = recordings_dir
            && let Some(name) = entry.relative_path.strip_prefix("recordings/")
        {
            dir.join(name)
        } else {
            return Err(crate::BackupError::Verification(format!(
                "no local payload for blob {} and no recordings_dir to stream it from",
                &entry.sha256[..8.min(entry.sha256.len())]
            )));
        };
        copy_blob_hashchecked(&source, &dest)?;
        stats.uploaded += 1;
    }

    // Commit: receipt + manifest + blob index + marker, laid down in a
    // temp dir and renamed into place so a crash mid-push can never look
    // committed (mirrors the agent's freeze semantics).
    let id = receipt.snapshot_id.clone();
    let snap_dir = store_root.join(&id);
    let tmp = store_root.join(format!(".tmp-{id}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&tmp)?;
    std::fs::write(tmp.join(RECEIPT_FILE), serde_json::to_vec_pretty(&receipt)?)?;
    std::fs::write(tmp.join(MANIFEST_FILE), &manifest_bytes)?;
    let mut hashes: Vec<&String> = seen.iter().collect();
    hashes.sort();
    std::fs::write(
        tmp.join(BLOBS_IDX_FILE),
        serde_json::to_vec_pretty(&hashes)?,
    )?;
    std::fs::write(tmp.join(COMMITTED_MARKER), b"1")?;
    set_readonly_tree_files(&tmp);
    std::fs::rename(&tmp, &snap_dir)?;
    Ok((receipt, stats))
}

/// List committed snapshots in a folder store, newest first (by receipt
/// `created_at`). Skips incomplete dirs and stray `.tmp-` push leftovers.
pub fn list_folder_snapshots(store_root: &Path) -> BackupResult<Vec<SnapshotReceipt>> {
    let mut committed: Vec<SnapshotReceipt> = Vec::new();
    for entry in std::fs::read_dir(store_root)? {
        let dir = entry?.path();
        if !dir.is_dir() || !dir.join(COMMITTED_MARKER).exists() {
            continue;
        }
        let Ok(bytes) = std::fs::read(dir.join(RECEIPT_FILE)) else {
            continue;
        };
        if let Ok(receipt) = serde_json::from_slice::<SnapshotReceipt>(&bytes) {
            committed.push(receipt);
        }
    }
    committed.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    Ok(committed)
}

/// The pull side for folder stores: assemble a verified local snapshot
/// directory (`out_dir/<snap-id>/`) from the store's receipt, encrypted
/// manifest, and CAS blobs. The assembled tree must pass full HMAC
/// verification (`snapshot::verify_snapshot`) before it is returned —
/// the same fail-closed contract as the HTTP client's `pull_snapshot`.
pub fn assemble_from_folder(
    store_root: &Path,
    id: Option<&str>,
    out_dir: &Path,
    wrapping_key: &[u8; 32],
) -> BackupResult<PathBuf> {
    let receipts = list_folder_snapshots(store_root)?;
    let receipt = match id {
        Some(wanted) => receipts
            .iter()
            .find(|r| r.snapshot_id == wanted)
            .ok_or_else(|| {
                crate::BackupError::Verification(format!("snapshot {wanted} not found in store"))
            })?,
        None => receipts.first().ok_or_else(|| {
            crate::BackupError::Verification("folder store has no snapshots".into())
        })?,
    };
    let store_snap_dir = store_root.join(&receipt.snapshot_id);
    let local_dir = out_dir.join(&receipt.snapshot_id);
    std::fs::create_dir_all(local_dir.join(PAYLOAD_DIR))?;
    std::fs::write(
        local_dir.join(RECEIPT_FILE),
        std::fs::read(store_snap_dir.join(RECEIPT_FILE))?,
    )?;
    let manifest_bytes = std::fs::read(store_snap_dir.join(MANIFEST_FILE))?;
    std::fs::write(local_dir.join(MANIFEST_FILE), &manifest_bytes)?;
    let manifest = decrypt_manifest(&manifest_bytes, wrapping_key)?;

    let mut fetched = std::collections::HashSet::new();
    for entry in &manifest.entries {
        // CAS: one shared blob serves every entry referencing it
        // (opaque_name == hash in v3).
        if !fetched.insert(entry.sha256.clone()) {
            continue;
        }
        let src = agent::blob_path(store_root, &entry.sha256);
        if !src.is_file() {
            return Err(crate::BackupError::Verification(format!(
                "store is missing blob {}",
                &entry.sha256[..8.min(entry.sha256.len())]
            )));
        }
        std::fs::copy(&src, local_dir.join(PAYLOAD_DIR).join(&entry.opaque_name))?;
    }
    // Fail closed: the assembled snapshot must verify before use.
    snapshot::verify_snapshot(&local_dir, wrapping_key)?;
    Ok(local_dir)
}

/// Retention on a folder destination: keep the newest `keep` committed
/// snapshots (at least one), remove the rest, then garbage-collect blobs
/// no remaining snapshot references. Blobs younger than the agent's GC
/// grace window are spared (in-flight-push protection). Returns pruned ids.
pub fn prune_folder_store(store_root: &Path, keep: usize) -> Vec<String> {
    let Ok(mut committed) = list_folder_snapshots(store_root) else {
        return Vec::new();
    };
    let keep = keep.max(1);
    let mut pruned = Vec::new();
    // List is newest-first; pop() removes the oldest.
    while committed.len() > keep {
        let victim = committed.pop().expect("len > keep");
        let dir = store_root.join(&victim.snapshot_id);
        if agent::unfreeze_and_remove(&dir).is_ok() {
            pruned.push(victim.snapshot_id);
        }
    }
    let _ = gc_blobs(store_root);
    pruned
}

// ── helpers ─────────────────────────────────────────────────────────────

fn decrypt_manifest(bytes: &[u8], wrapping_key: &[u8; 32]) -> BackupResult<SnapshotManifest> {
    let aes_key = crate::keys::snapshot_aes_key(wrapping_key);
    let plain = medical_security::file_crypto::decrypt_bytes_with_key(&aes_key, bytes)
        .map_err(|e| crate::BackupError::Verification(format!("manifest decrypt failed: {e}")))?;
    serde_json::from_slice(&plain)
        .map_err(|e| crate::BackupError::Format(format!("manifest parse: {e}")))
}

fn validate_blob_hash(hash: &str) -> BackupResult<()> {
    let ok = hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(crate::BackupError::Format(
            "manifest references a malformed blob hash".into(),
        ))
    }
}

/// Copy `source` to `dest` while hashing: the destination filename IS the
/// claimed content hash, so bytes that don't hash to it are rejected
/// before anything lands — the same anti-poisoning guarantee the agent
/// enforces at upload time. Atomic: temp + fsync + rename. Streams in
/// 256 KiB chunks (blobs can be ~1 GB).
fn copy_blob_hashchecked(source: &Path, dest: &Path) -> BackupResult<()> {
    use sha2::{Digest, Sha256};
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_file_name(format!(".tmp-{}", uuid::Uuid::new_v4().simple()));
    let mut reader = BufReader::with_capacity(256 * 1024, std::fs::File::open(source)?);
    let mut writer = BufWriter::with_capacity(256 * 1024, std::fs::File::create(&tmp)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    let hash = hex::encode(hasher.finalize());
    let claimed = dest.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !hash.eq_ignore_ascii_case(claimed) {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::BackupError::Verification(format!(
            "blob {} failed its content hash while copying",
            &claimed[..8.min(claimed.len())]
        )));
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Mark-and-sweep the blob store: a blob survives iff some committed
/// snapshot's `blobs.idx` references it OR it is younger than the GC
/// grace window. Same rules as the agent's `gc_blobs`.
fn gc_blobs(store_root: &Path) -> std::io::Result<usize> {
    let blobs_root = store_root.join(BLOBS_DIR);
    if !blobs_root.is_dir() {
        return Ok(0);
    }
    let mut referenced = std::collections::HashSet::new();
    for entry in std::fs::read_dir(store_root)?.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || !dir.join(COMMITTED_MARKER).exists() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(dir.join(BLOBS_IDX_FILE))
            && let Ok(hashes) = serde_json::from_slice::<Vec<String>>(&bytes)
        {
            referenced.extend(hashes);
        }
    }
    let grace_cutoff = std::time::SystemTime::now()
        .checked_sub(crate::agent::BLOB_GC_GRACE)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut removed = 0usize;
    for shard in std::fs::read_dir(&blobs_root)?.flatten() {
        let shard_dir = shard.path();
        if !shard_dir.is_dir() {
            continue;
        }
        for blob in std::fs::read_dir(&shard_dir)?.flatten() {
            let path = blob.path();
            let name = blob.file_name().to_string_lossy().into_owned();
            if name.starts_with(".tmp-") || referenced.contains(&name) {
                continue;
            }
            let aged = blob
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .is_some_and(|t| t < grace_cutoff);
            if !aged {
                continue;
            }
            agent::make_writable_file(&path);
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// chmod 0444 every FILE directly under `dir` (best-effort, unix only) —
/// tamper-evidence for committed snapshots. Dirs stay writable: pruning
/// must be able to remove them, and the agent's `unfreeze_and_remove`
/// handles the read-only files.
fn set_readonly_tree_files(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    let _ = std::fs::set_permissions(
                        entry.path(),
                        std::fs::Permissions::from_mode(0o444),
                    );
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::recording::Recording;
    use medical_db::recordings::RecordingsRepo;

    /// Real SQLCipher fixture: DB + one encrypted recording, built with
    /// `build_snapshot`. `existing_recordings` keeps the recordings dir
    /// STABLE across builds — real incremental semantics need immutable
    /// ciphertext, and a fresh GCM nonce per call would make every build
    /// look like a new recording (same trick as the transport tests).
    fn fixture_snapshot(
        dest: &Path,
        salt: u8,
        existing_recordings: Option<&Path>,
    ) -> (SnapshotReceipt, [u8; 32], PathBuf) {
        let db_key = [salt; 32];
        let wrapping = [salt ^ 0xFF; 32];
        let db_path = dest.join("medical.db");
        {
            let database = medical_db::Database::open(&db_path, Some(db_key)).unwrap();
            let conn = database.conn().unwrap();
            RecordingsRepo::insert(
                &conn,
                &Recording::new(format!("r{salt}.enc"), dest.join(format!("r{salt}.enc"))),
            )
            .unwrap();
        }
        let recordings: PathBuf = match existing_recordings {
            Some(dir) => dir.to_path_buf(),
            None => {
                let recordings = dest.join("recordings");
                std::fs::create_dir_all(&recordings).unwrap();
                let wav_key = medical_security::file_crypto::derive_file_key(&db_key);
                std::fs::write(
                    recordings.join("r0.enc"),
                    medical_security::file_crypto::encrypt_bytes_with_key(
                        &wav_key,
                        format!("RIFF patient audio {salt}").as_bytes(),
                    )
                    .unwrap(),
                )
                .unwrap();
                recordings
            }
        };
        let receipt = snapshot::build_snapshot(&snapshot::BuildOptions {
            db_path,
            recordings_dir: recordings.clone(),
            keystore_path: None,
            dest_dir: dest.to_path_buf(),
            db_key,
            wrapping_key: wrapping,
            staging: snapshot::StagingMode::Hardlink,
        })
        .unwrap();
        (receipt, wrapping, recordings)
    }

    #[test]
    fn push_creates_agent_layout_and_second_push_skips_blobs() {
        let built = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (r1, wrapping, recordings) = fixture_snapshot(built.path(), 0x11, None);

        let (pushed, stats1) = push_to_folder(
            &built.path().join(&r1.snapshot_id),
            store.path(),
            None,
            &wrapping,
        )
        .unwrap();
        assert_eq!(pushed.snapshot_id, r1.snapshot_id);
        assert_eq!(stats1.uploaded, 3, "db + wrapped key + recording");
        assert_eq!(stats1.skipped, 0);
        // Agent-store layout parity: committed dir + blob store.
        let snap = store.path().join(&r1.snapshot_id);
        assert!(snap.join(COMMITTED_MARKER).exists());
        assert!(snap.join(RECEIPT_FILE).is_file());
        assert!(snap.join(MANIFEST_FILE).is_file());
        assert!(snap.join(BLOBS_IDX_FILE).is_file());
        let idx: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(snap.join(BLOBS_IDX_FILE)).unwrap())
                .unwrap();
        for hash in &idx {
            assert!(
                agent::blob_path(store.path(), hash).is_file(),
                "blob {hash}"
            );
        }

        // Second build over the SAME recordings dir → the recording blob
        // is byte-identical (immutable ciphertext) so it is skipped; only
        // the always-fresh DB + wrapped-key blobs transfer.
        let (r2, _, _) = fixture_snapshot(built.path(), 0x11, Some(&recordings));
        let (_, stats2) = push_to_folder(
            &built.path().join(&r2.snapshot_id),
            store.path(),
            None,
            &wrapping,
        )
        .unwrap();
        assert_eq!(stats2.skipped, 1, "the recording blob is already stored");
        assert_eq!(
            stats2.uploaded, 2,
            "only the fresh DB + wrapped-key blobs transfer"
        );
    }

    #[test]
    fn assemble_verifies_and_fails_closed_on_tampered_blob() {
        let built = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (r1, wrapping, _) = fixture_snapshot(built.path(), 0x22, None);
        push_to_folder(
            &built.path().join(&r1.snapshot_id),
            store.path(),
            None,
            &wrapping,
        )
        .unwrap();

        // Clean assemble verifies end-to-end.
        let out = tempfile::tempdir().unwrap();
        let local = assemble_from_folder(store.path(), None, out.path(), &wrapping).unwrap();
        assert!(local.ends_with(&r1.snapshot_id));
        assert!(snapshot::verify_snapshot(&local, &wrapping).is_ok());

        // Flip one byte in a stored blob → assemble must fail closed.
        let idx: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(store.path().join(&r1.snapshot_id).join(BLOBS_IDX_FILE))
                .unwrap(),
        )
        .unwrap();
        let victim = agent::blob_path(store.path(), &idx[0]);
        agent::make_writable_file(&victim);
        let mut bytes = std::fs::read(&victim).unwrap();
        bytes[0] ^= 0xFF;
        std::fs::write(&victim, bytes).unwrap();
        let out2 = tempfile::tempdir().unwrap();
        assert!(
            assemble_from_folder(store.path(), None, out2.path(), &wrapping).is_err(),
            "tampered blob must fail closed"
        );
    }

    #[test]
    fn push_rejects_source_that_does_not_hash_to_claimed_key() {
        // Anti-poisoning: a staged payload file whose bytes don't match
        // its hash-named filename is rejected, nothing lands. Corrupt
        // EVERY payload file so blob-processing order can't mask it.
        let built = tempfile::tempdir().unwrap();
        let (r1, wrapping, _) = fixture_snapshot(built.path(), 0x33, None);
        let snap = built.path().join(&r1.snapshot_id);
        for name in std::fs::read_dir(snap.join(PAYLOAD_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        {
            std::fs::write(snap.join(PAYLOAD_DIR).join(&name), b"corrupted-bytes").unwrap();
        }
        let store = tempfile::tempdir().unwrap();
        let err = push_to_folder(&snap, store.path(), None, &wrapping);
        assert!(err.is_err(), "hash mismatch must be rejected");
        // No blob FILE and no committed snapshot landed. (An empty shard
        // dir may remain — harmless residue, swept like the agent's.)
        for shard in std::fs::read_dir(store.path().join(BLOBS_DIR)).unwrap() {
            let shard = shard.unwrap().path();
            assert!(
                shard.is_dir() && shard.read_dir().unwrap().next().is_none(),
                "no blob bytes may land: {}",
                shard.display()
            );
        }
        assert!(list_folder_snapshots(store.path()).unwrap().is_empty());
    }

    #[test]
    fn prune_keeps_newest_and_gcs_orphans_aged_past_grace() {
        let built = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (r1, wrapping, recordings) = fixture_snapshot(built.path(), 0x44, None);
        push_to_folder(
            &built.path().join(&r1.snapshot_id),
            store.path(),
            None,
            &wrapping,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let (r2, _, _) = fixture_snapshot(built.path(), 0x44, Some(&recordings));
        push_to_folder(
            &built.path().join(&r2.snapshot_id),
            store.path(),
            None,
            &wrapping,
        )
        .unwrap();

        // Age every blob past the GC grace window so the orphan sweep can
        // actually collect (fresh orphans are in-flight-push protection).
        {
            use std::os::unix::fs::PermissionsExt;
            let old = std::time::SystemTime::now()
                - crate::agent::BLOB_GC_GRACE
                - std::time::Duration::from_secs(60);
            for shard in std::fs::read_dir(store.path().join(BLOBS_DIR)).unwrap() {
                let shard = shard.unwrap().path();
                for blob in std::fs::read_dir(&shard).unwrap() {
                    let p = blob.unwrap().path();
                    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
                    std::fs::File::open(&p).unwrap().set_modified(old).unwrap();
                }
            }
        }

        let pruned = prune_folder_store(store.path(), 1);
        assert_eq!(pruned, vec![r1.snapshot_id.clone()]);
        assert!(
            !store.path().join(&r1.snapshot_id).exists(),
            "oldest pruned"
        );
        assert!(
            store.path().join(&r2.snapshot_id).exists(),
            "newest survives"
        );
        // The surviving snapshot still assembles (its blobs were kept).
        let out = tempfile::tempdir().unwrap();
        assert!(assemble_from_folder(store.path(), None, out.path(), &wrapping).is_ok());
    }
}
