//! # medical-agents
//!
//! Agentic orchestrator with tool use for clinical AI chat sessions.
//!
//! This crate drives a multi-step reasoning loop: an AI provider receives a
//! user message plus tool definitions, decides which tools to invoke, gets
//! back structured results, and synthesises a final response. Eight
//! specialised agents share five built-in tools (ICD lookup, drug
//! interactions, vitals extraction, RAG knowledge-base search, and clinical
//! checklists).
//!
//! # Architecture
//!
//! - [`orchestrator::AgentOrchestrator`] — the central loop that iterates
//!   between the AI provider and tool execution until a final answer is
//!   produced or the iteration limit is reached.
//! - [`agents`] — eight [`Agent`](medical_core::traits::Agent)
//!   implementations, each with its own system prompt and tool set. Use
//!   [`agents::all_agents()`] to get them all.
//! - [`tools`] — [`ToolRegistry`](tools::ToolRegistry) and five
//!   [`Tool`](medical_core::traits::Tool) implementations.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use medical_agents::orchestrator::AgentOrchestrator;
//! use medical_agents::agents::ChatAgent;
//! use medical_agents::tools::ToolRegistry;
//!
//! let registry = ToolRegistry::with_defaults();
//! let orchestrator = AgentOrchestrator::new(registry);
//! // orchestrator.execute(&ChatAgent, context, &provider, model, temp, cancel).await
//! ```

pub mod agents;
pub mod orchestrator;
pub mod tools;

use thiserror::Error;

/// Errors that can occur during agent execution.
///
/// Most agent-level errors are surfaced as
/// [`AppError::Agent`](medical_core::error::AppError::Agent) from the
/// orchestrator. This enum is reserved for errors that originate within the
/// agents crate itself (e.g. a tool's own `execute` returning an error
/// variant).
#[derive(Debug, Error)]
pub enum AgentError {
    /// A general execution failure during agent processing.
    #[error("agent execution error: {0}")]
    Execution(String),

    /// A tool returned an error during execution.
    #[error("tool error: {0}")]
    Tool(String),

    /// The orchestrator hit the maximum iteration limit without producing
    /// a final response.
    #[error("max iterations reached: {0}")]
    MaxIterations(u32),

    /// The agent run was cancelled via [`CancellationToken`](tokio_util::sync::CancellationToken).
    #[error("agent cancelled")]
    Cancelled,

    /// The underlying AI provider returned an error.
    #[error("provider error: {0}")]
    Provider(String),
}

/// Convenience result type for agent operations.
pub type AgentResult<T> = Result<T, AgentError>;
