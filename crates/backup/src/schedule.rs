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
use std::path::PathBuf;

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
        // Local-only config drops url/token args.
        let mut local = cfg();
        local.url = String::new();
        let args = schedule_args(&local);
        assert!(!args.contains(&"--url".to_string()));
    }
}
