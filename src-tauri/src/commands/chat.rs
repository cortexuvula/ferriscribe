use serde::{Deserialize, Serialize};
use futures_util::StreamExt;
use tauri::Emitter;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use medical_core::error::{AppError, AppResult};
use medical_core::traits::Agent;
use medical_core::types::{
    AgentContext, CompletionRequest, Message, MessageContent, Role, StreamChunk, UsageInfo,
};

use medical_agents::agents::{
    ChatAgent, ComplianceAgent, DataExtractionAgent, DiagnosticAgent, MedicationAgent,
    ReferralAgent, SynopsisAgent, WorkflowAgent,
};

use crate::state::AppState;

/// Maximum total character count for conversation / message history sent to
/// agents or chat completions. Protects against frontend-side loops or
/// malicious payloads that would blow the provider's token limit or exhaust
/// memory. 200k chars is roughly 50k tokens — well above any sane
/// per-conversation size and below the 128k/200k context windows of modern
/// models.
const MAX_HISTORY_CHARS: usize = 200_000;

// ---------------------------------------------------------------------------
// Input / output types
// ---------------------------------------------------------------------------

/// Lightweight message type received from the frontend.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessageInput {
    pub role: String,
    pub content: String,
}

/// Payload emitted for streaming token events.
#[derive(Debug, Clone, Serialize)]
struct TokenPayload {
    content: String,
}

/// Payload emitted when streaming completes.
#[derive(Debug, Clone, Serialize)]
struct DonePayload {
    usage: Option<UsageInfo>,
    finish_reason: Option<String>,
}

/// Payload emitted on streaming errors.
#[derive(Debug, Clone, Serialize)]
struct ErrorPayload {
    message: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load the full `AppConfig` from the DB synchronously.
///
/// Returns a hard error if the settings can't be read — a silent fallback to a
/// hardcoded model (previously `"gpt-4o"`) would route requests to the wrong
/// provider for any user configured for Anthropic/Ollama/etc.
fn load_app_config(state: &tauri::State<'_, AppState>) -> AppResult<medical_core::types::settings::AppConfig> {
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Config(format!("Failed to load chat settings: {e}")))?;
    let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
        .map_err(|e| AppError::Config(format!("Failed to load chat settings: {e}")))?;
    cfg.migrate();
    Ok(cfg)
}


/// Convert a frontend role string to the core `Role` enum.
fn parse_role(s: &str) -> Role {
    match s.to_lowercase().as_str() {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    }
}

/// Convert a `Vec<ChatMessageInput>` to `Vec<Message>`.
fn convert_messages(inputs: Vec<ChatMessageInput>) -> Vec<Message> {
    inputs
        .into_iter()
        .map(|m| Message {
            role: parse_role(&m.role),
            content: MessageContent::Text(m.content),
            tool_calls: vec![],
        })
        .collect()
}

/// Reject payloads whose aggregate message content exceeds `MAX_HISTORY_CHARS`.
/// Called before any history is cloned into a provider request, to fail fast
/// before we spend memory / tokens on an oversized request.
fn check_history_size(messages: &[ChatMessageInput]) -> AppResult<()> {
    let total: usize = messages.iter().map(|m| m.content.len()).sum();
    if total > MAX_HISTORY_CHARS {
        return Err(AppError::Other(format!(
            "Conversation history too large: {total} chars, limit is {MAX_HISTORY_CHARS}"
        )));
    }
    Ok(())
}

