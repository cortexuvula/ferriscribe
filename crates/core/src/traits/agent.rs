//! Agent and tool traits for the autonomous agent subsystem.

use async_trait::async_trait;

use crate::error::AppResult;
use crate::types::{AgentContext, AgentResponse, ToolDef, ToolOutput};

/// An autonomous agent that can use tools to respond to user requests.
///
/// Implemented by the `agents` crate. The orchestrator manages the
/// agent loop (tool calling, context assembly) and delegates execution
/// to this trait.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Short identifier for this agent.
    fn name(&self) -> &str;

    /// Human-readable description of what this agent does.
    fn description(&self) -> &str;

    /// The system prompt used to prime this agent.
    fn system_prompt(&self) -> &str;

    /// The set of tools this agent is allowed to invoke.
    fn available_tools(&self) -> Vec<ToolDef>;

    /// Process the given context and return a response, potentially
    /// invoking tools in a loop before producing a final answer.
    ///
    /// # Default implementation
    ///
    /// Returns an error directing callers to use the orchestrator. All
    /// agents delegate execution to `AgentOrchestrator::execute`; the
    /// per-agent implementations were identical stubs, so the default
    /// here removes that duplication.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Agent`](crate::error::AppError::Agent) on
    /// orchestration failure, or the underlying provider error if a
    /// tool call or completion fails.
    async fn execute(&self, _context: AgentContext) -> AppResult<AgentResponse> {
        Err(crate::error::AppError::agent(
            "Use AgentOrchestrator::execute instead",
        ))
    }
}

/// A discrete capability that an agent can invoke.
///
/// Each tool has a JSON schema definition (sent to the model) and an
/// execute method that takes the model's JSON arguments and returns a
/// [`ToolOutput`].
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's schema definition.
    fn definition(&self) -> ToolDef;

    /// Execute the tool with the given JSON arguments.
    ///
    /// # Errors
    ///
    /// Returns a [`ToolOutput`] with `is_error = true` for expected
    /// failures, or an `AppResult::Err` for unexpected failures that
    /// should abort the agent loop.
    async fn execute(&self, arguments: serde_json::Value) -> AppResult<ToolOutput>;
}
