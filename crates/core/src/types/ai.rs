//! AI completion types — requests, responses, messages, and streaming.

use serde::{Deserialize, Serialize};

/// Metadata about an available AI model.
///
/// Returned by [`AiProvider::available_models`](crate::traits::AiProvider::available_models).
/// The `supports_tools` and `supports_streaming` flags let the agent
/// orchestrator and UI decide which code path to use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Provider-specific model identifier (e.g. `"llama3:8b"`).
    pub id: String,
    /// Human-readable model name for display.
    pub name: String,
    /// Canonical provider name (e.g. `"ollama"`, `"lmstudio"`).
    pub provider: String,
    /// Maximum context window in tokens.
    pub max_tokens: u32,
    /// Whether this model supports function/tool calling.
    pub supports_tools: bool,
    /// Whether this model supports streaming responses.
    pub supports_streaming: bool,
}

/// A request to generate a chat completion.
///
/// Passed to [`AiProvider::complete`](crate::traits::AiProvider::complete)
/// and related methods. The `system_prompt` field, if present, is
/// prepended as a system-role message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Model identifier to use (must match a [`ModelInfo::id`]).
    pub model: String,
    /// Conversation messages in order.
    pub messages: Vec<Message>,
    /// Sampling temperature override (provider default if `None`).
    pub temperature: Option<f32>,
    /// Maximum tokens to generate (provider default if `None`).
    pub max_tokens: Option<u32>,
    /// System prompt prepended to the conversation.
    pub system_prompt: Option<String>,
}

/// A single message in a conversation.
///
/// The `tool_calls` field is populated only on assistant messages that
/// request tool invocations. Tool-result messages use
/// [`MessageContent::ToolResult`] as their content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// The role of the message author.
    pub role: Role,
    /// The message body — either plain text or a tool result.
    pub content: MessageContent,
    /// Tool calls requested by the model (assistant messages only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

/// The role of a message author.
///
/// Serialized as `snake_case` strings to match the OpenAI-compatible API
/// format used by both Ollama and LM Studio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// System instructions that prime the model.
    System,
    /// A message from the user.
    User,
    /// A response from the model.
    Assistant,
    /// The result of a tool invocation.
    Tool,
}

/// A single content part in a multipart (vision) message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlData },
}

/// The URL wrapper inside an image content part.
/// Carries a data URL like `"data:image/png;base64,iVBORw0K..."`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageUrlData {
    pub url: String,
}

/// The body of a message — either plain text or a tool result.
///
/// Uses `#[serde(untagged)]` so that text messages serialize as bare
/// strings (matching the OpenAI chat format), while tool results
/// serialize as objects with `tool_call_id` and `content`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain text content.
    Text(String),
    /// The result of a tool invocation, linked by `tool_call_id`.
    ToolResult {
        /// The ID of the [`ToolCall`] this result responds to.
        tool_call_id: String,
        /// The tool's output as a string.
        content: String,
    },
    /// Multipart content for vision models (OpenAI format).
    /// Serialized as a JSON array of `{type: "text"|"image_url", ...}` parts.
    Parts(Vec<ContentPart>),
}

/// A complete response from the AI provider.
///
/// Returned by [`AiProvider::complete`](crate::traits::AiProvider::complete).
/// If the model requested tool calls, they appear in `tool_calls` and
/// `content` may be empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// The model's text output (may be empty if tool calls were made).
    pub content: String,
    /// The model that produced the response.
    pub model: String,
    /// Token usage statistics.
    pub usage: UsageInfo,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCall>,
}

/// A tool invocation requested by the model.
///
/// When the model decides to call a tool, it emits a `ToolCall` in its
/// response. The agent orchestrator executes the tool and feeds the
/// result back as a [`MessageContent::ToolResult`] message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID linking this call to its result.
    pub id: String,
    /// The tool name (must match a [`ToolDef::name`](crate::types::agent::ToolDef::name)).
    pub name: String,
    /// JSON arguments for the tool.
    pub arguments: serde_json::Value,
}