/// Look up an agent by name and return a boxed trait object.
fn get_agent_by_name(name: &str) -> Option<Box<dyn Agent>> {
    match name {
        "chat" => Some(Box::new(ChatAgent)),
        "medication" => Some(Box::new(MedicationAgent)),
        "diagnostic" => Some(Box::new(DiagnosticAgent)),
        "compliance" => Some(Box::new(ComplianceAgent)),
        "data_extraction" => Some(Box::new(DataExtractionAgent)),
        "workflow" => Some(Box::new(WorkflowAgent)),
        "referral" => Some(Box::new(ReferralAgent)),
        "synopsis" => Some(Box::new(SynopsisAgent)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Inner implementation of `chat_send`, testable without the Tauri runtime.
async fn chat_send_inner(
    state: &AppState,
    messages: Vec<ChatMessageInput>,
    model: Option<String>,
    system_prompt: Option<String>,
) -> AppResult<String> {
    // Load full config for pre-flight (also provides model/temperature).
    let cfg = {
        let conn = state
            .db
            .conn()
            .map_err(|e| AppError::Config(format!("Failed to load chat settings: {e}")))?;
        let mut c = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Config(format!("Failed to load chat settings: {e}")))?;
        c.migrate();
        c
    };
    let settings_model = cfg.ai_model.clone();
    let settings_temp = cfg.temperature;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::Chat,
        &cfg,
    )
    .await?;

    let provider = {
        let registry = state.ai_providers.lock().await;
        registry.get_active_arc()
    }
    .ok_or_else(|| AppError::AiProvider("No active AI provider configured".to_string()))?;

    let core_messages = convert_messages(messages);

    let request = CompletionRequest {
        model: model.unwrap_or(settings_model),
        messages: core_messages,
        temperature: Some(settings_temp),
        max_tokens: Some(4096),
        system_prompt,
    };

    debug!("chat_send: calling provider '{}'", provider.name());

    let response = provider
        .complete(request)
        .await
        .map_err(|e| match e {
            // Preserve EndpointOffline as-is so the frontend dialog can fire.
            AppError::EndpointOffline { .. } => e,
            // For other errors, keep the existing nicer wrapping.
            _ => AppError::AiProvider(format!(
                "AI completion failed: {}",
                super::unwrap_app_error_message(e)
            )),
        })?;

    Ok(response.content)
}

/// Non-streaming chat completion.
///
/// Sends the provided messages to the active AI provider and returns the full
/// response content as a string.
#[tauri::command]
pub async fn chat_send(
    state: tauri::State<'_, AppState>,
    messages: Vec<ChatMessageInput>,
    model: Option<String>,
    system_prompt: Option<String>,
) -> AppResult<String> {
    check_history_size(&messages)?;
    chat_send_inner(&state, messages, model, system_prompt).await
}

/// Streaming chat completion via Tauri events.
///
/// Emits the following events on the given `AppHandle`:
/// - `chat-token`  — for each text delta (`TokenPayload`)
/// - `chat-done`   — when the stream finishes (`DonePayload`)
/// - `chat-error`  — on error (`ErrorPayload`)
#[tauri::command]
pub async fn chat_stream(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    messages: Vec<ChatMessageInput>,
    model: Option<String>,
    system_prompt: Option<String>,
) -> AppResult<()> {
    check_history_size(&messages)?;

    // Load full config for pre-flight (also provides model/temperature).
    let cfg = load_app_config(&state)?;
    let settings_model = cfg.ai_model.clone();
    let settings_temp = cfg.temperature;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::Chat,
        &cfg,
    )
    .await?;

    let provider = {
        let registry = state.ai_providers.lock().await;
        registry.get_active_arc()
    }
    .ok_or_else(|| AppError::AiProvider("No active AI provider configured".to_string()))?;

    let core_messages = convert_messages(messages);

    let request = CompletionRequest {
        model: model.unwrap_or(settings_model),
        messages: core_messages,
        temperature: Some(settings_temp),
        max_tokens: Some(4096),
        system_prompt,
    };

    debug!("chat_stream: calling provider '{}'", provider.name());

    let mut stream = provider
        .complete_stream(request)
        .await
        .map_err(|e| match e {
            // Preserve EndpointOffline as-is so the frontend dialog can fire.
            AppError::EndpointOffline { .. } => e,
            // For other errors, keep the existing nicer wrapping.
            _ => AppError::AiProvider(format!(
                "Failed to start streaming: {}",
                super::unwrap_app_error_message(e)
            )),
        })?;

    // Consume the stream in a background task so the command returns immediately.
    // The worker returns AppResult<()> so the supervisor below can emit a
    // terminal `chat-error` event if it exits via an error path OR panics —
    // without a supervisor, a panicking JoinHandle would leave the UI spinner
    // spinning forever.
    let worker_app = app.clone();
    let worker = tokio::spawn(async move {
        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => match chunk {
                    StreamChunk::Delta { text } => {
                        let _ = worker_app.emit("chat-token", TokenPayload { content: text });
                    }
                    StreamChunk::ToolCallDelta { .. } => {
                        // Tool-call deltas are not surfaced in the basic chat stream.
                    }
                    StreamChunk::Usage(usage) => {
                        let _ = worker_app.emit(
                            "chat-done",
                            DonePayload {
                                usage: Some(usage),
                                finish_reason: Some("stop".to_string()),
                            },
                        );
                    }
                    StreamChunk::Done => {
                        let _ = worker_app.emit(
                            "chat-done",
                            DonePayload {
                                usage: None,
                                finish_reason: Some("stop".to_string()),
                            },
                        );
                    }
                },
                Err(e) => {
                    let msg = super::unwrap_app_error_message_ref(&e);
                    error!("chat_stream error: {msg}");
                    return Err(e);
                }
            }
        }
        Ok::<(), AppError>(())
    });

    // Supervisor: ensures the UI always sees a terminal event even if the
    // worker task panics, is cancelled, or returns an error. The worker emits
    // `chat-done` itself on clean completion; we only emit `chat-error` here.
    let supervisor_app = app.clone();
    tokio::spawn(async move {
        match worker.await {
            Ok(Ok(())) => {
                // Normal completion — worker already emitted `chat-done`.
            }
            Ok(Err(e)) => {
                let _ = supervisor_app.emit(
                    "chat-error",
                    ErrorPayload {
                        message: super::unwrap_app_error_message(e),
                    },
                );
            }
            Err(join_err) => {
                let msg = if join_err.is_panic() {
                    format!("Chat stream panicked: {join_err}")
                } else {
                    format!("Chat stream cancelled: {join_err}")
                };
                error!("chat_stream supervisor: {msg}");
                let _ = supervisor_app.emit("chat-error", ErrorPayload { message: msg });
            }
        }
    });

    Ok(())
}

