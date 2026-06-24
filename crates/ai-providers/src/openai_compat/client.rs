//! `OpenAiCompatibleClient` struct + constructors + request/response helpers.

use reqwest::Client;
use tracing::warn;

use medical_core::types::{
    CompletionRequest, CompletionResponse, Message, MessageContent, Role, ToolCall, UsageInfo,
};

use crate::http_client::RetryConfig;

use super::wire::{ApiFunction, ApiToolCall, ChatMessage, ChatRequest, ChatResponse};

/// A client for any endpoint implementing the OpenAI chat-completions protocol.
///
/// This is the workhorse of the `ai-providers` crate. Both [`OllamaProvider`]
/// and [`LmStudioProvider`] delegate to an instance of this client for all
/// HTTP communication. It handles:
///
/// - Converting core [`CompletionRequest`] into the OpenAI wire format
///   (`ChatRequest`) via `build_request`.
/// - Parsing OpenAI wire responses (`ChatResponse`) back into core
///   [`CompletionResponse`] via `parse_response`.
/// - Bearer-token authentication via the `Authorization` header.
/// - Error classification: connectivity errors become `EndpointOffline`,
///   application-layer errors become `AiProvider(String)`.
///
/// The client is intentionally provider-agnostic — it works with any server
/// that speaks the OpenAI chat-completions protocol (Ollama, LM Studio,
/// vLLM, etc.), subject to the local-only constraint enforced at the
/// provider level.
///
/// [`OllamaProvider`]: crate::ollama::OllamaProvider
/// [`LmStudioProvider`]: crate::lmstudio::LmStudioProvider
pub struct OpenAiCompatibleClient {
    /// The underlying reqwest HTTP client with connection pooling.
    pub client: Client,
    /// Base URL including the `/v1` suffix (e.g., `http://localhost:11434/v1`).
    /// Updated dynamically by providers that support LAN/Tailscale resolution.
    pub base_url: String,
    /// Retry policy controlling exponential backoff on transient failures.
    pub policy: RetryConfig,
    /// Optional bearer token sent as `Authorization: Bearer <token>`.
    /// Updated when `set_endpoint()` is called on a provider.
    pub bearer: Option<String>,
    /// Human-readable provider name used in `EndpointOffline` errors (e.g. "Ollama").
    /// Surfaced to the user in connection-error dialogs.
    pub provider_name: String,
}

