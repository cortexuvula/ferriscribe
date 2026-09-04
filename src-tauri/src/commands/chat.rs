use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
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
use super::chat_docs;

const MAX_HISTORY_CHARS: usize = 200_000;

/// Per-request cap on conversation history actually SENT (~15k tokens at
/// 4 chars/token). [`trim_history`] keeps the newest whole messages within
/// this budget; documents are re-attached fresh every turn, so dropping
/// ancient turns degrades gracefully instead of tripping the model's
/// context limit mid-session (previously a loud context-exceeded error at
/// hour two of a long chart review).
const HISTORY_TRIM_CHARS: usize = 60_000;

/// Sliding-window history trim: whole-message granularity, newest kept,
/// the current (last) message always survives even if it alone exceeds the
/// budget — the hard caps and the provider's context-exceeded error remain
/// the backstop for that case. Pure; unit-tested.
fn trim_history(messages: Vec<ChatMessageInput>) -> Vec<ChatMessageInput> {
    let total: usize = messages.iter().map(|m| m.content.len()).sum();
    if total <= HISTORY_TRIM_CHARS {
        return messages;
    }
    let mut kept: Vec<ChatMessageInput> = Vec::new();
    let mut budget: usize = HISTORY_TRIM_CHARS;
    for m in messages.into_iter().rev() {
        let cost = m.content.len();
        if !kept.is_empty() && cost > budget {
            break;
        }
        budget = budget.saturating_sub(cost);
        kept.push(m);
    }
    kept.reverse();
    kept
}

/// Default system prompt for the chat tab, applied when the caller sends
/// none (the live UI path). Establishes the clinical-support role and the
/// hardened anti-fabrication stance used across the app's generators —
/// before this, chat ran the raw model with zero medical framing. Static
/// text only, no patient content (PHI rule); a caller-provided prompt
/// replaces it wholesale. The documents-drop feature (phase 1+) will append
/// its grounding section on top of this.
const DEFAULT_CHAT_SYSTEM_PROMPT: &str = "\
You are a clinical documentation assistant inside a local, offline medical records \
application used by healthcare professionals. The user may paste or drop patient \
material into this conversation; treat everything as confidential clinical information.

Rules:
- Ground every answer in the information provided in this conversation. Never \
fabricate facts, findings, values, dates, medications, or citations.
- If the conversation's material does not contain the answer, say so plainly. You \
may then offer well-established general medical knowledge, but clearly label it as \
background rather than as coming from the user's material.
- Keep what the user's material states and your own inferences visibly separate.
- You are clinical decision support, not a substitute for professional judgment. \
All outputs must be reviewed by a licensed healthcare provider before clinical use.";

/// Caller prompt wins; otherwise the default grounding prompt applies.
fn resolve_system_prompt(user: Option<String>) -> Option<String> {
    Some(user.unwrap_or_else(|| DEFAULT_CHAT_SYSTEM_PROMPT.to_string()))
}

/// A document attached to the conversation by the chat UI (OCR'd on the
/// frontend, passed verbatim). `name` is the source filename; `content` is
/// the extracted text.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChatDocumentInput {
    pub name: String,
    pub content: String,
}

/// Hard ceiling on total document text per request — same value as the
/// Letter Writer's source-document cap. The frontend additionally enforces
/// a much tighter token budget for its context-stuffing mode; this is the
/// backend's fail-safe against a misbehaving client.
const MAX_CHAT_DOCUMENT_CHARS: usize = 500_000;

/// Build the "Provided documents" prompt section. None when there are no
/// documents. Content is embedded verbatim — it never appears in logs
/// (PHI); only lengths are safe to log.
fn build_document_section(docs: &[ChatDocumentInput]) -> Option<String> {
    if docs.is_empty() {
        return None;
    }
    let mut section = String::from(
        "\n\n## Provided documents\n\n\
         The user attached the following documents to this conversation. Ground \
         answers in them, cite the document name when quoting or paraphrasing, and \
         say plainly when they do not contain the answer.\n\n",
    );
    for d in docs {
        section.push_str(&format!("--- Document: {} ---\n{}\n\n", d.name, d.content));
    }
    Some(section)
}