/// Inner implementation of `chat_with_agent`, testable without the Tauri runtime.
async fn chat_with_agent_inner(
    state: &AppState,
    message: String,
    agent_name: String,
    conversation_history: Option<Vec<ChatMessageInput>>,
) -> AppResult<serde_json::Value> {
    let agent = get_agent_by_name(&agent_name)
        .ok_or_else(|| AppError::Agent(format!("Unknown agent: '{agent_name}'")))?;

    let provider = {
        let registry = state.ai_providers.lock().await;
        registry.get_active_arc()
    }
    .ok_or_else(|| AppError::AiProvider("No active AI provider configured".to_string()))?;

    let history = conversation_history
        .map(convert_messages)
        .unwrap_or_default();

    let context = AgentContext {
        user_message: message,
        conversation_history: history,
        patient_context: None,
        rag_context: vec![],
        recording: None,
    };

    let cancel = CancellationToken::new();

    // Load full config so we can pass it to pre-flight (model/temperature are
    // also read from here, replacing the separate load_chat_settings call).
    let cfg = {
        let conn = state
            .db
            .conn()
            .map_err(|e| AppError::Config(format!("Failed to load chat settings: {e}")))?;
        let mut c = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Config(format!("Failed to load chat settings: {e}")))?;
        c.migrate();
        c
    };
    let model = cfg.ai_model.clone();
    let temperature = cfg.temperature;

    // Pre-flight: probe the remote AI endpoint before dispatching to the agent
    // orchestrator. Skipped for loopback hosts; returns EndpointOffline on
    // failure so the frontend dialog can fire.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::Chat,
        &cfg,
    )
    .await
    .map_err(|e| match e {
        // Preserve EndpointOffline as-is so the frontend dialog can fire.
        AppError::EndpointOffline { .. } => e,
        other => AppError::Agent(format!(
            "Pre-flight check failed: {}",
            super::unwrap_app_error_message(other)
        )),
    })?;

    debug!(
        "chat_with_agent: running agent '{}' with model '{}' (temperature={})",
        agent_name, model, temperature
    );

    let response = state
        .orchestrator
        .execute(
            agent.as_ref(),
            context,
            provider.as_ref(),
            &model,
            temperature,
            cancel,
        )
        .await
        .map_err(|e| match e {
            // Preserve EndpointOffline as-is so the frontend dialog can fire.
            AppError::EndpointOffline { .. } => e,
            other => AppError::Agent(format!(
                "Agent execution failed: {}",
                super::unwrap_app_error_message(other)
            )),
        })?;

    Ok(serde_json::to_value(&response)?)
}

