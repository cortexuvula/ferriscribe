//! FerriScribe Tauri app shell.
//!
//! This is the binary crate that produces the FerriScribe desktop application.
//! It wires together all 13 workspace crates behind Tauri commands that the
//! Svelte frontend calls via `invoke()`.
//!
//! # Responsibilities
//!
//! - **Logging** — two-layer `tracing` setup (console + rolling daily file).
//! - **Panic hook** — captures panics to the tracing log.
//! - **State initialization** — `AppState::initialize()` opens the encrypted DB,
//!   registers AI/STT providers, and sets up the RAG subsystem. On keychain
//!   failure it returns `InitError::DatabaseRecoveryNeeded` so the app can boot
//!   in recovery mode.
//! - **Plugin registration** — deep-link (`ferriscribe://pair?...`), opener,
//!   dialog, clipboard-manager.
//! - **Command registration** — ~80 `#[tauri::command]` functions organized by
//!   domain in the `commands` module.
//! - **Office-server auto-resume** — if the user previously enabled sharing,
//!   `start_sharing_inner` is spawned on startup.
//!
//! # Architecture
//!
//! ```text
//! Svelte frontend
//!   │ invoke('command_name', { ...args })
//!   ▼
//! commands::* (this crate)
//!   │ delegates to
//!   ▼
//! workspace crates (medical-processing, medical-agents, etc.)
//! ```
//!
//! See `state::AppState` for the managed state type and the `commands`
//! module for the full command inventory.

mod commands;
mod conditions_remote;
mod content_remote;
pub mod corpus_export;
mod sharing_vocab_api;
mod state;
mod sweeps;
mod sync_sparse_fields;
mod templates_remote;
#[cfg(test)]
pub(crate) mod testutil;
mod user_dict_remote;
mod vocab_remote;

use std::path::PathBuf;
use std::sync::Arc;

