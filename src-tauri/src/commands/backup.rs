//! Backup UI commands — thin, in-process wrappers over the `medical-backup`
//! LIBRARY (never the sidecar binary: the app configures, launchd fires).
//! One exception: `install-schedule` stages the bundled sidecar to a stable
//! path, because the plist needs an executable that outlives the app bundle.

use std::path::PathBuf;

use medical_backup::{escrow, job, keys, schedule, status};
use serde::Serialize;
use tauri::Emitter;
use tracing::warn;

use medical_core::error::{AppError, AppResult};
use medical_core::types::settings::SecretString;
use medical_db::settings::SettingsRepo;
use medical_security::keychain;

use crate::state::AppState;

fn backup_err(e: medical_backup::BackupError) -> AppError {
    AppError::Config(format!("backup: {e}"))
}

/// Everything the Settings → Backup pane renders. Counts/timestamps/bools
/// only — no PHI, no key material.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub ever_ran: bool,
    pub last_run_at: Option<String>,
    pub snapshot_id: Option<String>,
    pub drill_passed: bool,
    pub stale: bool,
    pub failure: Option<String>,
    pub pushed_to: Option<String>,
    /// Escrow bootstrap state: wrapping key exists in the keychain.
    pub wrapping_key_present: bool,
    /// launchd agent plist exists (macOS; always false elsewhere).
    pub schedule_installed: bool,
    /// The stable binary copy exists and matches the bundled sidecar
    /// (or simply exists when no sidecar is bundled, e.g. dev builds).
    pub tool_copy_ok: bool,
    /// Scheduling is macOS-only in this build (launchd).
    pub schedule_supported: bool,
    /// "agent" | "folder" | "local-only"
    pub destination_kind: String,
    /// For folder destinations: is the folder attached right now?
    pub destination_present: bool,
    /// The last run failed because the destination folder was missing.
    pub destination_missing: bool,
}

/// Load the persisted AppConfig (migrated) off the async worker.
async fn load_config(state: &AppState) -> AppResult<medical_core::types::settings::AppConfig> {
    let db = std::sync::Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<_> {
        let conn = db.conn()?;
        let mut config = SettingsRepo::load_config(&conn)?;
        config.migrate();
        Ok(config)
    })
    .await
    .map_err(|e| AppError::Config(format!("config task: {e}")))?
}

/// Resolve the recordings dir: the configured storage path when set,
/// otherwise inside the app data dir. Mirrors how the app stores audio.
fn recordings_dir_for(
    config: &medical_core::types::settings::AppConfig,
    data_dir: &std::path::Path,
) -> PathBuf {
    match config.storage_path.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => data_dir.join("recordings"),
    }
}

