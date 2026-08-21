//! Scheduler installation (R2): the backup must fire even when the
//! FerriScribe app is closed, crashed, or held hostage — so the trigger
//! lives in the OS, not the app.
//!
//! macOS: a launchd LaunchAgent (`StartCalendarInterval`) invoking the
//! standalone `ferriscribe-backup` binary. The app never holds the only
//! trigger; it only (optionally) installs/updates the agent.
//!
//! Linux: no plist is generated; see the README for the equivalent
//! systemd timer (OnCalendar=) — the same binary, the same arguments.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::BackupResult;

pub const LAUNCHD_LABEL: &str = "com.cortexuvula.ferriscribe.backup";

/// Configuration for the scheduled job.
pub struct ScheduleConfig {
    /// Absolute path to the ferriscribe-backup binary.
    pub binary_path: PathBuf,
    /// Cron-like daily fire time.
    pub hour: u32,
    pub minute: u32,
    /// Backup target agent URL (may be empty for local-only snapshots).
    pub url: String,
    /// Append token for the target agent.
    pub token: String,
    /// Where snapshot staging dirs are written.
    pub snapshots_dir: PathBuf,
    /// Recordings directory resolved AT INSTALL TIME from the app's
    /// configured storage path. Baked into the plist because the
    /// scheduled CLI run must not need to open the (encrypted) DB to
    /// discover it — without this, custom-folder installs silently
    /// backed up an empty default recordings dir.
    pub recordings_dir: PathBuf,
    /// Log files for launchd to capture stdout/stderr into.
    pub log_dir: PathBuf,
}

/// Render the launchd plist for the given config (pub for tests).
pub fn render_plist(cfg: &ScheduleConfig) -> String {
    let args = {
        let mut items = String::new();
        for arg in schedule_args(cfg) {
            items.push_str(&format!("        <string>{}</string>\n", xml_escape(arg)));
        }
        items
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
             <key>Label</key>\n\
             <string>{LAUNCHD_LABEL}</string>\n\
             <key>ProgramArguments</key>\n\
             <array>\n\
         {args}\
             </array>\n\
             <key>StartCalendarInterval</key>\n\
             <dict>\n\
                 <key>Hour</key>\n\
                 <integer>{hour}</integer>\n\
                 <key>Minute</key>\n\
                 <integer>{minute}</integer>\n\
             </dict>\n\
             <key>RunAtLoad</key>\n\
             <false/>\n\
             <key>StandardOutPath</key>\n\
             <string>{stdout_log}</string>\n\
             <key>StandardErrorPath</key>\n\
             <string>{stderr_log}</string>\n\
         </dict>\n\
         </plist>\n",
        args = args,
        hour = cfg.hour,
        minute = cfg.minute,
        stdout_log = xml_escape(cfg.log_dir.join("backup-out.log").to_string_lossy()),
        stderr_log = xml_escape(cfg.log_dir.join("backup-err.log").to_string_lossy()),
    )
}

/// The argument vector the scheduled job runs with. Extracted for tests
/// and for `install-schedule --dry-run` display.
pub fn schedule_args(cfg: &ScheduleConfig) -> Vec<String> {
    let mut args = vec![
        cfg.binary_path.to_string_lossy().into_owned(),
        "backup-and-push".to_string(),
        "--out".to_string(),
        cfg.snapshots_dir.to_string_lossy().into_owned(),
        "--recordings-dir".to_string(),
        cfg.recordings_dir.to_string_lossy().into_owned(),
    ];
    if !cfg.url.is_empty() {
        args.push("--url".into());
        args.push(cfg.url.clone());
        args.push("--token".into());
        args.push(cfg.token.clone());
    }
    args
}

/// Default plist location: `~/Library/LaunchAgents/<label>.plist`.
pub fn plist_path() -> BackupResult<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| crate::BackupError::Escrow("HOME is not set".into()))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

/// Write the plist and (best-effort) load it with launchctl. Returns the
/// plist path so the CLI can tell the user where it landed.
pub fn install(cfg: &ScheduleConfig) -> BackupResult<PathBuf> {
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&cfg.log_dir)?;
    std::fs::create_dir_all(&cfg.snapshots_dir)?;
    let mut f = std::fs::File::create(&path)?;
    f.write_all(render_plist(cfg).as_bytes())?;
    f.sync_all()?;

    // Best-effort load; if launchctl is missing (CI, Linux) the plist is
    // still correct for the user to load manually.
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .status();
    let _ = std::process::Command::new("launchctl")
        .args(["load", &path.to_string_lossy()])
        .status();
    Ok(path)
}

/// Resolve the sidecar binary bundled next to the app executable
/// (`FerriScribe.app/Contents/MacOS/ferriscribe-backup` on macOS). Returns
/// `None` when not running from a bundle with the sidecar present (dev
/// builds without `npm run build:sidecar`, or an installed app predating
/// the sidecar).
pub fn bundled_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join("ferriscribe-backup");
    sibling.is_file().then_some(sibling)
}

/// Copy the bundled sidecar into `dest_dir` and return the stable path.
/// The stable copy is what the launchd plist points at — it survives the
/// app being moved, updated, or dragged to the trash, none of which are
/// true of the path inside the .app bundle. Re-copies when the bundled
/// binary's content hash differs (app update shipped a newer tool);
/// returns the existing path unchanged when already current. Also copies
/// when the stable copy is missing the executable bit (partial install).
pub fn ensure_binary_copy(dest_dir: &Path) -> BackupResult<PathBuf> {
    let bundled = bundled_binary_path().ok_or_else(|| {
        crate::BackupError::Setup(
            "ferriscribe-backup sidecar not found next to the app executable".into(),
        )
    })?;
    copy_binary_if_changed(&bundled, dest_dir)
}

