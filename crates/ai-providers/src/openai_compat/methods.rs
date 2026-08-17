//! Public methods on [`OpenAiCompatibleClient`]: list_models, complete,
//! complete_stream, complete_with_tools.

use std::pin::Pin;

use futures_core::Stream;
use futures_util::StreamExt;
use tracing::{debug, warn};

use medical_core::{
    error::{AppError, AppResult},
    types::{
        CompletionRequest, CompletionResponse, StreamChunk, ToolCall, ToolCompletionResponse,
        ToolDef, UsageInfo,
    },
};

use crate::sse::parse_sse_response;

use super::client::OpenAiCompatibleClient;
use super::wire::{ApiTool, ApiToolDef, ChatResponse, ModelsListResponse, StreamOptions};

/// Hard ceiling for a single streamed generation. Long enough for a
/// reasoning model to think + write; the meaningful limit is the
/// consumer-side idle timeout (no data for N seconds), not this cap.
const STREAM_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

impl OpenAiCompatibleClient {
    /// Fetch the list of model IDs from the `/models` endpoint.
    ///
    /// Calls `GET {base_url}/models` and extracts the `id` field from each
    /// entry in the `data` array. The returned list is sorted alphabetically.
    ///
    /// # Errors
    ///
    /// - `EndpointOffline` if the server is unreachable.
    /// - `AiProvider(String)` on HTTP errors or JSON parse failures.
    pub async fn list_models(&self) -> AppResult<Vec<String>> {
        let url = format!("{}/models", self.base_url);
        let response = crate::http_client::send_with_retry(&self.policy, || self.get(&url))
            .await
            .map_err(|e| self.classify_send_error(e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = medical_core::http_error_body::read_error_body(response, 200).await;
            return Err(AppError::ai_provider(format!("HTTP {status}: {text}")));
        }

        let resp: ModelsListResponse = response
            .json()
            .await
            .map_err(|e| AppError::ai_provider_with_source(e.to_string(), e))?;

        let mut ids: Vec<String> = resp.data.into_iter().map(|m| m.id).collect();
        ids.sort();
        Ok(ids)
    }

    /// Send a non-streaming chat completion request and return the full response.
    ///
    /// Posts a `ChatRequest` to `{base_url}/chat/completions` with `stream: null`,
    /// parses the JSON response into a [`CompletionResponse`], and extracts
    /// content, usage, model name, and any tool calls.
    ///
    /// # Context window detection
    ///
    /// When the response has choices but empty content and `finish_reason: "length"`,
    /// this method returns a descriptive error suggesting the model's context
    /// window was exceeded. This heuristic catches cases where the prompt is
    /// too long for the model to produce any output.
    ///
    /// # Errors
    ///
    /// - `EndpointOffline` if the server is unreachable after retries.
    /// - `AiProvider(String)` on HTTP errors, JSON parse failures, or
    ///   context-window-exceeded conditions.
    pub async fn complete(&self, request: &CompletionRequest) -> AppResult<CompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = self.build_request(request);

        let response =
            crate::http_client::send_with_retry(&self.policy, || self.post_json(&url, &body))
                .await
                .map_err(|e| self.classify_send_error(e))?;

        let status = response.status();
        // Read full body — used for both the error message (truncated) and
        // JSON parsing on success. The shared read_error_body helper isn't
        // appropriate here because it always truncates, which would corrupt
        // a successful response body before serde sees it.
        let raw_body = match response.text().await {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "failed to read response body");
                String::new()
            }
        };

        if !status.is_success() {
            let preview: String = raw_body.chars().take(200).collect();
            return Err(AppError::ai_provider(format!("HTTP {status}: {preview}")));
        }

        let resp: ChatResponse = serde_json::from_str(&raw_body).map_err(|e| {
            warn!(
                body_len = raw_body.len(),
                "Failed to parse AI response JSON"
            );
            AppError::ai_provider(format!("JSON parse error: {e}"))
        })?;

        debug!(
            url = %url,
            model = %request.model,
            choices = resp.choices.len(),
            "AI completion response received"
        );

        let finish_reason = resp
            .choices
            .first()
            .and_then(|c| c.finish_reason.as_deref())
            .unwrap_or("unknown");
        let has_content = resp
            .choices
            .first()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.as_ref())
            .map(|c| !c.is_empty())
            .unwrap_or(false);

        if !has_content && finish_reason == "length" {
            return Err(AppError::ai_provider(format!(
                "Model '{}' context window exceeded: the prompt is too long for the model, \
                 leaving no room for output. Try a model with a larger context window, \
                 reduce the prompt size, or increase the model's context length in LM Studio.",
                request.model,
            )));
        }

        Ok(self.parse_response(resp, &request.model))
    }

    /// Send a streaming chat completion request and return an SSE-backed stream.
    ///
    /// Posts a `ChatRequest` with `stream: true` and `stream_options.include_usage: true`
    /// to `{base_url}/chat/completions`. The response body is parsed as SSE
    /// via [`parse_sse_response`]. Each SSE data line is deserialized into a
    /// `ChatResponse` and mapped to one or more [`StreamChunk`] items:
    ///
    /// - `delta.content` → `StreamChunk::Delta { text }`
    /// - `delta.tool_calls` → `StreamChunk::ToolCallDelta { id, name, arguments_delta }`
    /// - `usage` (separate event) → `StreamChunk::Usage(...)` followed by `StreamChunk::Done`
    ///
    /// Malformed JSON lines are silently dropped (not propagated as errors).
    ///
    /// # Errors
    ///
    /// - `EndpointOffline` if the server is unreachable after retries.
    /// - `AiProvider(String)` on non-2xx HTTP responses.
    pub async fn complete_stream(
        &self,
        request: &CompletionRequest,
    ) -> AppResult<Pin<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send>>> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut body = self.build_request(request);
        body.stream = Some(true);
        body.stream_options = Some(StreamOptions {
            include_usage: true,
        });

        let response = crate::http_client::send_with_retry(&self.policy, || {
            self.post_json_with_timeout(&url, &body, STREAM_TOTAL_TIMEOUT)
        })
        .await
        .map_err(|e| self.classify_send_error(e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = medical_core::http_error_body::read_error_body(response, 200).await;
            return Err(AppError::ai_provider(format!("HTTP {status}: {text}")));
        }

        let sse = parse_sse_response(response);

        // Convert each SSE data line into zero or more StreamChunks.
        let mapped = sse
            .map(|item| -> Vec<AppResult<StreamChunk>> {
                match item {
                    Err(e) => vec![Err(AppError::ai_provider(e))],
                    Ok(data) => match serde_json::from_str::<ChatResponse>(&data) {
                        Err(e) => {
                            // Log the failure (error + byte length only — never
                            // the data itself, which may contain model output /
                            // clinical text). Propagate as a stream error so the
                            // consumer knows the stream was truncated, rather than
                            // silently dropping the chunk (which risked incomplete
                            // SOAP notes with no indication).
                            warn!(error = %e, data_len = data.len(), "malformed SSE event; propagating as stream error");
                            vec![Err(AppError::ai_provider(format!(
                                "malformed SSE event ({} bytes): {e}",
                                data.len()
                            )))]
                        }
                        Ok(resp) => map_chat_event(&resp),
                    },
                }
            })
            .flat_map(tokio_stream::iter);

        Ok(Box::pin(mapped))
    }

    /// Send a chat completion request with tool definitions and return the response.
    ///
    /// Posts a `ChatRequest` with a `tools` array to `{base_url}/chat/completions`.
    /// The response may contain text content, tool calls, or both. The agent
    /// orchestrator uses this to implement the tool-calling loop: the model
    /// can request tool invocations, the caller executes them, and the results
    /// are fed back as `tool` role messages in a subsequent request.
    ///
    /// # Tool call format
    ///
    /// Tools are sent in the OpenAI `function` tool format:
    /// ```json
    /// { "type": "function", "function": { "name": "...", "description": "...", "parameters": {...} } }
    /// ```
    ///
    /// # Errors
    ///
    /// - `EndpointOffline` if the server is unreachable after retries.
    /// - `AiProvider(String)` on HTTP errors or JSON parse failures.
    pub async fn complete_with_tools(
        &self,
        request: &CompletionRequest,
        tools: Vec<ToolDef>,
    ) -> AppResult<ToolCompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut body = self.build_request(request);
        body.tools = Some(
            tools
                .into_iter()
                .map(|t| ApiTool {
                    kind: "function".into(),
                    function: ApiToolDef {
                        name: t.name,
                        description: t.description,
                        parameters: t.parameters,
                    },
                })
                .collect(),
        );

        let response =
            crate::http_client::send_with_retry(&self.policy, || self.post_json(&url, &body))
                .await
                .map_err(|e| self.classify_send_error(e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = medical_core::http_error_body::read_error_body(response, 200).await;
            return Err(AppError::ai_provider(format!("HTTP {status}: {text}")));
        }

        let resp: ChatResponse = response
            .json()
            .await
            .map_err(|e| AppError::ai_provider_with_source(e.to_string(), e))?;

        let usage = resp
            .usage
            .map(|u| UsageInfo {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            })
            .unwrap_or_default();

        let first_choice = resp.choices.into_iter().next();

        let content = first_choice
            .as_ref()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone());

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

        Ok(ToolCompletionResponse {
            content,
            tool_calls,
            usage,
        })
    }
}

