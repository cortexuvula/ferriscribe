//! AI completion provider trait.

use async_trait::async_trait;
use futures_core::Stream;

use crate::error::AppResult;
use crate::types::{
    CompletionRequest, CompletionResponse, ModelInfo, StreamChunk, ToolCompletionResponse, ToolDef,
};

/// Abstraction over any AI completion provider (Ollama, LM Studio).
///
/// Implemented by the `ai-providers` crate. Consumer crates (agents,
/// processing, translation) depend only on this trait for provider-
/// agnostic AI completion.
///
/// All methods return [`AppResult`] with [`AppError::AiProvider`](crate::error::AppError::AiProvider)
/// on failure.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// The canonical name of this provider (e.g. `"ollama"`, `"lmstudio"`).
    fn name(&self) -> &str;

    /// Returns the list of models this provider supports.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::AiProvider`](crate::error::AppError::AiProvider)
    /// if the model listing API call fails.
    async fn available_models(&self) -> AppResult<Vec<ModelInfo>>;

    /// Send a completion request and wait for the full response.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::AiProvider`](crate::error::AppError::AiProvider)
    /// on API failure, or [`AppError::EndpointOffline`](crate::error::AppError::EndpointOffline)
    /// if the provider's endpoint is unreachable.
    async fn complete(&self, request: CompletionRequest) -> AppResult<CompletionResponse>;

    /// Send a completion request and receive a stream of chunks.
    ///
    /// The returned stream yields [`StreamChunk`] items incrementally.
    /// The stream ends with [`StreamChunk::Done`].
    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>>;

    /// Send a completion request that may invoke tools.
    ///
    /// Returns the model's response along with any tool calls it
    /// requested. The agent orchestrator uses this to implement the
    /// tool-calling loop.
    async fn complete_with_tools(
        &self,
        request: CompletionRequest,
        tools: Vec<ToolDef>,
    ) -> AppResult<ToolCompletionResponse>;
}