/// Testable core of [`ensure_binary_copy`]: copy `bundled` into `dest_dir`
/// when missing, non-executable, or content-different from what's there.
/// ATOMIC: copies to a temp sibling, fsyncs, marks executable, then
/// renames — a crash mid-copy can never leave a truncated binary for
/// launchd to execute.
pub(crate) fn copy_binary_if_changed(bundled: &Path, dest_dir: &Path) -> BackupResult<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let dest = dest_dir.join("ferriscribe-backup");
    let src_hash = file_hash(bundled)?;
    let needs_copy = match std::fs::read(&dest) {
        Ok(existing) => {
            let dest_hash = sha256_hex(&existing);
            dest_hash != src_hash || !is_executable(&dest)
        }
        Err(_) => true,
    };
    if needs_copy {
        let tmp = dest.with_extension("tmp");
        std::fs::copy(bundled, &tmp)?;
        // fsync before rename so the published bytes are durable, and set
        // the exec bit on the TEMP file so the rename lands an executable
        // atomically. A permissions failure here must NOT be swallowed —
        // a non-executable binary means every scheduled run fails.
        std::fs::File::open(&tmp)?.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, &dest)?;
    }
    Ok(dest)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

fn file_hash(path: &Path) -> BackupResult<String> {
    Ok(sha256_hex(&std::fs::read(path)?))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Remove the plist and (best-effort) unload the agent. Idempotent.
pub fn uninstall() -> BackupResult<()> {
    let path = plist_path()?;
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .status();
    let _ = std::fs::remove_file(&path);
    Ok(())
}

fn xml_escape(s: impl AsRef<str>) -> String {
    s.as_ref()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ScheduleConfig {
        ScheduleConfig {
            binary_path: PathBuf::from("/usr/local/bin/ferriscribe-backup"),
            hour: 3,
            minute: 30,
            url: "http://100.64.0.2:8741".into(),
            token: "tok&<xml>".into(),
            snapshots_dir: PathBuf::from("/tmp/snaps"),
            recordings_dir: PathBuf::from("/Volumes/Audio/recordings"),
            log_dir: PathBuf::from("/tmp/logs"),
        }
    }

    #[test]
    fn plist_contains_label_schedule_and_escaped_args() {
        let plist = render_plist(&cfg());
        assert!(plist.contains(&format!("<string>{LAUNCHD_LABEL}</string>")));
        assert!(plist.contains("<integer>3</integer>"), "hour");
        assert!(plist.contains("<integer>30</integer>"), "minute");
        assert!(plist.contains("backup-and-push"));
        // The token contains XML specials — must be escaped.
        assert!(plist.contains("tok&amp;&lt;xml&gt;"), "token escaped");
        assert!(!plist.contains("tok&<xml>"), "no raw specials");
    }

    #[test]
    fn schedule_args_shape() {
        let args = schedule_args(&cfg());
        assert_eq!(args[1], "backup-and-push");
        assert!(args.contains(&"--url".to_string()));
        // The resolved recordings dir is ALWAYS baked in — the scheduled
        // run must not guess (custom storage-path installs).
        let idx = args.iter().position(|a| a == "--recordings-dir").unwrap();
        assert_eq!(args[idx + 1], "/Volumes/Audio/recordings");
        // Local-only config drops url/token args.
        let mut local = cfg();
        local.url = String::new();
        let args = schedule_args(&local);
        assert!(!args.contains(&"--url".to_string()));
    }

    #[test]
    fn copy_binary_if_changed_copies_missing_then_skips_unchanged() {
        let src_dir = tempfile::TempDir::new().unwrap();
        let bundled = src_dir.path().join("ferriscribe-backup");
        std::fs::write(&bundled, b"tool-bytes-v1").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bundled, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let dest_dir = tempfile::TempDir::new().unwrap();
        let dest = copy_binary_if_changed(&bundled, dest_dir.path()).unwrap();
        assert!(dest.is_file());
        // Atomic copy leaves no temp sibling behind.
        assert!(
            !dest.with_extension("tmp").exists(),
            "no .tmp residue after copy"
        );

        // Idempotent: same content → no error, path unchanged.
        let dest2 = copy_binary_if_changed(&bundled, dest_dir.path()).unwrap();
        assert_eq!(dest, dest2);

        // A NEWER bundled tool (different bytes) replaces the stale copy —
        // the "app update shipped a newer backup tool" path.
        std::fs::write(&bundled, b"tool-bytes-v2").unwrap();
        copy_binary_if_changed(&bundled, dest_dir.path()).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"tool-bytes-v2");
    }

    #[test]
    fn copy_binary_if_changed_restores_missing_exec_bit() {
        // A copy written without the exec bit (partial install / manual
        // copy) must be repaired even when the bytes match.
        let src_dir = tempfile::TempDir::new().unwrap();
        let bundled = src_dir.path().join("ferriscribe-backup");
        std::fs::write(&bundled, b"tool-bytes").unwrap();
        let dest_dir = tempfile::TempDir::new().unwrap();
        let dest = dest_dir.path().join("ferriscribe-backup");
        std::fs::write(&dest, b"tool-bytes").unwrap(); // same bytes, no +x

        copy_binary_if_changed(&bundled, dest_dir.path()).unwrap();
        assert!(is_executable(&dest), "exec bit must be restored");
    }
}