#[tauri::command]
pub async fn backup_status(state: tauri::State<'_, AppState>) -> AppResult<BackupStatus> {
    let config = load_config(&state).await?;
    let has_agent = config
        .backup_target_url
        .as_deref()
        .is_some_and(|u| !u.is_empty())
        && config.backup_append_token.is_some();
    let destination_kind = if has_agent {
        "agent"
    } else if config
        .backup_dest_path
        .as_deref()
        .is_some_and(|p| !p.is_empty())
    {
        "folder"
    } else {
        "local-only"
    }
    .to_string();
    let dest_path = config.backup_dest_path.filter(|p| !p.is_empty());
    let data_dir = state.data_dir.clone();
    // Everything below touches the keychain (which can prompt) and the
    // filesystem (tens-of-MB sidecar) — one blocking thread, not the
    // async worker (review: runtime starvation on every pane refresh).
    let (run, wrapping_key_present, schedule_installed, tool_copy_ok, destination_present) =
        tokio::task::spawn_blocking(move || {
            let run = status::read_status(&data_dir);
            let wrapping_key_present = keychain::get_secret(keychain::KEYCHAIN_BACKUP_KEY_ACCOUNT)
                .map(|k| k.is_some())
                .unwrap_or(false);
            let schedule_installed =
                cfg!(target_os = "macos") && schedule::plist_path().is_ok_and(|p| p.exists());
            // Tool-copy freshness: size + mtime compare. This is a UI
            // HINT — the authoritative copy/repair happens in
            // ensure_binary_copy at install time, so an approximation is
            // fine here and avoids hashing two binaries per refresh.
            let stable = data_dir.join("bin").join("ferriscribe-backup");
            let tool_copy_ok = match schedule::bundled_binary_path() {
                Some(bundled) => {
                    metadata_of(&stable)
                        .zip(metadata_of(&bundled))
                        .is_some_and(|(s, b)| {
                            s.len() == b.len() && s.modified().ok() == b.modified().ok()
                        })
                }
                None => stable.is_file(),
            };
            let destination_present = dest_path
                .as_deref()
                .is_some_and(|p| std::path::Path::new(p).is_dir());
            (
                run,
                wrapping_key_present,
                schedule_installed,
                tool_copy_ok,
                destination_present,
            )
        })
        .await
        .map_err(|e| AppError::Config(format!("status task: {e}")))?;

    Ok(BackupStatus {
        ever_ran: run.is_some(),
        stale: run.as_ref().is_none_or(|r| r.is_stale()),
        last_run_at: run.as_ref().map(|r| r.last_run_at.to_rfc3339()),
        snapshot_id: run.as_ref().and_then(|r| r.snapshot_id.clone()),
        drill_passed: run.as_ref().is_some_and(|r| r.drill_passed),
        failure: run.as_ref().and_then(|r| r.failure.clone()),
        pushed_to: run.as_ref().and_then(|r| r.pushed_to.clone()),
        wrapping_key_present,
        schedule_installed,
        tool_copy_ok,
        schedule_supported: cfg!(target_os = "macos"),
        destination_kind,
        destination_present,
        destination_missing: run.as_ref().is_some_and(|r| r.destination_missing),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowArtifacts {
    pub sheet_path: String,
    pub usb_path: String,
}

/// Generate (first run) or re-emit the escrow artifacts into `out_dir`.
/// The frontend MUST tell the user to print the sheet and copy the USB
/// file — a key in the keychain without escrow copies is NOT off-machine.
#[tauri::command]
pub async fn backup_escrow_init(out_dir: String) -> AppResult<EscrowArtifacts> {
    // Expand a leading `~` (the UI placeholder suggests ~/Desktop), then
    // validate BEFORE writing key material: the path must resolve to an
    // absolute, existing directory — never scatter recovery artifacts
    // relative to the process CWD.
    let expanded = expand_tilde(&out_dir);
    let out = crate::commands::validate_user_path(&expanded.to_string_lossy())?;
    if !out.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "not a directory: {}",
            out.display()
        )));
    }
    // Keychain access can prompt; keep it off the async worker.
    let wrapping = tokio::task::spawn_blocking(keys::load_or_create_wrapping_key)
        .await
        .map_err(|e| AppError::Config(format!("escrow task: {e}")))?
        .map_err(backup_err)?;
    let sheet = out.join(escrow::SHEET_FILENAME);
    let usb = out.join(escrow::USB_FILENAME);
    escrow::write_recovery_sheet(&sheet, &wrapping)
        .map_err(|e| AppError::Config(format!("escrow: {e}")))?;
    escrow::write_usb_file(&usb, &wrapping)
        .map_err(|e| AppError::Config(format!("escrow: {e}")))?;
    Ok(EscrowArtifacts {
        sheet_path: sheet.to_string_lossy().into_owned(),
        usb_path: usb.to_string_lossy().into_owned(),
    })
}

/// Verify an escrow artifact (sheet or USB). Returns a status message.
#[tauri::command]
pub async fn backup_escrow_verify(file: String) -> AppResult<String> {
    let path = PathBuf::from(&file);
    let expected = tokio::task::spawn_blocking(keys::load_wrapping_key)
        .await
        .map_err(|e| AppError::Config(format!("escrow task: {e}")))?
        .ok();
    tokio::task::spawn_blocking(move || escrow::verify_artifact(&path, expected.as_ref()))
        .await
        .map_err(|e| AppError::Config(format!("escrow task: {e}")))?
        .map_err(backup_err)
}

