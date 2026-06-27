//! Private serde types for the OpenAI chat-completions wire protocol.
//!
//! These types model the JSON shapes exchanged with Ollama and LM Studio.
//! They are `pub(super)` — not part of the crate's public API. Core types
//! ([`CompletionRequest`], [`CompletionResponse`]) are converted to/from
//! these wire types by [`OpenAiCompatibleClient`].
//!
//! # Type inventory
//!
//! **Request types (serialized):**
//! - [`ChatRequest`] — top-level chat completion request body
//! - [`ChatMessage`] — a single message in the conversation
//! - [`StreamOptions`] — `include_usage: true` for streaming requests
//! - [`ApiTool`] / [`ApiToolDef`] — tool definitions sent to the model
//! - [`ApiToolCall`] / [`ApiFunction`] — tool call references in messages
//!
//! **Response types (deserialized):**
//! - [`ChatResponse`] — top-level chat completion response
//! - [`ChatChoice`] — a single completion choice (message or delta)
//! - [`ChatResponseMessage`] — full message in non-streaming responses
//! - [`ChatDelta`] — incremental delta in streaming responses
//! - [`ApiToolCallDelta`] / [`ApiFunctionDelta`] — partial tool call deltas
//! - [`ApiUsage`] — token usage statistics
//! - [`ModelsListResponse`] / [`ApiModelEntry`] — model enumeration response
//!
//! Many fields are deserialized for completeness even when never read on our
//! side (e.g. `ApiToolCallDelta::index`); the blanket `dead_code` allow keeps
//! that explicit-schema discipline without forcing per-field annotations.
//!
//! [`CompletionRequest`]: medical_core::types::CompletionRequest
//! [`CompletionResponse`]: medical_core::types::CompletionResponse
//! [`OpenAiCompatibleClient`]: super::OpenAiCompatibleClient

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(super) struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
pub(super) struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(super) struct ApiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ApiFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(super) struct ApiFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize, Clone)]
pub(super) struct ApiToolCallDelta {
    pub index: Option<usize>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub function: Option<ApiFunctionDelta>,
}

#[derive(Debug, Deserialize, Clone)]
pub(super) struct ApiFunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ApiTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ApiToolDef,
}

#[derive(Debug, Serialize)]
pub(super) struct ApiToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatResponse {
    pub model: Option<String>,
    pub choices: Vec<ChatChoice>,
    pub usage: Option<ApiUsage>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatChoice {
    pub message: Option<ChatResponseMessage>,
    pub delta: Option<ChatDelta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatResponseMessage {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatDelta {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ApiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct ModelsListResponse {
    pub data: Vec<ApiModelEntry>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiModelEntry {
    pub id: String,
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_non_streaming_response() {
        // A typical Ollama/LM Studio non-streaming chat completion response.
        let json = serde_json::json!({
            "model": "llama3:8b",
            "choices": [{
                "message": {
                    "content": "Hello, world!",
                    "tool_calls": null
                },
                "delta": null,
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 3,
                "total_tokens": 13
            }
        });
        let resp: ChatResponse = serde_json::from_value(json).expect("must deserialize");
        assert_eq!(resp.model.as_deref(), Some("llama3:8b"));
        assert_eq!(resp.choices.len(), 1);
        let msg = resp.choices[0].message.as_ref().expect("message present");
        assert_eq!(msg.content.as_deref(), Some("Hello, world!"));
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        let usage = resp.usage.expect("usage present");
        assert_eq!(usage.total_tokens, 13);
    }

    #[test]
    fn deserialize_streaming_delta() {
        // A single SSE delta chunk from a streaming response.
        let json = serde_json::json!({
            "model": "llama3:8b",
            "choices": [{
                "message": null,
                "delta": {
                    "content": "Hello",
                    "tool_calls": null
                },
                "finish_reason": null
            }],
            "usage": null
        });
        let resp: ChatResponse = serde_json::from_value(json).expect("must deserialize");
        let delta = resp.choices[0].delta.as_ref().expect("delta present");
        assert_eq!(delta.content.as_deref(), Some("Hello"));
        assert!(resp.usage.is_none());
    }

    #[test]
    fn deserialize_tool_call_delta() {
        // A streaming chunk with a partial tool call delta.
        let json = serde_json::json!({
            "model": "llama3:8b",
            "choices": [{
                "delta": {
                    "content": null,
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "search_icd_codes",
                            "arguments": "{\"query\":\"hype"
                        }
                    }]
                }
            }],
            "usage": null
        });
        let resp: ChatResponse = serde_json::from_value(json).expect("must deserialize");
        let delta = resp.choices[0].delta.as_ref().expect("delta present");
        let tc = delta.tool_calls.as_ref().expect("tool_calls present");
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id.as_deref(), Some("call_abc123"));
        let func = tc[0].function.as_ref().expect("function present");
        assert!(func.arguments.as_deref().unwrap().starts_with("{\"query"));
    }

    #[test]
    fn deserialize_empty_content_field() {
        // Some providers send content: null instead of omitting it.
        let json = serde_json::json!({
            "model": "llama3:8b",
            "choices": [{
                "message": { "content": null },
                "finish_reason": "stop"
            }],
            "usage": null
        });
        let resp: ChatResponse = serde_json::from_value(json).expect("must deserialize");
        assert!(resp.choices[0].message.as_ref().unwrap().content.is_none());
    }

    #[test]
    fn deserialize_models_list() {
        let json = serde_json::json!({
            "data": [
                { "id": "llama3:8b", "owned_by": "meta" },
                { "id": "qwen2.5:7b" }
            ]
        });
        let resp: ModelsListResponse = serde_json::from_value(json).expect("must deserialize");
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].id, "llama3:8b");
        assert_eq!(resp.data[0].owned_by.as_deref(), Some("meta"));
        assert!(resp.data[1].owned_by.is_none()); // #[serde(default)]
    }
}
