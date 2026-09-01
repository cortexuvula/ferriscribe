//! Ollama provider — wraps [`OpenAiCompatibleClient`] against a local Ollama server.
//!
//! # Default configuration
//!
//! - Base URL: `http://localhost:11434/v1`
//! - Fallback model: `llama3`
//! - Provider name: `"ollama"`
//!
//! All endpoint-resolution, caching, retry, and thinking-control machinery
//! lives in [`crate::local_openai`]; this module only supplies the
//! [`ProviderMeta`] identity. See that module's docs for the LAN/Tailscale
//! `RemoteEndpoint` probing behavior.
//!
//! [`OpenAiCompatibleClient`]: crate::openai_compat::OpenAiCompatibleClient

use async_trait::async_trait;
use futures_core::Stream;

use medical_core::{
    error::AppResult,
    traits::AiProvider,
    types::{
        CompletionRequest, CompletionResponse, ModelInfo, RemoteEndpoint, StreamChunk,
        ToolCompletionResponse, ToolDef,
    },
};

use crate::http_client::RetryConfig;
use crate::local_openai::{LocalOpenAiProvider, ProviderMeta, ThinkingControl};

/// Static identity for the Ollama provider. `thinking` uses the
/// `reasoning_effort: "none"` switch because Ollama's
/// OpenAI-compatible `/v1/chat/completions` endpoint honors it (its
/// native-API `think: false` does NOT work on `/v1`).
pub(crate) static META: ProviderMeta = ProviderMeta {
    id: "ollama",
    display: "Ollama",
    default_base: "http://localhost:11434",
    err_field: "ollama_host",
    fallback_model: "llama3",
    thinking: ThinkingControl::ReasoningEffortNone,
};

/// Ollama provider implementing the [`AiProvider`] trait.
///
/// Wraps an [`OpenAiCompatibleClient`] pointed at an Ollama server. Supports
/// optional [`RemoteEndpoint`] for LAN/Tailscale resolution, bearer-token
/// authentication, and configurable retry policy.
///
/// [`OpenAiCompatibleClient`]: crate::openai_compat::OpenAiCompatibleClient
/// [`AiProvider`]: medical_core::traits::AiProvider
pub struct OllamaProvider {
    inner: LocalOpenAiProvider,
}

impl OllamaProvider {
    /// Create a new Ollama provider.
    ///
    /// `host` defaults to `http://localhost:11434` when `None`.
    /// `bearer` is an optional bearer token for auth-proxied remote connections.
    /// `policy` controls retry behavior for inner HTTP calls.
    /// Returns `Err(AppError::AiProvider)` if the reqwest client can't be built.
    ///
    /// **`allow_public` is captured at construction, not stored.** If the user
    /// changes `allow_public_endpoint` in settings, the provider keeps using
    /// the policy it was built with until `reinit_providers` reconstructs it.
    /// The frontend must call `reinit_providers` after changing that setting.
    pub fn new(
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
    ) -> AppResult<Self> {
        Ok(Self {
            inner: LocalOpenAiProvider::new(&META, host, allow_public, bearer, policy)?,
        })
    }

    /// Create a new Ollama provider with a `RemoteEndpoint` pre-configured.
    ///
    /// Equivalent to `new(host, bearer, policy)` followed by `set_endpoint(ep)`,
    /// but usable in synchronous initialization code (no running async runtime
    /// required). Static `host` is kept as the fallback when `ep` is `None`.
    pub fn new_with_endpoint(
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
        ep: Option<RemoteEndpoint>,
    ) -> AppResult<Self> {
        Ok(Self {
            inner: LocalOpenAiProvider::new_with_endpoint(
                &META,
                host,
                allow_public,
                bearer,
                policy,
                ep,
            )?,
        })
    }

    /// Toggle the reasoning/"thinking" phase off for this provider.
    ///
    /// When disabled, `complete` and `complete_stream` force
    /// `reasoning_effort: "none"` on every request — the disable value
    /// Ollama's OpenAI-compatible `/v1/chat/completions` endpoint honors.
    /// Called from `init_ai_providers` with the user's
    /// `ollama_disable_thinking` setting; the frontend must call
    /// `reinit_providers` after changing that setting for it to take
    /// effect.
    pub fn set_thinking_disabled(&self, disabled: bool) {
        self.inner.set_thinking_disabled(disabled);
    }

    /// Override the remote endpoint used for LAN/Tailscale resolution. See
    /// [`LocalOpenAiProvider::set_endpoint`] for the bearer-propagation and
    /// cache-invalidation contract.
    pub async fn set_endpoint(
        &self,
        ep: Option<RemoteEndpoint>,
        allow_public: bool,
    ) -> AppResult<()> {
        self.inner.set_endpoint(ep, allow_public).await
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
        self.inner.available_models().await
    }

    async fn complete(&self, request: CompletionRequest) -> AppResult<CompletionResponse> {
        self.inner.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>> {
        self.inner.complete_stream(request).await
    }

    async fn complete_with_tools(
        &self,
        request: CompletionRequest,
        tools: Vec<ToolDef>,
    ) -> AppResult<ToolCompletionResponse> {
        self.inner.complete_with_tools(request, tools).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_ollama_identity() {
        let p = OllamaProvider::new(None, false, None, RetryConfig::default())
            .expect("build default provider");
        assert_eq!(p.name(), "ollama");
        assert_eq!(META.id, "ollama");
        assert_eq!(META.default_base, "http://localhost:11434");
        assert_eq!(META.err_field, "ollama_host");
        assert_eq!(META.fallback_model, "llama3");
        assert_eq!(META.thinking, ThinkingControl::ReasoningEffortNone);
    }
}