/// Expand a leading `~/` to the user's home directory.
fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(input)
}

fn metadata_of(path: &std::path::Path) -> Option<std::fs::Metadata> {
    std::fs::metadata(path).ok()
}

/// The persisted, effective backup destination.
#[derive(Debug, Clone, PartialEq)]
enum EffectiveDestination {
    LocalOnly,
    Agent { url: String, token: String },
    Folder { path: String },
}

/// Merge install-time destination args into the persisted config and
/// validate. `None` KEEPS the stored value (the UI never re-populates
/// the token field, so a time-only reinstall must not erase the
/// credential); a URL without any token (passed or stored) is rejected —
/// the scheduled job would otherwise push with an empty credential,
/// failing unattended. Exactly one destination shape may be configured;
/// choosing the other side clears the stale one (last choice wins).
fn merge_destination(
    config: &mut medical_core::types::settings::AppConfig,
    url: Option<String>,
    token: Option<String>,
    dest: Option<String>,
) -> AppResult<EffectiveDestination> {
    if url.is_some() && dest.is_some() {
        return Err(AppError::InvalidInput(
            "choose one destination: the backup server (URL + token) or a folder — not both".into(),
        ));
    }
    // An actively-passed choice wins over a stale stored one of the other
    // kind (last choice wins); the loser's fields are cleared so the
    // persisted config can never hold both.
    if url.is_some() {
        config.backup_dest_path = None;
        config.backup_target_url = url;
    }
    if token.is_some() {
        config.backup_append_token = token.map(SecretString);
    }
    if dest.is_some() {
        config.backup_target_url = None;
        config.backup_append_token = None;
        config.backup_dest_path = dest;
    }
    let eff_url = config.backup_target_url.clone().unwrap_or_default();
    let eff_token = config
        .backup_append_token
        .as_ref()
        .map(|t| t.0.clone())
        .unwrap_or_default();
    let eff_dest = config.backup_dest_path.clone().unwrap_or_default();
    if !eff_url.is_empty() && !eff_token.is_empty() && !eff_dest.is_empty() {
        // Only reachable when BOTH kinds linger in stored config and the
        // call passed neither — an ambiguous state no UI flow creates.
        return Err(AppError::InvalidInput(
            "choose one destination: the backup server (URL + token) or a folder — not both".into(),
        ));
    }
    if !eff_url.is_empty() && eff_token.is_empty() {
        return Err(AppError::InvalidInput(
            "a target URL needs an append token — paste the target's \
             FERRISCRIBE_BACKUP_APPEND_TOKEN"
                .into(),
        ));
    }
    if !eff_url.is_empty() {
        // Switching to the agent clears a stale folder destination.
        config.backup_dest_path = None;
        Ok(EffectiveDestination::Agent {
            url: eff_url,
            token: eff_token,
        })
    } else if !eff_dest.is_empty() {
        // Switching to a folder clears stale agent credentials.
        config.backup_target_url = None;
        config.backup_append_token = None;
        Ok(EffectiveDestination::Folder { path: eff_dest })
    } else {
        Ok(EffectiveDestination::LocalOnly)
    }
}

