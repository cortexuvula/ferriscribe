//! The shared backup job: build a snapshot, push it (when configured),
//! drill the TARGET's re-pulled copy, apply local retention, and persist
//! a machine-readable status. Used by BOTH the CLI (`backup-and-push`,
//! what launchd invokes) and the app's "Back up now" button — one code
//! path, so the button exercises exactly what the schedule runs.

use std::path::{Path, PathBuf};

use crate::client::BackupClient;
use crate::drill;
use crate::snapshot::{self, BuildOptions};
use crate::status::{self, BackupRunStatus};

/// Where off-machine copies go. The HTTP agent is append-only (a
/// compromised source cannot erase history); a folder store is writable
/// by this machine — easiest to set up, weaker under ransomware.
#[derive(Debug, Clone)]
pub enum BackupTarget {
    Agent { url: String, token: String },
    Folder { path: PathBuf },
}

/// Where the job reads/writes everything. `data_dir` is the app data root
/// (status file + local `backups/` staging live there).
pub struct JobConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub recordings_dir: PathBuf,
    pub keystore_path: Option<PathBuf>,
    /// Target agent; `None` = local-only snapshot.
    pub target: Option<BackupTarget>,
    /// Local staging retention after a successful push (default 14).
    pub keep_local: usize,
}

/// Human-facing progress lines (also emitted as app events). PHI-free.
#[derive(Debug, Clone)]
pub struct JobEvent {
    pub line: String,
    pub kind: JobEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobEventKind {
    Step,
    Ok,
    Fail,
}

/// Outcome of a run — `status` is written to disk on every real run
/// (success or failure); a `skipped` run (another job held the lock)
/// deliberately leaves the previous status untouched.
pub struct JobOutcome {
    pub status: BackupRunStatus,
    pub events: Vec<JobEvent>,
    pub skipped: bool,
}

impl JobOutcome {
    pub fn success(&self) -> bool {
        !self.skipped && self.status.failure.is_none() && self.status.drill_passed
    }
}

/// Failure with partial progress, so a run that failed AFTER building
/// (or pushing) still records WHICH snapshot exists and where it went.
struct JobFail {
    msg: String,
    snapshot_id: Option<String>,
    pushed_to: Option<String>,
    destination_missing: bool,
}

impl JobFail {
    fn early(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            snapshot_id: None,
            pushed_to: None,
            destination_missing: false,
        }
    }

    /// Pre-build failure: the destination folder is not attached.
    fn early_missing(msg: impl Into<String>) -> Self {
        Self {
            destination_missing: true,
            ..Self::early(msg)
        }
    }
}

/// A lock file serializing jobs across processes (launchd sidecar vs the
/// app's run-now). Steal-if-stale: a crashed holder's lock is taken over
/// after `STALE_AFTER` — long enough to never race a real run (jobs are
/// minutes), short enough to self-heal.
const LOCK_FILE: &str = "backup.lock";
const LOCK_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

struct JobLock(PathBuf);
impl Drop for JobLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn acquire_job_lock(data_dir: &Path) -> Option<JobLock> {
    let path = data_dir.join(LOCK_FILE);
    let stale = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > LOCK_STALE_AFTER);
    if stale {
        // Steal: the holder crashed hours ago.
        let _ = std::fs::remove_file(&path);
    }
    match std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Some(JobLock(path)),
        Err(_) if path.exists() => None, // live holder
        Err(_) => None,
    }
}

