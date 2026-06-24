//! Agent orchestration engine.
//!
//! The [`AgentOrchestrator`] drives the multi-step reasoning loop: it builds
//! a message list from the [`AgentContext`], then iteratively calls the AI
//! provider with tool definitions, executes any tool calls the provider
//! requests, and feeds results back until the provider produces a final
//! text-only response (or the iteration limit is hit).

use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use medical_core::{
    error::{AppError, AppResult},
    traits::{Agent, AiProvider},
    types::{
        AgentContext, AgentResponse, AgentToolCallRecord, CompletionRequest, Message,
        MessageContent, Role, UsageInfo,
    },
};

use crate::tools::ToolRegistry;

/// Maximum number of tool-use iterations before aborting.
///
/// If the model keeps requesting tool calls without producing a final text
/// response after this many rounds, the orchestrator returns
/// [`AppError::Agent`] with a "max iterations" message.
const MAX_ITERATIONS: u32 = 10;

/// Drives an [`Agent`] through a reasoning + tool-use loop, delegating
/// AI completions to an [`AiProvider`] and tool execution to a [`ToolRegistry`].
///
/// # Lifecycle
///
/// 1. Construct with a [`ToolRegistry`] (typically [`ToolRegistry::with_defaults()`]).
/// 2. Call [`execute()`](Self::execute) for each user turn, passing the agent,
///    context, provider, model name, temperature, and a cancellation token.
/// 3. The orchestrator returns an [`AgentResponse`] containing the final text,
///    all tool-call records, cumulative token usage, and iteration count.
///
/// The orchestrator is stateless across calls — all per-turn state lives in
/// local variables inside `execute()`.
pub struct AgentOrchestrator {
    tool_registry: ToolRegistry,
}

impl AgentOrchestrator {
    /// Create a new orchestrator with the given tool registry.
    ///
    /// The registry determines which tools are available during execution.
    /// Use [`ToolRegistry::with_defaults()`] to get all five built-in tools,
    /// or [`ToolRegistry::new()`] for an empty registry you populate yourself.
    pub fn new(tool_registry: ToolRegistry) -> Self {
        Self { tool_registry }
    }

