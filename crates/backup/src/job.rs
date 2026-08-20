//! The shared backup job: build a snapshot, push it (when configured),
//! drill the TARGET's re-pulled copy, apply local retention, and persist
//! a machine-readable status. Used by BOTH the CLI (`backup-and-push`,
//! what launchd invokes) and the app's "Back up now" button — one code
//! path, so the button exercises exactly what the schedule runs.

use std::path::PathBuf;

use crate::client::BackupClient;
use crate::drill;
use crate::snapshot::{self, BuildOptions};
use crate::status::{self, BackupRunStatus};

/// Where the job reads/writes everything. `data_dir` is the app data root
/// (status file + local `backups/` staging live there).
pub struct JobConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub recordings_dir: PathBuf,
    pub keystore_path: Option<PathBuf>,
    /// Target agent URL + append token; `None` = local-only snapshot.
    pub target: Option<(String, String)>,
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

/// Outcome of a run — `status` is ALWAYS written to disk, success or not.
pub struct JobOutcome {
    pub status: BackupRunStatus,
    pub events: Vec<JobEvent>,
}

impl JobOutcome {
    pub fn success(&self) -> bool {
        self.status.failure.is_none() && self.status.drill_passed
    }
}

/// Run the full backup job. Never panics; failures become the status's
/// `failure` line plus a `Fail` event.
///
/// Synchronous BY DESIGN: the CLI calls it from a plain main, and the
/// app's command wraps it in `tokio::task::spawn_blocking` (calling it
/// directly from an async worker would panic on the nested `block_on`).
pub fn run_backup_job(cfg: &JobConfig, db_key: [u8; 32], wrapping_key: [u8; 32]) -> JobOutcome {
    let mut events = Vec::new();
    let run = |events: &mut Vec<JobEvent>| -> Result<BackupRunStatus, String> {
        let out_dir = cfg.data_dir.join("backups");
        std::fs::create_dir_all(&out_dir).map_err(|e| format!("staging dir: {e}"))?;

        // 1. Build.
        events.push(step("building snapshot…"));
        let receipt = snapshot::build_snapshot(&BuildOptions {
            db_path: cfg.db_path.clone(),
            recordings_dir: cfg.recordings_dir.clone(),
            keystore_path: cfg.keystore_path.clone(),
            dest_dir: out_dir.clone(),
            db_key,
            wrapping_key,
        })
        .map_err(|e| format!("snapshot build failed: {e}"))?;
        events.push(ok(&format!(
            "snapshot {} built ({} bytes)",
            receipt.snapshot_id, receipt.total_bytes
        )));
        let local_dir = out_dir.join(&receipt.snapshot_id);

        // 2. Push + drill the TARGET's copy.
        let mut pushed_to = None;
        let drill_dir: PathBuf = match &cfg.target {
            Some((url, token)) => {
                let client = BackupClient::new(url, token);
                let pushed = block_on(client.push_snapshot(&local_dir))
                    .map_err(|e| format!("push failed: {e}"))?
                    .map_err(|e| format!("push failed: {e}"))?;
                debug_assert_eq!(pushed.snapshot_id, receipt.snapshot_id);
                events.push(ok(&format!("pushed to {url}")));
                pushed_to = Some(url.clone());

                // Local retention only after a successful push; the drill
                // below still runs on the pulled copy, and a drill failure
                // keeps everything for forensics — so retention happens
                // only if the drill passes (end of this closure).
                let staging = std::env::temp_dir().join(format!(
                    "ferriscribe-postpush-drill-{}",
                    uuid::Uuid::new_v4().simple()
                ));
                std::fs::create_dir_all(&staging).map_err(|e| format!("drill staging: {e}"))?;
                let pulled = block_on(client.pull_snapshot(
                    Some(&receipt.snapshot_id),
                    &staging,
                    &wrapping_key,
                ))
                .map_err(|e| format!("re-pull failed: {e}"))?
                .map_err(|e| format!("re-pull failed: {e}"))?;
                events.push(step("drilling the target's copy (re-pulled + verified)"));
                pulled
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
            return Err(outcome
                .failures
                .first()
                .cloned()
                .unwrap_or_else(|| "drill failed".into()));
        }

        // 4. Local retention (push path only — local-only runs keep the
        // only copy there is).
        if cfg.target.is_some() {
            let removed = snapshot::prune_local_snapshots(&out_dir, cfg.keep_local);
            if !removed.is_empty() {
                events.push(step(&format!(
                    "local retention: removed {} old snapshot(s)",
                    removed.len()
                )));
            }
        }

        Ok(BackupRunStatus {
            last_run_at: chrono::Utc::now(),
            snapshot_id: Some(receipt.snapshot_id),
            drill_passed: true,
            pushed_to,
            failure: None,
        })
    };

    let status = match run(&mut events) {
        Ok(s) => s,
        Err(failure) => {
            events.push(fail(&failure));
            BackupRunStatus {
                last_run_at: chrono::Utc::now(),
                snapshot_id: None,
                drill_passed: false,
                pushed_to: None,
                failure: Some(failure),
            }
        }
    };
    // Status is written even on failure — a red pane beats a stale pane.
    let _ = status::write_status(&cfg.data_dir, &status);
    JobOutcome { status, events }
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

    #[test]
    fn job_runs_local_backup_and_writes_passing_status() {
        let data = tempfile::tempdir().unwrap();
        let db_key = [0x31u8; 32];
        let wrapping = [0x42u8; 32];

        // Fixture: real SQLCipher DB + one encrypted recording.
        let db_path = data.path().join("medical.db");
        let database = medical_db::Database::open(&db_path, Some(db_key)).unwrap();
        {
            let conn = database.conn().unwrap();
            RecordingsRepo::insert(
                &conn,
                &Recording::new("a.enc".to_string(), data.path().join("a.enc")),
            )
            .unwrap();
        }
        drop(database);
        let recordings = data.path().join("recordings");
        std::fs::create_dir_all(&recordings).unwrap();
        let wav_key = file_crypto::derive_file_key(&db_key);
        std::fs::write(
            recordings.join("a.enc"),
            file_crypto::encrypt_bytes_with_key(&wav_key, b"RIFF audio").unwrap(),
        )
        .unwrap();

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
        assert!(outcome.status.snapshot_id.is_some());
        // The status file is on disk and says the drill passed.
        let persisted = status::read_status(data.path()).unwrap();
        assert!(persisted.drill_passed);
        assert!(persisted.pushed_to.is_none());
        assert!(persisted.snapshot_id.is_some());
    }

    #[test]
    fn job_failure_writes_failing_status() {
        // Point the job at a nonexistent DB — it must fail CLOSED and
        // persist a failing status (never leave a stale green pane).
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
}