/// Map one parsed SSE `ChatResponse` event into stream chunks (pure; testable).
///
/// Handles only successfully parsed events; the malformed-JSON path stays in
/// `complete_stream`'s closure (it needs the raw `data` for `data_len`).
fn map_chat_event(resp: &ChatResponse) -> Vec<AppResult<StreamChunk>> {
    let mut out = Vec::new();
    if let Some(choice) = resp.choices.first()
        && let Some(delta) = &choice.delta
    {
        // Text delta
        if let Some(text) = &delta.content
            && !text.is_empty()
        {
            out.push(Ok(StreamChunk::Delta { text: text.clone() }));
        }
        // Reasoning/"thinking" delta — length only, never the text itself
        // (PHI: reasoning can echo clinical content; lengths are safe to emit).
        if let Some(reasoning) = &delta.reasoning_content
            && !reasoning.is_empty()
        {
            out.push(Ok(StreamChunk::ReasoningDelta {
                len: reasoning.len(),
            }));
        }
        // Tool-call deltas
        if let Some(tc_deltas) = &delta.tool_calls {
            for tc in tc_deltas {
                let id = tc.id.clone().unwrap_or_default();
                let name = tc.function.as_ref().and_then(|f| f.name.clone());
                let args_delta = tc
                    .function
                    .as_ref()
                    .and_then(|f| f.arguments.clone())
                    .unwrap_or_default();
                out.push(Ok(StreamChunk::ToolCallDelta {
                    id,
                    name,
                    arguments_delta: args_delta,
                }));
            }
        }
    }
    // Usage chunk (comes in a separate SSE event with usage data)
    if let Some(u) = &resp.usage {
        out.push(Ok(StreamChunk::Usage(UsageInfo {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        })));
        out.push(Ok(StreamChunk::Done));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::{Message, MessageContent, Role};
    use std::time::Duration;

    use crate::http_client::RetryConfig;

    fn fast_policy(max_retries: u32) -> RetryConfig {
        RetryConfig {
            max_retries,
            initial_delay: Duration::from_millis(20),
            backoff_factor: 2.0,
            max_delay: Duration::from_millis(200),
        }
    }

    fn build_test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("test client")
    }

    fn make_retry_request() -> CompletionRequest {
        CompletionRequest {
            model: "test-model".into(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
                tool_calls: vec![],
            }],
            temperature: Some(0.0),
            max_tokens: None,
            system_prompt: None,
        }
    }

    #[tokio::test]
    async fn complete_recovers_from_503() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "test-model",
                "choices": [{
                    "message": {"content": "hi back"},
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let client = OpenAiCompatibleClient::new(
            build_test_client(),
            format!("{}/v1", server.uri()),
            fast_policy(3),
        );

        let resp = client
            .complete(&make_retry_request())
            .await
            .expect("complete should recover");
        assert_eq!(resp.content, "hi back");
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn complete_does_not_retry_400() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .mount(&server)
            .await;

        let client = OpenAiCompatibleClient::new(
            build_test_client(),
            format!("{}/v1", server.uri()),
            fast_policy(3),
        );

        let err = client
            .complete(&make_retry_request())
            .await
            .expect_err("400 should be permanent");
        let msg = format!("{err}");
        assert!(msg.contains("400"), "expected 400 in error: {msg}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn complete_stream_retries_initial_send() {
        use futures_util::StreamExt;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;

        // First two POSTs to /v1/chat/completions return 503 — the initial
        // SSE send should retry them.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;

        // Third POST returns a minimal SSE stream with one delta and a usage chunk.
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                        data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n\
                        data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .mount(&server)
            .await;

        let client = OpenAiCompatibleClient::new(
            build_test_client(),
            format!("{}/v1", server.uri()),
            fast_policy(3),
        );

        let mut stream = client
            .complete_stream(&make_retry_request())
            .await
            .expect("stream should be established after retries");

        // Drain the stream; ensure at least one Delta with text "hi" is observed.
        let mut saw_delta = false;
        while let Some(item) = stream.next().await {
            let chunk = item.expect("no stream errors");
            if let medical_core::types::StreamChunk::Delta { text } = chunk
                && text == "hi"
            {
                saw_delta = true;
            }
        }
        assert!(saw_delta, "expected to see 'hi' delta");
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[test]
    fn maps_reasoning_delta_to_length_only() {
        let raw = r#"{"choices":[{"delta":{"reasoning_content":"abc"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        let chunks = map_chat_event(&resp);
        assert!(matches!(
            chunks[0],
            Ok(StreamChunk::ReasoningDelta { len: 3 })
        ));
    }

    #[test]
    fn maps_content_delta_and_usage() {
        let raw = r#"{"choices":[{"delta":{"content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#;
        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        let chunks = map_chat_event(&resp);
        assert!(matches!(&chunks[0], Ok(StreamChunk::Delta { text }) if text == "hi"));
        assert!(matches!(&chunks[1], Ok(StreamChunk::Usage(u)) if u.completion_tokens == 2));
        assert!(matches!(chunks[2], Ok(StreamChunk::Done)));
    }
}