/// Install the daily launchd agent (macOS only). Stages the bundled
/// sidecar to the stable `bin/` copy the plist points at, persists the
/// target URL + append token to AppConfig (encrypted DB) so "Back up now"
/// reuses them, then writes + loads the plist.
#[tauri::command]
pub async fn backup_install_schedule(
    state: tauri::State<'_, AppState>,
    hour: u32,
    minute: u32,
    url: Option<String>,
    token: Option<String>,
    dest_path: Option<String>,
) -> AppResult<String> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::Config(
            "scheduled backups are macOS-only in this build (launchd); see the README for Linux systemd instructions".into(),
        ));
    }
    if hour > 23 || minute > 59 {
        return Err(AppError::InvalidInput(
            "time must be a valid 24h hour/minute".into(),
        ));
    }
    // A folder destination must be attached at install time — a typo'd or
    // unplugged path must fail HERE, not at 3am (or worse, silently).
    if let Some(d) = &dest_path
        && !d.trim().is_empty()
        && !PathBuf::from(d.trim()).is_dir()
    {
        return Err(AppError::InvalidInput(
            "backup folder not found — connect the drive and pick it again".into(),
        ));
    }

    // Persist the destination FIRST so the schedule and the in-app
    // "Back up now" share one source of truth. MERGE semantics: the UI
    // never re-populates the token field ("never shown again after
    // saving"), so a `None` argument must NOT erase a stored credential —
    // a time-only reinstall would otherwise wipe the token and the new
    // plist would push with an empty token, failing unattended.
    let destination = {
        let db = std::sync::Arc::clone(&state.db);
        let url = url.clone();
        let token = token.clone();
        let dest_path = dest_path.clone();
        tokio::task::spawn_blocking(move || -> AppResult<EffectiveDestination> {
            let conn = db.conn()?;
            let mut config = SettingsRepo::load_config(&conn)?;
            config.migrate();
            let merged = merge_destination(&mut config, url, token, dest_path)?;
            SettingsRepo::save_config(&conn, &config)?;
            Ok(merged)
        })
        .await
        .map_err(|e| AppError::Config(format!("config task: {e}")))??
    };

    // Resolve the recordings dir from AppConfig (storage path when set)
    // and BAKE it into the schedule: the nightly CLI run must not need to
    // open the encrypted DB to discover it — without this, custom-folder
    // installs would silently back up an empty default recordings dir.
    let config = load_config(&state).await?;
    let recordings_dir = recordings_dir_for(&config, &state.data_dir);

    let data_dir = state.data_dir.clone();
    let (binary_path, plist_name) =
        tokio::task::spawn_blocking(move || -> AppResult<(PathBuf, String)> {
            let binary = schedule::ensure_binary_copy(&data_dir.join("bin")).map_err(backup_err)?;
            let cfg = schedule::ScheduleConfig {
                binary_path: binary.clone(),
                hour,
                minute,
                url: match &destination {
                    EffectiveDestination::Agent { url, .. } => url.clone(),
                    _ => String::new(),
                },
                token: match &destination {
                    EffectiveDestination::Agent { token, .. } => token.clone(),
                    _ => String::new(),
                },
                dest_dir: match &destination {
                    EffectiveDestination::Folder { path } => Some(PathBuf::from(path)),
                    _ => None,
                },
                snapshots_dir: data_dir.join("backups"),
                recordings_dir,
                log_dir: data_dir.join("logs"),
            };
            let plist = schedule::install(&cfg).map_err(backup_err)?;
            let name = plist
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((binary, name))
        })
        .await
        .map_err(|e| AppError::Config(format!("schedule task: {e}")))??;

    Ok(format!(
        "Daily backup scheduled at {hour:02}:{minute:02} ({plist_name}); tool staged at {}",
        binary_path.display()
    ))
}

/// Remove the launchd agent. Idempotent.
#[tauri::command]
pub async fn backup_uninstall_schedule() -> AppResult<String> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::Config("macOS-only".into()));
    }
    tokio::task::spawn_blocking(schedule::uninstall)
        .await
        .map_err(|e| AppError::Config(format!("schedule task: {e}")))?
        .map_err(backup_err)?;
    Ok("Scheduled backup removed".into())
}