/// Resolve the final system prompt: caller prompt (or the default grounding
/// prompt) plus the documents section when documents are attached. Enforces
/// the total document size cap.
fn compose_system_prompt(
    user: Option<String>,
    documents: Option<&[ChatDocumentInput]>,
) -> AppResult<String> {
    let mut prompt = resolve_system_prompt(user).expect("resolver always returns Some");
    if let Some(docs) = documents {
        let total: usize = docs.iter().map(|d| d.name.len() + d.content.len()).sum();
        if total > MAX_CHAT_DOCUMENT_CHARS {
            return Err(AppError::Other(format!(
                "Chat documents too large: {total} chars, limit is {MAX_CHAT_DOCUMENT_CHARS}. Trim or remove documents."
            )));
        }
        if let Some(section) = build_document_section(docs) {
            prompt.push_str(&section);
        }
    }
    Ok(prompt)
}

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
    /// Identifies the emitting stream so the frontend can discard events
    /// from a previous, still-draining stream after a tab switch or a
    /// superseding send (backend workers outlive their listeners).
    stream_id: String,
    content: String,
}

/// Payload emitted when streaming completes.
#[derive(Debug, Clone, Serialize)]
struct DonePayload {
    stream_id: String,
    usage: Option<UsageInfo>,
    finish_reason: Option<String>,
}

/// Payload emitted on streaming errors.
#[derive(Debug, Clone, Serialize)]
struct ErrorPayload {
    stream_id: String,
    message: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    let cfg = crate::commands::load_app_config(&state.db, "chat").await?;
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
    .ok_or_else(|| AppError::ai_provider("No active AI provider configured".to_string()))?;

    let core_messages = convert_messages(messages);

    let request = CompletionRequest {
        model: model.unwrap_or(settings_model),
        messages: core_messages,
        temperature: Some(settings_temp),
        max_tokens: Some(4096),
        system_prompt: resolve_system_prompt(system_prompt),
        reasoning_effort: None,
    };

    debug!("chat_send: calling provider '{}'", provider.name());

