//! Shared test helpers for generation command tests.
//! Compiled only in test configurations — never included in release builds.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
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
pub(crate) async fn build_test_state_with_recording(
    config: AppConfig,
    transcript_text: &str,
) -> (AppState, String) {
    build_test_state_inner(config, transcript_text, None).await
}

/// Like [`build_test_state_with_recording`], but registers `provider`
/// (under its own `name()`, set active) instead of a real Ollama provider.
/// `config.ai_provider` must match `provider.name()` so `resolve_provider`
/// finds it, and `config.ollama_host` should be loopback so the pre-flight
/// probe is skipped.
pub(crate) async fn build_test_state_with_provider(
    config: AppConfig,
    transcript_text: &str,
    provider: Arc<dyn medical_core::traits::AiProvider>,
) -> (AppState, String) {
    build_test_state_inner(config, transcript_text, Some(provider)).await
}

async fn build_test_state_inner(
    config: AppConfig,
    transcript_text: &str,
    provider_override: Option<Arc<dyn medical_core::traits::AiProvider>>,
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
    match provider_override {
        Some(provider) => {
            registry.register(provider);
            registry.set_active(&config.ai_provider);
        }
        None => {
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
        }
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
        chat_doc_index: Arc::new(tokio::sync::Mutex::new(None)),
        capture_handle: Arc::new(std::sync::Mutex::new(crate::state::SendCaptureHandle(
            None, None,
        ))),
        current_recording: Arc::new(std::sync::Mutex::new(None)),
        pipeline_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        sharing: Arc::new(RwLock::new(None)),
        vocab_api: RwLock::new(None),
        ollama_provider: RwLock::new(None),
        lmstudio_provider: RwLock::new(None),
        omlx_provider: RwLock::new(None),
        remote_stt_provider: RwLock::new(None),
        http_client,
        content_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
        chat_stream_cancel: Arc::new(tokio::sync::Mutex::new(None)),
        content_sse_cancel: Arc::new(std::sync::Mutex::new(None)),
        condition_sse_cancel: Arc::new(std::sync::Mutex::new(None)),
        dict_sse_cancel: Arc::new(std::sync::Mutex::new(None)),
    };

    (state, recording_id.to_string())
}

/// Shared assertion for the "returns EndpointOffline when the AI provider
/// is unreachable" preflight tests: the command must fail FAST (the probe
/// short-circuits at ~3s, well under the 8s ceiling) with
/// `AppError::EndpointOffline` naming the provider — never invoking the
/// provider itself. Generic over the command's success payload.
pub(crate) fn assert_endpoint_offline<T>(
    result: AppResult<T>,
    expected_provider: &str,
    started: std::time::Instant,
) where
    T: std::fmt::Debug,
{
    let err = result.expect_err("must fail with offline error");
    match err {
        AppError::EndpointOffline {
            service,
            reason,
            provider_name,
            ..
        } => {
            assert_eq!(service, medical_core::error::ServiceKind::AiProvider);
            assert_eq!(provider_name, expected_provider);
            assert!(
                matches!(
                    reason,
                    medical_core::error::OfflineReason::ConnectionRefused
                        | medical_core::error::OfflineReason::Timeout
                ),
                "expected ConnectionRefused or Timeout, got {reason:?}"
            );
        }
        other => panic!("expected EndpointOffline, got {other:?}"),
    }

    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(8),
        "should have short-circuited at ~3s; took {elapsed:?}"
    );
}

/// Deterministic in-process `AiProvider` for generation success-path tests.
///
/// `complete()` returns a fixed non-empty completion with a known token
/// usage; every other method is unused by these tests and returns an error
/// or an empty list. Never performs network I/O.
pub(crate) struct MockCompletionProvider {
    name: &'static str,
    content: String,
    usage: medical_core::types::UsageInfo,
}

impl MockCompletionProvider {
    /// `completion_tokens` drives the recorded throughput stat.
    pub(crate) fn new(name: &'static str, content: &str, completion_tokens: u32) -> Self {
        Self {
            name,
            content: content.to_string(),
            usage: medical_core::types::UsageInfo {
                prompt_tokens: 128,
                completion_tokens,
                total_tokens: 128 + completion_tokens,
                decode_tokens_per_second: None,
            },
        }
    }
}