/// Run the full backup job NOW (same code path as the schedule) using the
/// persisted target, emitting `backup-job` events `{kind, line}` as it
/// goes. Returns whether the job passed; the pane refreshes status after.
#[tauri::command]
pub async fn backup_run_now(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<bool> {
    #[derive(Clone, Serialize)]
    struct JobEventPayload {
        kind: &'static str,
        line: String,
    }

    let mut config = load_config(&state).await?;
    let target = match merge_destination(&mut config, None, None, None)? {
        EffectiveDestination::Agent { url, token } => Some(job::BackupTarget::Agent { url, token }),
        EffectiveDestination::Folder { path } => Some(job::BackupTarget::Folder {
            path: PathBuf::from(path),
        }),
        EffectiveDestination::LocalOnly => None,
    };

    let db_key = tokio::task::spawn_blocking(keychain::get_or_create_db_key)
        .await
        .map_err(|e| AppError::Config(format!("keychain task: {e}")))?
        .map_err(|e| AppError::Config(format!("keychain: {e}")))?;
    let wrapping = tokio::task::spawn_blocking(keys::load_or_create_wrapping_key)
        .await
        .map_err(|e| AppError::Config(format!("keychain task: {e}")))?
        .map_err(backup_err)?;

    let data_dir = state.data_dir.clone();
    let cfg = job::JobConfig {
        data_dir: data_dir.clone(),
        db_path: data_dir.join("medical.db"),
        recordings_dir: recordings_dir_for(&config, &data_dir),
        keystore_path: Some(data_dir.join("config").join("keys.json")),
        target,
        keep_local: 14,
    };

    let outcome = tokio::task::spawn_blocking(move || job::run_backup_job(&cfg, db_key, wrapping))
        .await
        .map_err(|e| AppError::Config(format!("job task: {e}")))?;

    for event in &outcome.events {
        let kind = match event.kind {
            job::JobEventKind::Ok => "ok",
            job::JobEventKind::Fail => "fail",
            job::JobEventKind::Step => "step",
        };
        let _ = app.emit(
            "backup-job",
            JobEventPayload {
                kind,
                line: event.line.clone(),
            },
        );
    }
    if !outcome.success() {
        warn!("in-app backup job failed (see status pane)");
    }
    Ok(outcome.success())
}

/// Wizard-time destination probe: does the folder exist, can we write
/// to it, and how much space is free. PHI-free by construction.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationProbe {
    pub writable: bool,
    pub free_bytes: Option<u64>,
    pub problem: Option<String>,
}

#[tauri::command]
pub async fn backup_test_destination(dest_path: String) -> AppResult<DestinationProbe> {
    let expanded = expand_tilde(&dest_path);
    let path = crate::commands::validate_user_path(&expanded.to_string_lossy())?;
    tokio::task::spawn_blocking(move || {
        if !path.is_dir() {
            return DestinationProbe {
                writable: false,
                free_bytes: None,
                problem: Some("folder not found — connect the drive and pick it again".into()),
            };
        }
        let free_bytes = fs2::available_space(&path).ok();
        let probe = path.join(".ferriscribe-probe");
        let writable =
            std::fs::write(&probe, b"probe").is_ok() && std::fs::remove_file(&probe).is_ok();
        DestinationProbe {
            writable,
            free_bytes,
            problem: if writable {
                None
            } else {
                Some(
                    "FerriScribe can't write to this folder — check the drive's permissions".into(),
                )
            },
        }
    })
    .await
    .map_err(|e| AppError::Config(format!("probe task: {e}")))
}

/// Wizard-time agent probe: can we reach the backup server, and does the
/// append token work? A one-shot `list_snapshots` doubles as both — the
/// list route requires valid append-token auth. PHI-free (errors carry
/// HTTP codes / io kinds only).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProbe {
    pub ok: bool,
    pub problem: Option<String>,
}