/// Token usage statistics for a completion.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageInfo {
    /// Tokens consumed by the prompt (input).
    pub prompt_tokens: u32,
    /// Tokens consumed by the generated completion (output).
    pub completion_tokens: u32,
    /// Sum of prompt and completion tokens.
    pub total_tokens: u32,
}

/// A chunk of a streaming completion response.
///
/// Used by [`AiProvider::complete_stream`](crate::traits::AiProvider::complete_stream).
/// Tagged with `type` for JSON serialization so the frontend can
/// dispatch on chunk kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamChunk {
    /// An incremental text delta from the model.
    Delta {
        /// The text fragment to append.
        text: String,
    },
    /// A reasoning/"thinking" delta, reduced to its byte length. The
    /// reasoning text itself never crosses the provider boundary — lengths
    /// and counts are safe to log/emit (AGENTS.md), content is not.
    ReasoningDelta {
        /// Byte length of the reasoning delta text.
        len: usize,
    },
    /// An incremental tool-call argument delta.
    ToolCallDelta {
        /// The tool call ID being built.
        id: String,
        /// Tool name (present only in the first delta for a call).
        name: Option<String>,
        /// JSON argument fragment to append.
        arguments_delta: String,
    },
    /// Final usage statistics (sent just before `Done`).
    Usage(UsageInfo),
    /// Stream has ended.
    Done,
}

/// Response from a completion that may include tool calls.
///
/// Returned by [`AiProvider::complete_with_tools`](crate::traits::AiProvider::complete_with_tools).
/// Either `content` or `tool_calls` (or both) may be present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCompletionResponse {
    /// The model's text output, if any.
    pub content: Option<String>,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCall>,
    /// Token usage statistics.
    pub usage: UsageInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_content_text_serializes() {
        let content = MessageContent::Text("hello".into());
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, serde_json::json!("hello"));
    }

    #[test]
    fn message_content_tool_result_serializes() {
        let content = MessageContent::ToolResult {
            tool_call_id: "call_1".into(),
            content: "result text".into(),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["tool_call_id"], "call_1");
        assert_eq!(json["content"], "result text");
    }

    #[test]
    fn stream_chunk_tagged_serialization() {
        let delta = StreamChunk::Delta { text: "Hi".into() };
        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["type"], "delta");
        assert_eq!(json["text"], "Hi");

        let done = StreamChunk::Done;
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["type"], "done");
    }

    #[test]
    fn role_serializes_snake_case() {
        let role = Role::Assistant;
        let json = serde_json::to_value(&role).unwrap();
        assert_eq!(json, "assistant");

        let system: Role = serde_json::from_str("\"system\"").unwrap();
        assert_eq!(system, Role::System);
    }

    #[test]
    fn completion_response_round_trip() {
        let resp = CompletionResponse {
            content: "Hello".into(),
            model: "gpt-4o".into(),
            usage: UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            tool_calls: vec![],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: CompletionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, "Hello");
        assert_eq!(back.usage.total_tokens, 15);
    }

    #[test]
    fn parts_with_image_serializes_to_multipart_array() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "Extract all text".to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrlData {
                        url: "data:image/png;base64,iVBOR=".to_string(),
                    },
                },
            ]),
            tool_calls: vec![],
        };
        let json_val = serde_json::to_value(&msg).expect("serialize");
        // content should be a JSON array (multipart), not a bare string
        let content = json_val.get("content").expect("content field");
        assert!(
            content.is_array(),
            "Parts should serialize as array: {content}"
        );
        let arr = content.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "Extract all text");
        assert_eq!(arr[1]["type"], "image_url");
        assert_eq!(arr[1]["image_url"]["url"], "data:image/png;base64,iVBOR=");
    }

    #[test]
    fn text_variant_still_serializes_as_string() {
        // Regression: the existing Text variant must still produce a bare JSON
        // string, not be broken by adding Parts.
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text("hello".to_string()),
            tool_calls: vec![],
        };
        let json_val = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(json_val["content"], serde_json::json!("hello"));
    }
}