use state::{AppState, InitError, RecoveryState};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Resolve the log directory inside the app data folder.
///
/// Returns the log directory, creating it if it doesn't exist.
///
/// The single source of truth for the log path — used by the tracing
/// subscriber (this file), the log-viewer command (`commands::logging`),
/// and the support-bundle export (`commands::support`).
///
/// Returns `~/{data}/rust-medical-assistant/logs/` on all platforms:
/// - Windows: `C:\Users\{user}\AppData\Roaming\rust-medical-assistant\logs\`
/// - macOS:   `~/Library/Application Support/rust-medical-assistant/logs/`
/// - Linux:   `~/.local/share/rust-medical-assistant/logs/`
pub(crate) fn log_dir() -> PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust-medical-assistant")
        .join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ── Logging ──────────────────────────────────────────────────────────
    //
    // Two layers:
    //   1. Console (stdout) — compact, human-readable
    //   2. Rolling file    — full detail, daily rotation, kept for 7 days
    //
    // Controlled via RUST_LOG env var; defaults shown below.

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "rust_medical_assistant=debug,\
             medical_stt_providers=debug,\
             medical_ai_providers=info,\
             medical_audio=info,\
             medical_processing=debug,\
             info",
        )
    });

    let log_directory = log_dir();

    // Rolling daily log file: ferri-scribe.log.YYYY-MM-DD
    // (tracing_appender appends the date AFTER the prefix; current
    // file is `ferri-scribe.log` without suffix until rotation.)
    let file_appender = tracing_appender::rolling::daily(&log_directory, "ferri-scribe.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Console layer — compact format for terminal
    let console_layer = tracing_subscriber::fmt::layer().compact();

    // File layer — full timestamps, structured fields, no ANSI colors
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    // ── Panic hook ───────────────────────────────────────────────────────
    //
    // Capture panics to the tracing log so they appear in the log file,
    // not just stderr.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        tracing::error!(
            panic.payload = %payload,
            panic.location = %location,
            "PANIC"
        );
        default_hook(info);
    }));

    // ── Startup banner ───────────────────────────────────────────────────
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        log_dir = %log_directory.display(),
        "FerriScribe starting"
    );

    // ── Clean up old log files (keep last 7 days) ────────────────────────
    cleanup_old_logs(&log_directory, 7);

    // ── App init ─────────────────────────────────────────────────────────
    //
    // `AppState::initialize` may return `InitError::DatabaseRecoveryNeeded`
    // when an encrypted DB exists on disk but the keychain entry is missing
    // or inaccessible. In that case we boot without managed `AppState` and
    // populate `RecoveryState` with the reason so the frontend can query
    // it on mount and render the recovery dialog. `RecoveryState` is always
    // managed — `Some(reason)` for recovery, `None` for normal boot — so the
    // recovery commands (which don't depend on `AppState`) always have access.
    let recovery_state = Arc::new(RecoveryState::default());
    // Same pattern for fatal (non-recovery) init errors: we set the message
    // and let the frontend render a dialog instead of panicking. Under
    // `panic = "abort"` (release) the old panic was a silent hard exit with
    // no UI and no recovery path for the user.
    let fatal_error_state = Arc::new(crate::state::FatalErrorState::default());

    let init_result = AppState::initialize();
    let mut builder = tauri::Builder::default();
    let mut app_state_managed = false;
    match init_result {
        Ok(state) => {
            builder = builder.manage(state);
            app_state_managed = true;
        }
        Err(InitError::DatabaseRecoveryNeeded { reason }) => {
            tracing::warn!(%reason, "Database recovery needed");
            recovery_state.set(reason);
            // Do not register AppState; do not start the background subsystems.
        }
        Err(InitError::EncryptionUnavailable { reason }) => {
            // Patient data exists but can't be encrypted — do NOT silently
            // fall back to plaintext. Surface as a recovery condition so the
            // clinician sees a clear message and can act (grant keychain
            // access, free disk space, contact support).
            tracing::error!(%reason, "Database encryption unavailable");
            recovery_state.set(reason);
        }
        Err(InitError::Other(e)) => {
            // Fatal but not panic-worthy: surface a dialog so the user sees what
            // went wrong and can quit / copy the error / report it. Previously
            // this was `panic!`, which under `panic = "abort"` killed the
            // process with no UI.
            tracing::error!(error = %e, "Fatal application initialization error");
            fatal_error_state.set(format!("{e}"));
        }
    }

    // Always managed so `get_database_recovery_state` / `get_fatal_error` are
    // always callable, even in the recovery or fatal-error boot branches.
    builder = builder.manage(recovery_state);
    builder = builder.manage(fatal_error_state);

    // Auto-resume office-server mode if the user enabled it in a previous
    // session. We only consider this when AppState was successfully managed
    // — there's nothing to start sharing on top of in recovery mode. Failures
    // are logged and never block app startup.
    if app_state_managed {
        builder = builder.setup(|app| {
            if let Some(cfg) = crate::state::load_server_config() {
                tracing::info!("auto-resuming office-server mode from saved config");
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Manager;
                    let state = app_handle.state::<crate::state::AppState>();
                    if let Err(e) = crate::commands::sharing::start_sharing_inner(
                        &state,
                        cfg.friendly_name,
                        app_handle.clone(),
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "auto-resume sharing failed");
                    }
                });
            }

            // Content sync: initial pull on startup if enabled. The 3s delay
            // lets the window finish rendering before the network round-trip;
            // failures are logged inside `run_initial_sync` and never block
            // boot — the app stays usable offline.
            {
                use tauri::Manager;
                let app_handle = app.handle().clone();
                let state = app_handle.state::<crate::state::AppState>();
                let db = state.db.clone();
                let config = crate::commands::settings::load_config_sync(&db).unwrap_or_default();
                if config.sync_content {
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        tracing::info!("starting initial content sync");
                        let _ =
                            crate::commands::content_sync::run_initial_sync(app_handle, db).await;
                    });
                }
            }

            // Auto-stop sharing when the main window closes, so the
            // whisper-server child process is killed instead of becoming an
            // orphan zombie on app quit. Best-effort: failures are logged.
            //
            // The close is PREVENTED and re-issued after the stop finishes:
            // the spawned task races the run loop otherwise, and the window
            // can be torn down before the whisper-server child is killed —
            // the exact orphan this handler exists to prevent. destroy()
            // (not close()) closes without re-firing CloseRequested, so
            // this cannot re-enter.
            use tauri::Manager;
            let close_handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                let close_window = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let handle = close_handle.clone();
                        let window = close_window.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = handle.state::<crate::state::AppState>();
                            if let Err(e) =
                                crate::commands::sharing::stop_sharing_inner(&state).await
                            {
                                tracing::warn!(error = %e, "auto-stop sharing on close failed");
                            }
                            let _ = window.destroy();
                        });
                    }
                });
            }

            // Note: the pdfium renderer for scanned-PDF OCR is initialized
            // lazily by the `ocr_documents` command (it downloads the library
            // into the app data dir on first use), so there's nothing to do
            // here at startup.

            Ok(())
        });
    }

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            commands::recordings::list_recordings,
            commands::recordings::get_recording,
            commands::recordings::search_recordings,
            commands::recordings::delete_recording,
            commands::recordings::restore_recording,
            commands::recordings::delete_all_recordings,
            commands::recordings::import_audio_file,
            commands::recordings_edit::save_recording_field,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::set_onboarding_started,
            commands::settings::get_api_key,
            commands::settings::set_api_key,
            commands::settings::get_default_prompt,
            commands::export::export_pdf,
            commands::export::export_docx,
            commands::export::export_fhir,
            commands::export::export_audio,
            commands::backup::backup_status,
            commands::backup::backup_escrow_init,
            commands::backup::backup_escrow_verify,
            commands::backup::backup_install_schedule,
            commands::backup::backup_uninstall_schedule,
            commands::backup::backup_run_now,
            commands::backup::backup_test_destination,
            commands::backup::backup_test_agent,
            commands::providers::reinit_providers,
            commands::providers::test_lmstudio_connection,
            commands::providers::test_stt_remote_connection,
            commands::providers::test_ollama_connection,
            commands::providers::test_omlx_connection,
            commands::providers::probe_endpoint_reachable,
            commands::audio::list_audio_devices,
            commands::audio::start_recording,
            commands::audio::stop_recording,
            commands::audio::cancel_recording,
            commands::audio::pause_recording,
            commands::audio::resume_recording,
            commands::audio::check_recording_audio_levels,
            commands::audio::run_microphone_probe,
            commands::audio::get_recording_state,
            commands::chat::chat_send,
            commands::chat::chat_stream,
            commands::chat::chat_cancel_stream,
            commands::chat_docs::chat_clear_documents,
            commands::chat::chat_with_agent,
            commands::chat::list_ai_providers,
            commands::chat::set_active_provider,
            commands::chat::list_models,
            commands::transcription::transcribe_recording,
            commands::transcription::list_stt_providers,
            commands::pipeline::process_recording,
            commands::pipeline::cancel_pipeline,
            commands::generation::soap::generate_soap,
            commands::generation::referral::generate_referral,
            commands::generation::letter::generate_letter,
            commands::generation::letter_writer::generate_letter_from_document,
            commands::letter_audiences::list_letter_audiences,
            commands::letter_audiences::upsert_letter_audience,
            commands::letter_audiences::delete_letter_audience,
            commands::generation::synopsis::generate_synopsis,
            commands::generation::peer_discussion::generate_peer_discussion,
            commands::icd::get_icd9_code_set,
            commands::icd::get_icd9_descriptions,
            commands::models::list_whisper_models,
            commands::models::list_pyannote_models,
            commands::models::download_model,
            commands::models::delete_model,
            commands::ocr::ocr_documents,
            commands::logging::get_log_path,
            commands::logging::get_recent_logs,
            commands::logging::frontend_log,
            commands::vocabulary::list_vocabulary_entries,
            commands::vocabulary::add_vocabulary_entry,
            commands::vocabulary::update_vocabulary_entry,
            commands::vocabulary::delete_vocabulary_entry,
            commands::vocabulary::delete_all_vocabulary_entries,
            commands::vocabulary::get_vocabulary_count,
            commands::vocabulary::import_vocabulary_json,
            commands::vocabulary::export_vocabulary_json,
            commands::vocabulary::test_vocabulary_correction,
            commands::context_templates::list_context_templates,
            commands::context_templates::upsert_context_template,
            commands::context_templates::rename_context_template,
            commands::context_templates::delete_context_template,
            commands::context_templates::import_context_templates_json,
            commands::context_templates::export_context_templates_json,
            commands::recovery::get_database_recovery_state,
            commands::recovery::get_fatal_error,
            commands::recovery::recover_database_from_path,
            commands::recovery::recover_database_wipe,
            commands::recovery::database_encryption_status,
            commands::support::export_support_bundle,
            commands::sharing::lifecycle::start_sharing,
            commands::sharing::lifecycle::stop_sharing,
            commands::sharing::lifecycle::sharing_status,
            commands::sharing::pairing::pairing_qr,
            commands::sharing::pairing::list_paired_clients,
            commands::sharing::pairing::revoke_client,
            commands::sharing::pairing::rename_client,
            commands::sharing::pairing::suggested_client_label,
            commands::sharing::discovery::discover_servers,
            commands::sharing::discovery::discover_via_tailscale,
            commands::sharing::pairing::pair_with_server,
            commands::sharing::pairing::paired_endpoint,
            commands::sharing::pairing::unpair,
            commands::training_corpus::training_corpus_counts,
            commands::training_corpus::training_corpus_list,
            commands::training_corpus::training_corpus_set_status,
            commands::training_corpus_export::training_corpus_export,
            commands::user_dictionary::user_dict_list,
            commands::user_dictionary::user_dict_add,
            commands::user_dictionary::user_dict_remove,
            commands::user_dictionary::sync_user_dictionary_cmd,
            commands::user_dictionary::subscribe_user_dictionary,
            commands::conditions::list_condition_chips,
            commands::conditions::add_condition_chip,
            commands::conditions::remove_condition_chip,
            commands::conditions::sync_condition_chips_cmd,
            commands::conditions::increment_condition_chip_use,
            commands::conditions::subscribe_condition_chips,
            commands::content_sync::sync_content_now,
            commands::content_sync::subscribe_content_sync,
            commands::content_sync::fetch_audio_from_server,
            commands::content_sync::upload_audio_to_server,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Remove log files older than `keep_days`.