#[tauri::command]
pub async fn backup_test_agent(url: String, token: String) -> AppResult<AgentProbe> {
    let url = url.trim().trim_end_matches('/').to_string();
    let token = token.trim().to_string();
    if url.is_empty() || token.is_empty() {
        return Ok(AgentProbe {
            ok: false,
            problem: Some("Enter both the target URL and the append token.".into()),
        });
    }
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| AppError::Config(format!("probe runtime: {e}")))?;
        Ok(rt.block_on(async {
            match medical_backup::client::BackupClient::new(&url, &token)
                .list_snapshots()
                .await
            {
                Ok(_) => AgentProbe {
                    ok: true,
                    problem: None,
                },
                Err(e) => AgentProbe {
                    ok: false,
                    problem: Some(format!("Can't reach the backup server: {e}")),
                },
            }
        }))
    })
    .await
    .map_err(|e| AppError::Config(format!("probe task: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::settings::AppConfig;

    /// Ship-blocker regression: a time-only reinstall (None args) must
    /// never erase a stored target/token.
    #[test]
    fn merge_destination_preserves_stored_values_on_none_args() {
        let mut config = AppConfig::default();
        config.backup_target_url = Some("http://t:8741".into());
        config.backup_append_token = Some(SecretString("secret".into()));

        let d = merge_destination(&mut config, None, None, None).expect("merge");
        assert_eq!(
            d,
            EffectiveDestination::Agent {
                url: "http://t:8741".into(),
                token: "secret".into()
            }
        );
        // And the config still holds them.
        assert_eq!(
            config.backup_append_token.as_ref().map(|t| t.0.as_str()),
            Some("secret")
        );
    }

    #[test]
    fn merge_destination_updates_when_provided() {
        let mut config = AppConfig::default();
        config.backup_target_url = Some("http://old:1".into());
        let d = merge_destination(
            &mut config,
            Some("http://new:2".into()),
            Some("tok".into()),
            None,
        )
        .expect("merge");
        assert_eq!(
            d,
            EffectiveDestination::Agent {
                url: "http://new:2".into(),
                token: "tok".into()
            }
        );
    }

    /// A URL with no token anywhere must be rejected AT INSTALL TIME —
    /// not discovered by a failing unattended 3am push.
    #[test]
    fn merge_destination_rejects_url_without_token() {
        let mut config = AppConfig::default();
        let err = merge_destination(&mut config, Some("http://t:8741".into()), None, None);
        assert!(err.is_err(), "fresh config + URL + no token must fail");

        // Also when a token was never stored and only the URL is re-sent.
        config.backup_target_url = Some("http://t:8741".into());
        config.backup_append_token = None;
        assert!(merge_destination(&mut config, Some("http://t:8741".into()), None, None).is_err());

        // A stored token satisfies the requirement.
        config.backup_append_token = Some(SecretString("kept".into()));
        let d = merge_destination(&mut config, Some("http://t:8741".into()), None, None).unwrap();
        assert_eq!(
            d,
            EffectiveDestination::Agent {
                url: "http://t:8741".into(),
                token: "kept".into()
            }
        );
    }

    /// Local-only schedules (no URL, no token) are fine.
    #[test]
    fn merge_destination_allows_local_only() {
        let mut config = AppConfig::default();
        assert_eq!(
            merge_destination(&mut config, None, None, None).unwrap(),
            EffectiveDestination::LocalOnly
        );
    }

    #[test]
    fn merge_destination_switching_to_folder_clears_agent() {
        let mut config = AppConfig::default();
        config.backup_target_url = Some("http://t:8741".into());
        config.backup_append_token = Some(SecretString("tok".into()));
        let d =
            merge_destination(&mut config, None, None, Some("/Volumes/BK".into())).expect("switch");
        assert_eq!(
            d,
            EffectiveDestination::Folder {
                path: "/Volumes/BK".into()
            }
        );
        assert_eq!(config.backup_target_url, None);
        assert_eq!(config.backup_append_token, None);
    }

    #[test]
    fn merge_destination_switching_to_agent_clears_folder() {
        let mut config = AppConfig::default();
        config.backup_dest_path = Some("/Volumes/BK".into());
        let d = merge_destination(
            &mut config,
            Some("http://t:8741".into()),
            Some("tok".into()),
            None,
        )
        .expect("switch");
        assert_eq!(
            d,
            EffectiveDestination::Agent {
                url: "http://t:8741".into(),
                token: "tok".into()
            }
        );
        assert_eq!(config.backup_dest_path, None);
    }

    #[test]
    fn merge_destination_rejects_both_passed_in_one_call() {
        let mut config = AppConfig::default();
        assert!(
            merge_destination(
                &mut config,
                Some("http://t:8741".into()),
                None,
                Some("/Volumes/BK".into())
            )
            .is_err()
        );
    }

    #[test]
    fn merge_destination_rejects_both_lingering_in_stored_config() {
        let mut config = AppConfig::default();
        config.backup_target_url = Some("http://t:8741".into());
        config.backup_append_token = Some(SecretString("tok".into()));
        config.backup_dest_path = Some("/Volumes/BK".into());
        assert!(merge_destination(&mut config, None, None, None).is_err());
    }
}
