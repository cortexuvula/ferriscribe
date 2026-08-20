//! Machine-readable run status (`backup-status.json` in the app data
//! dir), written by every scheduled/manual backup job and read by the
//! app's Settings → Backup pane to surface last-run recency and drill
//! health. Counts, ids, timestamps only — never PHI.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::BackupResult;

/// Filename of the status file inside the app data dir.
pub const STATUS_FILE: &str = "backup-status.json";
/// A run older than this (or absent) shows as stale/red in the UI.
pub const STALE_AFTER: chrono::Duration = chrono::Duration::hours(48);

/// Result of one backup job run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRunStatus {
    pub last_run_at: DateTime<Utc>,
    /// Snapshot id when one was built (`None` on pre-build failure).
    pub snapshot_id: Option<String>,
    /// The restore drill verdict for this run.
    pub drill_passed: bool,
    /// Target URL the snapshot was pushed to (`None` = local-only).
    pub pushed_to: Option<String>,
    /// First failure line when anything failed (PHI-free).
    pub failure: Option<String>,
}

impl BackupRunStatus {
    /// True when this run is older than [`STALE_AFTER`] — the pane's red
    /// condition (drill_passed == false is the other one).
    pub fn is_stale(&self) -> bool {
        Utc::now() - self.last_run_at > STALE_AFTER
    }
}

/// Write the status file into `data_dir` (atomic temp + rename so a
/// crash mid-write never corrupts the previous status).
pub fn write_status(data_dir: &Path, status: &BackupRunStatus) -> BackupResult<()> {
    let path = data_dir.join(STATUS_FILE);
    let tmp = data_dir.join(format!("{STATUS_FILE}.tmp"));
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&tmp, serde_json::to_vec_pretty(status)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the status file; `None` when absent or unreadable (treated as
/// "never ran" by callers).
pub fn read_status(data_dir: &Path) -> Option<BackupRunStatus> {
    std::fs::read(data_dir.join(STATUS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrip_and_staleness() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_status(dir.path()).is_none(), "absent → None");

        let fresh = BackupRunStatus {
            last_run_at: Utc::now(),
            snapshot_id: Some("snap-x".into()),
            drill_passed: true,
            pushed_to: Some("http://t".into()),
            failure: None,
        };
        write_status(dir.path(), &fresh).unwrap();
        let back = read_status(dir.path()).unwrap();
        assert_eq!(back.snapshot_id.as_deref(), Some("snap-x"));
        assert!(back.drill_passed);
        assert!(!back.is_stale(), "just ran — not stale");

        let old = BackupRunStatus {
            last_run_at: Utc::now() - chrono::Duration::hours(49),
            snapshot_id: None,
            drill_passed: false,
            pushed_to: None,
            failure: Some("verification failed: HMAC mismatch".into()),
        };
        write_status(dir.path(), &old).unwrap();
        let back = read_status(dir.path()).unwrap();
        assert!(back.is_stale(), "49h old → stale");
        assert!(!back.drill_passed);
    }
}
