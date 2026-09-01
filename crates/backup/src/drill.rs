//! Automated restore drill (R4): a backup only counts if a restore from
//! it has actually been tested. `run_drill` restores a snapshot into a
//! throwaway directory and verifies the three things that matter after a
//! disk dies:
//!
//! 1. integrity — the snapshot's HMAC and per-file hashes verify;
//! 2. the restored SQLCipher DB opens with the escrow-recoverable key and
//!    its record count matches the receipt;
//! 3. a sample recording file decrypts (FE1 + AES-GCM under the
//!    DB-derived WAV key).
//!
//! The outcome carries short status lines only — counts and errors, no
//! clinical content (PHI rule). Any failure sets `passed = false`; the
//! CLI turns that into a loud non-zero exit.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use medical_security::file_crypto;

use crate::BackupResult;
use crate::snapshot;

/// Result of a drill run. `checks`/`failures` are human-facing lines that
/// never contain PHI.
#[derive(Debug, Clone)]
pub struct DrillOutcome {
    pub passed: bool,
    pub snapshot_id: String,
    pub checks: Vec<String>,
    pub failures: Vec<String>,
}

/// Run the restore drill against the snapshot at `snapshot_dir`.
pub fn run_drill(snapshot_dir: &Path, wrapping_key: &[u8; 32]) -> DrillOutcome {
    let mut checks: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    // 1. Integrity + authenticity (R5).
    let summary = match snapshot::verify_snapshot(snapshot_dir, wrapping_key) {
        Ok(s) => {
            checks.push(format!(
                "integrity OK: {} payload files, {} bytes, HMAC verified",
                s.files_checked, s.total_bytes
            ));
            s
        }
        Err(e) => {
            failures.push(format!("verification failed: {e}"));
            return failed(snapshot_dir, checks, failures);
        }
    };
    let snapshot_id = summary.receipt.snapshot_id.clone();

    // 2. Restore into a throwaway directory (R6 path, exercised for real).
    // KeyInstall::Skip — the drill runs on the LIVE machine and must never
    // touch the operator's real keychain.
    let scratch = drill_scratch_dir(&snapshot_id);
    let report = match snapshot::restore_snapshot(
        snapshot_dir,
        wrapping_key,
        &scratch,
        snapshot::KeyInstall::Skip,
        // The scratch dir is freshly made — the non-empty guard is a
        // no-op here, but keep it enforced: a scratch dir with leftovers
        // would mean a drill bug worth failing on.
        false,
    ) {
        Ok(r) => r,
        Err(e) => {
            failures.push(format!("restore failed: {e}"));
            let _ = std::fs::remove_dir_all(&scratch);
            return DrillOutcome {
                passed: false,
                snapshot_id,
                checks,
                failures,
            };
        }
    };
    if !report.db_key_recovered {
        failures.push("restored snapshot did not yield the DB key".into());
    } else {
        checks.push("restore OK: DB key recovered from escrow material".into());
    }

    // 3. Open the restored DB with the recovered key, diff record counts (R4).
    let db_key = report.db_key;
    let restored_db = scratch.join("medical.db");
    match medical_db::Database::open(&restored_db, Some(db_key)) {
        Ok(db) => {
            let count = db
                .conn()
                .and_then(|conn| {
                    conn.query_row("SELECT count(*) FROM recordings", [], |r| {
                        r.get::<_, i64>(0)
                    })
                    .map_err(medical_db::DbError::Sqlite)
                })
                .unwrap_or(-1);
            if count as u64 == summary.receipt.recording_count {
                checks.push(format!(
                    "record count OK: {} rows in restored DB matches receipt",
                    count
                ));
            } else {
                failures.push(format!(
                    "record count MISMATCH: restored DB has {count} rows, receipt says {}",
                    summary.receipt.recording_count
                ));
            }
        }
        Err(e) => failures.push(format!(
            "restored DB failed to open with recovered key: {e}"
        )),
    }

    // 4. Sample recording decrypt (R4): FE1 magic + GCM auth.
    let recordings_dir = scratch.join("recordings");
    let sample: Option<PathBuf> = std::fs::read_dir(&recordings_dir).ok().and_then(|rd| {
        rd.filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.is_file())
    });
    match sample {
        Some(path) => {
            let wav_key = file_crypto::derive_file_key(&db_key);
            match fs_read_and_decrypt(&path, &wav_key) {
                Ok(len) => checks.push(format!(
                    "sample recording decrypts OK ({} bytes plaintext)",
                    len
                )),
                Err(e) => failures.push(format!("sample recording failed to decrypt: {e}")),
            }
        }
        None => checks.push("sample decrypt: skipped (no recording files in snapshot)".into()),
    }

    let _ = std::fs::remove_dir_all(&scratch);
    let passed = failures.is_empty();
    if !passed {
        failures.insert(0, "DRILL FAILED — this backup cannot be restored".into());
    }
    DrillOutcome {
        passed,
        snapshot_id,
        checks,
        failures,
    }
}