fn cleanup_old_logs(dir: &std::path::Path, keep_days: u64) {
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days * 24 * 3600);

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Rolled log files are named `ferri-scribe.log.YYYY-MM-DD` (the
        // date is appended after the prefix by tracing_appender::rolling::daily).
        // The current file (no rotation suffix yet) is `ferri-scribe.log`.
        // Match by filename prefix rather than extension — extension is the
        // date string, not `.log`.
        let is_log = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == "ferri-scribe.log" || n.starts_with("ferri-scribe.log."))
            .unwrap_or(false);
        if !is_log {
            continue;
        }
        if let Ok(meta) = path.metadata()
            && let Ok(modified) = meta.modified()
            && modified < cutoff
        {
            tracing::debug!(file = %path.display(), "Removing old log file");
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod cleanup_old_logs_tests {
    use super::cleanup_old_logs;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn touch(path: &std::path::Path, age: Duration) {
        fs::write(path, b"x").unwrap();
        let mtime = SystemTime::now() - age;
        let ft = filetime::FileTime::from_system_time(mtime);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    #[test]
    fn deletes_rolled_files_older_than_cutoff() {
        let tmp = tempfile::tempdir().unwrap();
        let old_rolled = tmp.path().join("ferri-scribe.log.2025-01-01");
        let old_base = tmp.path().join("ferri-scribe.log");
        let new_rolled = tmp.path().join("ferri-scribe.log.2026-05-11");
        let unrelated = tmp.path().join("other.txt");

        touch(&old_rolled, Duration::from_secs(30 * 24 * 3600));
        touch(&old_base, Duration::from_secs(30 * 24 * 3600));
        touch(&new_rolled, Duration::from_secs(60));
        touch(&unrelated, Duration::from_secs(30 * 24 * 3600));

        cleanup_old_logs(tmp.path(), 7);

        assert!(!old_rolled.exists(), "old rolled log should be deleted");
        assert!(
            !old_base.exists(),
            "old base log file (no date suffix) should be deleted — covers the `n == \"ferri-scribe.log\"` branch of is_log"
        );
        assert!(new_rolled.exists(), "recent rolled log should remain");
        assert!(unrelated.exists(), "unrelated files must not be touched");
    }
}
