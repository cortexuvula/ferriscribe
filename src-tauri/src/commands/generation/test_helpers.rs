//! Shared test helpers for generation command tests.
//! Compiled only in test configurations — never included in release builds.

use std::sync::Arc;

use medical_core::types::recording::{ProcessingStatus, Recording};
use medical_core::types::settings::AppConfig;
use medical_db::recordings::RecordingsRepo;
use medical_db::settings::SettingsRepo;
use tokio::sync::{Mutex, RwLock};

use crate::state::AppState;

/// Build a minimal `AppState` backed by an in-memory SQLite database.
///
/// The database is pre-loaded with `config` and a recording whose transcript
/// is `transcript_text`.  A real (but intentionally unrouteable) AI provider
/// is registered in the registry so `resolve_provider` succeeds, but
/// `preflight_for_command` short-circuits before the provider is ever invoked.
///
/// Returns `(AppState, recording_id_as_string)`.
pub(super) async fn build_test_state_with_recording(
    config: AppConfig,
    transcript_text: &str,
) -> (AppState, String) {
    // ── Database ─────────────────────────────────────────────────────────────
    let db = Arc::new(medical_db::Database::open_in_memory().expect("open in-memory db"));

    // Save the config so load_recording_and_settings picks it up and
    // preflight_for_command reads the right host/port.
    {
        let conn = db.conn().expect("conn");
        SettingsRepo::save_config(&conn, &config).expect("save_config");
    }

    // Insert a recording with the given transcript.
    let recording_id = {
        use std::path::PathBuf;
        let id = uuid::Uuid::new_v4();
        let mut rec = Recording::new(
            format!("{}.wav", id),
            PathBuf::from(format!("/tmp/{}.wav", id)),
        );
        rec.id = id;
        rec.status = ProcessingStatus::Pending;
        rec.transcript = Some(transcript_text.to_string());
        let conn = db.conn().expect("conn");
        RecordingsRepo::insert(&conn, &rec).expect("insert recording");
        id
    };

    // ── AI provider registry ──────────────────────────────────────────────────
    // Register the Ollama provider pointing at whatever host/port the config
    // specifies (typically TEST-NET-1, 192.0.2.1).  Pre-flight fires before
    // `provider.complete()` is ever called, so the unreachable endpoint is
    // never actually contacted via the provider path.
    let mut registry = medical_ai_providers::ProviderRegistry::new();
    let ollama_host = if config.ollama_host.is_empty() {
        "localhost"
    } else {
        config.ollama_host.as_str()
    };
    let ollama_url = format!("http://{}:{}", ollama_host, config.ollama_port);
    if let Ok(p) = medical_ai_providers::ollama::OllamaProvider::new_with_endpoint(
        Some(&ollama_url),
        config.allow_public_endpoint,
        None,
        medical_ai_providers::http_client::RetryConfig::default(),
        None,
    ) {
        registry.register(Arc::new(p) as Arc<dyn medical_core::traits::AiProvider>);
        registry.set_active(&config.ai_provider);
    }

    // ── Key storage ───────────────────────────────────────────────────────────
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path().join("config");
    let keys =
        medical_security::key_storage::KeyStorage::open(&config_dir).expect("KeyStorage::open");
    // Keep `tmp` alive for the duration of the state by leaking it (acceptable
    // in tests — memory is reclaimed when the process exits).
    std::mem::forget(tmp);

    // ── Agent orchestrator ────────────────────────────────────────────────────
    let tool_registry = medical_agents::tools::ToolRegistry::with_defaults();
    let orchestrator = Arc::new(medical_agents::orchestrator::AgentOrchestrator::new(
        tool_registry,
    ));

    // ── HTTP client ───────────────────────────────────────────────────────────
    let http_client = Arc::new(reqwest::Client::new());

    let state = AppState {
        db,
        keys: Arc::new(keys),
        data_dir: std::path::PathBuf::from("/tmp/test-data"),
        recording_active: Arc::new(Mutex::new(false)),
        ai_providers: Arc::new(Mutex::new(registry)),
        stt_providers: Arc::new(Mutex::new(None)),
        orchestrator,
        capture_handle: Arc::new(std::sync::Mutex::new(crate::state::SendCaptureHandle(None))),
        current_recording: Arc::new(std::sync::Mutex::new(None)),
        pipeline_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        sharing: Arc::new(RwLock::new(None)),
        vocab_api: RwLock::new(None),
        ollama_provider: RwLock::new(None),
        lmstudio_provider: RwLock::new(None),
        remote_stt_provider: RwLock::new(None),
        http_client,
    };

    (state, recording_id.to_string())
}
