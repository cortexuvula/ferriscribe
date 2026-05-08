mod state;
mod commands;
mod sharing_vocab_api;
mod vocab_remote;
mod templates_remote;

use std::path::PathBuf;
use std::sync::Arc;

use state::{AppState, InitError, RecoveryState};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Resolve the log directory inside the app data folder.
///
/// Returns `~/{data}/rust-medical-assistant/logs/` and ensures it exists.
fn log_dir() -> PathBuf {
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
             info"
        )
    });

    let log_directory = log_dir();

    // Rolling daily log file: ferri-scribe.YYYY-MM-DD.log
    let file_appender = tracing_appender::rolling::daily(&log_directory, "ferri-scribe.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Console layer — compact format for terminal
    let console_layer = tracing_subscriber::fmt::layer()
        .compact();

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
        Err(InitError::Other(e)) => {
            // Fatal — match the previous behaviour (panic so the process exits
            // with a clear error rather than silently failing).
            panic!("Failed to initialize application state: {e}");
        }
    }

    // Always managed so `get_database_recovery_state` is always callable.
    builder = builder.manage(recovery_state);

    // Auto-resume office-server mode if the user enabled it in a previous
    // session. We only consider this when AppState was successfully managed
    // — there's nothing to start sharing on top of in recovery mode. Failures
    // are logged and never block app startup.
    if app_state_managed {
        builder = builder.setup(|app| {
            if let Some(cfg) = crate::state::load_server_config() {
                tracing::info!(
                    "auto-resuming office-server mode from saved config"
                );
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Manager;
                    let state = app_handle.state::<crate::state::AppState>();
                    if let Err(e) = crate::commands::sharing::start_sharing_inner(
                        &state,
                        cfg.friendly_name,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "auto-resume sharing failed");
                    }
                });
            }
            Ok(())
        });
    }

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            commands::recordings::list_recordings,
            commands::recordings::get_recording,
            commands::recordings::search_recordings,
            commands::recordings::delete_recording,
            commands::recordings::delete_all_recordings,
            commands::recordings::import_audio_file,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::get_api_key,
            commands::settings::set_api_key,
            commands::settings::list_api_keys,
            commands::settings::get_default_prompt,
            commands::export::export_pdf,
            commands::export::export_docx,
            commands::export::export_fhir,
            commands::providers::reinit_providers,
            commands::providers::test_lmstudio_connection,
            commands::providers::test_stt_remote_connection,
            commands::providers::test_ollama_connection,
            commands::audio::list_audio_devices,
            commands::audio::start_recording,
            commands::audio::stop_recording,
            commands::audio::cancel_recording,
            commands::audio::pause_recording,
            commands::audio::resume_recording,
            commands::audio::check_recording_audio_levels,
            commands::audio::get_recording_state,
            commands::chat::chat_send,
            commands::chat::chat_stream,
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
            commands::generation::synopsis::generate_synopsis,
            commands::rag::ingest_document,
            commands::rag::search_rag,
            commands::rag::rag_stats,
            commands::models::list_whisper_models,
            commands::models::list_pyannote_models,
            commands::models::download_model,
            commands::models::delete_model,
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
            commands::recovery::recover_database_from_path,
            commands::recovery::recover_database_wipe,
            commands::recovery::database_encryption_status,
            commands::sharing::lifecycle::start_sharing,
            commands::sharing::lifecycle::stop_sharing,
            commands::sharing::lifecycle::sharing_status,
            commands::sharing::pairing::pairing_qr,
            commands::sharing::pairing::list_paired_clients,
            commands::sharing::pairing::revoke_client,
            commands::sharing::discovery::discover_servers,
            commands::sharing::discovery::discover_via_tailscale,
            commands::sharing::pairing::pair_with_server,
            commands::sharing::pairing::paired_endpoint,
            commands::sharing::pairing::unpair,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Remove log files older than `keep_days`.
fn cleanup_old_logs(dir: &std::path::Path, keep_days: u64) {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(keep_days * 24 * 3600);

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        if let Ok(meta) = path.metadata()
            && let Ok(modified) = meta.modified()
                && modified < cutoff {
                    tracing::debug!(file = %path.display(), "Removing old log file");
                    let _ = std::fs::remove_file(&path);
                }
    }
}