#[async_trait::async_trait]
impl medical_core::traits::AiProvider for MockCompletionProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn available_models(&self) -> AppResult<Vec<medical_core::types::ModelInfo>> {
        Ok(Vec::new())
    }

    async fn complete(
        &self,
        request: medical_core::types::CompletionRequest,
    ) -> AppResult<medical_core::types::CompletionResponse> {
        Ok(medical_core::types::CompletionResponse {
            content: self.content.clone(),
            model: request.model.clone(),
            usage: self.usage.clone(),
            tool_calls: Vec::new(),
        })
    }

    async fn complete_stream(
        &self,
        _request: medical_core::types::CompletionRequest,
    ) -> AppResult<
        Box<
            dyn futures_util::Stream<Item = AppResult<medical_core::types::StreamChunk>>
                + Send
                + Unpin,
        >,
    > {
        let chunks = vec![
            Ok(medical_core::types::StreamChunk::Delta {
                text: self.content.clone(),
            }),
            Ok(medical_core::types::StreamChunk::Usage(self.usage.clone())),
            Ok(medical_core::types::StreamChunk::Done),
        ];
        // `Box::pin` yields `Pin<Box<..>>`, but the trait wants a plain
        // `Box<dyn Stream + Send + Unpin>`; `Iter` is already `Unpin`.
        Ok(Box::new(tokio_stream::iter(chunks)))
    }

    async fn complete_with_tools(
        &self,
        _request: medical_core::types::CompletionRequest,
        _tools: Vec<medical_core::types::ToolDef>,
    ) -> AppResult<medical_core::types::ToolCompletionResponse> {
        Err(AppError::ai_provider(
            "mock provider does not support tools".to_string(),
        ))
    }
}

/// Stream-provider mock whose stream replays `chunks`, then optionally
/// stalls forever after the last chunk (for idle-timeout tests).
///
/// Errors in the script are stored as messages (not `AppError`, which is not
/// `Clone`) and re-wrapped via `AppError::ai_provider` at yield time, so the
/// stream can be replayed by any number of `complete_stream` calls.
pub(super) struct ScriptedStreamProvider {
    pub name: &'static str,
    pub chunks: Vec<Result<medical_core::types::StreamChunk, String>>,
    pub stall_after_last: bool,
}

#[async_trait::async_trait]
impl medical_core::traits::AiProvider for ScriptedStreamProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn available_models(&self) -> AppResult<Vec<medical_core::types::ModelInfo>> {
        Ok(Vec::new())
    }

    async fn complete(
        &self,
        _request: medical_core::types::CompletionRequest,
    ) -> AppResult<medical_core::types::CompletionResponse> {
        Err(AppError::ai_provider(
            "scripted provider is stream-only".to_string(),
        ))
    }

    async fn complete_stream(
        &self,
        _request: medical_core::types::CompletionRequest,
    ) -> AppResult<
        Box<
            dyn futures_util::Stream<Item = AppResult<medical_core::types::StreamChunk>>
                + Send
                + Unpin,
        >,
    > {
        let chunks = self.chunks.clone();
        let stall = self.stall_after_last;
        let stream = async_stream::stream! {
            for c in chunks {
                yield match c {
                    Ok(chunk) => Ok(chunk),
                    Err(message) => Err(AppError::ai_provider(message)),
                };
            }
            if stall {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        };
        // `Pin<Box<_>>` is always `Unpin` and implements `Stream`, so this
        // satisfies `Box<dyn Stream + Send + Unpin>` (same pattern as
        // `crates/ai-providers/src/ollama.rs`).
        Ok(Box::new(Box::pin(stream)))
    }

    async fn complete_with_tools(
        &self,
        _request: medical_core::types::CompletionRequest,
        _tools: Vec<medical_core::types::ToolDef>,
    ) -> AppResult<medical_core::types::ToolCompletionResponse> {
        Err(AppError::ai_provider(
            "scripted provider does not support tools".to_string(),
        ))
    }
}
