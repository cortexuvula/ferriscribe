//! Agent and tool types for the autonomous agent subsystem.

use serde::{Deserialize, Serialize};

use super::ai::{Message, UsageInfo};
use super::rag::RagResult;
use super::recording::Recording;

/// Definition of a tool that an agent can call.
///
/// Corresponds to the JSON schema sent to the AI model so it knows what
/// tools are available and what arguments they accept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Unique tool name (must match [`Tool::definition`](crate::traits::Tool::definition)).
    pub name: String,
    /// Human-readable description for the model's prompt.
    pub description: String,
    /// JSON Schema describing the tool's arguments.
    pub parameters: serde_json::Value,
}

/// The output of a tool invocation.
///
/// Produced by [`Tool::execute`](crate::traits::Tool::execute) and fed
/// back to the model as a [`MessageContent::ToolResult`](super::ai::MessageContent::ToolResult).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// The tool's output text.
    pub content: String,
    /// If `true`, the tool encountered an error and `content` describes it.
    pub is_error: bool,
}

impl ToolOutput {
    /// Construct a successful (non-error) output.
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// Construct an error output (`is_error = true`).
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// The runtime context passed to an agent when processing a request.
///
/// Assembled by the agent orchestrator from the current conversation,
/// patient data, RAG results, and optional recording. Passed to
/// [`Agent::execute`](crate::traits::Agent::execute).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContext {
    /// The user's most recent message.
    pub user_message: String,
    /// Prior conversation messages for context.
    pub conversation_history: Vec<Message>,
    /// Patient-specific grounding data (medications, conditions, etc.).
    pub patient_context: Option<PatientContext>,
    /// Relevant chunks retrieved from the RAG system.
    pub rag_context: Vec<RagResult>,
    /// The recording being discussed, if any.
    pub recording: Option<Recording>,
}

/// A snapshot of patient-specific context for grounding agent responses.
///
/// Frontend payloads from the SOAP generation flow may omit `patient_name`
/// and `prior_soap_notes` (those fields aren't surfaced in the UI today);
/// `#[serde(default)]` keeps deserialization forgiving.
///
/// # PHI note
///
/// These fields contain protected health information. Never log them
/// via `tracing::*` macros or `println!`. The manual `Debug` impl below
/// redacts every PHI field so accidental `{:?}` formatting cannot leak it.
#[derive(Clone, Serialize, Deserialize)]
pub struct PatientContext {
    /// Patient's name (optional — not always surfaced in the UI).
    #[serde(default)]
    pub patient_name: Option<String>,
    /// Prior SOAP note texts for longitudinal context.
    #[serde(default)]
    pub prior_soap_notes: Vec<String>,
    /// Current medications.
    #[serde(default)]
    pub medications: Vec<String>,
    /// Known medical conditions.
    #[serde(default)]
    pub conditions: Vec<String>,
    /// Known allergies.
    #[serde(default)]
    pub allergies: Vec<String>,
}

/// Manual Debug impl that redacts every PHI field. `patient_name` is
/// collapsed to a presence flag; the four list fields report only their
/// length so counts remain visible in diagnostics without leaking values.
impl std::fmt::Debug for PatientContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatientContext")
            .field(
                "patient_name",
                &self.patient_name.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "prior_soap_notes",
                &format!("<{} entries>", self.prior_soap_notes.len()),
            )
            .field(
                "medications",
                &format!("<{} items>", self.medications.len()),
            )
            .field("allergies", &format!("<{} items>", self.allergies.len()))
            .field("conditions", &format!("<{} items>", self.conditions.len()))
            .finish()
    }
}

/// The final response from an agent run.
///
/// Includes the text output, a record of all tool calls made during the
/// run, token usage, and the number of agent loop iterations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The agent's final text response.
    pub content: String,
    /// Record of every tool call made during the run.
    pub tool_calls_made: Vec<AgentToolCallRecord>,
    /// Cumulative token usage across all iterations.
    pub usage: UsageInfo,
    /// Number of agent loop iterations (including tool-call rounds).
    pub iterations: u32,
}

/// A record of a single tool invocation during an agent run.
///
/// Captured for debugging and audit trails. Stored alongside the agent
/// response so the full chain-of-thought is reconstructable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolCallRecord {
    /// The tool that was called.
    pub tool_name: String,
    /// The JSON arguments passed to the tool.
    pub arguments: serde_json::Value,
    /// The tool's output.
    pub result: ToolOutput,
    /// Wall-clock duration of the tool execution in milliseconds.
    pub duration_ms: u64,
}

/// Runtime settings for a specific agent.
///
/// Stored inside [`AppConfig::agent_settings`](super::settings::AppConfig::agent_settings)
/// keyed by agent name. Each agent can have its own model, temperature,
/// and system prompt override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    /// Whether this agent is enabled.
    pub enabled: bool,
    /// AI provider to use (e.g. `"lmstudio"`, `"ollama"`).
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Sampling temperature (lower = more deterministic).
    pub temperature: f32,
    /// Maximum tokens to generate per response.
    pub max_tokens: u32,
    /// Optional system prompt override (agent default if `None`).
    pub system_prompt: Option<String>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: "lmstudio".into(),
            model: String::new(),
            temperature: 0.2,
            max_tokens: 4096,
            system_prompt: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_success() {
        let out = ToolOutput::success("ok");
        assert_eq!(out.content, "ok");
        assert!(!out.is_error);
    }

    #[test]
    fn tool_output_error() {
        let out = ToolOutput::error("something went wrong");
        assert_eq!(out.content, "something went wrong");
        assert!(out.is_error);
    }

    #[test]
    fn agent_settings_defaults() {
        let settings = AgentSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.provider, "lmstudio");
        assert_eq!(settings.model, "");
        assert!((settings.temperature - 0.2).abs() < f32::EPSILON);
        assert_eq!(settings.max_tokens, 4096);
        assert!(settings.system_prompt.is_none());
    }

    #[test]
    fn tool_output_round_trip() {
        let out = ToolOutput::success("result data");
        let json = serde_json::to_string(&out).unwrap();
        let back: ToolOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, "result data");
        assert!(!back.is_error);
    }

    #[test]
    fn patient_context_deserializes_from_partial_payload() {
        // The frontend may send only the three structured fields. The two
        // unused fields (patient_name, prior_soap_notes) must default to
        // None / empty vec rather than erroring.
        let json = r#"{"medications":["A"],"conditions":["B"],"allergies":["C"]}"#;
        let parsed: PatientContext = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.medications, vec!["A"]);
        assert_eq!(parsed.conditions, vec!["B"]);
        assert_eq!(parsed.allergies, vec!["C"]);
        assert!(parsed.patient_name.is_none());
        assert!(parsed.prior_soap_notes.is_empty());
    }
}