impl OpenAiCompatibleClient {
    /// Create a client without authentication or a provider name.
    ///
    /// Suitable for local providers that don't require auth tokens.
    /// The `base_url` should include the `/v1` suffix.
    pub fn new(client: Client, base_url: impl Into<String>, policy: RetryConfig) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            policy,
            bearer: None,
            provider_name: String::new(),
        }
    }

    /// Create a client with an optional bearer token.
    ///
    /// The bearer is sent as `Authorization: Bearer <token>` on every request.
    /// Pass `None` for unauthenticated local providers.
    pub fn new_with_bearer(
        client: Client,
        base_url: impl Into<String>,
        policy: RetryConfig,
        bearer: Option<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            policy,
            bearer,
            provider_name: String::new(),
        }
    }

    /// Create a client with an optional bearer token and a human-readable provider name.
    ///
    /// The `provider_name` appears in `EndpointOffline` error messages shown
    /// to the user (e.g., "Ollama is not reachable at …").
    pub fn new_with_bearer_and_name(
        client: Client,
        base_url: impl Into<String>,
        policy: RetryConfig,
        bearer: Option<String>,
        provider_name: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            policy,
            bearer,
            provider_name: provider_name.into(),
        }
    }

    /// Convert a core `Message` into the OpenAI wire format (`ChatMessage`).
    /// For assistant messages that carry `tool_calls`, we include them so that
    /// subsequent `tool` role messages can reference the tool_call_id.
    fn convert_message(msg: &Message) -> ChatMessage {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };

        match &msg.content {
            MessageContent::Text(text) => {
                // For assistant messages with tool_calls, the content may be null/empty
                // and the tool_calls must be forwarded.
                let api_tool_calls: Option<Vec<ApiToolCall>> = if msg.tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        msg.tool_calls
                            .iter()
                            .map(|tc| ApiToolCall {
                                id: tc.id.clone(),
                                kind: "function".into(),
                                function: ApiFunction {
                                    name: tc.name.clone(),
                                    arguments: tc.arguments.to_string(),
                                },
                            })
                            .collect(),
                    )
                };
                // Content is null (not present) when tool_calls are the primary payload,
                // but some providers tolerate an empty string; send None when tool_calls
                // are present and text is empty to stay spec-compliant.
                let content = if text.is_empty() && api_tool_calls.is_some() {
                    None
                } else {
                    Some(serde_json::Value::String(text.clone()))
                };
                ChatMessage {
                    role: role.into(),
                    content,
                    tool_call_id: None,
                    tool_calls: api_tool_calls,
                }
            }
            MessageContent::ToolResult {
                tool_call_id,
                content,
            } => ChatMessage {
                role: "tool".into(),
                content: Some(serde_json::Value::String(content.clone())),
                tool_call_id: Some(tool_call_id.clone()),
                tool_calls: None,
            },
        }
    }

    pub(super) fn build_request(&self, request: &CompletionRequest) -> ChatRequest {
        let mut messages: Vec<ChatMessage> = Vec::new();

        // Inject system prompt as first message if present
        if let Some(sys) = &request.system_prompt {
            messages.push(ChatMessage {
                role: "system".into(),
                content: Some(serde_json::Value::String(sys.clone())),
                tool_call_id: None,
                tool_calls: None,
            });
        }

        for msg in &request.messages {
            messages.push(Self::convert_message(msg));
        }

        ChatRequest {
            model: request.model.clone(),
            messages,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: None,
            tools: None,
            stream_options: None,
        }
    }

    pub(super) fn parse_response(
        &self,
        resp: ChatResponse,
        default_model: &str,
    ) -> CompletionResponse {
        let model = resp.model.unwrap_or_else(|| default_model.to_string());
        let usage = resp
            .usage
            .map(|u| UsageInfo {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            })
            .unwrap_or_default();

        let num_choices = resp.choices.len();
        let first_choice = resp.choices.into_iter().next();

        if first_choice.is_none() {
            warn!(
                model = %model,
                "AI response contained no choices (choices array was empty)"
            );
        }

        let finish_reason = first_choice.as_ref().and_then(|c| c.finish_reason.clone());

        let content = first_choice
            .as_ref()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone())
            .unwrap_or_default();

        if content.is_empty() && num_choices > 0 {
            warn!(
                model = %model,
                finish_reason = ?finish_reason,
                has_message = first_choice.as_ref().and_then(|c| c.message.as_ref()).is_some(),
                "AI response content is empty (choices={num_choices}, finish_reason={finish_reason:?})"
            );
        }

        let tool_calls = first_choice
            .as_ref()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.tool_calls.as_ref())
            .map(|tcs| {
                tcs.iter()
                    .map(|tc| ToolCall {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        arguments: serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(serde_json::Value::Null),
                    })
                    .collect()
            })
            .unwrap_or_default();

        CompletionResponse {
            content,
            model,
            usage,
            tool_calls,
        }
    }

    /// Classify a reqwest error from a `send_with_retry` call into either a
    /// structured `EndpointOffline` (connectivity issue) or the existing
    /// `AiProvider(String)` shape (genuine application-layer error).
    pub(crate) fn classify_send_error(&self, e: reqwest::Error) -> medical_core::error::AppError {
        use medical_core::error::ServiceKind;
        use medical_core::preflight::classify_reqwest_error;
        match classify_reqwest_error(&e) {
            Some(reason) => medical_core::error::AppError::EndpointOffline {
                service: ServiceKind::AiProvider,
                endpoint: self.base_url.clone(),
                reason,
                provider_name: self.provider_name.clone(),
            },
            None => medical_core::error::AppError::AiProvider(format!("HTTP request failed: {e}")),
        }
    }

    /// Build an authorized `RequestBuilder` for a GET request.
    pub(super) fn get(&self, url: &str) -> reqwest::RequestBuilder {
        let mut req = self.client.get(url);
        if let Some(b) = &self.bearer {
            req = req.bearer_auth(b);
        }
        req
    }

    /// Build an authorized `RequestBuilder` for a POST request with a JSON body.
    pub(super) fn post_json<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
    ) -> reqwest::RequestBuilder {
        let mut req = self.client.post(url).json(body);
        if let Some(b) = &self.bearer {
            req = req.bearer_auth(b);
        }
        req
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::{Message, MessageContent, Role};

    use super::super::wire::{ApiUsage, ChatChoice, ChatResponseMessage};

    fn make_client() -> OpenAiCompatibleClient {
        // Build without real auth — only used for struct-level tests.
        OpenAiCompatibleClient::new(
            Client::new(),
            "https://api.openai.com/v1",
            RetryConfig::default(),
        )
    }

    fn make_request() -> CompletionRequest {
        CompletionRequest {
            model: "gpt-4o".into(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".into()),
                tool_calls: vec![],
            }],
            temperature: None,
            max_tokens: None,
            system_prompt: Some("You are a helpful assistant.".into()),
        }
    }

    #[test]
    fn build_request_includes_system_prompt() {
        let c = make_client();
        let req = make_request();
        let chat_req = c.build_request(&req);
        assert_eq!(chat_req.messages[0].role, "system");
        assert_eq!(
            chat_req.messages[0].content,
            Some(serde_json::Value::String(
                "You are a helpful assistant.".into()
            ))
        );
        assert_eq!(chat_req.messages[1].role, "user");
    }

    #[test]
    fn stream_flag() {
        let c = make_client();
        let req = make_request();
        let mut chat_req = c.build_request(&req);
        assert!(chat_req.stream.is_none());
        chat_req.stream = Some(true);
        assert_eq!(chat_req.stream, Some(true));
    }

    #[test]
    fn parse_response_extracts_content() {
        let c = make_client();
        let resp = ChatResponse {
            model: Some("gpt-4o".into()),
            choices: vec![ChatChoice {
                message: Some(ChatResponseMessage {
                    content: Some("The answer is 42.".into()),
                    tool_calls: None,
                }),
                delta: None,
                finish_reason: Some("stop".into()),
            }],
            usage: Some(ApiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        let completion = c.parse_response(resp, "gpt-4o");
        assert_eq!(completion.content, "The answer is 42.");
        assert_eq!(completion.model, "gpt-4o");
        assert_eq!(completion.usage.total_tokens, 15);
        assert!(completion.tool_calls.is_empty());
    }
}