    /// Execute an agent run for the given context using the provided AI provider.
    ///
    /// Builds the message list from context, then iterates:
    /// 1. Call provider with tool definitions
    /// 2. If the provider requests tool calls, execute them and append results
    /// 3. If no tool calls remain, return the final response
    ///
    /// # Arguments
    ///
    /// * `agent` — the agent whose system prompt and tool set define this run
    /// * `context` — user message, conversation history, patient/RAG context
    /// * `provider` — the AI provider that handles completion with tool calling
    /// * `model` — model identifier forwarded on every `CompletionRequest`
    /// * `temperature` — sampling temperature forwarded on every request
    /// * `cancel` — token checked at the top of each iteration and after tool
    ///   execution; cancelling aborts the run with [`AppError::Cancelled`]
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Cancelled`] if the token is cancelled,
    /// [`AppError::Agent`] if `MAX_ITERATIONS` is exceeded, or the underlying
    /// provider error if a completion call fails.
    ///
    /// `model` is the model identifier to pass into every `CompletionRequest`.
    /// `temperature` is the sampling temperature forwarded on every request.
    /// Callers should source both from user settings for the active provider.
    pub async fn execute(
        &self,
        agent: &dyn Agent,
        context: AgentContext,
        provider: &dyn AiProvider,
        model: &str,
        temperature: f32,
        cancel: CancellationToken,
    ) -> AppResult<AgentResponse> {
        // Get only the tools that are both requested by the agent and present in the registry
        let agent_tool_defs = agent.available_tools();
        let available_tool_defs: Vec<_> = agent_tool_defs
            .iter()
            .filter(|def| self.tool_registry.get(&def.name).is_some())
            .cloned()
            .collect();

        // Build the initial message list
        let mut messages = build_messages(&context);

        let mut tool_calls_made: Vec<AgentToolCallRecord> = Vec::new();
        let mut total_usage = UsageInfo::default();
        let mut iterations: u32 = 0;

        loop {
            // Check for cancellation at the top of each iteration
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }

            if iterations >= MAX_ITERATIONS {
                warn!(
                    "Agent '{}' reached max iterations ({})",
                    agent.name(),
                    MAX_ITERATIONS
                );
                return Err(AppError::Agent(format!(
                    "max iterations ({}) reached without a final response",
                    MAX_ITERATIONS
                )));
            }

            iterations += 1;

            let request = CompletionRequest {
                model: model.to_string(),
                messages: messages.clone(),
                temperature: Some(temperature),
                max_tokens: Some(4096),
                system_prompt: Some(agent.system_prompt().to_string()),
            };

            debug!(
                "Agent '{}' iteration {} — calling provider with {} tool(s)",
                agent.name(),
                iterations,
                available_tool_defs.len()
            );

            let response = provider
                .complete_with_tools(request, available_tool_defs.clone())
                .await?;

            // Accumulate token usage
            total_usage.prompt_tokens += response.usage.prompt_tokens;
            total_usage.completion_tokens += response.usage.completion_tokens;
            total_usage.total_tokens += response.usage.total_tokens;

            if response.tool_calls.is_empty() {
                // No tool calls — this is the final response
                let content = response.content.unwrap_or_default();
                return Ok(AgentResponse {
                    content,
                    tool_calls_made,
                    usage: total_usage,
                    iterations,
                });
            }

            // Append the assistant message that contains the tool call requests.
            // The tool_calls field is required by OpenAI/Anthropic so that subsequent
            // tool-result messages can reference the tool_call_id.
            let assistant_content = response.content.clone().unwrap_or_default();
            messages.push(Message {
                role: Role::Assistant,
                content: MessageContent::Text(assistant_content),
                tool_calls: response.tool_calls.clone(),
            });

            // Execute each requested tool call
            for tool_call in &response.tool_calls {
                let start = Instant::now();

                let tool_result = match self.tool_registry.get(&tool_call.name) {
                    Some(tool) => tool
                        .execute(tool_call.arguments.clone())
                        .await
                        .unwrap_or_else(|e| medical_core::types::ToolOutput::error(e.to_string())),
                    None => medical_core::types::ToolOutput::error(format!(
                        "Tool '{}' not found in registry",
                        tool_call.name
                    )),
                };

                let duration_ms = start.elapsed().as_millis() as u64;

                debug!(
                    "Tool '{}' executed in {}ms, is_error={}",
                    tool_call.name, duration_ms, tool_result.is_error
                );

                // Record the tool call for the response
                tool_calls_made.push(AgentToolCallRecord {
                    tool_name: tool_call.name.clone(),
                    arguments: tool_call.arguments.clone(),
                    result: tool_result.clone(),
                    duration_ms,
                });

                // Append tool result as a tool message
                messages.push(Message {
                    role: Role::Tool,
                    content: MessageContent::ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        content: tool_result.content,
                    },
                    tool_calls: vec![],
                });
            }

            // Check cancellation again after tool execution
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
        }
    }
}

/// Build the initial message list from the agent context.
///
/// Assembles messages in this order:
/// 1. Conversation history (verbatim)
/// 2. Patient context as a `System` message (medications, conditions, allergies,
///    prior SOAP note count) — only if present
/// 3. RAG context as a `System` message with scored excerpts — only if non-empty
/// 4. The current user message as a `User` message
fn build_messages(context: &AgentContext) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();

    // Include conversation history
    messages.extend(context.conversation_history.clone());

    // Add patient context if present
    if let Some(patient) = &context.patient_context {
        let mut patient_text = String::from("Patient Context:\n");

        if let Some(name) = &patient.patient_name {
            patient_text.push_str(&format!("- Name: {}\n", name));
        }
        if !patient.medications.is_empty() {
            patient_text.push_str(&format!(
                "- Current medications: {}\n",
                patient.medications.join(", ")
            ));
        }
        if !patient.conditions.is_empty() {
            patient_text.push_str(&format!(
                "- Known conditions: {}\n",
                patient.conditions.join(", ")
            ));
        }
        if !patient.allergies.is_empty() {
            patient_text.push_str(&format!("- Allergies: {}\n", patient.allergies.join(", ")));
        }
        if !patient.prior_soap_notes.is_empty() {
            patient_text.push_str(&format!(
                "- Prior SOAP notes available: {}\n",
                patient.prior_soap_notes.len()
            ));
        }

        messages.push(Message {
            role: Role::System,
            content: MessageContent::Text(patient_text),
            tool_calls: vec![],
        });
    }

    // Add RAG context if present
    if !context.rag_context.is_empty() {
        let rag_text = context
            .rag_context
            .iter()
            .map(|r| format!("[Source: score={:.2}]\n{}", r.score, r.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        messages.push(Message {
            role: Role::System,
            content: MessageContent::Text(format!(
                "Relevant knowledge base excerpts:\n\n{}",
                rag_text
            )),
            tool_calls: vec![],
        });
    }

    // Add the current user message
    messages.push(Message {
        role: Role::User,
        content: MessageContent::Text(context.user_message.clone()),
        tool_calls: vec![],
    });

    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;

    #[test]
    fn orchestrator_creates() {
        let registry = ToolRegistry::with_defaults();
        let orchestrator = AgentOrchestrator::new(registry);
        // Verify the orchestrator was constructed without panicking
        // and holds the registry (indirectly via tool access)
        let _ = orchestrator;
    }

    #[test]
    fn orchestrator_default_registry() {
        let registry = ToolRegistry::default();
        let orchestrator = AgentOrchestrator::new(registry);
        let _ = orchestrator;
    }

    #[test]
    fn build_messages_empty_context() {
        let context = AgentContext {
            user_message: "What is the treatment for hypertension?".into(),
            conversation_history: vec![],
            patient_context: None,
            rag_context: vec![],
            recording: None,
        };
        let messages = build_messages(&context);
        // Should have exactly 1 message: the user message
        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, Role::User));
    }

    #[test]
    fn build_messages_with_patient_context() {
        use medical_core::types::PatientContext;

        let context = AgentContext {
            user_message: "Check drug interactions".into(),
            conversation_history: vec![],
            patient_context: Some(PatientContext {
                patient_name: Some("John Doe".into()),
                prior_soap_notes: vec![],
                medications: vec!["warfarin".into(), "aspirin".into()],
                conditions: vec!["atrial fibrillation".into()],
                allergies: vec!["penicillin".into()],
            }),
            rag_context: vec![],
            recording: None,
        };
        let messages = build_messages(&context);
        // Should have patient context system message + user message
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, Role::System));
        if let MessageContent::Text(ref text) = messages[0].content {
            assert!(text.contains("warfarin"));
            assert!(text.contains("atrial fibrillation"));
        } else {
            panic!("Expected text content");
        }
    }

    use async_trait::async_trait;
    use futures_core::Stream;
    use medical_core::error::AppResult;
    use medical_core::traits::AiProvider;
    use medical_core::types::{
        CompletionRequest, CompletionResponse, ModelInfo, StreamChunk, ToolCompletionResponse,
        ToolDef, UsageInfo,
    };
    use std::sync::Mutex;

    /// Test double that records every model name and temperature it sees.
    struct ModelCapturingProvider {
        captured_models: Mutex<Vec<String>>,
        captured_temperatures: Mutex<Vec<Option<f32>>>,
    }

    impl ModelCapturingProvider {
        fn new() -> Self {
            Self {
                captured_models: Mutex::new(Vec::new()),
                captured_temperatures: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AiProvider for ModelCapturingProvider {
        fn name(&self) -> &str {
            "capturing"
        }
        async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
            Ok(vec![])
        }
        async fn complete(&self, _req: CompletionRequest) -> AppResult<CompletionResponse> {
            unreachable!("orchestrator uses complete_with_tools")
        }
        async fn complete_stream(
            &self,
            _req: CompletionRequest,
        ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>> {
            unreachable!()
        }
        async fn complete_with_tools(
            &self,
            request: CompletionRequest,
            _tools: Vec<ToolDef>,
        ) -> AppResult<ToolCompletionResponse> {
            self.captured_models
                .lock()
                .unwrap()
                .push(request.model.clone());
            self.captured_temperatures
                .lock()
                .unwrap()
                .push(request.temperature);
            Ok(ToolCompletionResponse {
                content: Some("done".into()),
                tool_calls: vec![],
                usage: UsageInfo::default(),
            })
        }
    }

    #[tokio::test]
    async fn execute_forwards_caller_supplied_model() {
        use crate::agents::ChatAgent;
        use medical_core::types::AgentContext;
        use tokio_util::sync::CancellationToken;

        let registry = ToolRegistry::default();
        let orchestrator = AgentOrchestrator::new(registry);
        let provider = ModelCapturingProvider::new();
        let agent = ChatAgent;
        let context = AgentContext {
            user_message: "hi".into(),
            conversation_history: vec![],
            patient_context: None,
            rag_context: vec![],
            recording: None,
        };

        let _ = orchestrator
            .execute(
                &agent,
                context,
                &provider,
                "claude-sonnet-4-6",
                0.5,
                CancellationToken::new(),
            )
            .await
            .expect("run");

        let captured_models = provider.captured_models.lock().unwrap();
        assert_eq!(
            captured_models.as_slice(),
            &["claude-sonnet-4-6".to_string()],
            "orchestrator must pass the caller-supplied model, not a hardcoded default"
        );
        let captured_temps = provider.captured_temperatures.lock().unwrap();
        assert_eq!(
            captured_temps.as_slice(),
            &[Some(0.5)],
            "orchestrator must pass the caller-supplied temperature, not a hardcoded default"
        );
    }

    #[tokio::test]
    async fn execute_forwards_caller_supplied_temperature() {
        use crate::agents::ChatAgent;
        use medical_core::types::AgentContext;
        use tokio_util::sync::CancellationToken;

        let registry = ToolRegistry::default();
        let orchestrator = AgentOrchestrator::new(registry);
        let provider = ModelCapturingProvider::new();
        let agent = ChatAgent;
        let context = AgentContext {
            user_message: "hi".into(),
            conversation_history: vec![],
            patient_context: None,
            rag_context: vec![],
            recording: None,
        };

        let _ = orchestrator
            .execute(
                &agent,
                context,
                &provider,
                "some-model",
                0.9,
                CancellationToken::new(),
            )
            .await
            .expect("run");

        let captured_temps = provider.captured_temperatures.lock().unwrap();
        assert_eq!(
            captured_temps.as_slice(),
            &[Some(0.9)],
            "orchestrator must not hardcode temperature — it must forward the caller-supplied value"
        );
    }
}