/// Execute a named agent against the active AI provider.
///
/// Available agent names: `chat`, `medication`, `diagnostic`, `compliance`,
/// `data_extraction`, `workflow`, `referral`, `synopsis`.
///
/// Returns the full `AgentResponse` as a JSON value.
#[tauri::command]
pub async fn chat_with_agent(
    state: tauri::State<'_, AppState>,
    message: String,
    agent_name: String,
    conversation_history: Option<Vec<ChatMessageInput>>,
) -> AppResult<serde_json::Value> {
    // Guard against unbounded payloads: sum message + full history before we
    // allocate an AgentContext or build a provider request.
    let history_chars: usize = conversation_history
        .as_ref()
        .map(|h| h.iter().map(|m| m.content.len()).sum())
        .unwrap_or(0);
    let total = history_chars.saturating_add(message.len());
    if total > MAX_HISTORY_CHARS {
        return Err(AppError::Other(format!(
            "Conversation history too large: {total} chars, limit is {MAX_HISTORY_CHARS}"
        )));
    }

    chat_with_agent_inner(&state, message, agent_name, conversation_history).await
}

/// List all registered AI provider names.
#[tauri::command]
pub async fn list_ai_providers(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<String>> {
    let registry = state.ai_providers.lock().await;
    Ok(registry.list_available())
}

/// Set the active AI provider by name. Returns `true` if the provider exists
/// and was activated, `false` otherwise.
#[tauri::command]
pub async fn set_active_provider(
    state: tauri::State<'_, AppState>,
    name: String,
) -> AppResult<bool> {
    let mut registry = state.ai_providers.lock().await;
    Ok(registry.set_active(&name))
}

/// Fetch available models for a given provider (or active provider if name is None).
#[tauri::command]
pub async fn list_models(
    state: tauri::State<'_, AppState>,
    provider_name: Option<String>,
) -> AppResult<Vec<medical_core::types::ModelInfo>> {
    let provider = {
        let registry = state.ai_providers.lock().await;
        match provider_name {
            Some(name) => registry.get_arc(&name),
            None => registry.get_active_arc(),
        }
    };
    let provider = provider
        .ok_or_else(|| AppError::AiProvider("Provider not found or not configured".to_string()))?;
    provider.available_models().await
}

#[cfg(test)]
mod preflight_tests {
    use std::sync::Arc;

    use medical_core::error::{AppError, OfflineReason, ServiceKind};
    use medical_core::types::settings::AppConfig;
    use medical_db::settings::SettingsRepo;
    use tokio::sync::{Mutex, RwLock};

    use crate::state::AppState;

    use super::*;

    /// Build a minimal `AppState` backed by an in-memory SQLite database and
    /// a pre-saved `AppConfig`. Used exclusively for pre-flight tests.
    ///
    /// `provider` is the provider name (e.g. `"ollama"`), `host` and `port`
    /// control where connection attempts are directed.
    async fn build_chat_test_state(
        provider: &str,
        host: &str,
        port: u16,
    ) -> (AppState, tempfile::TempDir) {
        let mut config = AppConfig::default();
        config.ai_provider = provider.to_string();
        config.ollama_host = host.to_string();
        config.ollama_port = port;
        config.ai_model = "llama3".to_string();
        // Tests use non-localhost addresses (e.g. TEST-NET 192.0.2.1); allow
        // them so the provider can be registered and the offline path exercised.
        config.allow_public_endpoint = true;

        let db = Arc::new(medical_db::Database::open_in_memory().expect("open in-memory db"));
        {
            let conn = db.conn().expect("conn");
            SettingsRepo::save_config(&conn, &config).expect("save_config");
        }

        let mut registry = medical_ai_providers::ProviderRegistry::new();
        let ollama_host = if config.ollama_host.is_empty() {
            "localhost".to_string()
        } else {
            config.ollama_host.clone()
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

        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path().join("config");
        let keys = medical_security::key_storage::KeyStorage::open(&config_dir)
            .expect("KeyStorage::open");

        let embedding_generator = Arc::new(
            medical_rag::embeddings::EmbeddingGenerator::new_ollama(None, None)
                .expect("reqwest client build in test"),
        );
        let vector_store = Arc::new(medical_rag::vector_store::VectorStore::new(Arc::clone(&db)));
        let bm25_search = Arc::new(medical_rag::bm25::Bm25Search::new(Arc::clone(&db)));
        let graph_search = Arc::new(medical_rag::graph_search::GraphSearch::new(Arc::clone(&db)));
        let ingestion = Arc::new(medical_rag::ingestion::IngestionPipeline::new(
            Arc::clone(&embedding_generator),
            Arc::clone(&vector_store),
            Arc::clone(&graph_search),
        ));
        let tool_registry = medical_agents::tools::ToolRegistry::with_defaults();
        let orchestrator = Arc::new(medical_agents::orchestrator::AgentOrchestrator::new(tool_registry));
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
            embedding_generator,
            vector_store,
            bm25_search,
            graph_search,
            ingestion,
            sharing: Arc::new(RwLock::new(None)),
            vocab_api: RwLock::new(None),
            ollama_provider: RwLock::new(None),
            lmstudio_provider: RwLock::new(None),
            remote_stt_provider: RwLock::new(None),
            http_client,
        };

        (state, tmp)
    }

    #[tokio::test]
    async fn chat_send_returns_endpoint_offline_when_ai_unreachable() {
        // 192.0.2.1 is RFC 5737 TEST-NET-1 — guaranteed unrouteable, so
        // the probe times out within PROBE_TIMEOUT (3s).
        let (state, _tmp) = build_chat_test_state("ollama", "192.0.2.1", 11434).await;

        let start = std::time::Instant::now();
        let result = chat_send_inner(
            &state,
            vec![ChatMessageInput {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            None,
            None,
        )
        .await;
        let elapsed = start.elapsed();

        let err = result.expect_err("must fail with offline error");
        match err {
            AppError::EndpointOffline {
                service,
                reason,
                provider_name,
                ..
            } => {
                assert_eq!(service, ServiceKind::AiProvider);
                assert_eq!(provider_name, "Ollama");
                assert!(
                    matches!(
                        reason,
                        OfflineReason::ConnectionRefused | OfflineReason::Timeout
                    ),
                    "expected ConnectionRefused or Timeout, got {reason:?}"
                );
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }

        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "should have short-circuited at ~3s; took {elapsed:?}"
        );
    }

    /// Verify that `chat_with_agent_inner` short-circuits with `EndpointOffline`
    /// when the AI provider is unreachable — exercises the actual pre-flight call
    /// site inside `chat_with_agent`'s code path (unlike the previous test which
    /// called `preflight_for_command` directly and was redundant with Task 3).
    #[tokio::test]
    async fn chat_with_agent_inner_returns_endpoint_offline_when_ai_unreachable() {
        // 192.0.2.1 is RFC 5737 TEST-NET-1 — guaranteed unrouteable.
        let (state, _tmp) = build_chat_test_state("ollama", "192.0.2.1", 11434).await;

        let start = std::time::Instant::now();
        let result = chat_with_agent_inner(
            &state,
            "Hello".to_string(),
            "chat".to_string(),
            None,
        )
        .await;
        let elapsed = start.elapsed();

        let err = result.expect_err("must fail with offline error");
        match err {
            AppError::EndpointOffline {
                service,
                provider_name,
                reason,
                ..
            } => {
                assert_eq!(service, ServiceKind::AiProvider);
                assert_eq!(provider_name, "Ollama");
                assert!(
                    matches!(
                        reason,
                        OfflineReason::ConnectionRefused | OfflineReason::Timeout
                    ),
                    "expected ConnectionRefused or Timeout, got {reason:?}"
                );
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }

        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "should short-circuit; took {elapsed:?}"
        );
    }
}