    let response = provider.complete(request).await.map_err(|e| match e {
        // Preserve EndpointOffline as-is so the frontend dialog can fire.
        AppError::EndpointOffline { .. } => e,
        // For other errors, keep the existing nicer wrapping.
        _ => AppError::ai_provider(format!(
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
/// Emits the following events on the given `AppHandle`, every payload
/// carrying the `stream_id` supplied by the caller so the frontend can
/// filter out events from a previous, still-draining stream:
/// - `chat-token`  — for each text delta (`TokenPayload`)
/// - `chat-done`   — when the stream finishes (`DonePayload`)
/// - `chat-error`  — on error (`ErrorPayload`)
///
/// Only one stream is active at a time: starting a new one cancels the
/// previous worker (its remaining tokens are discarded server-side rather
/// than leaking into the new stream's global events).
#[tauri::command]
pub async fn chat_stream(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    messages: Vec<ChatMessageInput>,
    model: Option<String>,
    system_prompt: Option<String>,
    documents: Option<Vec<ChatDocumentInput>>,
    stream_id: Option<String>,
) -> AppResult<()> {
    check_history_size(&messages)?;

    // Register this stream as the active one, cancelling any predecessor.
    let stream_id = stream_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let cancel = {
        let mut slot = state.chat_stream_cancel.lock().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        if let Some((_, prev)) = slot.replace((stream_id.clone(), cancel.clone())) {
            prev.cancel();
            tracing::info!("chat_stream: superseding previous stream; cancelling it");
        }
        cancel
    };

    // Load full config for pre-flight (also provides model/temperature).
    let cfg = crate::commands::load_app_config(&state.db, "chat").await?;
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
    .ok_or_else(|| AppError::ai_provider("No active AI provider configured".to_string()))?;

    // Retrieval query for chart-review mode — composed BEFORE `messages`
    // is consumed. Includes a slice of the preceding turn so follow-ups
    // keep their conversational referents (see compose_retrieval_query).
    let retrieval_query = chat_docs::compose_retrieval_query(&messages);
    // Sliding-window trim keeps the request inside realistic context
    // budgets on long sessions.
    let messages = trim_history(messages);
    let core_messages = convert_messages(messages);

    // Document mode: stuff whole documents when they fit the context
    // budget; retrieve cited excerpts when they don't (chart-review
    // scale — 300-600 page drops). See commands/chat_docs.rs.
    let doc_chars = documents
        .as_deref()
        .map(chat_docs::documents_total_chars)
        .unwrap_or(0);
    let resolved_system = if doc_chars > chat_docs::STUFFING_CHAR_LIMIT {
        if doc_chars > chat_docs::MAX_RETRIEVAL_CHAR_LIMIT {
            return Err(AppError::Other(format!(
                "Chat documents too large: {doc_chars} chars, limit is {}. Split the chart into smaller drops.",
                chat_docs::MAX_RETRIEVAL_CHAR_LIMIT
            )));
        }
        let base = resolve_system_prompt(system_prompt).expect("resolver always returns Some");
        let mut slot = state.chat_doc_index.lock().await;
        let docs = documents.as_deref().expect("non-empty checked above");
        if slot.as_ref().is_none_or(|i| !i.matches(docs)) {
            let embeddings = chat_docs::embeddings_for_config(&cfg)?;
            *slot = Some(chat_docs::ChatDocIndex::build(docs, embeddings).await?);
        }
        let index = slot.as_ref().expect("just built or reused");
        let excerpts = index.retrieve(&retrieval_query).await?;
        let mut prompt = base;
        prompt.push_str(&chat_docs::build_excerpt_section(&excerpts));
        prompt
    } else {
        compose_system_prompt(system_prompt, documents.as_deref())?
    };

    let request = CompletionRequest {
        model: model.unwrap_or(settings_model),
        messages: core_messages,
        temperature: Some(settings_temp),
        max_tokens: Some(4096),
        system_prompt: Some(resolved_system),
        reasoning_effort: None,
    };

    debug!("chat_stream: calling provider '{}'", provider.name());

    let mut stream = provider
        .complete_stream(request)
        .await
        .map_err(|e| match e {
            // Preserve EndpointOffline as-is so the frontend dialog can fire.
            AppError::EndpointOffline { .. } => e,
            // For other errors, keep the existing nicer wrapping.
            _ => AppError::ai_provider(format!(
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
    let worker_stream_id = stream_id.clone();
    let worker_cancel = cancel.clone();
    let worker = tokio::spawn(async move {
        // Tracks whether the worker has already emitted a terminal `chat-done`
        // event. If the provider closes the SSE stream without ever sending a
        // `Usage` or `Done` chunk (e.g. a server crash mid-stream), the loop
        // below exits normally and `emitted_done` stays false — in which case
        // we emit a `chat-done` ourselves so the frontend spinner doesn't hang.
        let mut emitted_done = false;
        loop {
            let chunk = tokio::select! {
                biased;
                _ = worker_cancel.cancelled() => {
                    // Frontend-driven cancel (tab switch with cleanup, or a
                    // superseding send). Emit a terminal event so any still-
                    // listening consumer stops its spinner.
                    let _ = worker_app.emit(
                        "chat-done",
                        DonePayload {
                            stream_id: worker_stream_id.clone(),
                            usage: None,
                            finish_reason: Some("cancelled".to_string()),
                        },
                    );
                    emitted_done = true;
                    break;
                }
                next = stream.next() => match next {
                    Some(result) => result,
                    None => break,
                },
            };
            match chunk {
                Ok(chunk) => match chunk {
                    StreamChunk::Delta { text } => {
                        let _ = worker_app.emit(
                            "chat-token",
                            TokenPayload {
                                stream_id: worker_stream_id.clone(),
                                content: text,
                            },
                        );
                    }
                    StreamChunk::ToolCallDelta { .. } => {
                        // Tool-call deltas are not surfaced in the basic chat stream.
                    }
                    StreamChunk::ReasoningDelta { .. } => {
                        // Reasoning deltas carry only a length; nothing to emit.
                    }
                    StreamChunk::Usage(usage) => {
                        let _ = worker_app.emit(
                            "chat-done",
                            DonePayload {
                                stream_id: worker_stream_id.clone(),
                                usage: Some(usage),
                                finish_reason: Some("stop".to_string()),
                            },
                        );
                        emitted_done = true;
                    }
                    StreamChunk::Done => {
                        let _ = worker_app.emit(
                            "chat-done",
                            DonePayload {
                                stream_id: worker_stream_id.clone(),
                                usage: None,
                                finish_reason: Some("stop".to_string()),
                            },
                        );
                        emitted_done = true;
                    }
                },
                Err(e) => {
                    let msg = super::unwrap_app_error_message_ref(&e);
                    error!("chat_stream error: {msg}");
                    return Err(e);
                }
            }
        }
        // If the provider closed the SSE stream without sending a terminal
        // chunk, emit `chat-done` so the frontend stops its spinner.
        if !emitted_done {
            error!("chat_stream: stream ended without a usage/Done chunk; emitting chat-done");
            let _ = worker_app.emit(
                "chat-done",
                DonePayload {
                    stream_id: worker_stream_id.clone(),
                    usage: None,
                    finish_reason: Some("stream ended".to_string()),
                },
            );
        }
        Ok::<(), AppError>(())
    });

    // Supervisor: ensures the UI always sees a terminal event even if the
    // worker task panics, is cancelled, or returns an error. The worker emits
    // `chat-done` itself on clean completion; we only emit `chat-error` here.
    // Also clears the active-stream slot so a later `chat_cancel_stream`
    // doesn't fire a stale token.
    let supervisor_app = app.clone();
    let supervisor_stream_id = stream_id.clone();
    let supervisor_state_cancel = std::sync::Arc::clone(&state.chat_stream_cancel);
    tokio::spawn(async move {
        let result = worker.await;
        {
            let mut slot = supervisor_state_cancel.lock().await;
            if slot
                .as_ref()
                .is_some_and(|(id, _)| *id == supervisor_stream_id)
            {
                *slot = None;
            }
        }
        match result {
            Ok(Ok(())) => {
                // Normal completion — worker already emitted `chat-done`.
            }
            Ok(Err(e)) => {
                let _ = supervisor_app.emit(
                    "chat-error",
                    ErrorPayload {
                        stream_id: supervisor_stream_id.clone(),
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
                let _ = supervisor_app.emit(
                    "chat-error",
                    ErrorPayload {
                        stream_id: supervisor_stream_id.clone(),
                        message: msg,
                    },
                );
            }
        }
    });

    Ok(())
}

/// Cancel the active chat stream (if any). Emits nothing itself — the
/// worker's cancel branch emits the terminal `chat-done`.
#[tauri::command]
pub async fn chat_cancel_stream(state: tauri::State<'_, AppState>) -> AppResult<()> {
    let slot = state.chat_stream_cancel.lock().await;
    if let Some((id, token)) = slot.as_ref() {
        tracing::info!(stream_id = %id, "chat_cancel_stream: cancelling active stream");
        token.cancel();
    }
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
        .ok_or_else(|| AppError::agent(format!("Unknown agent: '{agent_name}'")))?;

    let provider = {
        let registry = state.ai_providers.lock().await;
        registry.get_active_arc()
    }
    .ok_or_else(|| AppError::ai_provider("No active AI provider configured".to_string()))?;

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
    // also read from here).
    let cfg = crate::commands::load_app_config(&state.db, "chat").await?;
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
        other => AppError::agent(format!(
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
            other => AppError::agent(format!(
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
pub async fn list_ai_providers(state: tauri::State<'_, AppState>) -> AppResult<Vec<String>> {
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
        .ok_or_else(|| AppError::ai_provider("Provider not found or not configured".to_string()))?;
    provider.available_models().await
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn default_grounding_prompt_applies_when_caller_sends_none() {
        let resolved = resolve_system_prompt(None).expect("always Some");
        assert!(
            resolved.contains("Never fabricate"),
            "anti-fabrication rule"
        );
        assert!(
            resolved.contains("licensed healthcare provider"),
            "guardrail"
        );
        // No PHI-shaped content in the static prompt itself.
        assert!(!resolved.contains("patient_name"));
    }

    #[test]
    fn caller_prompt_replaces_default_wholesale() {
        assert_eq!(
            resolve_system_prompt(Some("custom".into())).as_deref(),
            Some("custom")
        );
    }

    fn doc(name: &str, content: &str) -> ChatDocumentInput {
        ChatDocumentInput {
            name: name.into(),
            content: content.into(),
        }
    }

    #[test]
    fn documents_section_appended_after_grounding_prompt() {
        let prompt = compose_system_prompt(None, Some(&[doc("consult.pdf", "Cardiology says hi")]))
            .expect("compose");
        assert!(
            prompt.contains("Never fabricate"),
            "grounding base retained"
        );
        assert!(prompt.contains("## Provided documents"));
        assert!(prompt.contains("--- Document: consult.pdf ---"));
        assert!(prompt.contains("Cardiology says hi"));
        // Grounding text comes first, documents after.
        assert!(prompt.find("Never fabricate") < prompt.find("consult.pdf"));
    }

    #[test]
    fn trim_history_passes_short_conversations_through_untouched() {
        let msgs = vec![
            ChatMessageInput {
                role: "user".into(),
                content: "hi".into(),
            },
            ChatMessageInput {
                role: "assistant".into(),
                content: "hello".into(),
            },
        ];
        let trimmed = trim_history(msgs.clone());
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0].content, "hi");
    }

    #[test]
    fn trim_history_drops_oldest_whole_messages_keeps_newest() {
        let big = "x".repeat(30_000); // 3 messages x 30k = 90k > 60k budget
        let msgs = vec![
            ChatMessageInput {
                role: "user".into(),
                content: big.clone(),
            },
            ChatMessageInput {
                role: "assistant".into(),
                content: big.clone(),
            },
            ChatMessageInput {
                role: "user".into(),
                content: "keep me".into(),
            },
            ChatMessageInput {
                role: "assistant".into(),
                content: "final".into(),
            },
        ];
        let trimmed = trim_history(msgs);
        // Oldest (30k) message is dropped; the newest three fit the budget.
        assert_eq!(trimmed.len(), 3);
        assert_eq!(trimmed.first().unwrap().content, big);
        assert_eq!(trimmed.last().unwrap().content, "final");
    }

    #[test]
    fn trim_history_always_keeps_the_current_message() {
        let msgs = vec![ChatMessageInput {
            role: "user".into(),
            content: "y".repeat(HISTORY_TRIM_CHARS + 10_000),
        }];
        let trimmed = trim_history(msgs);
        assert_eq!(trimmed.len(), 1, "the current question always survives");
    }

    #[test]
    fn no_documents_leaves_prompt_unchanged() {
        let with_none = compose_system_prompt(None, None).expect("compose");
        assert_eq!(with_none, DEFAULT_CHAT_SYSTEM_PROMPT);
        let with_empty = compose_system_prompt(None, Some(&[])).expect("compose");
        assert_eq!(with_empty, DEFAULT_CHAT_SYSTEM_PROMPT);
    }

    #[test]
    fn oversized_documents_are_rejected() {
        let big = "x".repeat(MAX_CHAT_DOCUMENT_CHARS + 1);
        let err =
            compose_system_prompt(None, Some(&[doc("big.pdf", &big)])).expect_err("must reject");
        assert!(err.to_string().contains("too large"));
    }
}

#[cfg(test)]
mod preflight_tests {
    use std::sync::Arc;

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
        let keys =
            medical_security::key_storage::KeyStorage::open(&config_dir).expect("KeyStorage::open");

        let tool_registry = medical_agents::tools::ToolRegistry::with_defaults();
        let orchestrator = Arc::new(medical_agents::orchestrator::AgentOrchestrator::new(
            tool_registry,
        ));
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
            generation_locks: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            sharing: Arc::new(RwLock::new(None)),
            sharing_lifecycle: Arc::new(tokio::sync::Mutex::new(())),
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
        crate::commands::generation::test_helpers::assert_endpoint_offline(result, "Ollama", start);
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
        let result =
            chat_with_agent_inner(&state, "Hello".to_string(), "chat".to_string(), None).await;
        crate::commands::generation::test_helpers::assert_endpoint_offline(result, "Ollama", start);
    }
}