/// Run the full backup job. Never panics; failures become the status's
/// `failure` line plus a `Fail` event. Serialized across processes by a
/// lock file — an overlapping run returns a `skipped` outcome WITHOUT
/// touching the status file (the running job owns it).
///
/// Synchronous BY DESIGN: the CLI calls it from a plain main, and the
/// app's command wraps it in `tokio::task::spawn_blocking` (calling it
/// directly from an async worker would panic on the nested `block_on`).
pub fn run_backup_job(cfg: &JobConfig, db_key: [u8; 32], wrapping_key: [u8; 32]) -> JobOutcome {
    let mut events = Vec::new();

    let Some(_lock) = acquire_job_lock(&cfg.data_dir) else {
        events.push(step("skipped: another backup job is already running"));
        return JobOutcome {
            status: BackupRunStatus {
                last_run_at: chrono::Utc::now(),
                snapshot_id: None,
                drill_passed: false,
                pushed_to: None,
                failure: Some("skipped: concurrent run".into()),
                destination_missing: false,
            },
            events,
            skipped: true,
        };
    };

    // The re-pull staging dir lives for the whole run and is REMOVED at
    // the end, pass or fail — the forensics copy is the one in out_dir,
    // so this throwaway copy must not leak into shared temp forever.
    let staging = std::env::temp_dir().join(format!(
        "ferriscribe-job-staging-{}",
        uuid::Uuid::new_v4().simple()
    ));

    let run = |events: &mut Vec<JobEvent>, staging: &Path| -> Result<BackupRunStatus, JobFail> {
        let out_dir = cfg.data_dir.join("backups");
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| JobFail::early(format!("staging dir: {e}")))?;

        // 1. Build. Staging by mode: a target push streams recordings from
        // their source (nothing large is staged locally); a local-only
        // run hardlinks them in so the snapshot is self-contained.
        events.push(step("building snapshot…"));
        let receipt = snapshot::build_snapshot(&BuildOptions {
            db_path: cfg.db_path.clone(),
            recordings_dir: cfg.recordings_dir.clone(),
            keystore_path: cfg.keystore_path.clone(),
            dest_dir: out_dir.clone(),
            db_key,
            wrapping_key,
            staging: if cfg.target.is_some() {
                snapshot::StagingMode::Stream
            } else {
                snapshot::StagingMode::Hardlink
            },
        })
        .map_err(|e| JobFail::early(format!("snapshot build failed: {e}")))?;
        events.push(ok(&format!(
            "snapshot {} built ({} bytes)",
            receipt.snapshot_id, receipt.total_bytes
        )));
        let local_dir = out_dir.join(&receipt.snapshot_id);
        let built = receipt.snapshot_id.clone();

        // 2. Push + drill the TARGET's copy.
        let mut pushed_to: Option<String> = None;
        let drill_dir: PathBuf = match &cfg.target {
            Some(BackupTarget::Agent { url, token }) => {
                let client = BackupClient::new(url, token);
                let push = |e: String| JobFail {
                    msg: format!("push failed: {e}"),
                    snapshot_id: Some(built.clone()),
                    pushed_to: None,
                    destination_missing: false,
                };
                let (pushed, push_stats) = block_on(client.push_snapshot(
                    &local_dir,
                    Some(&cfg.recordings_dir),
                    &wrapping_key,
                ))
                .map_err(&push)?
                .map_err(|e| push(e.to_string()))?;
                debug_assert_eq!(pushed.snapshot_id, receipt.snapshot_id);
                events.push(ok(&format!(
                    "pushed to {} ({} new blob(s), {} already on target)",
                    url, push_stats.uploaded, push_stats.skipped
                )));
                pushed_to = Some(url.clone());

                std::fs::create_dir_all(staging).map_err(|e| JobFail {
                    msg: format!("drill staging: {e}"),
                    snapshot_id: Some(built),
                    pushed_to: pushed_to.clone(),
                    destination_missing: false,
                })?;
                let pull = |e: String| JobFail {
                    msg: format!("re-pull failed: {e}"),
                    snapshot_id: Some(receipt.snapshot_id.clone()),
                    pushed_to: pushed_to.clone(),
                    destination_missing: false,
                };
                let pulled = block_on(client.pull_snapshot(
                    Some(&receipt.snapshot_id),
                    staging,
                    &wrapping_key,
                ))
                .map_err(&pull)?
                .map_err(|e| pull(e.to_string()))?;
                events.push(step("drilling the target's copy (re-pulled + verified)"));
                pulled
            }
            Some(BackupTarget::Folder { path }) => {
                if !path.is_dir() {
                    return Err(JobFail::early_missing(format!(
                        "backup destination not available: {}",
                        path.display()
                    )));
                }
                let (pushed, push_stats) = crate::store::push_to_folder(
                    &local_dir,
                    path,
                    Some(&cfg.recordings_dir),
                    &wrapping_key,
                )
                .map_err(|e| JobFail {
                    msg: format!("push failed: {e}"),
                    snapshot_id: Some(built.clone()),
                    pushed_to: None,
                    destination_missing: false,
                })?;
                debug_assert_eq!(pushed.snapshot_id, receipt.snapshot_id);
                events.push(ok(&format!(
                    "pushed to {} ({} new blob(s), {} already present)",
                    path.display(),
                    push_stats.uploaded,
                    push_stats.skipped
                )));
                pushed_to = Some(path.display().to_string());

                std::fs::create_dir_all(staging).map_err(|e| JobFail {
                    msg: format!("drill staging: {e}"),
                    snapshot_id: Some(built),
                    pushed_to: pushed_to.clone(),
                    destination_missing: false,
                })?;
                let assembled = crate::store::assemble_from_folder(
                    path,
                    Some(&receipt.snapshot_id),
                    staging,
                    &wrapping_key,
                )
                .map_err(|e| JobFail {
                    msg: format!("re-assemble failed: {e}"),
                    snapshot_id: Some(receipt.snapshot_id.clone()),
                    pushed_to: pushed_to.clone(),
                    destination_missing: false,
                })?;
                events.push(step("drilling the folder copy (re-assembled + verified)"));
                assembled
            }
            None => {
                events.push(step("drilling the local snapshot"));
                local_dir.clone()
            }
        };

        // 3. Drill.
        let outcome = drill::run_drill(&drill_dir, &wrapping_key);
        for check in &outcome.checks {
            events.push(ok(check));
        }
        if !outcome.passed {
            for failure in &outcome.failures {
                events.push(fail(failure));
            }
            return Err(JobFail {
                msg: outcome
                    .failures
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "drill failed".into()),
                snapshot_id: Some(receipt.snapshot_id),
                pushed_to,
                destination_missing: false,
            });
        }

        // 4. Local retention. Runs on EVERY successful run — without it a
        // nightly local-only job (a supported configuration) accumulates
        // full copies forever. The keep count floors at 1: never delete the
        // newest local snapshot. (Skipped on drill failure so the suspect
        // copy survives for forensics.)
        let keep = cfg.keep_local.max(1);
        let removed = snapshot::prune_local_snapshots(&out_dir, keep);
        if !removed.is_empty() {
            events.push(step(&format!(
                "local retention: removed {} old snapshot(s)",
                removed.len()
            )));
        }

        // Folder destinations are writable by this machine, so retention
        // there is the writer's job (the agent prunes with its own
        // authority instead).
        if let Some(BackupTarget::Folder { path }) = &cfg.target {
            let pruned = crate::store::prune_folder_store(path, crate::store::DEFAULT_FOLDER_KEEP);
            if !pruned.is_empty() {
                events.push(step(&format!(
                    "folder retention: removed {} old snapshot(s)",
                    pruned.len()
                )));
            }
        }

        Ok(BackupRunStatus {
            last_run_at: chrono::Utc::now(),
            snapshot_id: Some(receipt.snapshot_id),
            drill_passed: true,
            pushed_to,
            failure: None,
            destination_missing: false,
        })
    };

    let status = match run(&mut events, &staging) {
        Ok(s) => s,
        Err(f) => {
            events.push(fail(&f.msg));
            BackupRunStatus {
                last_run_at: chrono::Utc::now(),
                snapshot_id: f.snapshot_id,
                drill_passed: false,
                pushed_to: f.pushed_to,
                failure: Some(f.msg),
                destination_missing: f.destination_missing,
            }
        }
    };
    // Staging cleanup happens pass OR fail (for target runs the durable,
    // restorable copy lives on the TARGET — the local Stream-staged dir
    // holds only the manifest + small always-new blobs by design).
    let _ = std::fs::remove_dir_all(&staging);

    // Status is written even on failure — a red pane beats a stale pane.
    // If the write itself fails, say so loudly instead of vanishing.
    if let Err(e) = status::write_status(&cfg.data_dir, &status) {
        events.push(fail(&format!("status persistence failed: {e}")));
        tracing::warn!(error = %e, "backup status write failed");
    }
    JobOutcome {
        status,
        events,
        skipped: false,
    }
}

