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