fn fs_read_and_decrypt(path: &Path, key: &[u8; 32]) -> BackupResult<usize> {
    let bytes = std::fs::read(path)?;
    let plain = file_crypto::decrypt_bytes_with_key(key, &bytes)?;
    Ok(plain.len())
}

fn failed(dir: &Path, checks: Vec<String>, failures: Vec<String>) -> DrillOutcome {
    // Best-effort snapshot id from the directory name for the report.
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut failures = failures;
    failures.insert(0, "DRILL FAILED — this backup cannot be restored".into());
    DrillOutcome {
        passed: false,
        snapshot_id: id,
        checks,
        failures,
    }
}

fn drill_scratch_dir(snapshot_id: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ferriscribe-drill-{snapshot_id}-{}",
        Uuid::new_v4().simple()
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{BuildOptions, StagingMode, build_snapshot};
    use medical_core::types::recording::Recording;
    use medical_db::recordings::RecordingsRepo;

    fn fixture(
        dir: &std::path::Path,
        db_key: [u8; 32],
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let db_path = dir.join("medical.db");
        let database = medical_db::Database::open(&db_path, Some(db_key)).expect("db");
        {
            let conn = database.conn().expect("conn");
            for i in 0..2 {
                RecordingsRepo::insert(
                    &conn,
                    &Recording::new(format!("r{i}.enc"), dir.join(format!("r{i}.enc"))),
                )
                .expect("insert");
            }
        }
        drop(database);
        let recordings = dir.join("recordings");
        std::fs::create_dir_all(&recordings).unwrap();
        let wav_key = file_crypto::derive_file_key(&db_key);
        let blob =
            file_crypto::encrypt_bytes_with_key(&wav_key, b"RIFF patient audio PHI").unwrap();
        std::fs::write(recordings.join("r0.enc"), blob).unwrap();
        (db_path, recordings)
    }

    #[test]
    fn drill_passes_on_healthy_snapshot() {
        let src = tempfile::tempdir().unwrap();
        let db_key = [0x7Cu8; 32];
        let wrapping = [0x8Du8; 32];
        let (db_path, recordings) = fixture(src.path(), db_key);
        let dest = tempfile::tempdir().unwrap();
        let receipt = build_snapshot(&BuildOptions {
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            dest_dir: dest.path().to_path_buf(),
            db_key,
            wrapping_key: wrapping,
            staging: StagingMode::Hardlink,
        })
        .expect("build");

        let outcome = run_drill(&dest.path().join(&receipt.snapshot_id), &wrapping);
        assert!(outcome.passed, "failures: {:?}", outcome.failures);
        assert!(outcome.checks.iter().any(|c| c.contains("record count OK")));
        assert!(outcome.checks.iter().any(|c| c.contains("decrypts OK")));
    }

    #[test]
    fn drill_fails_on_deliberate_corruption() {
        // The acceptance-criteria corruption injection: flip one payload
        // byte and the drill must fail LOUDLY.
        let src = tempfile::tempdir().unwrap();
        let db_key = [0x9Eu8; 32];
        let wrapping = [0xAFu8; 32];
        let (db_path, recordings) = fixture(src.path(), db_key);
        let dest = tempfile::tempdir().unwrap();
        let receipt = build_snapshot(&BuildOptions {
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            dest_dir: dest.path().to_path_buf(),
            db_key,
            wrapping_key: wrapping,
            staging: StagingMode::Hardlink,
        })
        .expect("build");
        let dir = dest.path().join(&receipt.snapshot_id);

        let payload = {
            let mut names: Vec<_> = std::fs::read_dir(dir.join("payload"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            names.sort();
            names[0].clone()
        };
        let mut bytes = std::fs::read(&payload).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        std::fs::write(&payload, bytes).unwrap();

        let outcome = run_drill(&dir, &wrapping);
        assert!(!outcome.passed);
        assert!(outcome.failures[0].contains("DRILL FAILED"));
        assert!(
            outcome
                .failures
                .iter()
                .any(|f| f.contains("verification failed"))
        );
    }
}