// Blocking wrappers over the async client for the sync job runner. Each
// builds a tiny current-thread runtime — legal because the runner is
// called from a plain thread (CLI main or spawn_blocking), never from an
// async worker.
fn block_on<F: std::future::Future>(fut: F) -> Result<F::Output, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(rt.block_on(fut))
}

fn step(line: &str) -> JobEvent {
    JobEvent {
        line: line.into(),
        kind: JobEventKind::Step,
    }
}
fn ok(line: &str) -> JobEvent {
    JobEvent {
        line: line.into(),
        kind: JobEventKind::Ok,
    }
}
fn fail(line: &str) -> JobEvent {
    JobEvent {
        line: line.into(),
        kind: JobEventKind::Fail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::recording::Recording;
    use medical_db::recordings::RecordingsRepo;
    use medical_security::file_crypto;

    fn fixture_db(data: &Path, db_key: [u8; 32]) -> (PathBuf, PathBuf) {
        let db_path = data.join("medical.db");
        let database = medical_db::Database::open(&db_path, Some(db_key)).unwrap();
        {
            let conn = database.conn().unwrap();
            RecordingsRepo::insert(
                &conn,
                &Recording::new("a.enc".to_string(), data.join("a.enc")),
            )
            .unwrap();
        }
        drop(database);
        let recordings = data.join("recordings");
        std::fs::create_dir_all(&recordings).unwrap();
        let wav_key = file_crypto::derive_file_key(&db_key);
        std::fs::write(
            recordings.join("a.enc"),
            file_crypto::encrypt_bytes_with_key(&wav_key, b"RIFF audio").unwrap(),
        )
        .unwrap();
        (db_path, recordings)
    }

    #[test]
    fn job_runs_local_backup_and_writes_passing_status() {
        let data = tempfile::tempdir().unwrap();
        let db_key = [0x31u8; 32];
        let wrapping = [0x42u8; 32];
        let (db_path, recordings) = fixture_db(data.path(), db_key);

        let cfg = JobConfig {
            data_dir: data.path().to_path_buf(),
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            target: None,
            keep_local: 14,
        };
        let outcome = run_backup_job(&cfg, db_key, wrapping);
        assert!(outcome.success(), "events: {:?}", outcome.events);
        assert!(!outcome.skipped);
        assert!(outcome.status.snapshot_id.is_some());
        let persisted = status::read_status(data.path()).unwrap();
        assert!(persisted.drill_passed);
        assert!(persisted.snapshot_id.is_some());

        // The lock is released when the job finishes.
        assert!(
            !data.path().join(LOCK_FILE).exists(),
            "lock must be released"
        );
    }

    #[test]
    fn local_only_runs_prune_to_keep_local() {
        let data = tempfile::tempdir().unwrap();
        let db_key = [0x91u8; 32];
        let (db_path, recordings) = fixture_db(data.path(), db_key);

        // Two consecutive local-only runs with keep_local = 1: the second
        // run's retention must remove the first snapshot — otherwise a
        // nightly local-only job accumulates full copies forever.
        for _ in 0..2 {
            let cfg = JobConfig {
                data_dir: data.path().to_path_buf(),
                db_path: db_path.clone(),
                recordings_dir: recordings.clone(),
                keystore_path: None,
                target: None,
                keep_local: 1,
            };
            let outcome = run_backup_job(&cfg, db_key, [0xA2u8; 32]);
            assert!(outcome.success(), "events: {:?}", outcome.events);
        }

        let backups = data.path().join("backups");
        let dirs: Vec<String> = std::fs::read_dir(&backups)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            dirs.len(),
            1,
            "local-only retention must keep exactly 1 snapshot: {dirs:?}"
        );
    }

    #[test]
    fn job_failure_writes_failing_status() {
        let data = tempfile::tempdir().unwrap();
        let cfg = JobConfig {
            data_dir: data.path().to_path_buf(),
            db_path: data.path().join("does-not-exist.db"),
            recordings_dir: data.path().join("recordings"),
            keystore_path: None,
            target: None,
            keep_local: 14,
        };
        let outcome = run_backup_job(&cfg, [1u8; 32], [2u8; 32]);
        assert!(!outcome.success());
        assert!(outcome.status.failure.is_some());
        let persisted = status::read_status(data.path()).unwrap();
        assert!(!persisted.drill_passed);
        assert!(persisted.failure.is_some());
    }

    #[test]
    fn overlapping_job_is_skipped_and_leaves_status_untouched() {
        let data = tempfile::tempdir().unwrap();
        // Pre-existing status representing the RUNNING job's last result.
        let prior = BackupRunStatus {
            last_run_at: chrono::Utc::now(),
            snapshot_id: Some("snap-prior".into()),
            drill_passed: true,
            pushed_to: None,
            failure: None,
            destination_missing: false,
        };
        status::write_status(data.path(), &prior).unwrap();
        // Simulate a live holder: fresh lock file.
        std::fs::write(data.path().join(LOCK_FILE), "held").unwrap();

        let (db_path, recordings) = fixture_db(data.path(), [0x51u8; 32]);
        let cfg = JobConfig {
            data_dir: data.path().to_path_buf(),
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            target: None,
            keep_local: 14,
        };
        let outcome = run_backup_job(&cfg, [0x51u8; 32], [0x62u8; 32]);
        assert!(outcome.skipped);
        assert!(!outcome.success());
        // The prior status is untouched — a skipped run must not clobber
        // the running job's record with a red "concurrent run" entry.
        let persisted = status::read_status(data.path()).unwrap();
        assert_eq!(persisted.snapshot_id.as_deref(), Some("snap-prior"));
        assert!(persisted.drill_passed);
    }

    #[test]
    fn job_with_folder_target_pushes_and_drills_end_to_end() {
        let data = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let db_key = [0x31u8; 32];
        let wrapping = [0x42u8; 32];
        let (db_path, recordings) = fixture_db(data.path(), db_key);

        let cfg = JobConfig {
            data_dir: data.path().to_path_buf(),
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            target: Some(BackupTarget::Folder {
                path: store.path().to_path_buf(),
            }),
            keep_local: 14,
        };
        let outcome = run_backup_job(&cfg, db_key, wrapping);
        assert!(outcome.success(), "events: {:?}", outcome.events);
        assert_eq!(
            outcome.status.pushed_to.as_deref(),
            Some(store.path().to_string_lossy().as_ref())
        );
        assert!(!outcome.status.destination_missing);
        // The store holds exactly one committed snapshot.
        assert_eq!(
            crate::store::list_folder_snapshots(store.path())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn job_with_missing_folder_destination_reports_destination_missing() {
        let data = tempfile::tempdir().unwrap();
        let db_key = [0x51u8; 32];
        let (db_path, recordings) = fixture_db(data.path(), db_key);
        let missing = data.path().join("no-such-drive");

        let cfg = JobConfig {
            data_dir: data.path().to_path_buf(),
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            target: Some(BackupTarget::Folder { path: missing }),
            keep_local: 14,
        };
        let outcome = run_backup_job(&cfg, db_key, [0x62u8; 32]);
        assert!(!outcome.success());
        assert!(outcome.status.destination_missing);
        assert!(
            outcome
                .status
                .failure
                .as_deref()
                .is_some_and(|f| f.contains("not available"))
        );
        // The status file records it for the pane.
        let persisted = status::read_status(data.path()).unwrap();
        assert!(persisted.destination_missing);
    }

    #[test]
    fn stale_lock_is_stolen() {
        let data = tempfile::tempdir().unwrap();
        let lock = data.path().join(LOCK_FILE);
        std::fs::write(&lock, "crashed holder").unwrap();
        // Age it past the steal threshold.
        let old =
            std::time::SystemTime::now() - LOCK_STALE_AFTER - std::time::Duration::from_secs(60);
        std::fs::File::open(&lock)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let (db_path, recordings) = fixture_db(data.path(), [0x71u8; 32]);
        let cfg = JobConfig {
            data_dir: data.path().to_path_buf(),
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            target: None,
            keep_local: 14,
        };
        let outcome = run_backup_job(&cfg, [0x71u8; 32], [0x82u8; 32]);
        assert!(!outcome.skipped, "stale lock must be stolen");
        assert!(outcome.success(), "events: {:?}", outcome.events);
    }
}
